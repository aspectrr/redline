// Tauri commands over the shared `email_learn` library. Every command opens its
// own connection (cheap for a local SQLite file; WAL lets the CLI and UI share).

use email_learn as el;

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

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![
            list_drafts,
            get_draft,
            create_draft,
            save_revision,
            restore_revision,
            finalize_draft,
            update_draft_meta,
            delete_draft,
            list_pairs,
            show_pair,
            list_lessons,
            add_lesson,
            search,
            summarize_lessons,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
