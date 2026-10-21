//! Agent identity, the list-row metadata type, and multi-agent discovery.
//!
//! Each supported agent keeps its own discovery + parsing in its module
//! ([`super::claude`], [`super::codex`], [`super::cursor`]); this file only ties
//! them together.

use std::cmp::Reverse;
use std::collections::HashSet;
use std::fs;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde_json::Value;

use super::model::{ParseStats, Session};
use super::{claude, codex, cursor};
use crate::datefmt::parse_timestamp;
use crate::tags;

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
    /// When the session last had activity (see [`last_active`]).
    pub last_active: Option<SystemTime>,
    /// First real user message (command/meta wrappers skipped), else a raw
    /// fallback (see [`PreviewPicker`]); empty if none.
    pub summary: String,
    /// Whether `summary` is a real prompt. Judged once while scanning, since
    /// `summary` is joined onto one line and no longer has the shape the
    /// judgment reads.
    pub is_prompt: bool,
}

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

/// When a session last had activity: the last record `timestamp` in the file,
/// else the file mtime. The mtime alone also moves when something touches the
/// file without adding a record, which would lift an idle session to the top of
/// the list.
pub(crate) fn last_active(path: &Path) -> Option<SystemTime> {
    last_record_timestamp(path).or_else(|| fs::metadata(path).and_then(|m| m.modified()).ok())
}

/// Reads only a window at the end of the file, widening it until a timestamped
/// line is found: a single tool result can outgrow any fixed window.
fn last_record_timestamp(path: &Path) -> Option<SystemTime> {
    const FIRST_WINDOW: u64 = 16 * 1024;
    let mut file = fs::File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    let mut window = FIRST_WINDOW;
    loop {
        let start = len.saturating_sub(window);
        file.seek(SeekFrom::Start(start)).ok()?;
        let mut bytes = Vec::new();
        (&mut file).take(window).read_to_end(&mut bytes).ok()?;
        let lines = if start == 0 {
            &bytes[..]
        } else {
            // The window may begin mid-line; that partial line is not JSON.
            bytes
                .iter()
                .position(|&b| b == b'\n')
                .map_or(&[][..], |i| &bytes[i + 1..])
        };
        // Relies on file order being chronological (see `Session`), so the
        // lines before the last timestamped one need not be parsed.
        let newest = String::from_utf8_lossy(lines)
            .lines()
            .rev()
            .filter_map(|line| serde_json::from_str::<Value>(line.trim()).ok())
            .find_map(|v| parse_timestamp(v.get("timestamp")?.as_str()?));
        if newest.is_some() || start == 0 {
            return newest;
        }
        window *= 4;
    }
}

/// The JSON value of each line of a file, skipping blank and unparsable lines.
/// Lazy, so a scan can stop at the first lines it needs; unlike `fold_lines`
/// nothing is counted.
pub(crate) fn json_lines(path: &Path) -> Option<impl Iterator<Item = Value>> {
    let reader = BufReader::new(fs::File::open(path).ok()?);
    Some(
        reader
            .lines()
            .map_while(Result::ok)
            .filter_map(|line| serde_json::from_str(line.trim()).ok()),
    )
}

/// The part of `text` a preview shows, if it is a real prompt. Injected
/// wrappers aren't: blank text, interrupt markers, and text with nothing left
/// once harness-style tag scaffolding is set aside (see [`tags::prompt_text`]).
///
/// This is a text heuristic for Claude and for Codex logs that predate the
/// per-turn origin metadata; current Codex logs are classified structurally
/// (see `codex::scan`).
fn prompt_of(text: &str) -> Option<String> {
    if text.trim_start().starts_with("[Request interrupted") {
        return None;
    }
    tags::prompt_text(text)
}

/// Chooses a session's preview from its user utterances (offered in file order):
/// the first real prompt, else the raw utterance the agent first replied to (the
/// last one, if it never replied) — never inventing text. Per-format extraction of each utterance stays in each
/// agent.
///
/// The raw fallback takes the last utterance before the first reply rather than
/// the first one: a session started after `/clear` always opens with that
/// command, while the one the agent acted on tells sessions apart.
#[derive(Default)]
pub(crate) struct PreviewPicker {
    summary: Option<String>,
    raw_user: Option<String>,
    replied: bool,
}

impl PreviewPicker {
    /// Returns true once a real prompt is found, so a preview-only scan can stop.
    pub(crate) fn offer(&mut self, text: String) -> bool {
        if self.summary.is_some() {
            return true;
        }
        match prompt_of(&text) {
            Some(prompt) => {
                self.summary = Some(prompt);
                true
            }
            None => {
                let settled = self.replied && self.raw_user.is_some();
                if !settled && !text.trim().is_empty() {
                    self.raw_user = Some(text);
                }
                false
            }
        }
    }

    /// Called when the agent replies, which settles the raw fallback.
    pub(crate) fn reply_seen(&mut self) {
        self.replied = true;
    }

    /// A caller may still scan on for other metadata (e.g. `cwd`) after this is true.
    pub(crate) fn has_summary(&self) -> bool {
        self.summary.is_some()
    }

    /// The preview is shown and filtered as one line, so line breaks become
    /// spaces here rather than vanishing as control characters on display.
    pub(crate) fn finish(self) -> String {
        self.summary
            .or(self.raw_user)
            .unwrap_or_default()
            .split(['\r', '\n'])
            .filter(|line| !line.is_empty())
            .collect::<Vec<_>>()
            .join(" ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn is_command_or_meta(text: &str) -> bool {
        prompt_of(text).is_none()
    }

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
            "<multi_agent_mode>\nbe careful\n</multi_agent_mode>"
        ));
        assert!(is_command_or_meta("<skills_instructions>\n- a\n"));
        // Words after a tag on the same line are the user's.
        assert!(!is_command_or_meta(
            "<multi_agent_mode>be careful</multi_agent_mode> now fix it"
        ));
        assert!(!is_command_or_meta("最適化できますか？"));
        // A real prompt that merely starts with `<` must not be treated as meta.
        assert!(!is_command_or_meta("<div> isn't working"));
        assert!(!is_command_or_meta("<T> in generics?"));
        assert!(!is_command_or_meta("<my_var means what"));
        assert!(!is_command_or_meta("<-- is this an arrow"));
    }

    #[test]
    fn a_prompt_after_a_wrapped_paste_is_the_preview() {
        let mut picker = PreviewPicker::default();
        assert!(picker.offer(
            "<pasted_content id=\"1\">\nlong paste\n</pasted_content>\n\nreview this".to_string()
        ));
        assert_eq!(picker.finish(), "review this");
    }

    #[test]
    fn without_a_prompt_the_utterance_first_replied_to_is_the_preview() {
        let mut picker = PreviewPicker::default();
        picker.offer("<command-name>/clear</command-name>".to_string());
        picker.offer("<command-name>/check</command-name>".to_string());
        picker.reply_seen();
        picker.offer("<command-name>/later</command-name>".to_string());
        assert_eq!(picker.finish(), "<command-name>/check</command-name>");

        // A reply before any utterance settles on the first one after it.
        let mut picker = PreviewPicker::default();
        picker.reply_seen();
        picker.offer("<command-name>/a</command-name>".to_string());
        picker.offer("<command-name>/b</command-name>".to_string());
        assert_eq!(picker.finish(), "<command-name>/a</command-name>");
    }

    #[test]
    fn a_multi_line_prompt_becomes_one_spaced_line() {
        let mut picker = PreviewPicker::default();
        picker.offer("fix the\r\nparser\n\nplease".to_string());
        assert_eq!(picker.finish(), "fix the parser please");
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
    fn last_active_is_the_newest_record_not_the_mtime() {
        let tmp = std::env::temp_dir().join(format!("dtrail-active-{}", std::process::id()));
        fs::create_dir_all(&tmp).unwrap();
        let at = |ts: &str| parse_timestamp(ts).unwrap();

        // Written now, so the mtime is far newer than any record.
        let touched = tmp.join("touched.jsonl");
        fs::write(
            &touched,
            concat!(
                r#"{"type":"user","timestamp":"2026-01-01T00:00:00Z"}"#,
                "\n",
                r#"{"type":"assistant","timestamp":"2026-01-01T00:05:00Z"}"#,
                "\n",
                r#"{"type":"custom-title"}"#,
                "\n",
            ),
        )
        .unwrap();

        // A last record larger than the first tail window.
        let big = tmp.join("big.jsonl");
        let huge = "x".repeat(200 * 1024);
        fs::write(
            &big,
            format!(
                "{}\n{{\"type\":\"user\",\"timestamp\":\"2026-01-02T01:00:00Z\",\"t\":\"{huge}\"}}\n",
                r#"{"type":"user","timestamp":"2026-01-02T00:00:00Z"}"#,
            ),
        )
        .unwrap();

        let untimed = tmp.join("untimed.jsonl");
        fs::write(&untimed, "{\"role\":\"user\"}\n").unwrap();
        let mtime = fs::metadata(&untimed).unwrap().modified().unwrap();

        let results = (
            last_active(&touched),
            last_active(&big),
            last_active(&untimed),
        );
        fs::remove_dir_all(&tmp).ok();

        assert_eq!(results.0, Some(at("2026-01-01T00:05:00Z")));
        assert_eq!(results.1, Some(at("2026-01-02T01:00:00Z")));
        assert_eq!(results.2, Some(mtime));
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
