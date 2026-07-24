mod cli;
mod client;
mod flash;
mod ui;

use std::io::{self, BufRead, Write};
use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use clap::Parser;
use crossterm::event::{self, Event, KeyCode};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen};
use crossterm::execute;
use openmicro_proto::Command;
use ratatui::prelude::*;

use client::{adjust_brightness, step_preset, SnapshotDto, PALETTE};
use ui::{ConfigUiState, InstallerUiState, MAX_SLEEP_MINUTES, PANEL_STATES, SLEEP_ROW};

/// Shared write handle for sending `Command`s to the daemon. `None` when
/// disconnected. Populated by the reader thread on each successful connect.
type Writer = Arc<Mutex<Option<UnixStream>>>;

fn send_command(writer: &Writer, cmd: &Command) {
    if let Some(stream) = writer.lock().unwrap().as_mut() {
        if let Ok(mut line) = serde_json::to_string(cmd) {
            line.push('\n');
            let _ = stream.write_all(line.as_bytes());
        }
    }
}

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
    let cli = cli::Cli::parse();
    match cli.command {
        // No subcommand: launch the interactive TUI exactly as before.
        None => run_tui(),
        // A subcommand: run it and exit (some arms exit the process directly).
        Some(command) => cli::run(command),
    }
}

/// Launch the interactive TUI: spawn the control-socket reader thread, install
/// the terminal guard, and run the render/input loop. This is the historical
/// `main` body, unchanged in behavior, reached only when no subcommand is given.
fn run_tui() -> anyhow::Result<()> {
    let rt = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".into());
    let path = format!("{rt}/openmicro-ctl.sock");

    let snap = Arc::new(Mutex::new(SnapshotDto::default()));
    let connected = Arc::new(AtomicBool::new(false));
    let writer: Writer = Arc::new(Mutex::new(None));
    {
        let snap = snap.clone();
        let connected = connected.clone();
        let writer = writer.clone();
        std::thread::spawn(move || loop {
            match UnixStream::connect(&path) {
                Ok(stream) => {
                    // Clone a write handle for the main thread to send Commands;
                    // this handle reads snapshots.
                    if let Ok(wclone) = stream.try_clone() {
                        *writer.lock().unwrap() = Some(wclone);
                    }
                    connected.store(true, Ordering::Relaxed);
                    let reader = std::io::BufReader::new(stream);
                    for line in reader.lines().map_while(Result::ok) {
                        if let Some(parsed) = client::parse_snapshot(&line) {
                            *snap.lock().unwrap() = parsed;
                        }
                    }
                    // stream ended => daemon closed/restarted
                    connected.store(false, Ordering::Relaxed);
                    *writer.lock().unwrap() = None;
                }
                Err(_) => {
                    connected.store(false, Ordering::Relaxed);
                    *writer.lock().unwrap() = None;
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(1000));
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

    run(&mut terminal, &snap, &connected, &writer)
}

fn run(
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    snap: &Arc<Mutex<SnapshotDto>>,
    connected: &Arc<AtomicBool>,
    writer: &Writer,
) -> anyhow::Result<()> {
    let mut cfg = ConfigUiState::new(200);
    let mut installer = InstallerUiState::new();
    loop {
        {
            let snap = snap.lock().unwrap();
            let is_connected = connected.load(Ordering::Relaxed);
            terminal.draw(|f| ui::render(f, &snap, is_connected, &cfg, &installer))?;
        }
        if event::poll(std::time::Duration::from_millis(200))? {
            if let Event::Key(k) = event::read()? {
                if installer.open {
                    match k.code {
                        KeyCode::Char('q') => break,
                        KeyCode::Char('f') | KeyCode::Esc => installer.open = false,
                        KeyCode::Char('r') => installer.refresh(),
                        KeyCode::Enter => run_installer_flash(&mut installer),
                        _ => {}
                    }
                } else if cfg.open {
                    match k.code {
                        KeyCode::Char('q') => break,
                        KeyCode::Char('c') | KeyCode::Esc => cfg.open = false,
                        KeyCode::Up => cfg.select_prev(),
                        KeyCode::Down => cfg.select_next(),
                        KeyCode::Left => adjust(&mut cfg, -1, writer),
                        KeyCode::Right => adjust(&mut cfg, 1, writer),
                        _ => {}
                    }
                } else {
                    match k.code {
                        KeyCode::Char('q') | KeyCode::Esc => break,
                        KeyCode::Char('c') => {
                            // Seed from the daemon's latest snapshot on open, so
                            // the panel starts from the real live config instead
                            // of a stale/hardcoded UI default (Fix 1).
                            cfg.seed_from_snapshot(&snap.lock().unwrap());
                            cfg.open = true;
                        }
                        KeyCode::Char('f') => {
                            installer.open = true;
                            installer.refresh();
                        }
                        _ => {}
                    }
                }
            }
        }
    }

    Ok(())
}

/// Attempt a real flash from the installer screen, capturing esptool's output
/// into the scrollable area. Only proceeds when every prerequisite passes; it
/// never fabricates success — a failure surfaces esptool's actual error.
fn run_installer_flash(installer: &mut InstallerUiState) {
    if !installer.ready {
        installer.output = vec![
            "not ready: resolve the ✗ items above (refresh with `r`).".to_string(),
        ];
        return;
    }
    installer.output = vec!["flashing…".to_string()];
    match flash::flash_capture(None, None) {
        Ok(out) => {
            installer.output = out;
            installer.output.push("flash complete.".to_string());
        }
        Err(msg) => {
            installer.output = msg.lines().map(|l| l.to_string()).collect();
        }
    }
    // Re-derive the checklist after a flash attempt (device may re-enumerate).
    installer.refresh();
}

/// Apply a left/right adjustment on the selected config row and send the
/// resulting `Command` to the daemon, keeping a local echo in `cfg`.
fn adjust(cfg: &mut ConfigUiState, dir: i32, writer: &Writer) {
    if cfg.selected == 0 {
        cfg.brightness = adjust_brightness(cfg.brightness, dir * 8);
        send_command(writer, &Command::SetBrightness(cfg.brightness));
    } else if cfg.selected == SLEEP_ROW {
        // Clamp mirrors the daemon's own clamp on `SetSleepMinutes` (Fix 5): a
        // stuck key can't drive this arbitrarily high even before the command
        // round-trips.
        cfg.sleep_minutes =
            ((cfg.sleep_minutes as i32 + dir).max(0) as u32).min(MAX_SLEEP_MINUTES);
        send_command(writer, &Command::SetSleepMinutes(cfg.sleep_minutes));
    } else {
        let i = cfg.selected - 1;
        cfg.color_idx[i] = step_preset(cfg.color_idx[i], dir);
        let rgb = PALETTE[cfg.color_idx[i]];
        cfg.colors[i] = rgb;
        send_command(writer, &Command::SetStateColor { state: PANEL_STATES[i], rgb });
    }
}
