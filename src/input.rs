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
        // A notice (e.g. "Yanked N lines") lives for one keypress.
        app.clear_notice();

        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            app.should_quit = true;
            return;
        }

        match app.mode() {
            Mode::Search => return self.handle_search_input(app, key),
            Mode::Visual => return self.handle_visual(app, key, was_pending_g),
            Mode::Normal => {}
        }

        match app.focus() {
            Focus::List => self.handle_list(app, key, was_pending_g),
            Focus::Conversation => self.handle_conversation(app, key, was_pending_g),
        }
    }

    /// Bracketed paste: typed into the query while searching, ignored otherwise.
    /// Line breaks are dropped since the query is a single line.
    pub fn paste(&mut self, app: &mut App, text: &str) {
        if app.mode() != Mode::Search {
            return;
        }
        for c in text.chars().filter(|c| !c.is_control()) {
            app.search_push(c);
        }
    }

    /// Line-wise visual selection in the conversation: movement extends the
    /// selection, `y` yanks it to the clipboard, `v`/`V`/`Esc` cancel.
    fn handle_visual(&mut self, app: &mut App, key: KeyEvent, was_pending_g: bool) {
        match key.code {
            KeyCode::Esc | KeyCode::Char('v') | KeyCode::Char('V') => app.cancel_visual(),
            KeyCode::Char('y') => {
                let lines = app.conversation().and_then(|v| v.visual_range());
                let text = app.conversation().and_then(|v| v.selected_text());
                if let (Some((lo, hi)), Some(text)) = (lines, text) {
                    let n = hi - lo + 1;
                    app.yank(text);
                    app.set_notice(format!("Yanked {n} line{}", if n == 1 { "" } else { "s" }));
                }
                app.cancel_visual();
            }
            _ => {
                if let Some(view) = app.conversation_mut() {
                    self.handle_movement(view, key, was_pending_g);
                }
            }
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
            KeyCode::Char('p') => app.toggle_prose_only(),
            KeyCode::Char('/') => app.start_search(),
            KeyCode::Char('z') => app.toggle_maximize(),
            KeyCode::Enter => app.dive_into_conversation(),
            KeyCode::Tab | KeyCode::Char('l') | KeyCode::Right => app.focus_conversation(),
            _ => self.handle_movement(app, key, was_pending_g),
        }
    }

    fn handle_conversation(&mut self, app: &mut App, key: KeyEvent, was_pending_g: bool) {
        match key.code {
            KeyCode::Char('q') => app.back(),
            // Only `q` quits a directly opened session; these just mean "go left".
            KeyCode::Char('h') | KeyCode::Left | KeyCode::Tab if !app.is_direct() => app.back(),
            KeyCode::Char('z') => app.toggle_maximize(),
            KeyCode::Char('/') => app.start_search(),
            KeyCode::Char('n') => app.search_next(),
            KeyCode::Char('N') => app.search_prev(),
            KeyCode::Char('v') | KeyCode::Char('V') => app.begin_visual(),
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
            last_active: Some(SystemTime::UNIX_EPOCH),
            summary: "summary".to_string(),
            is_prompt: true,
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
        let mut app = App::with_session(claude::parse(&jsonl));
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
    fn entering_a_head_only_view_shows_loading_then_loads_the_whole_file() {
        let fixture = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/minimal-session.jsonl"
        );
        let mut loadable = entry("/a");
        loadable.path = PathBuf::from(fixture);
        let mut app = App::with_entries(vec![loadable]);
        app.sync_list_viewport(10);
        app.sync_selection();
        // Stand in for a file large enough to have been loaded head-only.
        app.conversation_mut()
            .unwrap()
            .set_partial(50 * 1024 * 1024);
        let mut input = InputState::default();

        input.handle(&mut app, code(KeyCode::Enter));
        assert_eq!(app.focus(), Focus::Conversation);
        assert_eq!(app.notice(), Some("loading 50 MB…"));

        // The event loop runs the load after drawing the notice.
        assert!(app.finish_pending_load());
        assert_eq!(app.notice(), None);
        assert_eq!(app.conversation().unwrap().partial_of(), None);
        assert!(!app.finish_pending_load(), "nothing left to load");
    }

    #[test]
    fn l_peeks_into_the_body_keeping_the_split_while_enter_reads_full_width() {
        // A real file so sync_selection can load a conversation to focus.
        let fixture = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/minimal-session.jsonl"
        );
        let mut loadable = entry("/a");
        loadable.path = PathBuf::from(fixture);
        let mut app = App::with_entries(vec![loadable]);
        app.sync_list_viewport(10);
        app.sync_selection(); // load the selected session's conversation
        let mut input = InputState::default();

        // l focuses the body but keeps the split, so the list stays visible.
        input.handle(&mut app, key('l'));
        assert_eq!(app.focus(), Focus::Conversation);
        assert!(!app.maximized(), "peek must not hide the list");

        // h returns to the list.
        input.handle(&mut app, key('h'));
        assert_eq!(app.focus(), Focus::List);

        // Enter dives full-width.
        input.handle(&mut app, code(KeyCode::Enter));
        assert_eq!(app.focus(), Focus::Conversation);
        assert!(app.maximized(), "Enter reads full-width");
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
    fn p_toggles_hiding_command_only_and_empty_previews() {
        let prose1 = entry("/a"); // entry() gives a plain "summary"
        let mut command = entry("/b");
        command.summary = "<command-name>/clear</command-name>".to_string();
        command.is_prompt = false;
        let prose2 = entry("/c");
        let mut app = App::with_entries(vec![prose1, command, prose2]);
        app.sync_list_viewport(10);
        let mut input = InputState::default();

        assert_eq!(app.filtered().len(), 3); // all shown by default

        // Cursor on the last session (a prose one) before hiding.
        input.handle(&mut app, key('G'));
        assert_eq!(app.list_selected(), 2);

        input.handle(&mut app, key('p'));
        assert!(app.prose_only());
        assert_eq!(app.filtered().len(), 2); // the command-only session is hidden
        assert_eq!(app.list_selected(), 1); // still on the same session, not the top

        input.handle(&mut app, key('p'));
        assert!(!app.prose_only());
        assert_eq!(app.filtered().len(), 3);
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
        let mut app = App::with_session(claude::parse(jsonl));
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

    #[test]
    fn h_does_not_quit_a_directly_opened_session() {
        let mut app = conversation_app();
        let mut input = InputState::default();
        input.handle(&mut app, key('h'));
        input.handle(&mut app, code(KeyCode::Left));
        assert!(!app.should_quit);
    }

    #[test]
    fn paste_goes_into_the_query_only_while_searching() {
        let mut app = list_app();
        let mut input = InputState::default();
        input.paste(&mut app, "ignored");
        assert_eq!(app.filter_query(), "");

        input.handle(&mut app, key('/'));
        input.paste(&mut app, "/b\n");
        assert_eq!(app.filter_query(), "/b");
        assert_eq!(app.filtered().len(), 1);
    }

    #[test]
    fn clearing_the_filter_keeps_the_selected_session() {
        let mut app = list_app();
        let mut input = InputState::default();
        input.handle(&mut app, key('/'));
        input.handle(&mut app, key('c'));
        assert_eq!(app.filtered(), &[2]);
        input.handle(&mut app, code(KeyCode::Esc));
        // Back on the full list, still on "/c" rather than whatever row 0 was.
        assert_eq!(app.list_selected(), 2);
    }

    #[test]
    fn visual_select_then_yank_copies_and_exits() {
        let mut app = conversation_app();
        let mut input = InputState::default();

        input.handle(&mut app, key('v'));
        assert_eq!(app.mode(), Mode::Visual);
        input.handle(&mut app, key('j')); // extend the selection by one row
        input.handle(&mut app, key('y'));

        assert_eq!(app.mode(), Mode::Normal);
        assert_eq!(app.notice(), Some("Yanked 2 lines"));
        // Left for the event loop to send, so tests never touch the terminal.
        assert_eq!(app.take_clipboard().as_deref(), Some("● user\nline0"));
    }

    #[test]
    fn esc_cancels_visual_without_yanking() {
        let mut app = conversation_app();
        let mut input = InputState::default();

        input.handle(&mut app, key('v'));
        input.handle(&mut app, code(KeyCode::Esc));

        assert_eq!(app.mode(), Mode::Normal);
        assert_eq!(app.notice(), None);
    }
}
