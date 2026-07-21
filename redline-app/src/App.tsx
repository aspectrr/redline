import { createSignal, createMemo, createEffect, onMount, onCleanup, on, For, Show, type JSX } from "solid-js";
import { api, parseDiff } from "./lib/api";
import type { Draft, DraftWithRevisions, Lesson, Pair, SearchResult, Pattern, Violation, Feedback, DiffAnalysis } from "./lib/api";
import "./App.css";

type View = "drafts" | "library" | "search" | "lessons" | "patterns" | "feedback";
type RightTab = "diff" | "revisions" | "lessons" | "lint";

export default function App() {
  const [view, setView] = createSignal<View>("drafts");

  // drafts
  const [drafts, setDrafts] = createSignal<Draft[]>([]);
  const [showFinalized, setShowFinalized] = createSignal(false);
  const [selectedId, setSelectedId] = createSignal<number | null>(null);
  const [current, setCurrent] = createSignal<DraftWithRevisions | null>(null);
  const [editorText, setEditorText] = createSignal("");
  const [dirty, setDirty] = createSignal(false);
  const [saving, setSaving] = createSignal(false);
  let saveTimer: ReturnType<typeof setTimeout> | null = null;
  const [ctx, setCtx] = createSignal("");
  const [tagsStr, setTagsStr] = createSignal("");
  const [rightTab, setRightTab] = createSignal<RightTab>("diff");
  const [error, setError] = createSignal<string | null>(null);

  // library
  const [pairs, setPairs] = createSignal<Pair[]>([]);
  const [selectedPair, setSelectedPair] = createSignal<Pair | null>(null);
  const [lessons, setLessons] = createSignal<Lesson[]>([]);

  // search
  const [q, setQ] = createSignal("");
  const [results, setResults] = createSignal<SearchResult | null>(null);

  // lint + patterns + feedback
  const [violations, setViolations] = createSignal<Violation[]>([]);
  const [linting, setLinting] = createSignal(false);
  const [patterns, setPatterns] = createSignal<Pattern[]>([]);
  const [feedback, setFeedback] = createSignal<Feedback[]>([]);

  const flushError = (e: unknown) => setError(e instanceof Error ? e.message : String(e));

  const refreshDrafts = async () => {
    try {
      setDrafts(await api.listDrafts(showFinalized()));
    } catch (e) { flushError(e); }
  };

  const loadDraft = async (id: number) => {
    try {
      const d = await api.getDraft(id);
      setCurrent(d);
      if (d) {
        setEditorText(d.revisions[d.revisions.length - 1]?.content ?? "");
        setCtx(d.draft.context ?? "");
        setTagsStr(d.draft.tags.join(", "));
        setDirty(false);
        setRightTab("diff");
      }
    } catch (e) { flushError(e); }
  };

  // View-scoped reloads. createEffect(on(view)) runs on mount (view="drafts")
  // and on every tab switch — covers the original mount + per-view effects.
  createEffect(on(view, (v) => {
    if (v === "drafts") refreshDrafts();
    if (v === "library" || v === "lessons") reloadLibrary();
    if (v === "patterns") reloadPatterns();
    if (v === "feedback") reloadFeedback();
  }));

  // Agents push drafts via CLI/MCP while the user is in another window. Refresh
  // on window focus (switch back to the app).
  onMount(() => {
    const refresh = () => refreshDrafts();
    window.addEventListener("focus", refresh);
    onCleanup(() => window.removeEventListener("focus", refresh));
  });

  // Backend emits `db-changed` when an external process (CLI/MCP) writes to
  // the shared DB. Replaces the 5s poll with push-based refresh.
  onMount(() => {
    let unlisten: (() => void) | undefined;
    (async () => {
      const { listen } = await import("@tauri-apps/api/event");
      unlisten = await listen("db-changed", () => refreshDrafts());
    })();
    onCleanup(() => unlisten?.());
  });

  // autosave flush: write pending edit synchronously, returns when done.
  const flushSave = async () => {
    if (saveTimer) { clearTimeout(saveTimer); saveTimer = null; }
    if (!dirty() || selectedId() == null || !current()) return;
    const id = selectedId()!;
    const last = current()!.revisions[current()!.revisions.length - 1]?.content;
    if (editorText() === last) { setDirty(false); return; }
    setSaving(true);
    try {
      await api.saveRevision(id, editorText(), "user");
      setCurrent(await api.getDraft(id));
      setDirty(false);
    } catch (e) { flushError(e); }
    finally { setSaving(false); }
  };

  const onEditorInput = (val: string) => {
    setEditorText(val);
    setDirty(true);
    if (saveTimer) clearTimeout(saveTimer);
    saveTimer = setTimeout(async () => {
      if (selectedId() == null) return;
      const id = selectedId()!;
      const last = current()?.revisions[current()!.revisions.length - 1]?.content;
      if (val === last) { setDirty(false); return; }
      setSaving(true);
      try {
        await api.saveRevision(id, val, "user");
        setCurrent(await api.getDraft(id));
        setDirty(false);
        refreshDrafts();
      } catch (e) { flushError(e); }
      finally { setSaving(false); }
    }, 1200);
  };

  const selectDraft = async (id: number) => {
    if (dirty()) await flushSave();
    setSelectedId(id);
    await loadDraft(id);
  };

  const newDraft = async () => {
    try {
      const id = await api.createDraft("", "", [], "user");
      await refreshDrafts();
      setSelectedId(id);
      await loadDraft(id);
    } catch (e) { flushError(e); }
  };

  const saveMeta = async () => {
    if (selectedId() == null) return;
    const id = selectedId()!;
    try {
      const tags = tagsStr().split(",").map(t => t.trim()).filter(Boolean);
      await api.updateDraftMeta(id, ctx().trim() || null, tags);
      await loadDraft(id);
      refreshDrafts();
    } catch (e) { flushError(e); }
  };

  const restore = async (revisionId: number) => {
    if (selectedId() == null) return;
    const id = selectedId()!;
    try {
      await api.restoreRevision(id, revisionId);
      await loadDraft(id);
      refreshDrafts();
    } catch (e) { flushError(e); }
  };

  const finalize = async () => {
    if (selectedId() == null) return;
    const id = selectedId()!;
    try {
      await flushSave();
      await api.finalizeDraft(id);
      await loadDraft(id);
      await refreshDrafts();
      setRightTab("lessons");
    } catch (e) { flushError(e); }
  };

  const deleteDraft = async () => {
    if (selectedId() == null) return;
    const id = selectedId()!;
    try {
      await api.deleteDraft(id);
      setSelectedId(null);
      setCurrent(null);
      setEditorText("");
      setCtx("");
      setTagsStr("");
      setRightTab("diff");
      await refreshDrafts();
    } catch (e) { flushError(e); }
  };

  const deletePair = async (id: number) => {
    try {
      await api.deletePair(id);
      setSelectedPair(null);
      await reloadLibrary();
    } catch (e) { flushError(e); }
  };

  const deleteLesson = async (id: number) => {
    try {
      await api.deleteLesson(id);
      await reloadLibrary();
    } catch (e) { flushError(e); }
  };

  const reloadLibrary = async () => {
    try {
      setPairs(await api.listPairs(200));
      setLessons(await api.listLessons());
    } catch (e) { flushError(e); }
  };

  const runSearch = async () => {
    try { setResults(await api.search(q())); } catch (e) { flushError(e); }
  };

  const runLint = async (content: string) => {
    setLinting(true);
    try { setViolations(await api.lintDraft(content)); } catch (e) { flushError(e); }
    finally { setLinting(false); }
  };

  const reloadPatterns = async () => {
    try { setPatterns(await api.listPatterns()); } catch (e) { flushError(e); }
  };

  const reloadFeedback = async () => {
    try { setFeedback(await api.listFeedback()); } catch (e) { flushError(e); }
  };

  const openPair = async (id: number) => {
    try { setSelectedPair(await api.showPair(id)); } catch (e) { flushError(e); }
  };

  return (
    <div class="app">
      <header class="topbar">
        <div class="brand">✉️ Redline</div>
        <nav class="tabs">
          <button class={view() === "drafts" ? "active" : ""} onClick={() => setView("drafts")}>Drafts</button>
          <button class={view() === "library" ? "active" : ""} onClick={() => setView("library")}>Library</button>
          <button class={view() === "search" ? "active" : ""} onClick={() => setView("search")}>Search</button>
          <button class={view() === "lessons" ? "active" : ""} onClick={() => setView("lessons")}>Lessons</button>
          <button class={view() === "patterns" ? "active" : ""} onClick={() => setView("patterns")}>Patterns</button>
          <button class={view() === "feedback" ? "active" : ""} onClick={() => setView("feedback")}>Feedback</button>
        </nav>
        {view() === "search" && (
          <div class="search-inline">
            <input value={q()} onInput={e => setQ(e.currentTarget.value)}
              onKeyDown={e => { if (e.key === "Enter") runSearch(); }}
              placeholder="search drafts, pairs, lessons…" />
            <button onClick={runSearch}>Search</button>
          </div>
        )}
      </header>

      {error() && <div class="error" onClick={() => setError(null)}>⚠ {error()}</div>}

      {view() === "drafts" && (
        <div class="drafts-layout">
          <aside class="list-pane">
            <div class="list-head">
              <button class="primary" onClick={newDraft}>+ New draft</button>
              <label class="toggle">
                <input type="checkbox" checked={showFinalized()}
                  onChange={e => setShowFinalized(e.currentTarget.checked)} />
                show finalized
              </label>
            </div>
            <ul class="draft-list">
              <For each={drafts()}>{(d) => (
                <li class={selectedId() === d.id ? "selected" : ""} onClick={() => selectDraft(d.id)}>
                  <div class="row1">
                    <span class={"status " + d.status}>{d.status === "finalized" ? "✓" : "✎"}</span>
                    <span class="ctx">{d.context || "(no context)"}</span>
                  </div>
                  <div class="row2">
                    <span>#{d.id}</span>
                    <span class="tags">{d.tags.join(", ")}</span>
                    <span class="when">{shortWhen(d.updated_at)}</span>
                  </div>
                </li>
              )}</For>
              {drafts().length === 0 && <li class="empty">No drafts. The agent can push one via <code>redline draft</code>, or click + New draft.</li>}
            </ul>
          </aside>

          <section class="editor-pane">
            {current() ? (
              <>
                <div class="meta-row">
                  <input class="ctx-input" value={ctx()} onInput={e => setCtx(e.currentTarget.value)}
                    placeholder="context (topic + recipient type)" />
                  <input class="tags-input" value={tagsStr()} onInput={e => setTagsStr(e.currentTarget.value)}
                    placeholder="tags: pitch, external" />
                  <button onClick={saveMeta}>Save meta</button>
                  {current()!.draft.status === "finalized" ? (
                    <button class="primary" disabled>Finalized ✓</button>
                  ) : (
                    <ConfirmButton label="Finalize →" confirmLabel="Confirm finalize" onConfirm={finalize} />
                  )}
                  <ConfirmButton label="Delete" confirmLabel="Confirm delete" danger onConfirm={deleteDraft} />
                </div>
                <div class="editor-status">
                  <span>draft #{current()!.draft.id}</span>
                  <span>{current()!.revisions.length} revision(s)</span>
                  <span>{saving() ? "saving…" : dirty() ? "unsaved" : "saved"}</span>
                </div>
                <textarea class="editor" value={editorText()}
                  onInput={e => onEditorInput(e.currentTarget.value)}
                  disabled={current()!.draft.status === "finalized"}
                  placeholder="Write the email…" spellcheck={false} />
              </>
            ) : (
              <div class="empty-editor">Select a draft, or start a new one.</div>
            )}
          </section>

          <aside class="right-pane">
            <div class="right-tabs">
              <button class={rightTab() === "diff" ? "active" : ""} onClick={() => setRightTab("diff")}>Diff</button>
              <button class={rightTab() === "revisions" ? "active" : ""} onClick={() => setRightTab("revisions")}>Revisions</button>
              <button class={rightTab() === "lint" ? "active" : ""} onClick={() => { setRightTab("lint"); if (current()) runLint(editorText()); }}>Lint</button>
              <button class={rightTab() === "lessons" ? "active" : ""} onClick={() => setRightTab("lessons")}>Lessons</button>
            </div>
            <div class="right-body">
              {rightTab() === "diff" && current() && (
                <DiffPane diff={current()!.working_diff}
                  oldText={current()!.revisions[0]?.content}
                  newText={current()!.revisions[current()!.revisions.length - 1]?.content} />
              )}
              {rightTab() === "lint" && current() && (
                <LintPane violations={violations()} linting={linting()} onRelint={() => runLint(editorText())} />
              )}
              {rightTab() === "revisions" && current() && (
                <RevisionsPane revisions={current()!.revisions} onRestore={restore} />
              )}
              {rightTab() === "lessons" && current() && (
                <LessonsPane lessons={lessons()}
                  pairId={current()!.draft.finalized_pair_id}
                  onChanged={reloadLibrary}
                  onDeleteLesson={deleteLesson} />
              )}
            </div>
          </aside>
        </div>
      )}

      {view() === "library" && (
        <div class="library-layout">
          <aside class="list-pane">
            <div class="list-head"><strong>Pairs ({pairs().length})</strong></div>
            <ul class="draft-list">
              <For each={pairs()}>{(p) => (
                <li class={selectedPair()?.id === p.id ? "selected" : ""} onClick={() => openPair(p.id)}>
                  <div class="row1"><span class="ctx">{p.context || "(no context)"}</span></div>
                  <div class="row2"><span>#{p.id}</span><span class="tags">{p.tags.join(", ")}</span><span class="when">{shortWhen(p.created_at)}</span></div>
                </li>
              )}</For>
              {pairs().length === 0 && <li class="empty">No pairs yet. Finalize a draft to create one.</li>}
            </ul>
          </aside>
          <section class="editor-pane pair-detail">
            {selectedPair() ? <PairDetail pair={selectedPair()!} onDelete={deletePair} /> : <div class="empty-editor">Select a pair.</div>}
          </section>
          <aside class="right-pane">
            <div class="right-tabs"><button class="active">Lessons</button></div>
            <div class="right-body">
              <LessonsPane lessons={lessons()} pairId={selectedPair()?.id ?? null} onChanged={reloadLibrary} onDeleteLesson={deleteLesson} />
            </div>
          </aside>
        </div>
      )}

      {view() === "lessons" && (
        <div class="search-layout">
          <section>
            <h3>All lessons ({lessons().length})</h3>
            <ul class="lesson-list">
              <For each={lessons()}>{(l) => (
                <li>
                  <div class="lesson-row">
                    <div class="lesson-text">
                      <div>{l.lesson}</div>
                      <div class="lesson-meta">L{l.id} · pair #{l.pair_id ?? "—"} · [{l.tags.join(", ")}]</div>
                    </div>
                    <ConfirmButton label="Delete" confirmLabel="Confirm" danger onConfirm={() => deleteLesson(l.id)} />
                  </div>
                </li>
              )}</For>
              {lessons().length === 0 && <li class="empty">No lessons yet. Derive one from a pair's diff.</li>}
            </ul>
          </section>
        </div>
      )}

      {view() === "search" && (
        <div class="search-layout">
          {results() == null ? (
            <div class="empty-editor">Search across every draft, revision, pair, and lesson. Start with the box above.</div>
          ) : (
            <>
              <section>
                <h3>Drafts ({results()!.drafts.length})</h3>
                <ul class="result-list"><For each={results()!.drafts}>{(d) => <li><b>#{d.id}</b> {d.context || "(no context)"} — {d.tags.join(", ")}</li>}</For></ul>
              </section>
              <section>
                <h3>Pairs ({results()!.pairs.length})</h3>
                <ul class="result-list"><For each={results()!.pairs}>{(p) => <li><b>#{p.id}</b> {p.context || ""} — {p.tags.join(", ")}</li>}</For></ul>
              </section>
              <section>
                <h3>Lessons ({results()!.lessons.length})</h3>
                <ul class="result-list"><For each={results()!.lessons}>{(l) => <li>{l.lesson} <i>[{l.tags.join(", ")}]</i></li>}</For></ul>
              </section>
            </>
          )}
        </div>
      )}

      {view() === "patterns" && (
        <PatternsView patterns={patterns()} onChanged={reloadPatterns} />
      )}

      {view() === "feedback" && (
        <FeedbackView feedback={feedback()} />
      )}
    </div>
  );
}

function DiffPane(props: { diff: string; oldText?: string; newText?: string }) {
  const [mode, setMode] = createSignal<"lines" | "sentences" | "split">("lines");
  const [sentenceDiff, setSentenceDiff] = createSignal<string>("");

  createEffect(on([() => mode(), () => props.oldText, () => props.newText], () => {
    if (mode() === "sentences" && props.oldText != null && props.newText != null) {
      api.computeDiff(props.oldText, props.newText, "sentences").then(setSentenceDiff).catch(() => setSentenceDiff(""));
    }
  }));

  const rows = createMemo(() => parseDiff(mode() === "sentences" ? sentenceDiff() : props.diff));
  const hasChanges = createMemo(() => rows().some(r => r.kind !== "equal"));

  const modes = (
    <div class="diff-mode">
      <button class={mode() === "lines" ? "active" : ""} onClick={() => setMode("lines")}>lines</button>
      <button class={mode() === "sentences" ? "active" : ""} onClick={() => setMode("sentences")}>sentences</button>
      <button class={mode() === "split" ? "active" : ""} onClick={() => setMode("split")}>side-by-side</button>
    </div>
  );

  return (
    <div class="diff-pane">
      {modes}
      {!hasChanges() ? (
        <div class="empty small">No changes yet — the editor matches the original draft.</div>
      ) : mode() === "split" ? (
        <div class="diff-split">
          <div class="diff-split-col">
            <div class="diff-split-head del">Removed</div>
            <pre class="diff">
              <For each={rows().filter(r => r.kind === "removed")}>{(row) => <DiffLine row={row} />}</For>
            </pre>
          </div>
          <div class="diff-split-col">
            <div class="diff-split-head add">Added</div>
            <pre class="diff">
              <For each={rows().filter(r => r.kind === "added")}>{(row) => <DiffLine row={row} />}</For>
            </pre>
          </div>
        </div>
      ) : (
        <pre class="diff">
          <For each={rows()}>{(row) => <DiffLine row={row} />}</For>
        </pre>
      )}
    </div>
  );
}

function DiffLine(props: { row: import("./lib/api").DiffRow }) {
  const cls = () => props.row.kind === "added" ? "add" : props.row.kind === "removed" ? "del" : "ctx";
  const sign = () => props.row.kind === "added" ? "+" : props.row.kind === "removed" ? "-" : " ";
  return (
    <div class={"diff-line " + cls()}>
      <span class="sign">{sign()}</span>
      <For each={props.row.segments}>{(s) =>
        s.tag === "ctx" ? <span class="seg">{s.text}</span> : <span class={"seg hl " + s.tag}>{s.text}</span>
      }</For>
    </div>
  );
}

function ConfirmButton(props: { label: string; confirmLabel: string; onConfirm: () => void; danger?: boolean }) {
  const [armed, setArmed] = createSignal(false);
  let timer: ReturnType<typeof setTimeout> | null = null;
  onCleanup(() => { if (timer) clearTimeout(timer); });
  const disarm = () => {
    setArmed(false);
    if (timer) { clearTimeout(timer); timer = null; }
  };
  return (
    <>
      {!armed() ? (
        <button class={props.danger ? "danger" : "primary"} onClick={() => {
          setArmed(true);
          timer = setTimeout(() => setArmed(false), 4000);
        }}>{props.label}</button>
      ) : (
        <span class="confirm-group">
          <button onClick={disarm}>Cancel</button>
          <button class={props.danger ? "danger" : "primary"} onClick={() => { disarm(); props.onConfirm(); }}>{props.confirmLabel}</button>
        </span>
      )}
    </>
  );
}

function RevisionsPane(props: { revisions: DraftWithRevisions["revisions"]; onRestore: (id: number) => void }) {
  const reversed = createMemo(() => [...props.revisions].reverse());
  return (
    <ul class="rev-list">
      <For each={reversed()}>{(r, i) => (
        <li class={i() === 0 ? "latest" : ""}>
          <div class="rev-head">
            <span class={"rev-src " + r.source}>{r.source}</span>
            <span class="when">{shortWhen(r.created_at)}</span>
            <span>rev #{r.id}</span>
            {i() !== 0 && <button class="mini" onClick={() => props.onRestore(r.id)}>restore</button>}
          </div>
          <pre class="rev-preview">{truncate(r.content, 160)}</pre>
        </li>
      )}</For>
    </ul>
  );
}

function LessonsPane(props: {
  lessons: Lesson[]; pairId: number | null; onChanged: () => void; onDeleteLesson: (id: number) => void;
}) {
  const [text, setText] = createSignal("");
  const [tagsStr, setTagsStr] = createSignal("");
  // pair-scoped: only lessons derived from this pair, not the whole corpus.
  // Use the All Lessons tab to see every lesson.
  const shown = createMemo(() => props.pairId == null ? [] : props.lessons.filter(l => l.pair_id === props.pairId));
  const add = async () => {
    if (props.pairId == null || !text().trim()) return;
    const tags = tagsStr().split(",").map(t => t.trim()).filter(Boolean);
    await api.addLesson(props.pairId, text().trim(), tags);
    setText(""); setTagsStr("");
    props.onChanged();
  };
  return (
    <div class="lessons">
      <ul class="lesson-list">
        <For each={shown()}>{(l) => (
          <li>
            <div class="lesson-row">
              <div class="lesson-text">
                <div>{l.lesson}</div>
                <div class="lesson-meta">L{l.id} · pair #{l.pair_id ?? "—"} · [{l.tags.join(", ")}]</div>
              </div>
              <ConfirmButton label="Delete" confirmLabel="Confirm" danger onConfirm={() => props.onDeleteLesson(l.id)} />
            </div>
          </li>
        )}</For>
        {shown().length === 0 && props.pairId != null && <li class="empty small">No lessons for this pair yet. Derive one from the diff and add it here.</li>}
      </ul>
      <div class="add-lesson">
        <textarea value={text()} onInput={e => setText(e.currentTarget.value)} placeholder="a specific lesson (e.g. ‘use quick note, not I wanted to reach out’)" rows={2} />
        <div class="add-row">
          <input value={tagsStr()} onInput={e => setTagsStr(e.currentTarget.value)} placeholder="tags: pitch, external" />
          <button class="primary" onClick={add} disabled={props.pairId == null || !text().trim()}>Add lesson</button>
        </div>
        {props.pairId == null && <div class="hint">Finalize a draft to attach a lesson to its pair.</div>}
      </div>
    </div>
  );
}

function PairDetail(props: { pair: Pair; onDelete: (id: number) => void }) {
  return (
    <div class="pair-detail-inner">
      <div class="meta-row">
        <h3>Pair #{props.pair.id} {props.pair.context && <span class="muted">— {props.pair.context}</span>}</h3>
        <ConfirmButton label="Delete" confirmLabel="Confirm delete" danger onConfirm={() => props.onDelete(props.pair.id)} />
      </div>
      <div class="pair-tags">{props.pair.tags.join(", ")}</div>
      <div class="pair-cols">
        <div><h4>Draft</h4><pre>{props.pair.draft}</pre></div>
        <div><h4>Final</h4><pre>{props.pair.final}</pre></div>
      </div>
      <DeletionsSection pairId={props.pair.id} />
      <h4>Diff</h4>
      <DiffPane diff={props.pair.diff} oldText={props.pair.draft} newText={props.pair.final} />
    </div>
  );
}

function shortWhen(iso: string): string {
  const d = new Date(iso);
  if (isNaN(d.getTime())) return iso;
  const now = new Date();
  const sameDay = d.toDateString() === now.toDateString();
  return sameDay ? d.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" })
    : d.toLocaleDateString([], { month: "short", day: "numeric" });
}

function truncate(s: string, n: number): string {
  s = s.replace(/\s+/g, " ").trim();
  return s.length > n ? s.slice(0, n) + "…" : s;
}

// Surfaces deletions and categorized changes above the raw diff so the user
// sees the voice signal first, not the line-level noise.
function DeletionsSection(props: { pairId: number }) {
  const [analysis, setAnalysis] = createSignal<DiffAnalysis | null>(null);

  createEffect(on(() => props.pairId, (pairId) => {
    api.analyzePair(pairId).then(setAnalysis).catch(() => setAnalysis(null));
  }));

  return (
    <Show when={analysis()} keyed>
      {(a) => {
        const deletions = a.deletions;
        const categorized = a.categorized || [];
        const noSignal = deletions.length === 0 && categorized.length === 0;
        if (noSignal) return <></>;
        return (
          <div class="analysis-section">
            {deletions.length > 0 && (
              <>
                <h4 class="signal-head">✂ Deletions ({deletions.length})</h4>
                <ul class="signal-list">
                  <For each={deletions}>{(d) => <li class="signal-item del">{d}</li>}</For>
                </ul>
              </>
            )}
            {categorized.length > 0 && (
              <>
                <h4 class="signal-head">Categorized changes</h4>
                <ul class="cat-list">
                  <For each={categorized}>{(c) => (
                    <li class={"cat-item " + c.category}>
                      <span class={"cat-badge " + c.category}>{c.category}</span>
                      <span class="cat-desc">{c.description}</span>
                    </li>
                  )}</For>
                </ul>
              </>
            )}
          </div>
        );
      }}
    </Show>
  );
}

// --- lint panel (drafts right-pane) ---

function LintPane(props: { violations: Violation[]; linting: boolean; onRelint: () => void }) {
  return (
    <div class="lint-pane">
      <div class="lint-head">
        <strong>{props.violations.length} violation(s)</strong>
        <button class="mini" onClick={props.onRelint} disabled={props.linting}>
          {props.linting ? "linting…" : "re-lint"}
        </button>
      </div>
      {props.violations.length === 0 ? (
        <div class="empty small">
          {props.linting ? "Checking…" : "No violations. Draft matches all stored patterns."}
        </div>
      ) : (
        <ul class="violation-list">
          <For each={props.violations}>{(v) => (
            <li class={"violation " + v.category}>
              <div class="v-rule">⚠ {v.rule}</div>
              {v.matched_text && <div class="v-match">"{v.matched_text}" — line {v.line}</div>}
              <div class="v-ctx">{v.context}</div>
              <div class="v-meta">[{v.category}] {v.direction}</div>
            </li>
          )}</For>
        </ul>
      )}
    </div>
  );
}

// --- patterns management view ---

function PatternsView(props: { patterns: Pattern[]; onChanged: () => void }) {
  const [rule, setRule] = createSignal("");
  const [pattern, setPattern] = createSignal("");
  const [patternType, setPatternType] = createSignal("literal");
  const [direction, setDirection] = createSignal("avoid");
  const [category, setCategory] = createSignal("style");

  const add = async () => {
    if (!rule().trim() || !pattern().trim()) return;
    await api.addPattern(rule().trim(), pattern().trim(), patternType(), direction(), category(), null, null, null);
    setRule(""); setPattern("");
    props.onChanged();
  };

  const del = async (id: number) => {
    await api.deletePattern(id);
    props.onChanged();
  };

  return (
    <div class="search-layout">
      <section class="pattern-list-section">
        <h3>Voice patterns ({props.patterns.length})</h3>
        <p class="hint">Patterns are matchable rules the lint engine checks drafts against.
        Literal matches are case-insensitive substring searches. Regex uses Rust regex syntax.</p>
        <ul class="pattern-list">
          <For each={props.patterns}>{(p) => (
            <li>
              <div class="pattern-row">
                <div class="pattern-text">
                  <div><strong>{p.rule}</strong></div>
                  <div class="pattern-meta">
                    <code>{p.pattern}</code> · {p.pattern_type} · {p.direction} · [{p.category}] · {p.confidence}
                  </div>
                  {p.before_text && p.after_text && (
                    <div class="pattern-ex">
                      <span class="del">{p.before_text}</span> → <span class="add">{p.after_text}</span>
                    </div>
                  )}
                </div>
                <ConfirmButton label="Delete" confirmLabel="Confirm" danger onConfirm={() => del(p.id)} />
              </div>
            </li>
          )}</For>
          {props.patterns.length === 0 && <li class="empty">No patterns yet. Add one below — it will immediately lint future drafts.</li>}
        </ul>
      </section>
      <section class="add-pattern-section">
        <h3>Add pattern</h3>
        <input value={rule()} onInput={e => setRule(e.currentTarget.value)} placeholder="Rule: e.g. No em-dashes in client emails" />
        <input value={pattern()} onInput={e => setPattern(e.currentTarget.value)} placeholder="Pattern: e.g. — or \b—\b" />
        <div class="pattern-opts">
          <select value={patternType()} onChange={e => setPatternType(e.currentTarget.value)}>
            <option value="literal">literal</option>
            <option value="regex">regex</option>
          </select>
          <select value={direction()} onChange={e => setDirection(e.currentTarget.value)}>
            <option value="avoid">avoid</option>
            <option value="prefer">prefer</option>
          </select>
          <select value={category()} onChange={e => setCategory(e.currentTarget.value)}>
            <option value="style">style</option>
            <option value="punctuation">punctuation</option>
            <option value="structure">structure</option>
            <option value="factual">factual</option>
            <option value="deletion">deletion</option>
          </select>
          <button class="primary" onClick={add} disabled={!rule().trim() || !pattern().trim()}>Add</button>
        </div>
      </section>
    </div>
  );
}

// --- feedback view ---

function FeedbackView(props: { feedback: Feedback[] }): JSX.Element {
  return (
    <div class="search-layout">
      <section>
        <h3>Feedback ({props.feedback.length})</h3>
        <p class="hint">Agents log feedback via the <code>give_feedback</code> MCP tool or <code>redline feedback</code> CLI command.</p>
        {props.feedback.length === 0 ? (
          <div class="empty">No feedback yet.</div>
        ) : (
          <ul class="feedback-list">
            <For each={props.feedback}>{(f) => (
              <li class={"feedback-item " + f.severity}>
                <div class="fb-head">
                  <span class={"sev " + f.severity}>{f.severity}</span>
                  {f.tool_name && <span class="fb-tool">{f.tool_name}</span>}
                  {f.rating != null && <span class="fb-rating">{"★".repeat(f.rating)}{"☆".repeat(5 - f.rating)}</span>}
                  <span class="when">{shortWhen(f.created_at)}</span>
                </div>
                <div class="fb-msg">{f.message}</div>
                {f.agent_id && <div class="fb-agent">from: {f.agent_id}</div>}
              </li>
            )}</For>
          </ul>
        )}
      </section>
    </div>
  );
}
