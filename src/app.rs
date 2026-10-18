//! App state. Applies key actions and exposes what the UI needs to draw.
//!
//! A tig-like two-pane layout: the session list on the left and the selected
//! session's conversation on the right. Focus moves between the panes; the
//! conversation follows the list cursor. Conversation state (cursor, per-item
//! fold, search) lives in [`ConversationView`].

use crate::conversation::ConversationView;
use crate::scroll::Scroll;
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
                self.recompute_filter();
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
        self.recompute_filter();
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
        // Narrowing lands on the first match (so Enter opens it); otherwise keep
        // the current selection, clamped into range. Reset the scroll to the top.
        let narrowed = !q.is_empty() || agent.is_some();
        let cursor = if narrowed { 0 } else { self.list.cursor() };
        self.list.restore(cursor, 0, self.filtered.len());
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
