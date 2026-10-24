//! Internal model for display, kept separate from the raw JSONL shape.

use std::path::PathBuf;
use std::time::SystemTime;

use serde_json::Value;

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
    /// When the session last had activity (see [`super::scan::last_active`]).
    pub last_active: Option<SystemTime>,
    /// First real user message (command/meta wrappers skipped), else a raw
    /// fallback (see [`super::scan::PreviewPicker`]); empty if none.
    pub summary: String,
    /// Whether `summary` is a real prompt. Judged once while scanning, since
    /// `summary` is joined onto one line and no longer has the shape the
    /// judgment reads.
    pub is_prompt: bool,
}

/// Outcome of parsing a session file, shared by every agent's parser.
#[derive(Debug, Default)]
pub struct ParseStats {
    pub total_lines: usize,
    /// Lines that failed to parse as JSON (e.g. a partially written last line).
    pub skipped_lines: usize,
}

/// One session's records in file order.
///
/// An assistant turn is split across several records sharing a `message.id`,
/// but each record carries its own timestamp and file order is chronological,
/// so no reordering is done.
#[derive(Debug, Clone, Default)]
pub struct Session {
    pub session_id: Option<String>,
    pub records: Vec<Record>,
}

#[derive(Debug, Clone)]
pub struct Record {
    pub timestamp: Option<String>,
    pub kind: RecordKind,
}

#[derive(Debug, Clone)]
pub enum RecordKind {
    User(Vec<Block>),
    Assistant(Vec<Block>),
    System {
        subtype: Option<String>,
        text: String,
    },
    /// Non-conversational record; keeps only the original `type` string.
    Meta {
        record_type: String,
    },
}

#[derive(Debug, Clone)]
pub enum Block {
    Text(String),
    Thinking(String),
    ToolUse {
        /// Kept as recorded though the view doesn't read it yet: it is what a
        /// result would be matched to its tool use by (`tool_use_id`).
        id: String,
        name: String,
        /// Kept as raw JSON since input shape varies per tool.
        input: Value,
    },
    ToolResult {
        tool_use_id: String,
        content: ToolResultContent,
        is_error: bool,
    },
    /// Block type outside the known set; preserved rather than dropped.
    Unknown {
        block_type: String,
    },
}

/// `tool_result.content` is either a string or an array of
/// `text`/`image`/`tool_reference`. Binary (images) is reduced to a placeholder.
#[derive(Debug, Clone)]
pub enum ToolResultContent {
    Text(String),
    Blocks(Vec<String>),
}

impl ToolResultContent {
    pub fn as_display_string(&self) -> String {
        match self {
            ToolResultContent::Text(s) => s.clone(),
            ToolResultContent::Blocks(parts) => parts.join("\n"),
        }
    }
}

/// Shared line scan for every agent's `parse`: skip blank lines, count totals,
/// decode each line as JSON (counting failures), and hand the value to `f`.
/// Agent-specific interpretation (records, session id, timestamp) stays in `f`.
pub(crate) fn fold_lines(
    input: &str,
    mut f: impl FnMut(&mut Session, &Value),
) -> (Session, ParseStats) {
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

        f(&mut session, &value);
    }

    (session, stats)
}

impl Session {
    /// First record timestamp (ISO8601, as recorded), if any.
    pub fn first_timestamp(&self) -> Option<&str> {
        self.records.iter().find_map(|r| r.timestamp.as_deref())
    }
}
