// Async lesson derivation. When a draft is finalized, a derivation job is
// enqueued. A background task (in the Tauri app, or `redline derive` CLI)
// picks up pending jobs, calls an LLM with the diff analysis + tool
// definitions, and the LLM stores lessons/patterns/feedback via tool calls.
//
// The LLM sees the structured diff — not the agent's original conversation.
// The diff IS the signal: deletions, word swaps, categorized changes. The
// LLM decides how many lessons to create and whether to add lint patterns.

use crate as el;
use crate::{Connection, ModelConfig};

use rusqlite::params;
use serde::{Deserialize, Serialize};

// ---------- job management ----------

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct DerivationJob {
    pub id: i64,
    pub pair_id: i64,
    pub status: String, // pending | processing | done | failed
    pub attempts: i64,
    pub error: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

/// Enqueue a derivation job for a finalized pair. Called on finalize.
/// Idempotent — if a job already exists for this pair, does nothing.
pub fn enqueue(conn: &Connection, pair_id: i64) -> anyhow::Result<()> {
    let now = el::now_iso();
    conn.execute(
        "INSERT OR IGNORE INTO derivation_jobs (pair_id, status, attempts, created_at, updated_at)
         VALUES (?1, 'pending', 0, ?2, ?2)",
        params![pair_id, now],
    )?;
    Ok(())
}

/// Fetch pending jobs (status = pending, or failed with < 3 attempts).
pub fn pending_jobs(conn: &Connection) -> anyhow::Result<Vec<DerivationJob>> {
    let mut stmt = conn.prepare(
        "SELECT id, pair_id, status, attempts, error, created_at, updated_at
         FROM derivation_jobs
         WHERE status = 'pending'
            OR (status = 'failed' AND attempts < 3)
         ORDER BY id ASC",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok(DerivationJob {
            id: r.get(0)?,
            pair_id: r.get(1)?,
            status: r.get(2)?,
            attempts: r.get(3)?,
            error: r.get(4)?,
            created_at: r.get(5)?,
            updated_at: r.get(6)?,
        })
    })?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r?);
    }
    Ok(out)
}

fn mark_processing(conn: &Connection, job_id: i64) -> anyhow::Result<()> {
    let now = el::now_iso();
    conn.execute(
        "UPDATE derivation_jobs SET status = 'processing', updated_at = ?1 WHERE id = ?2",
        params![now, job_id],
    )?;
    Ok(())
}

fn mark_done(conn: &Connection, job_id: i64) -> anyhow::Result<()> {
    let now = el::now_iso();
    conn.execute(
        "UPDATE derivation_jobs SET status = 'done', updated_at = ?1 WHERE id = ?2",
        params![now, job_id],
    )?;
    Ok(())
}

fn mark_failed(conn: &Connection, job_id: i64, err: &str) -> anyhow::Result<()> {
    let now = el::now_iso();
    conn.execute(
        "UPDATE derivation_jobs
         SET status = CASE WHEN attempts >= 2 THEN 'failed' ELSE 'pending' END,
             attempts = attempts + 1,
             error = ?1,
             updated_at = ?2
         WHERE id = ?3",
        params![err, now, job_id],
    )?;
    Ok(())
}

// ---------- LLM call + tool execution ----------

/// Build the system prompt for derivation.
fn system_prompt() -> String {
    "You are a writing voice analyzer for redline, a system that learns how \
     a user writes by studying their edits.\n\
     \nYou will be given a draft that an AI assistant wrote, the final version \
     after the user edited it, and a structured diff analysis showing what \
     changed.\n\
     \nYour task: derive concrete, specific voice lessons from the edits. \
     Focus on:\n\
     - Deletions (what got cut — strongest signal)\n\
     - Word swaps (specific before→after replacements)\n\
     - Categorized changes (structural, stylistic, factual, punctuation)\n\
     \nUse the provided tools to store each lesson and create lint patterns.\n\
     A good lesson is specific and actionable:\n\
     ✅ \"Use 'Quick note' instead of 'I wanted to reach out'\"\n\
     ✅ \"No em-dashes — use periods\"\n\
     ❌ \"Be clear and concise\" (too generic)\n\
     \nStore 1-3 lessons per pair. Always create a pattern alongside each \
     lesson — patterns are what the lint engine uses to catch issues in \
     future drafts. Don't store a lesson if you can't name what specifically \
     changed. Don't duplicate existing lessons."
        .into()
}

/// Build the user message from a pair + its analysis.
fn user_message(conn: &Connection, pair_id: i64) -> anyhow::Result<String> {
    let pair = el::show_pair(conn, pair_id)?
        .ok_or_else(|| anyhow::anyhow!("pair {} not found", pair_id))?;

    let analysis = el::analyze_diff(conn, pair_id)?;

    let mut msg = String::new();
    if let Some(ctx) = &pair.context {
        msg.push_str(&format!("Context: {ctx}\n"));
    }
    if !pair.tags.is_empty() {
        msg.push_str(&format!("Tags: {}\n", pair.tags.join(", ")));
    }

    msg.push_str("\n## Draft (AI wrote this):\n");
    msg.push_str(&pair.draft);
    if !pair.draft.ends_with('\n') {
        msg.push('\n');
    }

    msg.push_str("\n## Final (user edited to this):\n");
    msg.push_str(&pair.final_);
    if !pair.final_.ends_with('\n') {
        msg.push('\n');
    }

    if let Some(a) = analysis {
        msg.push_str("\n## Diff Analysis:\n");

        if !a.deletions.is_empty() {
            msg.push_str("\n### Deletions:\n");
            for d in &a.deletions {
                msg.push_str(&format!("- \"{d}\"\n"));
            }
        }
        if !a.word_swaps.is_empty() {
            msg.push_str("\n### Word swaps:\n");
            for (old, new) in &a.word_swaps {
                msg.push_str(&format!("- \"{old}\" → \"{new}\"\n"));
            }
        }
        if !a.categorized.is_empty() {
            msg.push_str("\n### Categorized changes:\n");
            for c in &a.categorized {
                msg.push_str(&format!("- [{}] {}\n", c.category, c.description));
            }
        }
    }

    // existing lessons so the LLM doesn't duplicate
    let existing = el::lessons(conn, &[])?;
    if !existing.is_empty() {
        msg.push_str("\n## Existing lessons (don't duplicate):\n");
        for l in &existing {
            msg.push_str(&format!("- {}\n", l.lesson));
        }
    }

    // Include the captured agent transcript if available — this gives the
    // deriving LLM the full context: what was asked, what the agent reasoned,
    // what tools it called. This is the key signal for distinguishing intent
    // corrections from execution refinements.
    if let Some(transcript) = el::get_pair_transcript(conn, pair_id)? {
        msg.push_str("\n## Agent context (conversation that produced the draft):\n");
        msg.push_str(&transcript);
        if !transcript.ends_with('\n') {
            msg.push('\n');
        }
        msg.push_str("\nUse this context to understand WHY the draft was written this way.\n");
        msg.push_str("Edits may correct the agent's understanding (intent) or refine its execution (style).\n");
    }

    Ok(msg)
}

/// Define the tools available to the derivation LLM.
/// OpenAI-compatible function-calling format.
fn tool_definitions() -> serde_json::Value {
    serde_json::json!([
        {
            "type": "function",
            "function": {
                "name": "add_lesson",
                "description": "Store a concrete voice lesson derived from this diff. Be specific.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "lesson": {
                            "type": "string",
                            "description": "Specific, actionable voice rule. e.g. \"Use 'Quick note' not 'I wanted to reach out'\""
                        },
                        "tags": {
                            "type": "array",
                            "items": {"type": "string"},
                            "description": "Content-type and context tags"
                        }
                    },
                    "required": ["lesson"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "add_pattern",
                "description": "Create a matchable lint pattern. Always create alongside a lesson. pattern_type='literal' for substring, 'regex' for Rust regex. direction='avoid' flags if found, 'prefer' flags if absent.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "rule": {"type": "string", "description": "Human-readable rule name"},
                        "pattern": {"type": "string", "description": "The match string (literal text or regex)"},
                        "pattern_type": {"type": "string", "enum": ["literal", "regex"], "default": "literal"},
                        "direction": {"type": "string", "enum": ["avoid", "prefer"], "default": "avoid"},
                        "category": {"type": "string", "enum": ["punctuation", "style", "structure", "factual", "deletion"], "default": "style"}
                    },
                    "required": ["rule", "pattern"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "add_feedback",
                "description": "Log feedback about the derivation process or the diff quality.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "message": {"type": "string"},
                        "severity": {"type": "string", "enum": ["info", "warning", "error", "suggestion"], "default": "info"}
                    },
                    "required": ["message"]
                }
            }
        }
    ])
}

// ---------- LLM API response types ----------

#[derive(Deserialize, Debug)]
struct ChatResponse {
    choices: Vec<ChatChoice>,
}

#[derive(Deserialize, Debug)]
struct ChatChoice {
    message: ChatMessage,
}

#[derive(Deserialize, Debug)]
struct ChatMessage {
    #[serde(default)]
    tool_calls: Vec<ToolCall>,
}

#[derive(Deserialize, Debug)]
struct ToolCall {
    function: ToolCallFunction,
}

#[derive(Deserialize, Debug)]
struct ToolCallFunction {
    name: String,
    arguments: String, // JSON string
}

/// Process a single derivation job: build prompt, call LLM, execute tool calls.
pub async fn process_job(conn: &Connection, job: &DerivationJob, config: &ModelConfig) -> anyhow::Result<usize> {
    let user_msg = user_message(conn, job.pair_id)?;

    let body = serde_json::json!({
        "model": config.model,
        "messages": [
            {"role": "system", "content": system_prompt()},
            {"role": "user", "content": user_msg},
        ],
        "tools": tool_definitions(),
        "tool_choice": "auto",
    });

    let url = format!("{}/chat/completions", config.base_url.trim_end_matches('/'));
    let client = reqwest::Client::new();
    let mut req = client.post(&url).json(&body);
    if let Some(key) = &config.api_key {
        req = req.bearer_auth(key);
    }

    let resp = req.send().await?;
    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        anyhow::bail!("LLM API error {status}: {text}");
    }

    let chat: ChatResponse = resp.json().await?;
    let tool_calls: Vec<&ToolCall> = chat.choices
        .first()
        .map(|c| c.message.tool_calls.iter().collect())
        .unwrap_or_default();

    let mut executed = 0;
    for tc in tool_calls {
        let args: serde_json::Value = serde_json::from_str(&tc.function.arguments)
            .unwrap_or(serde_json::json!({}));

        match tc.function.name.as_str() {
            "add_lesson" => {
                let lesson = args["lesson"].as_str().unwrap_or("");
                if lesson.is_empty() {
                    continue;
                }
                let tags: Vec<String> = args["tags"]
                    .as_array()
                    .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
                    .unwrap_or_default();
                el::add_lesson(conn, job.pair_id, lesson, &tags)?;
                // link a pattern if the LLM also called add_pattern separately
                executed += 1;
            }
            "add_pattern" => {
                let rule = args["rule"].as_str().unwrap_or("");
                let pattern = args["pattern"].as_str().unwrap_or("");
                if rule.is_empty() || pattern.is_empty() {
                    continue;
                }
                let pattern_type = args["pattern_type"].as_str().unwrap_or("literal");
                let direction = args["direction"].as_str().unwrap_or("avoid");
                let category = args["category"].as_str().unwrap_or("style");
                el::add_pattern(conn, None, rule, pattern, pattern_type, direction, category, None, None)?;
                executed += 1;
            }
            "add_feedback" => {
                let msg = args["message"].as_str().unwrap_or("");
                let severity = args["severity"].as_str().unwrap_or("info");
                if !msg.is_empty() {
                    el::add_feedback(conn, Some("deriver"), msg, severity, None, Some("auto-deriver"))?;
                }
                executed += 1;
            }
            other => {
                tracing::warn!("deriver: unknown tool call: {other}");
            }
        }
    }

    // If the LLM stored lessons but no patterns, that's a write-only graveyard.
    // Log it as feedback so it's visible.
    let lesson_count = count_lessons_for_pair(conn, job.pair_id)?;
    let pattern_count = count_patterns_for_pair(conn, job.pair_id)?;
    if lesson_count > 0 && pattern_count == 0 {
        el::add_feedback(
            conn, Some("deriver"),
            &format!("Pair {} has {lesson_count} lessons but 0 patterns — lessons without patterns won't lint", job.pair_id),
            "warning", None, Some("auto-deriver"),
        )?;
    }

    Ok(executed)
}

fn count_lessons_for_pair(conn: &Connection, pair_id: i64) -> anyhow::Result<i64> {
    Ok(conn.query_row(
        "SELECT COUNT(*) FROM lessons WHERE pair_id = ?1",
        params![pair_id], |r| r.get(0),
    )?)
}

fn count_patterns_for_pair(conn: &Connection, pair_id: i64) -> anyhow::Result<i64> {
    Ok(conn.query_row(
        "SELECT COUNT(*) FROM patterns p
         JOIN lessons l ON p.lesson_id = l.id
         WHERE l.pair_id = ?1",
        params![pair_id], |r| r.get(0),
    )?)
}

/// Process all pending derivation jobs. Returns (processed, succeeded, failed).
/// Called by the Tauri background task or the CLI. Auto-detects model config
/// from pi session if available, falls back to settings DB, then env vars.
pub async fn process_pending() -> (usize, usize, usize) {
    let conn = match el::connect() {
        Ok(c) => c,
        Err(e) => {
            tracing::error!("deriver: failed to open DB: {e}");
            return (0, 0, 0);
        }
    };

    // Auto-detect model config: pi session → settings DB → env vars
    let config = match el::auto_detect_model_config(&conn) {
        Some(c) => c,
        None => {
            tracing::debug!("deriver: no model config available (not in pi, no settings, no env) — skipping");
            return (0, 0, 0);
        }
    };

    let jobs = match pending_jobs(&conn) {
        Ok(j) => j,
        Err(e) => {
            tracing::error!("deriver: failed to query pending jobs: {e}");
            return (0, 0, 0);
        }
    };

    let mut processed = 0;
    let mut succeeded = 0;
    let mut failed = 0;

    for job in &jobs {
        processed += 1;
        if let Err(e) = mark_processing(&conn, job.id) {
            tracing::error!("deriver: failed to mark processing: {e}");
            continue;
        }

        match process_job(&conn, job, &config).await {
            Ok(n) => {
                if let Err(e) = mark_done(&conn, job.id) {
                    tracing::error!("deriver: failed to mark done: {e}");
                }
                tracing::info!("deriver: pair {} done — {} tool calls executed", job.pair_id, n);
                succeeded += 1;
            }
            Err(e) => {
                let err_msg = format!("{e:#}");
                tracing::warn!("deriver: pair {} failed: {err_msg}", job.pair_id);
                if let Err(e2) = mark_failed(&conn, job.id, &err_msg) {
                    tracing::error!("deriver: failed to mark failed: {e2}");
                }
                failed += 1;
            }
        }
    }

    (processed, succeeded, failed)
}

/// How many pending jobs exist. Used for status reporting.
pub fn pending_count(conn: &Connection) -> anyhow::Result<i64> {
    Ok(conn.query_row(
        "SELECT COUNT(*) FROM derivation_jobs WHERE status IN ('pending', 'failed')",
        [], |r| r.get(0),
    )?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enqueue_and_pending_works() {
        let conn = el::connect_at(&std::path::PathBuf::from(":memory:")).unwrap();
        // Need a pair first
        let pair_id = el::add_pair(&conn, "draft", "final", None, &[]).unwrap();

        enqueue(&conn, pair_id).unwrap();
        let pending = pending_jobs(&conn).unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].pair_id, pair_id);
        assert_eq!(pending[0].status, "pending");

        // idempotent — second enqueue is a no-op
        enqueue(&conn, pair_id).unwrap();
        let pending2 = pending_jobs(&conn).unwrap();
        assert_eq!(pending2.len(), 1);
    }

    #[test]
    fn mark_done_removes_from_pending() {
        let conn = el::connect_at(&std::path::PathBuf::from(":memory:")).unwrap();
        let pair_id = el::add_pair(&conn, "draft", "final", None, &[]).unwrap();
        enqueue(&conn, pair_id).unwrap();
        mark_done(&conn, 1).unwrap();
        assert!(pending_jobs(&conn).unwrap().is_empty());
    }

    #[test]
    fn mark_failed_retries_until_limit() {
        let conn = el::connect_at(&std::path::PathBuf::from(":memory:")).unwrap();
        let pair_id = el::add_pair(&conn, "draft", "final", None, &[]).unwrap();
        enqueue(&conn, pair_id).unwrap();

        // attempt 1: stays pending (attempts 0 → 1, status back to pending)
        mark_failed(&conn, 1, "err").unwrap();
        assert_eq!(pending_jobs(&conn).unwrap().len(), 1);

        // attempt 2: stays pending (attempts 1 → 2, status back to pending)
        mark_failed(&conn, 1, "err").unwrap();
        assert_eq!(pending_jobs(&conn).unwrap().len(), 1);

        // attempt 3: marks failed (attempts 2 → 3, status → failed)
        mark_failed(&conn, 1, "err").unwrap();
        // failed with attempts >= 3 drops off pending
        assert_eq!(pending_jobs(&conn).unwrap().len(), 0);
    }
}
