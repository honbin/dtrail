//! Multi-agent discovery and loading.
//!
//! Each supported agent keeps its own discovery + parsing in its module
//! ([`super::claude`], [`super::codex`], [`super::cursor`]); this file only ties
//! them together.

use std::cmp::Reverse;
use std::fs;
use std::io::Read;
use std::path::Path;

use serde_json::Value;

use super::model::{Agent, ParseStats, Session, SessionEntry};
use super::scan::{collect_jsonl, json_lines, session_id_from_stem};
use super::{claude, codex, cursor};

/// All sessions across supported agents, newest first.
pub fn discover() -> Vec<SessionEntry> {
    let mut entries = claude::discover();
    entries.extend(codex::discover());
    entries.extend(cursor::discover());
    entries.sort_by_key(|e| Reverse(e.last_active));
    entries
}

/// All sessions under an arbitrary directory (recursive), with each file's
/// agent detected from its content. Backs `dtrail <dir>`; agents may be mixed.
pub fn discover_dir(root: &Path) -> Vec<SessionEntry> {
    let mut files = Vec::new();
    collect_jsonl(root, &mut files);
    let mut entries: Vec<SessionEntry> = files
        .iter()
        .filter_map(|p| match agent_of_file(p)? {
            Agent::Claude => claude::scan(p),
            Agent::Codex => codex::scan(p),
            Agent::Cursor => cursor::scan(p),
        })
        .collect();
    entries.sort_by_key(|e| Reverse(e.last_active));
    entries
}

/// Parse a session's text with the parser for its agent. The single place that
/// dispatches on [`Agent`], so a new agent surfaces as a non-exhaustive match.
pub fn parse(agent: Agent, input: &str) -> (Session, ParseStats) {
    match agent {
        Agent::Claude => claude::parse(input),
        Agent::Codex => codex::parse(input),
        Agent::Cursor => cursor::parse(input),
    }
}

/// Read and parse a session file. `agent` is detected from the content when not
/// known. Invalid UTF-8 is replaced rather than failing the whole file, since a
/// session still being written can end mid-character. A session id missing from
/// the content falls back to the file name.
pub fn load(path: &Path, agent: Option<Agent>) -> std::io::Result<(Session, ParseStats)> {
    Ok(parse_bytes(path, agent, &fs::read(path)?))
}

/// Like [`load`] but parses only the first `max_bytes`, cut back to the last
/// complete line: a quick look at a file too large to parse on every selection.
pub fn load_head(
    path: &Path,
    agent: Option<Agent>,
    max_bytes: u64,
) -> std::io::Result<(Session, ParseStats)> {
    let mut bytes = Vec::new();
    fs::File::open(path)?
        .take(max_bytes)
        .read_to_end(&mut bytes)?;
    let complete = bytes.iter().rposition(|&b| b == b'\n').map_or(0, |i| i + 1);
    bytes.truncate(complete);
    Ok(parse_bytes(path, agent, &bytes))
}

fn parse_bytes(path: &Path, agent: Option<Agent>, bytes: &[u8]) -> (Session, ParseStats) {
    let input = String::from_utf8_lossy(bytes);
    let agent = agent.unwrap_or_else(|| detect(&input));
    let (mut session, stats) = parse(agent, &input);
    if session.session_id.is_none() {
        session.session_id = Some(session_id_from_stem(path));
    }
    (session, stats)
}

/// Classify one parsed line by agent, or `None` if it carries no agent signal.
/// Codex and Claude tag every line with a `type`; Cursor's `turn_ended` events do
/// too, while its message lines have a top-level `role` and no `type`.
fn agent_of_value(value: &Value) -> Option<Agent> {
    match value.get("type").and_then(Value::as_str) {
        Some("session_meta" | "response_item" | "event_msg") => Some(Agent::Codex),
        Some("turn_ended") => Some(Agent::Cursor),
        Some(_) => Some(Agent::Claude),
        None => value.get("role").is_some().then_some(Agent::Cursor),
    }
}

/// Guess the agent from a file's content, for a path given on the command line.
pub fn detect(input: &str) -> Agent {
    input
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line.trim()).ok())
        .find_map(|value| agent_of_value(&value))
        .unwrap_or(Agent::Claude)
}

/// Like [`detect`] but reads only the head of a file (for `discover_dir`).
fn agent_of_file(path: &Path) -> Option<Agent> {
    json_lines(path)?.find_map(|value| agent_of_value(&value))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_distinguishes_the_agents() {
        let codex = r#"{"type":"session_meta","payload":{}}"#;
        let claude = r#"{"type":"user","message":{"role":"user","content":"hi"}}"#;
        // Cursor message lines have a top-level `role` and no `type`.
        let cursor = r#"{"role":"user","message":{"content":[{"type":"text","text":"hi"}]}}"#;
        assert_eq!(detect(codex), Agent::Codex);
        assert_eq!(detect(claude), Agent::Claude);
        assert_eq!(detect(cursor), Agent::Cursor);
    }

    #[test]
    fn load_tolerates_invalid_utf8_and_names_the_session_from_the_file() {
        let tmp = std::env::temp_dir().join(format!("dtrail-load-{}", std::process::id()));
        fs::create_dir_all(&tmp).unwrap();
        let path = tmp.join("sess-9.jsonl");
        let mut bytes = br#"{"type":"user","message":{"role":"user","content":"hi"}}"#.to_vec();
        // A last line cut off in the middle of a multi-byte character.
        bytes.extend_from_slice(b"\n{\"type\":\"user\",\"x\":\"\xe3\x81");

        fs::write(&path, &bytes).unwrap();
        let result = load(&path, None);
        fs::remove_dir_all(&tmp).ok();

        let (session, stats) = result.unwrap();
        assert_eq!(session.records.len(), 1);
        assert_eq!(stats.skipped_lines, 1);
        assert_eq!(session.session_id.as_deref(), Some("sess-9"));
    }

    #[test]
    fn load_head_parses_only_the_complete_lines_within_the_limit() {
        let tmp = std::env::temp_dir().join(format!("dtrail-head-{}", std::process::id()));
        fs::create_dir_all(&tmp).unwrap();
        let path = tmp.join("sess-h.jsonl");
        let first = r#"{"type":"user","message":{"role":"user","content":"first"}}"#;
        let second = r#"{"type":"user","message":{"role":"user","content":"second"}}"#;
        fs::write(&path, format!("{first}\n{second}\n")).unwrap();

        // The limit falls inside the second line, which is left out whole.
        let limit = (first.len() + 10) as u64;
        let result = load_head(&path, None, limit);
        fs::remove_dir_all(&tmp).ok();

        let (session, stats) = result.unwrap();
        assert_eq!(session.records.len(), 1);
        assert_eq!(stats.skipped_lines, 0);
    }

    #[test]
    fn discover_dir_finds_both_agents_recursively() {
        let tmp = std::env::temp_dir().join(format!("dtrail-dir-{}", std::process::id()));
        let sub = tmp.join("nested");
        fs::create_dir_all(&sub).unwrap();
        fs::write(
            tmp.join("a.jsonl"),
            r#"{"type":"user","cwd":"/p/one","message":{"role":"user","content":"claude hi"}}"#,
        )
        .unwrap();
        fs::write(
            sub.join("b.jsonl"),
            concat!(
                r#"{"type":"session_meta","payload":{"cwd":"/p/two","session_id":"s"}}"#,
                "\n",
                r#"{"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"codex hi"}]}}"#,
                "\n",
            ),
        )
        .unwrap();

        let entries = discover_dir(&tmp);
        fs::remove_dir_all(&tmp).ok();

        assert_eq!(entries.len(), 2);
        let claude = entries.iter().find(|e| e.agent == Agent::Claude).unwrap();
        let codex = entries.iter().find(|e| e.agent == Agent::Codex).unwrap();
        assert_eq!(claude.summary, "claude hi");
        assert_eq!(codex.summary, "codex hi");
        assert_eq!(codex.project, "/p/two");
    }
}
