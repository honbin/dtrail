//! Translates key events into app actions, kept separate from state changes so
//! the keymap is easy to change. Dispatches per mode and focus, and holds the
//! pending state for multi-key chords like `gg`.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::app::{App, Focus, Mode};

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

    fn handle_list(&mut self, app: &mut App, key: KeyEvent, was_pending_g: bool) {
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => app.should_quit = true,
            KeyCode::Char('j') | KeyCode::Down => app.list_down(1),
            KeyCode::Char('k') | KeyCode::Up => app.list_up(1),
            KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                app.list_half_page_down()
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                app.list_half_page_up()
            }
            KeyCode::Char('f') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                app.list_page_down()
            }
            KeyCode::Char('b') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                app.list_page_up()
            }
            KeyCode::Char(' ') => app.list_page_down(),
            KeyCode::Char('g') => {
                if was_pending_g {
                    app.list_top();
                } else {
                    self.pending_g = true;
                }
            }
            KeyCode::Char('G') => app.list_bottom(),
            KeyCode::Char('a') => app.cycle_agent_filter(),
            KeyCode::Char('/') => app.start_search(),
            KeyCode::Char('O') => app.toggle_maximize(),
            KeyCode::Enter | KeyCode::Tab | KeyCode::Char('l') => app.dive_into_conversation(),
            _ => {}
        }
    }

    fn handle_conversation(&mut self, app: &mut App, key: KeyEvent, was_pending_g: bool) {
        match key.code {
            KeyCode::Char('q') | KeyCode::Char('h') => {
                app.back();
                return;
            }
            KeyCode::Tab => {
                if !app.is_direct() {
                    app.back();
                }
                return;
            }
            KeyCode::Char('O') => {
                app.toggle_maximize();
                return;
            }
            KeyCode::Char('/') => {
                app.start_search();
                return;
            }
            KeyCode::Char('n') => {
                app.search_next();
                return;
            }
            KeyCode::Char('N') => {
                app.search_prev();
                return;
            }
            _ => {}
        }

        let Some(view) = app.conversation_mut() else {
            return;
        };
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => view.cursor_down(1),
            KeyCode::Char('k') | KeyCode::Up => view.cursor_up(1),
            KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                view.half_page_down()
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                view.half_page_up()
            }
            KeyCode::Char('f') if key.modifiers.contains(KeyModifiers::CONTROL) => view.page_down(),
            KeyCode::Char('b') if key.modifiers.contains(KeyModifiers::CONTROL) => view.page_up(),
            KeyCode::Char('g') => {
                if was_pending_g {
                    view.cursor_top();
                } else {
                    self.pending_g = true;
                }
            }
            KeyCode::Char('G') => view.cursor_bottom(),
            KeyCode::Char(' ') => view.page_down(),
            KeyCode::Char('o') | KeyCode::Enter => view.toggle_fold_under_cursor(),
            _ => {}
        }
    }
}
