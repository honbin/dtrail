//! Cursor agent sessions: discovery and parsing.
//!
//! Layout: `~/.cursor/projects/<encoded-project>/agent-transcripts/<uuid>/<uuid>.jsonl`,
//! one file per agent session. Each line is either a message
//! (`{role, message:{content:[...]}}`) or a `turn_ended` control event (ignored).
//! Content blocks are `text` and `tool_use` — the Anthropic message shape — so the
//! parsing mirrors [`super::claude`]; there are no tool results or reasoning in the
//! log, and no per-line `cwd`/`timestamp`/session id (project comes from the dir
//! name, the session id from the file name).

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;

use super::content::{first_block_text, parse_content};
use super::discovery::{
    Agent, PreviewPicker, SessionEntry, decode_project_dir, json_lines, session_id_from_stem,
};
use super::model::{Block, ParseStats, Record, RecordKind, Session, fold_lines};

/// Cursor's agent-transcript root: `~/.cursor/projects`.
fn base_dir() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_default();
    PathBuf::from(home).join(".cursor").join("projects")
}

/// All Cursor sessions, newest first. Unreadable dirs/files are skipped.
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
        // Transcripts are nested under agent-transcripts/<uuid>/, so walk down.
        let mut files = Vec::new();
        super::discovery::collect_jsonl(&dir_path, &mut files);
        for path in files {
            if let Some(entry) = scan_entry(&path, &dir_path) {
                entries.push(entry);
            }
        }
    }
    entries
}

/// Scan one Cursor transcript file (used by `discover_dir`, which supplies files
/// from an arbitrary tree).
pub(crate) fn scan(path: &Path) -> Option<SessionEntry> {
    scan_entry(path, project_dir_of(path))
}

/// The project dir for a standalone file. Cursor nests transcripts under
/// `<project>/agent-transcripts/<uuid>/`, so climb to the `agent-transcripts`
/// parent rather than using the file's own parent (which is the `<uuid>` dir).
fn project_dir_of(path: &Path) -> &Path {
    let mut cur = path;
    while let Some(parent) = cur.parent() {
        if parent.file_name().and_then(|n| n.to_str()) == Some("agent-transcripts") {
            return parent.parent().unwrap_or(parent);
        }
        cur = parent;
    }
    path.parent().unwrap_or(path)
}

fn scan_entry(path: &Path, project_dir: &Path) -> Option<SessionEntry> {
    let modified = fs::metadata(path).and_then(|m| m.modified()).ok();
    let session_id = session_id_from_stem(path);

    let values = json_lines(path)?;
    let mut preview = PreviewPicker::default();
    for value in values {
        if value.get("role").and_then(Value::as_str) != Some("user") {
            continue;
        }
        let Some(text) = preview_text(&value) else {
            continue;
        };
        if preview.offer(text) {
            break;
        }
    }

    Some(SessionEntry {
        agent: Agent::Cursor,
        path: path.to_path_buf(),
        session_id,
        project: decode_project_dir(project_dir),
        modified,
        summary: preview.finish(),
    })
}

/// The user's prompt for the preview: the first non-empty `text` block, unwrapped
/// from Cursor's `<user_query>…</user_query>`. That block also carries injected
/// context (timestamps, git diff, attached files); the query is what the user
/// actually typed, so the preview drops the rest.
fn preview_text(value: &Value) -> Option<String> {
    let text = first_block_text(value.get("message")?.get("content")?)?;
    Some(user_query(&text).unwrap_or(text))
}

/// Tag Cursor wraps the typed prompt in, apart from the injected context
/// around it. The body view keeps it unfolded for the same reason.
pub(crate) const USER_QUERY_TAG: &str = "user_query";

/// Inner text of the first `<user_query>…</user_query>`, trimmed, if present.
fn user_query(text: &str) -> Option<String> {
    let open = format!("<{USER_QUERY_TAG}>");
    let start = text.find(&open)? + open.len();
    let rest = &text[start..];
    let end = rest.find(&format!("</{USER_QUERY_TAG}>"))?;
    Some(rest[..end].trim().to_string())
}

/// Parse each line independently; a line that fails to parse is skipped.
pub fn parse(input: &str) -> (Session, ParseStats) {
    fold_lines(input, |session, value| {
        if let Some(record) = parse_record(value) {
            session.records.push(record);
        }
    })
}

/// A message line becomes a record; `turn_ended` (no `message`) is dropped.
fn parse_record(value: &Value) -> Option<Record> {
    let content = value.get("message")?.get("content")?;
    let kind = match value.get("role").and_then(Value::as_str)? {
        "user" => RecordKind::User(parse_content(content)),
        "assistant" => RecordKind::Assistant(parse_content(content)),
        other => other_role(other, content),
    };
    Some(Record {
        uuid: None,
        parent_uuid: None,
        timestamp: None,
        kind,
    })
}

/// `message.content` is an array of `text`/`tool_use` blocks (string tolerated).
/// A role dtrail doesn't model keeps its text as system text, or a placeholder
/// when it has none, so the message is never silently dropped from the body.
fn other_role(role: &str, content: &Value) -> RecordKind {
    let text = parse_content(content)
        .into_iter()
        .filter_map(|b| match b {
            Block::Text(t) => Some(t),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n");
    if text.is_empty() {
        RecordKind::Assistant(vec![Block::Unknown {
            block_type: role.to_string(),
        }])
    } else {
        RecordKind::System {
            subtype: Some(role.to_string()),
            text,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = concat!(
        r#"{"role":"user","message":{"content":[{"type":"text","text":"fix the bug"}]}}"#,
        "\n",
        r#"{"role":"assistant","message":{"content":[{"type":"text","text":"on it"},{"type":"tool_use","name":"Bash","input":{"command":"ls"}}]}}"#,
        "\n",
        r#"{"type":"turn_ended","status":"completed"}"#,
        "\n",
    );

    #[test]
    fn discover_in_walks_nested_transcripts_and_tags_agent() {
        let tmp = std::env::temp_dir().join(format!("dtrail-cursor-{}", std::process::id()));
        let dir = tmp
            .join("Users-me-projects-demo")
            .join("agent-transcripts")
            .join("abc-123");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("abc-123.jsonl"), SAMPLE).unwrap();

        let entries = discover_in(&tmp);
        fs::remove_dir_all(&tmp).ok();

        assert_eq!(entries.len(), 1);
        let e = &entries[0];
        assert_eq!(e.agent, Agent::Cursor);
        assert_eq!(e.summary, "fix the bug");
        assert_eq!(e.session_id, "abc-123");
        assert_eq!(e.project, "Users/me/projects/demo");
    }

    #[test]
    fn scan_derives_project_from_the_agent_transcripts_parent() {
        let tmp = std::env::temp_dir().join(format!("dtrail-cursor-pd-{}", std::process::id()));
        let dir = tmp
            .join("Users-me-projects-demo")
            .join("agent-transcripts")
            .join("u9");
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("u9.jsonl");
        fs::write(&path, SAMPLE).unwrap();

        // discover_dir calls scan() with just the file path; the project is the
        // dir above agent-transcripts, not the <uuid> dir below it.
        let e = scan(&path).unwrap();
        fs::remove_dir_all(&tmp).ok();
        assert_eq!(e.project, "Users/me/projects/demo");
    }

    #[test]
    fn content_blocks_parse_as_claude_does() {
        let line =
            r#"{"role":"assistant","message":{"content":[{"type":"thinking","thinking":"hmm"}]}}"#;
        let (session, _) = parse(line);
        assert!(matches!(
            &session.records[0].kind,
            RecordKind::Assistant(b) if matches!(&b[0], Block::Thinking(t) if t == "hmm")
        ));
    }

    #[test]
    fn other_roles_are_shown_not_dropped() {
        let jsonl = concat!(
            r#"{"role":"system","message":{"content":[{"type":"text","text":"be terse"}]}}"#,
            "\n",
            r#"{"role":"tool","message":{"content":[]}}"#,
            "\n",
        );
        let (session, _) = parse(jsonl);
        assert!(matches!(
            &session.records[0].kind,
            RecordKind::System { subtype: Some(r), text } if r == "system" && text == "be terse"
        ));
        assert!(matches!(
            &session.records[1].kind,
            RecordKind::Assistant(b) if matches!(&b[0], Block::Unknown { block_type } if block_type == "tool")
        ));
    }

    #[test]
    fn preview_unwraps_the_user_query_from_surrounding_context() {
        let tmp = std::env::temp_dir().join(format!("dtrail-cursor-uq-{}", std::process::id()));
        let dir = tmp.join("proj").join("agent-transcripts").join("u1");
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("u1.jsonl"),
            r#"{"role":"user","message":{"content":[{"type":"text","text":"<timestamp>t</timestamp>\n<user_query>fix the parser</user_query>\n<attached_files>a.rs</attached_files>"}]}}"#,
        )
        .unwrap();

        let entries = discover_in(&tmp);
        fs::remove_dir_all(&tmp).ok();

        // The preview is the query, not the injected context around it.
        assert_eq!(entries[0].summary, "fix the parser");
    }

    #[test]
    fn parse_maps_roles_and_blocks_and_ignores_turn_ended() {
        let (session, stats) = parse(SAMPLE);
        assert_eq!(stats.total_lines, 3);
        // turn_ended is not a record; the two messages are.
        assert_eq!(session.records.len(), 2);
        assert!(matches!(session.records[0].kind, RecordKind::User(_)));
        match &session.records[1].kind {
            RecordKind::Assistant(blocks) => {
                assert!(matches!(blocks[0], Block::Text(_)));
                assert!(matches!(blocks[1], Block::ToolUse { .. }));
            }
            _ => panic!("expected an assistant record"),
        }
    }
}
