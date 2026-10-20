//! Pure logic that flattens a session into display rows wrapped to a width.
//! No TUI state: the set of expanded fold ids is passed in, so the same input
//! yields the same rows (unit testable).

use std::collections::HashSet;

use crate::session::cursor::USER_QUERY_TAG;
use crate::session::discovery::split_tag_name;
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
/// Foldable items (tool input, tool output, thinking, multi-line system text,
/// tag-delimited regions of user text) are numbered in flatten order; an item's
/// body rows are emitted only when `fold` expands that id.
pub fn build_rows(session: &Session, fold: Fold, width: u16) -> Vec<Row> {
    let mut b = Builder::new(fold, false);
    walk(session, &mut b);
    wrap_rows(b.logical, width.max(1) as usize)
}

/// Searchable text of each foldable item, indexed by fold id. Lets search find
/// matches inside folded items without building and wrapping every row; it
/// shares [`walk`] with [`build_rows`] so the ids cannot drift apart.
pub fn fold_item_texts(session: &Session) -> Vec<String> {
    let collapsed = HashSet::new();
    let mut b = Builder::new(Fold::Ids(&collapsed), true);
    walk(session, &mut b);
    b.fold_texts.unwrap_or_default()
}

fn walk(session: &Session, b: &mut Builder) {
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
                        Block::Text(text) => b.user_text(text),
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
            RecordKind::System { subtype, text } => {
                if system_folds(text) {
                    b.system(subtype.as_deref(), text);
                } else if !text.is_empty() {
                    b.text(Role::System, text);
                }
            }
            RecordKind::Meta { .. } => {}
        }
    }
}

/// Rows [`Builder::body`] emits for `text`, so a header's line count matches
/// what unfolding shows, trailing newline included.
fn row_count(text: &str) -> usize {
    text.split('\n').count()
}

/// Long system text (e.g. Codex's developer instructions) can bury the
/// conversation; folding by shape rather than by tag keeps this agent-agnostic,
/// and one-liners such as Claude's status notices stay inline.
fn system_folds(text: &str) -> bool {
    text.trim_end().contains('\n')
}

/// Lines `start..=end` of a text block, from a line holding only `<name ...>`
/// to the matching `</name>` line.
struct Region<'a> {
    start: usize,
    end: usize,
    name: &'a str,
}

/// Outermost multi-line tag regions in user text. Harness context is often
/// wrapped this way; matching on shape rather than a list of tag names keeps
/// dtrail from tracking each agent's vocabulary. A user's own pasted markup
/// may fold too, which is harmless since folding hides nothing for good.
///
/// The one exception is Cursor's prompt wrapper, which the preview already
/// knows: folding it would hide what the user typed. Keep it the only one.
fn tagged_regions<'a>(lines: &[&'a str]) -> Vec<Region<'a>> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        if let Some(name) = open_tag(lines[i])
            && name != USER_QUERY_TAG
            && let Some(end) = matching_close(lines, i, name)
        {
            out.push(Region {
                start: i,
                end,
                name,
            });
            i = end + 1;
        } else {
            i += 1;
        }
    }
    out
}

/// The tag name if `line` is nothing but an opening tag (attributes allowed).
fn open_tag(line: &str) -> Option<&str> {
    let inner = line.trim().strip_prefix('<')?.strip_suffix('>')?;
    if inner.contains(['<', '>']) || inner.ends_with('/') {
        return None;
    }
    let (name, after) = split_tag_name(inner)?;
    (after.is_empty() || after.starts_with(char::is_whitespace)).then_some(name)
}

/// Index of the `</name>` line closing the tag opened at `open`, with at least
/// one line between them; same-name nesting is counted.
fn matching_close(lines: &[&str], open: usize, name: &str) -> Option<usize> {
    let close = format!("</{name}>");
    let mut depth = 0;
    for (j, line) in lines.iter().enumerate().skip(open + 1) {
        if open_tag(line) == Some(name) {
            depth += 1;
        } else if line.trim() == close {
            if depth == 0 {
                return (j > open + 1).then_some(j);
            }
            depth -= 1;
        }
    }
    None
}

struct Builder<'a> {
    logical: Vec<Row>,
    fold: Fold<'a>,
    next_fold_id: usize,
    /// Row index, block type and count of the trailing unknown placeholder.
    unknown_run: Option<(usize, String, usize)>,
    /// Each fold's searchable text, collected only for [`fold_item_texts`].
    fold_texts: Option<Vec<String>>,
}

impl<'a> Builder<'a> {
    fn new(fold: Fold<'a>, collect_texts: bool) -> Self {
        Builder {
            logical: Vec::new(),
            fold,
            next_fold_id: 0,
            unknown_run: None,
            fold_texts: collect_texts.then(Vec::new),
        }
    }

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

    /// User text with each tag region folded under a `<name>` header.
    fn user_text(&mut self, text: &str) {
        let lines: Vec<&str> = text.split('\n').collect();
        let mut next = 0;
        for r in tagged_regions(&lines) {
            for line in &lines[next..r.start] {
                self.plain(Role::UserText, line);
            }
            let region = lines[r.start..=r.end].join("\n");
            let (id, expanded) = self.next_fold(|| region.clone());
            self.header(
                Role::UserText,
                id,
                expanded,
                &format!("<{}> [{} lines]", r.name, row_count(&region)),
            );
            if expanded {
                self.body(Role::UserText, id, &region);
            }
            next = r.end + 1;
        }
        for line in &lines[next..] {
            self.plain(Role::UserText, line);
        }
    }

    /// Take the next fold id and whether that item is currently expanded,
    /// recording the item's searchable text when collecting.
    fn next_fold(&mut self, text: impl FnOnce() -> String) -> (usize, bool) {
        if let Some(texts) = &mut self.fold_texts {
            texts.push(text());
        }
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

    /// Consecutive placeholders of one type share a counted row: Codex can log
    /// a long run of the same unknown item, which would otherwise fill a screen.
    fn unknown(&mut self, block_type: &str) {
        let label = format!("[unknown block: {block_type}]");
        if let Some((idx, kind, count)) = &mut self.unknown_run
            && *idx + 1 == self.logical.len()
            && kind == block_type
        {
            *count += 1;
            self.logical[*idx].text = format!("{label} ×{count}");
            return;
        }
        self.unknown_run = Some((self.logical.len(), block_type.to_string(), 1));
        self.plain(Role::System, &label);
    }

    fn system(&mut self, subtype: Option<&str>, text: &str) {
        let (id, expanded) = self.next_fold(|| text.to_string());
        let line_count = row_count(text);
        let name = subtype.unwrap_or("system");
        self.header(
            Role::System,
            id,
            expanded,
            &format!("{name} [{line_count} lines]"),
        );
        if expanded {
            self.body(Role::System, id, text);
        }
    }

    fn tool_use(&mut self, name: &str, input: &serde_json::Value) {
        let (id, expanded) = self.next_fold(|| format!("{name} {}", tool_input_text(input)));
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
        let body = content.as_display_string();
        let (id, expanded) = self.next_fold(|| body.clone());
        // An empty result shows no body when unfolded.
        let line_count = if body.is_empty() { 0 } else { row_count(&body) };
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
        let (id, expanded) = self.next_fold(|| text.to_string());
        let line_count = row_count(text);
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

    /// Non-blank row texts, the shape every row assertion here compares.
    fn row_texts(session: &Session, fold: Fold) -> Vec<String> {
        build_rows(session, fold, 80)
            .into_iter()
            .map(|r| r.text)
            .filter(|t| !t.is_empty())
            .collect()
    }

    fn codex_rows(jsonl: &str, fold: Fold) -> Vec<String> {
        row_texts(&crate::session::codex::parse(jsonl).0, fold)
    }

    #[test]
    fn multi_line_system_text_starts_folded() {
        let jsonl = concat!(
            r#"{"type":"response_item","payload":{"type":"message","role":"developer","content":[{"type":"input_text","text":"<instructions>\nbe terse\n</instructions>"}]}}"#,
            "\n",
            r#"{"type":"response_item","payload":{"type":"message","role":"developer","content":[{"type":"input_text","text":"one line\n"}]}}"#,
            "\n",
        );
        assert_eq!(
            codex_rows(jsonl, Fold::Ids(&HashSet::new())),
            vec!["▸ developer [3 lines]", "one line"]
        );
        assert_eq!(
            codex_rows(jsonl, Fold::All),
            vec![
                "▾ developer [3 lines]",
                "<instructions>",
                "be terse",
                "</instructions>",
                "one line",
            ]
        );

        let (session, _) = crate::session::codex::parse(jsonl);
        assert_eq!(
            fold_item_texts(&session),
            vec!["<instructions>\nbe terse\n</instructions>"]
        );
    }

    #[test]
    fn a_run_of_one_unknown_type_shares_a_counted_row() {
        let item = |t: &str| format!(r#"{{"type":"response_item","payload":{{"type":"{t}"}}}}"#);
        let jsonl = [item("x"), item("x"), item("x"), item("y"), item("x")].join("\n");
        assert_eq!(
            codex_rows(&jsonl, Fold::All),
            vec![
                "[unknown block: x] ×3",
                "[unknown block: y]",
                "[unknown block: x]",
            ]
        );
    }

    fn user_text_rows(text: &str, fold: Fold) -> Vec<String> {
        let line = serde_json::json!({"type": "user", "message": {"content": text}}).to_string();
        row_texts(&crate::session::claude::parse(&line).0, fold)
    }

    #[test]
    fn a_tag_region_in_user_text_starts_folded() {
        let text = "see below\n<ctx kind=\"x\">\n  a\n  b\n</ctx>\nthanks";
        assert_eq!(
            user_text_rows(text, Fold::Ids(&HashSet::new())),
            vec!["● user", "see below", "▸ <ctx> [4 lines]", "thanks"]
        );
        assert_eq!(
            user_text_rows(text, Fold::All),
            vec![
                "● user",
                "see below",
                "▾ <ctx> [4 lines]",
                "<ctx kind=\"x\">",
                "  a",
                "  b",
                "</ctx>",
                "thanks",
            ]
        );
    }

    #[test]
    fn only_closed_multi_line_tag_regions_fold() {
        for text in [
            "<a>one line</a>",
            "<a>\nnever closed",
            "<a>\n</a>",
            "<br/>\nx\n</br>",
            "</a>\nx\n</a>",
            // Same name rule as the preview's harness-tag check.
            "<ns:a>\nx\n</ns:a>",
        ] {
            let rows = user_text_rows(text, Fold::Ids(&HashSet::new()));
            assert!(
                rows.iter().all(|r| !r.starts_with('▸')),
                "{text:?}: {rows:?}"
            );
        }
    }

    #[test]
    fn cursor_user_query_stays_open_while_injected_blocks_fold() {
        let text = "<attached_files>\na.rs\nb.rs\n</attached_files>\n<user_query>\nfix the\nparser\n</user_query>";
        let line = serde_json::json!({
            "role": "user",
            "message": {"content": [{"type": "text", "text": text}]}
        })
        .to_string();
        let (session, _) = crate::session::cursor::parse(&line);
        let rows = row_texts(&session, Fold::Ids(&HashSet::new()));
        assert_eq!(
            rows,
            vec![
                "● user",
                "▸ <attached_files> [4 lines]",
                "<user_query>",
                "fix the",
                "parser",
                "</user_query>",
            ]
        );
    }

    #[test]
    fn nested_tags_fold_as_their_outermost_region() {
        let lines = ["<a>", "<a>", "x", "</a>", "</a>", "<b>", "y", "</b>"];
        let regions = tagged_regions(&lines);
        let spans: Vec<_> = regions.iter().map(|r| (r.start, r.end, r.name)).collect();
        assert_eq!(spans, vec![(0, 4, "a"), (5, 7, "b")]);
    }

    #[test]
    fn tag_regions_align_with_fold_item_texts() {
        let jsonl = concat!(
            r#"{"type":"user","message":{"content":"<a>\nx\n</a>\nhi\n<b>\ny\n</b>"}}"#,
            "\n",
            r#"{"type":"assistant","message":{"content":[{"type":"thinking","thinking":"hmm"}]}}"#,
            "\n",
        );
        let (session, _) = crate::session::claude::parse(jsonl);
        let texts = fold_item_texts(&session);
        assert_eq!(texts, vec!["<a>\nx\n</a>", "<b>\ny\n</b>", "hmm"]);
        let ids: std::collections::BTreeSet<usize> = build_rows(&session, Fold::All, 100)
            .iter()
            .filter_map(|r| r.fold)
            .collect();
        assert!(ids.into_iter().eq(0..texts.len()));
    }

    #[test]
    fn every_fold_kind_aligns_with_fold_item_texts() {
        let jsonl = concat!(
            r#"{"type":"user","message":{"content":"<ctx>\nx\n</ctx>\nhello"}}"#,
            "\n",
            r#"{"type":"assistant","message":{"content":[{"type":"thinking","thinking":"hmm"},{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"ls"}}]}}"#,
            "\n",
            r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t1","content":"out"}]}}"#,
            "\n",
            r#"{"type":"system","subtype":"note","content":"a\nb"}"#,
            "\n",
        );
        let (session, _) = crate::session::claude::parse(jsonl);
        let texts = fold_item_texts(&session);
        assert_eq!(texts.len(), 5);
        assert_eq!(texts[0], "<ctx>\nx\n</ctx>");
        assert_eq!(texts[1], "hmm");
        assert!(texts[2].starts_with("Bash "));
        assert_eq!(texts[3], "out");
        assert_eq!(texts[4], "a\nb");
        let ids: std::collections::BTreeSet<usize> = build_rows(&session, Fold::All, 100)
            .iter()
            .filter_map(|r| r.fold)
            .collect();
        assert!(ids.into_iter().eq(0..texts.len()));
    }

    #[test]
    fn a_turn_header_ends_an_unknown_run() {
        let jsonl = concat!(
            r#"{"type":"assistant","message":{"content":[{"type":"x"}]}}"#,
            "\n",
            r#"{"type":"user","message":{"content":[{"type":"x"}]}}"#,
            "\n",
        );
        let (session, _) = crate::session::claude::parse(jsonl);
        assert_eq!(
            row_texts(&session, Fold::All),
            vec!["[unknown block: x]", "● user", "[unknown block: x]"]
        );
    }

    #[test]
    fn a_header_counts_the_rows_unfolding_shows() {
        let line =
            r#"{"type":"assistant","message":{"content":[{"type":"thinking","thinking":"a\n"}]}}"#;
        let (session, _) = crate::session::claude::parse(line);
        let rows = build_rows(&session, Fold::All, 80);
        assert_eq!(rows[0].text, "▾ thinking [2 lines]");
        assert_eq!(rows.len() - 1, 2);
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
        row_texts(&crate::session::claude::parse(&jsonl).0, Fold::All)
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
