//! App state. Applies key actions and exposes what the UI needs to draw.
//!
//! A tig-like two-pane layout: the session list on the left and the selected
//! session's conversation on the right. Focus moves between the panes; the
//! conversation follows the list cursor. Conversation state (cursor, per-item
//! fold, search) lives in [`ConversationView`].

use crate::conversation::ConversationView;
use crate::session::{Agent, Session, SessionEntry};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    List,
    Conversation,
}

/// Normal navigation vs. editing a search/filter query.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Normal,
    Search,
}

pub struct App {
    entries: Vec<SessionEntry>,
    /// Indices into `entries` that pass the current filter (all when empty).
    filtered: Vec<usize>,
    filter_query: String,
    /// Show only this agent's sessions; `None` means all agents.
    agent_filter: Option<Agent>,
    list_selected: usize,
    list_offset: usize,
    list_height: u16,
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
            list_selected: 0,
            list_offset: 0,
            list_height: 0,
            conversation: None,
            loaded_index: None,
            direct: false,
            focus: Focus::List,
            maximized: false,
            mode: Mode::Normal,
            input_buffer: String::new(),
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
            list_selected: 0,
            list_offset: 0,
            list_height: 0,
            conversation: Some(ConversationView::new(session)),
            loaded_index: None,
            direct: true,
            focus: Focus::Conversation,
            maximized: true,
            mode: Mode::Normal,
            input_buffer: String::new(),
            should_quit: false,
        }
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

    /// Dive from the list into the conversation: focus it and read full-width.
    /// Carry the list filter in as a body search so it jumps to that word and
    /// `n`/`N` step through its occurrences.
    pub fn dive_into_conversation(&mut self) {
        if self.conversation.is_none() {
            return;
        }
        self.focus = Focus::Conversation;
        self.maximized = true;
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
        if self.focus == Focus::Conversation
            && let Some(view) = self.conversation.as_mut()
        {
            view.begin_search();
        }
        self.apply_query();
    }

    pub fn search_push(&mut self, c: char) {
        self.input_buffer.push(c);
        self.apply_query();
    }

    pub fn search_backspace(&mut self) {
        self.input_buffer.pop();
        self.apply_query();
    }

    /// Accept the query, keeping the current match position.
    pub fn search_confirm(&mut self) {
        self.mode = Mode::Normal;
        if self.focus == Focus::Conversation
            && let Some(view) = self.conversation.as_mut()
        {
            view.accept_search();
        }
    }

    /// Cancel: drop the query and, for the conversation, return to the origin.
    pub fn search_cancel(&mut self) {
        self.mode = Mode::Normal;
        self.input_buffer.clear();
        match self.focus {
            Focus::List => {
                self.filter_query.clear();
                self.recompute_filter();
            }
            Focus::Conversation => {
                if let Some(view) = self.conversation.as_mut() {
                    view.cancel_search();
                }
            }
        }
    }

    /// Push the edited query into the focused pane's search/filter.
    fn apply_query(&mut self) {
        match self.focus {
            Focus::List => {
                self.filter_query = self.input_buffer.clone();
                self.recompute_filter();
            }
            Focus::Conversation => {
                if let Some(view) = self.conversation.as_mut() {
                    view.set_search(&self.input_buffer);
                }
            }
        }
    }

    /// Go to the next search result: the next matching row in the list, or the
    /// next match in the conversation. Works both while editing a query and
    /// (for the conversation) after it is confirmed.
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

    /// Whether the loaded sessions span more than one agent. Gates both the
    /// list's agent tag and the agent filter, so the two stay consistent.
    pub fn has_multiple_agents(&self) -> bool {
        self.entries
            .first()
            .is_some_and(|first| self.entries.iter().any(|e| e.agent != first.agent))
    }

    /// Cycle the agent filter: all -> Claude -> Codex -> all. The exhaustive
    /// match means a new agent variant must be slotted in here. A no-op with a
    /// single agent, where the filter would only cycle through empty states.
    pub fn cycle_agent_filter(&mut self) {
        if !self.has_multiple_agents() {
            return;
        }
        self.agent_filter = match self.agent_filter {
            None => Some(Agent::Claude),
            Some(Agent::Claude) => Some(Agent::Codex),
            Some(Agent::Codex) => None,
        };
        self.recompute_filter();
    }

    pub fn list_selected(&self) -> usize {
        self.list_selected
    }

    pub fn list_offset(&self) -> usize {
        self.list_offset
    }

    pub fn sync_list_viewport(&mut self, height: u16) {
        self.list_height = height;
        self.ensure_list_visible();
    }

    pub fn list_half_page(&self) -> usize {
        (self.list_height as usize / 2).max(1)
    }

    /// A full page less one row, mirroring [`ConversationView::full_page`].
    pub fn list_full_page(&self) -> usize {
        (self.list_height as usize).saturating_sub(1).max(1)
    }

    pub fn list_down(&mut self, n: usize) {
        if self.filtered.is_empty() {
            return;
        }
        self.list_selected = (self.list_selected + n).min(self.filtered.len() - 1);
        self.ensure_list_visible();
    }

    pub fn list_up(&mut self, n: usize) {
        self.list_selected = self.list_selected.saturating_sub(n);
        self.ensure_list_visible();
    }

    pub fn list_top(&mut self) {
        self.list_selected = 0;
        self.ensure_list_visible();
    }

    pub fn list_bottom(&mut self) {
        self.list_selected = self.filtered.len().saturating_sub(1);
        self.ensure_list_visible();
    }

    /// Entry index the list cursor currently points at.
    fn selected_entry_index(&self) -> Option<usize> {
        self.filtered.get(self.list_selected).copied()
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
            .and_then(load_session)
            .map(ConversationView::new);
    }

    fn recompute_filter(&mut self) {
        // Agent filter and text query compose with AND.
        let q = self.filter_query.to_lowercase();
        let agent = self.agent_filter;
        self.filtered = self
            .entries
            .iter()
            .enumerate()
            .filter(|(_, e)| agent.is_none_or(|a| e.agent == a))
            .filter(|(_, e)| {
                q.is_empty()
                    || e.project.to_lowercase().contains(&q)
                    || e.summary.to_lowercase().contains(&q)
            })
            .map(|(i, _)| i)
            .collect();
        let narrowed = !q.is_empty() || agent.is_some();
        if narrowed {
            // Narrowing: land on the first match so Enter opens it.
            self.list_selected = 0;
        } else if self.list_selected >= self.filtered.len() {
            // No filter: just keep the selection in range.
            self.list_selected = self.filtered.len().saturating_sub(1);
        }
        self.list_offset = 0;
        self.ensure_list_visible();
    }

    /// Keep the selected row within the viewport by adjusting list scroll.
    fn ensure_list_visible(&mut self) {
        let h = self.list_height as usize;
        if h == 0 {
            return;
        }
        if self.list_selected < self.list_offset {
            self.list_offset = self.list_selected;
        } else if self.list_selected >= self.list_offset + h {
            self.list_offset = self.list_selected + 1 - h;
        }
    }

    pub fn conversation(&self) -> Option<&ConversationView> {
        self.conversation.as_ref()
    }

    pub fn conversation_mut(&mut self) -> Option<&mut ConversationView> {
        self.conversation.as_mut()
    }
}

fn load_session(entry: &SessionEntry) -> Option<Session> {
    let input = std::fs::read_to_string(&entry.path).ok()?;
    Some(crate::session::discovery::parse(entry.agent, &input).0)
}
