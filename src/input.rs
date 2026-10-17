//! Translates key events into app actions, kept separate from state changes so
//! the keymap is easy to change. Dispatches per mode and focus, and holds the
//! pending state for multi-key chords like `gg`.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::app::{App, Focus, Mode};
use crate::conversation::ConversationView;

/// A pane that scrolls. The movement keymap drives this uniformly so the list
/// and the conversation respond to the same keys without duplicating the wiring.
trait Scrollable {
    fn move_down(&mut self);
    fn move_up(&mut self);
    fn half_down(&mut self);
    fn half_up(&mut self);
    fn page_fwd(&mut self);
    fn page_back(&mut self);
    fn go_top(&mut self);
    fn go_bottom(&mut self);
}

impl Scrollable for ConversationView {
    fn move_down(&mut self) {
        self.cursor_down(1);
    }
    fn move_up(&mut self) {
        self.cursor_up(1);
    }
    fn half_down(&mut self) {
        self.half_page_down();
    }
    fn half_up(&mut self) {
        self.half_page_up();
    }
    fn page_fwd(&mut self) {
        self.page_down();
    }
    fn page_back(&mut self) {
        self.page_up();
    }
    fn go_top(&mut self) {
        self.cursor_top();
    }
    fn go_bottom(&mut self) {
        self.cursor_bottom();
    }
}

impl Scrollable for App {
    fn move_down(&mut self) {
        self.list_down(1);
    }
    fn move_up(&mut self) {
        self.list_up(1);
    }
    fn half_down(&mut self) {
        self.list_half_page_down();
    }
    fn half_up(&mut self) {
        self.list_half_page_up();
    }
    fn page_fwd(&mut self) {
        self.list_page_down();
    }
    fn page_back(&mut self) {
        self.list_page_up();
    }
    fn go_top(&mut self) {
        self.list_top();
    }
    fn go_bottom(&mut self) {
        self.list_bottom();
    }
}

#[derive(Default)]
pub struct InputState {
    /// A `g` was pressed and we are waiting for the second `g`.
    pending_g: bool,
}

impl InputState {
    pub fn handle(&mut self, app: &mut App, key: KeyEvent) {
        let was_pending_g = std::mem::take(&mut self.pending_g);

        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            app.should_quit = true;
            return;
        }

        if app.mode() == Mode::Search {
            self.handle_search_input(app, key);
            return;
        }

        match app.focus() {
            Focus::List => self.handle_list(app, key, was_pending_g),
            Focus::Conversation => self.handle_conversation(app, key, was_pending_g),
        }
    }

    /// While editing a query, keys build the string. Up/Down (or Ctrl-n/Ctrl-p)
    /// move the selection/match without leaving the query, and Enter commits:
    /// on the list it opens the highlighted session, in a conversation it keeps
    /// the current match. Esc cancels.
    fn handle_search_input(&mut self, app: &mut App, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Enter => {
                app.search_confirm();
                if app.focus() == Focus::List {
                    app.dive_into_conversation();
                }
            }
            KeyCode::Esc => app.search_cancel(),
            KeyCode::Backspace => app.search_backspace(),
            KeyCode::Down => app.search_next(),
            KeyCode::Up => app.search_prev(),
            KeyCode::Char('n') if ctrl => app.search_next(),
            KeyCode::Char('p') if ctrl => app.search_prev(),
            KeyCode::Char(c) => app.search_push(c),
            _ => {}
        }
    }

    /// The movement keymap, shared by both panes (see [`Scrollable`]). Returns
    /// whether the key was a movement key; `g` arms the `gg` chord.
    fn handle_movement(&mut self, s: &mut dyn Scrollable, key: KeyEvent, was_pending_g: bool) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => s.move_down(),
            KeyCode::Char('k') | KeyCode::Up => s.move_up(),
            KeyCode::Char('d') if ctrl => s.half_down(),
            KeyCode::Char('u') if ctrl => s.half_up(),
            KeyCode::Char('f') if ctrl => s.page_fwd(),
            KeyCode::Char('b') if ctrl => s.page_back(),
            KeyCode::Char(' ') => s.page_fwd(),
            KeyCode::Char('G') => s.go_bottom(),
            KeyCode::Char('g') => {
                if was_pending_g {
                    s.go_top();
                } else {
                    self.pending_g = true;
                }
            }
            _ => {}
        }
    }

    fn handle_list(&mut self, app: &mut App, key: KeyEvent, was_pending_g: bool) {
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => app.should_quit = true,
            KeyCode::Char('a') => app.cycle_agent_filter(),
            KeyCode::Char('/') => app.start_search(),
            KeyCode::Char('O') => app.toggle_maximize(),
            KeyCode::Enter | KeyCode::Tab | KeyCode::Char('l') => app.dive_into_conversation(),
            _ => self.handle_movement(app, key, was_pending_g),
        }
    }

    fn handle_conversation(&mut self, app: &mut App, key: KeyEvent, was_pending_g: bool) {
        match key.code {
            KeyCode::Char('q') | KeyCode::Char('h') => app.back(),
            KeyCode::Tab if !app.is_direct() => app.back(),
            KeyCode::Char('O') => app.toggle_maximize(),
            KeyCode::Char('/') => app.start_search(),
            KeyCode::Char('n') => app.search_next(),
            KeyCode::Char('N') => app.search_prev(),
            KeyCode::Char('o') | KeyCode::Enter => {
                if let Some(view) = app.conversation_mut() {
                    view.toggle_fold_under_cursor();
                }
            }
            _ => {
                if let Some(view) = app.conversation_mut() {
                    self.handle_movement(view, key, was_pending_g);
                }
            }
        }
    }
}
