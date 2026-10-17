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

    /// The movement keymap, shared by both panes (see [`Scrollable`]); `g` arms
    /// the `gg` chord. Non-movement keys fall through untouched.
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
            KeyCode::Char('q') => app.should_quit = true,
            KeyCode::Char('a') => app.cycle_agent_filter(),
            KeyCode::Char('/') => app.start_search(),
            KeyCode::Char('z') => app.toggle_maximize(),
            KeyCode::Enter | KeyCode::Tab | KeyCode::Char('l') => app.dive_into_conversation(),
            _ => self.handle_movement(app, key, was_pending_g),
        }
    }

    fn handle_conversation(&mut self, app: &mut App, key: KeyEvent, was_pending_g: bool) {
        match key.code {
            KeyCode::Char('q') | KeyCode::Char('h') => app.back(),
            KeyCode::Tab if !app.is_direct() => app.back(),
            KeyCode::Char('z') => app.toggle_maximize(),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::{Agent, SessionEntry, claude};
    use std::path::PathBuf;
    use std::time::SystemTime;

    fn key(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
    }
    fn ctrl(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }
    fn code(c: KeyCode) -> KeyEvent {
        KeyEvent::new(c, KeyModifiers::NONE)
    }

    fn entry(project: &str) -> SessionEntry {
        SessionEntry {
            agent: Agent::Claude,
            path: PathBuf::from("/x"),
            session_id: "id".to_string(),
            project: project.to_string(),
            modified: Some(SystemTime::UNIX_EPOCH),
            summary: "summary".to_string(),
        }
    }

    fn list_app() -> App {
        let mut app = App::with_entries(vec![entry("/a"), entry("/b"), entry("/c")]);
        app.sync_list_viewport(10);
        app
    }

    fn conversation_app() -> App {
        let lines: Vec<String> = (0..40).map(|i| format!("line{i}")).collect();
        let content = lines.join("\\n");
        let jsonl = format!(
            r#"{{"type":"user","sessionId":"s","message":{{"role":"user","content":"{content}"}}}}"#
        );
        let mut app = App::with_session(claude::parse(&jsonl).0);
        app.conversation_mut().unwrap().sync_viewport(80, 10);
        app
    }

    #[test]
    fn q_quits_the_list_but_esc_does_not() {
        let mut app = list_app();
        let mut input = InputState::default();

        input.handle(&mut app, code(KeyCode::Esc));
        assert!(!app.should_quit, "Esc must not quit the list");

        input.handle(&mut app, key('q'));
        assert!(app.should_quit);
    }

    #[test]
    fn z_toggles_zoom() {
        let mut app = list_app();
        let mut input = InputState::default();
        assert!(!app.maximized());
        input.handle(&mut app, key('z'));
        assert!(app.maximized());
        input.handle(&mut app, key('z'));
        assert!(!app.maximized());
    }

    #[test]
    fn jk_and_gg_g_move_the_list_selection() {
        let mut app = list_app();
        let mut input = InputState::default();

        input.handle(&mut app, key('j'));
        assert_eq!(app.list_selected(), 1);
        input.handle(&mut app, key('k'));
        assert_eq!(app.list_selected(), 0);

        input.handle(&mut app, key('G'));
        assert_eq!(app.list_selected(), 2);
        input.handle(&mut app, key('g')); // gg chord
        input.handle(&mut app, key('g'));
        assert_eq!(app.list_selected(), 0);
    }

    #[test]
    fn slash_opens_search_mode() {
        let mut app = list_app();
        let mut input = InputState::default();
        input.handle(&mut app, key('/'));
        assert_eq!(app.mode(), Mode::Search);
    }

    #[test]
    fn paging_keys_flip_a_page_with_the_cursor_at_the_top() {
        let mut app = conversation_app();
        let mut input = InputState::default();

        input.handle(&mut app, key('j'));
        assert_eq!(app.conversation().unwrap().cursor(), 1);

        // Ctrl-f and Space both page forward to the top of the new page.
        for k in [ctrl('f'), key(' ')] {
            let before = app.conversation().unwrap().offset();
            input.handle(&mut app, k);
            let view = app.conversation().unwrap();
            assert!(view.offset() > before);
            assert_eq!(view.cursor(), view.offset());
        }
    }

    #[test]
    fn o_and_enter_toggle_the_fold_under_the_cursor() {
        let jsonl = concat!(
            r#"{"type":"assistant","sessionId":"s","message":{"content":[{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"ls"}}]}}"#,
            "\n",
            r#"{"type":"user","sessionId":"s","message":{"content":[{"type":"tool_result","tool_use_id":"t1","content":"one\ntwo"}]}}"#,
            "\n",
        );
        let mut app = App::with_session(claude::parse(jsonl).0);
        app.conversation_mut().unwrap().sync_viewport(80, 20);
        let mut input = InputState::default();

        let folded = app.conversation().unwrap().total_rows();
        input.handle(&mut app, key('j')); // move onto the tool_result header
        input.handle(&mut app, key('o')); // expand
        assert!(app.conversation().unwrap().total_rows() > folded);
        input.handle(&mut app, code(KeyCode::Enter)); // Enter folds it back
        assert_eq!(app.conversation().unwrap().total_rows(), folded);
    }

    #[test]
    fn q_exits_a_directly_opened_session() {
        let mut app = conversation_app(); // opened directly (with_session)
        let mut input = InputState::default();
        input.handle(&mut app, key('q'));
        assert!(app.should_quit);
    }
}
