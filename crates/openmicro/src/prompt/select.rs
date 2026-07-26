use crossterm::event::{KeyCode, KeyEvent};

use super::render::{gutter, Theme};
use super::term::{is_cancel_key, next_event, FrameZone, PromptEvent, RawGuard};
use super::{Cancelled, SelectOption, Ui};

pub(crate) fn clack_window_start(
    prev: usize,
    cursor: usize,
    len: usize,
    max_items: usize,
) -> usize {
    if len <= max_items {
        return 0;
    }
    if cursor >= prev + max_items - 3 {
        (cursor + 3).saturating_sub(max_items).min(len - max_items)
    } else if cursor < prev + 2 {
        cursor.saturating_sub(2)
    } else {
        prev
    }
}

fn option_row(t: &Theme, label: &str, hint: Option<&str>, active: bool) -> String {
    if active {
        let hint = hint
            .map(|h| format!(" {}", t.style.dim(&format!("({h})"))))
            .unwrap_or_default();
        format!(
            "{} {}{hint}",
            t.style.green(t.sym.radio_active),
            t.style.underline(label)
        )
    } else {
        format!(
            "{} {}",
            t.style.dim(t.sym.radio_inactive),
            t.style.dim(label)
        )
    }
}

fn select_footer(t: &Theme) -> String {
    format!(
        "{}/{} to navigate {} {} confirm",
        t.style.dim(t.sym.arrow_up),
        t.style.dim(t.sym.arrow_down),
        t.sym.bullet,
        t.style.dim("Enter:")
    )
}

pub(crate) fn select_frame(
    t: &Theme,
    message: &str,
    options: &[(&str, Option<&str>)],
    cursor: usize,
    start: usize,
    max_items: usize,
) -> Vec<String> {
    let cbar = t.style.cyan(t.sym.bar);
    let mut lines = vec![
        gutter(t),
        format!("{}  {message}", t.style.cyan(t.sym.step_active)),
    ];
    let end = (start + max_items).min(options.len());
    let top_overflow = start > 0;
    let bottom_overflow = end < options.len();
    for (i, (label, hint)) in options[start..end].iter().enumerate() {
        let idx = start + i;
        let row = if (i == 0 && top_overflow) || (idx + 1 == end && bottom_overflow) {
            t.style.dim("...")
        } else {
            option_row(t, label, *hint, idx == cursor)
        };
        lines.push(format!("{cbar}  {row}"));
    }
    lines.push(format!("{cbar}  {}", select_footer(t)));
    lines.push(t.style.cyan(t.sym.bar_end));
    lines
}

pub(crate) fn select_done_frame(
    t: &Theme,
    message: &str,
    value: &str,
    cancelled: bool,
) -> Vec<String> {
    let symbol = if cancelled {
        t.style.red(t.sym.step_cancel)
    } else {
        t.style.green(t.sym.step_submit)
    };
    let value = if cancelled {
        t.style.strikethrough(&t.style.dim(value))
    } else {
        t.style.dim(value)
    };
    let mut lines = vec![
        gutter(t),
        format!("{symbol}  {message}"),
        format!("{}  {value}", gutter(t)),
    ];
    if cancelled {
        lines.push(gutter(t));
    }
    lines
}

pub(crate) fn confirm_frame(t: &Theme, message: &str, value: bool) -> Vec<String> {
    let pick = |label: &str, active: bool| {
        if active {
            format!("{} {label}", t.style.green(t.sym.radio_active))
        } else {
            format!(
                "{} {}",
                t.style.dim(t.sym.radio_inactive),
                t.style.dim(label)
            )
        }
    };
    vec![
        gutter(t),
        format!("{}  {message}", t.style.cyan(t.sym.step_active)),
        format!(
            "{}  {} {} {}",
            t.style.cyan(t.sym.bar),
            pick("Yes", value),
            t.style.dim("/"),
            pick("No", !value)
        ),
        t.style.cyan(t.sym.bar_end),
    ]
}

pub(crate) enum NavAction {
    Moved,
    Submit,
    Cancel,
    None,
}

pub(crate) fn select_handle_key(cursor: &mut usize, len: usize, key: &KeyEvent) -> NavAction {
    if is_cancel_key(key) {
        return NavAction::Cancel;
    }
    match key.code {
        KeyCode::Enter => NavAction::Submit,
        KeyCode::Up | KeyCode::Left | KeyCode::Char('k') => {
            *cursor = if *cursor == 0 { len - 1 } else { *cursor - 1 };
            NavAction::Moved
        }
        KeyCode::Down | KeyCode::Right | KeyCode::Char('j') => {
            *cursor = if *cursor + 1 == len { 0 } else { *cursor + 1 };
            NavAction::Moved
        }
        _ => NavAction::None,
    }
}

fn live_max_items() -> usize {
    let rows = crossterm::terminal::size().map(|(_, r)| r as usize).unwrap_or(24);
    rows.saturating_sub(4).max(5)
}

pub(crate) fn run_select<T: Clone>(
    ui: &mut Ui,
    message: &str,
    options: &[SelectOption<T>],
) -> Result<T, Cancelled> {
    if options.is_empty() || !ui.tty {
        return Err(Cancelled);
    }
    let t = ui.theme();
    let rows: Vec<(&str, Option<&str>)> = options
        .iter()
        .map(|o| (o.label.as_str(), o.hint.as_deref()))
        .collect();
    let _guard = RawGuard::new(true).map_err(|_| Cancelled)?;
    let mut zone = FrameZone::new();
    let mut cursor = 0usize;
    let mut start = 0usize;
    loop {
        let max_items = live_max_items();
        start = clack_window_start(start, cursor, rows.len(), max_items);
        let frame = select_frame(&t, message, &rows, cursor, start, max_items);
        zone.draw(&mut ui.out, &frame, ui.columns).map_err(|_| Cancelled)?;
        match next_event().map_err(|_| Cancelled)? {
            PromptEvent::Resize(cols) => {
                ui.columns = cols;
                zone.resize(cols);
            }
            PromptEvent::Key(key) => {
                match select_handle_key(&mut cursor, rows.len(), &key) {
                    NavAction::Submit => {
                        let done = select_done_frame(&t, message, rows[cursor].0, false);
                        let _ = zone.draw(&mut ui.out, &done, ui.columns);
                        return Ok(options[cursor].value.clone());
                    }
                    NavAction::Cancel => {
                        let done = select_done_frame(&t, message, rows[cursor].0, true);
                        let _ = zone.draw(&mut ui.out, &done, ui.columns);
                        return Err(Cancelled);
                    }
                    NavAction::Moved | NavAction::None => {}
                }
            }
        }
    }
}

pub(crate) fn run_confirm(ui: &mut Ui, message: &str, initial: bool) -> Result<bool, Cancelled> {
    if !ui.tty {
        return Err(Cancelled);
    }
    let t = ui.theme();
    let _guard = RawGuard::new(true).map_err(|_| Cancelled)?;
    let mut zone = FrameZone::new();
    let mut value = initial;
    loop {
        let frame = confirm_frame(&t, message, value);
        zone.draw(&mut ui.out, &frame, ui.columns).map_err(|_| Cancelled)?;
        match next_event().map_err(|_| Cancelled)? {
            PromptEvent::Resize(cols) => {
                ui.columns = cols;
                zone.resize(cols);
            }
            PromptEvent::Key(key) => {
                if is_cancel_key(&key) {
                    let label = if value { "Yes" } else { "No" };
                    let done = select_done_frame(&t, message, label, true);
                    let _ = zone.draw(&mut ui.out, &done, ui.columns);
                    return Err(Cancelled);
                }
                match key.code {
                    KeyCode::Enter => {
                        let label = if value { "Yes" } else { "No" };
                        let done = select_done_frame(&t, message, label, false);
                        let _ = zone.draw(&mut ui.out, &done, ui.columns);
                        return Ok(value);
                    }
                    KeyCode::Up | KeyCode::Down | KeyCode::Left | KeyCode::Right
                    | KeyCode::Char('h') | KeyCode::Char('l') => value = !value,
                    _ => {}
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::render::test_util::{plain, theme};
    use super::*;
    use crossterm::event::KeyModifiers;

    fn opts() -> Vec<(&'static str, Option<&'static str>)> {
        vec![("Alpha", Some("first")), ("Beta", None), ("Gamma", None)]
    }

    #[test]
    fn select_active_frame_layout() {
        let t = theme();
        let f = plain(&select_frame(&t, "Pick one", &opts(), 0, 0, 5));
        assert_eq!(f, vec![
            "│",
            "◆  Pick one",
            "│  ● Alpha (first)",
            "│  ○ Beta",
            "│  ○ Gamma",
            "│  ↑/↓ to navigate • Enter: confirm",
            "└",
        ]);
    }

    #[test]
    fn select_active_frame_palette() {
        let t = theme();
        let f = select_frame(&t, "Pick", &opts(), 0, 0, 5);
        assert!(f[1].starts_with("\x1b[36m◆\x1b[39m"), "step: {:?}", f[1]);
        assert!(f[2].starts_with("\x1b[36m│\x1b[39m"), "gutter: {:?}", f[2]);
        assert!(f[2].contains("\x1b[32m●\x1b[39m \x1b[4mAlpha\x1b[24m"), "row: {:?}", f[2]);
        assert!(f[2].contains("\x1b[2m(first)\x1b[22m"), "hint: {:?}", f[2]);
        assert!(f[3].contains("\x1b[2m○\x1b[22m \x1b[2mBeta\x1b[22m"), "row: {:?}", f[3]);
        assert_eq!(*f.last().unwrap(), "\x1b[36m└\x1b[39m");
    }

    #[test]
    fn select_window_replaces_overflow_rows_with_dim_dots() {
        let t = theme();
        let items: Vec<(&str, Option<&str>)> =
            vec![("a", None), ("b", None), ("c", None), ("d", None), ("e", None), ("f", None), ("g", None)];
        let f = plain(&select_frame(&t, "m", &items, 3, 1, 5));
        assert_eq!(f[2], "│  ...");
        assert_eq!(f[6], "│  ...");
        assert_eq!(f[4], "│  ● d");
    }

    #[test]
    fn clack_window_scrolls_at_cursor_ge_max_minus_three() {
        assert_eq!(clack_window_start(0, 0, 10, 5), 0);
        assert_eq!(clack_window_start(0, 1, 10, 5), 0);
        assert_eq!(clack_window_start(0, 2, 10, 5), 0);
        assert_eq!(clack_window_start(0, 3, 10, 5), 1);
        assert_eq!(clack_window_start(1, 4, 10, 5), 2);
        assert_eq!(clack_window_start(4, 9, 10, 5), 5);
        assert_eq!(clack_window_start(5, 6, 10, 5), 4);
        assert_eq!(clack_window_start(3, 4, 5, 5), 0);
    }

    #[test]
    fn select_submitted_frame_collapses_to_dim_value_on_gray_rail() {
        let t = theme();
        let f = select_done_frame(&t, "Pick one", "Beta", false);
        assert_eq!(plain(&f), vec!["│", "◇  Pick one", "│  Beta"]);
        assert!(f[1].starts_with("\x1b[32m◇\x1b[39m"), "symbol: {:?}", f[1]);
        assert!(f[0].starts_with("\x1b[90m"), "gutter must be gray: {:?}", f[0]);
        assert!(f[2].contains("\x1b[2mBeta\x1b[22m"), "value: {:?}", f[2]);
    }

    #[test]
    fn select_cancelled_frame_strikes_the_value() {
        let t = theme();
        let f = select_done_frame(&t, "Pick one", "Beta", true);
        assert_eq!(plain(&f), vec!["│", "■  Pick one", "│  Beta", "│"]);
        assert!(f[1].starts_with("\x1b[31m■\x1b[39m"), "symbol: {:?}", f[1]);
        assert!(
            f[2].contains("\x1b[9m\x1b[2mBeta\x1b[22m\x1b[29m"),
            "value must be strikethrough+dim: {:?}",
            f[2]
        );
    }

    #[test]
    fn confirm_renders_inline_yes_no_pair() {
        let t = theme();
        let yes = confirm_frame(&t, "Sure?", true);
        assert_eq!(plain(&yes)[2], "│  ● Yes / ○ No");
        assert!(yes[2].contains("\x1b[32m●\x1b[39m Yes"), "yes: {:?}", yes[2]);
        assert!(yes[2].contains("\x1b[2m○\x1b[22m \x1b[2mNo\x1b[22m"), "no: {:?}", yes[2]);
        let no = confirm_frame(&t, "Sure?", false);
        assert_eq!(plain(&no)[2], "│  ○ Yes / ● No");
    }

    #[test]
    fn select_keys_wrap_around_both_ends() {
        let up = KeyEvent::new(KeyCode::Up, KeyModifiers::NONE);
        let down = KeyEvent::new(KeyCode::Down, KeyModifiers::NONE);
        let mut cursor = 0usize;
        select_handle_key(&mut cursor, 3, &up);
        assert_eq!(cursor, 2, "up from first wraps to last");
        select_handle_key(&mut cursor, 3, &down);
        assert_eq!(cursor, 0, "down from last wraps to first");
    }

    #[test]
    fn select_enter_and_escape_map_to_submit_and_cancel() {
        let mut cursor = 1usize;
        let enter = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
        let esc = KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE);
        assert!(matches!(select_handle_key(&mut cursor, 3, &enter), NavAction::Submit));
        assert!(matches!(select_handle_key(&mut cursor, 3, &esc), NavAction::Cancel));
        assert_eq!(cursor, 1, "submit/cancel must not move the cursor");
    }
}
