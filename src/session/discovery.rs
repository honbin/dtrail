//! Agent identity, the list-row metadata type, and multi-agent discovery.
//!
//! Each supported agent keeps its own discovery + parsing in its module
//! ([`super::claude`], [`super::codex`]); this file only ties them together.

use std::cmp::Reverse;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde_json::Value;

use super::model::{ParseStats, Session};
use super::{claude, codex, cursor};

/// Which coding agent produced a session. Identity only — the parsing and
/// discovery for each live in that agent's module.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Agent {
    Claude,
    Codex,
    Cursor,
}

impl Agent {
    /// Short name shown as the per-row tag in the session list.
    pub fn label(self) -> &'static str {
        match self {
            Agent::Claude => "Claude",
            Agent::Codex => "Codex",
            Agent::Cursor => "Cursor",
        }
    }

    /// Full name shown in the status line.
    pub fn name(self) -> &'static str {
        match self {
            Agent::Claude => "Claude Code",
            Agent::Codex => "Codex",
            Agent::Cursor => "Cursor",
        }
    }
}

/// One row in the session list. Holds only what the list needs; the body is
/// parsed lazily when the session is opened.
#[derive(Debug, Clone)]
pub struct SessionEntry {
    pub agent: Agent,
    pub path: PathBuf,
    /// File stem; usually the session id.
    pub session_id: String,
    /// Project path (working directory) the session ran in.
    pub project: String,
    /// File mtime, or `None` when it cannot be read.
    pub modified: Option<SystemTime>,
    /// First real user message (command/meta wrappers skipped); empty if none.
    pub summary: String,
}

/// All sessions across supported agents, newest first.
pub fn discover() -> Vec<SessionEntry> {
    let mut entries = claude::discover();
    entries.extend(codex::discover());
    entries.extend(cursor::discover());
    entries.sort_by_key(|e| Reverse(e.modified));
    entries
}

/// All sessions under an arbitrary directory (recursive), with each file's
/// agent detected from its content. Backs `ccs <dir>`; agents may be mixed.
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
    entries.sort_by_key(|e| Reverse(e.modified));
    entries
}

/// Recursively collect `*.jsonl` files under `dir`. Unreadable dirs are skipped.
pub(crate) fn collect_jsonl(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = fs::read_dir(dir) else {
        return;
    };
    for entry in rd.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_jsonl(&path, out);
        } else if path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
            out.push(path);
        }
    }
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
    let file = fs::File::open(path).ok()?;
    BufReader::new(file)
        .lines()
        .map_while(Result::ok)
        .filter_map(|line| serde_json::from_str::<Value>(line.trim()).ok())
        .find_map(|value| agent_of_value(&value))
}

/// Injected wrappers that aren't a real prompt: the slash-command family
/// (`<command-name>`, `<command-message>`, `<command-args>`), local command
/// output (`<local-command-stdout>`, `<local-command-stderr>`), interrupt
/// markers, and Claude Code's IDE context turns (`<ide_opened_file>`,
/// `<ide_selection>`). Matched by their specific tag prefixes, not a bare `<`, so
/// a genuine prompt that happens to start with `<` (e.g. "<div> isn't working") is
/// not mistaken for metadata.
///
/// This is a text heuristic for Claude and for Codex logs that predate the
/// per-turn origin metadata; current Codex logs are classified structurally
/// (see `codex::scan`).
pub(crate) fn is_command_or_meta(text: &str) -> bool {
    let t = text.trim_start();
    t.is_empty()
        || t.starts_with("<command-")
        || t.starts_with("<local-command-")
        || t.starts_with("[Request interrupted")
        || t.starts_with("<ide_opened_file")
        || t.starts_with("<ide_selection")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_wrappers_are_skipped_as_preview() {
        assert!(is_command_or_meta("<command-name>/clear</command-name>"));
        assert!(is_command_or_meta(
            "<command-message>run the thing</command-message>"
        ));
        assert!(is_command_or_meta(
            "<local-command-stdout>output</local-command-stdout>"
        ));
        assert!(is_command_or_meta("[Request interrupted by user]"));
        assert!(is_command_or_meta("   "));
        // Claude Code IDE context turns are injected, not typed by the user.
        assert!(is_command_or_meta(
            "<ide_opened_file>/src/main.rs</ide_opened_file>"
        ));
        assert!(is_command_or_meta(
            "<ide_selection>lines 1-5</ide_selection>"
        ));
        assert!(!is_command_or_meta("最適化できますか？"));
        // A real prompt that merely starts with `<` must not be treated as meta.
        assert!(!is_command_or_meta("<div> isn't working"));
    }

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
