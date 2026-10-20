//! A scrollable viewport shared by the session list and the conversation: a
//! cursor (the selection / line cursor) plus the top-row offset over a list of
//! `len` items. The item count lives in the owner (filtered entries, rows) and
//! can change, so it is passed into each call rather than stored here.
//!
//! Movement keeps the cursor on screen. `scrolloff` is the rows of context kept
//! above and below it: 0 for plain movement (the cursor may sit on an edge),
//! more for a search jump so a match never lands on an edge.

#[derive(Default)]
pub struct Scroll {
    offset: usize,
    cursor: usize,
    height: u16,
}

impl Scroll {
    pub fn cursor(&self) -> usize {
        self.cursor
    }

    pub fn offset(&self) -> usize {
        self.offset
    }

    pub fn height(&self) -> u16 {
        self.height
    }

    pub fn half_page(&self) -> usize {
        (self.height as usize / 2).max(1)
    }

    /// A full page less one row, so a row of context carries across a page flip.
    pub fn full_page(&self) -> usize {
        (self.height as usize).saturating_sub(1).max(1)
    }

    pub fn set_height(&mut self, height: u16) {
        self.height = height;
    }

    /// Set the height and bring the cursor back on screen (e.g. after a resize).
    pub fn resize(&mut self, height: u16, len: usize) {
        self.height = height;
        self.reclamp(len);
    }

    /// Keep the cursor valid and visible after the row set or size changed.
    pub fn reclamp(&mut self, len: usize) {
        self.clamp_cursor(len);
        self.scroll_into_view(len, 0);
    }

    /// Move the cursor by a signed number of rows, keeping it visible.
    pub fn move_by(&mut self, delta: isize, len: usize, scrolloff: usize) {
        self.cursor = add(self.cursor, delta, len.saturating_sub(1));
        self.scroll_into_view(len, scrolloff);
    }

    pub fn to_top(&mut self, len: usize) {
        self.cursor = 0;
        self.scroll_into_view(len, 0);
    }

    pub fn to_bottom(&mut self, len: usize) {
        self.cursor = len.saturating_sub(1);
        self.scroll_into_view(len, 0);
    }

    /// Jump the cursor to a row (e.g. a search match) with context around it.
    pub fn jump_to(&mut self, cursor: usize, len: usize, scrolloff: usize) {
        self.cursor = cursor.min(len.saturating_sub(1));
        self.scroll_into_view(len, scrolloff);
    }

    /// Scroll the view and cursor together (vim Ctrl-d / Ctrl-u): the cursor
    /// keeps its screen position, drifting toward an edge only once the view can
    /// scroll no further.
    pub fn scroll_by(&mut self, delta: isize, len: usize) {
        self.offset = add(self.offset, delta, max_offset(len, self.height));
        self.cursor = add(self.cursor, delta, len.saturating_sub(1));
    }

    /// Flip a full page, landing the cursor at the top of the new page (vim
    /// Ctrl-f / Ctrl-b). When the view can't scroll further (including a list
    /// that fits on screen), the cursor goes to the last / first row instead.
    pub fn page(&mut self, forward: bool, len: usize) {
        let step = self.full_page() as isize;
        let delta = if forward { step } else { -step };
        let before = self.offset;
        self.offset = add(self.offset, delta, max_offset(len, self.height));
        self.cursor = if self.offset != before {
            self.offset.min(len.saturating_sub(1))
        } else if forward {
            len.saturating_sub(1)
        } else {
            0
        };
    }

    pub fn snapshot(&self) -> (usize, usize) {
        (self.cursor, self.offset)
    }

    /// Restore a (cursor, offset) captured by [`Self::snapshot`], then re-assert
    /// visibility in case the row set shrank in between.
    pub fn restore(&mut self, cursor: usize, offset: usize, len: usize) {
        self.cursor = cursor;
        self.offset = offset;
        self.clamp_cursor(len);
        self.scroll_into_view(len, 0);
    }

    fn clamp_cursor(&mut self, len: usize) {
        let max = len.saturating_sub(1);
        if self.cursor > max {
            self.cursor = max;
        }
    }

    fn scroll_into_view(&mut self, len: usize, scrolloff: usize) {
        let h = self.height as usize;
        if h == 0 {
            return;
        }
        let margin = scrolloff.min(h.saturating_sub(1) / 2);
        if self.cursor < self.offset + margin {
            self.offset = self.cursor.saturating_sub(margin);
        } else if self.cursor + margin >= self.offset + h {
            self.offset = (self.cursor + margin + 1).saturating_sub(h);
        }
        let max = max_offset(len, self.height);
        if self.offset > max {
            self.offset = max;
        }
    }
}

fn max_offset(len: usize, height: u16) -> usize {
    len.saturating_sub(height as usize)
}

/// Add a signed delta to an index, clamped to `0..=max`.
fn add(base: usize, delta: isize, max: usize) -> usize {
    (base as isize + delta).clamp(0, max as isize) as usize
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(height: u16, len: usize) -> Scroll {
        let mut s = Scroll::default();
        s.resize(height, len);
        s
    }

    #[test]
    fn full_page_steps_further_than_half_page() {
        let s = at(10, 100);
        assert_eq!(s.half_page(), 5);
        assert_eq!(s.full_page(), 9);
        assert!(s.full_page() > s.half_page());
    }

    #[test]
    fn plain_movement_lets_the_cursor_reach_the_edges() {
        let mut s = at(10, 100);
        // No scrolloff: the cursor rides to the bottom row before the view moves.
        for _ in 0..9 {
            s.move_by(1, 100, 0);
        }
        assert_eq!(s.cursor(), 9);
        assert_eq!(s.offset(), 0); // cursor on the bottom row, not scrolled yet
        s.move_by(1, 100, 0);
        assert_eq!(s.cursor(), 10);
        assert_eq!(s.offset(), 1); // now it scrolls, cursor stays on the edge
    }

    #[test]
    fn a_page_lands_the_cursor_at_the_top() {
        let mut s = at(10, 100);
        s.page(true, 100);
        assert_eq!(s.offset(), 9);
        assert_eq!(s.cursor(), 9); // top of the new page
        s.page(false, 100);
        assert_eq!((s.offset(), s.cursor()), (0, 0));
    }

    #[test]
    fn a_page_on_a_list_that_fits_moves_to_the_far_end() {
        let mut s = at(10, 4);
        s.page(true, 4);
        assert_eq!((s.offset(), s.cursor()), (0, 3));
        s.page(false, 4);
        assert_eq!((s.offset(), s.cursor()), (0, 0));
    }

    #[test]
    fn half_page_scroll_keeps_the_cursor_on_screen_not_at_an_edge() {
        let mut s = at(10, 100);
        s.page(true, 100); // offset 9, cursor 9 (top)
        let screen_pos = s.cursor() - s.offset();
        s.scroll_by(s.half_page() as isize, 100);
        // View and cursor moved together, so the cursor keeps its screen row.
        assert_eq!(s.cursor() - s.offset(), screen_pos);
        assert_eq!(s.offset(), 14);
        assert_eq!(s.cursor(), 14);
    }

    #[test]
    fn a_resize_pulls_the_cursor_back_into_view() {
        let mut s = at(20, 100);
        s.to_bottom(100);
        s.resize(5, 100);
        assert!(s.cursor() >= s.offset());
        assert!(s.cursor() < s.offset() + 5);
    }

    #[test]
    fn a_search_jump_keeps_context_around_the_match() {
        let mut s = at(10, 100);
        s.jump_to(50, 100, 3);
        assert!(s.cursor() >= s.offset() + 2); // context above
        assert!(s.cursor() + 2 < s.offset() + 10); // context below
    }
}
