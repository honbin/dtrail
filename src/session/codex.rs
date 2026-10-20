//! Codex sessions: discovery and parsing.
//!
//! Layout: `$CODEX_HOME/sessions` (else `~/.codex/sessions`), nested by date,
//! one `rollout-*.jsonl` file per session. Each line wraps a `payload` with a
//! `type`; the `response_item` stream is the canonical conversation (the
//! `event_msg` stream is a UI duplicate and is ignored). Items map onto the
//! shared model: message -> user/assistant text, reasoning -> thinking,
//! function/custom tool calls and their outputs -> tool use / tool result,
//! linked by `call_id`.

use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use serde_json::Value;

use super::discovery::{Agent, PreviewPicker, SessionEntry, session_id_from_stem};
use super::model::{Block, ParseStats, Record, RecordKind, Session, ToolResultContent, fold_lines};

/// Codex's session directory: `CODEX_HOME/sessions`, else `~/.codex/sessions`.
fn base_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("CODEX_HOME")
        && !dir.is_empty()
    {
        return PathBuf::from(dir).join("sessions");
    }
    let home = std::env::var("HOME").unwrap_or_default();
    PathBuf::from(home).join(".codex").join("sessions")
}

/// All Codex sessions, newest first. Unreadable dirs/files are skipped.
pub fn discover() -> Vec<SessionEntry> {
    discover_in(&base_dir())
}

fn discover_in(root: &Path) -> Vec<SessionEntry> {
    // Sessions are nested under date directories, so walk recursively.
    let mut files = Vec::new();
    super::discovery::collect_jsonl(root, &mut files);
    let mut entries: Vec<SessionEntry> = files.iter().filter_map(|p| scan(p)).collect();
    entries.sort_by_key(|e| std::cmp::Reverse(e.modified));
    entries
}

/// Scan one Codex session file into list metadata.
pub(crate) fn scan(path: &Path) -> Option<SessionEntry> {
    let modified = fs::metadata(path).and_then(|m| m.modified()).ok();
    let reader = BufReader::new(fs::File::open(path).ok()?);

    let mut project: Option<String> = None;
    let mut session_id: Option<String> = None;
    let mut preview = PreviewPicker::default();

    for line in reader.lines().map_while(Result::ok) {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        match value.get("type").and_then(Value::as_str) {
            Some("session_meta") => {
                let payload = value.get("payload");
                if project.is_none() {
                    project = payload
                        .and_then(|p| p.get("cwd"))
                        .and_then(Value::as_str)
                        .map(str::to_string);
                }
                if session_id.is_none() {
                    session_id = payload
                        .and_then(|p| p.get("session_id"))
                        .and_then(Value::as_str)
                        .map(str::to_string);
                }
            }
            Some("response_item") if !preview.has_summary() => {
                let payload = value.get("payload");
                let is_message =
                    payload.and_then(|p| p.get("type")).and_then(Value::as_str) == Some("message");
                let role = payload.and_then(|p| p.get("role")).and_then(Value::as_str);
                if is_message && role == Some("user") && !is_injected_context(payload.unwrap()) {
                    preview.offer(message_text(payload.unwrap()));
                }
            }
            _ => {}
        }
        if project.is_some() && preview.has_summary() {
            break;
        }
    }

    let session_id = session_id.unwrap_or_else(|| session_id_from_stem(path));

    Some(SessionEntry {
        agent: Agent::Codex,
        path: path.to_path_buf(),
        session_id,
        project: project.unwrap_or_default(),
        modified,
        summary: preview.finish(),
    })
}

/// Parse each line independently; a line that fails to parse is skipped.
pub fn parse(input: &str) -> (Session, ParseStats) {
    fold_lines(input, |session, value| {
        let record_type = value.get("type").and_then(Value::as_str).unwrap_or("");
        if record_type == "session_meta" && session.session_id.is_none() {
            session.session_id = value
                .get("payload")
                .and_then(|p| p.get("session_id"))
                .and_then(Value::as_str)
                .map(str::to_string);
        }
        // Only response_item carries the conversation; other records are UI/meta.
        if record_type != "response_item" {
            return;
        }
        let Some(payload) = value.get("payload") else {
            return;
        };
        let timestamp = value
            .get("timestamp")
            .and_then(Value::as_str)
            .map(str::to_string);

        if let Some(kind) = record_kind(payload) {
            session.records.push(Record {
                uuid: None,
                parent_uuid: None,
                timestamp,
                kind,
            });
        }
    })
}

fn record_kind(payload: &Value) -> Option<RecordKind> {
    match payload.get("type").and_then(Value::as_str)? {
        "message" => {
            let text = message_text(payload);
            match payload.get("role").and_then(Value::as_str) {
                Some("user") => Some(RecordKind::User(vec![Block::Text(text)])),
                Some("assistant") => Some(RecordKind::Assistant(vec![Block::Text(text)])),
                other => Some(RecordKind::System {
                    subtype: other.map(str::to_string),
                    text,
                }),
            }
        }
        "reasoning" => {
            let text = reasoning_text(payload);
            if text.is_empty() {
                None
            } else {
                Some(RecordKind::Assistant(vec![Block::Thinking(text)]))
            }
        }
        "function_call" | "custom_tool_call" => Some(RecordKind::Assistant(vec![Block::ToolUse {
            id: str_field(payload, "call_id"),
            name: str_field(payload, "name"),
            // function_call uses `arguments`, custom_tool_call uses `input`.
            input: payload
                .get("arguments")
                .or_else(|| payload.get("input"))
                .and_then(Value::as_str)
                .map(parse_tool_input)
                .unwrap_or(Value::Null),
        }])),
        "function_call_output" | "custom_tool_call_output" => {
            Some(RecordKind::User(vec![Block::ToolResult {
                tool_use_id: str_field(payload, "call_id"),
                content: ToolResultContent::Text(output_text(payload)),
                is_error: false,
            }]))
        }
        // Built-in tools whose input is already structured rather than a string.
        // The local shell's output arrives as a `function_call_output` with the
        // same `call_id`.
        kind @ ("local_shell_call" | "web_search_call" | "tool_search_call") => {
            Some(RecordKind::Assistant(vec![Block::ToolUse {
                id: str_field(payload, "call_id"),
                name: kind.to_string(),
                input: payload
                    .get("action")
                    .or_else(|| payload.get("arguments"))
                    .cloned()
                    .unwrap_or(Value::Null),
            }]))
        }
        // Kept visible as a placeholder, like Claude's unknown blocks.
        other => Some(RecordKind::Assistant(vec![Block::Unknown {
            block_type: other.to_string(),
        }])),
    }
}

/// Whether a user `message` contains only known harness-injected context.
/// Codex records each turn's origin kinds in passthrough metadata. Older logs,
/// empty kind lists, and unknown kinds fall back to the text heuristic in `scan`.
fn is_injected_context(payload: &Value) -> bool {
    let Some(kinds) = payload
        .get("internal_chat_message_metadata_passthrough")
        .and_then(|m| m.get("content_item_kinds"))
        .and_then(Value::as_array)
    else {
        return false;
    };

    !kinds.is_empty()
        && kinds.iter().all(|kind| {
            kind.as_str().is_some_and(|kind| {
                kind.starts_with("environments.") || kind.starts_with("additional_content.")
            })
        })
}

/// Concatenate the text of a message's content parts (`input_text`/`output_text`).
fn message_text(payload: &Value) -> String {
    payload
        .get("content")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|it| it.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("")
        })
        .unwrap_or_default()
}

/// Reasoning items carry human-readable text under `summary[].text`.
fn reasoning_text(payload: &Value) -> String {
    payload
        .get("summary")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|it| it.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default()
}

/// Tool call arguments arrive as a JSON string; keep the parsed value when
/// possible (nicer to pretty-print), else fall back to the raw string.
fn parse_tool_input(s: &str) -> Value {
    serde_json::from_str(s).unwrap_or_else(|_| Value::String(s.to_string()))
}

fn output_text(payload: &Value) -> String {
    match payload.get("output") {
        Some(Value::String(s)) => s.clone(),
        Some(other) => other.to_string(),
        None => String::new(),
    }
}

fn str_field(payload: &Value, key: &str) -> String {
    payload
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = concat!(
        r#"{"type":"session_meta","timestamp":"2026-10-02T16:30:56Z","payload":{"cwd":"/Users/me/proj","session_id":"sid-1"}}"#,
        "\n",
        r#"{"type":"event_msg","payload":{"type":"task_started"}}"#,
        "\n",
        r#"{"type":"response_item","timestamp":"2026-10-02T16:31:00Z","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"fix the bug"}]}}"#,
        "\n",
        r#"{"type":"response_item","payload":{"type":"reasoning","summary":[{"type":"summary_text","text":"think think"}]}}"#,
        "\n",
        r#"{"type":"response_item","payload":{"type":"function_call","call_id":"c1","name":"shell","arguments":"{\"cmd\":\"ls\"}"}}"#,
        "\n",
        r#"{"type":"response_item","payload":{"type":"function_call_output","call_id":"c1","output":"file1\nfile2"}}"#,
        "\n",
        r#"{"type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"done"}]}}"#,
        "\n",
    );

    #[test]
    fn parses_codex_items_into_the_shared_model() {
        let (session, stats) = parse(SAMPLE);
        assert_eq!(stats.skipped_lines, 0);
        assert_eq!(session.session_id.as_deref(), Some("sid-1"));

        // session_meta + event_msg are not conversation records.
        // user message, reasoning, tool_use, tool_result, assistant message.
        assert_eq!(session.records.len(), 5);

        // tool_use and tool_result link by call_id.
        let index = session.tool_use_index();
        assert!(index.contains_key("c1"));

        // reasoning became thinking; assistant text present.
        let has_thinking = session.records.iter().any(|r| {
            matches!(&r.kind, RecordKind::Assistant(b) if matches!(b.first(), Some(Block::Thinking(_))))
        });
        assert!(has_thinking);
    }

    #[test]
    fn other_item_types_are_shown_not_dropped() {
        let jsonl = concat!(
            r#"{"type":"response_item","payload":{"type":"local_shell_call","call_id":"c2","status":"completed","action":{"type":"exec","command":["ls"]}}}"#,
            "\n",
            r#"{"type":"response_item","payload":{"type":"function_call_output","call_id":"c2","output":"a"}}"#,
            "\n",
            r#"{"type":"response_item","payload":{"type":"image_generation_call","status":"completed","result":""}}"#,
            "\n",
        );
        let (session, _) = parse(jsonl);
        // The shell call is a tool use its output links to.
        assert!(session.tool_use_index().contains_key("c2"));
        assert!(matches!(
            &session.records[2].kind,
            RecordKind::Assistant(b) if matches!(&b[0], Block::Unknown { block_type } if block_type == "image_generation_call")
        ));
    }

    #[test]
    fn empty_reasoning_is_dropped() {
        let line = r#"{"type":"response_item","payload":{"type":"reasoning","summary":[]}}"#;
        let (session, _) = parse(line);
        assert!(session.records.is_empty());
    }

    #[test]
    fn discover_in_missing_directory_is_empty() {
        let missing = std::env::temp_dir().join("dtrail-codex-not-here-54321");
        assert!(discover_in(&missing).is_empty());
    }

    #[test]
    fn discover_in_walks_nested_dirs_and_tags_agent() {
        let tmp = std::env::temp_dir().join(format!("dtrail-codex-{}", std::process::id()));
        let day = tmp.join("2026").join("10").join("02");
        fs::create_dir_all(&day).unwrap();
        let path = day.join("rollout-2026-10-02T16-30-56-sid-1.jsonl");
        fs::write(&path, SAMPLE).unwrap();

        let entries = discover_in(&tmp);
        fs::remove_dir_all(&tmp).ok();

        assert_eq!(entries.len(), 1);
        let e = &entries[0];
        assert_eq!(e.agent, Agent::Codex);
        assert_eq!(e.project, "/Users/me/proj");
        assert_eq!(e.summary, "fix the bug");
        assert_eq!(e.session_id, "sid-1");
    }

    #[test]
    fn injected_context_turns_are_skipped_for_the_preview() {
        let tmp = std::env::temp_dir().join(format!("dtrail-codex-inj-{}", std::process::id()));
        fs::create_dir_all(&tmp).unwrap();
        let path = tmp.join("rollout.jsonl");
        fs::write(
            &path,
            concat!(
                r#"{"type":"session_meta","payload":{"cwd":"/p","session_id":"sid"}}"#,
                "\n",
                // Harness-injected environment context (kinds lack `user.text`).
                r#"{"type":"response_item","payload":{"type":"message","role":"user","internal_chat_message_metadata_passthrough":{"content_item_kinds":["environments.environment_context"]},"content":[{"type":"input_text","text":"<environment_context>\n  <cwd>/p</cwd>"}]}}"#,
                "\n",
                r#"{"type":"response_item","payload":{"type":"message","role":"user","internal_chat_message_metadata_passthrough":{"content_item_kinds":["user.text"]},"content":[{"type":"input_text","text":"fix the parser"}]}}"#,
                "\n",
            ),
        )
        .unwrap();

        let e = scan(&path).unwrap();
        fs::remove_dir_all(&tmp).ok();

        // The injected context is skipped; the preview is the real prompt.
        assert_eq!(e.summary, "fix the parser");
    }

    #[test]
    fn a_session_of_only_injected_turns_has_no_preview() {
        let tmp = std::env::temp_dir().join(format!("dtrail-codex-inj2-{}", std::process::id()));
        fs::create_dir_all(&tmp).unwrap();
        let path = tmp.join("rollout.jsonl");
        fs::write(
            &path,
            concat!(
                r#"{"type":"session_meta","payload":{"cwd":"/p","session_id":"sid"}}"#,
                "\n",
                r#"{"type":"response_item","payload":{"type":"message","role":"user","internal_chat_message_metadata_passthrough":{"content_item_kinds":["environments.environment_context"]},"content":[{"type":"input_text","text":"<environment_context>x</environment_context>"}]}}"#,
                "\n",
                r#"{"type":"response_item","payload":{"type":"message","role":"user","internal_chat_message_metadata_passthrough":{"content_item_kinds":["additional_content.codex_apps_open_page"]},"content":[{"type":"input_text","text":"<external_codex_apps_open_page>{}</external_codex_apps_open_page>"}]}}"#,
                "\n",
            ),
        )
        .unwrap();

        let e = scan(&path).unwrap();
        fs::remove_dir_all(&tmp).ok();

        // No typed prompt and no raw fallback from injected turns: empty preview.
        assert_eq!(e.summary, "");
    }

    #[test]
    fn summary_falls_back_to_the_raw_user_utterance() {
        let tmp = std::env::temp_dir().join(format!("dtrail-codex-fb-{}", std::process::id()));
        fs::create_dir_all(&tmp).unwrap();
        let path = tmp.join("rollout.jsonl");
        fs::write(
            &path,
            concat!(
                r#"{"type":"session_meta","payload":{"cwd":"/p","session_id":"sid"}}"#,
                "\n",
                r#"{"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"<command-name>/clear</command-name>"}]}}"#,
                "\n",
                r#"{"type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"the answer"}]}}"#,
                "\n",
            ),
        )
        .unwrap();

        let e = scan(&path).unwrap();
        fs::remove_dir_all(&tmp).ok();

        // No real prompt → the raw first user utterance (the command), not the
        // assistant's reply.
        assert_eq!(e.summary, "<command-name>/clear</command-name>");
    }
}
