//! Rendering only. Reads App state and paints it; holds no mutable logic
//! beyond the lazy selection load needed before drawing the conversation.

use std::time::SystemTime;

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::app::{App, Focus, Mode};
use crate::conversation::ConversationView;
use crate::datefmt::{format_timestamp, relative_time};
use crate::session::{Agent, SessionEntry};
use crate::text::{clip_to_width, display_width, fit_to_width, sanitize_display};
use crate::view::Role;

/// Below this width the split is dropped and only the focused pane is shown.
const SPLIT_MIN_WIDTH: u16 = 90;

pub fn draw(frame: &mut Frame, app: &mut App) {
    app.sync_selection();
    let now = SystemTime::now();

    // A line below the status bar is always reserved (tig/vim style): the
    // search/filter state renders there, keeping it out of the status bar and
    // keeping the body height stable whether or not a search is active.
    let outer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(1),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .split(frame.area());
    let body = outer[0];
    let status = outer[1];
    let message = outer[2];

    let single = app.maximized() || app.entries().is_empty() || body.width < SPLIT_MIN_WIDTH;
    if single {
        match app.focus() {
            Focus::List => draw_list(frame, app, body, now),
            Focus::Conversation => draw_conversation(frame, app, body),
        }
    } else {
        // List takes 45% of the width (floor 32, no upper cap): a wider
        // terminal widens the list; the conversation keeps the remaining 55%.
        let left_w = (body.width * 9 / 20).max(32);
        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Length(left_w),
                Constraint::Length(1),
                Constraint::Min(1),
            ])
            .split(body);
        draw_list(frame, app, cols[0], now);
        draw_divider(frame, cols[1]);
        draw_conversation(frame, app, cols[2]);
    }

    draw_status(frame, app, status);
    frame.render_widget(
        Paragraph::new(Line::from(Span::raw(
            status_message(app).unwrap_or_default(),
        ))),
        message,
    );

    // While typing a query, place the (visible) cursor at the end of the prompt
    // on the reserved line, so IME composition appears there, not at the corner.
    if app.mode() == Mode::Search {
        let prompt = 2; // " /"
        let x = message.x + prompt + display_width(app.input_buffer()) as u16;
        let x = x.min(message.x + message.width.saturating_sub(1));
        frame.set_cursor_position((x, message.y));
    }
}

fn draw_divider(frame: &mut Frame, area: Rect) {
    let lines: Vec<Line> = (0..area.height)
        .map(|_| Line::from(Span::styled("│", Style::default().fg(Color::DarkGray))))
        .collect();
    frame.render_widget(Paragraph::new(lines), area);
}

fn draw_list(frame: &mut Frame, app: &mut App, area: Rect, now: SystemTime) {
    app.sync_list_viewport(area.height);
    let focused = app.focus() == Focus::List;
    let offset = app.list_offset();
    let selected = app.list_selected();
    let entries = app.entries();
    let filtered = app.filtered();

    let lines: Vec<Line> = if filtered.is_empty() {
        let msg = if !app.filter_query().is_empty() {
            "no matches"
        } else if app.prose_only() {
            "no sessions with a prose preview"
        } else if app.agent_filter().is_some() {
            "no sessions for this agent"
        } else {
            "no sessions found"
        };
        vec![Line::from(Span::styled(
            msg,
            Style::default().fg(Color::DarkGray),
        ))]
    } else {
        // Tag shown only when sessions span both agents (see App); judged over
        // all entries, so it stays put when filtered to one and never shows for
        // someone who only uses one.
        let show_agent = app.has_multiple_agents();
        filtered
            .iter()
            .enumerate()
            .skip(offset)
            .take(area.height as usize)
            .map(|(pos, &idx)| {
                list_line(
                    &entries[idx],
                    pos == selected,
                    focused,
                    now,
                    area.width,
                    show_agent,
                )
            })
            .collect()
    };

    frame.render_widget(Paragraph::new(lines), area);
}

fn list_line(
    entry: &SessionEntry,
    selected: bool,
    focused: bool,
    now: SystemTime,
    width: u16,
    show_agent: bool,
) -> Line<'static> {
    let when = relative_time(entry.modified, now);
    let project = short_project(&entry.project);
    let summary = if entry.summary.is_empty() {
        "(no preview)"
    } else {
        &entry.summary
    };
    let age = format!("{when:>6}");
    // Split the agent tag out so it can be tinted on its own. Only shown (and
    // only non-empty) when the list spans both agents; padded to the widest
    // label so the project column stays aligned.
    let (tag, body) = if show_agent {
        (
            sanitize_display(&format!("  {:<6}", entry.agent.label())),
            sanitize_display(&format!("  {project}  {summary}")),
        )
    } else {
        (
            String::new(),
            sanitize_display(&format!("  {project}  {summary}")),
        )
    };

    // The selected row is a uniform highlight; column coloring would clash.
    if selected {
        let style = if focused {
            Style::default()
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD)
        };
        let text = fit_to_width(&format!("{age}{tag}{body}"), width as usize);
        return Line::from(Span::styled(text, style));
    }

    // Otherwise dim the age column, tint the agent tag, leave the rest plain.
    let total = width as usize;
    let age = clip_to_width(&age, total);
    let mut used = display_width(&age);
    let mut spans = vec![Span::styled(age, Style::default().fg(Color::Gray))];
    if !tag.is_empty() {
        let tag = clip_to_width(&tag, total.saturating_sub(used));
        used += display_width(&tag);
        spans.push(Span::styled(
            tag,
            Style::default().fg(agent_color(entry.agent)),
        ));
    }
    // Dim rows whose preview is only a slash-command/meta line (or empty): still
    // in date order, just de-emphasized so the eye skips them while scanning.
    let body_fg = if crate::session::discovery::is_command_or_meta(&entry.summary) {
        Color::DarkGray
    } else {
        Color::White
    };
    let body = fit_to_width(&body, total.saturating_sub(used));
    spans.push(Span::styled(body, Style::default().fg(body_fg)));
    Line::from(spans)
}

/// Per-agent tint for the list tag, so Claude and Codex rows read apart at a
/// glance in a mixed list.
fn agent_color(agent: Agent) -> Color {
    match agent {
        Agent::Claude => Color::Yellow,
        Agent::Codex => Color::Green,
        Agent::Cursor => Color::Blue,
    }
}

fn short_project(path: &str) -> String {
    path.rsplit('/').next().unwrap_or(path).to_string()
}

fn draw_conversation(frame: &mut Frame, app: &mut App, area: Rect) {
    let focused = app.focus() == Focus::Conversation;
    let Some(view) = app.conversation_mut() else {
        frame.render_widget(
            Paragraph::new(Span::styled(
                "no session selected",
                Style::default().fg(Color::DarkGray),
            )),
            area,
        );
        return;
    };
    // One-column left gutter so text isn't flush against the divider.
    let inner = Rect {
        x: area.x + 1,
        y: area.y,
        width: area.width.saturating_sub(1),
        height: area.height,
    };
    view.sync_viewport(inner.width, inner.height);

    let offset = view.offset();
    let cursor = view.cursor();
    let selection = view.visual_range();
    let row_width = inner.width as usize;
    let lines: Vec<Line> = view
        .visible_rows()
        .iter()
        .enumerate()
        .map(|(i, row)| {
            // rows are already wrapped to width, so scroll maps 1:1 to lines.
            let abs = offset + i;
            let on_cursor = focused && abs == cursor;
            let on_selected = selection.is_some_and(|(lo, hi)| abs >= lo && abs <= hi);
            let on_match = view.is_match(abs);
            // Cursor reverses; the visual selection and a search match each get a
            // background, cursor taking priority, then selection, then match.
            let mut style = style_for(row.role);
            if on_cursor {
                style = style.add_modifier(Modifier::REVERSED);
            } else if on_selected {
                style = style.bg(Color::Indexed(24));
            } else if on_match {
                style = style.bg(Color::Indexed(237));
            }
            // Pad only highlighted rows so the bar spans the pane; plain rows and
            // the area below are cleared by ratatui's per-frame buffer reset.
            let text = if on_cursor || on_selected || on_match {
                fit_to_width(&row.text, row_width)
            } else {
                row.text.clone()
            };
            Line::from(Span::styled(text, style))
        })
        .collect();

    frame.render_widget(Paragraph::new(lines), inner);
}

fn draw_status(frame: &mut Frame, app: &App, area: Rect) {
    let (left, right) = status_text(app);
    let pad = (area.width as usize).saturating_sub(display_width(&left) + display_width(right));
    let text = format!("{left}{}{right}", " ".repeat(pad));
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            text,
            Style::default().fg(Color::Black).bg(Color::Gray),
        ))),
        area,
    );
}

fn status_text(app: &App) -> (String, &'static str) {
    match app.focus() {
        Focus::List => {
            let count = app.filtered().len();
            let shown = count.min(app.list_selected() + 1);
            let agent = app.agent_filter().map(|a| a.name()).unwrap_or("All");
            let prose = if app.prose_only() { "  prose-only" } else { "" };
            let left = format!(" agent:{agent}{prose}  {shown}/{count}");
            let hint = if app.mode() == Mode::Search {
                "↑↓ select  Enter open  Esc cancel "
            } else {
                "j/k move  a agent  p prose  / filter  q quit "
            };
            (left, hint)
        }
        Focus::Conversation => {
            let left = app
                .conversation()
                .map(conversation_status_left)
                .unwrap_or_default();
            let hint = if app.mode() == Mode::Visual {
                "j/k extend  y yank  Esc cancel "
            } else if app.mode() == Mode::Search {
                "↑↓ next  Enter accept  Esc cancel "
            } else if app.conversation().is_some_and(|v| v.match_info().is_some()) {
                "n/N match  o fold  v select  q back "
            } else {
                "j/k move  o fold  v select  / search  q back "
            };
            (left, hint)
        }
    }
}

fn conversation_status_left(view: &ConversationView) -> String {
    let total = view.total_rows();
    let pos = (view.cursor() + 1)
        .checked_mul(100)
        .and_then(|n| n.checked_div(total.max(1)))
        .unwrap_or(0);
    let when = view.started_at().map(format_timestamp).unwrap_or_default();
    let sid = view.session_id().unwrap_or("(unknown)");
    // `when` and `sid` come straight from the session file (raw timestamp / id),
    // so sanitize before they reach the terminal — same as body and list rows.
    sanitize_display(&format!(" {when}  {sid}  {pos}%"))
}

/// The reserved line below the status bar: a transient notice, the visual-mode
/// hint, or otherwise the search/filter state.
fn status_message(app: &App) -> Option<String> {
    if let Some(note) = app.notice() {
        return Some(format!(" {note}"));
    }
    if app.mode() == Mode::Visual {
        let n = app
            .conversation()
            .and_then(|v| v.visual_range())
            .map_or(0, |(lo, hi)| hi - lo + 1);
        return Some(format!(" -- VISUAL --  {n} line(s)   y yank   Esc cancel"));
    }
    search_message(app)
}

/// Text for the reserved line below the status bar: the live prompt while
/// typing, else the committed search/filter result, or `None` when idle.
fn search_message(app: &App) -> Option<String> {
    if app.mode() == Mode::Search {
        return Some(format!(" /{}", app.input_buffer()));
    }
    match app.focus() {
        Focus::List => {
            let q = app.filter_query();
            (!q.is_empty()).then(|| format!(" Filter: '{q}'"))
        }
        Focus::Conversation => {
            let view = app.conversation()?;
            match view.match_info()? {
                (_, 0) => Some(format!(" No matches for '{}'", view.search_query())),
                (cur, tot) => Some(format!(
                    " Match {cur} of {tot} for '{}'",
                    view.search_query()
                )),
            }
        }
    }
}

/// `2026-10-13T12:48:20.086Z` -> `2026-10-13 12:48` (as recorded, UTC).
fn style_for(role: Role) -> Style {
    match role {
        Role::UserHeader => Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
        Role::UserText => Style::default().fg(Color::White),
        Role::AssistantText => Style::default().fg(Color::Reset),
        Role::Thinking => Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::ITALIC),
        Role::ToolHeader => Style::default().fg(Color::Yellow),
        Role::ToolBody => Style::default().fg(Color::Gray),
        Role::System => Style::default().fg(Color::Magenta),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn entry(summary: &str) -> SessionEntry {
        SessionEntry {
            agent: Agent::Claude,
            path: PathBuf::from("/x"),
            session_id: "id".to_string(),
            project: "/home/me/widget".to_string(),
            modified: Some(SystemTime::UNIX_EPOCH),
            summary: summary.to_string(),
        }
    }

    fn body_fg(summary: &str) -> Option<Color> {
        let line = list_line(
            &entry(summary),
            false,
            true,
            SystemTime::UNIX_EPOCH,
            80,
            false,
        );
        line.spans.last().and_then(|s| s.style.fg)
    }

    #[test]
    fn a_command_only_or_empty_preview_row_is_dimmed() {
        assert_eq!(body_fg("real prompt text"), Some(Color::White));
        assert_eq!(
            body_fg("<command-name>/clear</command-name>"),
            Some(Color::DarkGray)
        );
        assert_eq!(body_fg(""), Some(Color::DarkGray)); // "(no preview)"
    }
}
