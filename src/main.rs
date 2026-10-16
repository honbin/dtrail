//! Startup and CLI argument handling. No parsing or rendering logic lives here.
//!
//! With no argument, opens the session list. With a path, opens that session
//! directly.

use std::process::ExitCode;
use std::time::Duration;

use crossterm::event::{self, Event};

use ccscope::app::App;
use ccscope::input::InputState;
use ccscope::session::{discover, discovery};
use ccscope::terminal::TerminalGuard;
use ccscope::ui;

fn main() -> ExitCode {
    let app = match std::env::args().nth(1) {
        // A directory: browse every session under it (agent detected per file).
        Some(path) if std::path::Path::new(&path).is_dir() => {
            App::with_entries(discovery::discover_dir(std::path::Path::new(&path)))
        }
        // A file: open it directly, agent detected from its content.
        Some(path) => match std::fs::read_to_string(&path) {
            Ok(input) => {
                let agent = discovery::detect(&input);
                App::with_session(discovery::parse(agent, &input).0)
            }
            Err(e) => {
                eprintln!("cannot read {path}: {e}");
                return ExitCode::FAILURE;
            }
        },
        None => App::with_entries(discover()),
    };

    match run(app) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run(mut app: App) -> std::io::Result<()> {
    let mut guard = TerminalGuard::enter()?;
    let mut keys = InputState::default();

    while !app.should_quit {
        guard.terminal().draw(|frame| ui::draw(frame, &mut app))?;

        // Poll so a pending quit can be honored without blocking on input.
        if event::poll(Duration::from_millis(250))?
            && let Event::Key(key) = event::read()?
            && key.kind == event::KeyEventKind::Press
        {
            // Ctrl-l forces a full repaint, the usual escape hatch if the
            // terminal and our model ever diverge and leave stale cells.
            if key.code == event::KeyCode::Char('l')
                && key.modifiers.contains(event::KeyModifiers::CONTROL)
            {
                guard.terminal().clear()?;
            } else {
                keys.handle(&mut app, key);
            }
        }
    }

    Ok(())
}
