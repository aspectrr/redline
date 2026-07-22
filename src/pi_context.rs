// Pi-specific context capture. Programmatically detects the active pi session,
// extracts the transcript, and reads model config — without relying on the
// agent to pass any of it in.
//
// Detection chain:
// 1. PI_CODING_AGENT env confirms we're inside pi
// 2. ORCA_PI_SOURCE_AGENT_DIR (or ~/.pi/agent fallback) gives the agent dir
// 3. CWD → derive session directory path (~/.pi/agent/sessions/-{cwd-encoded}-)
// 4. Most recent .jsonl in that dir = active session
// 5. Session .jsonl contains: conversation messages + model_change events
//
// Model config:
// 1. ~/.pi/agent/settings.json → defaultProvider, defaultModel
// 2. ~/.pi/agent/models.json → provider config (baseUrl, apiKey, api)
// 3. Cross-reference: session .jsonl model_change events for the model actually used

use crate::ModelConfig;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Pi session detection result.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct PiSession {
    pub session_id: String,
    pub session_file: PathBuf,
    pub cwd: String,
    pub provider: Option<String>,
    pub model: Option<String>,
}

/// Detect the active pi session from the current environment.
/// Returns None if not running inside pi or no session found.
pub fn detect_session() -> Option<PiSession> {
    if std::env::var("PI_CODING_AGENT").is_err() {
        return None;
    }

    let agent_dir = pi_agent_dir();
    let sessions_dir = agent_dir.join("sessions");
    let cwd = std::env::current_dir().ok()?;

    // Session dir naming: replace / with -, wrap in -- and --
    let encoded = cwd.to_string_lossy().replace('/', "-");
    let session_dir_name = format!("-{encoded}-");
    let session_dir = sessions_dir.join(&session_dir_name);

    if !session_dir.exists() {
        return None;
    }

    // Find the most recently modified .jsonl file
    let mut entries: Vec<_> = std::fs::read_dir(&session_dir).ok()?
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.path().extension().map(|ext| ext == "jsonl").unwrap_or(false)
        })
        .collect();

    // Sort by modification time, newest first
    entries.sort_by(|a, b| {
        b.metadata().and_then(|m| m.modified()).ok()
            .cmp(&a.metadata().and_then(|m| m.modified()).ok())
    });

    let session_entry = entries.first()?;
    let session_file = session_entry.path();

    // Read the first line to get session metadata
    let first_line = std::fs::read_to_string(&session_file)
        .ok()?
        .lines()
        .next()?
        .to_string();

    let session_meta: serde_json::Value = serde_json::from_str(&first_line).ok()?;
    if session_meta.get("type").and_then(|t| t.as_str()) != Some("session") {
        return None;
    }

    let session_id = session_meta.get("id")?.as_str()?.to_string();
    let session_cwd = session_meta.get("cwd")?.as_str()?.to_string();

    // Scan for the latest model_change event
    let (provider, model) = scan_model_from_session(&session_file);

    Some(PiSession {
        session_id,
        session_file,
        cwd: session_cwd,
        provider,
        model,
    })
}

/// Extract a compact transcript from a session .jsonl file.
/// Includes user messages, assistant text, and tool call summaries.
/// Excludes thinking blocks (too verbose for derivation context).
pub fn extract_transcript(session_file: &std::path::Path) -> Option<String> {
    let content = std::fs::read_to_string(session_file).ok()?;
    let mut transcript = String::new();

    for line in content.lines() {
        let entry: serde_json::Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => continue,
        };

        let entry_type = entry.get("type").and_then(|t| t.as_str()).unwrap_or("");

        match entry_type {
            "model_change" => {
                let provider = entry.get("provider").and_then(|v| v.as_str()).unwrap_or("?");
                let model = entry.get("modelId").and_then(|v| v.as_str()).unwrap_or("?");
                transcript.push_str(&format!("\n[model: {provider}/{model}]\n"));
            }
            "message" => {
                let msg = match entry.get("message") {
                    Some(m) => m,
                    None => continue,
                };
                let role = msg.get("role").and_then(|r| r.as_str()).unwrap_or("unknown");
                let content_arr = match msg.get("content").and_then(|c| c.as_array()) {
                    Some(a) => a,
                    None => continue,
                };

                for block in content_arr {
                    let block_type = block.get("type").and_then(|t| t.as_str()).unwrap_or("");
                    match block_type {
                        "text" => {
                            let text = block.get("text").and_then(|t| t.as_str()).unwrap_or("");
                            if !text.is_empty() {
                                let label = if role == "user" { "User" } else { "Assistant" };
                                transcript.push_str(&format!("\n[{label}]: {text}\n"));
                            }
                        }
                        "toolCall" => {
                            let name = block.get("name").and_then(|n| n.as_str()).unwrap_or("unknown");
                            let args = block.get("arguments")
                                .and_then(|a| serde_json::to_string(a).ok())
                                .unwrap_or_default();
                            // ponytail: truncate long tool args — derivation needs to know
                            // what was called, not the full payload
                            let args_preview = if args.len() > 200 {
                                format!("{}...", &args[..200])
                            } else {
                                args
                            };
                            transcript.push_str(&format!("\n[tool_call: {name}({args_preview})]\n"));
                        }
                        "toolResult" => {
                            let name = block.get("toolName").and_then(|n| n.as_str()).unwrap_or("unknown");
                            // ponytail: skip tool result bodies — they're too verbose and
                            // the tool call name is the signal that matters
                            transcript.push_str(&format!("\n[tool_result: {name}]\n"));
                        }
                        // skip thinking blocks, reasoning_content — too verbose, not useful
                        // for derivation context
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }

    if transcript.is_empty() {
        None
    } else {
        Some(transcript)
    }
}

/// Read model config from pi's configuration files.
/// Chain: settings.json → models.json → auth.json
pub fn detect_model_config() -> Option<ModelConfig> {
    let agent_dir = pi_agent_dir();

    // 1. Read settings.json for default provider + model
    let settings_path = agent_dir.join("settings.json");
    let settings: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(&settings_path).ok()?
    ).ok()?;

    let provider = settings.get("defaultProvider")?.as_str()?.to_string();
    let model = settings.get("defaultModel")?.as_str()?.to_string();

    // 2. Read models.json for provider config (baseUrl, apiKey)
    let models_path = agent_dir.join("models.json");
    let models_data: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(&models_path).ok()?
    ).ok()?;

    let provider_config = models_data
        .get("providers")?
        .get(&provider)?;

    let base_url = provider_config.get("baseUrl")?.as_str()?.to_string();
    let api_key = provider_config.get("apiKey").and_then(|k| k.as_str()).map(String::from);

    // If no API key in models.json, try auth.json
    let api_key = match api_key {
        Some(k) => Some(k),
        None => {
            let auth_path = agent_dir.join("auth.json");
            if let Ok(auth_str) = std::fs::read_to_string(&auth_path) {
                let auth: serde_json::Value = serde_json::from_str(&auth_str).ok()?;
                auth.get(&provider)?
                    .get("key")?
                    .as_str()
                    .map(String::from)
            } else {
                None
            }
        }
    };

    Some(ModelConfig {
        provider,
        model,
        api_key,
        base_url,
    })
}

/// Get the pi agent directory from env or default.
pub fn pi_agent_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("ORCA_PI_SOURCE_AGENT_DIR") {
        return PathBuf::from(dir);
    }
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    home.join(".pi").join("agent")
}

/// Scan a session .jsonl for the latest model_change event.
/// Returns (provider, modelId).
fn scan_model_from_session(session_file: &std::path::Path) -> (Option<String>, Option<String>) {
    let content = match std::fs::read_to_string(session_file) {
        Ok(c) => c,
        Err(_) => return (None, None),
    };

    let mut provider = None;
    let mut model = None;

    for line in content.lines() {
        let entry: serde_json::Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => continue,
        };

        if entry.get("type").and_then(|t| t.as_str()) == Some("model_change") {
            if let Some(p) = entry.get("provider").and_then(|v| v.as_str()) {
                provider = Some(p.to_string());
            }
            if let Some(m) = entry.get("modelId").and_then(|v| v.as_str()) {
                model = Some(m.to_string());
            }
        }
    }

    (provider, model)
}

/// Auto-capture the transcript for a draft. Called from create_draft.
/// Returns the transcript string if we're in pi and a session exists.
pub fn auto_capture_transcript() -> Option<String> {
    let session = detect_session()?;
    extract_transcript(&session.session_file)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_dir_naming() {
    // The session dir naming replaces / with - in the cwd path
    let cwd = "/Users/test/projects/my-app";
    let encoded = cwd.replace('/', "-");
    let dir_name = format!("-{encoded}-");
    assert_eq!(dir_name, "--Users-test-projects-my-app-");
    }

    #[test]
    fn pi_agent_dir_uses_env() {
    std::env::set_var("ORCA_PI_SOURCE_AGENT_DIR", "/custom/pi/agent");
    assert_eq!(pi_agent_dir(), PathBuf::from("/custom/pi/agent"));
    std::env::remove_var("ORCA_PI_SOURCE_AGENT_DIR");
    }
}
