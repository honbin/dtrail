//! What each agent's scan shares when it turns a session file into a list entry:
//! finding the files, reading their lines, dating them, and choosing the preview.

use std::collections::HashSet;
use std::fs;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde_json::Value;

use crate::datefmt::parse_timestamp;
use crate::tags;

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

/// When a session last had activity: the last record `timestamp` in the file,
/// else the file mtime. The mtime alone also moves when something touches the
/// file without adding a record, which would lift an idle session to the top of
/// the list.
pub(crate) fn last_active(path: &Path) -> Option<SystemTime> {
    last_record_time(path).or_else(|| fs::metadata(path).and_then(|m| m.modified()).ok())
}

/// Reads only a window at the end of the file, widening it until a line with a
/// timestamp is found: a single tool result can outgrow any fixed window.
fn last_record_time(path: &Path) -> Option<SystemTime> {
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
        // last line with a timestamp decides. One that doesn't parse (or isn't a
        // string) falls back to the mtime rather than to an older line, so a
        // changed format costs one window, not a read of the whole file.
        let last = String::from_utf8_lossy(lines)
            .lines()
            .rev()
            .filter_map(|line| serde_json::from_str::<Value>(line.trim()).ok())
            .find_map(|v| {
                v.get("timestamp")
                    .map(|t| t.as_str().and_then(parse_timestamp))
            });
        if let Some(time) = last {
            return time;
        }
        if start == 0 {
            return None;
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
    /// The next user utterance. Once a real prompt is found the rest are
    /// ignored, and a preview-only scan can stop (see [`Self::has_summary`]).
    pub(crate) fn offer(&mut self, text: String) {
        if self.summary.is_some() {
            return;
        }
        match prompt_of(&text) {
            Some(prompt) => self.summary = Some(prompt),
            None => {
                let settled = self.replied && self.raw_user.is_some();
                if !settled && !text.trim().is_empty() {
                    self.raw_user = Some(text);
                }
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
        picker.offer(
            "<pasted_content id=\"1\">\nlong paste\n</pasted_content>\n\nreview this".to_string(),
        );
        assert!(picker.has_summary());
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
    fn last_active_is_the_last_record_not_the_mtime() {
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
        let untimed_mtime = fs::metadata(&untimed).unwrap().modified().unwrap();

        // A last timestamp in an unreadable format: the mtime, not the older line.
        let unread = tmp.join("unread.jsonl");
        fs::write(
            &unread,
            concat!(
                r#"{"type":"user","timestamp":"2026-01-03T00:00:00Z"}"#,
                "\n",
                r#"{"type":"user","timestamp":"2026-01-03T09:00:00+09:00"}"#,
                "\n",
            ),
        )
        .unwrap();
        let unread_mtime = fs::metadata(&unread).unwrap().modified().unwrap();

        // Likewise a timestamp that isn't a string.
        let numeric = tmp.join("numeric.jsonl");
        fs::write(
            &numeric,
            concat!(
                r#"{"type":"user","timestamp":"2026-01-04T00:00:00Z"}"#,
                "\n",
                r#"{"type":"user","timestamp":1767484800000}"#,
                "\n",
            ),
        )
        .unwrap();
        let numeric_mtime = fs::metadata(&numeric).unwrap().modified().unwrap();

        let results = (
            last_active(&touched),
            last_active(&big),
            last_active(&untimed),
            last_active(&unread),
            last_active(&numeric),
        );
        fs::remove_dir_all(&tmp).ok();

        assert_eq!(results.0, Some(at("2026-01-01T00:05:00Z")));
        assert_eq!(results.1, Some(at("2026-01-02T01:00:00Z")));
        assert_eq!(results.2, Some(untimed_mtime));
        assert_eq!(results.3, Some(unread_mtime));
        assert_eq!(results.4, Some(numeric_mtime));
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
}
