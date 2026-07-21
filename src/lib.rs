// email-learn library: store (draft, final) email pairs, agent-derived voice
// lessons, and the in-flight drafting surface. Shared by the `email-learn` CLI
// (including its MCP server), the `email-app` Tauri UI, and any MCP client.
// No LLM call from here on purpose — the agent derives lessons in-session
// against the diffs we surface.

pub mod mcp;

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
CREATE TABLE IF NOT EXISTS patterns (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    lesson_id    INTEGER REFERENCES lessons(id) ON DELETE SET NULL,
    rule         TEXT NOT NULL,
    pattern      TEXT NOT NULL,
    pattern_type TEXT NOT NULL DEFAULT 'literal',
    direction    TEXT NOT NULL DEFAULT 'avoid',
    category     TEXT NOT NULL DEFAULT 'style',
    before_text  TEXT,
    after_text   TEXT,
    confidence   TEXT NOT NULL DEFAULT 'unconfirmed',
    created_at   TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS feedback (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    tool_name    TEXT,
    message      TEXT NOT NULL,
    severity     TEXT NOT NULL DEFAULT 'info',
    rating       INTEGER,
    agent_id     TEXT,
    created_at   TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_patterns_lesson ON patterns(lesson_id);
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

// ---------- sentence-level diff ----------
//
// Emails aren't code. Line-level diffs split mid-sentence and make paragraph
// rewrites look like 20 separate changes. Sentence-level diff groups by
// sentence so "this sentence was rewritten" is one insight, not noise.

/// Split text into sentences, keeping the trailing punctuation/whitespace.
fn split_sentences(text: &str) -> Vec<String> {
    let mut result = Vec::new();
    let mut current = String::new();
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        current.push(c);
        if matches!(c, '.' | '!' | '?') {
            if let Some(&next) = chars.peek() {
                if next.is_whitespace() {
                    result.push(std::mem::take(&mut current));
                }
            }
        }
    }
    if !current.is_empty() {
        result.push(current);
    }
    result
}

/// Sentence-level diff: same algorithm as rich_diff but splits on sentence
/// boundaries instead of newlines. Each row is a sentence, not a line.
pub fn rich_diff_sentences(old: &str, new: &str) -> Vec<DiffRow> {
    let old_s = split_sentences(old).join("\n");
    let new_s = split_sentences(new).join("\n");
    rich_diff(&old_s, &new_s)
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

// ---------- lint patterns ----------

/// A matchable voice rule. The `pattern` field is what the lint engine tests
/// against draft content; `before_text`/`after_text` are human-readable
/// examples that help the agent understand the rule's intent.
#[derive(Serialize, Deserialize, Clone)]
pub struct Pattern {
    pub id: i64,
    pub lesson_id: Option<i64>,
    pub rule: String,
    pub pattern: String,
    pub pattern_type: String,  // "literal" | "regex"
    pub direction: String,     // "avoid" | "prefer"
    pub category: String,      // "punctuation" | "style" | "structure" | "factual" | "deletion"
    pub before_text: Option<String>,
    pub after_text: Option<String>,
    pub confidence: String,    // "unconfirmed" | "confirmed"
    pub created_at: String,
}

/// One rule violation found in draft content.
#[derive(Serialize, Deserialize, Clone)]
pub struct Violation {
    pub pattern_id: i64,
    pub lesson_id: Option<i64>,
    pub rule: String,
    pub category: String,
    pub direction: String,
    pub matched_text: String,
    pub context: String,
    pub line: usize,
}

// ---------- feedback ----------

#[derive(Serialize, Deserialize, Clone)]
pub struct Feedback {
    pub id: i64,
    pub tool_name: Option<String>,
    pub message: String,
    pub severity: String,
    pub rating: Option<i64>,
    pub agent_id: Option<String>,
    pub created_at: String,
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
        // Delegate to row_to_pair so the stored diff is normalized via
        // pair_diff() (legacy plain-text rows recomputed into JSON) — same as
        // recent_pairs. Reading the raw column here skipped normalization and
        // made the UI show "no changes yet" for pre-migration pairs.
        Ok(Some(row_to_pair(r)?))
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

/// Rich result from create_draft with patterns to respect + lint violations.
/// This is the write-loop injection point: the agent gets back everything it
/// needs to adjust the draft before the user sees it.
#[derive(Serialize, Deserialize, Clone)]
pub struct DraftContext {
    pub draft_id: i64,
    /// All stored voice patterns the agent should respect.
    pub patterns: Vec<Pattern>,
    /// Lint violations found in the just-created content.
    pub violations: Vec<Violation>,
}

/// Create a draft AND return the patterns + lint violations so the agent can
/// adjust immediately. This is the main entry point for agents.
pub fn create_draft_with_context(
    conn: &Connection,
    content: &str,
    context: Option<&str>,
    tags: &[String],
    source: &str,
) -> anyhow::Result<DraftContext> {
    let draft_id = create_draft(conn, content, context, tags, source)?;
    let patterns = list_patterns(conn, None)?;
    let violations = lint_draft(conn, content)?;
    Ok(DraftContext { draft_id, patterns, violations })
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

/// Delete a pair. The DB ON DELETE SET NULL on lessons.pair_id and
/// drafts.finalized_pair_id leaves derived lessons (the corpus) and any
/// finalized draft intact, just unlinked from this pair.
pub fn delete_pair(conn: &Connection, pair_id: i64) -> anyhow::Result<()> {
    conn.execute("DELETE FROM pairs WHERE id = ?1", params![pair_id])?;
    Ok(())
}

/// Delete a single lesson by id. Its source pair, if any, is left intact.
pub fn delete_lesson(conn: &Connection, lesson_id: i64) -> anyhow::Result<()> {
    conn.execute("DELETE FROM lessons WHERE id = ?1", params![lesson_id])?;
    Ok(())
}

// ---------- lint patterns ----------

pub fn add_pattern(
    conn: &Connection,
    lesson_id: Option<i64>,
    rule: &str,
    pattern: &str,
    pattern_type: &str,
    direction: &str,
    category: &str,
    before_text: Option<&str>,
    after_text: Option<&str>,
) -> anyhow::Result<i64> {
    let now = now_iso();
    conn.execute(
        "INSERT INTO patterns (lesson_id, rule, pattern, pattern_type, direction, category, before_text, after_text, confidence, created_at)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,'unconfirmed',?9)",
        params![lesson_id, rule, pattern, pattern_type, direction, category, before_text, after_text, now],
    )?;
    Ok(conn.last_insert_rowid())
}

pub fn list_patterns(conn: &Connection, lesson_id: Option<i64>) -> anyhow::Result<Vec<Pattern>> {
    let mut stmt = if lesson_id.is_some() {
        conn.prepare(
            "SELECT id, lesson_id, rule, pattern, pattern_type, direction, category, before_text, after_text, confidence, created_at
             FROM patterns WHERE lesson_id = ?1 ORDER BY id DESC",
        )?
    } else {
        conn.prepare(
            "SELECT id, lesson_id, rule, pattern, pattern_type, direction, category, before_text, after_text, confidence, created_at
             FROM patterns ORDER BY id DESC",
        )?
    };
    let rows = if lesson_id.is_some() {
        stmt.query_map(params![lesson_id], row_to_pattern)?
    } else {
        stmt.query_map([], row_to_pattern)?
    };
    let mut out = Vec::new();
    for r in rows {
        out.push(r?);
    }
    Ok(out)
}

pub fn delete_pattern(conn: &Connection, pattern_id: i64) -> anyhow::Result<()> {
    conn.execute("DELETE FROM patterns WHERE id = ?1", params![pattern_id])?;
    Ok(())
}

/// Check if a pattern matches anywhere in text.
fn pattern_matches(pat: &Pattern, text: &str) -> bool {
    match pat.pattern_type.as_str() {
        "regex" => regex::Regex::new(&pat.pattern).map(|r| r.is_match(text)).unwrap_or(false),
        _ => text.to_lowercase().contains(&pat.pattern.to_lowercase()),
    }
}

/// Count how many pairs' drafts contain this pattern. Returns (pair_count, pair_ids).
fn count_pattern_in_pairs(conn: &Connection, pat: &Pattern) -> anyhow::Result<(usize, Vec<i64>)> {
    let pairs = all_pairs_asc(conn)?;
    let hits: Vec<i64> = pairs.iter()
        .filter(|p| pattern_matches(pat, &p.draft))
        .map(|p| p.id)
        .collect();
    Ok((hits.len(), hits))
}

/// Scan all patterns against all pairs. For "avoid" patterns, count how many
/// drafts contain the pattern. Auto-promote unconfirmed → confirmed at 3+.
/// Returns (promoted_count, details) so callers can report what changed.
pub fn promote_patterns(conn: &Connection) -> anyhow::Result<Vec<(i64, String, usize, Vec<i64>)>> {
    let patterns = list_patterns(conn, None)?;
    let mut promoted = Vec::new();
    for pat in &patterns {
        if pat.confidence == "confirmed" {
            continue;
        }
        let (count, pair_ids) = count_pattern_in_pairs(conn, pat)?;
        if count >= 3 {
            conn.execute(
                "UPDATE patterns SET confidence = 'confirmed' WHERE id = ?1",
                params![pat.id],
            )?;
            promoted.push((pat.id, pat.rule.clone(), count, pair_ids));
        }
    }
    Ok(promoted)
}

/// Scan draft content against all stored patterns. Returns violations for
/// "avoid" patterns that matched and suggestions for "prefer" patterns that
/// were absent.
pub fn lint_draft(conn: &Connection, content: &str) -> anyhow::Result<Vec<Violation>> {
    let patterns = list_patterns(conn, None)?;
    let mut violations = Vec::new();

    for pat in &patterns {
        let matches = match pat.pattern_type.as_str() {
            "regex" => {
                match regex::Regex::new(&pat.pattern) {
                    Ok(re) => {
                        re.find_iter(content)
                            .map(|m| (m.as_str().to_string(), content[..m.start()].matches('\n').count() + 1))
                            .collect::<Vec<_>>()
                    }
                    Err(_) => continue, // skip invalid regex
                }
            }
            _ => {
                // literal, case-insensitive
                let lower = content.to_lowercase();
                let needle = pat.pattern.to_lowercase();
                lower.match_indices(&needle)
                    .map(|(idx, _)| {
                        let line = content[..idx].matches('\n').count() + 1;
                        let matched = &content[idx..idx + pat.pattern.len()];
                        (matched.to_string(), line)
                    })
                    .collect::<Vec<_>>()
            }
        };

        match pat.direction.as_str() {
            "prefer" => {
                if matches.is_empty() {
                    // pattern absent — suggest it
                    violations.push(Violation {
                        pattern_id: pat.id,
                        lesson_id: pat.lesson_id,
                        rule: pat.rule.clone(),
                        category: pat.category.clone(),
                        direction: "prefer".into(),
                        matched_text: String::new(),
                        context: format!("consider using: {}", pat.pattern),
                        line: 0,
                    });
                }
            }
            _ => { // "avoid"
                for (matched, line) in &matches {
                    // ponytail: simple context window — 40 chars each side of the match
                    let match_pos = content.find(matched.as_str()).unwrap_or(0);
                    let ctx_start = match_pos.saturating_sub(40);
                    let ctx_end = (match_pos + matched.len() + 40).min(content.len());
                    let context = &content[ctx_start..ctx_end];
                    violations.push(Violation {
                        pattern_id: pat.id,
                        lesson_id: pat.lesson_id,
                        rule: pat.rule.clone(),
                        category: pat.category.clone(),
                        direction: "avoid".into(),
                        matched_text: matched.clone(),
                        context: context.replace('\n', " "),
                        line: *line,
                    });
                }
            }
        }
    }
    Ok(violations)
}

// ---------- diff analysis (surface patterns for lesson derivation) ----------

/// A single categorized change from a pair's diff. Each hunk gets tagged
/// so agents can prioritize voice learning without manual archaeology.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct CategorizedChange {
    pub category: String,  // "structural" | "stylistic" | "factual" | "deletion" | "punctuation"
    pub description: String,
    pub before: String,
    pub after: String,
}

/// Extracted signal from a pair's diff, structured so an agent can derive
/// candidate voice patterns without manual archaeology.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct DiffAnalysis {
    pub pair_id: i64,
    /// All deleted segments — the strongest voice signal (what got cut).
    pub deletions: Vec<String>,
    /// All added segments.
    pub additions: Vec<String>,
    /// Word-level swaps: (old_word, new_word) for inline replacements.
    pub word_swaps: Vec<(String, String)>,
    /// Each change categorized: structural, stylistic, factual, deletion, punctuation.
    pub categorized: Vec<CategorizedChange>,
    /// Existing lint patterns that fire on this pair's draft (what was avoided
    /// before and got through — or was deliberately kept).
    pub draft_pattern_hits: Vec<PatternHit>,
    pub final_pattern_hits: Vec<PatternHit>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct PatternHit {
    pub pattern_id: i64,
    pub rule: String,
}

/// Classify a single diff hunk. The heuristics are deliberately simple —
/// they prioritize the categories the reviewer identified as highest signal:
/// deletions (strongest voice signal), then factual (numbers/names/dates),
/// then punctuation (em-dashes, commas), then structural (whole lines added/
/// removed), and stylistic as the catch-all.
fn categorize_change(before: &str, after: &str) -> CategorizedChange {
    let b = before.trim();
    let a = after.trim();

    // pure deletion — line removed with no replacement
    if !b.is_empty() && a.is_empty() {
        return CategorizedChange {
            category: "deletion".into(),
            description: format!("Deleted: \"{}\"", truncate_str(b, 80)),
            before: b.into(),
            after: a.into(),
        };
    }
    // pure addition — new line with no removed counterpart
    if b.is_empty() && !a.is_empty() {
        // adding a whole new line is structural
        return CategorizedChange {
            category: "structural".into(),
            description: format!("Added: \"{}\"", truncate_str(a, 80)),
            before: b.into(),
            after: a.into(),
        };
    }

    // Check for factual changes: numbers, names, dates
    let num_re = regex::Regex::new(r"\$?[0-9][0-9,]*(\.[0-9]+)?%?").unwrap();
    let date_re = regex::Regex::new(r"\b(jan|feb|mar|apr|may|jun|jul|aug|sep|oct|nov|dec)[a-z]*\s+\d{1,2}\b").unwrap();
    let b_nums: Vec<&str> = num_re.find_iter(b).map(|m| m.as_str()).collect();
    let a_nums: Vec<&str> = num_re.find_iter(a).map(|m| m.as_str()).collect();
    let b_dates = date_re.is_match(b);
    let a_dates = date_re.is_match(a);
    if b_nums != a_nums || (b_dates && !a_dates) || (!b_dates && a_dates) {
        return CategorizedChange {
            category: "factual".into(),
            description: format!("Factual change: \"{}\" \u{2192} \"{}\"", truncate_str(b, 50), truncate_str(a, 50)),
            before: b.into(),
            after: a.into(),
        };
    }

    // Check for punctuation-only changes (em-dashes, commas, periods)
    let letters_unchanged = b.chars()
        .filter(|c| c.is_alphanumeric())
        .collect::<String>()
        .to_lowercase()
        == a.chars()
            .filter(|c| c.is_alphanumeric())
            .collect::<String>()
            .to_lowercase();
    let punct_changed = b != a;
    if letters_unchanged && punct_changed {
        return CategorizedChange {
            category: "punctuation".into(),
            description: format!("Punctuation: \"{}\" \u{2192} \"{}\"", truncate_str(b, 50), truncate_str(a, 50)),
            before: b.into(),
            after: a.into(),
        };
    }

    // Check for structural: very different lengths or many words changed
    let b_words = b.split_whitespace().count();
    let a_words = a.split_whitespace().count();
    let len_diff = (b_words as i64 - a_words as i64).unsigned_abs();
    if len_diff >= 4 || (b_words > 3 && a_words > 3 && b_words as f64 / a_words.max(1) as f64 > 1.5)
        || (a_words > 3 && b_words > 3 && a_words as f64 / b_words.max(1) as f64 > 1.5)
    {
        return CategorizedChange {
            category: "structural".into(),
            description: format!("Structural rewrite: \"{}\" \u{2192} \"{}\"", truncate_str(b, 50), truncate_str(a, 50)),
            before: b.into(),
            after: a.into(),
        };
    }

    // Default: stylistic (rewording, tone shifts)
    CategorizedChange {
        category: "stylistic".into(),
        description: format!("Stylistic: \"{}\" \u{2192} \"{}\"", truncate_str(b, 50), truncate_str(a, 50)),
        before: b.into(),
        after: a.into(),
    }
}

fn truncate_str(s: &str, n: usize) -> &str {
    if s.len() <= n { s } else { &s[..n] }
}

/// Analyze a finalized pair's diff. Surfaces deletions, additions, word swaps,
/// and existing-pattern hits so the agent can derive new patterns or confirm
/// existing ones in one call.
pub fn analyze_diff(conn: &Connection, pair_id: i64) -> anyhow::Result<Option<DiffAnalysis>> {
    let pair = match show_pair(conn, pair_id)? {
        Some(p) => p,
        None => return Ok(None),
    };

    let rows: Vec<DiffRow> = serde_json::from_str(&pair.diff).unwrap_or_default();
    let mut deletions = Vec::new();
    let mut additions = Vec::new();
    let mut word_swaps = Vec::new();
    let mut categorized = Vec::new();

    let mut i = 0;
    while i < rows.len() {
        if rows[i].kind == "removed" {
            let del_text: String = rows[i].segments.iter()
                .filter(|s| s.tag == "del" || s.tag == "ctx")
                .map(|s| s.text.as_str())
                .collect();
            let del_words: Vec<&str> = rows[i].segments.iter()
                .filter(|s| s.tag == "del")
                .map(|s| s.text.trim())
                .filter(|s| !s.is_empty())
                .collect();

            // pair with next added row for word-level swaps + categorization
            if i + 1 < rows.len() && rows[i + 1].kind == "added" {
                let add_words: Vec<&str> = rows[i + 1].segments.iter()
                    .filter(|s| s.tag == "add")
                    .map(|s| s.text.trim())
                    .filter(|s| !s.is_empty())
                    .collect();
                for (d, a) in del_words.iter().zip(add_words.iter()) {
                    word_swaps.push((d.to_string(), a.to_string()));
                }
                let add_text: String = rows[i + 1].segments.iter()
                    .filter(|s| s.tag == "add" || s.tag == "ctx")
                    .map(|s| s.text.as_str())
                    .collect();
                additions.push(add_text.trim().to_string());
                categorized.push(categorize_change(&del_text, &add_text));
                i += 2;
            } else {
                deletions.push(del_text.trim().to_string());
                categorized.push(categorize_change(&del_text, ""));
                i += 1;
            }
        } else if rows[i].kind == "added" {
            let add_text: String = rows[i].segments.iter()
                .map(|s| s.text.as_str())
                .collect();
            additions.push(add_text.trim().to_string());
            categorized.push(categorize_change("", &add_text));
            i += 1;
        } else {
            i += 1;
        }
    }

    // Check existing patterns against draft and final
    let patterns = list_patterns(conn, None)?;
    let draft_hits: Vec<PatternHit> = patterns.iter()
        .filter(|p| match p.pattern_type.as_str() {
            "regex" => regex::Regex::new(&p.pattern).map(|r| r.is_match(&pair.draft)).unwrap_or(false),
            _ => pair.draft.to_lowercase().contains(&p.pattern.to_lowercase()),
        })
        .map(|p| PatternHit { pattern_id: p.id, rule: p.rule.clone() })
        .collect();
    let final_hits: Vec<PatternHit> = patterns.iter()
        .filter(|p| match p.pattern_type.as_str() {
            "regex" => regex::Regex::new(&p.pattern).map(|r| r.is_match(&pair.final_)).unwrap_or(false),
            _ => pair.final_.to_lowercase().contains(&p.pattern.to_lowercase()),
        })
        .map(|p| PatternHit { pattern_id: p.id, rule: p.rule.clone() })
        .collect();

    Ok(Some(DiffAnalysis {
        pair_id,
        deletions,
        additions,
        word_swaps,
        categorized,
        draft_pattern_hits: draft_hits,
        final_pattern_hits: final_hits,
    }))
}

// ---------- feedback ----------

pub fn add_feedback(
    conn: &Connection,
    tool_name: Option<&str>,
    message: &str,
    severity: &str,
    rating: Option<i64>,
    agent_id: Option<&str>,
) -> anyhow::Result<i64> {
    let now = now_iso();
    conn.execute(
        "INSERT INTO feedback (tool_name, message, severity, rating, agent_id, created_at)
         VALUES (?1,?2,?3,?4,?5,?6)",
        params![tool_name, message, severity, rating, agent_id, now],
    )?;
    Ok(conn.last_insert_rowid())
}

pub fn list_feedback(conn: &Connection) -> anyhow::Result<Vec<Feedback>> {
    let mut stmt = conn.prepare(
        "SELECT id, tool_name, message, severity, rating, agent_id, created_at
         FROM feedback ORDER BY id DESC",
    )?;
    let rows = stmt.query_map([], row_to_feedback)?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r?);
    }
    Ok(out)
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

/// Rich result from finalize: pair_id + analysis data + promoted patterns.
/// The agent gets back everything it needs to derive lessons and patterns.
#[derive(Serialize, Deserialize, Clone)]
pub struct FinalizeResult {
    pub pair_id: i64,
    pub analysis: DiffAnalysis,
    pub promoted_patterns: Vec<(i64, String, usize, Vec<i64>)>,
}

/// Finalize: latest revision becomes the final, first revision is the agent's
/// original, compute the diff, write a `pairs` row, link it. Then run
/// occurrence-based promotion on all patterns. Returns analysis data.
pub fn finalize_draft(conn: &Connection, draft_id: i64) -> anyhow::Result<i64> {
    finalize_draft_impl(conn, draft_id).map(|r| r.pair_id)
}

/// Full finalize with analysis + promotion. This is the main entry point for agents.
pub fn finalize_draft_with_analysis(conn: &Connection, draft_id: i64) -> anyhow::Result<FinalizeResult> {
    finalize_draft_impl(conn, draft_id)
}

fn finalize_draft_impl(conn: &Connection, draft_id: i64) -> anyhow::Result<FinalizeResult> {
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

    // Auto-promote patterns based on the new pair data
    let promoted = promote_patterns(conn)?;

    // Return the analysis so the agent can derive lessons immediately
    let analysis = analyze_diff(conn, pair_id)?
        .unwrap_or(DiffAnalysis {
            pair_id,
            deletions: vec![],
            additions: vec![],
            word_swaps: vec![],
            categorized: vec![],
            draft_pattern_hits: vec![],
            final_pattern_hits: vec![],
        });

    Ok(FinalizeResult { pair_id, analysis, promoted_patterns: promoted })
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

fn row_to_pattern(r: &rusqlite::Row<'_>) -> rusqlite::Result<Pattern> {
    Ok(Pattern {
        id: r.get(0)?,
        lesson_id: r.get(1)?,
        rule: r.get(2)?,
        pattern: r.get(3)?,
        pattern_type: r.get(4)?,
        direction: r.get(5)?,
        category: r.get(6)?,
        before_text: r.get(7)?,
        after_text: r.get(8)?,
        confidence: r.get(9)?,
        created_at: r.get(10)?,
    })
}

fn row_to_feedback(r: &rusqlite::Row<'_>) -> rusqlite::Result<Feedback> {
    Ok(Feedback {
        id: r.get(0)?,
        tool_name: r.get(1)?,
        message: r.get(2)?,
        severity: r.get(3)?,
        rating: r.get(4)?,
        agent_id: r.get(5)?,
        created_at: r.get(6)?,
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

    #[test]
    fn show_pair_normalizes_legacy_plain_text_diff() {
        // A pair written before the rich-diff JSON migration stores a plain
        // unified diff in the `diff` column. show_pair must normalize it back
        // to structured JSON (via pair_diff) — otherwise the frontend's
        // JSON.parse fails and the UI shows "no changes yet".
        let path = tmp_db();
        let conn = connect_at(&path).unwrap();
        conn.execute(
            "INSERT INTO pairs (draft, final, diff, context, tags, created_at) \
             VALUES (?1,?2,?3,?4,?5,?6)",
            params![
                "Hey man,\nOption 1 works.\n",
                "Hi Carolina,\nThanks.\n",
                "-Hey man,\n+Hi Carolina,\n+Thanks.\n", // legacy plain text
                "test",
                "[]",
                now_iso(),
            ],
        ).unwrap();
        let pair = show_pair(&conn, 1).unwrap().expect("pair exists");
        // diff must parse as structured JSON, not plain text
        let rows: Vec<DiffRow> = serde_json::from_str(&pair.diff)
            .expect("show_pair diff is JSON Vec<DiffRow>");
        let added: String = rows.iter()
            .find(|r| r.kind == "added").expect("added row")
            .segments.iter().map(|s| s.text.as_str()).collect();
        assert!(added.contains("Carolina"), "recomputed added line: {added}");
        // and recent_pairs must agree (same normalization path)
        let list = recent_pairs(&conn, 10).unwrap();
        assert_eq!(list[0].diff, pair.diff, "show_pair and recent_pairs agree");
    }

    #[test]
    fn lint_catches_avoid_pattern_and_suggests_prefer() {
        let path = tmp_db();
        let conn = connect_at(&path).unwrap();

        // avoid pattern: em-dashes
        add_pattern(&conn, None, "No em-dashes", "—", "literal", "avoid", "punctuation", None, None).unwrap();
        // prefer pattern: use the word "quick" not "fast"
        add_pattern(&conn, None, "Prefer quick over fast", "quick", "literal", "prefer", "style", None, None).unwrap();

        // content with an em-dash and no "quick" → 2 violations
        let v = lint_draft(&conn, "Hey — that was fast.").unwrap();
        assert_eq!(v.len(), 2, "should have 2 violations: em-dash + missing 'quick'");
        let has_emdash = v.iter().any(|x| x.rule == "No em-dashes");
        assert!(has_emdash, "em-dash violation present");
        let has_quick = v.iter().any(|x| x.direction == "prefer");
        assert!(has_quick, "prefer suggestion for missing 'quick'");

        // content that's clean → 0 violations
        let v2 = lint_draft(&conn, "Hey. That was quick.").unwrap();
        assert_eq!(v2.len(), 0, "clean content should have 0 violations");
    }

    #[test]
    fn lint_regex_pattern_works() {
        let path = tmp_db();
        let conn = connect_at(&path).unwrap();
        add_pattern(&conn, None, "No terse openers", r"^.{1,10}$", "regex", "avoid", "style", None, None).unwrap();
        let v = lint_draft(&conn, "Hi.").unwrap();
        assert!(!v.is_empty(), "regex pattern should match terse 'Hi.'");
    }

    #[test]
    fn analyze_diff_surfaces_word_swaps_and_pattern_hits() {
        let path = tmp_db();
        let conn = connect_at(&path).unwrap();
        // pattern: avoid em-dashes
        add_pattern(&conn, None, "No em-dashes", "—", "literal", "avoid", "punctuation", None, None).unwrap();
        // pair: draft had em-dash, final removed it
        let pair_id = add_pair(&conn, "Hey — quick update.", "Hey. Quick update.", Some("test"), &[]).unwrap();
        let a = analyze_diff(&conn, pair_id).unwrap().expect("analysis exists");
        assert!(!a.word_swaps.is_empty(), "should have word swaps");
        assert!(!a.draft_pattern_hits.is_empty(), "draft should hit em-dash pattern");
        assert!(a.final_pattern_hits.is_empty(), "final should have no em-dash hits");
    }

    #[test]
    fn sentence_diff_groups_by_sentence() {
        let old = "I wanted to reach out about the project.\nThis is really important.\nLet's talk next week.";
        let new = "Quick note on the project.\nThis matters a lot.\nLet's chat next week.";
        let sent_rows = rich_diff_sentences(old, new);
        // both should detect changes
        assert!(sent_rows.iter().any(|r| r.kind != "equal"), "should have changes");
        // each changed row should contain a full sentence, not a partial line fragment
        let changed_texts: Vec<String> = sent_rows.iter()
            .filter(|r| r.kind != "equal")
            .map(|r| r.segments.iter().map(|s| s.text.as_str()).collect::<String>())
            .collect();
        // at least one row should contain a complete sentence with punctuation
        assert!(changed_texts.iter().any(|t| t.contains('.') || t.contains('!') || t.contains('?')),
            "sentence diff rows should contain sentence-level text: {changed_texts:?}");
    }

    #[test]
    fn create_draft_with_context_returns_patterns_and_violations() {
        let path = tmp_db();
        let conn = connect_at(&path).unwrap();
        // seed a pattern
        add_pattern(&conn, None, "No em-dashes", "—", "literal", "avoid", "punctuation", None, None).unwrap();

        let ctx = create_draft_with_context(
            &conn, "Hey — quick note.", Some("test"), &[], "agent",
        ).unwrap();

        assert!(ctx.draft_id > 0);
        assert_eq!(ctx.patterns.len(), 1, "should return the stored pattern");
        assert!(!ctx.violations.is_empty(), "should detect em-dash violation");
        assert_eq!(ctx.violations[0].rule, "No em-dashes");
    }

    #[test]
    fn finalize_with_analysis_returns_diff_data() {
        let path = tmp_db();
        let conn = connect_at(&path).unwrap();
        let id = create_draft(&conn, "I wanted to reach out.\n", None, &[], "agent").unwrap();
        save_revision(&conn, id, "Quick note.\n", "user").unwrap();

        let result = finalize_draft_with_analysis(&conn, id).unwrap();
        assert!(result.pair_id > 0);
        assert!(!result.analysis.word_swaps.is_empty() || !result.analysis.deletions.is_empty(),
            "analysis should surface the diff");
    }

    #[test]
    fn promote_patterns_auto_confirms_at_three_occurrences() {
        let path = tmp_db();
        let conn = connect_at(&path).unwrap();
        add_pattern(&conn, None, "No reach out", "reach out", "literal", "avoid", "style", None, None).unwrap();

        // add 2 pairs with the pattern in the draft — not enough yet
        add_pair(&conn, "I wanted to reach out.", "Quick note.", None, &[]).unwrap();
        add_pair(&conn, "Going to reach out soon.", "Hi.", None, &[]).unwrap();
        let promoted = promote_patterns(&conn).unwrap();
        assert!(promoted.is_empty(), "should not promote at 2 occurrences");

        // add a 3rd pair with the pattern — now promotes
        add_pair(&conn, "Wanted to reach out again.", "Hello.", None, &[]).unwrap();
        let promoted = promote_patterns(&conn).unwrap();
        assert_eq!(promoted.len(), 1, "should promote at 3 occurrences");
        assert_eq!(promoted[0].1, "No reach out");
        assert_eq!(promoted[0].2, 3, "3 occurrences");

        // verify confidence is now confirmed
        let pats = list_patterns(&conn, None).unwrap();
        assert_eq!(pats[0].confidence, "confirmed");
    }

    #[test]
    fn categorize_change_detects_each_type() {
        // deletion: line removed with no replacement
        let d = categorize_change("This is important.", "");
        assert_eq!(d.category, "deletion");

        // structural: added line with no removed counterpart
        let s = categorize_change("", "Brand new paragraph here.");
        assert_eq!(s.category, "structural");

        // punctuation: em-dash to period
        let p = categorize_change("Hey — quick note", "Hey. Quick note");
        assert_eq!(p.category, "punctuation");

        // factual: number changed
        let f = categorize_change("Budget is $500", "Budget is $750");
        assert_eq!(f.category, "factual");

        // stylistic: word swap, same structure
        let st = categorize_change("I wanted to reach out", "Quick note on this");
        assert_eq!(st.category, "stylistic");
    }

    #[test]
    fn analyze_diff_includes_categorized() {
        let path = tmp_db();
        let conn = connect_at(&path).unwrap();
        // pair with deletion, punctuation change, and factual change
        let pair_id = add_pair(
            &conn,
            "Hey — budget is $500.\nTagline goes here.\n",
            "Hey. Budget is $750.\n",
            None, &[],
        ).unwrap();
        let a = analyze_diff(&conn, pair_id).unwrap().expect("analysis exists");
        assert!(!a.categorized.is_empty(), "should have categorized changes");
        // should have at least one deletion (the tagline)
        assert!(a.categorized.iter().any(|c| c.category == "deletion"),
            "should detect deletion: {:?}", a.categorized);
    }
}
