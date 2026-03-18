use std::io;

use anyhow::Result;
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

pub type OrcasTerminal = Terminal<CrosstermBackend<io::Stdout>>;

pub struct PassThroughTerminalMode {
    active: bool,
}

pub struct SuspendedOrcasTerminal<'terminal> {
    terminal: Option<&'terminal mut OrcasTerminal>,
}

pub fn suspend_terminal(terminal: &mut OrcasTerminal) -> Result<SuspendedOrcasTerminal<'_>> {
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    Ok(SuspendedOrcasTerminal {
        terminal: Some(terminal),
    })
}

pub fn enter_pass_through_mode() -> Result<PassThroughTerminalMode> {
    enable_raw_mode()?;
    Ok(PassThroughTerminalMode { active: true })
}

impl SuspendedOrcasTerminal<'_> {
    pub fn resume(mut self) -> Result<()> {
        if let Some(terminal) = self.terminal.take() {
            restore_terminal(terminal)?;
        }
        Ok(())
    }
}

impl Drop for PassThroughTerminalMode {
    fn drop(&mut self) {
        if self.active {
            let _ = disable_raw_mode();
            self.active = false;
        }
    }
}

impl Drop for SuspendedOrcasTerminal<'_> {
    fn drop(&mut self) {
        if let Some(terminal) = self.terminal.take() {
            let _ = restore_terminal(terminal);
        }
    }
}

fn restore_terminal(terminal: &mut OrcasTerminal) -> Result<()> {
    enable_raw_mode()?;
    execute!(terminal.backend_mut(), EnterAlternateScreen)?;
    terminal.hide_cursor()?;
    terminal.clear()?;
    Ok(())
}
