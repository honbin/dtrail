//! Search tests: conversation in-session search and list filtering.

use std::path::PathBuf;
use std::time::SystemTime;

use dtrail::app::{App, Mode};
use dtrail::conversation::ConversationView;
use dtrail::session::claude;
use dtrail::session::{Agent, SessionEntry};

fn conversation() -> ConversationView {
    let jsonl = concat!(
        r#"{"type":"user","sessionId":"s","message":{"role":"user","content":"find the needle here"}}"#,
        "\n",
        r#"{"type":"assistant","sessionId":"s","message":{"content":[{"type":"text","text":"no match on this line"}]}}"#,
        "\n",
        r#"{"type":"assistant","sessionId":"s","message":{"content":[{"type":"text","text":"another needle appears"}]}}"#,
        "\n",
    );
    let (session, _) = claude::parse(jsonl);
    let mut view = ConversationView::new(session);
    view.sync_viewport(80, 10);
    view
}

#[test]
fn search_finds_all_matching_rows_case_insensitive() {
    let mut view = conversation();
    view.set_search("NEEDLE");
    assert_eq!(view.match_info(), Some((1, 2)));
}

#[test]
fn next_and_prev_wrap_around_matches() {
    let mut view = conversation();
    // Search moves the line cursor onto the matching row.
    view.set_search("needle");
    let first = view.cursor();
    view.next_match();
    let second = view.cursor();
    assert!(second > first);
    view.next_match(); // wraps back to the first
    assert_eq!(view.cursor(), first);
    view.prev_match(); // wraps to the last
    assert_eq!(view.cursor(), second);
}

#[test]
fn empty_query_reports_no_match_info() {
    let mut view = conversation();
    view.set_search("");
    assert_eq!(view.match_info(), None);
}

#[test]
fn jumping_to_a_match_keeps_context_around_it() {
    // A long message with the needle in the middle; a small viewport.
    let lines: Vec<String> = (0..40)
        .map(|i| {
            if i == 20 {
                "needle".into()
            } else {
                format!("line{i}")
            }
        })
        .collect();
    let content = lines.join("\\n"); // JSON-escaped newlines
    let jsonl = format!(
        r#"{{"type":"user","sessionId":"s","message":{{"role":"user","content":"{content}"}}}}"#
    );
    let (session, _) = claude::parse(&jsonl);
    let mut view = ConversationView::new(session);
    let height = 10u16;
    view.sync_viewport(80, height);

    view.set_search("needle");
    let cursor = view.cursor();
    let offset = view.offset();
    let bottom = offset + height as usize - 1;
    // The match is not glued to the top edge or the bottom (status bar) edge.
    assert!(cursor >= offset + 2, "need context above the match");
    assert!(cursor + 2 <= bottom, "need context below the match");
}

#[test]
fn carried_query_jumps_to_the_first_body_match() {
    let jsonl = concat!(
        r#"{"type":"user","sessionId":"s","message":{"role":"user","content":"debug the timeout please"}}"#,
        "\n",
        r#"{"type":"assistant","sessionId":"s","message":{"content":[{"type":"text","text":"the timeout happens in retry"}]}}"#,
        "\n",
    );
    let (session, _) = claude::parse(jsonl);
    let mut view = ConversationView::new(session);
    // A query carried in from the list filter is applied on the next layout.
    view.carry_search("timeout");
    view.sync_viewport(80, 20);

    assert_eq!(view.match_info(), Some((1, 2)));
    let row = &view.visible_rows()[view.cursor() - view.offset()];
    assert!(row.text.to_lowercase().contains("timeout"));
}

#[test]
fn canceling_a_search_returns_to_the_origin() {
    let mut view = conversation();
    let origin = view.cursor();

    view.begin_search();
    view.set_search("needle"); // jumps the cursor onto a match
    assert_ne!(view.cursor(), origin);

    view.cancel_search(); // Esc: back to where the search started
    assert_eq!(view.cursor(), origin);
    assert_eq!(view.match_info(), None);
}

#[test]
fn missing_query_reports_zero_of_zero() {
    let mut view = conversation();
    view.set_search("zzz-not-present");
    assert_eq!(view.match_info(), Some((0, 0)));
}

#[test]
fn search_reveals_a_match_inside_a_folded_tool_result() {
    let jsonl = concat!(
        r#"{"type":"assistant","sessionId":"s","message":{"content":[{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"run"}}]}}"#,
        "\n",
        r#"{"type":"user","sessionId":"s","message":{"content":[{"type":"tool_result","tool_use_id":"t1","content":"line one\nNullPointerException here\nline three"}]}}"#,
        "\n",
    );
    let (session, _) = claude::parse(jsonl);
    let mut view = ConversationView::new(session);
    view.sync_viewport(80, 20);

    // Folded: just the two headers; the error text is not yet a row.
    assert_eq!(view.total_rows(), 2);

    // Searching the folded output finds it, reveals the item, and lands the
    // cursor on the matching line.
    view.set_search("nullpointer");
    assert_eq!(view.match_info(), Some((1, 1)));
    assert!(view.total_rows() > 2);
    let row = &view.visible_rows()[view.cursor() - view.offset()];
    assert!(row.text.to_lowercase().contains("nullpointer"));
}

#[test]
fn folding_one_tool_does_not_expand_the_other() {
    let jsonl = concat!(
        r#"{"type":"assistant","sessionId":"s","message":{"content":[{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"ls"}}]}}"#,
        "\n",
        r#"{"type":"user","sessionId":"s","message":{"content":[{"type":"tool_result","tool_use_id":"t1","content":"alpha-line-1\nalpha-line-2"}]}}"#,
        "\n",
        r#"{"type":"assistant","sessionId":"s","message":{"content":[{"type":"tool_use","id":"t2","name":"Bash","input":{"command":"pwd"}}]}}"#,
        "\n",
    );
    let (session, _) = claude::parse(jsonl);
    let mut view = ConversationView::new(session);
    view.sync_viewport(80, 20);

    // All folded initially: 3 header rows, no bodies.
    assert_eq!(view.total_rows(), 3);

    // Expand only the tool_result (second row) and confirm its body appears
    // while the other items stay folded.
    view.cursor_down(1);
    view.toggle_fold_under_cursor();
    assert_eq!(view.total_rows(), 5); // 3 headers + 2 body lines

    // Folding it again returns to all-folded.
    view.toggle_fold_under_cursor();
    assert_eq!(view.total_rows(), 3);
}

fn entry(project: &str, summary: &str) -> SessionEntry {
    SessionEntry {
        agent: Agent::Claude,
        path: PathBuf::from("/x"),
        session_id: "id".to_string(),
        project: project.to_string(),
        modified: Some(SystemTime::UNIX_EPOCH),
        summary: summary.to_string(),
    }
}

#[test]
fn conversation_search_is_retained_after_confirm_and_prefilled_next_time() {
    let jsonl = concat!(
        r#"{"type":"user","sessionId":"s","message":{"role":"user","content":"a needle and another needle"}}"#,
        "\n",
        r#"{"type":"assistant","sessionId":"s","message":{"content":[{"type":"text","text":"needle again"}]}}"#,
        "\n",
    );
    let (session, _) = claude::parse(jsonl);
    let mut app = App::with_session(session);
    app.conversation_mut().unwrap().sync_viewport(80, 20);

    app.start_search();
    for c in "needle".chars() {
        app.search_push(c);
    }
    app.search_confirm();

    // Confirmed: query and matches are retained, and n/N still move.
    assert_eq!(app.mode(), Mode::Normal);
    let before = app.conversation().unwrap().cursor();
    assert!(app.conversation().unwrap().match_info().is_some());
    app.search_next();
    assert_ne!(app.conversation().unwrap().cursor(), before);

    // Starting a new search pre-fills the previous query for editing.
    app.start_search();
    assert_eq!(app.input_buffer(), "needle");
}

#[test]
fn conversation_search_does_not_move_until_confirmed() {
    // The only match sits well below the first screen.
    let lines: Vec<String> = (0..40)
        .map(|i| {
            if i == 30 {
                "needle".into()
            } else {
                format!("line{i}")
            }
        })
        .collect();
    let content = lines.join("\\n");
    let jsonl = format!(
        r#"{{"type":"user","sessionId":"s","message":{{"role":"user","content":"{content}"}}}}"#
    );
    let mut app = App::with_session(claude::parse(&jsonl).0);
    app.conversation_mut().unwrap().sync_viewport(80, 10);
    let start = app.conversation().unwrap().cursor();
    let rows = app.conversation().unwrap().total_rows();

    // Typing the query must not scroll the body, jump the cursor, or reveal folds.
    app.start_search();
    for c in "needle".chars() {
        app.search_push(c);
    }
    assert_eq!(app.conversation().unwrap().cursor(), start);
    assert_eq!(app.conversation().unwrap().offset(), 0);
    assert_eq!(app.conversation().unwrap().total_rows(), rows);
    assert_eq!(app.conversation().unwrap().match_info(), None);

    // Enter applies the search: jump to the match and report it.
    app.search_confirm();
    let view = app.conversation().unwrap();
    assert!(view.cursor() > start);
    assert_eq!(view.match_info(), Some((1, 1)));
}

#[test]
fn list_filter_narrows_by_project_or_summary() {
    let mut app = App::with_entries(vec![
        entry("/home/me/widget", "add preview"),
        entry("/home/me/blog", "rss feed"),
        entry("/home/me/tools", "widget mention in summary"),
    ]);
    assert_eq!(app.filtered().len(), 3);

    app.start_search();
    assert_eq!(app.mode(), Mode::Search);
    for c in "widget".chars() {
        app.search_push(c);
    }
    // Matches the widget project and the summary that mentions widget.
    assert_eq!(app.filtered().len(), 2);
    // Narrowing lands the selection on the first match.
    assert_eq!(app.list_selected(), 0);

    app.search_cancel();
    assert_eq!(app.mode(), Mode::Normal);
    assert_eq!(app.filtered().len(), 3);
}

#[test]
fn agent_filter_cycles_and_composes_with_text_filter() {
    let agent_entry = |agent, project: &str, summary: &str| SessionEntry {
        agent,
        path: PathBuf::from("/x"),
        session_id: "id".to_string(),
        project: project.to_string(),
        modified: Some(SystemTime::UNIX_EPOCH),
        summary: summary.to_string(),
    };
    let mut app = App::with_entries(vec![
        agent_entry(Agent::Claude, "/p/a", "api work"),
        agent_entry(Agent::Codex, "/p/b", "api design"),
        agent_entry(Agent::Codex, "/p/c", "docs"),
    ]);
    assert_eq!(app.filtered().len(), 3); // all

    app.cycle_agent_filter(); // -> Claude
    assert_eq!(app.agent_filter(), Some(Agent::Claude));
    assert_eq!(app.filtered().len(), 1);

    app.cycle_agent_filter(); // -> Codex
    assert_eq!(app.agent_filter(), Some(Agent::Codex));
    assert_eq!(app.filtered().len(), 2);

    // Compose with the text filter: Codex AND "api".
    app.start_search();
    for c in "api".chars() {
        app.search_push(c);
    }
    assert_eq!(app.filtered().len(), 1);
    app.search_cancel();

    app.cycle_agent_filter(); // -> all
    assert_eq!(app.agent_filter(), None);
    assert_eq!(app.filtered().len(), 3);
}
