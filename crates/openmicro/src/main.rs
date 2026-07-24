mod client;
mod ui;

use std::io::{self, BufRead};
use std::sync::{Arc, Mutex};

use crossterm::event::{self, Event, KeyCode};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen};
use crossterm::execute;
use ratatui::prelude::*;

use client::SnapshotDto;

/// RAII guard that restores the terminal on every exit path.
///
/// Constructed immediately after raw mode is enabled and the alternate screen
/// is entered. Its `Drop` impl runs on normal return, on any early
/// `?`-propagated error, and during panic unwinding, so the user's shell is
/// never left wedged in raw mode / the alternate screen. Errors from the
/// restore calls are intentionally ignored (`let _ = ...`) so cleanup never
/// itself panics or short-circuits the other restore step.
struct TerminalGuard;

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen);
    }
}

fn main() -> anyhow::Result<()> {
    let rt = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".into());
    let path = format!("{rt}/openmicro-ctl.sock");

    let snap = Arc::new(Mutex::new(SnapshotDto::default()));
    {
        let snap = snap.clone();
        std::thread::spawn(move || {
            if let Ok(stream) = std::os::unix::net::UnixStream::connect(&path) {
                let reader = std::io::BufReader::new(stream);
                for line in reader.lines().map_while(Result::ok) {
                    if let Some(parsed) = client::parse_snapshot(&line) {
                        *snap.lock().unwrap() = parsed;
                    }
                }
            }
        });
    }

    // Enable raw mode and enter the alternate screen, then IMMEDIATELY install
    // the RAII guard so every subsequent fallible step (Terminal::new, the run
    // loop) is covered: any early return via `?` or panic unwinds through
    // `TerminalGuard::drop`, restoring the terminal. There is no explicit
    // restore afterwards — the guard owns cleanup, so it cannot be skipped or
    // double-run in a way that errors.
    enable_raw_mode()?;
    let _guard = TerminalGuard;
    let mut stdout = std::io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(stdout))?;

    run(&mut terminal, &snap)
}

fn run(
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    snap: &Arc<Mutex<SnapshotDto>>,
) -> anyhow::Result<()> {
    loop {
        {
            let snap = snap.lock().unwrap();
            terminal.draw(|f| ui::render(f, &snap))?;
        }
        if event::poll(std::time::Duration::from_millis(200))? {
            if let Event::Key(k) = event::read()? {
                if matches!(k.code, KeyCode::Char('q') | KeyCode::Esc) {
                    break;
                }
            }
        }
    }

    Ok(())
}
