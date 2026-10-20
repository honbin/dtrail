//! App state. Applies key actions and exposes what the UI needs to draw.
//!
//! A tig-like two-pane layout: the session list on the left and the selected
//! session's conversation on the right. Focus moves between the panes; the
//! conversation follows the list cursor. Conversation state (cursor, per-item
//! fold, search) lives in [`ConversationView`].

use crate::conversation::ConversationView;
use crate::scroll::Scroll;
use crate::session::discovery::{self, is_command_or_meta};
use crate::session::{Agent, Session, SessionEntry};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    List,
    Conversation,
}

/// Normal navigation, editing a search/filter query, or a line-wise visual
/// selection in the conversation (for yanking).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Normal,
    Search,
    Visual,
}

pub struct App {
    entries: Vec<SessionEntry>,
    /// Indices into `entries` that pass the current filter (all when empty).
    filtered: Vec<usize>,
    filter_query: String,
    /// Show only this agent's sessions; `None` means all agents.
    agent_filter: Option<Agent>,
    /// Hide sessions whose preview is only a slash-command/meta line (or empty),
    /// i.e. those with no human prose, to cut noise when retracing.
    prose_only: bool,
    /// Selection and scroll offset over `filtered`.
    list: Scroll,
    conversation: Option<ConversationView>,
    /// `entries` index currently loaded into `conversation`, to avoid reparsing.
    loaded_index: Option<usize>,
    /// Opened with a single file (no list); `q` quits instead of returning.
    direct: bool,
    focus: Focus,
    /// Hide the unfocused pane and let the focused one fill the screen.
    maximized: bool,
    mode: Mode,
    /// Query being edited while in [`Mode::Search`].
    input_buffer: String,
    /// A transient one-line notice (e.g. "Yanked 3 lines"), shown until the next
    /// key. Cleared at the start of each input dispatch.
    notice: Option<String>,
    pub should_quit: bool,
}

impl App {
    /// Start on the session list.
    pub fn with_entries(entries: Vec<SessionEntry>) -> Self {
        let filtered = (0..entries.len()).collect();
        Self {
            entries,
            filtered,
            filter_query: String::new(),
            agent_filter: None,
            prose_only: false,
            list: Scroll::default(),
            conversation: None,
            loaded_index: None,
            direct: false,
            focus: Focus::List,
            maximized: false,
            mode: Mode::Normal,
            input_buffer: String::new(),
            notice: None,
            should_quit: false,
        }
    }

    /// Start with one session already open (path given on the command line).
    pub fn with_session(session: Session) -> Self {
        Self {
            entries: Vec::new(),
            filtered: Vec::new(),
            filter_query: String::new(),
            agent_filter: None,
            prose_only: false,
            list: Scroll::default(),
            conversation: Some(ConversationView::new(session)),
            loaded_index: None,
            direct: true,
            focus: Focus::Conversation,
            maximized: true,
            mode: Mode::Normal,
            input_buffer: String::new(),
            notice: None,
            should_quit: false,
        }
    }

    /// Record how many unparseable lines the open session had, for the status line.
    pub fn with_skipped_lines(mut self, n: usize) -> Self {
        if let Some(view) = self.conversation.as_mut() {
            view.set_skipped_lines(n);
        }
        self
    }

    pub fn focus(&self) -> Focus {
        self.focus
    }

    pub fn maximized(&self) -> bool {
        self.maximized
    }

    pub fn is_direct(&self) -> bool {
        self.direct
    }

    pub fn mode(&self) -> Mode {
        self.mode
    }

    pub fn input_buffer(&self) -> &str {
        &self.input_buffer
    }

    /// A transient notice to show on the reserved line, if any.
    pub fn notice(&self) -> Option<&str> {
        self.notice.as_deref()
    }

    pub fn set_notice(&mut self, text: impl Into<String>) {
        self.notice = Some(text.into());
    }

    pub fn clear_notice(&mut self) {
        self.notice = None;
    }

    /// Enter a line-wise visual selection in the conversation (no-op elsewhere).
    pub fn begin_visual(&mut self) {
        if self.focus == Focus::Conversation
            && let Some(view) = self.conversation.as_mut()
        {
            view.begin_visual();
            self.mode = Mode::Visual;
        }
    }

    /// Leave visual mode, dropping the selection.
    pub fn cancel_visual(&mut self) {
        self.mode = Mode::Normal;
        if let Some(view) = self.conversation.as_mut() {
            view.end_visual();
        }
    }

    /// Focus the conversation but keep the split, so the body can be scrolled
    /// with the list still visible beside it (a peek, not a full read).
    pub fn focus_conversation(&mut self) {
        if self.conversation.is_none() {
            return;
        }
        self.focus = Focus::Conversation;
        self.carry_filter_into_search();
    }

    /// Dive from the list into the conversation: focus it and read full-width.
    pub fn dive_into_conversation(&mut self) {
        if self.conversation.is_none() {
            return;
        }
        self.focus = Focus::Conversation;
        self.maximized = true;
        self.carry_filter_into_search();
    }

    /// Carry the list filter in as a body search so entering the conversation
    /// jumps to that word and `n`/`N` step through its occurrences.
    fn carry_filter_into_search(&mut self) {
        if !self.filter_query.is_empty() {
            let q = self.filter_query.clone();
            if let Some(view) = self.conversation.as_mut() {
                view.carry_search(&q);
            }
        }
    }

    /// Return to the split list (and drop the full-width read), or quit when
    /// opened directly.
    pub fn back(&mut self) {
        if self.direct {
            self.should_quit = true;
        } else {
            self.focus = Focus::List;
            self.maximized = false;
        }
    }

    /// Manual override of the full-width/split layout (independent of diving).
    pub fn toggle_maximize(&mut self) {
        self.maximized = !self.maximized;
    }

    pub fn start_search(&mut self) {
        self.mode = Mode::Search;
        // Pre-fill with the pane's current query so it can be edited.
        self.input_buffer = match self.focus {
            Focus::List => self.filter_query.clone(),
            Focus::Conversation => self
                .conversation
                .as_ref()
                .map(|v| v.search_query().to_string())
                .unwrap_or_default(),
        };
        // The list narrows live as you type; the conversation search is deferred
        // to Enter (search_confirm) so the body doesn't scroll and folds don't
        // pop open mid-search — here we only note where to return on cancel.
        match self.focus {
            Focus::List => self.apply_filter(),
            Focus::Conversation => {
                if let Some(view) = self.conversation.as_mut() {
                    view.begin_search();
                }
            }
        }
    }

    pub fn search_push(&mut self, c: char) {
        self.input_buffer.push(c);
        if self.focus == Focus::List {
            self.apply_filter();
        }
    }

    pub fn search_backspace(&mut self) {
        self.input_buffer.pop();
        if self.focus == Focus::List {
            self.apply_filter();
        }
    }

    /// Confirm the search. The conversation's search is applied here: reveal
    /// folds that contain a match and jump to the first.
    pub fn search_confirm(&mut self) {
        self.mode = Mode::Normal;
        if self.focus == Focus::Conversation {
            let query = self.input_buffer.clone();
            if let Some(view) = self.conversation.as_mut() {
                view.set_search(&query);
                view.accept_search();
            }
        }
    }

    /// Cancel: drop the query and, for the conversation, return to the origin.
    pub fn search_cancel(&mut self) {
        self.mode = Mode::Normal;
        self.input_buffer.clear();
        match self.focus {
            Focus::List => {
                self.filter_query.clear();
                self.recompute_filter(false);
            }
            Focus::Conversation => {
                if let Some(view) = self.conversation.as_mut() {
                    view.cancel_search();
                }
            }
        }
    }

    fn apply_filter(&mut self) {
        self.filter_query = self.input_buffer.clone();
        // A typed query lands on its first match so Enter opens it.
        self.recompute_filter(!self.filter_query.is_empty());
    }

    /// Next result: down one row in the list, or the next match in the conversation.
    pub fn search_next(&mut self) {
        match self.focus {
            Focus::List => self.list_down(1),
            Focus::Conversation => {
                if let Some(view) = self.conversation.as_mut() {
                    view.next_match();
                }
            }
        }
    }

    /// Go to the previous search result; see [`App::search_next`].
    pub fn search_prev(&mut self) {
        match self.focus {
            Focus::List => self.list_up(1),
            Focus::Conversation => {
                if let Some(view) = self.conversation.as_mut() {
                    view.prev_match();
                }
            }
        }
    }

    pub fn entries(&self) -> &[SessionEntry] {
        &self.entries
    }

    pub fn filtered(&self) -> &[usize] {
        &self.filtered
    }

    pub fn filter_query(&self) -> &str {
        &self.filter_query
    }

    /// Active agent filter, or `None` for all agents.
    pub fn agent_filter(&self) -> Option<Agent> {
        self.agent_filter
    }

    pub fn prose_only(&self) -> bool {
        self.prose_only
    }

    /// Toggle hiding command-only/empty-preview sessions (default off).
    pub fn toggle_prose_only(&mut self) {
        self.prose_only = !self.prose_only;
        self.recompute_filter(false);
    }

    /// Whether the loaded sessions span more than one agent. Gates both the
    /// list's agent tag and the agent filter, so the two stay consistent.
    pub fn has_multiple_agents(&self) -> bool {
        self.entries
            .first()
            .is_some_and(|first| self.entries.iter().any(|e| e.agent != first.agent))
    }

    /// Cycle the agent filter: all -> Claude -> Codex -> Cursor -> all. The exhaustive
    /// match means a new agent variant must be slotted in here. A no-op with a
    /// single agent, where the filter would only cycle through empty states.
    pub fn cycle_agent_filter(&mut self) {
        if !self.has_multiple_agents() {
            return;
        }
        self.agent_filter = match self.agent_filter {
            None => Some(Agent::Claude),
            Some(Agent::Claude) => Some(Agent::Codex),
            Some(Agent::Codex) => Some(Agent::Cursor),
            Some(Agent::Cursor) => None,
        };
        self.recompute_filter(false);
    }

    pub fn list_selected(&self) -> usize {
        self.list.cursor()
    }

    pub fn list_offset(&self) -> usize {
        self.list.offset()
    }

    pub fn sync_list_viewport(&mut self, height: u16) {
        self.list.resize(height, self.filtered.len());
    }

    pub fn list_page_down(&mut self) {
        self.list.page(true, self.filtered.len());
    }

    pub fn list_page_up(&mut self) {
        self.list.page(false, self.filtered.len());
    }

    pub fn list_half_page_down(&mut self) {
        self.list
            .scroll_by(self.list.half_page() as isize, self.filtered.len());
    }

    pub fn list_half_page_up(&mut self) {
        self.list
            .scroll_by(-(self.list.half_page() as isize), self.filtered.len());
    }

    pub fn list_down(&mut self, n: usize) {
        self.list.move_by(n as isize, self.filtered.len(), 0);
    }

    pub fn list_up(&mut self, n: usize) {
        self.list.move_by(-(n as isize), self.filtered.len(), 0);
    }

    pub fn list_top(&mut self) {
        self.list.to_top(self.filtered.len());
    }

    pub fn list_bottom(&mut self) {
        self.list.to_bottom(self.filtered.len());
    }

    /// Entry index the list cursor currently points at.
    fn selected_entry_index(&self) -> Option<usize> {
        self.filtered.get(self.list.cursor()).copied()
    }

    /// Load the selected session into the conversation pane if the selection
    /// changed since the last load. Called before drawing the conversation.
    pub fn sync_selection(&mut self) {
        let idx = self.selected_entry_index();
        if idx == self.loaded_index {
            return;
        }
        self.loaded_index = idx;
        self.conversation = idx
            .and_then(|i| self.entries.get(i))
            .and_then(|e| discovery::load(&e.path, Some(e.agent)).ok())
            .map(|(session, stats)| {
                let mut view = ConversationView::new(session);
                view.set_skipped_lines(stats.skipped_lines);
                view
            });
    }

    /// Re-filter the list. `to_first_match` puts the cursor on the first row;
    /// otherwise it stays on the same session when that session survives (so
    /// widening the list or toggling `p` doesn't land on an unrelated one), and
    /// goes to the top when it doesn't.
    fn recompute_filter(&mut self, to_first_match: bool) {
        let current = self.selected_entry_index();
        // Agent filter, prose-only filter, and text query compose with AND.
        let q = self.filter_query.to_lowercase();
        let agent = self.agent_filter;
        let prose_only = self.prose_only;
        self.filtered = self
            .entries
            .iter()
            .enumerate()
            .filter(|(_, e)| agent.is_none_or(|a| e.agent == a))
            .filter(|(_, e)| !prose_only || !is_command_or_meta(&e.summary))
            .filter(|(_, e)| {
                q.is_empty()
                    || e.project.to_lowercase().contains(&q)
                    || e.summary.to_lowercase().contains(&q)
            })
            .map(|(i, _)| i)
            .collect();
        let kept = current
            .filter(|_| !to_first_match)
            .and_then(|idx| self.filtered.iter().position(|&i| i == idx));
        self.list.restore(0, 0, self.filtered.len());
        if let Some(pos) = kept {
            self.list.jump_to(pos, self.filtered.len(), 0);
        }
    }

    pub fn conversation(&self) -> Option<&ConversationView> {
        self.conversation.as_ref()
    }

    pub fn conversation_mut(&mut self) -> Option<&mut ConversationView> {
        self.conversation.as_mut()
    }
}
