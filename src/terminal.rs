//! Terminal control. Restore is centralized in [`Drop`] so the terminal is
//! never left in raw/alternate mode even on panic or early return.

use std::io::{self, Stdout};

use crossterm::event::{DisableBracketedPaste, EnableBracketedPaste};
use crossterm::terminal::{
    DisableLineWrap, EnableLineWrap, EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode,
    enable_raw_mode,
};
use crossterm::{ExecutableCommand, execute};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

pub type Tui = Terminal<CrosstermBackend<Stdout>>;

pub struct TerminalGuard {
    terminal: Tui,
}

impl TerminalGuard {
    pub fn enter() -> io::Result<Self> {
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        // Disable line wrap so a line that overflows the pane is truncated by
        // the terminal instead of wrapping onto the next row (which would land
        // on the other pane and leave stray characters).
        execute!(
            stdout,
            EnterAlternateScreen,
            EnableBracketedPaste,
            DisableLineWrap
        )?;
        let terminal = Terminal::new(CrosstermBackend::new(stdout))?;
        Ok(Self { terminal })
    }

    pub fn terminal(&mut self) -> &mut Tui {
        &mut self.terminal
    }

    fn restore() -> io::Result<()> {
        let mut stdout = io::stdout();
        stdout.execute(EnableLineWrap)?;
        stdout.execute(LeaveAlternateScreen)?;
        stdout.execute(DisableBracketedPaste)?;
        disable_raw_mode()?;
        Ok(())
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = Self::restore();
    }
}
