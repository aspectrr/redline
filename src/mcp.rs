// MCP server over stdio. Exposes the whole redline surface as tools so a
// coding agent (pi, Claude, …) can read pairs/diffs/lessons, record derived
// lessons, push and edit drafts, and search — the same loop the CLI offers, but
// callable as MCP tools instead of a subprocess.
//
// Design:
// - Lesson *derivation* stays in the agent session (it has the request context
//   a context-free call lacks). This server only stores/retrieves.
// - Every tool opens its own SQLite connection (cheap, local, WAL) — same model
//   as the Tauri commands, so the CLI, the app, and MCP all read/write one DB.
// - Tool-level failures (not found, DB error) return Ok(CallToolResult::error)
//   so the *agent sees the message*. Only unroutable requests become Err.
// - Diffs are returned as plain unified-diff text (render_diff_plain) — the
//   readable form an agent derives lessons from, matching the CLI `show`.

use rmcp::{
    ErrorData as McpError, ServerHandler, ServiceExt,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::*,
    schemars,
    tool, tool_handler, tool_router,
};

// mcp.rs lives inside the redline crate, so reference siblings via `crate`.
use crate as el;

type ToolResult = Result<CallToolResult, McpError>;

/// Run the MCP server over stdio. Logs go to stderr — stdout is the protocol.
pub async fn serve() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive(tracing::Level::INFO.into()),
        )
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .init();

    tracing::info!(
        "redline MCP server starting (db: {})",
        el::db_path().display()
    );

    let service = EmailServer::new()
        .serve(rmcp::transport::stdio())
        .await
        .map_err(|e| anyhow::anyhow!("MCP serve error: {e:?}"))?;
    service.waiting().await?;
    Ok(())
}

// `tool_router` is read by macro-generated ServerHandler methods (call_tool/
// list_tools) that dead-code analysis can't see.
#[derive(Clone)]
#[allow(dead_code)]
struct EmailServer {
    tool_router: ToolRouter<EmailServer>,
}

// ---------- tool parameter schemas ----------

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct AddPairParams {
    /// The agent's original draft text.
    draft: String,
    /// The user's edited final text.
    #[serde(rename = "final")]
    final_text: String,
    /// One-line context: topic + recipient type (e.g. "cold intro to investor").
    context: Option<String>,
    /// Optional tags, e.g. ["pitch", "external"].
    tags: Option<Vec<String>>,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct IdParams {
    /// The pair, draft, or revision id to operate on (see tool description).
    id: i64,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct ListPatternsParams {
    /// Optional: filter patterns by lesson_id. Omit to list ALL patterns.
    lesson_id: Option<i64>,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct RecentParams {
    /// How many pairs to return (default 10).
    limit: Option<i64>,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct TagsFilterParams {
    /// Filter to lessons tagged with any of these.
    tags: Option<Vec<String>>,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct AddLessonParams {
    /// The pair this lesson was derived from.
    pair_id: i64,
    /// A specific, concrete voice rule (e.g. `Open with "Quick note", not "I wanted to reach out"`).
    lesson: String,
    /// Optional tags.
    tags: Option<Vec<String>>,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct NeedleParams {
    /// Substring to search for (case-insensitive LIKE).
    needle: String,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct CreateDraftParams {
    /// The draft body text.
    content: String,
    /// One-line context.
    context: Option<String>,
    /// Optional tags.
    tags: Option<Vec<String>>,
    /// Who produced this draft: "agent" (default) or "user".
    source: Option<String>,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct ListDraftsParams {
    /// Include finalized drafts (default false = open drafts only).
    include_finalized: Option<bool>,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct SaveRevisionParams {
    /// The draft to edit.
    draft_id: i64,
    /// The full new content for the draft.
    content: String,
    /// Source of this edit: "user" (default), "agent", or "restore".
    source: Option<String>,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct RestoreRevisionParams {
    draft_id: i64,
    /// The revision id to restore (appends a new revision — history is never destroyed).
    revision_id: i64,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct DraftIdParams {
    draft_id: i64,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct UpdateDraftMetaParams {
    draft_id: i64,
    /// New one-line context.
    context: Option<String>,
    /// New tags (replaces existing).
    tags: Option<Vec<String>>,
}

// --- pattern + analysis + feedback params ---

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct AddPatternParams {
    /// Human-readable rule: "No em-dashes in client emails"
    rule: String,
    /// The match string: literal text or regex pattern.
    pattern: String,
    /// "literal" (default) or "regex".
    #[serde(default = "default_literal")]
    pattern_type: String,
    /// "avoid" (default — flag if found) or "prefer" (flag if absent).
    #[serde(default = "default_avoid")]
    direction: String,
    /// "punctuation", "style", "structure", "factual", "deletion". Default: "style".
    #[serde(default = "default_style")]
    category: String,
    /// Link to an existing lesson (optional).
    lesson_id: Option<i64>,
    /// Example before-text (optional, for agent context).
    before_text: Option<String>,
    /// Example after-text (optional, for agent context).
    after_text: Option<String>,
}

fn default_literal() -> String { "literal".into() }
fn default_avoid() -> String { "avoid".into() }
fn default_style() -> String { "style".into() }

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct AnalyzeParams {
    /// The pair to analyze.
    pair_id: i64,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct FeedbackParams {
    /// Free-text feedback message.
    message: String,
    /// Which tool/command the feedback is about (optional).
    tool_name: Option<String>,
    /// "info", "warning", "error", "suggestion". Default: "info".
    #[serde(default = "default_info")]
    severity: String,
    /// 1-5 rating (optional).
    rating: Option<i64>,
    /// Which agent is reporting (optional).
    agent_id: Option<String>,
}

fn default_info() -> String { "info".into() }

// ---------- tools ----------

#[tool_router]
impl EmailServer {
    pub fn new() -> Self {
        Self {
            tool_router: Self::tool_router(),
        }
    }

    /// Store a completed (draft → final) email pair and return its id plus the
    /// diff an agent should learn from. The core of the learning loop.
    #[tool(description = "Store a completed (draft, final) email pair. Returns the new pair id and the unified diff to learn from.")]
    fn add_pair(&self, Parameters(p): Parameters<AddPairParams>) -> ToolResult {
        tool_op(|conn| {
            let tags = p.tags.unwrap_or_default();
            let pair_id = el::add_pair(conn, &p.draft, &p.final_text, p.context.as_deref(), &tags)?;
            let diff = el::render_diff_plain(&el::rich_diff(&p.draft, &p.final_text));
            Ok(ok_json(serde_json::json!({ "pair_id": pair_id, "diff": diff })))
        })
    }

    /// Show one pair: draft, final, and the plain unified diff.
    #[tool(description = "Show one pair: draft, final, and the unified diff. Use this to derive voice lessons.")]
    fn show_pair(&self, Parameters(p): Parameters<IdParams>) -> ToolResult {
        tool_op(|conn| match el::show_pair(conn, p.id)? {
            Some(pair) => {
                let diff = el::render_diff_plain(&el::rich_diff(&pair.draft, &pair.final_));
                Ok(ok_json(serde_json::json!({
                    "id": pair.id, "draft": pair.draft, "final": pair.final_,
                    "diff": diff, "context": pair.context, "tags": pair.tags, "created_at": pair.created_at,
                })))
            }
            None => Ok(err_text(format!("no pair with id {}", p.id))),
        })
    }

    /// List the most recent pairs (compact: id, context, tags, created_at, preview).
    #[tool(description = "List the most recent pairs. Returns compact summaries — call show_pair for full diff.")]
    fn recent_pairs(&self, Parameters(p): Parameters<RecentParams>) -> ToolResult {
        tool_op(|conn| {
            let pairs = el::recent_pairs(conn, p.limit.unwrap_or(10) as usize)?;
            let out: Vec<_> = pairs.iter().map(|x| serde_json::json!({
                "id": x.id, "context": x.context, "tags": x.tags, "created_at": x.created_at,
                "preview": preview(&x.final_, 120),
            })).collect();
            Ok(ok_json(out))
        })
    }

    /// List stored voice lessons (optionally filtered by tags).
    #[tool(description = "List stored voice lessons (optionally filtered by tags). These are the concrete rules derived from past diffs.")]
    fn list_lessons(&self, Parameters(p): Parameters<TagsFilterParams>) -> ToolResult {
        tool_op(|conn| {
            let ls = el::lessons(conn, &p.tags.unwrap_or_default())?;
            Ok(ok_json(ls))
        })
    }

    /// Record a voice lesson derived from a pair's diff.
    #[tool(description = "Record a concrete voice lesson derived from a pair's diff. Keep it specific, not generic.")]
    fn add_lesson(&self, Parameters(p): Parameters<AddLessonParams>) -> ToolResult {
        tool_op(|conn| {
            let tags = p.tags.unwrap_or_default();
            let id = el::add_lesson(conn, p.pair_id, &p.lesson, &tags)?;
            Ok(ok_json(serde_json::json!({ "lesson_id": id })))
        })
    }

    /// Substring search across pairs (context/tags/draft/final) and lessons.
    #[tool(description = "Substring search across pairs (context, tags, draft, final) and lessons. Returns matching pairs and lessons.")]
    fn query(&self, Parameters(p): Parameters<NeedleParams>) -> ToolResult {
        tool_op(|conn| {
            let (pairs, lessons) = el::query(conn, &p.needle)?;
            Ok(ok_json(serde_json::json!({
                "pairs": pairs.iter().map(|x| serde_json::json!({
                    "id": x.id, "context": x.context, "tags": x.tags, "preview": preview(&x.final_, 120)
                })).collect::<Vec<_>>(),
                "lessons": lessons,
            })))
        })
    }

    /// Broad search across drafts, pairs, and lessons (includes draft revision bodies).
    #[tool(description = "Broad search across drafts (including revision bodies), pairs, and lessons.")]
    fn search(&self, Parameters(p): Parameters<NeedleParams>) -> ToolResult {
        tool_op(|conn| {
            let res = el::search_all(conn, &p.needle)?;
            Ok(ok_json(res))
        })
    }

    /// Dump the entire library (pairs + lessons) as markdown for bulk injection.
    #[tool(description = "Dump every pair and lesson as a single markdown document, for bulk injection into a prompt.")]
    fn export(&self) -> ToolResult {
        tool_op(|conn| {
            let md = el::export_md(conn)?;
            Ok(ok_text(md))
        })
    }

    /// Summarize/audit stored lessons via the (optional, currently noop) LLM seam.
    #[tool(description = "Summarize/audit stored lessons via the optional LLM seam. Currently a noop stub (no provider wired).")]
    fn summarize(&self) -> ToolResult {
        tool_op(|conn| {
            let s = el::summarize_lessons(conn)?;
            Ok(ok_text(s))
        })
    }

    /// Push a new in-flight draft (agent ingest). Returns the draft id PLUS
    /// all stored voice patterns to respect and lint violations found in the
    /// content. The agent should adjust the draft to resolve violations before
    /// the user sees it.
    #[tool(description = "Create a new in-flight draft. Returns the draft id, all voice patterns to respect, and lint violations in the content. Fix violations with save_revision before the user sees the draft. This is the main agent entry point — the returned patterns ARE the write loop.")]
    fn create_draft(&self, Parameters(p): Parameters<CreateDraftParams>) -> ToolResult {
        tool_op(|conn| {
            let tags = p.tags.unwrap_or_default();
            let source = p.source.as_deref().unwrap_or("agent");
            let ctx = el::create_draft_with_context(conn, &p.content, p.context.as_deref(), &tags, source)?;
            Ok(ok_json(serde_json::json!({
                "draft_id": ctx.draft_id,
                "patterns": ctx.patterns,
                "violations": ctx.violations,
                "violation_count": ctx.violations.len(),
                "next_step": if ctx.violations.is_empty() {
                    "No violations. The draft is ready for the user to edit."
                } else {
                    "Fix the violations above with save_revision, then wait for the user to edit."
                },
            })))
        })
    }

    /// Read a draft: metadata, every revision (append-only), and the working
    /// diff between the original and latest revision.
    #[tool(description = "Read a draft: metadata, full revision history, and the working diff (original → latest).")]
    fn get_draft(&self, Parameters(p): Parameters<DraftIdParams>) -> ToolResult {
        tool_op(|conn| match el::get_draft(conn, p.draft_id)? {
            Some(d) => {
                let (first, last) = (d.revisions.first(), d.revisions.last());
                let working_diff = match (first, last) {
                    (Some(f), Some(l)) if f.id != l.id => {
                        el::render_diff_plain(&el::rich_diff(&f.content, &l.content))
                    }
                    _ => String::new(),
                };
                let revs: Vec<_> = d.revisions.iter().map(|r| serde_json::json!({
                    "id": r.id, "source": r.source, "created_at": r.created_at, "preview": preview(&r.content, 160),
                })).collect();
                Ok(ok_json(serde_json::json!({
                    "draft": d.draft, "revisions": revs, "working_diff": working_diff,
                    "revision_count": d.revisions.len(),
                })))
            }
            None => Ok(err_text(format!("no draft with id {}", p.draft_id))),
        })
    }

    /// List drafts (open by default; set include_finalized for all).
    #[tool(description = "List drafts. Open drafts by default; set include_finalized=true to include finalized ones.")]
    fn list_drafts(&self, Parameters(p): Parameters<ListDraftsParams>) -> ToolResult {
        tool_op(|conn| {
            let drafts = el::list_drafts(conn, p.include_finalized.unwrap_or(false))?;
            Ok(ok_json(drafts))
        })
    }

    /// Save a new revision of a draft (append-only). Returns the revision id
    /// PLUS lint violations so the agent knows what still needs fixing.
    #[tool(description = "Save a new revision of a draft (append-only — history is never destroyed). Returns the revision id and updated lint violations so the agent can see what's still wrong.")]
    fn save_revision(&self, Parameters(p): Parameters<SaveRevisionParams>) -> ToolResult {
        tool_op(|conn| {
            let source = p.source.as_deref().unwrap_or("user");
            let id = el::save_revision(conn, p.draft_id, &p.content, source)?;
            let violations = el::lint_draft(conn, &p.content)?;
            Ok(ok_json(serde_json::json!({
                "revision_id": id,
                "violations": violations,
                "violation_count": violations.len(),
            })))
        })
    }

    /// Restore a past revision (appends a copy — history stays intact).
    #[tool(description = "Restore a past revision of a draft. Appends a new revision copying the old one; history is never destroyed.")]
    fn restore_revision(&self, Parameters(p): Parameters<RestoreRevisionParams>) -> ToolResult {
        tool_op(|conn| {
            let id = el::restore_revision(conn, p.draft_id, p.revision_id)?;
            Ok(ok_json(serde_json::json!({ "revision_id": id, "restored": true })))
        })
    }

    /// Finalize a draft: latest revision becomes the final, the first revision
    /// is the original, the diff is stored as a pair. Returns the pair id.
    /// Finalize: stores a (original → latest) pair, marks the draft finalized,
    /// auto-promotes patterns based on occurrence count, and returns the diff
    /// analysis so the agent can derive lessons immediately.
    #[tool(description = "Finalize a draft: stores a (original → edited) pair for learning and marks the draft finalized. Returns the pair id, diff analysis (deletions, additions, word swaps, pattern hits), and any patterns that were auto-promoted to confirmed. Derive voice lessons from the analysis and store them with add_lesson.")]
    fn finalize_draft(&self, Parameters(p): Parameters<DraftIdParams>) -> ToolResult {
        tool_op(|conn| {
            let result = el::finalize_draft_with_analysis(conn, p.draft_id)?;
            Ok(ok_json(serde_json::json!({
                "pair_id": result.pair_id,
                "finalized": true,
                "analysis": result.analysis,
                "promoted_patterns": result.promoted_patterns.iter().map(|(id, rule, count, pairs)| {
                    serde_json::json!({ "pattern_id": id, "rule": rule, "occurrences": count, "pairs": pairs })
                }).collect::<Vec<_>>(),
                "next_step": "Review the analysis. Derive voice lessons from the deletions and word swaps, then store them with add_lesson and create matchable patterns with add_pattern.",
            })))
        })
    }

    /// Delete a draft and its revisions. A finalized pair, if any, is kept.
    #[tool(description = "Delete a draft and its revisions. A finalized pair (the learning corpus) is kept intact.")]
    fn delete_draft(&self, Parameters(p): Parameters<DraftIdParams>) -> ToolResult {
        tool_op(|conn| {
            el::delete_draft(conn, p.draft_id)?;
            Ok(ok_json(serde_json::json!({ "deleted": true, "draft_id": p.draft_id })))
        })
    }

    /// Delete a pair. Derived lessons are unlinked (kept), not deleted.
    #[tool(description = "Delete a pair. Derived lessons are unlinked (pair_id set to null) but kept in the corpus; a finalized draft is unlinked too.")]
    fn delete_pair(&self, Parameters(p): Parameters<IdParams>) -> ToolResult {
        tool_op(|conn| {
            el::delete_pair(conn, p.id)?;
            Ok(ok_json(serde_json::json!({ "deleted": true, "pair_id": p.id })))
        })
    }

    /// Delete a single lesson by id.
    #[tool(description = "Delete a single voice lesson by id. Its source pair, if any, is left intact.")]
    fn delete_lesson(&self, Parameters(p): Parameters<IdParams>) -> ToolResult {
        tool_op(|conn| {
            el::delete_lesson(conn, p.id)?;
            Ok(ok_json(serde_json::json!({ "deleted": true, "lesson_id": p.id })))
        })
    }

    /// Update a draft's context and/or tags without touching its content.
    #[tool(description = "Update a draft's context and tags without touching its revision history.")]
    fn update_draft_meta(&self, Parameters(p): Parameters<UpdateDraftMetaParams>) -> ToolResult {
        tool_op(|conn| {
            let tags = p.tags.unwrap_or_default();
            el::update_draft_meta(conn, p.draft_id, p.context.as_deref(), &tags)?;
            Ok(ok_json(serde_json::json!({ "updated": true, "draft_id": p.draft_id })))
        })
    }

    // --- patterns + analysis + feedback ---
    // Linting is automatic: create_draft and save_revision return violations.
    // No standalone lint tool — fewer round-trips for the agent.

    /// Add a matchable voice pattern the lint engine will check drafts against.
    #[tool(description = "Add a matchable voice pattern for the lint engine. pattern_type is 'literal' or 'regex'. direction is 'avoid' (flag if found) or 'prefer' (flag if absent). Returns the new pattern id.")]
    fn add_pattern(&self, Parameters(p): Parameters<AddPatternParams>) -> ToolResult {
        tool_op(|conn| {
            let id = el::add_pattern(
                conn, p.lesson_id, &p.rule, &p.pattern, &p.pattern_type,
                &p.direction, &p.category, p.before_text.as_deref(), p.after_text.as_deref(),
            )?;
            Ok(ok_json(serde_json::json!({ "pattern_id": id })))
        })
    }

    /// List stored voice patterns.
    #[tool(description = "List all stored voice patterns, or filter by lesson_id.")]
    fn list_patterns(&self, Parameters(p): Parameters<ListPatternsParams>) -> ToolResult {
        tool_op(|conn| {
            let lesson_id = p.lesson_id.filter(|&id| id > 0);
            let patterns = el::list_patterns(conn, lesson_id)?;
            Ok(ok_json(serde_json::json!({ "patterns": patterns })))
        })
    }

    /// Delete a pattern by id.
    #[tool(description = "Delete a voice pattern by id.")]
    fn delete_pattern(&self, Parameters(p): Parameters<IdParams>) -> ToolResult {
        tool_op(|conn| {
            el::delete_pattern(conn, p.id)?;
            Ok(ok_json(serde_json::json!({ "deleted": true, "pattern_id": p.id })))
        })
    }

    /// Analyze a finalized pair's diff: surface deletions, additions, word
    /// swaps, and existing-pattern hits. Data for deriving candidate patterns.
    #[tool(description = "Analyze a pair's diff to surface deletions, additions, word-level swaps, and existing pattern hits. Use this to derive candidate voice patterns instead of manual archaeology over a raw diff.")]
    fn analyze_diff(&self, Parameters(p): Parameters<AnalyzeParams>) -> ToolResult {
        tool_op(|conn| match el::analyze_diff(conn, p.pair_id)? {
            Some(a) => Ok(ok_json(serde_json::json!({ "analysis": a }))),
            None => Ok(err_text(format!("no pair with id {}", p.pair_id))),
        })
    }

    // --- feedback (agents and humans can log issues) ---

    /// Log feedback about the tool — bugs, suggestions, confusion points.
    #[tool(description = "Log feedback about the redline tool. Accepts a free-text message, optional tool_name, severity (info/warning/error/suggestion), and 1-5 rating. This is how agents report what's painful.")]
    fn give_feedback(&self, Parameters(p): Parameters<FeedbackParams>) -> ToolResult {
        tool_op(|conn| {
            let id = el::add_feedback(
                conn, p.tool_name.as_deref(), &p.message, &p.severity, p.rating, p.agent_id.as_deref(),
            )?;
            Ok(ok_json(serde_json::json!({ "feedback_id": id })))
        })
    }

    /// List all feedback entries.
    #[tool(description = "List all feedback entries, newest first.")]
    fn list_feedback(&self) -> ToolResult {
        tool_op(|conn| {
            let fb = el::list_feedback(conn)?;
            Ok(ok_json(serde_json::json!({ "feedback": fb })))
        })
    }
}

// ---------- ServerHandler ----------

#[tool_handler]
impl ServerHandler for EmailServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(
            ServerCapabilities::builder()
                .enable_tools()
                .build(),
        )
        .with_server_info(Implementation::from_build_env())
        .with_protocol_version(ProtocolVersion::V_2024_11_05)
        .with_instructions(
            "redline: a local voice-learning system for email agents.\n\
             \nWORKFLOW (one pass through the loop):\n\
             1. create_draft — writes the draft AND returns all voice patterns + lint\n\
                violations. Fix violations with save_revision before the user sees it.\n\
             2. The user edits the draft in the Tauri app (outside this session).\n\
             3. finalize_draft — stores the (original → edited) pair AND returns diff\n\
                analysis (deletions, word swaps, pattern hits) + auto-promoted patterns.\n\
                Derive voice lessons from the analysis and store them with add_lesson\n\
                + add_pattern. Patterns auto-promote to 'confirmed' after 3+ occurrences.\n\
             \nLinting is automatic — create_draft and save_revision both return violations.\n\
             No separate lint call needed. Patterns are the write loop: they catch voice\n\
             issues before the user sees the draft.\n\
             \nGive feedback with give_feedback if anything is painful. One DB is shared\n\
             by this server, the CLI, and the Tauri app."
                .to_string(),
        )
    }
}

// ---------- helpers ----------

/// Open the DB, run `f` against the connection, and map every failure (DB open
/// or the op itself) to a caller-visible error result. `?` inside `f` works on
/// `anyhow::Result`, so tools read naturally.
fn tool_op<F>(f: F) -> ToolResult
where
    F: FnOnce(&el::Connection) -> anyhow::Result<CallToolResult>,
{
    match el::connect() {
        Ok(conn) => match f(&conn) {
            Ok(r) => Ok(r),
            Err(e) => Ok(err_text(format!("{e:#}"))),
        },
        Err(e) => Ok(err_text(format!(
            "failed to open email DB at {}: {e:#}",
            el::db_path().display()
        ))),
    }
}

fn ok_text(s: impl Into<String>) -> CallToolResult {
    CallToolResult::success(vec![ContentBlock::text(s.into())])
}

fn ok_json(v: impl serde::Serialize) -> CallToolResult {
    match serde_json::to_string_pretty(&v) {
        Ok(s) => ok_text(s),
        Err(e) => err_text(format!("serialize error: {e}")),
    }
}

fn err_text(msg: impl Into<String>) -> CallToolResult {
    CallToolResult::error(vec![ContentBlock::text(msg.into())])
}

fn preview(s: &str, n: usize) -> String {
    let collapsed: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.chars().count() <= n {
        collapsed
    } else {
        let cut: String = collapsed.chars().take(n).collect();
        format!("{cut}…")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preview_truncates_and_collapses() {
        assert_eq!(preview("hello   world\nfoo", 20), "hello world foo");
        let long = "word ".repeat(50);
        let p = preview(&long, 10);
        assert!(p.ends_with('…'));
        assert_eq!(p.chars().count(), 11); // 10 + ellipsis
    }
}
