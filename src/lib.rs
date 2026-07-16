// email-learn library: store (draft, final) email pairs, agent-derived voice
// lessons, and the in-flight drafting surface. Shared by the `email-learn` CLI
// and the `email-app` Tauri UI. No LLM call from here on purpose — the agent
// derives lessons in-session against the diffs we surface.

use rusqlite::params;
use serde::{Deserialize, Serialize};
use similar::{ChangeTag, TextDiff};
use std::path::PathBuf;

pub use rusqlite::Connection;

pub const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS pairs (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    draft        TEXT NOT NULL,
    final        TEXT NOT NULL,
    diff         TEXT NOT NULL,
    context      TEXT,
    tags         TEXT,
    created_at   TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS lessons (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    pair_id      INTEGER REFERENCES pairs(id) ON DELETE SET NULL,
    lesson       TEXT NOT NULL,
    tags         TEXT,
    created_at   TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS drafts (
    id                INTEGER PRIMARY KEY AUTOINCREMENT,
    context           TEXT,
    tags              TEXT,
    status            TEXT NOT NULL DEFAULT 'draft',
    finalized_pair_id INTEGER REFERENCES pairs(id) ON DELETE SET NULL,
    created_at        TEXT NOT NULL,
    updated_at        TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS draft_revisions (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    draft_id   INTEGER NOT NULL REFERENCES drafts(id) ON DELETE CASCADE,
    content    TEXT NOT NULL,
    source     TEXT NOT NULL DEFAULT 'agent',
    created_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_pairs_tags ON pairs(tags);
CREATE INDEX IF NOT EXISTS idx_lessons_tags ON lessons(tags);
CREATE INDEX IF NOT EXISTS idx_drafts_status ON drafts(status);
CREATE INDEX IF NOT EXISTS idx_draft_revisions_draft ON draft_revisions(draft_id);
";

/// Where the shared, cross-project voice DB lives.
/// Override with `EMAIL_LEARN_DB=/abs/path/emails.db`.
pub fn db_path() -> PathBuf {
    if let Ok(p) = std::env::var("EMAIL_LEARN_DB") {
        return PathBuf::from(p);
    }
    let mut p = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    p.push(".email-learn");
    p.push("emails.db");
    p
}

pub fn connect() -> anyhow::Result<Connection> {
    connect_at(&db_path())
}

/// Open (and migrate) the voice DB at an explicit path — used by `connect()`
/// and by tests that want an isolated file.
pub fn connect_at(path: &std::path::Path) -> anyhow::Result<Connection> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let conn = Connection::open(path)?;
    conn.execute_batch(SCHEMA)?;
    // WAL so the Tauri UI and the CLI can both touch the DB without clobbering.
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    Ok(conn)
}

pub fn now_iso() -> String {
    chrono::Utc::now().to_rfc3339()
}

// ---------- diff (GitHub-style: line-level with intra-line word highlights) ----------
//
// The algorithm lives in `similar` (a pure-Rust Myers diff — the same family
// git uses) rather than a `git` subprocess. A line-level diff decides which
// lines changed; for each removed→added line pair we run a second word-level
// diff so only the actually-edited words are flagged. That is exactly how
// GitHub's diff viewer highlights intra-line edits.

#[derive(Serialize, Deserialize, Clone)]
pub struct DiffSegment {
    /// "add" | "del" | "ctx"
    pub tag: String,
    pub text: String,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct DiffRow {
    /// "equal" | "removed" | "added"
    pub kind: String,
    pub segments: Vec<DiffSegment>,
}

fn trim_line(s: &str) -> &str {
    s.strip_suffix('\n').unwrap_or(s).strip_suffix('\r').unwrap_or(s)
}

/// Word-level diff of a paired (old line, new line), split into segments for
/// the removed side and the added side. Common words carry tag "ctx" on both
/// sides so the UI can dim them; changed words are "del" / "add".
fn paired_word_diff(old: &str, new: &str) -> (Vec<DiffSegment>, Vec<DiffSegment>) {
    let wd = TextDiff::from_words(old, new);
    let mut old_segs = Vec::new();
    let mut new_segs = Vec::new();
    for c in wd.iter_all_changes() {
        let text = c.value().to_string();
        match c.tag() {
            ChangeTag::Equal => {
                old_segs.push(DiffSegment { tag: "ctx".into(), text: text.clone() });
                new_segs.push(DiffSegment { tag: "ctx".into(), text });
            }
            ChangeTag::Delete => old_segs.push(DiffSegment { tag: "del".into(), text }),
            ChangeTag::Insert => new_segs.push(DiffSegment { tag: "add".into(), text }),
        }
    }
    (old_segs, new_segs)
}

/// GitHub-style structured diff: one row per line. Changed line pairs get
/// intra-line word segments so the UI can highlight just the edited words.
pub fn rich_diff(old: &str, new: &str) -> Vec<DiffRow> {
    let ops: Vec<(ChangeTag, String)> = TextDiff::from_lines(old, new)
        .iter_all_changes()
        .map(|c| (c.tag(), c.value().to_string()))
        .collect();

    let mut rows = Vec::new();
    let mut i = 0;
    while i < ops.len() {
        match ops[i].0 {
            ChangeTag::Equal => {
                rows.push(DiffRow {
                    kind: "equal".into(),
                    segments: vec![DiffSegment { tag: "ctx".into(), text: trim_line(&ops[i].1).into() }],
                });
                i += 1;
            }
            ChangeTag::Delete | ChangeTag::Insert => {
                // collect the contiguous run of removed/added lines in this hunk
                let mut dels: Vec<String> = Vec::new();
                let mut ins: Vec<String> = Vec::new();
                while i < ops.len() && ops[i].0 != ChangeTag::Equal {
                    match ops[i].0 {
                        ChangeTag::Delete => dels.push(ops[i].1.clone()),
                        ChangeTag::Insert => ins.push(ops[i].1.clone()),
                        _ => {}
                    }
                    i += 1;
                }
                let paired = dels.len().min(ins.len());
                for k in 0..paired {
                    let (o, n) = paired_word_diff(trim_line(&dels[k]), trim_line(&ins[k]));
                    rows.push(DiffRow { kind: "removed".into(), segments: o });
                    rows.push(DiffRow { kind: "added".into(), segments: n });
                }
                for d in &dels[paired..] {
                    rows.push(DiffRow {
                        kind: "removed".into(),
                        segments: vec![DiffSegment { tag: "del".into(), text: trim_line(d).into() }],
                    });
                }
                for s in &ins[paired..] {
                    rows.push(DiffRow {
                        kind: "added".into(),
                        segments: vec![DiffSegment { tag: "add".into(), text: trim_line(s).into() }],
                    });
                }
            }
        }
    }
    rows
}

/// Flatten structured rows back to a plain unified diff (for export / CLI).
pub fn render_diff_plain(rows: &[DiffRow]) -> String {
    let mut out = String::new();
    for row in rows {
        let prefix = match row.kind.as_str() {
            "removed" => '-',
            "added" => '+',
            _ => ' ',
        };
        out.push(prefix);
        for seg in &row.segments {
            out.push_str(&seg.text);
        }
        out.push('\n');
    }
    out
}

/// Convenience: plain unified-diff text (kept for callers/tests).
pub fn unified_diff(old: &str, new: &str) -> String {
    render_diff_plain(&rich_diff(old, new))
}

/// Stored diff is JSON once written by the current code; rows from before the
/// rich-diff change hold plain text and are recomputed from draft/final.
fn pair_diff(stored: &str, draft: &str, final_: &str) -> String {
    if serde_json::from_str::<Vec<DiffRow>>(stored).is_ok() {
        stored.to_string()
    } else {
        serde_json::to_string(&rich_diff(draft, final_)).unwrap_or_default()
    }
}

// ---------- types ----------

#[derive(Serialize, Deserialize, Clone)]
pub struct Pair {
    pub id: i64,
    pub draft: String,
    #[serde(rename = "final")]
    pub final_: String,
    pub diff: String,
    pub context: Option<String>,
    pub tags: Vec<String>,
    pub created_at: String,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct Lesson {
    pub id: i64,
    pub pair_id: Option<i64>,
    pub lesson: String,
    pub tags: Vec<String>,
    pub created_at: String,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct Draft {
    pub id: i64,
    pub context: Option<String>,
    pub tags: Vec<String>,
    pub status: String,
    pub finalized_pair_id: Option<i64>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct DraftRevision {
    pub id: i64,
    pub draft_id: i64,
    pub content: String,
    pub source: String,
    pub created_at: String,
}

/// A draft plus every revision (append-only) and a precomputed working diff
/// between the original (first revision) and current (latest revision).
#[derive(Serialize, Deserialize, Clone)]
pub struct DraftWithRevisions {
    pub draft: Draft,
    pub revisions: Vec<DraftRevision>,
    pub working_diff: String,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct SearchResult {
    pub drafts: Vec<Draft>,
    pub pairs: Vec<Pair>,
    pub lessons: Vec<Lesson>,
}

// ---------- tag helpers ----------

pub fn parse_tags(s: Option<&str>) -> Vec<String> {
    match s {
        None | Some("") => Vec::new(),
        Some(t) => serde_json::from_str::<Vec<String>>(t).unwrap_or_default(),
    }
}

pub fn tags_to_json(tags: &[String]) -> String {
    serde_json::to_string(tags).unwrap_or_else(|_| "[]".into())
}

// ---------- pairs + lessons ----------

pub fn add_pair(
    conn: &Connection,
    draft: &str,
    final_: &str,
    context: Option<&str>,
    tags: &[String],
) -> anyhow::Result<i64> {
    let diff = serde_json::to_string(&rich_diff(draft, final_))?;
    let tags_json = tags_to_json(tags);
    let now = now_iso();
    conn.execute(
        "INSERT INTO pairs (draft, final, diff, context, tags, created_at) VALUES (?1,?2,?3,?4,?5,?6)",
        params![draft, final_, diff, context, tags_json, now],
    )?;
    Ok(conn.last_insert_rowid())
}

pub fn show_pair(conn: &Connection, id: i64) -> anyhow::Result<Option<Pair>> {
    let mut stmt = conn.prepare(
        "SELECT id, draft, final, diff, context, tags, created_at FROM pairs WHERE id = ?1",
    )?;
    let mut rows = stmt.query(params![id])?;
    if let Some(r) = rows.next()? {
        let tags = parse_tags(r.get::<_, Option<String>>(5)?.as_deref());
        Ok(Some(Pair {
            id: r.get(0)?,
            draft: r.get(1)?,
            final_: r.get(2)?,
            diff: r.get(3)?,
            context: r.get(4)?,
            tags,
            created_at: r.get(6)?,
        }))
    } else {
        Ok(None)
    }
}

pub fn recent_pairs(conn: &Connection, n: usize) -> anyhow::Result<Vec<Pair>> {
    let mut stmt = conn.prepare(
        "SELECT id, draft, final, diff, context, tags, created_at FROM pairs ORDER BY id DESC LIMIT ?1",
    )?;
    let rows = stmt.query_map(params![n as i64], row_to_pair)?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r?);
    }
    Ok(out)
}

pub fn lessons(conn: &Connection, tags: &[String]) -> anyhow::Result<Vec<Lesson>> {
    let mut out: Vec<Lesson> = Vec::new();
    if tags.is_empty() {
        let mut stmt = conn.prepare(
            "SELECT id, pair_id, lesson, tags, created_at FROM lessons ORDER BY id DESC",
        )?;
        let rows = stmt.query_map([], row_to_lesson)?;
        for x in rows {
            out.push(x?);
        }
    } else {
        let pats: Vec<String> = tags
            .iter()
            .map(|t| format!("%\"{}\"%", t.replace('"', "\\\"")))
            .collect();
        let placeholders = (0..pats.len())
            .map(|_| "tags LIKE ?")
            .collect::<Vec<_>>()
            .join(" OR ");
        let sql = format!(
            "SELECT id, pair_id, lesson, tags, created_at FROM lessons WHERE {placeholders} ORDER BY id DESC"
        );
        let mut stmt = conn.prepare(&sql)?;
        let refs: Vec<&dyn rusqlite::ToSql> = pats.iter().map(|p| p as &dyn rusqlite::ToSql).collect();
        let rows = stmt.query_map(refs.as_slice(), row_to_lesson)?;
        for x in rows {
            out.push(x?);
        }
    }
    Ok(out)
}

pub fn add_lesson(
    conn: &Connection,
    pair_id: i64,
    lesson: &str,
    tags: &[String],
) -> anyhow::Result<i64> {
    let tags_json = tags_to_json(tags);
    let now = now_iso();
    conn.execute(
        "INSERT INTO lessons (pair_id, lesson, tags, created_at) VALUES (?1,?2,?3,?4)",
        params![pair_id, lesson, tags_json, now],
    )?;
    Ok(conn.last_insert_rowid())
}

pub fn query(conn: &Connection, needle: &str) -> anyhow::Result<(Vec<Pair>, Vec<Lesson>)> {
    let pat = format!("%{needle}%");
    let mut pairs: Vec<Pair> = Vec::new();
    {
        let mut stmt = conn.prepare(
            "SELECT id, draft, final, diff, context, tags, created_at FROM pairs
             WHERE context LIKE ?1 OR tags LIKE ?1 OR final LIKE ?1 OR draft LIKE ?1
             ORDER BY id DESC LIMIT 50",
        )?;
        let rows = stmt.query_map(params![pat], row_to_pair)?;
        for x in rows {
            pairs.push(x?);
        }
    }
    let mut lessons: Vec<Lesson> = Vec::new();
    {
        let mut stmt = conn.prepare(
            "SELECT id, pair_id, lesson, tags, created_at FROM lessons
             WHERE lesson LIKE ?1 OR tags LIKE ?1 ORDER BY id DESC LIMIT 50",
        )?;
        let rows = stmt.query_map(params![pat], row_to_lesson)?;
        for x in rows {
            lessons.push(x?);
        }
    }
    Ok((pairs, lessons))
}

pub fn export_md(conn: &Connection) -> anyhow::Result<String> {
    let pairs = all_pairs_asc(conn)?;
    let lessons = lessons(conn, &[])?;

    let mut md = String::new();
    md.push_str("# Voice Lessons (exported)\n\n");
    md.push_str("## Lessons\n\n");
    if lessons.is_empty() {
        md.push_str("_(none yet)_\n\n");
    }
    for l in &lessons {
        md.push_str(&format!(
            "- **L{}** (pair #{}) {}: {}  _{}_\n",
            l.id,
            l.pair_id.map(|i| i.to_string()).unwrap_or_else(|| "—".into()),
            l.tags.join(","),
            l.lesson,
            l.created_at
        ));
    }
    md.push_str("\n## Pairs\n\n");
    for p in &pairs {
        md.push_str(&format!("### Pair #{} — {}\n", p.id, p.created_at));
        if let Some(c) = &p.context {
            md.push_str(&format!("context: {c}\n"));
        }
        if !p.tags.is_empty() {
            md.push_str(&format!("tags: {}\n", p.tags.join(", ")));
        }
        md.push_str("\n#### Draft\n```\n");
        md.push_str(&p.draft);
        if !p.draft.ends_with('\n') {
            md.push('\n');
        }
        md.push_str("```\n#### Final\n```\n");
        md.push_str(&p.final_);
        if !p.final_.ends_with('\n') {
            md.push('\n');
        }
        md.push_str("```\n#### Diff\n```diff\n");
        let diff_plain = render_diff_plain(&rich_diff(&p.draft, &p.final_));
        md.push_str(&diff_plain);
        if !diff_plain.ends_with('\n') {
            md.push('\n');
        }
        md.push_str("```\n\n");
    }
    Ok(md)
}

// ---------- drafts + revisions ----------

/// Create a new in-flight draft and record its first revision (the agent's
/// original text). Returns the new draft id.
pub fn create_draft(
    conn: &Connection,
    content: &str,
    context: Option<&str>,
    tags: &[String],
    source: &str,
) -> anyhow::Result<i64> {
    let now = now_iso();
    let tags_json = tags_to_json(tags);
    conn.execute(
        "INSERT INTO drafts (context, tags, status, created_at, updated_at) VALUES (?1,?2,'draft',?3,?3)",
        params![context, tags_json, now],
    )?;
    let draft_id = conn.last_insert_rowid();
    conn.execute(
        "INSERT INTO draft_revisions (draft_id, content, source, created_at) VALUES (?1,?2,?3,?4)",
        params![draft_id, content, source, now],
    )?;
    Ok(draft_id)
}

pub fn list_drafts(conn: &Connection, include_finalized: bool) -> anyhow::Result<Vec<Draft>> {
    let sql = if include_finalized {
        "SELECT id, context, tags, status, finalized_pair_id, created_at, updated_at
         FROM drafts ORDER BY updated_at DESC"
    } else {
        "SELECT id, context, tags, status, finalized_pair_id, created_at, updated_at
         FROM drafts WHERE status = 'draft' ORDER BY updated_at DESC"
    };
    let mut stmt = conn.prepare(sql)?;
    let rows = stmt.query_map([], row_to_draft)?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r?);
    }
    Ok(out)
}

pub fn get_draft(conn: &Connection, id: i64) -> anyhow::Result<Option<DraftWithRevisions>> {
    let draft: Option<Draft> = match conn.query_row(
        "SELECT id, context, tags, status, finalized_pair_id, created_at, updated_at
         FROM drafts WHERE id = ?1",
        params![id],
        row_to_draft,
    ) {
        Ok(d) => Some(d),
        Err(rusqlite::Error::QueryReturnedNoRows) => None,
        Err(e) => return Err(e.into()),
    };
    let Some(draft) = draft else { return Ok(None) };

    let mut revisions: Vec<DraftRevision> = Vec::new();
    {
        let mut stmt = conn.prepare(
            "SELECT id, draft_id, content, source, created_at FROM draft_revisions
             WHERE draft_id = ?1 ORDER BY id ASC",
        )?;
        let rows = stmt.query_map(params![id], |r| {
            Ok(DraftRevision {
                id: r.get(0)?,
                draft_id: r.get(1)?,
                content: r.get(2)?,
                source: r.get(3)?,
                created_at: r.get(4)?,
            })
        })?;
        for x in rows {
            revisions.push(x?);
        }
    }

    let working_diff = match (revisions.first(), revisions.last()) {
        (Some(first), Some(last)) if first.id != last.id => {
            serde_json::to_string(&rich_diff(&first.content, &last.content)).unwrap_or_default()
        }
        _ => String::new(),
    };

    Ok(Some(DraftWithRevisions {
        draft,
        revisions,
        working_diff,
    }))
}

/// Record a new revision (append-only). `source` is "agent" | "user" | "restore".
pub fn save_revision(
    conn: &Connection,
    draft_id: i64,
    content: &str,
    source: &str,
) -> anyhow::Result<i64> {
    let now = now_iso();
    conn.execute(
        "INSERT INTO draft_revisions (draft_id, content, source, created_at) VALUES (?1,?2,?3,?4)",
        params![draft_id, content, source, now],
    )?;
    let rev_id = conn.last_insert_rowid();
    conn.execute(
        "UPDATE drafts SET updated_at = ?1 WHERE id = ?2",
        params![now, draft_id],
    )?;
    Ok(rev_id)
}

/// Update a draft's context + tags without touching its content history.
pub fn update_draft_meta(
    conn: &Connection,
    draft_id: i64,
    context: Option<&str>,
    tags: &[String],
) -> anyhow::Result<()> {
    let now = now_iso();
    let tags_json = tags_to_json(tags);
    conn.execute(
        "UPDATE drafts SET context = ?1, tags = ?2, updated_at = ?3 WHERE id = ?4",
        params![context, tags_json, now, draft_id],
    )?;
    Ok(())
}

/// Delete a draft and (via ON DELETE CASCADE) its revisions. A finalized pair
/// is the permanent learning artifact and is left intact.
pub fn delete_draft(conn: &Connection, draft_id: i64) -> anyhow::Result<()> {
    conn.execute("DELETE FROM drafts WHERE id = ?1", params![draft_id])?;
    Ok(())
}
/// History is never destroyed — this is your "restore in one place".
pub fn restore_revision(
    conn: &Connection,
    draft_id: i64,
    revision_id: i64,
) -> anyhow::Result<i64> {
    let content: String = conn.query_row(
        "SELECT content FROM draft_revisions WHERE id = ?1 AND draft_id = ?2",
        params![revision_id, draft_id],
        |r| r.get(0),
    )?;
    save_revision(conn, draft_id, &content, "restore")
}

/// Finalize: latest revision becomes the final, first revision is the agent's
/// original, compute the diff, write a `pairs` row, link it. Returns the pair id.
pub fn finalize_draft(conn: &Connection, draft_id: i64) -> anyhow::Result<i64> {
    let (draft_row, revisions) = match get_draft(conn, draft_id)? {
        Some(d) => (d.draft, d.revisions),
        None => anyhow::bail!("no draft with id {draft_id}"),
    };
    let (first, last) = match (revisions.first(), revisions.last()) {
        (Some(f), Some(l)) => (f, l),
        _ => anyhow::bail!("draft {draft_id} has no revisions"),
    };
    let pair_id = add_pair(
        conn,
        &first.content,
        &last.content,
        draft_row.context.as_deref(),
        &draft_row.tags,
    )?;
    let now = now_iso();
    conn.execute(
        "UPDATE drafts SET status = 'finalized', finalized_pair_id = ?1, updated_at = ?2 WHERE id = ?3",
        params![pair_id, now, draft_id],
    )?;
    Ok(pair_id)
}

// ---------- search across everything ----------

pub fn search_all(conn: &Connection, needle: &str) -> anyhow::Result<SearchResult> {
    let pat = format!("%{needle}%");
    let mut drafts: Vec<Draft> = Vec::new();
    {
        // match on draft context/tags OR any of its revisions' content.
        let mut stmt = conn.prepare(
            "SELECT DISTINCT d.id, d.context, d.tags, d.status, d.finalized_pair_id, d.created_at, d.updated_at
             FROM drafts d
             LEFT JOIN draft_revisions r ON r.draft_id = d.id
             WHERE d.context LIKE ?1 OR d.tags LIKE ?1 OR r.content LIKE ?1
             ORDER BY d.updated_at DESC LIMIT 50",
        )?;
        let rows = stmt.query_map(params![pat], row_to_draft)?;
        for x in rows {
            drafts.push(x?);
        }
    }
    let (pairs, lessons) = query(conn, needle)?;
    Ok(SearchResult {
        drafts,
        pairs,
        lessons,
    })
}

// ---------- optional LLM summarization/audit seam ----------
//
// Lesson *derivation* (diff -> a specific rule) stays in the agent session —
// the agent has the request's context a context-free LLM call lacks. This seam
// is only for *summarization/audit* over already-derived lessons (distill
// themes, surface contradictions/stale rules). It is behind an env flag so the
// default build makes zero LLM calls. Stub now; wire a provider later
// (ollama | openai | mcp keyed off EMAIL_LEARN_LLM).

pub trait LessonSummarizer: Send + Sync {
    fn summarize(&self, lessons: &[Lesson]) -> anyhow::Result<String>;
}

pub struct NoopSummarizer;
impl LessonSummarizer for NoopSummarizer {
    fn summarize(&self, _: &[Lesson]) -> anyhow::Result<String> {
        Ok("LLM summarization disabled. Set EMAIL_LEARN_LLM to enable \
            (future: ollama | openai | mcp). Derivation stays in the agent session.".into())
    }
}

/// Build a summarizer from the environment. Today this is always the noop stub;
/// the env var is the documented extension point for a future provider.
pub fn summarizer_from_env() -> Box<dyn LessonSummarizer> {
    Box::new(NoopSummarizer)
}

pub fn summarize_lessons(conn: &Connection) -> anyhow::Result<String> {
    let ls = lessons(conn, &[])?;
    summarizer_from_env().summarize(&ls)
}

// ---------- row mappers ----------

fn row_to_pair(r: &rusqlite::Row<'_>) -> rusqlite::Result<Pair> {
    let draft: String = r.get(1)?;
    let final_: String = r.get(2)?;
    let stored_diff: String = r.get(3)?;
    let tags = parse_tags(r.get::<_, Option<String>>(5)?.as_deref());
    let diff = pair_diff(&stored_diff, &draft, &final_);
    Ok(Pair {
        id: r.get(0)?,
        draft,
        final_,
        diff,
        context: r.get(4)?,
        tags,
        created_at: r.get(6)?,
    })
}

fn row_to_lesson(r: &rusqlite::Row<'_>) -> rusqlite::Result<Lesson> {
    let t = parse_tags(r.get::<_, Option<String>>(3)?.as_deref());
    Ok(Lesson {
        id: r.get(0)?,
        pair_id: r.get(1)?,
        lesson: r.get(2)?,
        tags: t,
        created_at: r.get(4)?,
    })
}

fn row_to_draft(r: &rusqlite::Row<'_>) -> rusqlite::Result<Draft> {
    let tags = parse_tags(r.get::<_, Option<String>>(2)?.as_deref());
    Ok(Draft {
        id: r.get(0)?,
        context: r.get(1)?,
        tags,
        status: r.get(3)?,
        finalized_pair_id: r.get(4)?,
        created_at: r.get(5)?,
        updated_at: r.get(6)?,
    })
}

// all pairs, ascending — used by export so oldest-to-newest reads like a log.
fn all_pairs_asc(conn: &Connection) -> anyhow::Result<Vec<Pair>> {
    let mut stmt = conn.prepare(
        "SELECT id, draft, final, diff, context, tags, created_at FROM pairs ORDER BY id ASC",
    )?;
    let rows = stmt.query_map([], row_to_pair)?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r?);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    fn tmp_db() -> PathBuf {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let pid = std::process::id();
        let mut p = std::env::temp_dir();
        p.push(format!("el_test_{pid}_{n}.db"));
        let _ = std::fs::remove_file(&p);
        p
    }

    #[test]
    fn draft_edit_diff_finalize_restore_loop() {
        let path = tmp_db();
        let conn = connect_at(&path).unwrap();

        // agent pushes an original draft
        let id = create_draft(
            &conn,
            "I wanted to reach out about the project.\n",
            Some("cold intro to investor"),
            &["pitch".into(), "external".into()],
            "agent",
        )
        .unwrap();

        // user edits it (the voice swap the agent should learn from)
        save_revision(&conn, id, "Quick note on the project.\n", "user").unwrap();

        // working diff exists and is non-empty (original vs latest)
        let d = get_draft(&conn, id).unwrap().unwrap();
        assert_eq!(d.revisions.len(), 2);
        let wrows: Vec<DiffRow> = serde_json::from_str(&d.working_diff).unwrap();
        let removed_text: String = wrows.iter()
            .find(|r| r.kind == "removed").expect("removed row")
            .segments.iter().map(|s| s.text.as_str()).collect();
        assert!(removed_text.contains("reach out"), "removed line: {removed_text}");
        let added_text: String = wrows.iter()
            .find(|r| r.kind == "added").expect("added row")
            .segments.iter().map(|s| s.text.as_str()).collect();
        assert!(added_text.contains("Quick"), "added line: {added_text}");

        // finalize -> writes a pair capturing the (original, edited) diff
        let pair_id = finalize_draft(&conn, id).unwrap();
        let pair = show_pair(&conn, pair_id).unwrap().unwrap();
        assert_eq!(pair.draft, "I wanted to reach out about the project.\n");
        assert_eq!(pair.final_, "Quick note on the project.\n");
        let prows: Vec<DiffRow> = serde_json::from_str(&pair.diff).unwrap();
        let added_text: String = prows.iter()
            .find(|r| r.kind == "added").expect("added row")
            .segments.iter().map(|s| s.text.as_str()).collect();
        assert!(added_text.contains("Quick"), "pair added line: {added_text}");

        // the draft is now finalized and linked
        let d = get_draft(&conn, id).unwrap().unwrap();
        assert_eq!(d.draft.status, "finalized");
        assert_eq!(d.draft.finalized_pair_id, Some(pair_id));

        // restore the original: append-only, so 3 revisions now, history intact
        let first_rev_id = d.revisions[0].id;
        restore_revision(&conn, id, first_rev_id).unwrap();
        let d = get_draft(&conn, id).unwrap().unwrap();
        assert_eq!(d.revisions.len(), 3);
        assert_eq!(d.revisions.last().unwrap().source, "restore");
        assert_eq!(d.revisions.last().unwrap().content, "I wanted to reach out about the project.\n");
        // first revision still the original — never destroyed
        assert_eq!(d.revisions.first().unwrap().content, "I wanted to reach out about the project.\n");
    }

    #[test]
    fn search_finds_draft_by_revision_body() {
        let path = tmp_db();
        let conn = connect_at(&path).unwrap();
        create_draft(&conn, "Schedule the quarterly review\n", None, &[], "agent").unwrap();
        let res = search_all(&conn, "quarterly").unwrap();
        assert_eq!(res.drafts.len(), 1);
        assert_eq!(res.lessons.len(), 0);
        assert_eq!(res.pairs.len(), 0);
    }

    #[test]
    fn rich_diff_highlights_words_not_whole_lines() {
        // one line, one word changed — only that word should be flagged
        let rows = rich_diff("Quick note on the launch", "Quick note on the release");
        let removed: Vec<_> = rows.iter().filter(|r| r.kind == "removed").collect();
        let added: Vec<_> = rows.iter().filter(|r| r.kind == "added").collect();
        assert_eq!(removed.len(), 1, "one changed line on the removed side");
        assert_eq!(added.len(), 1, "one changed line on the added side");
        // only the changed word is marked del; the common words stay ctx
        let del_text: String = removed[0].segments.iter()
            .filter(|s| s.tag == "del").map(|s| s.text.as_str()).collect();
        assert_eq!(del_text.trim(), "launch");
        assert!(!del_text.contains("note"));
        let add_text: String = added[0].segments.iter()
            .filter(|s| s.tag == "add").map(|s| s.text.as_str()).collect();
        assert_eq!(add_text.trim(), "release");
    }

    #[test]
    fn delete_draft_removes_draft_and_revisions_keeps_pair() {
        let path = tmp_db();
        let conn = connect_at(&path).unwrap();
        let id = create_draft(&conn, "original\n", None, &[], "agent").unwrap();
        save_revision(&conn, id, "edited\n", "user").unwrap();
        let pair_id = finalize_draft(&conn, id).unwrap();
        assert!(show_pair(&conn, pair_id).unwrap().is_some());
        delete_draft(&conn, id).unwrap();
        // draft + its revisions gone
        assert!(get_draft(&conn, id).unwrap().is_none());
        let rev_count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM draft_revisions WHERE draft_id = ?1",
            params![id], |r| r.get(0),
        ).unwrap();
        assert_eq!(rev_count, 0);
        // the finalized pair (learning corpus) is left intact
        assert!(show_pair(&conn, pair_id).unwrap().is_some());
    }
}
