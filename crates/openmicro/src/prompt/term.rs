//! Terminal plumbing: raw-mode guard, in-place frame redraw, key events.
//!
//! Everything impure lives here so the frame builders stay pure functions.
//! The rendering model (see `docs/tui-style.md`) is append-only: no alternate
//! screen, only the active prompt block is redrawn, via `ESC[{n}A` +
//! `ESC[J` and then the whole frame in a single `write` — clearing rows one
//! by one makes large prompts visibly flash.

use std::io::{self, Write};

use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::terminal;

use super::width::frame_rows;

/// RAII guard for raw mode and (optionally) cursor visibility.
///
/// Constructed at the top of every interactive prompt. `Drop` runs on normal
/// return, on `?`-propagated errors, and during panic unwinding, so the
/// user's shell is never left wedged in raw mode with a hidden cursor.
/// Restore errors are deliberately ignored — cleanup must never panic.
pub(crate) struct RawGuard {
    cursor_hidden: bool,
}

impl RawGuard {
    /// `hide_cursor` is true for clack prompts; the search prompt keeps the
    /// real cursor visible (its caret is a fake `inverse(' ')`, and the
    /// original tool never hides the cursor there).
    pub(crate) fn new(hide_cursor: bool) -> io::Result<RawGuard> {
        terminal::enable_raw_mode()?;
        if hide_cursor {
            let mut out = io::stdout();
            let _ = out.write_all(b"\x1b[?25l");
            let _ = out.flush();
        }
        Ok(RawGuard {
            cursor_hidden: hide_cursor,
        })
    }
}

impl Drop for RawGuard {
    fn drop(&mut self) {
        let _ = terminal::disable_raw_mode();
        if self.cursor_hidden {
            let mut out = io::stdout();
            let _ = out.write_all(b"\x1b[?25h");
            let _ = out.flush();
        }
    }
}

/// The redraw zone of the active prompt: remembers how many physical rows the
/// previous frame occupied so the next draw can move up over exactly that
/// region, and keeps the last frame so a resize can re-count its rows under
/// the new width.
pub(crate) struct FrameZone {
    rows: usize,
    last: Vec<String>,
}

impl FrameZone {
    pub(crate) fn new() -> FrameZone {
        FrameZone {
            rows: 0,
            last: Vec::new(),
        }
    }

    /// Erase the previous frame and write the new one in a single `write`.
    /// Lines are terminated with `\r\n` because we are in raw mode; the
    /// cursor ends at column 0 on the row just below the frame, which is
    /// exactly where the erase sequence expects it next time.
    pub(crate) fn draw(
        &mut self,
        out: &mut impl Write,
        lines: &[String],
        columns: usize,
    ) -> io::Result<()> {
        let mut buf = String::new();
        if self.rows > 0 {
            buf.push_str(&format!("\x1b[{}A\r\x1b[J", self.rows));
        }
        for line in lines {
            buf.push_str(line);
            buf.push_str("\r\n");
        }
        out.write_all(buf.as_bytes())?;
        out.flush()?;
        self.rows = frame_rows(lines, columns);
        self.last = lines.to_vec();
        Ok(())
    }

    /// Re-count the previous frame's rows under a new terminal width. Called
    /// on `Resize` before the next draw. This is best-effort — terminals that
    /// reflow scrollback wrap the old frame themselves — but it keeps the
    /// prompt itself intact, which is more than the original tool manages
    /// (it ignores SIGWINCH and corrupts).
    pub(crate) fn resize(&mut self, columns: usize) {
        self.rows = frame_rows(&self.last, columns);
    }
}

/// Events an interactive prompt loop cares about.
pub(crate) enum PromptEvent {
    Key(KeyEvent),
    Resize(usize),
}

/// Block for the next relevant event. Key releases are filtered out (they
/// arrive on Windows and under the kitty keyboard protocol; acting on both
/// press and release double-steps every keystroke).
pub(crate) fn next_event() -> io::Result<PromptEvent> {
    loop {
        match crossterm::event::read()? {
            Event::Key(k) if k.kind != KeyEventKind::Release => return Ok(PromptEvent::Key(k)),
            Event::Resize(cols, _) => return Ok(PromptEvent::Resize(cols as usize)),
            _ => {}
        }
    }
}

/// Esc or Ctrl-C. In raw mode Ctrl-C is an ordinary key event, not SIGINT,
/// which is what lets prompts return `Cancelled` instead of killing the
/// process mid-frame.
pub(crate) fn is_cancel_key(k: &KeyEvent) -> bool {
    k.code == KeyCode::Esc
        || (k.code == KeyCode::Char('c') && k.modifiers.contains(KeyModifiers::CONTROL))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Sanity-check the erase maths without a terminal: a frame with a
    /// wrapping line must record its *physical* rows, and a resize must
    /// re-count them, because `ESC[{n}A` with a logical-line count leaves
    /// ghost rows on narrow terminals.
    #[test]
    fn frame_zone_counts_wrapped_rows_and_recounts_on_resize() {
        let mut zone = FrameZone::new();
        let mut sink = Vec::new();
        let lines = vec!["x".repeat(100), "short".to_string()];
        zone.draw(&mut sink, &lines, 80).unwrap();
        assert_eq!(zone.rows, 2 + 1);
        zone.resize(40);
        assert_eq!(zone.rows, 3 + 1);
    }

    #[test]
    fn second_draw_erases_previous_rows_in_one_write() {
        let mut zone = FrameZone::new();
        let mut sink = Vec::new();
        zone.draw(&mut sink, &["a".to_string(), "b".to_string()], 80)
            .unwrap();
        sink.clear();
        zone.draw(&mut sink, &["c".to_string()], 80).unwrap();
        let out = String::from_utf8(sink).unwrap();
        assert!(
            out.starts_with("\x1b[2A\r\x1b[J"),
            "redraw did not erase two rows: {out:?}"
        );
    }

    #[test]
    fn first_draw_does_not_move_the_cursor_up() {
        let mut zone = FrameZone::new();
        let mut sink = Vec::new();
        zone.draw(&mut sink, &["a".to_string()], 80).unwrap();
        let out = String::from_utf8(sink).unwrap();
        assert_eq!(out, "a\r\n");
    }

    #[test]
    fn cancel_keys_are_esc_and_ctrl_c() {
        let esc = KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE);
        let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        let plain_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::NONE);
        assert!(is_cancel_key(&esc));
        assert!(is_cancel_key(&ctrl_c));
        assert!(!is_cancel_key(&plain_c));
    }
}
