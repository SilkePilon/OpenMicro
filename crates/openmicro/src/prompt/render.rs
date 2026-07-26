use super::style::{Style, BANNER_GRADIENT};
use super::symbols::Symbols;
use super::width::{display_width, wrap};

#[derive(Clone, Copy)]
pub(crate) struct Theme {
    pub style: Style,
    pub sym: &'static Symbols,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LogKind {
    Info,
    Success,
    Warn,
    Error,
}

impl LogKind {
    pub(crate) fn symbol(self, t: &Theme) -> String {
        match self {
            LogKind::Info => t.style.blue(t.sym.info),
            LogKind::Success => t.style.green(t.sym.success),
            LogKind::Warn => t.style.yellow(t.sym.warn),
            LogKind::Error => t.style.red(t.sym.error),
        }
    }
}

pub(crate) fn gutter(t: &Theme) -> String {
    t.style.gray(t.sym.bar)
}

pub(crate) fn intro_frame(t: &Theme, msg: &str) -> Vec<String> {
    vec![format!("{}  {msg}", t.style.gray(t.sym.bar_start))]
}

pub(crate) fn outro_frame(t: &Theme, msg: &str) -> Vec<String> {
    vec![
        gutter(t),
        format!("{}  {msg}", t.style.gray(t.sym.bar_end)),
        String::new(),
    ]
}

pub(crate) fn cancel_frame(t: &Theme, msg: &str) -> Vec<String> {
    vec![
        format!("{}  {}", t.style.gray(t.sym.bar_end), t.style.red(msg)),
        String::new(),
    ]
}

pub(crate) fn log_frame(t: &Theme, kind: LogKind, msg: &str) -> Vec<String> {
    let mut lines = vec![gutter(t)];
    for (i, ln) in msg.split('\n').enumerate() {
        let prefix = if i == 0 { kind.symbol(t) } else { gutter(t) };
        if ln.is_empty() {
            lines.push(prefix);
        } else {
            lines.push(format!("{prefix}  {ln}"));
        }
    }
    lines
}

pub(crate) fn note_frame(t: &Theme, title: &str, body: &str, columns: usize) -> Vec<String> {
    let wrap_width = columns.saturating_sub(6).max(10);
    let mut content = vec![String::new()];
    content.extend(wrap(body, wrap_width));
    content.push(String::new());

    let n = display_width(title);
    let widest = content.iter().map(|l| display_width(l)).max().unwrap_or(0);
    let box_w = widest.max(n) + 2;

    let mut lines = vec![gutter(t)];
    lines.push(format!(
        "{}  {title} {}",
        t.style.green(t.sym.step_submit),
        t.style.gray(&format!(
            "{}{}",
            t.sym.bar_h.repeat((box_w.saturating_sub(n + 1)).max(1)),
            t.sym.corner_top_right
        )),
    ));
    for ln in &content {
        lines.push(format!(
            "{}  {ln}{}{}",
            t.style.gray(t.sym.bar),
            " ".repeat(box_w - display_width(ln)),
            t.style.gray(t.sym.bar),
        ));
    }
    lines.push(t.style.gray(&format!(
        "{}{}{}",
        t.sym.connect_left,
        t.sym.bar_h.repeat(box_w + 2),
        t.sym.corner_bottom_right
    )));
    lines
}

pub(crate) fn banner_frame(style: Style, rows: &[&str]) -> Vec<String> {
    let mut lines = vec![String::new()];
    for (i, row) in rows.iter().enumerate() {
        let shade = BANNER_GRADIENT[i.min(BANNER_GRADIENT.len() - 1)];
        lines.push(style.fg256(shade, row));
    }
    lines
}

#[cfg(test)]
pub(crate) mod test_util {
    use super::*;

    pub(crate) fn theme() -> Theme {
        Theme {
            style: Style::new(true),
            sym: &super::super::symbols::UNICODE,
        }
    }

    pub(crate) fn plain(lines: &[String]) -> Vec<String> {
        lines
            .iter()
            .map(|l| super::super::width::strip_ansi(l))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::test_util::{plain, theme};
    use super::*;

    #[test]
    fn intro_is_corner_two_spaces_message() {
        let t = theme();
        assert_eq!(intro_frame(&t, "OpenMicro"), vec![
            "\x1b[90m┌\x1b[39m  OpenMicro".to_string()
        ]);
    }

    #[test]
    fn outro_ends_with_blank_line() {
        let t = theme();
        let f = outro_frame(&t, "Done");
        assert_eq!(plain(&f), vec!["│", "└  Done", ""]);
        assert!(f[0].starts_with("\x1b[90m"));
        assert!(f[1].starts_with("\x1b[90m└\x1b[39m"));
    }

    #[test]
    fn cancel_is_red_message_after_gray_corner() {
        let t = theme();
        let f = cancel_frame(&t, "Aborted");
        assert_eq!(f[0], "\x1b[90m└\x1b[39m  \x1b[31mAborted\x1b[39m");
        assert_eq!(f[1], "");
    }

    #[test]
    fn log_variant_symbols_and_colours_match_spec() {
        let t = theme();
        assert_eq!(LogKind::Info.symbol(&t), "\x1b[34m●\x1b[39m");
        assert_eq!(LogKind::Success.symbol(&t), "\x1b[32m◆\x1b[39m");
        assert_eq!(LogKind::Warn.symbol(&t), "\x1b[33m▲\x1b[39m");
        assert_eq!(LogKind::Error.symbol(&t), "\x1b[31m■\x1b[39m");
    }

    #[test]
    fn log_prefixes_continuation_lines_with_gutter() {
        let t = theme();
        let f = log_frame(&t, LogKind::Info, "first\nsecond");
        assert_eq!(plain(&f), vec!["│", "●  first", "│  second"]);
    }

    #[test]
    fn log_empty_line_is_bare_symbol_without_trailing_spaces() {
        let t = theme();
        let f = log_frame(&t, LogKind::Warn, "");
        assert_eq!(plain(&f), vec!["│", "▲"]);
        assert!(!f[1].ends_with(' '));
    }

    #[test]
    fn note_box_geometry_and_corners() {
        let t = theme();
        let f = plain(&note_frame(&t, "Note", "hi", 80));
        assert_eq!(f, vec![
            "│",
            "◇  Note ─╮",
            "│        │",
            "│  hi    │",
            "│        │",
            "├────────╯",
        ]);
        let widths: Vec<usize> = f[1..].iter().map(|l| display_width(l)).collect();
        assert!(widths.iter().all(|w| *w == widths[0]), "ragged box: {f:?}");
    }

    #[test]
    fn note_header_dash_run_never_drops_below_one() {
        let t = theme();
        let f = plain(&note_frame(&t, "A very long note title", "x", 80));
        assert!(f[1].contains(" ─╮"), "header was {:?}", f[1]);
    }

    #[test]
    fn note_footer_is_connect_left_not_corner() {
        let t = theme();
        let f = plain(&note_frame(&t, "T", "body", 80));
        let footer = f.last().unwrap();
        assert!(footer.starts_with('├'), "footer was {footer:?}");
        assert!(footer.ends_with('╯'), "footer was {footer:?}");
    }

    #[test]
    fn banner_uses_gradient_codes_per_row() {
        let s = Style::new(true);
        let f = banner_frame(s, &["a", "b", "c", "d", "e", "f"]);
        assert_eq!(f[0], "");
        assert_eq!(f[1], "\x1b[38;5;250ma\x1b[39m");
        assert_eq!(f[2], "\x1b[38;5;248mb\x1b[39m");
        assert_eq!(f[3], "\x1b[38;5;245mc\x1b[39m");
        assert_eq!(f[4], "\x1b[38;5;243md\x1b[39m");
        assert_eq!(f[5], "\x1b[38;5;240me\x1b[39m");
        assert_eq!(f[6], "\x1b[38;5;238mf\x1b[39m");
    }

    #[test]
    fn banner_rows_beyond_six_keep_darkest_shade() {
        let s = Style::new(true);
        let f = banner_frame(s, &["1", "2", "3", "4", "5", "6", "7"]);
        assert_eq!(f[7], "\x1b[38;5;238m7\x1b[39m");
    }
}
