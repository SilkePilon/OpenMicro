mod client;
mod ui;

use std::io::BufRead;
use std::sync::{Arc, Mutex};

use crossterm::event::{self, Event, KeyCode};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen};
use crossterm::execute;
use ratatui::prelude::*;

use client::SnapshotDto;

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

    enable_raw_mode()?;
    let mut stdout = std::io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(stdout))?;

    // Run the loop and capture the result rather than propagating it directly
    // with `?`, so the terminal is always restored (raw mode disabled, leave
    // alternate screen) whether the loop exits normally or via an I/O error.
    let result = run(&mut terminal, &snap);

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;

    result
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
