use std::io::{self, Write};

use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::terminal;

use super::width::frame_rows;

pub(crate) struct RawGuard {
    cursor_hidden: bool,
}

impl RawGuard {
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

    pub(crate) fn resize(&mut self, columns: usize) {
        self.rows = frame_rows(&self.last, columns);
    }
}

pub(crate) enum PromptEvent {
    Key(KeyEvent),
    Resize(usize),
}

pub(crate) fn next_event() -> io::Result<PromptEvent> {
    loop {
        match crossterm::event::read()? {
            Event::Key(k) if k.kind != KeyEventKind::Release => return Ok(PromptEvent::Key(k)),
            Event::Resize(cols, _) => return Ok(PromptEvent::Resize(cols as usize)),
            _ => {}
        }
    }
}

pub(crate) fn is_cancel_key(k: &KeyEvent) -> bool {
    k.code == KeyCode::Esc
        || (k.code == KeyCode::Char('c') && k.modifiers.contains(KeyModifiers::CONTROL))
}

#[cfg(test)]
mod tests {
    use super::*;

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
