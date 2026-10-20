//! Pure logic that flattens a session into display rows wrapped to a width.
//! No TUI state: the set of expanded fold ids is passed in, so the same input
//! yields the same rows (unit testable).

use std::collections::HashSet;

use crate::session::{Block, RecordKind, Session, ToolResultContent};
use crate::text::{sanitize_display, wrap_line};

/// Semantic role of a row; the UI maps this to a color.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    UserHeader,
    UserText,
    AssistantText,
    Thinking,
    ToolHeader,
    ToolBody,
    System,
}

/// A single display row, already wrapped to width.
#[derive(Debug, Clone)]
pub struct Row {
    pub text: String,
    pub role: Role,
    /// Foldable item this row belongs to, if any. Stable across rebuilds since
    /// ids are assigned in flatten order. A row with `Some` id can be toggled.
    pub fold: Option<usize>,
    /// Soft-wrapped continuation of the previous row, so a yank can rejoin it.
    pub continued: bool,
}

/// Which foldable items are expanded when building rows.
#[derive(Clone, Copy)]
pub enum Fold<'a> {
    /// Only the given ids are expanded (the display state).
    Ids(&'a HashSet<usize>),
    /// Every item expanded — used to search the full content.
    All,
}

impl Fold<'_> {
    fn is_expanded(&self, id: usize) -> bool {
        match self {
            Fold::Ids(set) => set.contains(&id),
            Fold::All => true,
        }
    }
}

/// Flatten a session into display rows wrapped to `width`.
///
/// Foldable items (tool input, tool output, thinking) are numbered in flatten
/// order; an item's body rows are emitted only when `fold` expands that id.
pub fn build_rows(session: &Session, fold: Fold, width: u16) -> Vec<Row> {
    let width = width.max(1) as usize;
    let mut b = Builder {
        logical: Vec::new(),
        fold,
        next_fold_id: 0,
    };

    for record in &session.records {
        match &record.kind {
            RecordKind::User(blocks) => {
                let mut pushed_header = false;
                for block in blocks {
                    // Tool results belong to the assistant's turn; any other
                    // block opens the user's, whatever its kind.
                    if !pushed_header && !matches!(block, Block::ToolResult { .. }) {
                        b.turn_gap();
                        b.plain(Role::UserHeader, "● user");
                        pushed_header = true;
                    }
                    match block {
                        Block::Text(text) => b.text(Role::UserText, text),
                        Block::ToolResult {
                            tool_use_id,
                            content,
                            is_error,
                        } => b.tool_result(tool_use_id, content, *is_error),
                        Block::Unknown { block_type } => b.unknown(block_type),
                        _ => {}
                    }
                }
            }
            RecordKind::Assistant(blocks) => {
                for block in blocks {
                    match block {
                        Block::Text(text) => {
                            b.turn_gap();
                            b.plain(Role::UserHeader, "◆ assistant");
                            b.text(Role::AssistantText, text);
                        }
                        Block::Thinking(text) => b.thinking(text),
                        Block::ToolUse { name, input, .. } => b.tool_use(name, input),
                        Block::Unknown { block_type } => b.unknown(block_type),
                        Block::ToolResult { .. } => {}
                    }
                }
            }
            RecordKind::System { text, .. } => {
                if !text.is_empty() {
                    b.text(Role::System, text);
                }
            }
            RecordKind::Meta { .. } => {}
        }
    }

    wrap_rows(b.logical, width)
}

/// Searchable text of each foldable item, in the same order [`build_rows`]
/// assigns fold ids (so the index is the fold id). Lets search find matches
/// inside folded items without building and wrapping every row.
pub fn fold_item_texts(session: &Session) -> Vec<String> {
    let mut out = Vec::new();
    for record in &session.records {
        match &record.kind {
            RecordKind::User(blocks) => {
                for block in blocks {
                    if let Block::ToolResult { content, .. } = block {
                        out.push(content.as_display_string());
                    }
                }
            }
            RecordKind::Assistant(blocks) => {
                for block in blocks {
                    match block {
                        Block::Thinking(text) => out.push(text.clone()),
                        Block::ToolUse { name, input, .. } => {
                            out.push(format!("{name} {}", tool_input_text(input)));
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
    out
}

struct Builder<'a> {
    logical: Vec<Row>,
    fold: Fold<'a>,
    next_fold_id: usize,
}

impl Builder<'_> {
    /// Blank separator before a speaker turn, unless at the top or already blank.
    fn turn_gap(&mut self) {
        if self.logical.last().is_some_and(|r| !r.text.is_empty()) {
            self.logical.push(Row {
                role: Role::UserText,
                text: String::new(),
                fold: None,
                continued: false,
            });
        }
    }

    /// A non-foldable row (no fold id).
    fn plain(&mut self, role: Role, text: &str) {
        self.logical.push(Row {
            role,
            text: text.to_string(),
            fold: None,
            continued: false,
        });
    }

    /// Non-foldable text that may contain newlines; one row per line.
    fn text(&mut self, role: Role, text: &str) {
        for line in text.split('\n') {
            self.plain(role, line);
        }
    }

    /// Take the next fold id and whether that item is currently expanded.
    fn next_fold(&mut self) -> (usize, bool) {
        let id = self.next_fold_id;
        self.next_fold_id += 1;
        (id, self.fold.is_expanded(id))
    }

    fn body(&mut self, role: Role, id: usize, text: &str) {
        for line in text.split('\n') {
            self.logical.push(Row {
                role,
                text: line.to_string(),
                fold: Some(id),
                continued: false,
            });
        }
    }

    fn header(&mut self, role: Role, id: usize, expanded: bool, label: &str) {
        let marker = if expanded { '▾' } else { '▸' };
        self.logical.push(Row {
            role,
            text: format!("{marker} {label}"),
            fold: Some(id),
            continued: false,
        });
    }

    fn unknown(&mut self, block_type: &str) {
        self.plain(Role::System, &format!("[unknown block: {block_type}]"));
    }

    fn tool_use(&mut self, name: &str, input: &serde_json::Value) {
        let (id, expanded) = self.next_fold();
        let summary = summarize_input(input);
        self.header(
            Role::ToolHeader,
            id,
            expanded,
            &format!("⚙ {name}  {summary}"),
        );
        if expanded {
            self.body(Role::ToolBody, id, &tool_input_text(input));
        }
    }

    fn tool_result(&mut self, _tool_use_id: &str, content: &ToolResultContent, is_error: bool) {
        let (id, expanded) = self.next_fold();
        let body = content.as_display_string();
        let line_count = body
            .lines()
            .count()
            .max(if body.is_empty() { 0 } else { 1 });
        let tag = if is_error { "error" } else { "result" };
        self.header(
            Role::ToolHeader,
            id,
            expanded,
            &format!("└ {tag} [{line_count} lines]"),
        );
        if expanded && !body.is_empty() {
            self.body(Role::ToolBody, id, &body);
        }
    }

    fn thinking(&mut self, text: &str) {
        let (id, expanded) = self.next_fold();
        let line_count = text.lines().count().max(1);
        self.header(
            Role::Thinking,
            id,
            expanded,
            &format!("thinking [{line_count} lines]"),
        );
        if expanded {
            self.body(Role::Thinking, id, text);
        }
    }
}

/// A tool's input as shown in its body. Freeform input (e.g. a Codex patch) is
/// a plain string; pretty-printing it as JSON would collapse it onto one line
/// of `\n` escapes.
fn tool_input_text(input: &serde_json::Value) -> String {
    match input {
        serde_json::Value::String(s) => s.clone(),
        other => serde_json::to_string_pretty(other).unwrap_or_default(),
    }
}

fn summarize_input(input: &serde_json::Value) -> String {
    if let Some(s) = input.as_str() {
        return truncate_one_line(s, 60);
    }
    // Prefer a common key for a one-line summary; fall back to compact JSON.
    for key in [
        "command",
        "file_path",
        "pattern",
        "path",
        "description",
        "query",
    ] {
        match input.get(key) {
            Some(serde_json::Value::String(v)) => return truncate_one_line(v, 60),
            // An argv array, e.g. Codex's `shell` command.
            Some(serde_json::Value::Array(parts)) if parts.iter().all(|p| p.is_string()) => {
                let argv: Vec<&str> = parts.iter().filter_map(|p| p.as_str()).collect();
                return truncate_one_line(&argv.join(" "), 60);
            }
            _ => {}
        }
    }
    truncate_one_line(&input.to_string(), 60)
}

fn truncate_one_line(s: &str, max: usize) -> String {
    let one = s.replace('\n', " ");
    if one.chars().count() > max {
        let kept: String = one.chars().take(max).collect();
        format!("{kept}…")
    } else {
        one
    }
}

/// Wrap each logical row to `width`; empty rows are kept as one line.
fn wrap_rows(logical: Vec<Row>, width: usize) -> Vec<Row> {
    let mut out = Vec::with_capacity(logical.len());
    for r in logical {
        let text = sanitize_display(&r.text);
        if text.is_empty() {
            out.push(Row {
                role: r.role,
                text,
                fold: r.fold,
                continued: false,
            });
            continue;
        }
        for (i, chunk) in wrap_line(&text, width).into_iter().enumerate() {
            out.push(Row {
                role: r.role,
                text: chunk,
                fold: r.fold,
                continued: i > 0,
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fold_item_texts_align_with_build_rows_ids() {
        let jsonl = concat!(
            r#"{"type":"assistant","message":{"content":[{"type":"thinking","thinking":"hmm"},{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"ls"}}]}}"#,
            "\n",
            r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t1","content":"out"}]}}"#,
            "\n",
        );
        let (session, _) = crate::session::claude::parse(jsonl);
        let texts = fold_item_texts(&session);
        let ids: std::collections::BTreeSet<usize> = build_rows(&session, Fold::All, 100)
            .iter()
            .filter_map(|r| r.fold)
            .collect();
        // Same count, and ids are exactly 0..len (no drift between the two).
        assert_eq!(ids.len(), texts.len());
        assert!(ids.into_iter().eq(0..texts.len()));
    }

    #[test]
    fn a_string_tool_input_keeps_its_line_breaks() {
        let input = serde_json::Value::String("*** Begin Patch\n*** End Patch".to_string());
        assert_eq!(tool_input_text(&input), "*** Begin Patch\n*** End Patch");
        assert_eq!(summarize_input(&input), "*** Begin Patch *** End Patch");
    }

    #[test]
    fn an_argv_command_is_summarized_as_a_command_line() {
        let input = serde_json::json!({"command": ["bash", "-lc", "ls"]});
        assert_eq!(summarize_input(&input), "bash -lc ls");
    }

    /// Texts of the rows for one user message with `content`, after an
    /// assistant turn so a misplaced placeholder would land in that turn.
    fn user_message_rows(content: &str) -> Vec<String> {
        let jsonl = format!(
            r#"{{"type":"assistant","message":{{"content":[{{"type":"text","text":"before"}}]}}}}
{{"type":"user","message":{{"content":{content}}}}}"#
        );
        let (session, _) = crate::session::claude::parse(&jsonl);
        build_rows(&session, Fold::All, 80)
            .into_iter()
            .map(|r| r.text)
            .filter(|t| !t.is_empty())
            .collect()
    }

    #[test]
    fn an_unknown_user_block_sits_under_the_user_header_in_any_order() {
        let text = r#"{"type":"text","text":"see"}"#;
        let image = r#"{"type":"image","source":{}}"#;
        for (content, expected) in [
            (
                format!("[{text},{image}]"),
                vec![
                    "◆ assistant",
                    "before",
                    "● user",
                    "see",
                    "[unknown block: image]",
                ],
            ),
            (
                format!("[{image},{text}]"),
                vec![
                    "◆ assistant",
                    "before",
                    "● user",
                    "[unknown block: image]",
                    "see",
                ],
            ),
            (
                format!("[{image}]"),
                vec!["◆ assistant", "before", "● user", "[unknown block: image]"],
            ),
        ] {
            assert_eq!(user_message_rows(&content), expected, "{content}");
        }
    }
}
