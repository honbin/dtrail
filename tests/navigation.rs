//! Navigation wiring: paging and half-page scroll in both panes. The scroll
//! maths themselves are unit-tested in `src/scroll.rs`; these check the panes
//! drive a `Scroll` correctly. Viewport height is 10, so a full page is 9 rows
//! and a half page is 5.

use std::path::PathBuf;
use std::time::SystemTime;

use dtrail::app::App;
use dtrail::conversation::ConversationView;
use dtrail::session::claude;
use dtrail::session::{Agent, SessionEntry};

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
fn page_down_advances_a_page_with_cursor_at_the_top() {
    let mut view = long_conversation(10);
    assert_eq!((view.offset(), view.cursor()), (0, 0));

    view.page_down();
    assert_eq!(view.offset(), 9); // advanced a full page
    assert_eq!(view.cursor(), view.offset()); // cursor at the top of it

    view.page_up();
    assert_eq!((view.offset(), view.cursor()), (0, 0));
}

#[test]
fn a_page_jump_survives_a_plain_redraw() {
    let mut view = long_conversation(10);
    view.page_down();
    let (off, cur) = (view.offset(), view.cursor());

    // A redraw at the same size must not reposition the cursor.
    view.sync_viewport(80, 10);
    assert_eq!((view.offset(), view.cursor()), (off, cur));
}

#[test]
fn moving_down_after_a_page_jump_does_not_snap() {
    let mut view = long_conversation(10);
    view.page_down();
    let off = view.offset();

    // One line down moves the cursor by one without the view jumping.
    view.cursor_down(1);
    assert_eq!(view.offset(), off);
    assert_eq!(view.cursor(), off + 1);
}

#[test]
fn half_page_scroll_keeps_the_cursor_off_the_bottom() {
    let mut view = long_conversation(10);
    view.cursor_down(5); // cursor to the middle of the screen
    let screen_row = view.cursor() - view.offset();

    // A half page scrolls view and cursor together, so the cursor keeps its
    // screen row rather than drifting down to the bottom edge.
    view.half_page_down();
    assert_eq!(view.cursor() - view.offset(), screen_row);
}

#[test]
fn a_resize_still_pulls_the_cursor_back_into_view() {
    let mut view = long_conversation(20);
    view.cursor_bottom();
    let cur = view.cursor();

    view.sync_viewport(80, 5);
    assert_eq!(view.cursor(), cur);
    assert!(view.cursor() >= view.offset());
    assert!(view.cursor() < view.offset() + 5);
}

#[test]
fn yanking_an_empty_conversation_is_a_no_op_not_a_panic() {
    let (session, _) = claude::parse("");
    let mut view = ConversationView::new(session);
    view.sync_viewport(80, 10);
    view.begin_visual();
    assert_eq!(view.selected_text(), None); // must not index into empty rows
}

#[test]
fn visual_selection_spans_anchor_to_cursor_and_yanks_those_rows() {
    let mut view = long_conversation(10);
    view.begin_visual();
    view.cursor_down(2);
    assert_eq!(view.visual_range(), Some((0, 2)));
    let text = view.selected_text().unwrap();
    assert_eq!(text.lines().count(), 3); // rows 0, 1, 2
    view.end_visual();
    assert_eq!(view.visual_range(), None);
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
fn list_page_down_advances_a_page_and_selects_the_top() {
    let mut app = app_with_entries(60);
    app.sync_list_viewport(10);

    app.list_page_down();
    assert_eq!(app.list_offset(), 9);
    assert_eq!(app.list_selected(), app.list_offset());

    app.list_page_up();
    assert_eq!(app.list_offset(), 0);
    assert_eq!(app.list_selected(), 0);
}
