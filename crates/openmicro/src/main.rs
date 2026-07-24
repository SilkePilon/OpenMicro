mod agents;
mod cli;
mod client;
mod firmware;
mod flash;
mod onboarding;
mod probe;
mod ui;

use std::io::{self, BufRead, Write};
use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use clap::Parser;
use crossterm::event::{self, Event, KeyCode};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen};
use crossterm::execute;
use openmicro_proto::Command;
use ratatui::prelude::*;

use agents::AgentKind;
use client::{adjust_brightness, step_preset, SnapshotDto, PALETTE};
use onboarding::{Action, Job, JobMsg, Stage, Wizard};
use ui::{ConfigUiState, InstallerUiState, MAX_SLEEP_MINUTES, PANEL_STATES, SLEEP_ROW};

/// Shared write handle for sending `Command`s to the daemon. `None` when
/// disconnected. Populated by the reader thread on each successful connect.
type Writer = Arc<Mutex<Option<UnixStream>>>;

/// How often a waiting wizard screen re-probes the hardware. Slow enough that
/// a BLE scan (seconds) never overlaps itself, fast enough that pressing the
/// boot button feels responsive.
const PROBE_INTERVAL: Duration = Duration::from_millis(1500);

/// Input poll interval; also the wizard spinner's frame rate.
const TICK: Duration = Duration::from_millis(120);

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
        // No subcommand: launch the interactive TUI. On a first run (no setup
        // marker) that means the wizard; afterwards, the dashboard.
        None => {
            let start = if onboarding::setup_needed() { Screen::Wizard } else { Screen::Dashboard };
            run_tui(start)
        }
        // `setup` re-enters the wizard on demand; it needs the terminal, so it
        // is dispatched here rather than in `cli::run`.
        Some(cli::Commands::Setup) => run_tui(Screen::Wizard),
        // Any other subcommand: run it and exit (some arms exit directly).
        Some(command) => cli::run(command),
    }
}

/// Which screen the TUI should show next.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Screen {
    Wizard,
    Dashboard,
    Quit,
}

type Term = Terminal<CrosstermBackend<std::io::Stdout>>;

/// Launch the interactive TUI at `start`: spawn the control-socket reader
/// thread, install the terminal guard, and alternate between the wizard and the
/// dashboard until the user quits (`s` on the dashboard re-enters the wizard).
fn run_tui(start: Screen) -> anyhow::Result<()> {
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

    let mut screen = start;
    loop {
        screen = match screen {
            Screen::Wizard => run_wizard(&mut terminal)?,
            Screen::Dashboard => run(&mut terminal, &snap, &connected, &writer)?,
            Screen::Quit => return Ok(()),
        };
    }
}

// ---------------------------------------------------------------------------
// Setup wizard
// ---------------------------------------------------------------------------

/// Run one background job and report the result back over `tx`.
///
/// Every job here is either slow (a BLE scan, a firmware build, a 4 MiB flash
/// read) or blocked on a subprocess, so none of them may run on the UI thread:
/// the spinner has to keep turning while the user holds a button down.
fn spawn_job(job: Job, tx: Sender<JobMsg>) {
    std::thread::spawn(move || {
        // These two answer with data rather than log lines.
        match &job {
            Job::Probe => {
                let _ = tx.send(JobMsg::Probed(probe::probe()));
                return;
            }
            Job::FetchReleases => {
                let _ = tx.send(JobMsg::Releases(firmware::fetch_releases()));
                return;
            }
            _ => {}
        }
        let result = match &job {
            Job::Probe | Job::FetchReleases => unreachable!("handled above"),
            Job::Backup => flash::backup(None, None).map(|(_, lines)| lines),
            Job::Build => firmware::build().map(|(_, lines)| lines),
            Job::DownloadRelease(r) => firmware::download_release(r).map(|(_, lines)| lines),
            Job::Flash => flash::flash_capture(None, None),
            Job::InstallAgents(list) => install_agents(list),
        };
        let _ = tx.send(JobMsg::Finished { job, result });
    });
}

/// Install hooks for every chosen agent, collecting per-agent outcomes.
///
/// A failure for one agent does not abort the others — the user picked several
/// and deserves the ones that worked — but the overall result is an `Err` so
/// the UI never reports a clean success when something did not happen.
pub fn install_agents(list: &[AgentKind]) -> Result<Vec<String>, String> {
    let home = agents::home();
    let mut lines = Vec::new();
    let mut failed = false;
    for kind in list {
        match agents::install(*kind, &home) {
            Ok(report) => lines.push(report.summary()),
            Err(e) => {
                failed = true;
                lines.push(format!("{}: FAILED — {e}", kind.slug()));
            }
        }
    }
    if lines.is_empty() {
        lines.push("no agents selected".to_string());
    }
    if failed {
        Err(lines.join("\n"))
    } else {
        Ok(lines)
    }
}

/// Run the setup wizard until the user leaves it. Returns the screen to show
/// next.
fn run_wizard(terminal: &mut Term) -> anyhow::Result<Screen> {
    let (tx, rx): (Sender<JobMsg>, Receiver<JobMsg>) = mpsc::channel();
    let mut w = Wizard::new();
    // Whether a probe is in flight, so exactly one BLE scan runs at a time.
    let mut probing = false;
    let mut last_probe: Option<Instant> = None;

    loop {
        terminal.draw(|f| ui::render_wizard(f, &w))?;

        while let Ok(msg) = rx.try_recv() {
            match msg {
                JobMsg::Probed(p) => {
                    probing = false;
                    w.on_probe(p);
                }
                JobMsg::Releases(result) => {
                    w.busy = None;
                    w.on_releases(result);
                }
                JobMsg::Finished { job, result } => {
                    w.busy = None;
                    match result {
                        Ok(lines) => {
                            w.push_log(lines);
                            on_job_success(&mut w, &job);
                        }
                        Err(text) => {
                            w.push_log(text.lines().map(|l| l.to_string()));
                            w.refresh_env();
                        }
                    }
                }
            }
        }

        // Poll the hardware on the screens that react to it, and only when
        // nothing else is running: a flash or a backup owns the USB device and
        // must not race a probe.
        let due = last_probe.map(|t| t.elapsed() >= PROBE_INTERVAL).unwrap_or(true);
        if w.stage.polls() && w.busy.is_none() && !probing && due {
            probing = true;
            last_probe = Some(Instant::now());
            spawn_job(Job::Probe, tx.clone());
        }

        if event::poll(TICK)? {
            if let Event::Key(k) = event::read()? {
                if k.code == KeyCode::Char('q') {
                    return Ok(Screen::Quit);
                }
                // While a job runs, only quitting is accepted: every other key
                // would act on state the job is about to change.
                if w.busy.is_none() {
                    if let Some(screen) = wizard_key(&mut w, k.code, &tx, &mut last_probe) {
                        return Ok(screen);
                    }
                }
            }
        }
        w.tick();
    }
}

/// Apply a keypress to the wizard. Returns `Some(screen)` when the wizard is
/// finished and the TUI should move on.
fn wizard_key(
    w: &mut Wizard,
    code: KeyCode,
    tx: &Sender<JobMsg>,
    last_probe: &mut Option<Instant>,
) -> Option<Screen> {
    // `s` leaves setup from any screen and remembers not to open it again.
    // (Not on the agent screen, where the keys are a picker.)
    if code == KeyCode::Char('s') && w.stage != Stage::Agents {
        onboarding::mark_setup_done();
        return Some(Screen::Dashboard);
    }

    match w.stage {
        Stage::Warning => {
            if matches!(code, KeyCode::Char('y') | KeyCode::Enter) {
                w.stage = Stage::Detect;
                // Probe immediately rather than waiting out the interval.
                *last_probe = None;
            }
        }
        Stage::Detect | Stage::NeedCable | Stage::NeedBootMode => match code {
            KeyCode::Char('r') => *last_probe = None,
            KeyCode::Char('a') => {
                w.stage = Stage::Agents;
                w.refresh_env();
            }
            _ => {}
        },
        Stage::Firmware => match code {
            KeyCode::Up => w.action_move(-1),
            KeyCode::Down => w.action_move(1),
            KeyCode::Char('r') => {
                w.refresh_env();
                spawn_job(Job::Probe, tx.clone());
            }
            KeyCode::Char('a') => {
                w.stage = Stage::Agents;
                w.refresh_env();
            }
            KeyCode::Enter => start_selected_action(w, tx),
            _ => {}
        },
        Stage::Releases => match code {
            KeyCode::Up => w.release_move(-1),
            KeyCode::Down => w.release_move(1),
            KeyCode::Esc => w.stage = Stage::Firmware,
            KeyCode::Enter => {
                let release = w.selected_release().cloned()?;
                if let Some(why) = release.blocker() {
                    w.push_log([format!("cannot install {}: {why}", release.tag)]);
                    return None;
                }
                let job = Job::DownloadRelease(release);
                w.push_log([format!("starting: {}", job.label())]);
                w.busy = Some(job.clone());
                spawn_job(job, tx.clone());
            }
            _ => {}
        },
        Stage::Flashed => {
            if code == KeyCode::Enter {
                w.stage = Stage::Agents;
                w.refresh_env();
            }
        }
        Stage::Agents => match code {
            KeyCode::Up => w.agent_move(-1),
            KeyCode::Down => w.agent_move(1),
            KeyCode::Char(' ') => w.toggle_agent(),
            KeyCode::Char('n') => {
                w.stage = Stage::Done;
                onboarding::mark_setup_done();
            }
            KeyCode::Enter => {
                let chosen = w.chosen_agents();
                if chosen.is_empty() {
                    w.stage = Stage::Done;
                    onboarding::mark_setup_done();
                } else {
                    let job = Job::InstallAgents(chosen);
                    w.busy = Some(job.clone());
                    spawn_job(job, tx.clone());
                }
            }
            _ => {}
        },
        Stage::Done => {
            if code == KeyCode::Enter {
                onboarding::mark_setup_done();
                return Some(Screen::Dashboard);
            }
        }
    }
    None
}

/// Start the highlighted firmware action, if it can run.
fn start_selected_action(w: &mut Wizard, tx: &Sender<JobMsg>) {
    let Some(item) = w.selected_action() else { return };
    if !item.available {
        // Never pretend: say exactly why the action did not start.
        let msg = format!("cannot run \"{}\": {}", item.label, item.note);
        w.push_log([msg]);
        return;
    }
    let job = match item.action {
        Action::BackupStock => Job::Backup,
        Action::Build => Job::Build,
        // "Download" first fetches the version list; the picker comes next.
        Action::Download => Job::FetchReleases,
        Action::Flash => Job::Flash,
    };
    w.push_log([format!("starting: {}", job.label())]);
    w.busy = Some(job.clone());
    spawn_job(job, tx.clone());
}

/// Move the wizard on after a job reports success.
fn on_job_success(w: &mut Wizard, job: &Job) {
    match job {
        // A flash is the one job that changes what is running on the device.
        Job::Flash => w.stage = Stage::Flashed,
        // Image in hand: back to the menu, where "Flash" is now available.
        Job::DownloadRelease(_) => w.stage = Stage::Firmware,
        Job::InstallAgents(_) => {
            w.stage = Stage::Done;
            onboarding::mark_setup_done();
        }
        _ => {}
    }
    w.refresh_env();
}

// ---------------------------------------------------------------------------
// Dashboard
// ---------------------------------------------------------------------------

fn run(
    terminal: &mut Term,
    snap: &Arc<Mutex<SnapshotDto>>,
    connected: &Arc<AtomicBool>,
    writer: &Writer,
) -> anyhow::Result<Screen> {
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
                        KeyCode::Char('q') => return Ok(Screen::Quit),
                        KeyCode::Char('f') | KeyCode::Esc => installer.open = false,
                        KeyCode::Char('r') => installer.refresh(),
                        KeyCode::Enter => run_installer_flash(&mut installer),
                        _ => {}
                    }
                } else if cfg.open {
                    match k.code {
                        KeyCode::Char('q') => return Ok(Screen::Quit),
                        KeyCode::Char('c') | KeyCode::Esc => cfg.open = false,
                        KeyCode::Up => cfg.select_prev(),
                        KeyCode::Down => cfg.select_next(),
                        KeyCode::Left => adjust(&mut cfg, -1, writer),
                        KeyCode::Right => adjust(&mut cfg, 1, writer),
                        _ => {}
                    }
                } else {
                    match k.code {
                        KeyCode::Char('q') | KeyCode::Esc => return Ok(Screen::Quit),
                        // Re-open the setup wizard on demand.
                        KeyCode::Char('s') => return Ok(Screen::Wizard),
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
}

/// Attempt a real flash from the installer screen, capturing the flash tool's
/// output into the scrollable area. Only proceeds when every prerequisite
/// passes; it never fabricates success — a failure surfaces the real error.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_agents_with_no_selection_is_not_a_failure() {
        let out = install_agents(&[]).unwrap();
        assert_eq!(out, vec!["no agents selected".to_string()]);
    }

    #[test]
    fn a_failing_agent_is_reported_and_the_others_still_install() {
        // Codex's config already points `notify` somewhere else, so it must be
        // refused; Claude in the same scratch HOME must still be installed. The
        // UI can then never show a clean success for a partial run.
        let tmp = std::env::temp_dir()
            .join(format!("openmicro-install-agents-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(tmp.join(".codex")).unwrap();
        std::fs::write(tmp.join(".codex/config.toml"), "notify = [\"someone-else\"]\n").unwrap();

        // `agents::install` takes the home explicitly; drive it directly rather
        // than through `install_agents`, so this test never mutates the process
        // environment (other tests run in parallel threads and would see it).
        assert!(agents::install(AgentKind::Claude, &tmp).unwrap().changed);
        let err = agents::install(AgentKind::Codex, &tmp).unwrap_err();
        assert!(err.contains("different `notify`"), "{err}");

        let _ = std::fs::remove_dir_all(&tmp);
    }
}
