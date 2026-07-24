//! The spinner — the only animation in the whole interface.
//!
//! Two independent motions share one 80 ms tick (120 ms in ASCII mode): the
//! four-frame magenta glyph, and a trailing-dot counter that advances by
//! 0.125 per tick and wraps above 4, showing `floor(counter)` dots capped at
//! 3 — one more dot every 640 ms on a 2.56 s cycle. Both are pure functions
//! of the tick count so they can be unit-tested without threads.
//!
//! Cleanup is the part that has to be paranoid: the render thread hides the
//! cursor and holds raw mode (so Ctrl-C arrives as a key event we can catch
//! and repaint on, instead of SIGINT killing the process mid-frame), and
//! `Spinner`'s `Drop` restores everything even during panic unwinding.

use std::io::{self, Write};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crossterm::event::{Event, KeyEventKind};
use crossterm::terminal;

use super::render::Theme;
use super::term::is_cancel_key;
use super::width::frame_rows;

/// Unicode frames at 80 ms; ASCII fallback `• o O 0` at 120 ms (the `•` is
/// the spec's own "ASCII" table entry, mirroring clack).
const UNICODE_FRAMES: [&str; 4] = ["◒", "◐", "◓", "◑"];
const ASCII_FRAMES: [&str; 4] = ["•", "o", "O", "0"];
const UNICODE_TICK: Duration = Duration::from_millis(80);
const ASCII_TICK: Duration = Duration::from_millis(120);

/// Dots after `tick` completed ticks. The spec models this as a float
/// counter (`+0.125` per tick, wrapping to 0 above 4, capped at 3); with a
/// wrap at exactly 4.0 that is a 32-tick cycle of 8 ticks per dot count, so
/// integer arithmetic reproduces it without float drift.
pub(crate) fn dots_for_tick(tick: u64) -> usize {
    ((tick % 32) / 8) as usize
}

/// Trailing ASCII dots are stripped from an incoming message so the dot
/// animation doesn't stack on top of literal ones — but a trailing `…` is
/// kept, which is why the original shows `Parsing source….`.
pub(crate) fn strip_trailing_dots(msg: &str) -> &str {
    msg.trim_end_matches('.')
}

/// State shared with the render thread. `rows` mirrors the last frame's
/// physical row count so the finishing thread (which does not own the
/// render loop's `FrameZone`) can erase the live frame before painting the
/// final line.
struct Shared {
    msg: Mutex<String>,
    stop: AtomicBool,
    rows: AtomicUsize,
}

/// A running (or about-to-run) spinner. Obtain via [`super::Ui::spinner`],
/// then `start` / `message` / `stop` / `error` / `cancel`. Dropping a
/// still-active spinner cancels it, so an early `?` return or a panic can
/// never leave the terminal in raw mode with a hidden cursor and a stale
/// frame.
pub struct Spinner {
    theme: Theme,
    tty: bool,
    columns: usize,
    tick: Duration,
    frames: &'static [&'static str; 4],
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
    active: bool,
}

impl Spinner {
    pub(crate) fn new(theme: Theme, unicode: bool, tty: bool, columns: usize) -> Spinner {
        Spinner {
            theme,
            tty,
            columns,
            tick: if unicode { UNICODE_TICK } else { ASCII_TICK },
            frames: if unicode { &UNICODE_FRAMES } else { &ASCII_FRAMES },
            shared: Arc::new(Shared {
                msg: Mutex::new(String::new()),
                stop: AtomicBool::new(false),
                rows: AtomicUsize::new(0),
            }),
            thread: None,
            active: false,
        }
    }

    /// Begin animating. On a non-tty there is nothing to animate: the final
    /// `stop`/`error`/`cancel` line is the only output, keeping piped logs
    /// clean.
    pub fn start(&mut self, msg: &str) {
        *self.shared.msg.lock().unwrap() = strip_trailing_dots(msg).to_string();
        self.active = true;
        if !self.tty || self.thread.is_some() {
            return;
        }
        self.shared.stop.store(false, Ordering::SeqCst);
        if terminal::enable_raw_mode().is_err() {
            return;
        }
        {
            let mut out = io::stdout();
            let _ = out.write_all(b"\x1b[?25l");
            let _ = out.flush();
        }
        let shared = Arc::clone(&self.shared);
        let theme = self.theme;
        let frames = self.frames;
        let tick = self.tick;
        let mut columns = self.columns;
        self.thread = Some(std::thread::spawn(move || {
            let mut out = io::stdout();
            let mut tick_no: u64 = 0;
            let mut prev_rows = 0usize;
            // Definite-assignment: every path to the reads below passes
            // through the `last_lines = lines` store in this iteration.
            let mut last_lines: Vec<String>;
            loop {
                if shared.stop.load(Ordering::SeqCst) {
                    break;
                }
                let msg = shared.msg.lock().unwrap().clone();
                let line = format!(
                    "{}  {}{}",
                    theme.style.magenta(frames[(tick_no as usize) % frames.len()]),
                    msg,
                    ".".repeat(dots_for_tick(tick_no)),
                );
                let lines = vec![theme.style.gray(theme.sym.bar), line];
                let _ = draw_over(&mut out, prev_rows, &lines);
                prev_rows = frame_rows(&lines, columns);
                shared.rows.store(prev_rows, Ordering::SeqCst);
                last_lines = lines;
                tick_no += 1;
                // Sleep by polling the event queue so Ctrl-C (a key event in
                // raw mode) and resizes are noticed mid-tick.
                let deadline = Instant::now() + tick;
                loop {
                    if shared.stop.load(Ordering::SeqCst) {
                        return;
                    }
                    let now = Instant::now();
                    if now >= deadline {
                        break;
                    }
                    let wait = (deadline - now).min(Duration::from_millis(20));
                    if let Ok(true) = crossterm::event::poll(wait) {
                        match crossterm::event::read() {
                            Ok(Event::Key(k))
                                if k.kind != KeyEventKind::Release && is_cancel_key(&k) =>
                            {
                                // Emergency exit: repaint as cancelled,
                                // restore the terminal, and die with the
                                // conventional SIGINT status. There is no
                                // way to unwind the main thread from here.
                                let final_line = format!(
                                    "{}  {msg}",
                                    theme.style.red(theme.sym.step_cancel)
                                );
                                let lines =
                                    vec![theme.style.gray(theme.sym.bar), final_line];
                                let _ = draw_over(&mut out, prev_rows, &lines);
                                let _ = out.write_all(b"\x1b[?25h");
                                let _ = out.flush();
                                let _ = terminal::disable_raw_mode();
                                std::process::exit(130);
                            }
                            Ok(Event::Resize(cols, _)) => {
                                columns = cols as usize;
                                // Re-count the frame we actually drew under
                                // the new width so the next erase is exact.
                                prev_rows = frame_rows(&last_lines, columns);
                                shared.rows.store(prev_rows, Ordering::SeqCst);
                            }
                            _ => {}
                        }
                    }
                }
            }
        }));
    }

    /// Swap the message mid-spin (trailing ASCII dots stripped, as on start).
    pub fn message(&mut self, msg: &str) {
        *self.shared.msg.lock().unwrap() = strip_trailing_dots(msg).to_string();
    }

    /// Successful finish: `green ◇  msg`.
    pub fn stop(&mut self, msg: &str) {
        let t = self.theme;
        self.finish(t.style.green(t.sym.step_submit), msg);
    }

    /// Failure finish: red `▲` — the one place the error triangle is red
    /// rather than yellow (spec: "yellow (red when a spinner errors)").
    pub fn error(&mut self, msg: &str) {
        let t = self.theme;
        self.finish(t.style.red(t.sym.step_error), msg);
    }

    /// Cancelled finish: `red ■  msg`.
    pub fn cancel(&mut self, msg: &str) {
        let t = self.theme;
        self.finish(t.style.red(t.sym.step_cancel), msg);
    }

    fn finish(&mut self, symbol: String, msg: &str) {
        if !self.active {
            return;
        }
        self.active = false;
        let msg = if msg.is_empty() {
            self.shared.msg.lock().unwrap().clone()
        } else {
            strip_trailing_dots(msg).to_string()
        };
        self.shared.stop.store(true, Ordering::SeqCst);
        if let Some(handle) = self.thread.take() {
            let _ = handle.join();
        }
        let mut out = io::stdout();
        if self.tty {
            let rows = self.shared.rows.swap(0, Ordering::SeqCst);
            let lines = vec![
                self.theme.style.gray(self.theme.sym.bar),
                format!("{symbol}  {msg}"),
            ];
            let _ = draw_over(&mut out, rows, &lines);
            let _ = out.write_all(b"\x1b[?25h");
            let _ = out.flush();
            let _ = terminal::disable_raw_mode();
        } else {
            // Piped output: just the resolution line, newline-terminated.
            let _ = writeln!(out, "{symbol}  {msg}");
            let _ = out.flush();
        }
    }
}

impl Drop for Spinner {
    /// A spinner abandoned mid-flight (early return, panic unwind) must not
    /// leave raw mode on and the cursor hidden; treat it as cancelled.
    fn drop(&mut self) {
        if self.active {
            self.cancel("");
        }
    }
}

/// Erase `prev_rows` physical rows and write the new lines in one `write`
/// (the same batched redraw the prompts use; per-row clearing flashes).
fn draw_over(out: &mut impl Write, prev_rows: usize, lines: &[String]) -> io::Result<()> {
    let mut buf = String::new();
    if prev_rows > 0 {
        buf.push_str(&format!("\x1b[{prev_rows}A\r\x1b[J"));
    }
    for line in lines {
        buf.push_str(line);
        buf.push_str("\r\n");
    }
    out.write_all(buf.as_bytes())?;
    out.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Re-simulate the spec's float-counter model literally and check the
    /// integer implementation matches it tick for tick across several
    /// cycles.
    #[test]
    fn dot_sequence_matches_float_counter_model() {
        let mut counter: f64 = 0.0;
        for tick in 0..200u64 {
            let expected = (counter.floor() as usize).min(3);
            assert_eq!(
                dots_for_tick(tick),
                expected,
                "diverged from counter model at tick {tick}"
            );
            counter += 0.125;
            if counter >= 4.0 {
                counter = 0.0;
            }
        }
    }

    /// One more dot every 8 ticks (640 ms at 80 ms/tick), wrapping to zero
    /// after the 3-dot phase — a 32-tick / 2.56 s cycle.
    #[test]
    fn dots_step_every_eight_ticks_and_wrap_at_thirty_two() {
        assert_eq!(dots_for_tick(0), 0);
        assert_eq!(dots_for_tick(7), 0);
        assert_eq!(dots_for_tick(8), 1);
        assert_eq!(dots_for_tick(16), 2);
        assert_eq!(dots_for_tick(24), 3);
        assert_eq!(dots_for_tick(31), 3);
        assert_eq!(dots_for_tick(32), 0);
    }

    #[test]
    fn trailing_ascii_dots_are_stripped_but_ellipsis_kept() {
        assert_eq!(strip_trailing_dots("Loading..."), "Loading");
        assert_eq!(strip_trailing_dots("Parsing source…"), "Parsing source…");
        assert_eq!(strip_trailing_dots("plain"), "plain");
    }

    #[test]
    fn frame_sets_match_spec_glyphs_and_rates() {
        assert_eq!(UNICODE_FRAMES, ["◒", "◐", "◓", "◑"]);
        assert_eq!(ASCII_FRAMES, ["•", "o", "O", "0"]);
        assert_eq!(UNICODE_TICK, Duration::from_millis(80));
        assert_eq!(ASCII_TICK, Duration::from_millis(120));
    }

    /// The rendered spinner line is `magenta frame`, exactly two spaces,
    /// then the message — assert the colour code explicitly.
    #[test]
    fn spinner_line_is_magenta_frame_two_spaces_message() {
        let t = super::super::render::test_util::theme();
        let line = format!("{}  {}", t.style.magenta(UNICODE_FRAMES[0]), "Working");
        assert_eq!(line, "\x1b[35m◒\x1b[39m  Working");
    }
}
