// email-learn CLI: thin wrapper over the `email_learn` library so both this
// binary and the `email-app` Tauri UI share one implementation.

use clap::{Parser, Subcommand};
use email_learn as el;
use std::path::PathBuf;
use std::process::ExitCode;

#[derive(Parser)]
#[command(name = "email-learn", version, about = "Store (draft, final) email pairs + voice lessons for agent learning.")]
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
    /// Default noop stub; wire a provider behind EMAIL_LEARN_LLM later.
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
            let id = el::create_draft(&conn, &body, context.as_deref(), &tags, &source)?;
            println!("{id}");
        }
        Cmd::Finalize { draft_id } => {
            let pair_id = el::finalize_draft(&conn, draft_id)?;
            println!("{pair_id}");
        }
        Cmd::Drafts { all } => {
            let out = el::list_drafts(&conn, all)?;
            println!("{}", serde_json::to_string_pretty(&out)?);
        }
        Cmd::DeleteDraft { draft_id } => {
            el::delete_draft(&conn, draft_id)?;
            println!("deleted draft {draft_id}");
        }
    }
    Ok(())
}
