//! Display state for one open session: a line cursor, per-item fold state, and
//! incremental search. Kept separate from [`crate::app`] so the whole-app state
//! and a single conversation's state don't share one file.

use std::collections::HashSet;

use crate::session::Session;
use crate::view::{self, Fold, Row, build_rows};

pub struct ConversationView {
    session: Session,
    /// Searchable text per foldable item (index = fold id), computed once since
    /// the session is immutable — avoids rescanning it on every keystroke.
    fold_texts: Vec<String>,
    /// Fold ids currently expanded; empty means everything is folded.
    expanded: HashSet<usize>,
    offset: usize,
    /// Highlighted row (the line cursor).
    cursor: usize,
    rows: Vec<Row>,
    rows_width: u16,
    /// The effective fold set `rows` were last built from. Rows depend only on
    /// (session, this set, width), so a rebuild with all three unchanged is a
    /// no-op and can skip the flatten+wrap.
    last_effective: HashSet<usize>,
    viewport_height: u16,
    search_query: String,
    matches: Vec<usize>,
    match_cursor: usize,
    /// Cursor/offset captured when an incremental search began, so Esc can
    /// return there. `Some` only while a search is in progress.
    search_origin: Option<(usize, usize)>,
    /// A query to apply on the next layout, carried in from the list filter so
    /// it is applied at the real (full) width rather than the preview width.
    carried_search: Option<String>,
}

impl ConversationView {
    pub fn new(session: Session) -> Self {
        let fold_texts = view::fold_item_texts(&session);
        Self {
            session,
            fold_texts,
            expanded: HashSet::new(),
            offset: 0,
            cursor: 0,
            rows: Vec::new(),
            rows_width: 0,
            last_effective: HashSet::new(),
            viewport_height: 0,
            search_query: String::new(),
            matches: Vec::new(),
            match_cursor: 0,
            search_origin: None,
            carried_search: None,
        }
    }

    pub fn session_id(&self) -> Option<&str> {
        self.session.session_id.as_deref()
    }

    /// First record timestamp (ISO8601, as recorded), if any.
    pub fn started_at(&self) -> Option<&str> {
        self.session.first_timestamp()
    }

    pub fn sync_viewport(&mut self, width: u16, height: u16) {
        self.viewport_height = height;
        if width != self.rows_width || self.rows.is_empty() {
            self.rebuild(width);
        }
        // Apply a carried-in query now that the real width is known.
        if let Some(query) = self.carried_search.take() {
            self.set_search(&query);
        }
        self.clamp_cursor();
        self.ensure_cursor_visible();
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
            set.extend(self.fold_ids_containing_match());
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
        let start = self.offset.min(self.rows.len());
        let end = (self.offset + self.viewport_height as usize).min(self.rows.len());
        &self.rows[start..end]
    }

    pub fn offset(&self) -> usize {
        self.offset
    }

    pub fn cursor(&self) -> usize {
        self.cursor
    }

    pub fn total_rows(&self) -> usize {
        self.rows.len()
    }

    pub fn viewport_height(&self) -> u16 {
        self.viewport_height
    }

    pub fn cursor_down(&mut self, n: usize) {
        self.cursor = self.cursor.saturating_add(n);
        self.clamp_cursor();
        self.ensure_cursor_visible();
    }

    pub fn cursor_up(&mut self, n: usize) {
        self.cursor = self.cursor.saturating_sub(n);
        self.ensure_cursor_visible();
    }

    pub fn cursor_top(&mut self) {
        self.cursor = 0;
        self.ensure_cursor_visible();
    }

    pub fn cursor_bottom(&mut self) {
        self.cursor = self.rows.len().saturating_sub(1);
        self.ensure_cursor_visible();
    }

    pub fn half_page(&self) -> usize {
        (self.viewport_height as usize / 2).max(1)
    }

    /// A full page less one line, so a line of context carries across the jump
    /// (vim's Ctrl-f / Ctrl-b).
    pub fn full_page(&self) -> usize {
        (self.viewport_height as usize).saturating_sub(1).max(1)
    }

    /// Toggle the foldable item under the cursor, if any.
    pub fn toggle_fold_under_cursor(&mut self) {
        let Some(Some(id)) = self.rows.get(self.cursor).map(|r| r.fold) else {
            return;
        };
        if !self.expanded.remove(&id) {
            self.expanded.insert(id);
        }
        self.rebuild(self.rows_width.max(1));
        // Keep the cursor on the item's header row after the rebuild.
        if let Some(pos) = self.rows.iter().position(|r| r.fold == Some(id)) {
            self.cursor = pos;
        }
        self.clamp_cursor();
        self.ensure_cursor_visible();
    }

    pub fn search_query(&self) -> &str {
        &self.search_query
    }

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
        self.search_origin = Some((self.cursor, self.offset));
    }

    /// Keep the match position and end the incremental session (Enter).
    pub fn accept_search(&mut self) {
        self.search_origin = None;
    }

    /// Abandon the search and return to where it began (Esc).
    pub fn cancel_search(&mut self) {
        let origin = self.search_origin.take();
        self.search_query.clear();
        self.rebuild(self.rows_width.max(1));
        if let Some((cursor, offset)) = origin {
            self.cursor = cursor;
            self.offset = offset;
        }
        self.clamp_cursor();
        self.ensure_cursor_visible();
    }

    /// Update the live query, anchoring the jump at the search origin so the
    /// result stays stable as more characters are typed.
    pub fn set_search(&mut self, query: &str) {
        self.search_query = query.to_string();
        // Rebuild so matches inside folded items are revealed and indexed.
        self.rebuild(self.rows_width.max(1));

        let anchor = self.search_origin.map(|(c, _)| c).unwrap_or(self.cursor);
        if self.search_query.is_empty() {
            // Deleting the whole query returns to the starting position.
            if let Some((c, o)) = self.search_origin {
                self.cursor = c;
                self.offset = o;
                self.clamp_cursor();
                self.ensure_cursor_visible();
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
        self.match_cursor = (self.match_cursor + 1) % self.matches.len();
        self.move_cursor_to_match();
    }

    pub fn prev_match(&mut self) {
        if self.matches.is_empty() {
            return;
        }
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
            self.cursor = row;
            self.clamp_cursor();
            self.ensure_cursor_visible();
        }
    }

    fn clamp_cursor(&mut self) {
        let max = self.rows.len().saturating_sub(1);
        if self.cursor > max {
            self.cursor = max;
        }
    }

    /// Scroll so the cursor stays within the viewport, keeping a few rows of
    /// context above and below (scrolloff) so a match is never glued to the
    /// top edge or the status bar at the bottom.
    fn ensure_cursor_visible(&mut self) {
        let h = self.viewport_height as usize;
        if h == 0 {
            return;
        }
        let margin = (h.saturating_sub(1) / 2).min(3);
        if self.cursor < self.offset + margin {
            self.offset = self.cursor.saturating_sub(margin);
        } else if self.cursor + margin >= self.offset + h {
            self.offset = (self.cursor + margin + 1).saturating_sub(h);
        }
        let max_offset = self.rows.len().saturating_sub(h);
        if self.offset > max_offset {
            self.offset = max_offset;
        }
    }
}
