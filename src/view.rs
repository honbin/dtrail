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
                    match block {
                        Block::Text(text) => {
                            if !pushed_header {
                                b.turn_gap();
                                b.plain(Role::UserHeader, "● user");
                                pushed_header = true;
                            }
                            b.text(Role::UserText, text);
                        }
                        Block::ToolResult {
                            tool_use_id,
                            content,
                            is_error,
                        } => b.tool_result(tool_use_id, content, *is_error),
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
                        Block::Unknown { block_type } => {
                            b.plain(Role::System, &format!("[unknown block: {block_type}]"));
                        }
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
                            let input = serde_json::to_string_pretty(input).unwrap_or_default();
                            out.push(format!("{name} {input}"));
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
            });
        }
    }

    /// A non-foldable row (no fold id).
    fn plain(&mut self, role: Role, text: &str) {
        self.logical.push(Row {
            role,
            text: text.to_string(),
            fold: None,
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
            });
        }
    }

    fn header(&mut self, role: Role, id: usize, expanded: bool, label: &str) {
        let marker = if expanded { '▾' } else { '▸' };
        self.logical.push(Row {
            role,
            text: format!("{marker} {label}"),
            fold: Some(id),
        });
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
            let pretty = serde_json::to_string_pretty(input).unwrap_or_default();
            self.body(Role::ToolBody, id, &pretty);
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

fn summarize_input(input: &serde_json::Value) -> String {
    // Prefer a common key for a one-line summary; fall back to compact JSON.
    for key in [
        "command",
        "file_path",
        "pattern",
        "path",
        "description",
        "query",
    ] {
        if let Some(v) = input.get(key).and_then(|v| v.as_str()) {
            return truncate_one_line(v, 60);
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
            });
            continue;
        }
        for chunk in wrap_line(&text, width) {
            out.push(Row {
                role: r.role,
                text: chunk,
                fold: r.fold,
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
}
