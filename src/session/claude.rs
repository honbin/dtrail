//! Claude Code sessions: discovery and parsing.
//!
//! Layout: `$CLAUDE_CONFIG_DIR/projects` (else `~/.claude/projects`), one
//! directory per project, one `.jsonl` file per session. Parsing turns each
//! line into the shared model; unknown events and broken lines are tolerated.

use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use serde_json::Value;

use super::discovery::{Agent, SessionEntry, is_command_or_meta};
use super::model::{Block, ParseStats, Record, RecordKind, Session, ToolResultContent};

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

/// All Claude sessions, newest first. Unreadable dirs/files are skipped.
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
    entries.sort_by_key(|e| std::cmp::Reverse(e.modified));
    entries
}

/// Scan one Claude session file (used by `discover_dir`, which supplies files
/// from an arbitrary tree). The parent dir is the decode fallback for `cwd`.
pub(crate) fn scan(path: &Path) -> Option<SessionEntry> {
    scan_entry(path, path.parent().unwrap_or(path))
}

fn scan_entry(path: &Path, project_dir: &Path) -> Option<SessionEntry> {
    let modified = fs::metadata(path).and_then(|m| m.modified()).ok();
    let session_id = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or_default()
        .to_string();

    let reader = BufReader::new(fs::File::open(path).ok()?);
    let mut project: Option<String> = None;
    let mut summary: Option<String> = None;
    for line in reader.lines().map_while(Result::ok) {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if project.is_none()
            && let Some(cwd) = value.get("cwd").and_then(Value::as_str)
        {
            project = Some(cwd.to_string());
        }
        if summary.is_none()
            && let Some(text) = user_text(&value)
            && !is_command_or_meta(&text)
        {
            summary = Some(text);
        }
        if project.is_some() && summary.is_some() {
            break;
        }
    }

    Some(SessionEntry {
        agent: Agent::Claude,
        path: path.to_path_buf(),
        session_id,
        project: project.unwrap_or_else(|| decode_project_dir(project_dir)),
        modified,
        summary: summary.unwrap_or_default(),
    })
}

/// String content of a user record (array content is not used for previews).
fn user_text(value: &Value) -> Option<String> {
    if value.get("type").and_then(Value::as_str) != Some("user") {
        return None;
    }
    if value.get("isMeta").and_then(Value::as_bool) == Some(true) {
        return None;
    }
    value
        .get("message")?
        .get("content")?
        .as_str()
        .map(str::to_string)
}

/// Fallback when `cwd` is absent: undo the dir-name encoding (`-` back to `/`).
fn decode_project_dir(dir: &Path) -> String {
    let name = dir.file_name().and_then(|n| n.to_str()).unwrap_or_default();
    name.replace('-', "/")
}

/// Parse each line independently; a line that fails to parse is skipped.
pub fn parse(input: &str) -> (Session, ParseStats) {
    let mut session = Session::default();
    let mut stats = ParseStats::default();

    for line in input.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        stats.total_lines += 1;

        let value: Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => {
                stats.skipped_lines += 1;
                continue;
            }
        };

        if session.session_id.is_none()
            && let Some(sid) = value.get("sessionId").and_then(Value::as_str)
        {
            session.session_id = Some(sid.to_string());
        }

        if let Some(record) = parse_record(&value) {
            session.records.push(record);
        }
    }

    (session, stats)
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
        uuid: value
            .get("uuid")
            .and_then(Value::as_str)
            .map(str::to_string),
        parent_uuid: value
            .get("parentUuid")
            .and_then(Value::as_str)
            .map(str::to_string),
        timestamp: value
            .get("timestamp")
            .and_then(Value::as_str)
            .map(str::to_string),
        kind,
    })
}

/// `message.content`: either a string or an array of blocks.
fn parse_content(content: &Value) -> Vec<Block> {
    match content {
        Value::String(s) => vec![Block::Text(s.clone())],
        Value::Array(items) => items.iter().filter_map(parse_block).collect(),
        _ => Vec::new(),
    }
}

fn parse_block(block: &Value) -> Option<Block> {
    let block_type = block.get("type").and_then(Value::as_str)?;
    let b = match block_type {
        "text" => Block::Text(
            block
                .get("text")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
        ),
        "thinking" => Block::Thinking(
            block
                .get("thinking")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
        ),
        "tool_use" => Block::ToolUse {
            id: block
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            name: block
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            input: block.get("input").cloned().unwrap_or(Value::Null),
        },
        "tool_result" => Block::ToolResult {
            tool_use_id: block
                .get("tool_use_id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            content: parse_tool_result_content(block.get("content")),
            is_error: block
                .get("is_error")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        },
        other => Block::Unknown {
            block_type: other.to_string(),
        },
    };
    Some(b)
}

/// `tool_result.content`: a string, or an array of `text`/`image`/`tool_reference`.
/// Non-text elements become placeholders; no binary is retained.
fn parse_tool_result_content(content: Option<&Value>) -> ToolResultContent {
    match content {
        Some(Value::String(s)) => ToolResultContent::Text(s.clone()),
        Some(Value::Array(items)) => {
            let parts = items
                .iter()
                .map(|item| {
                    let t = item
                        .get("type")
                        .and_then(Value::as_str)
                        .unwrap_or("unknown");
                    match t {
                        "text" => item
                            .get("text")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        "image" => "[image]".to_string(),
                        other => format!("[{other}]"),
                    }
                })
                .collect();
            ToolResultContent::Blocks(parts)
        }
        _ => ToolResultContent::Text(String::new()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discover_in_missing_directory_is_empty() {
        let missing = std::env::temp_dir().join("ccscope-definitely-not-here-12345");
        assert!(discover_in(&missing).is_empty());
    }

    #[test]
    fn discover_in_reads_metadata_and_tags_agent() {
        let tmp = std::env::temp_dir().join(format!("ccscope-claude-{}", std::process::id()));
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
    }
}
