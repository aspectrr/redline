// Tauri commands over the shared `email_learn` library. Every command opens its
// own connection (cheap for a local SQLite file; WAL lets the CLI and UI share).

use email_learn as el;
use tauri::Emitter;

type CmdResult<T> = Result<T, String>;

fn conn() -> CmdResult<el::Connection> {
    el::connect().map_err(|e| format!("{e:#}"))
}

/// Map any error into a string for the frontend.
fn s<T, E: std::fmt::Debug>(r: Result<T, E>) -> CmdResult<T> {
    r.map_err(|e| format!("{e:?}"))
}

#[tauri::command]
fn list_drafts(include_finalized: Option<bool>) -> CmdResult<Vec<el::Draft>> {
    let c = conn()?;
    s(el::list_drafts(&c, include_finalized.unwrap_or(false)))
}

#[tauri::command]
fn get_draft(id: i64) -> CmdResult<Option<el::DraftWithRevisions>> {
    let c = conn()?;
    s(el::get_draft(&c, id))
}

#[tauri::command]
fn create_draft(
    content: String,
    context: Option<String>,
    tags: Vec<String>,
    source: Option<String>,
) -> CmdResult<i64> {
    let c = conn()?;
    s(el::create_draft(
        &c,
        &content,
        context.as_deref(),
        &tags,
        source.as_deref().unwrap_or("agent"),
    ))
}

#[tauri::command]
fn save_revision(draft_id: i64, content: String, source: Option<String>) -> CmdResult<i64> {
    let c = conn()?;
    s(el::save_revision(
        &c,
        draft_id,
        &content,
        source.as_deref().unwrap_or("user"),
    ))
}

#[tauri::command]
fn restore_revision(draft_id: i64, revision_id: i64) -> CmdResult<i64> {
    let c = conn()?;
    s(el::restore_revision(&c, draft_id, revision_id))
}

#[tauri::command]
fn finalize_draft(draft_id: i64) -> CmdResult<i64> {
    let c = conn()?;
    s(el::finalize_draft(&c, draft_id))
}

#[tauri::command]
fn update_draft_meta(
    draft_id: i64,
    context: Option<String>,
    tags: Vec<String>,
) -> CmdResult<()> {
    let c = conn()?;
    s(el::update_draft_meta(&c, draft_id, context.as_deref(), &tags))
}

#[tauri::command]
fn delete_draft(draft_id: i64) -> CmdResult<()> {
    let c = conn()?;
    s(el::delete_draft(&c, draft_id))
}

#[tauri::command]
fn delete_pair(pair_id: i64) -> CmdResult<()> {
    let c = conn()?;
    s(el::delete_pair(&c, pair_id))
}

#[tauri::command]
fn delete_lesson(lesson_id: i64) -> CmdResult<()> {
    let c = conn()?;
    s(el::delete_lesson(&c, lesson_id))
}

#[tauri::command]
fn list_pairs(limit: Option<i64>) -> CmdResult<Vec<el::Pair>> {
    let c = conn()?;
    s(el::recent_pairs(&c, limit.unwrap_or(50) as usize))
}

#[tauri::command]
fn show_pair(id: i64) -> CmdResult<Option<el::Pair>> {
    let c = conn()?;
    s(el::show_pair(&c, id))
}

#[tauri::command]
fn list_lessons(tags: Vec<String>) -> CmdResult<Vec<el::Lesson>> {
    let c = conn()?;
    s(el::lessons(&c, &tags))
}

#[tauri::command]
fn add_lesson(pair_id: i64, lesson: String, tags: Vec<String>) -> CmdResult<i64> {
    let c = conn()?;
    s(el::add_lesson(&c, pair_id, &lesson, &tags))
}

#[tauri::command]
fn search(needle: String) -> CmdResult<el::SearchResult> {
    let c = conn()?;
    s(el::search_all(&c, &needle))
}

#[tauri::command]
fn summarize_lessons() -> CmdResult<String> {
    let c = conn()?;
    s(el::summarize_lessons(&c))
}

#[tauri::command]
fn lint_draft(content: String) -> CmdResult<Vec<el::Violation>> {
    let c = conn()?;
    s(el::lint_draft(&c, &content))
}

#[tauri::command]
fn list_patterns() -> CmdResult<Vec<el::Pattern>> {
    let c = conn()?;
    s(el::list_patterns(&c, None))
}

#[tauri::command]
fn add_pattern(
    rule: String, pattern: String, pattern_type: String, direction: String, category: String,
    lesson_id: Option<i64>, before_text: Option<String>, after_text: Option<String>,
) -> CmdResult<i64> {
    let c = conn()?;
    s(el::add_pattern(
        &c, lesson_id, &rule, &pattern, &pattern_type, &direction, &category,
        before_text.as_deref(), after_text.as_deref(),
    ))
}

#[tauri::command]
fn delete_pattern(pattern_id: i64) -> CmdResult<()> {
    let c = conn()?;
    s(el::delete_pattern(&c, pattern_id))
}

#[tauri::command]
fn list_feedback() -> CmdResult<Vec<el::Feedback>> {
    let c = conn()?;
    s(el::list_feedback(&c))
}

#[tauri::command]
fn compute_diff(old: String, new: String, mode: Option<String>) -> CmdResult<String> {
    let rows = if mode.as_deref() == Some("sentences") {
        el::rich_diff_sentences(&old, &new)
    } else {
        el::rich_diff(&old, &new)
    };
    s(serde_json::to_string(&rows))
}

#[tauri::command]
fn analyze_pair(pair_id: i64) -> CmdResult<Option<serde_json::Value>> {
    let c = conn()?;
    let analysis = el::analyze_diff(&c, pair_id)
        .map_err(|e| format!("{e:?}"))?;
    let result = analysis.map(|x| serde_json::to_value(x).unwrap_or(serde_json::json!(null)));
    Ok(result)
}

use notify_debouncer_mini::{new_debouncer, DebounceEventResult};
use std::time::Duration;

/// Spawn a file watcher on the DB directory. Emits `db-changed` Tauri events
/// when the `.db` or `.db-wal` file is modified by an external process
/// (CLI or MCP server writing to the same shared DB).
fn spawn_db_watcher(app: tauri::AppHandle) {
    let db_path = el::db_path();
    let watch_dir = db_path.parent().map(|p| p.to_path_buf());

    let Some(watch_dir) = watch_dir else { return };

    let (tx, rx) = std::sync::mpsc::channel();
    let mut debouncer = match new_debouncer(Duration::from_millis(500), move |_ev: DebounceEventResult| {
        let _ = tx.send(());
    }) {
        Ok(d) => d,
        Err(_) => return,
    };

    if debouncer.watcher().watch(&watch_dir, notify::RecursiveMode::NonRecursive).is_err() {
        return;
    }

    std::thread::spawn(move || {
        // Keep debouncer alive — dropping it stops watching.
        let _debouncer = debouncer;
        while rx.recv().is_ok() {
            app.emit("db-changed", ()).ok();
        }
    });
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            spawn_db_watcher(app.handle().clone());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            list_drafts,
            get_draft,
            create_draft,
            save_revision,
            restore_revision,
            finalize_draft,
            update_draft_meta,
            delete_draft,
            delete_pair,
            delete_lesson,
            list_pairs,
            show_pair,
            list_lessons,
            add_lesson,
            search,
            summarize_lessons,
            lint_draft,
            list_patterns,
            add_pattern,
            delete_pattern,
            list_feedback,
            compute_diff,
            analyze_pair,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
