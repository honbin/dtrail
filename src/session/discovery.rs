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
use super::{claude, codex};

/// Which coding agent produced a session. Identity only — the parsing and
/// discovery for each live in that agent's module.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Agent {
    Claude,
    Codex,
}

impl Agent {
    /// Short name shown as the per-row tag in the session list.
    pub fn label(self) -> &'static str {
        match self {
            Agent::Claude => "Claude",
            Agent::Codex => "Codex",
        }
    }

    /// Full name shown in the status line.
    pub fn name(self) -> &'static str {
        match self {
            Agent::Claude => "Claude Code",
            Agent::Codex => "Codex",
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
    }
}

fn agent_of_type(t: &str) -> Agent {
    match t {
        "session_meta" | "response_item" | "event_msg" => Agent::Codex,
        _ => Agent::Claude,
    }
}

/// Guess the agent from a file's content, for a path given on the command line.
pub fn detect(input: &str) -> Agent {
    for line in input.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Ok(value) = serde_json::from_str::<Value>(line)
            && let Some(t) = value.get("type").and_then(Value::as_str)
        {
            return agent_of_type(t);
        }
    }
    Agent::Claude
}

/// Like [`detect`] but reads only the head of a file (for `discover_dir`).
fn agent_of_file(path: &Path) -> Option<Agent> {
    let file = fs::File::open(path).ok()?;
    for line in BufReader::new(file).lines().map_while(Result::ok) {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Ok(value) = serde_json::from_str::<Value>(line)
            && let Some(t) = value.get("type").and_then(Value::as_str)
        {
            return Some(agent_of_type(t));
        }
    }
    None
}

/// Slash commands and wrappers (`<command-name>…`, `[Request interrupted…]`)
/// are useless as a preview, so skip them. Shared by the agents' discovery.
pub(crate) fn is_command_or_meta(text: &str) -> bool {
    let t = text.trim_start();
    t.is_empty() || t.starts_with('<') || t.starts_with("[Request interrupted")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_wrappers_are_skipped_as_preview() {
        assert!(is_command_or_meta("<command-name>/clear</command-name>"));
        assert!(is_command_or_meta("[Request interrupted by user]"));
        assert!(is_command_or_meta("   "));
        assert!(!is_command_or_meta("最適化できますか？"));
    }

    #[test]
    fn detect_distinguishes_codex_from_claude() {
        let codex = r#"{"type":"session_meta","payload":{}}"#;
        let claude = r#"{"type":"user","message":{"role":"user","content":"hi"}}"#;
        assert_eq!(detect(codex), Agent::Codex);
        assert_eq!(detect(claude), Agent::Claude);
    }

    #[test]
    fn discover_dir_finds_both_agents_recursively() {
        let tmp = std::env::temp_dir().join(format!("ccscope-dir-{}", std::process::id()));
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
