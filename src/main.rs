// redline CLI: thin wrapper over the `redline` library so both this
// binary and the `redline-app` Tauri UI share one implementation.

use clap::{Parser, Subcommand};
use redline as el;
use std::path::PathBuf;
use std::process::ExitCode;

#[derive(Parser)]
#[command(name = "redline", version, about = "Store (draft, final) email pairs + voice lessons for agent learning.")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Store a new (draft, final) email pair. Prints the new pair id.
    Add {
        draft_path: PathBuf,
        final_path: PathBuf,
        #[arg(long)]
        context: Option<String>,
        #[arg(long, value_delimiter = ',')]
        tags: Vec<String>,
    },
    /// Show one pair (draft, final, diff).
    Show { id: i64 },
    /// List the N most recent pairs.
    #[command(alias = "ls")]
    Recent {
        #[arg(default_value = "10")]
        n: usize,
    },
    /// List stored voice lessons.
    Lessons {
        #[arg(long, value_delimiter = ',')]
        tags: Vec<String>,
    },
    /// Store a lesson derived from a pair.
    AddLesson {
        pair_id: i64,
        lesson: String,
        #[arg(long, value_delimiter = ',')]
        tags: Vec<String>,
    },
    /// Full-text-ish search across pairs and lessons.
    Query { needle: String },
    /// Dump everything as markdown.
    Export,

    /// Summarize/audit stored lessons via the (optional) LLM seam.
    /// Default noop stub; wire a provider behind REDLINE_LLM later.
    Summarize,

    // --- drafting surface (agent ingest) ---
    /// Create a new in-flight draft from stdin or a file. Prints the new draft id.
    /// Agent ingest path: the Tauri UI picks this up for editing.
    Draft {
        /// Read draft body from this file; use `-` for stdin.
        path: PathBuf,
        #[arg(long)]
        context: Option<String>,
        #[arg(long, value_delimiter = ',')]
        tags: Vec<String>,
        #[arg(long, default_value = "agent")]
        source: String,
    },
    /// Finalize a draft (latest revision vs first → pair). Prints the pair id.
    Finalize { draft_id: i64 },
    /// List drafts (add --all to include finalized).
    Drafts {
        #[arg(long)]
        all: bool,
    },
    /// Delete a draft and its revisions (a finalized pair, if any, is kept).
    DeleteDraft { draft_id: i64 },
    /// Delete a pair. Derived lessons are unlinked (kept), not deleted.
    DeletePair { pair_id: i64 },
    /// Delete a single lesson by id.
    DeleteLesson { lesson_id: i64 },
    /// Run as an MCP server over stdio. Agents (pi, Claude, …) connect to this
    /// and get every CLI feature as tools: read pairs/diffs/lessons, record
    /// derived lessons, push and edit drafts, search.
    Mcp,

    // --- lint ---
    /// Scan a draft (or raw text) against stored voice patterns.
    /// Prints violations as JSON. Key write-loop entry point.
    Lint {
        /// Draft id to lint (uses latest revision content). Mutually exclusive with --text.
        draft_id: Option<i64>,
        #[arg(long)]
        text: Option<String>,
    },
    /// Add a matchable voice pattern for the lint engine.
    AddPattern {
        #[arg(long)]
        rule: String,
        #[arg(long)]
        pattern: String,
        #[arg(long, default_value = "literal")]
        pattern_type: String,
        #[arg(long, default_value = "avoid")]
        direction: String,
        #[arg(long, default_value = "style")]
        category: String,
        #[arg(long)]
        lesson_id: Option<i64>,
        #[arg(long)]
        before: Option<String>,
        #[arg(long)]
        after: Option<String>,
    },
    /// List stored voice patterns (optionally filtered by lesson).
    ListPatterns {
        #[arg(long)]
        lesson_id: Option<i64>,
    },
    /// Delete a pattern by id.
    DeletePattern { pattern_id: i64 },
    /// Analyze a finalized pair's diff: surface deletions, additions, word
    /// swaps, and existing-pattern hits. Data for deriving new patterns.
    Analyze { pair_id: i64 },

    // --- feedback ---
    /// Log feedback (from an agent or human) about the tool.
    Feedback {
        message: String,
        #[arg(long)]
        tool: Option<String>,
        #[arg(long, default_value = "info")]
        severity: String,
        #[arg(long)]
        rating: Option<i64>,
    },
    /// List all feedback entries.
    FeedbackList,
    /// Manually trigger pattern promotion. Scans all pairs, auto-promotes
    /// patterns with 3+ draft occurrences to 'confirmed'.
    Promote,
    /// Process pending derivation jobs. Requires REDLINE_MODEL_PROVIDER,
    /// REDLINE_MODEL_NAME, and optionally REDLINE_API_KEY env vars.
    /// Processes all pending jobs once, or polls every 30s with --watch.
    Derive {
        #[arg(long)]
        watch: bool,
    },
}

fn read_text(path: &PathBuf) -> anyhow::Result<String> {
    if path == std::path::Path::new("-") {
        use std::io::Read;
        let mut s = String::new();
        std::io::stdin().read_to_string(&mut s)?;
        Ok(s)
    } else {
        std::fs::read_to_string(path).map_err(|e| anyhow::anyhow!("read {}: {e}", path.display()))
    }
}

/// Render a pair as JSON with a readable plain diff (the stored `diff` field is
/// structured JSON for the UI; the CLI surfaces plain text for humans/agents).
fn pair_json(p: &el::Pair) -> serde_json::Value {
    serde_json::json!({
        "id": p.id,
        "draft": p.draft,
        "final": p.final_,
        "diff": el::render_diff_plain(&el::rich_diff(&p.draft, &p.final_)),
        "context": p.context,
        "tags": p.tags,
        "created_at": p.created_at,
    })
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let conn = el::connect()?;
    match cli.cmd {
        Cmd::Add { draft_path, final_path, context, tags } => {
            let draft = read_text(&draft_path)?;
            let final_ = read_text(&final_path)?;
            let id = el::add_pair(&conn, &draft, &final_, context.as_deref(), &tags)?;
            println!("{id}");
        }
        Cmd::Show { id } => match el::show_pair(&conn, id)? {
            Some(p) => println!("{}", serde_json::to_string_pretty(&pair_json(&p))?),
            None => eprintln!("no pair with id {id}"),
        },
        Cmd::Recent { n } => {
            let out: Vec<_> = el::recent_pairs(&conn, n)?.iter().map(pair_json).collect();
            println!("{}", serde_json::to_string_pretty(&out)?);
        }
        Cmd::Lessons { tags } => {
            let out = el::lessons(&conn, &tags)?;
            println!("{}", serde_json::to_string_pretty(&out)?);
        }
        Cmd::AddLesson { pair_id, lesson, tags } => {
            let id = el::add_lesson(&conn, pair_id, &lesson, &tags)?;
            println!("{id}");
        }
        Cmd::Query { needle } => {
            let (pairs, lessons) = el::query(&conn, &needle)?;
            let pairs: Vec<_> = pairs.iter().map(pair_json).collect();
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "pairs": pairs,
                    "lessons": lessons,
                }))?
            );
        }
        Cmd::Export => {
            print!("{}", el::export_md(&conn)?);
        }

        Cmd::Summarize => {
            print!("{}", el::summarize_lessons(&conn)?);
        }

        Cmd::Draft { path, context, tags, source } => {
            let body = read_text(&path)?;
            let ctx = el::create_draft_with_context(&conn, &body, context.as_deref(), &tags, &source, None)?;
            println!("{}", serde_json::to_string_pretty(&serde_json::json!({
                "draft_id": ctx.draft_id,
                "patterns": ctx.patterns,
                "violations": ctx.violations,
                "pending_lessons": ctx.pending_lessons,
            }))?);
        }
        Cmd::Finalize { draft_id } => {
            let result = el::finalize_draft_with_analysis(&conn, draft_id)?;
            println!("{}", serde_json::to_string_pretty(&serde_json::json!({
                "pair_id": result.pair_id,
                "analysis": result.analysis,
                "promoted_patterns": result.promoted_patterns,
            }))?);
        }
        Cmd::Drafts { all } => {
            let out = el::list_drafts(&conn, all)?;
            println!("{}", serde_json::to_string_pretty(&out)?);
        }
        Cmd::DeleteDraft { draft_id } => {
            el::delete_draft(&conn, draft_id)?;
            println!("deleted draft {draft_id}");
        }
        Cmd::DeletePair { pair_id } => {
            el::delete_pair(&conn, pair_id)?;
            println!("deleted pair {pair_id}");
        }
        Cmd::DeleteLesson { lesson_id } => {
            el::delete_lesson(&conn, lesson_id)?;
            println!("deleted lesson {lesson_id}");
        }

        Cmd::Lint { draft_id, text } => {
            let content = if let Some(t) = text {
                t
            } else if let Some(id) = draft_id {
                match el::get_draft(&conn, id)? {
                    Some(d) => d.revisions.last().map(|r| r.content.clone())
                        .unwrap_or_default(),
                    None => anyhow::bail!("no draft with id {id}"),
                }
            } else {
                anyhow::bail!("provide a draft id or --text");
            };
            let violations = el::lint_draft(&conn, &content)?;
            println!("{}", serde_json::to_string_pretty(&violations)?);
        }
        Cmd::AddPattern { rule, pattern, pattern_type, direction, category, lesson_id, before, after } => {
            let id = el::add_pattern(
                &conn, lesson_id, &rule, &pattern, &pattern_type, &direction, &category,
                before.as_deref(), after.as_deref(),
            )?;
            println!("{id}");
        }
        Cmd::ListPatterns { lesson_id } => {
            let out = el::list_patterns(&conn, lesson_id)?;
            println!("{}", serde_json::to_string_pretty(&out)?);
        }
        Cmd::DeletePattern { pattern_id } => {
            el::delete_pattern(&conn, pattern_id)?;
            println!("deleted pattern {pattern_id}");
        }
        Cmd::Analyze { pair_id } => {
            match el::analyze_diff(&conn, pair_id)? {
                Some(a) => println!("{}", serde_json::to_string_pretty(&a)?),
                None => eprintln!("no pair with id {pair_id}"),
            }
        }
        Cmd::Feedback { message, tool, severity, rating } => {
            let id = el::add_feedback(&conn, tool.as_deref(), &message, &severity, rating, None)?;
            println!("{id}");
        }
        Cmd::FeedbackList => {
            let out = el::list_feedback(&conn)?;
            println!("{}", serde_json::to_string_pretty(&out)?);
        }
        Cmd::Promote => {
            let promoted = el::promote_patterns(&conn)?;
            if promoted.is_empty() {
                println!("No patterns promoted.");
            } else {
                for (id, rule, count, pairs) in &promoted {
                    println!("promoted #{id} \"{rule}\" — {count} occurrences in pairs {:?}", pairs);
                }
            }
        }
        Cmd::Mcp => {
            // The MCP server speaks JSON-RPC over stdio and needs the tokio
            // runtime. Every other subcommand stays synchronous.
            tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()?
                .block_on(el::mcp::serve())?;
        }
        Cmd::Derive { watch } => {
            let rt = tokio::runtime::Runtime::new()?;
            rt.block_on(async {
                loop {
                    let (processed, succeeded, failed) = el::deriver::process_pending().await;
                    if processed > 0 {
                        println!("derived: {succeeded}/{processed} succeeded, {failed} failed");
                    } else if !watch {
                        println!("no pending derivation jobs");
                    }
                    if !watch {
                        break;
                    }
                    tokio::time::sleep(std::time::Duration::from_secs(30)).await;
                }
            });
        }
    }
    Ok(())
}
