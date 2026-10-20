//! Agent identity, the list-row metadata type, and multi-agent discovery.
//!
//! Each supported agent keeps its own discovery + parsing in its module
//! ([`super::claude`], [`super::codex`], [`super::cursor`]); this file only ties
//! them together.

use std::cmp::Reverse;
use std::collections::HashSet;
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
    entries.sort_by_key(|e| Reverse(e.modified));
    entries
}

/// Fallback when a session has no `cwd`: undo the dir-name encoding (`-` to `/`).
pub(crate) fn decode_project_dir(dir: &Path) -> String {
    let name = dir.file_name().and_then(|n| n.to_str()).unwrap_or_default();
    name.replace('-', "/")
}

/// Session id from the file name (the id when the file is not named otherwise).
pub(crate) fn session_id_from_stem(path: &Path) -> String {
    path.file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or_default()
        .to_string()
}

/// Recursively collect `*.jsonl` files under `dir`. Unreadable dirs are skipped.
/// Symlinked dirs are followed, but each real dir is walked once so a link
/// cycle can't list the same files over and over.
pub(crate) fn collect_jsonl(dir: &Path, out: &mut Vec<PathBuf>) {
    walk_jsonl(dir, out, &mut HashSet::new());
}

fn walk_jsonl(dir: &Path, out: &mut Vec<PathBuf>, visited: &mut HashSet<PathBuf>) {
    let Ok(real) = fs::canonicalize(dir) else {
        return;
    };
    if !visited.insert(real) {
        return;
    }
    let Ok(rd) = fs::read_dir(dir) else {
        return;
    };
    for entry in rd.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk_jsonl(&path, out, visited);
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

/// Read and parse a session file. `agent` is detected from the content when not
/// known. Invalid UTF-8 is replaced rather than failing the whole file, since a
/// session still being written can end mid-character. A session id missing from
/// the content falls back to the file name.
pub fn load(path: &Path, agent: Option<Agent>) -> std::io::Result<(Session, ParseStats)> {
    let bytes = fs::read(path)?;
    let input = String::from_utf8_lossy(&bytes);
    let agent = agent.unwrap_or_else(|| detect(&input));
    let (mut session, stats) = parse(agent, &input);
    if session.session_id.is_none() {
        session.session_id = Some(session_id_from_stem(path));
    }
    Ok((session, stats))
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

/// Injected wrappers that aren't a real prompt: blank text, interrupt markers,
/// and anything opening with a harness-style tag such as `<command-name>`,
/// `<ide_selection>` or `<environment_context>`. Harness tags tend to be snake/kebab
/// case while HTML a user might paste ("<div> isn't working") is not, so
/// the shape tells them apart without tracking each agent's tag names.
///
/// This is a text heuristic for Claude and for Codex logs that predate the
/// per-turn origin metadata; current Codex logs are classified structurally
/// (see `codex::scan`).
pub(crate) fn is_command_or_meta(text: &str) -> bool {
    let t = text.trim_start();
    t.is_empty() || t.starts_with("[Request interrupted") || opens_with_harness_tag(t)
}

fn opens_with_harness_tag(t: &str) -> bool {
    let Some(rest) = t.strip_prefix('<') else {
        return false;
    };
    let end = rest
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '-'))
        .unwrap_or(rest.len());
    let (name, after) = rest.split_at(end);
    name.starts_with(|c: char| c.is_ascii_alphabetic())
        && name.contains(['_', '-'])
        && after.starts_with(|c: char| c == '>' || c == '/' || c.is_whitespace())
        && after.contains('>')
}

/// Chooses a session's preview from its user utterances (offered in file order):
/// the first real prompt, else the first non-empty raw utterance — never inventing
/// text. Per-format extraction of each utterance stays in each agent.
#[derive(Default)]
pub(crate) struct PreviewPicker {
    summary: Option<String>,
    raw_user: Option<String>,
}

impl PreviewPicker {
    /// Returns true once a real prompt is found, so a preview-only scan can stop.
    pub(crate) fn offer(&mut self, text: String) -> bool {
        if self.summary.is_some() {
            return true;
        }
        if is_command_or_meta(&text) {
            if self.raw_user.is_none() && !text.trim().is_empty() {
                self.raw_user = Some(text);
            }
            false
        } else {
            self.summary = Some(text);
            true
        }
    }

    /// A caller may still scan on for other metadata (e.g. `cwd`) after this is true.
    pub(crate) fn has_summary(&self) -> bool {
        self.summary.is_some()
    }

    pub(crate) fn finish(self) -> String {
        self.summary.or(self.raw_user).unwrap_or_default()
    }
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
        // Codex context injected as a user message in logs without origin metadata.
        assert!(is_command_or_meta(
            "<environment_context>\n  <cwd>/p</cwd>\n</environment_context>"
        ));
        assert!(is_command_or_meta(
            "<user_instructions>be terse</user_instructions>"
        ));
        // Tags the shape covers without being listed anywhere.
        assert!(is_command_or_meta(
            "<multi_agent_mode>be careful</multi_agent_mode> now fix it"
        ));
        assert!(is_command_or_meta("<skills_instructions>\n- a\n"));
        assert!(!is_command_or_meta("最適化できますか？"));
        // A real prompt that merely starts with `<` must not be treated as meta.
        assert!(!is_command_or_meta("<div> isn't working"));
        assert!(!is_command_or_meta("<T> in generics?"));
        assert!(!is_command_or_meta("<my_var means what"));
        assert!(!is_command_or_meta("<-- is this an arrow"));
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

    #[cfg(unix)]
    #[test]
    fn collect_jsonl_follows_symlinked_dirs_but_not_cycles() {
        let tmp = std::env::temp_dir().join(format!("dtrail-loop-{}", std::process::id()));
        let root = tmp.join("root");
        let elsewhere = tmp.join("elsewhere");
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&elsewhere).unwrap();
        fs::write(root.join("a.jsonl"), "").unwrap();
        fs::write(elsewhere.join("b.jsonl"), "").unwrap();
        // A legitimately linked-in dir, plus a link back to the root.
        std::os::unix::fs::symlink(&elsewhere, root.join("linked")).unwrap();
        std::os::unix::fs::symlink(&root, root.join("loop")).unwrap();

        let mut files = Vec::new();
        collect_jsonl(&root, &mut files);
        fs::remove_dir_all(&tmp).ok();

        let mut names: Vec<_> = files
            .iter()
            .map(|p| p.file_name().unwrap().to_str().unwrap().to_string())
            .collect();
        names.sort();
        assert_eq!(names, ["a.jsonl", "b.jsonl"]);
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
