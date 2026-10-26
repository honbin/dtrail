//! Render smoke tests: draw each screen with a TestBackend and assert it does
//! not panic and produces non-empty output. Uses synthetic data only.

use dtrail::app::App;
use dtrail::session::{Agent, SessionEntry, claude};
use dtrail::ui;
use ratatui::Terminal;
use ratatui::backend::TestBackend;

use std::path::PathBuf;
use std::time::SystemTime;

fn render(app: &mut App) -> String {
    render_at(app, 80)
}

fn render_at(app: &mut App, width: u16) -> String {
    let backend = TestBackend::new(width, 20);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| ui::draw(f, app)).unwrap();
    let buffer = terminal.backend().buffer();
    let mut out = String::new();
    for y in 0..buffer.area.height {
        for x in 0..buffer.area.width {
            out.push_str(buffer[(x, y)].symbol());
        }
    }
    out
}

fn fixture_session() -> App {
    let jsonl = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/minimal-session.jsonl"
    ))
    .unwrap();
    App::with_session(claude::parse(&jsonl))
}

#[test]
fn conversation_renders_without_panicking() {
    let mut app = fixture_session();
    let out = render(&mut app);
    assert!(out.contains("user"));
    // The fixture's broken last line is reported, not silently dropped.
    assert!(out.contains("(1 unparsed line hidden)"), "got: {out}");

    // Exercise the fold and search draw paths too.
    if let Some(view) = app.conversation_mut() {
        view.cursor_bottom();
        view.toggle_fold_under_cursor();
        view.set_search("done");
    }
    let out = render(&mut app);
    assert!(!out.trim().is_empty());
}

#[test]
fn an_active_search_renders_on_its_own_line() {
    let mut app = fixture_session();
    render(&mut app); // sync the viewport to a real width before searching

    // A committed search with a hit: the message line reports which match.
    app.conversation_mut().unwrap().set_search("done");
    let out = render(&mut app);
    assert!(out.contains("Match 1 of 1 for 'done'"), "got: {out}");

    // A query with no hit says so rather than showing a count.
    app.conversation_mut().unwrap().set_search("zzz-absent");
    let out = render(&mut app);
    assert!(out.contains("No matches for 'zzz-absent'"), "got: {out}");
}

#[test]
fn an_active_list_filter_renders_on_its_own_line() {
    let entry = SessionEntry {
        agent: Agent::Claude,
        path: PathBuf::from("/x"),
        session_id: "id".to_string(),
        project: "/home/me/widget".to_string(),
        last_active: Some(SystemTime::UNIX_EPOCH),
        summary: "add preview".to_string(),
        is_prompt: true,
    };
    let mut app = App::with_entries(vec![entry]);
    render(&mut app); // sync the viewport

    // While typing, the prompt is on its own line, not in the status bar.
    app.start_search();
    for c in "widget".chars() {
        app.search_push(c);
    }
    let out = render(&mut app);
    assert!(out.contains("/widget"), "typing prompt: {out}");

    // Committed, the line names the active filter.
    app.search_confirm();
    let out = render(&mut app);
    assert!(out.contains("Filter: 'widget'"), "committed: {out}");
}

#[test]
fn list_renders_with_entries_and_filter() {
    let entry = SessionEntry {
        agent: Agent::Claude,
        path: PathBuf::from("/x"),
        session_id: "id".to_string(),
        project: "/home/me/widget".to_string(),
        last_active: Some(SystemTime::UNIX_EPOCH),
        summary: "add preview".to_string(),
        is_prompt: true,
    };
    let mut app = App::with_entries(vec![entry]);
    let out = render(&mut app);
    assert!(out.contains("widget"));

    app.start_search();
    app.search_push('z'); // matches nothing
    let out = render(&mut app);
    assert!(out.contains("no matches"));
}

#[test]
fn a_mixed_agent_list_tags_each_row() {
    let row = |agent, project: &str, summary: &str| SessionEntry {
        agent,
        path: PathBuf::from("/x"),
        session_id: "id".to_string(),
        project: project.to_string(),
        last_active: Some(SystemTime::UNIX_EPOCH),
        summary: summary.to_string(),
        is_prompt: true,
    };
    // Two agents present -> each row carries a (tinted) agent tag.
    let mut app = App::with_entries(vec![
        row(Agent::Claude, "/home/me/a", "alpha"),
        row(Agent::Codex, "/home/me/b", "beta"),
    ]);
    let out = render(&mut app);
    assert!(out.contains("Claude"));
    assert!(out.contains("Codex"));
    assert!(out.contains("alpha") && out.contains("beta"));
}

#[test]
fn a_wide_terminal_shows_the_list_beside_the_selected_conversation() {
    let entry = SessionEntry {
        agent: Agent::Claude,
        path: PathBuf::from(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/minimal-session.jsonl"
        )),
        session_id: "id".to_string(),
        project: "/home/me/widget".to_string(),
        last_active: Some(SystemTime::UNIX_EPOCH),
        summary: "add preview".to_string(),
        is_prompt: true,
    };
    let mut app = App::with_entries(vec![entry]);
    app.sync_selection();
    let out = render_at(&mut app, 120);
    assert!(out.contains("add preview"), "list: {out}");
    assert!(out.contains("hello"), "conversation: {out}");

    // Below the split width only the focused pane (the list) is drawn.
    let out = render_at(&mut app, 80);
    assert!(
        out.contains("add preview") && !out.contains("hello"),
        "got: {out}"
    );
}

#[test]
fn empty_list_renders_without_panicking() {
    // e.g. the session directory does not exist, so discovery found nothing.
    let mut app = App::with_entries(vec![]);
    let out = render(&mut app);
    assert!(out.contains("no sessions found"));
}

#[test]
fn the_status_line_sanitizes_a_malicious_session_id() {
    // A session id carrying an ANSI escape (from the file) must not reach the
    // terminal raw via the status line.
    let jsonl =
        r#"{"type":"user","sessionId":"\u001b[31mBAD","message":{"role":"user","content":"hi"}}"#;
    let mut app = App::with_session(claude::parse(jsonl));
    let out = render(&mut app);
    assert!(
        !out.contains('\u{1b}'),
        "escape leaked into the status line"
    );
    assert!(
        out.contains("BAD"),
        "the sanitized id text should still show"
    );
}
