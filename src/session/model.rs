//! Internal model for display, kept separate from the raw JSONL shape.

use serde_json::Value;

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
