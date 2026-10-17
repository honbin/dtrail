//! Navigation: full-page paging (Ctrl-f / Ctrl-b) in both panes.

use std::path::PathBuf;
use std::time::SystemTime;

use ccscope::app::App;
use ccscope::conversation::ConversationView;
use ccscope::session::claude;
use ccscope::session::{Agent, SessionEntry};

fn long_conversation(height: u16) -> ConversationView {
    let lines: Vec<String> = (0..60).map(|i| format!("line{i}")).collect();
    let content = lines.join("\\n"); // JSON-escaped newlines
    let jsonl = format!(
        r#"{{"type":"user","sessionId":"s","message":{{"role":"user","content":"{content}"}}}}"#
    );
    let (session, _) = claude::parse(&jsonl);
    let mut view = ConversationView::new(session);
    view.sync_viewport(80, height);
    view
}

#[test]
fn full_page_moves_about_a_screen_and_back() {
    let mut view = long_conversation(10);
    assert_eq!(view.cursor(), 0);

    let page = view.full_page();
    assert_eq!(page, 9); // viewport 10, one line of overlap

    view.cursor_down(page);
    assert_eq!(view.cursor(), page);

    view.cursor_up(page);
    assert_eq!(view.cursor(), 0);
}

#[test]
fn full_page_steps_further_than_half_page() {
    let view = long_conversation(10);
    assert!(view.full_page() > view.half_page());
}

fn app_with_entries(n: usize) -> App {
    let entries = (0..n)
        .map(|i| SessionEntry {
            agent: Agent::Claude,
            path: PathBuf::from("/x"),
            session_id: format!("id{i}"),
            project: format!("/p/proj{i}"),
            modified: Some(SystemTime::UNIX_EPOCH),
            summary: format!("summary {i}"),
        })
        .collect();
    App::with_entries(entries)
}

#[test]
fn list_full_page_pages_the_selection_and_back() {
    let mut app = app_with_entries(60);
    app.sync_list_viewport(10);
    assert_eq!(app.list_selected(), 0);

    let page = app.list_full_page();
    assert_eq!(page, 9);

    app.list_down(page);
    assert_eq!(app.list_selected(), page);

    app.list_up(page);
    assert_eq!(app.list_selected(), 0);
}
