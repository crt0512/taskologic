//! Terminal setup and teardown. Also the panic hook, which matters more here
//! than in most programs: this binary is a login shell, so a panic must end
//! the process cleanly rather than unwind into anything interactive.

use std::io::{self, Stdout, Write};

use crossterm::cursor::Show;
use crossterm::event::{DisableMouseCapture, EnableMouseCapture};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

pub type Term = Terminal<CrosstermBackend<Stdout>>;

pub fn setup() -> anyhow::Result<Term> {
    enable_raw_mode()?;
    let mut out = io::stdout();
    execute!(out, EnterAlternateScreen, EnableMouseCapture)?;
    Ok(Terminal::new(CrosstermBackend::new(out))?)
}

pub fn restore() {
    let _ = disable_raw_mode();
    // Show explicitly: ratatui hides the cursor while drawing and leaving
    // the alternate screen does not bring it back. Without it the shell
    // prompt afterwards has no cursor, which reads as a frozen terminal.
    let _ = execute!(
        io::stdout(),
        DisableMouseCapture,
        LeaveAlternateScreen,
        Show
    );
}

pub fn install_panic_hook() {
    std::panic::set_hook(Box::new(|info| {
        restore();
        eprintln!("Taskologic crashed: {info}");
        eprintln!("The session will now close.");
        let _ = io::stderr().flush();
        std::process::exit(1);
    }));
}

/// Terminal bell, used for scan feedback.
pub fn bell() {
    let mut out = io::stdout();
    let _ = out.write_all(b"\x07");
    let _ = out.flush();
}
