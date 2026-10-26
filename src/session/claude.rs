//! Claude Code sessions: discovery and parsing.
//!
//! Layout: `$CLAUDE_CONFIG_DIR/projects` (else `~/.claude/projects`), one
//! directory per project, one `.jsonl` file per session. Parsing turns each
//! line into the shared model; unknown events and broken lines are tolerated.

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;

use super::content::{first_block_text, parse_content};
use super::model::{Agent, ParseStats, Record, RecordKind, Session, SessionEntry, fold_lines};
use super::scan::{
    PreviewPicker, decode_project_dir, json_lines, last_active, session_id_from_stem,
};

/// Claude's session directory: `CLAUDE_CONFIG_DIR/projects`, else `~/.claude/projects`.
fn base_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("CLAUDE_CONFIG_DIR")
        && !dir.is_empty()
    {
        return PathBuf::from(dir).join("projects");
    }
    let home = std::env::var("HOME").unwrap_or_default();
    PathBuf::from(home).join(".claude").join("projects")
}

/// All Claude sessions, unordered. Unreadable dirs/files are skipped.
pub fn discover() -> Vec<SessionEntry> {
    discover_in(&base_dir())
}

fn discover_in(projects_dir: &Path) -> Vec<SessionEntry> {
    let mut entries = Vec::new();
    let Ok(project_dirs) = fs::read_dir(projects_dir) else {
        return entries;
    };
    for project_dir in project_dirs.flatten() {
        let dir_path = project_dir.path();
        if !dir_path.is_dir() {
            continue;
        }
        let Ok(files) = fs::read_dir(&dir_path) else {
            continue;
        };
        for file in files.flatten() {
            let path = file.path();
            if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }
            if let Some(entry) = scan_entry(&path, &dir_path) {
                entries.push(entry);
            }
        }
    }
    entries
}

/// Scan one Claude session file (used by `discover_dir`, which supplies files
/// from an arbitrary tree). The parent dir is the decode fallback for `cwd`.
pub(crate) fn scan(path: &Path) -> Option<SessionEntry> {
    scan_entry(path, path.parent().unwrap_or(path))
}

fn scan_entry(path: &Path, project_dir: &Path) -> Option<SessionEntry> {
    let last_active = last_active(path);
    let session_id = session_id_from_stem(path);

    let values = json_lines(path)?;
    let mut project: Option<String> = None;
    let mut preview = PreviewPicker::default();
    for value in values {
        if project.is_none()
            && let Some(cwd) = value.get("cwd").and_then(Value::as_str)
        {
            project = Some(cwd.to_string());
        }
        if let Some(text) = preview_text(&value) {
            preview.offer(text);
        } else if value.get("type").and_then(Value::as_str) == Some("assistant") {
            preview.reply_seen();
        }
        if project.is_some() && preview.has_summary() {
            break;
        }
    }

    Some(SessionEntry {
        agent: Agent::Claude,
        path: path.to_path_buf(),
        session_id,
        project: project.unwrap_or_else(|| decode_project_dir(project_dir)),
        last_active,
        is_prompt: preview.has_summary(),
        summary: preview.finish(),
    })
}

fn preview_text(value: &Value) -> Option<String> {
    if value.get("type").and_then(Value::as_str) != Some("user") {
        return None;
    }
    if value.get("isMeta").and_then(Value::as_bool) == Some(true) {
        return None;
    }
    // Content is a plain string or an array of blocks; a user message of only
    // tool_results has no prompt text and yields None.
    let content = value.get("message")?.get("content")?;
    match content.as_str() {
        Some(s) => Some(s.to_string()),
        None => first_block_text(content),
    }
}

/// Parse each line independently; a line that fails to parse is skipped.
pub fn parse(input: &str) -> (Session, ParseStats) {
    fold_lines(input, |session, value| {
        if session.session_id.is_none()
            && let Some(sid) = value.get("sessionId").and_then(Value::as_str)
        {
            session.session_id = Some(sid.to_string());
        }

        if let Some(record) = parse_record(value) {
            session.records.push(record);
        }
    })
}

fn parse_record(value: &Value) -> Option<Record> {
    let record_type = value.get("type").and_then(Value::as_str)?;

    let kind = match record_type {
        "user" => RecordKind::User(parse_content(value.get("message")?.get("content")?)),
        "assistant" => RecordKind::Assistant(parse_content(value.get("message")?.get("content")?)),
        "system" => RecordKind::System {
            subtype: value
                .get("subtype")
                .and_then(Value::as_str)
                .map(str::to_string),
            text: value
                .get("content")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
        },
        other => RecordKind::Meta {
            record_type: other.to_string(),
        },
    };

    Some(Record {
        timestamp: value
            .get("timestamp")
            .and_then(Value::as_str)
            .map(str::to_string),
        kind,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discover_in_missing_directory_is_empty() {
        let missing = std::env::temp_dir().join("dtrail-definitely-not-here-12345");
        assert!(discover_in(&missing).is_empty());
    }

    #[test]
    fn discover_in_reads_metadata_and_tags_agent() {
        let tmp = std::env::temp_dir().join(format!("dtrail-claude-{}", std::process::id()));
        let proj = tmp.join("-Users-me-projects-demo");
        fs::create_dir_all(&proj).unwrap();
        let path = proj.join("sess-1.jsonl");
        fs::write(
            &path,
            concat!(
                r#"{"type":"user","cwd":"/Users/me/projects/demo","message":{"role":"user","content":"<command-name>/clear</command-name>"}}"#,
                "\n",
                r#"{"type":"user","cwd":"/Users/me/projects/demo","message":{"role":"user","content":"本当の最初の質問"}}"#,
                "\n",
            ),
        )
        .unwrap();

        let entries = discover_in(&tmp);
        fs::remove_dir_all(&tmp).ok();

        assert_eq!(entries.len(), 1);
        let e = &entries[0];
        assert_eq!(e.agent, Agent::Claude);
        assert_eq!(e.session_id, "sess-1");
        assert_eq!(e.project, "/Users/me/projects/demo");
        assert_eq!(e.summary, "本当の最初の質問");
        assert!(e.is_prompt);
    }

    #[test]
    fn summary_falls_back_to_the_raw_user_utterance() {
        let tmp = std::env::temp_dir().join(format!("dtrail-claude-fb-{}", std::process::id()));
        fs::create_dir_all(&tmp).unwrap();
        let path = tmp.join("sess-x.jsonl");
        fs::write(
            &path,
            concat!(
                r#"{"type":"user","cwd":"/p","message":{"role":"user","content":"<command-name>/clear</command-name>"}}"#,
                "\n",
                r#"{"type":"assistant","message":{"content":[{"type":"text","text":"here is the answer"}]}}"#,
                "\n",
            ),
        )
        .unwrap();

        let e = scan(&path).unwrap();
        fs::remove_dir_all(&tmp).ok();

        // No real prompt, so the preview shows the raw utterance the assistant
        // replied to (the command) as a recorded fact — not the reply itself.
        assert_eq!(e.summary, "<command-name>/clear</command-name>");
        assert!(!e.is_prompt);
    }

    #[test]
    fn a_session_with_no_user_message_has_an_empty_preview() {
        let tmp = std::env::temp_dir().join(format!("dtrail-claude-empty-{}", std::process::id()));
        fs::create_dir_all(&tmp).unwrap();
        let path = tmp.join("sess-x.jsonl");
        fs::write(
            &path,
            // Only an assistant turn; nothing is invented from it.
            concat!(
                r#"{"type":"assistant","cwd":"/p","message":{"content":[{"type":"text","text":"unprompted"}]}}"#,
                "\n",
            ),
        )
        .unwrap();

        let e = scan(&path).unwrap();
        fs::remove_dir_all(&tmp).ok();

        assert_eq!(e.summary, "");
    }

    #[test]
    fn summary_skips_a_meta_opener_then_reads_the_array_prompt() {
        // Mirrors a real envelope: meta records, a command-only string user
        // message, then the real prompt as an array-content user message.
        // The isMeta text is untagged so only the flag keeps it out.
        let tmp = std::env::temp_dir().join(format!("dtrail-claude-env-{}", std::process::id()));
        fs::create_dir_all(&tmp).unwrap();
        let path = tmp.join("sess-x.jsonl");
        fs::write(
            &path,
            concat!(
                r#"{"type":"mode"}"#,
                "\n",
                r#"{"type":"file-history-snapshot"}"#,
                "\n",
                r#"{"type":"user","isMeta":true,"cwd":"/p","message":{"role":"user","content":"injected note"}}"#,
                "\n",
                r#"{"type":"user","cwd":"/p","message":{"role":"user","content":"<command-name>/clear</command-name>"}}"#,
                "\n",
                r#"{"type":"user","cwd":"/p","message":{"role":"user","content":[{"type":"text","text":"the real prompt"}]}}"#,
                "\n",
                r#"{"type":"attachment"}"#,
                "\n",
            ),
        )
        .unwrap();

        let e = scan(&path).unwrap();
        fs::remove_dir_all(&tmp).ok();

        assert_eq!(e.summary, "the real prompt");
    }
}
