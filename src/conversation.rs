//! Display state for one open session: a line cursor, per-item fold state, and
//! incremental search. Kept separate from [`crate::app`] so the whole-app state
//! and a single conversation's state don't share one file.

use std::collections::HashSet;

use crate::scroll::Scroll;
use crate::session::Session;
use crate::view::{self, Fold, Row, build_rows};

/// Rows of context kept above and below a search match so it never lands on an
/// edge. Plain cursor movement uses no scrolloff (the cursor reaches the edges).
const SEARCH_SCROLLOFF: usize = 3;

pub struct ConversationView {
    session: Session,
    /// Searchable text per foldable item (index = fold id), computed once since
    /// the session is immutable — avoids rescanning it on every keystroke.
    fold_texts: Vec<String>,
    /// Fold ids currently expanded; empty means everything is folded.
    expanded: HashSet<usize>,
    /// Items a search revealed that the user folded back; they stay folded until
    /// the query changes.
    collapsed: HashSet<usize>,
    /// Line cursor and scroll offset over `rows`.
    scroll: Scroll,
    rows: Vec<Row>,
    rows_width: u16,
    /// The effective fold set `rows` were last built from. Rows depend only on
    /// (session, this set, width), so a rebuild with all three unchanged is a
    /// no-op and can skip the flatten+wrap.
    last_effective: HashSet<usize>,
    search_query: String,
    matches: Vec<usize>,
    match_cursor: usize,
    /// `match_cursor` names the match `n` visits next rather than the one last
    /// visited, as after a fold moved the rows from under the cursor; `N` still
    /// goes to the one before it.
    match_ahead: bool,
    /// Cursor/offset captured when an incremental search began, so Esc can
    /// return there. `Some` only while a search is in progress.
    search_origin: Option<(usize, usize)>,
    /// A query to apply on the next layout, carried in from the list filter so
    /// it is applied at the real (full) width rather than the split width.
    carried_search: Option<String>,
    /// Row where a line-wise visual selection was started; the selection spans
    /// from here to the cursor. `None` outside visual mode.
    visual_anchor: Option<usize>,
    /// Lines of the file that failed to parse and are not shown.
    skipped_lines: usize,
    partial_of: Option<u64>,
}

impl ConversationView {
    pub fn new(session: Session) -> Self {
        let fold_texts = view::fold_item_texts(&session);
        Self {
            session,
            fold_texts,
            expanded: HashSet::new(),
            collapsed: HashSet::new(),
            scroll: Scroll::default(),
            rows: Vec::new(),
            rows_width: 0,
            last_effective: HashSet::new(),
            search_query: String::new(),
            matches: Vec::new(),
            match_cursor: 0,
            match_ahead: false,
            search_origin: None,
            carried_search: None,
            visual_anchor: None,
            skipped_lines: 0,
            partial_of: None,
        }
    }

    /// Mark this view as only the head of a larger file of `total_bytes`.
    pub fn set_partial(&mut self, total_bytes: u64) {
        self.partial_of = Some(total_bytes);
    }

    /// Size of the whole file when only its head is loaded.
    pub fn partial_of(&self) -> Option<u64> {
        self.partial_of
    }

    pub fn set_skipped_lines(&mut self, n: usize) {
        self.skipped_lines = n;
    }

    pub fn skipped_lines(&self) -> usize {
        self.skipped_lines
    }

    /// Start a line-wise visual selection anchored at the current row.
    pub fn begin_visual(&mut self) {
        self.visual_anchor = Some(self.scroll.cursor());
    }

    /// Leave visual mode, dropping the selection.
    pub fn end_visual(&mut self) {
        self.visual_anchor = None;
    }

    /// The selected row range (inclusive), or `None` when not selecting.
    pub fn visual_range(&self) -> Option<(usize, usize)> {
        let anchor = self.visual_anchor?;
        let cursor = self.scroll.cursor();
        Some((anchor.min(cursor), anchor.max(cursor)))
    }

    /// Text of the selected rows (as shown), for yanking. Soft-wrapped rows are
    /// rejoined so a long line copies as one line.
    pub fn selected_text(&self) -> Option<String> {
        if self.rows.is_empty() {
            return None;
        }
        let (lo, hi) = self.visual_range()?;
        let last = self.rows.len() - 1;
        let (lo, hi) = (lo.min(last), hi.min(last));
        let mut out = String::new();
        for (i, row) in self.rows[lo..=hi].iter().enumerate() {
            if i > 0 && !row.continued {
                out.push('\n');
            }
            out.push_str(&row.text);
        }
        Some(out)
    }

    pub fn session_id(&self) -> Option<&str> {
        self.session.session_id.as_deref()
    }

    /// First record timestamp (ISO8601, as recorded), if any.
    pub fn started_at(&self) -> Option<&str> {
        self.session.first_timestamp()
    }

    pub fn sync_viewport(&mut self, width: u16, height: u16) {
        self.scroll.set_height(height);
        if width != self.rows_width || self.rows.is_empty() {
            self.rebuild(width);
        }
        // Apply a carried-in query now that the real width is known.
        if let Some(query) = self.carried_search.take() {
            self.set_search(&query);
        }
        self.scroll.reclamp(self.rows.len());
    }

    /// Queue a query (from the list filter) to jump to on the next layout.
    pub fn carry_search(&mut self, query: &str) {
        self.carried_search = Some(query.to_string());
    }

    fn rebuild(&mut self, width: u16) {
        // Reveal items that contain a match so folded tool output is searchable;
        // the user's own fold state is left untouched.
        let effective = if self.search_query.is_empty() {
            self.expanded.clone()
        } else {
            let mut set = self.expanded.clone();
            set.extend(
                self.fold_ids_containing_match()
                    .into_iter()
                    .filter(|id| !self.collapsed.contains(id)),
            );
            set
        };
        if self.rows.is_empty() || width != self.rows_width || effective != self.last_effective {
            self.rows = build_rows(&self.session, Fold::Ids(&effective), width);
            self.rows_width = width;
            self.last_effective = effective;
        }
        self.recompute_matches();
    }

    /// Fold ids whose content contains the current query. Scans the cached item
    /// texts rather than building every row.
    fn fold_ids_containing_match(&self) -> HashSet<usize> {
        let q = self.search_query.to_lowercase();
        self.fold_texts
            .iter()
            .enumerate()
            .filter(|(_, text)| text.to_lowercase().contains(&q))
            .map(|(id, _)| id)
            .collect()
    }

    pub fn visible_rows(&self) -> &[Row] {
        let start = self.scroll.offset().min(self.rows.len());
        let end = (self.scroll.offset() + self.scroll.height() as usize).min(self.rows.len());
        &self.rows[start..end]
    }

    pub fn offset(&self) -> usize {
        self.scroll.offset()
    }

    pub fn cursor(&self) -> usize {
        self.scroll.cursor()
    }

    pub fn total_rows(&self) -> usize {
        self.rows.len()
    }

    pub fn viewport_height(&self) -> u16 {
        self.scroll.height()
    }

    pub fn cursor_down(&mut self, n: usize) {
        self.scroll.move_by(n as isize, self.rows.len(), 0);
    }

    pub fn cursor_up(&mut self, n: usize) {
        self.scroll.move_by(-(n as isize), self.rows.len(), 0);
    }

    pub fn cursor_top(&mut self) {
        self.scroll.to_top(self.rows.len());
    }

    pub fn cursor_bottom(&mut self) {
        self.scroll.to_bottom(self.rows.len());
    }

    pub fn half_page_down(&mut self) {
        self.scroll
            .scroll_by(self.scroll.half_page() as isize, self.rows.len());
    }

    pub fn half_page_up(&mut self) {
        self.scroll
            .scroll_by(-(self.scroll.half_page() as isize), self.rows.len());
    }

    pub fn page_down(&mut self) {
        self.scroll.page(true, self.rows.len());
    }

    pub fn page_up(&mut self) {
        self.scroll.page(false, self.rows.len());
    }

    /// Toggle the foldable item under the cursor, if any.
    pub fn toggle_fold_under_cursor(&mut self) {
        let Some(Some(id)) = self.rows.get(self.scroll.cursor()).map(|r| r.fold) else {
            return;
        };
        // Toggle what is shown, which may be open only because a search revealed it.
        if self.last_effective.contains(&id) {
            self.expanded.remove(&id);
            self.collapsed.insert(id);
        } else {
            self.expanded.insert(id);
            self.collapsed.remove(&id);
        }
        self.rebuild(self.rows_width.max(1));
        // Keep the cursor on the item's header row after the rebuild.
        match self.rows.iter().position(|r| r.fold == Some(id)) {
            Some(pos) => self.scroll.jump_to(pos, self.rows.len(), 0),
            None => self.scroll.reclamp(self.rows.len()),
        }
        // The match rows moved, so re-anchor `n`/`N` at the cursor.
        let cursor = self.scroll.cursor();
        self.match_cursor = self.matches.iter().position(|&r| r >= cursor).unwrap_or(0);
        self.match_ahead = self.matches.get(self.match_cursor) != Some(&cursor);
    }

    pub fn search_query(&self) -> &str {
        &self.search_query
    }

    /// `(k, total)` for the status line. `k` is the match last visited, or the
    /// one `n` goes to next after a fold (see `match_ahead`).
    pub fn match_info(&self) -> Option<(usize, usize)> {
        if self.search_query.is_empty() {
            None
        } else if self.matches.is_empty() {
            Some((0, 0))
        } else {
            Some((self.match_cursor + 1, self.matches.len()))
        }
    }

    /// Whether a row is a search match (for highlighting). `matches` is sorted.
    pub fn is_match(&self, row: usize) -> bool {
        self.matches.binary_search(&row).is_ok()
    }

    /// Begin an incremental search, remembering where to return on cancel.
    pub fn begin_search(&mut self) {
        self.search_origin = Some(self.scroll.snapshot());
    }

    /// Keep the match position and end the incremental session (Enter).
    pub fn accept_search(&mut self) {
        self.search_origin = None;
    }

    /// Abandon the search and return to where it began (Esc).
    pub fn cancel_search(&mut self) {
        let origin = self.search_origin.take();
        self.search_query.clear();
        self.collapsed.clear();
        self.match_ahead = false;
        self.rebuild(self.rows_width.max(1));
        match origin {
            Some((cursor, offset)) => self.scroll.restore(cursor, offset, self.rows.len()),
            None => self.scroll.reclamp(self.rows.len()),
        }
    }

    /// Update the live query, anchoring the jump at the search origin so the
    /// result stays stable as more characters are typed.
    pub fn set_search(&mut self, query: &str) {
        self.search_query = query.to_string();
        self.collapsed.clear();
        self.match_ahead = false;
        // Rebuild so matches inside folded items are revealed and indexed.
        self.rebuild(self.rows_width.max(1));

        let anchor = self
            .search_origin
            .map(|(c, _)| c)
            .unwrap_or(self.scroll.cursor());
        if self.search_query.is_empty() {
            // Deleting the whole query returns to the starting position.
            if let Some((c, o)) = self.search_origin {
                self.scroll.restore(c, o, self.rows.len());
            }
            return;
        }
        self.match_cursor = self.matches.iter().position(|&r| r >= anchor).unwrap_or(0);
        self.move_cursor_to_match();
    }

    pub fn next_match(&mut self) {
        if self.matches.is_empty() {
            return;
        }
        if !std::mem::take(&mut self.match_ahead) {
            self.match_cursor = (self.match_cursor + 1) % self.matches.len();
        }
        self.move_cursor_to_match();
    }

    pub fn prev_match(&mut self) {
        if self.matches.is_empty() {
            return;
        }
        self.match_ahead = false;
        self.match_cursor = (self.match_cursor + self.matches.len() - 1) % self.matches.len();
        self.move_cursor_to_match();
    }

    fn recompute_matches(&mut self) {
        self.matches.clear();
        if self.search_query.is_empty() {
            return;
        }
        let q = self.search_query.to_lowercase();
        for (i, row) in self.rows.iter().enumerate() {
            if row.text.to_lowercase().contains(&q) {
                self.matches.push(i);
            }
        }
        if self.match_cursor >= self.matches.len() {
            self.match_cursor = 0;
        }
    }

    fn move_cursor_to_match(&mut self) {
        if let Some(&row) = self.matches.get(self.match_cursor) {
            self.scroll.jump_to(row, self.rows.len(), SEARCH_SCROLLOFF);
        }
    }
}
