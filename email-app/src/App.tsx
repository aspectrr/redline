import { useCallback, useEffect, useRef, useState } from "react";
import { api, parseDiff } from "./lib/api";
import type { Draft, DraftWithRevisions, DiffRow, Lesson, Pair, SearchResult } from "./lib/api";
import "./App.css";

type View = "drafts" | "library" | "search";
type RightTab = "diff" | "revisions" | "lessons";

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

  const reloadLibrary = useCallback(async () => {
    try {
      setPairs(await api.listPairs(200));
      setLessons(await api.listLessons());
    } catch (e) { flushError(e); }
  }, []);

  useEffect(() => { if (view === "library") reloadLibrary(); }, [view, reloadLibrary]);

  const runSearch = async () => {
    try { setResults(await api.search(q)); } catch (e) { flushError(e); }
  };

  const openPair = async (id: number) => {
    try { setSelectedPair(await api.showPair(id)); } catch (e) { flushError(e); }
  };

  return (
    <div className="app">
      <header className="topbar">
        <div className="brand">✉️ Email for Agents</div>
        <nav className="tabs">
          <button className={view === "drafts" ? "active" : ""} onClick={() => setView("drafts")}>Drafts</button>
          <button className={view === "library" ? "active" : ""} onClick={() => setView("library")}>Library</button>
          <button className={view === "search" ? "active" : ""} onClick={() => setView("search")}>Search</button>
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
              {drafts.length === 0 && <li className="empty">No drafts. The agent can push one via <code>email-learn draft</code>, or click + New draft.</li>}
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
              <button className={rightTab === "lessons" ? "active" : ""} onClick={() => setRightTab("lessons")}>Lessons</button>
            </div>
            <div className="right-body">
              {rightTab === "diff" && current && <DiffPane diff={current.working_diff} />}
              {rightTab === "revisions" && current && (
                <RevisionsPane revisions={current.revisions} onRestore={restore} />
              )}
              {rightTab === "lessons" && current && (
                <LessonsPane lessons={lessons}
                  pairId={current.draft.finalized_pair_id}
                  onChanged={reloadLibrary} />
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
            {selectedPair ? <PairDetail pair={selectedPair} /> : <div className="empty-editor">Select a pair.</div>}
          </section>
          <aside className="right-pane">
            <div className="right-tabs"><button className="active">Lessons</button></div>
            <div className="right-body">
              <LessonsPane lessons={lessons} pairId={selectedPair?.id ?? null} onChanged={reloadLibrary} />
            </div>
          </aside>
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
    </div>
  );
}

function DiffPane({ diff }: { diff: string }) {
  const rows = parseDiff(diff);
  if (!rows.some(r => r.kind !== "equal")) {
    return <div className="empty small">No changes yet — the editor matches the original draft.</div>;
  }
  return (
    <pre className="diff">
      {rows.map((row, i) => <DiffLine key={i} row={row} />)}
    </pre>
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

function LessonsPane({ lessons, pairId, onChanged }: {
  lessons: Lesson[]; pairId: number | null; onChanged: () => void;
}) {
  const [text, setText] = useState("");
  const [tagsStr, setTagsStr] = useState("");
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
        {lessons.map(l => (
          <li key={l.id}><div>{l.lesson}</div><div className="lesson-meta">L{l.id} · pair #{l.pair_id ?? "—"} · [{l.tags.join(", ")}]</div></li>
        ))}
        {lessons.length === 0 && <li className="empty small">No lessons yet. Derive one from a diff and add it here.</li>}
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

function PairDetail({ pair }: { pair: Pair }) {
  return (
    <div className="pair-detail-inner">
      <h3>Pair #{pair.id} {pair.context && <span className="muted">— {pair.context}</span>}</h3>
      <div className="pair-tags">{pair.tags.join(", ")}</div>
      <div className="pair-cols">
        <div><h4>Draft</h4><pre>{pair.draft}</pre></div>
        <div><h4>Final</h4><pre>{pair.final}</pre></div>
      </div>
      <h4>Diff</h4>
      <DiffPane diff={pair.diff} />
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
