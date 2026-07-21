import { useCallback, useEffect, useRef, useState } from "react";
import { api, parseDiff } from "./lib/api";
import type { Draft, DraftWithRevisions, DiffRow, Lesson, Pair, SearchResult, Pattern, Violation, Feedback, DiffAnalysis } from "./lib/api";
import "./App.css";

type View = "drafts" | "library" | "search" | "lessons" | "patterns" | "feedback";
type RightTab = "diff" | "revisions" | "lessons" | "lint";

export default function App() {
  const [view, setView] = useState<View>("drafts");

  // drafts
  const [drafts, setDrafts] = useState<Draft[]>([]);
  const [showFinalized, setShowFinalized] = useState(false);
  const [selectedId, setSelectedId] = useState<number | null>(null);
  const [current, setCurrent] = useState<DraftWithRevisions | null>(null);
  const [editorText, setEditorText] = useState("");
  const [dirty, setDirty] = useState(false);
  const [saving, setSaving] = useState(false);
  const saveTimer = useRef<number | null>(null);
  const [ctx, setCtx] = useState("");
  const [tagsStr, setTagsStr] = useState("");
  const [rightTab, setRightTab] = useState<RightTab>("diff");
  const [error, setError] = useState<string | null>(null);

  // library
  const [pairs, setPairs] = useState<Pair[]>([]);
  const [selectedPair, setSelectedPair] = useState<Pair | null>(null);
  const [lessons, setLessons] = useState<Lesson[]>([]);

  // search
  const [q, setQ] = useState("");
  const [results, setResults] = useState<SearchResult | null>(null);

  // lint + patterns + feedback
  const [violations, setViolations] = useState<Violation[]>([]);
  const [linting, setLinting] = useState(false);
  const [patterns, setPatterns] = useState<Pattern[]>([]);
  const [feedback, setFeedback] = useState<Feedback[]>([]);

  const flushError = (e: unknown) => setError(e instanceof Error ? e.message : String(e));

  const refreshDrafts = useCallback(async () => {
    try {
      setDrafts(await api.listDrafts(showFinalized));
    } catch (e) { flushError(e); }
  }, [showFinalized]);

  const loadDraft = useCallback(async (id: number) => {
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
  }, []);

  useEffect(() => { refreshDrafts(); }, [refreshDrafts]);

  // Agents push drafts via CLI/MCP while the user is in another window. The only
  // built-in refresh triggers are mount and the showFinalized toggle — so refresh
  // on window focus (switch back to the app) and when switching to the Drafts tab.
  useEffect(() => {
    const refresh = () => refreshDrafts();
    window.addEventListener("focus", refresh);
    return () => window.removeEventListener("focus", refresh);
  }, [refreshDrafts]);
  useEffect(() => { if (view === "drafts") refreshDrafts(); }, [view, refreshDrafts]);

  // Backend emits `db-changed` when an external process (CLI/MCP) writes to
  // the shared DB. Replaces the 5s poll with push-based refresh.
  useEffect(() => {
    let unlisten: (() => void) | undefined;
    let active = true;
    (async () => {
      const { listen } = await import("@tauri-apps/api/event");
      unlisten = await listen("db-changed", () => { if (active) refreshDrafts(); });
    })();
    return () => { active = false; unlisten?.(); };
  }, [refreshDrafts]);

  // autosave flush: write pending edit synchronously, returns when done.
  const flushSave = useCallback(async () => {
    if (saveTimer.current) { window.clearTimeout(saveTimer.current); saveTimer.current = null; }
    if (!dirty || selectedId == null || !current) return;
    const last = current.revisions[current.revisions.length - 1]?.content;
    if (editorText === last) { setDirty(false); return; }
    setSaving(true);
    try {
      await api.saveRevision(selectedId, editorText, "user");
      const d = await api.getDraft(selectedId);
      setCurrent(d);
      setDirty(false);
    } catch (e) { flushError(e); }
    finally { setSaving(false); }
  }, [dirty, editorText, selectedId, current]);

  const onEditorChange = (val: string) => {
    setEditorText(val);
    setDirty(true);
    if (saveTimer.current) window.clearTimeout(saveTimer.current);
    saveTimer.current = window.setTimeout(async () => {
      if (selectedId == null) return;
      const last = current?.revisions[current.revisions.length - 1]?.content;
      if (val === last) { setDirty(false); return; }
      setSaving(true);
      try {
        await api.saveRevision(selectedId, val, "user");
        const d = await api.getDraft(selectedId);
        setCurrent(d);
        setDirty(false);
        refreshDrafts();
      } catch (e) { flushError(e); }
      finally { setSaving(false); }
    }, 1200);
  };

  const selectDraft = async (id: number) => {
    if (dirty) await flushSave();
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
    if (selectedId == null) return;
    try {
      const tags = tagsStr.split(",").map(t => t.trim()).filter(Boolean);
      await api.updateDraftMeta(selectedId, ctx.trim() || null, tags);
      await loadDraft(selectedId);
      refreshDrafts();
    } catch (e) { flushError(e); }
  };

  const restore = async (revisionId: number) => {
    if (selectedId == null) return;
    try {
      await api.restoreRevision(selectedId, revisionId);
      await loadDraft(selectedId);
      refreshDrafts();
    } catch (e) { flushError(e); }
  };

  const finalize = async () => {
    if (selectedId == null) return;
    try {
      await flushSave();
      await api.finalizeDraft(selectedId);
      await loadDraft(selectedId);
      await refreshDrafts();
      setRightTab("lessons");
    } catch (e) { flushError(e); }
  };

  const deleteDraft = async () => {
    if (selectedId == null) return;
    const id = selectedId;
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

  const reloadLibrary = useCallback(async () => {
    try {
      setPairs(await api.listPairs(200));
      setLessons(await api.listLessons());
    } catch (e) { flushError(e); }
  }, []);

  useEffect(() => { if (view === "library" || view === "lessons") reloadLibrary(); }, [view, reloadLibrary]);

  const runSearch = async () => {
    try { setResults(await api.search(q)); } catch (e) { flushError(e); }
  };

  const runLint = async (content: string) => {
    setLinting(true);
    try { setViolations(await api.lintDraft(content)); } catch (e) { flushError(e); }
    finally { setLinting(false); }
  };

  const reloadPatterns = useCallback(async () => {
    try { setPatterns(await api.listPatterns()); } catch (e) { flushError(e); }
  }, []);

  const reloadFeedback = useCallback(async () => {
    try { setFeedback(await api.listFeedback()); } catch (e) { flushError(e); }
  }, []);

  useEffect(() => { if (view === "patterns") reloadPatterns(); }, [view, reloadPatterns]);
  useEffect(() => { if (view === "feedback") reloadFeedback(); }, [view, reloadFeedback]);

  const openPair = async (id: number) => {
    try { setSelectedPair(await api.showPair(id)); } catch (e) { flushError(e); }
  };

  return (
    <div className="app">
      <header className="topbar">
        <div className="brand">✉️ Redline</div>
        <nav className="tabs">
          <button className={view === "drafts" ? "active" : ""} onClick={() => setView("drafts")}>Drafts</button>
          <button className={view === "library" ? "active" : ""} onClick={() => setView("library")}>Library</button>
          <button className={view === "search" ? "active" : ""} onClick={() => setView("search")}>Search</button>
          <button className={view === "lessons" ? "active" : ""} onClick={() => setView("lessons")}>Lessons</button>
          <button className={view === "patterns" ? "active" : ""} onClick={() => setView("patterns")}>Patterns</button>
          <button className={view === "feedback" ? "active" : ""} onClick={() => setView("feedback")}>Feedback</button>
        </nav>
        {view === "search" && (
          <div className="search-inline">
            <input value={q} onChange={e => setQ(e.target.value)}
              onKeyDown={e => { if (e.key === "Enter") runSearch(); }}
              placeholder="search drafts, pairs, lessons…" />
            <button onClick={runSearch}>Search</button>
          </div>
        )}
      </header>

      {error && <div className="error" onClick={() => setError(null)}>⚠ {error}</div>}

      {view === "drafts" && (
        <div className="drafts-layout">
          <aside className="list-pane">
            <div className="list-head">
              <button className="primary" onClick={newDraft}>+ New draft</button>
              <label className="toggle">
                <input type="checkbox" checked={showFinalized}
                  onChange={e => { setShowFinalized(e.target.checked); }} />
                show finalized
              </label>
            </div>
            <ul className="draft-list">
              {drafts.map(d => (
                <li key={d.id} className={selectedId === d.id ? "selected" : ""} onClick={() => selectDraft(d.id)}>
                  <div className="row1">
                    <span className={"status " + d.status}>{d.status === "finalized" ? "✓" : "✎"}</span>
                    <span className="ctx">{d.context || "(no context)"}</span>
                  </div>
                  <div className="row2">
                    <span>#{d.id}</span>
                    <span className="tags">{d.tags.join(", ")}</span>
                    <span className="when">{shortWhen(d.updated_at)}</span>
                  </div>
                </li>
              ))}
              {drafts.length === 0 && <li className="empty">No drafts. The agent can push one via <code>redline draft</code>, or click + New draft.</li>}
            </ul>
          </aside>

          <section className="editor-pane">
            {current ? (
              <>
                <div className="meta-row">
                  <input className="ctx-input" value={ctx} onChange={e => setCtx(e.target.value)}
                    placeholder="context (topic + recipient type)" />
                  <input className="tags-input" value={tagsStr} onChange={e => setTagsStr(e.target.value)}
                    placeholder="tags: pitch, external" />
                  <button onClick={saveMeta}>Save meta</button>
                  {current.draft.status === "finalized" ? (
                    <button className="primary" disabled>Finalized ✓</button>
                  ) : (
                    <ConfirmButton label="Finalize →" confirmLabel="Confirm finalize" onConfirm={finalize} />
                  )}
                  <ConfirmButton label="Delete" confirmLabel="Confirm delete" danger onConfirm={deleteDraft} />
                </div>
                <div className="editor-status">
                  <span>draft #{current.draft.id}</span>
                  <span>{current.revisions.length} revision(s)</span>
                  <span>{saving ? "saving…" : dirty ? "unsaved" : "saved"}</span>
                </div>
                <textarea className="editor" value={editorText}
                  onChange={e => onEditorChange(e.target.value)}
                  disabled={current.draft.status === "finalized"}
                  placeholder="Write the email…" spellCheck={false} />
              </>
            ) : (
              <div className="empty-editor">Select a draft, or start a new one.</div>
            )}
          </section>

          <aside className="right-pane">
            <div className="right-tabs">
              <button className={rightTab === "diff" ? "active" : ""} onClick={() => setRightTab("diff")}>Diff</button>
              <button className={rightTab === "revisions" ? "active" : ""} onClick={() => setRightTab("revisions")}>Revisions</button>
              <button className={rightTab === "lint" ? "active" : ""} onClick={() => { setRightTab("lint"); if (current) runLint(editorText); }}>Lint</button>
              <button className={rightTab === "lessons" ? "active" : ""} onClick={() => setRightTab("lessons")}>Lessons</button>
            </div>
            <div className="right-body">
              {rightTab === "diff" && current && (
                <DiffPane diff={current.working_diff}
                  oldText={current.revisions[0]?.content}
                  newText={current.revisions[current.revisions.length - 1]?.content} />
              )}
              {rightTab === "lint" && current && (
                <LintPane violations={violations} linting={linting} onRelint={() => runLint(editorText)} />
              )}
              {rightTab === "revisions" && current && (
                <RevisionsPane revisions={current.revisions} onRestore={restore} />
              )}
              {rightTab === "lessons" && current && (
                <LessonsPane lessons={lessons}
                  pairId={current.draft.finalized_pair_id}
                  onChanged={reloadLibrary}
                  onDeleteLesson={deleteLesson} />
              )}
            </div>
          </aside>
        </div>
      )}

      {view === "library" && (
        <div className="library-layout">
          <aside className="list-pane">
            <div className="list-head"><strong>Pairs ({pairs.length})</strong></div>
            <ul className="draft-list">
              {pairs.map(p => (
                <li key={p.id} className={selectedPair?.id === p.id ? "selected" : ""} onClick={() => openPair(p.id)}>
                  <div className="row1"><span className="ctx">{p.context || "(no context)"}</span></div>
                  <div className="row2"><span>#{p.id}</span><span className="tags">{p.tags.join(", ")}</span><span className="when">{shortWhen(p.created_at)}</span></div>
                </li>
              ))}
              {pairs.length === 0 && <li className="empty">No pairs yet. Finalize a draft to create one.</li>}
            </ul>
          </aside>
          <section className="editor-pane pair-detail">
            {selectedPair ? <PairDetail pair={selectedPair} onDelete={deletePair} /> : <div className="empty-editor">Select a pair.</div>}
          </section>
          <aside className="right-pane">
            <div className="right-tabs"><button className="active">Lessons</button></div>
            <div className="right-body">
              <LessonsPane lessons={lessons} pairId={selectedPair?.id ?? null} onChanged={reloadLibrary} onDeleteLesson={deleteLesson} />
            </div>
          </aside>
        </div>
      )}

      {view === "lessons" && (
        <div className="search-layout">
          <section>
            <h3>All lessons ({lessons.length})</h3>
            <ul className="lesson-list">
              {lessons.map(l => (
                <li key={l.id}>
                  <div className="lesson-row">
                    <div className="lesson-text">
                      <div>{l.lesson}</div>
                      <div className="lesson-meta">L{l.id} · pair #{l.pair_id ?? "—"} · [{l.tags.join(", ")}]</div>
                    </div>
                    <ConfirmButton label="Delete" confirmLabel="Confirm" danger onConfirm={() => deleteLesson(l.id)} />
                  </div>
                </li>
              ))}
              {lessons.length === 0 && <li className="empty">No lessons yet. Derive one from a pair's diff.</li>}
            </ul>
          </section>
        </div>
      )}

      {view === "search" && (
        <div className="search-layout">
          {results == null ? (
            <div className="empty-editor">Search across every draft, revision, pair, and lesson. Start with the box above.</div>
          ) : (
            <>
              <section>
                <h3>Drafts ({results.drafts.length})</h3>
                <ul className="result-list">{results.drafts.map(d => <li key={d.id}><b>#{d.id}</b> {d.context || "(no context)"} — {d.tags.join(", ")}</li>)}</ul>
              </section>
              <section>
                <h3>Pairs ({results.pairs.length})</h3>
                <ul className="result-list">{results.pairs.map(p => <li key={p.id}><b>#{p.id}</b> {p.context || ""} — {p.tags.join(", ")}</li>)}</ul>
              </section>
              <section>
                <h3>Lessons ({results.lessons.length})</h3>
                <ul className="result-list">{results.lessons.map(l => <li key={l.id}>{l.lesson} <i>[{l.tags.join(", ")}]</i></li>)}</ul>
              </section>
            </>
          )}
        </div>
      )}

      {view === "patterns" && (
        <PatternsView patterns={patterns} onChanged={reloadPatterns} />
      )}

      {view === "feedback" && (
        <FeedbackView feedback={feedback} />
      )}
    </div>
  );
}

function DiffPane({ diff, oldText, newText }: { diff: string; oldText?: string; newText?: string }) {
  const [mode, setMode] = useState<"lines" | "sentences" | "split">("lines");
  const [sentenceDiff, setSentenceDiff] = useState<string>("");

  useEffect(() => {
    if (mode === "sentences" && oldText != null && newText != null) {
      api.computeDiff(oldText, newText, "sentences").then(setSentenceDiff).catch(() => setSentenceDiff(""));
    }
  }, [mode, oldText, newText]);

  const activeDiff = mode === "sentences" ? sentenceDiff : diff;
  const rows = parseDiff(activeDiff);
  const hasChanges = rows.some(r => r.kind !== "equal");

  const modes = (
    <div className="diff-mode">
      <button className={mode === "lines" ? "active" : ""} onClick={() => setMode("lines")}>lines</button>
      <button className={mode === "sentences" ? "active" : ""} onClick={() => setMode("sentences")}>sentences</button>
      <button className={mode === "split" ? "active" : ""} onClick={() => setMode("split")}>side-by-side</button>
    </div>
  );

  if (!hasChanges) {
    return (
      <div className="diff-pane">
        {modes}
        <div className="empty small">No changes yet — the editor matches the original draft.</div>
      </div>
    );
  }

  if (mode === "split") {
    return (
      <div className="diff-pane">
        {modes}
        <div className="diff-split">
          <div className="diff-split-col">
            <div className="diff-split-head del">Removed</div>
            <pre className="diff">
              {rows.map((row, i) => row.kind === "removed" ? <DiffLine key={i} row={row} /> : null)}
            </pre>
          </div>
          <div className="diff-split-col">
            <div className="diff-split-head add">Added</div>
            <pre className="diff">
              {rows.map((row, i) => row.kind === "added" ? <DiffLine key={i} row={row} /> : null)}
            </pre>
          </div>
        </div>
      </div>
    );
  }

  return (
    <div className="diff-pane">
      {modes}
      <pre className="diff">
        {rows.map((row, i) => <DiffLine key={i} row={row} />)}
      </pre>
    </div>
  );
}

function DiffLine({ row }: { row: DiffRow }) {
  const cls = row.kind === "added" ? "add" : row.kind === "removed" ? "del" : "ctx";
  const sign = row.kind === "added" ? "+" : row.kind === "removed" ? "-" : " ";
  return (
    <div className={"diff-line " + cls}>
      <span className="sign">{sign}</span>
      {row.segments.map((s, i) =>
        s.tag === "ctx"
          ? <span key={i} className="seg">{s.text}</span>
          : <span key={i} className={"seg hl " + s.tag}>{s.text}</span>
      )}
    </div>
  );
}

function ConfirmButton({ label, confirmLabel, onConfirm, danger }: {
  label: string;
  confirmLabel: string;
  onConfirm: () => void;
  danger?: boolean;
}) {
  const [armed, setArmed] = useState(false);
  const timer = useRef<number | null>(null);
  useEffect(() => () => { if (timer.current) window.clearTimeout(timer.current); }, []);
  const disarm = () => {
    setArmed(false);
    if (timer.current) { window.clearTimeout(timer.current); timer.current = null; }
  };
  if (!armed) {
    return (
      <button className={danger ? "danger" : "primary"} onClick={() => {
        setArmed(true);
        timer.current = window.setTimeout(() => setArmed(false), 4000);
      }}>{label}</button>
    );
  }
  return (
    <span className="confirm-group">
      <button onClick={disarm}>Cancel</button>
      <button className={danger ? "danger" : "primary"} onClick={() => { disarm(); onConfirm(); }}>{confirmLabel}</button>
    </span>
  );
}

function RevisionsPane({ revisions, onRestore }: {
  revisions: DraftWithRevisions["revisions"]; onRestore: (id: number) => void;
}) {
  return (
    <ul className="rev-list">
      {revisions.slice().reverse().map((r, i) => (
        <li key={r.id} className={i === 0 ? "latest" : ""}>
          <div className="rev-head">
            <span className={"rev-src " + r.source}>{r.source}</span>
            <span className="when">{shortWhen(r.created_at)}</span>
            <span>rev #{r.id}</span>
            {i !== 0 && <button className="mini" onClick={() => onRestore(r.id)}>restore</button>}
          </div>
          <pre className="rev-preview">{truncate(r.content, 160)}</pre>
        </li>
      ))}
    </ul>
  );
}

function LessonsPane({ lessons, pairId, onChanged, onDeleteLesson }: {
  lessons: Lesson[]; pairId: number | null; onChanged: () => void; onDeleteLesson: (id: number) => void;
}) {
  const [text, setText] = useState("");
  const [tagsStr, setTagsStr] = useState("");
  // pair-scoped: only lessons derived from this pair, not the whole corpus.
  // Use the All Lessons tab to see every lesson.
  const shown = pairId == null ? [] : lessons.filter(l => l.pair_id === pairId);
  const add = async () => {
    if (pairId == null || !text.trim()) return;
    const tags = tagsStr.split(",").map(t => t.trim()).filter(Boolean);
    await api.addLesson(pairId, text.trim(), tags);
    setText(""); setTagsStr("");
    onChanged();
  };
  return (
    <div className="lessons">
      <ul className="lesson-list">
        {shown.map(l => (
          <li key={l.id}>
            <div className="lesson-row">
              <div className="lesson-text">
                <div>{l.lesson}</div>
                <div className="lesson-meta">L{l.id} · pair #{l.pair_id ?? "—"} · [{l.tags.join(", ")}]</div>
              </div>
              <ConfirmButton label="Delete" confirmLabel="Confirm" danger onConfirm={() => onDeleteLesson(l.id)} />
            </div>
          </li>
        ))}
        {shown.length === 0 && pairId != null && <li className="empty small">No lessons for this pair yet. Derive one from the diff and add it here.</li>}
      </ul>
      <div className="add-lesson">
        <textarea value={text} onChange={e => setText(e.target.value)} placeholder="a specific lesson (e.g. ‘use quick note, not I wanted to reach out’)" rows={2} />
        <div className="add-row">
          <input value={tagsStr} onChange={e => setTagsStr(e.target.value)} placeholder="tags: pitch, external" />
          <button className="primary" onClick={add} disabled={pairId == null || !text.trim()}>Add lesson</button>
        </div>
        {pairId == null && <div className="hint">Finalize a draft to attach a lesson to its pair.</div>}
      </div>
    </div>
  );
}

function PairDetail({ pair, onDelete }: { pair: Pair; onDelete: (id: number) => void }) {
  return (
    <div className="pair-detail-inner">
      <div className="meta-row">
        <h3>Pair #{pair.id} {pair.context && <span className="muted">— {pair.context}</span>}</h3>
        <ConfirmButton label="Delete" confirmLabel="Confirm delete" danger onConfirm={() => onDelete(pair.id)} />
      </div>
      <div className="pair-tags">{pair.tags.join(", ")}</div>
      <div className="pair-cols">
        <div><h4>Draft</h4><pre>{pair.draft}</pre></div>
        <div><h4>Final</h4><pre>{pair.final}</pre></div>
      </div>
      <DeletionsSection pairId={pair.id} />
      <h4>Diff</h4>
      <DiffPane diff={pair.diff} oldText={pair.draft} newText={pair.final} />
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
function DeletionsSection({ pairId }: { pairId: number }) {
  const [analysis, setAnalysis] = useState<DiffAnalysis | null>(null);

  useEffect(() => {
    api.analyzePair(pairId).then(setAnalysis).catch(() => setAnalysis(null));
  }, [pairId]);

  if (!analysis) return null;
  const deletions = analysis.deletions;
  const categorized = analysis.categorized || [];
  const noSignal = deletions.length === 0 && categorized.length === 0;
  if (noSignal) return null;

  return (
    <div className="analysis-section">
      {deletions.length > 0 && (
        <>
          <h4 className="signal-head">✂ Deletions ({deletions.length})</h4>
          <ul className="signal-list">
            {deletions.map((d, i) => (
              <li key={i} className="signal-item del">{d}</li>
            ))}
          </ul>
        </>
      )}
      {categorized.length > 0 && (
        <>
          <h4 className="signal-head">Categorized changes</h4>
          <ul className="cat-list">
            {categorized.map((c, i) => (
              <li key={i} className={"cat-item " + c.category}>
                <span className={"cat-badge " + c.category}>{c.category}</span>
                <span className="cat-desc">{c.description}</span>
              </li>
            ))}
          </ul>
        </>
      )}
    </div>
  );
}

// --- lint panel (drafts right-pane) ---

function LintPane({ violations, linting, onRelint }: {
  violations: Violation[]; linting: boolean; onRelint: () => void;
}) {
  return (
    <div className="lint-pane">
      <div className="lint-head">
        <strong>{violations.length} violation(s)</strong>
        <button className="mini" onClick={onRelint} disabled={linting}>
          {linting ? "linting…" : "re-lint"}
        </button>
      </div>
      {violations.length === 0 ? (
        <div className="empty small">
          {linting ? "Checking…" : "No violations. Draft matches all stored patterns."}
        </div>
      ) : (
        <ul className="violation-list">
          {violations.map((v, i) => (
            <li key={i} className={"violation " + v.category}>
              <div className="v-rule">⚠ {v.rule}</div>
              {v.matched_text && <div className="v-match">"{v.matched_text}" — line {v.line}</div>}
              <div className="v-ctx">{v.context}</div>
              <div className="v-meta">[{v.category}] {v.direction}</div>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}

// --- patterns management view ---

function PatternsView({ patterns, onChanged }: {
  patterns: Pattern[]; onChanged: () => void;
}) {
  const [rule, setRule] = useState("");
  const [pattern, setPattern] = useState("");
  const [patternType, setPatternType] = useState("literal");
  const [direction, setDirection] = useState("avoid");
  const [category, setCategory] = useState("style");

  const add = async () => {
    if (!rule.trim() || !pattern.trim()) return;
    await api.addPattern(rule.trim(), pattern.trim(), patternType, direction, category, null, null, null);
    setRule(""); setPattern("");
    onChanged();
  };

  const del = async (id: number) => {
    await api.deletePattern(id);
    onChanged();
  };

  return (
    <div className="search-layout">
      <section className="pattern-list-section">
        <h3>Voice patterns ({patterns.length})</h3>
        <p className="hint">Patterns are matchable rules the lint engine checks drafts against.
        Literal matches are case-insensitive substring searches. Regex uses Rust regex syntax.</p>
        <ul className="pattern-list">
          {patterns.map(p => (
            <li key={p.id}>
              <div className="pattern-row">
                <div className="pattern-text">
                  <div><strong>{p.rule}</strong></div>
                  <div className="pattern-meta">
                    <code>{p.pattern}</code> · {p.pattern_type} · {p.direction} · [{p.category}] · {p.confidence}
                  </div>
                  {p.before_text && p.after_text && (
                    <div className="pattern-ex">
                      <span className="del">{p.before_text}</span> → <span className="add">{p.after_text}</span>
                    </div>
                  )}
                </div>
                <ConfirmButton label="Delete" confirmLabel="Confirm" danger onConfirm={() => del(p.id)} />
              </div>
            </li>
          ))}
          {patterns.length === 0 && <li className="empty">No patterns yet. Add one below — it will immediately lint future drafts.</li>}
        </ul>
      </section>
      <section className="add-pattern-section">
        <h3>Add pattern</h3>
        <input value={rule} onChange={e => setRule(e.target.value)} placeholder="Rule: e.g. No em-dashes in client emails" />
        <input value={pattern} onChange={e => setPattern(e.target.value)} placeholder="Pattern: e.g. — or \b—\b" />
        <div className="pattern-opts">
          <select value={patternType} onChange={e => setPatternType(e.target.value)}>
            <option value="literal">literal</option>
            <option value="regex">regex</option>
          </select>
          <select value={direction} onChange={e => setDirection(e.target.value)}>
            <option value="avoid">avoid</option>
            <option value="prefer">prefer</option>
          </select>
          <select value={category} onChange={e => setCategory(e.target.value)}>
            <option value="style">style</option>
            <option value="punctuation">punctuation</option>
            <option value="structure">structure</option>
            <option value="factual">factual</option>
            <option value="deletion">deletion</option>
          </select>
          <button className="primary" onClick={add} disabled={!rule.trim() || !pattern.trim()}>Add</button>
        </div>
      </section>
    </div>
  );
}

// --- feedback view ---

function FeedbackView({ feedback }: { feedback: Feedback[] }) {
  return (
    <div className="search-layout">
      <section>
        <h3>Feedback ({feedback.length})</h3>
        <p className="hint">Agents log feedback via the <code>give_feedback</code> MCP tool or <code>redline feedback</code> CLI command.</p>
        {feedback.length === 0 ? (
          <div className="empty">No feedback yet.</div>
        ) : (
          <ul className="feedback-list">
            {feedback.map(f => (
              <li key={f.id} className={"feedback-item " + f.severity}>
                <div className="fb-head">
                  <span className={"sev " + f.severity}>{f.severity}</span>
                  {f.tool_name && <span className="fb-tool">{f.tool_name}</span>}
                  {f.rating != null && <span className="fb-rating">{"★".repeat(f.rating)}{"☆".repeat(5 - f.rating)}</span>}
                  <span className="when">{shortWhen(f.created_at)}</span>
                </div>
                <div className="fb-msg">{f.message}</div>
                {f.agent_id && <div className="fb-agent">from: {f.agent_id}</div>}
              </li>
            ))}
          </ul>
        )}
      </section>
    </div>
  );
}
