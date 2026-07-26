use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::render::Theme;
use super::term::{is_cancel_key, next_event, FrameZone, PromptEvent, RawGuard};
use super::width::wrap_clipped;
use super::{Cancelled, Item, MultiOpts, Ui};

const LEADING_DASHES: usize = 2;
const LOCKED_TRAILING_DASHES: usize = 12;
const LIST_TRAILING_DASHES: usize = 29;

const LOCKED_SUFFIX: &str = "always included";

#[derive(Clone, Debug, Default)]
pub(crate) struct MultiState {
    pub query: String,
    pub cursor: usize,
    pub selected: Vec<String>,
}

impl MultiState {
    pub(crate) fn filtered(&self, items: &[Item]) -> Vec<usize> {
        if self.query.is_empty() {
            return (0..items.len()).collect();
        }
        let q = self.query.to_lowercase();
        items
            .iter()
            .enumerate()
            .filter(|(_, it)| {
                it.label.to_lowercase().contains(&q) || it.value.to_lowercase().contains(&q)
            })
            .map(|(i, _)| i)
            .collect()
    }

    fn toggle(&mut self, value: &str) {
        if let Some(pos) = self.selected.iter().position(|v| v == value) {
            self.selected.remove(pos);
        } else {
            self.selected.push(value.to_string());
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Action {
    Redraw,
    Submit,
    Cancel,
}

pub(crate) fn handle_key(
    state: &mut MultiState,
    key: &KeyEvent,
    items: &[Item],
    opts: &MultiOpts,
) -> Action {
    if is_cancel_key(key) {
        return Action::Cancel;
    }
    let filtered = state.filtered(items);
    match key.code {
        KeyCode::Enter => {
            if opts.required && state.selected.is_empty() {
                Action::Redraw
            } else {
                Action::Submit
            }
        }
        KeyCode::Up => {
            state.cursor = state.cursor.saturating_sub(1);
            Action::Redraw
        }
        KeyCode::Down => {
            if !filtered.is_empty() && state.cursor + 1 < filtered.len() {
                state.cursor += 1;
            }
            Action::Redraw
        }
        KeyCode::Char(' ') => {
            if let Some(&idx) = filtered.get(state.cursor) {
                let value = items[idx].value.clone();
                state.toggle(&value);
            }
            Action::Redraw
        }
        KeyCode::Backspace => {
            if opts.searchable {
                state.query.pop();
                state.cursor = 0;
            }
            Action::Redraw
        }
        KeyCode::Char(c)
            if opts.searchable
                && !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
        {
            state.query.push(c);
            state.cursor = 0;
            Action::Redraw
        }
        _ => Action::Redraw,
    }
}

pub(crate) fn centered_window(cursor: usize, len: usize, max_visible: usize) -> (usize, usize) {
    if len <= max_visible {
        return (0, len);
    }
    let start = cursor
        .saturating_sub(max_visible / 2)
        .min(len - max_visible);
    (start, start + max_visible)
}

fn summary_body(labels: &[String]) -> String {
    let shown = labels.iter().take(3).cloned().collect::<Vec<_>>().join(", ");
    if labels.len() > 3 {
        format!("{shown} +{} more", labels.len() - 3)
    } else {
        shown
    }
}

fn selected_labels(selected: &[String], items: &[Item]) -> Vec<String> {
    selected
        .iter()
        .map(|v| {
            items
                .iter()
                .find(|it| &it.value == v)
                .map(|it| it.label.clone())
                .unwrap_or_else(|| v.clone())
        })
        .collect()
}

pub(crate) struct MultiView<'a> {
    pub message: &'a str,
    pub items: &'a [Item],
    pub state: &'a MultiState,
    pub opts: &'a MultiOpts,
    pub columns: usize,
}

pub(crate) fn active_frame(t: &Theme, v: &MultiView<'_>) -> Vec<String> {
    let s = t.style;
    let sym = t.sym;
    let bar = s.dim(sym.bar);
    let dash = |n: usize| sym.bar_h.repeat(n);
    let mut lines = Vec::new();

    lines.push(format!("{}  {}", s.green(sym.step_active), s.bold(v.message)));
    lines.push(bar.clone());

    if let Some(locked) = &v.opts.locked {
        lines.push(format!(
            "{bar}  {} {} {} {} {}",
            s.dim(&dash(LEADING_DASHES)),
            s.bold(&locked.title),
            s.dim(&dash(LEADING_DASHES)),
            s.dim(LOCKED_SUFFIX),
            s.dim(&dash(LOCKED_TRAILING_DASHES)),
        ));
        for item in locked.items.iter().take(locked.max_shown) {
            lines.push(format!("{bar}    {} {}", s.dim(sym.bullet), s.bold(item)));
        }
        if locked.items.len() > locked.max_shown {
            lines.push(format!(
                "{bar}    {}",
                s.dim(&format!(
                    "{}and {} more",
                    sym.ellipsis,
                    locked.items.len() - locked.max_shown
                ))
            ));
        }
        lines.push(bar.clone());
    }

    if let Some(title) = &v.opts.list_title {
        lines.push(format!(
            "{bar}  {} {} {}",
            s.dim(&dash(LEADING_DASHES)),
            s.bold(title),
            s.dim(&dash(LIST_TRAILING_DASHES)),
        ));
    }
    if v.opts.searchable {
        lines.push(format!("{bar}  Search: {}{}", v.state.query, s.inverse(" ")));
    }
    lines.push(format!(
        "{bar}  {}",
        s.dim(&format!(
            "{}{} move, space select, enter confirm",
            sym.arrow_up, sym.arrow_down
        ))
    ));
    lines.push(bar.clone());

    let filtered = v.state.filtered(v.items);
    let cursor = v.state.cursor.min(filtered.len().saturating_sub(1));
    if filtered.is_empty() {
        lines.push(format!("{bar}  {}", s.dim("(no matches)")));
    } else {
        let (start, end) = centered_window(cursor, filtered.len(), v.opts.max_visible.max(1));
        for (row, &idx) in filtered[start..end].iter().enumerate() {
            let item = &v.items[idx];
            let at_cursor = start + row == cursor;
            let is_selected = v.state.selected.iter().any(|sel| sel == &item.value);
            let pointer = if at_cursor {
                s.cyan(sym.cursor)
            } else {
                " ".to_string()
            };
            let glyph = if is_selected {
                s.green(sym.radio_active)
            } else if at_cursor {
                s.cyan(sym.radio_inactive)
            } else {
                s.dim(sym.radio_inactive)
            };
            let label = if at_cursor {
                s.underline(&item.label)
            } else {
                item.label.clone()
            };
            let hint = item
                .hint
                .as_deref()
                .map(|h| format!(" {}", s.dim(&format!("({h})"))))
                .unwrap_or_default();
            lines.push(format!("{bar} {pointer} {glyph} {label}{hint}"));
        }
        let mut overflow = Vec::new();
        if start > 0 {
            overflow.push(format!("{} {start} more", sym.arrow_up));
        }
        if end < filtered.len() {
            overflow.push(format!("{} {} more", sym.arrow_down, filtered.len() - end));
        }
        if !overflow.is_empty() {
            lines.push(format!("{bar}  {}", s.dim(&overflow.join("  "))));
        }
    }
    lines.push(bar.clone());

    if v.opts.show_detail {
        lines.push(format!("{bar}  Description"));
        let detail = filtered
            .get(cursor)
            .and_then(|&idx| v.items[idx].detail.as_deref())
            .unwrap_or("");
        let width = v.columns.saturating_sub(5).max(10);
        let body = wrap_clipped(detail, width, v.opts.detail_lines, sym.ellipsis);
        for i in 0..v.opts.detail_lines {
            match body.get(i) {
                Some(ln) => lines.push(format!("{bar}  {}", s.dim(ln))),
                None => lines.push(bar.clone()),
            }
        }
        lines.push(bar.clone());
    }

    let labels = selected_labels(&v.state.selected, v.items);
    if labels.is_empty() {
        lines.push(format!("{bar}  {}", s.dim("Selected: (none)")));
    } else {
        lines.push(format!(
            "{bar}  {} {}",
            s.green("Selected:"),
            summary_body(&labels)
        ));
    }
    lines.push(s.dim(sym.bar_end));
    lines
}

pub(crate) fn done_frame(
    t: &Theme,
    message: &str,
    items: &[Item],
    selected: &[String],
    cancelled: bool,
) -> Vec<String> {
    let s = t.style;
    if cancelled {
        return vec![
            format!("{}  {}", s.red(t.sym.step_cancel), s.bold(message)),
            format!(
                "{}  {}",
                s.dim(t.sym.bar),
                s.strikethrough(&s.dim("Cancelled"))
            ),
        ];
    }
    let labels = selected_labels(selected, items);
    let joined = if labels.is_empty() {
        "(none)".to_string()
    } else {
        labels.join(", ")
    };
    vec![
        format!("{}  {}", s.green(t.sym.step_submit), s.bold(message)),
        format!("{}  {}", s.dim(t.sym.bar), s.dim(&joined)),
    ]
}

pub(crate) fn run(
    ui: &mut Ui,
    message: &str,
    items: &[Item],
    opts: &MultiOpts,
) -> Result<Vec<String>, Cancelled> {
    if !ui.tty {
        return Err(Cancelled);
    }
    let t = ui.theme();
    let _guard = RawGuard::new(false).map_err(|_| Cancelled)?;
    let mut zone = FrameZone::new();
    let mut state = MultiState {
        selected: opts.initial_selected.clone(),
        ..MultiState::default()
    };
    loop {
        let frame = active_frame(
            &t,
            &MultiView {
                message,
                items,
                state: &state,
                opts,
                columns: ui.columns,
            },
        );
        zone.draw(&mut ui.out, &frame, ui.columns).map_err(|_| Cancelled)?;
        match next_event().map_err(|_| Cancelled)? {
            PromptEvent::Resize(cols) => {
                ui.columns = cols;
                zone.resize(cols);
            }
            PromptEvent::Key(key) => match handle_key(&mut state, &key, items, opts) {
                Action::Submit => {
                    let done = done_frame(&t, message, items, &state.selected, false);
                    let _ = zone.draw(&mut ui.out, &done, ui.columns);
                    return Ok(state.selected);
                }
                Action::Cancel => {
                    let done = done_frame(&t, message, items, &state.selected, true);
                    let _ = zone.draw(&mut ui.out, &done, ui.columns);
                    return Err(Cancelled);
                }
                Action::Redraw => {}
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::render::test_util::{plain, theme};
    use super::super::LockedSection;
    use super::*;

    fn items(n: usize) -> Vec<Item> {
        (0..n)
            .map(|i| Item::new(format!("val{i}"), format!("Item {i}")))
            .collect()
    }

    fn base_opts() -> MultiOpts {
        MultiOpts {
            max_visible: 3,
            list_title: Some("Additional agents".to_string()),
            ..MultiOpts::default()
        }
    }

    fn view<'a>(
        message: &'a str,
        items: &'a [Item],
        state: &'a MultiState,
        opts: &'a MultiOpts,
    ) -> MultiView<'a> {
        MultiView {
            message,
            items,
            state,
            opts,
            columns: 80,
        }
    }

    #[test]
    fn frame_with_locked_section_matches_spec_layout() {
        let t = theme();
        let its = items(3);
        let opts = MultiOpts {
            locked: Some(LockedSection {
                title: "Core".to_string(),
                items: vec!["A".into(), "B".into(), "C".into(), "D".into(), "E".into()],
                max_shown: 2,
            }),
            ..base_opts()
        };
        let state = MultiState::default();
        let f = plain(&active_frame(&t, &view("Add agents", &its, &state, &opts)));
        assert_eq!(f, vec![
            "◆  Add agents",
            "│",
            "│  ── Core ── always included ────────────",
            "│    • A",
            "│    • B",
            "│    …and 3 more",
            "│",
            "│  ── Additional agents ─────────────────────────────",
            "│  Search:  ",
            "│  ↑↓ move, space select, enter confirm",
            "│",
            "│ ❯ ○ Item 0",
            "│   ○ Item 1",
            "│   ○ Item 2",
            "│",
            "│  Selected: (none)",
            "└",
        ]);
    }

    #[test]
    fn frame_without_locked_section_skips_straight_to_list_header() {
        let t = theme();
        let its = items(2);
        let opts = base_opts();
        let state = MultiState::default();
        let f = plain(&active_frame(&t, &view("Add", &its, &state, &opts)));
        assert_eq!(f[1], "│");
        assert!(
            f[2].starts_with("│  ── Additional agents"),
            "expected list header, got {:?}",
            f[2]
        );
    }

    #[test]
    fn section_dash_counts_are_fixed() {
        let t = theme();
        let its = items(1);
        let opts = MultiOpts {
            locked: Some(LockedSection {
                title: "T".to_string(),
                items: vec!["A".into()],
                max_shown: 5,
            }),
            ..base_opts()
        };
        let state = MultiState::default();
        let f = plain(&active_frame(&t, &view("m", &its, &state, &opts)));
        let locked = &f[2];
        let list = f.iter().find(|l| l.contains("Additional")).unwrap();
        assert!(locked.ends_with(&"─".repeat(12)), "locked: {locked:?}");
        assert!(!locked.ends_with(&"─".repeat(13)), "locked: {locked:?}");
        assert!(list.ends_with(&"─".repeat(29)), "list: {list:?}");
        assert!(!list.ends_with(&"─".repeat(30)), "list: {list:?}");
    }

    #[test]
    fn search_prompt_palette_differs_from_clack() {
        let t = theme();
        let its = items(2);
        let opts = base_opts();
        let state = MultiState::default();
        let f = active_frame(&t, &view("Add", &its, &state, &opts));
        assert!(f[0].starts_with("\x1b[32m◆\x1b[39m"), "step: {:?}", f[0]);
        assert_eq!(f[1], "\x1b[2m│\x1b[22m", "gutter must be dim");
        let search = f.iter().find(|l| l.contains("Search:")).unwrap();
        assert!(search.ends_with("\x1b[7m \x1b[27m"), "caret: {search:?}");
        let row = f.iter().find(|l| l.contains("Item 0")).unwrap();
        assert!(row.contains("\x1b[36m❯\x1b[39m"), "pointer: {row:?}");
        assert!(row.contains("\x1b[36m○\x1b[39m"), "hovered glyph cyan: {row:?}");
        assert!(row.contains("\x1b[4mItem 0\x1b[24m"), "label underline: {row:?}");
        assert_eq!(*f.last().unwrap(), "\x1b[2m└\x1b[22m");
    }

    #[test]
    fn selected_rows_are_green_regardless_of_cursor() {
        let t = theme();
        let its = items(2);
        let opts = base_opts();
        let state = MultiState {
            selected: vec!["val1".to_string()],
            ..MultiState::default()
        };
        let f = active_frame(&t, &view("Add", &its, &state, &opts));
        let row = f.iter().find(|l| l.contains("Item 1")).unwrap();
        assert!(row.contains("\x1b[32m●\x1b[39m"), "selected glyph: {row:?}");
    }

    #[test]
    fn query_filters_case_insensitively_over_label_and_value() {
        let its = vec![
            Item::new("alpha", "First"),
            Item::new("beta", "Second ALPHA"),
            Item::new("gamma", "Third"),
        ];
        let state = MultiState {
            query: "alph".to_string(),
            ..MultiState::default()
        };
        assert_eq!(state.filtered(&its), vec![0, 1]);
    }

    #[test]
    fn filtered_frame_shows_only_matches() {
        let t = theme();
        let its = vec![
            Item::new("alpha", "Alpha"),
            Item::new("beta", "Beta"),
            Item::new("gamma", "Gamma"),
        ];
        let opts = base_opts();
        let state = MultiState {
            query: "ta".to_string(),
            ..MultiState::default()
        };
        let f = plain(&active_frame(&t, &view("Add", &its, &state, &opts)));
        assert!(f.iter().any(|l| l.contains("Beta")));
        assert!(!f.iter().any(|l| l.contains("Alpha")));
        assert!(f.iter().any(|l| l.contains("Search: ta")));
    }

    #[test]
    fn scrolled_frame_shows_combined_overflow_line() {
        let t = theme();
        let its = items(10);
        let opts = base_opts();
        let state = MultiState {
            cursor: 5,
            ..MultiState::default()
        };
        let f = plain(&active_frame(&t, &view("Add", &its, &state, &opts)));
        assert!(f.iter().any(|l| l.contains("Item 5")));
        let overflow = f.iter().find(|l| l.contains("more")).unwrap();
        assert_eq!(overflow.as_str(), "│  ↑ 4 more  ↓ 3 more");
    }

    #[test]
    fn centered_window_clamps_at_both_ends() {
        assert_eq!(centered_window(0, 10, 4), (0, 4));
        assert_eq!(centered_window(9, 10, 4), (6, 10));
        assert_eq!(centered_window(5, 10, 4), (3, 7));
        assert_eq!(centered_window(2, 3, 5), (0, 3));
    }

    #[test]
    fn empty_result_state_renders_no_matches_row() {
        let t = theme();
        let its = items(3);
        let opts = base_opts();
        let state = MultiState {
            query: "zzz".to_string(),
            ..MultiState::default()
        };
        let f = plain(&active_frame(&t, &view("Add", &its, &state, &opts)));
        assert!(f.iter().any(|l| l == "│  (no matches)"), "frame: {f:?}");
        assert!(!f.iter().any(|l| l.contains("Item")), "frame: {f:?}");
    }

    #[test]
    fn selected_summary_truncates_at_three_labels() {
        let t = theme();
        let its = items(6);
        let opts = base_opts();
        let state = MultiState {
            selected: vec![
                "val0".into(),
                "val1".into(),
                "val2".into(),
                "val3".into(),
                "val4".into(),
            ],
            ..MultiState::default()
        };
        let f = plain(&active_frame(&t, &view("Add", &its, &state, &opts)));
        let summary = f.iter().find(|l| l.contains("Selected:")).unwrap();
        assert_eq!(summary.as_str(), "│  Selected: Item 0, Item 1, Item 2 +2 more");
    }

    #[test]
    fn empty_selection_summary_is_fully_dim_including_label() {
        let t = theme();
        let its = items(1);
        let opts = base_opts();
        let state = MultiState::default();
        let f = active_frame(&t, &view("Add", &its, &state, &opts));
        let summary = f.iter().find(|l| l.contains("Selected:")).unwrap();
        assert!(summary.contains("\x1b[2mSelected: (none)\x1b[22m"), "{summary:?}");
        assert!(!summary.contains("\x1b[32m"), "{summary:?}");
    }

    #[test]
    fn non_empty_selection_label_is_green() {
        let t = theme();
        let its = items(2);
        let opts = base_opts();
        let state = MultiState {
            selected: vec!["val0".into()],
            ..MultiState::default()
        };
        let f = active_frame(&t, &view("Add", &its, &state, &opts));
        let summary = f.iter().find(|l| l.contains("Selected:")).unwrap();
        assert!(summary.contains("\x1b[32mSelected:\x1b[39m"), "{summary:?}");
    }

    #[test]
    fn detail_pane_height_is_fixed() {
        let t = theme();
        let mut its = items(2);
        its[0].detail = Some("word ".repeat(60));
        let opts = MultiOpts {
            show_detail: true,
            detail_lines: 2,
            ..base_opts()
        };
        let long = MultiState::default();
        let none = MultiState {
            cursor: 1,
            ..MultiState::default()
        };
        let f_long = plain(&active_frame(&t, &view("Add", &its, &long, &opts)));
        let f_none = plain(&active_frame(&t, &view("Add", &its, &none, &opts)));
        assert_eq!(f_long.len(), f_none.len(), "frame height moved with cursor");
        assert!(f_long.iter().any(|l| l.ends_with('…')), "{f_long:?}");
    }

    #[test]
    fn space_toggles_selection_of_cursor_row() {
        let its = items(3);
        let opts = base_opts();
        let mut state = MultiState::default();
        let space = KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE);
        handle_key(&mut state, &space, &its, &opts);
        assert_eq!(state.selected, vec!["val0".to_string()]);
        handle_key(&mut state, &space, &its, &opts);
        assert!(state.selected.is_empty(), "second space must untoggle");
    }

    #[test]
    fn arrows_clamp_without_wraparound() {
        let its = items(3);
        let opts = base_opts();
        let mut state = MultiState::default();
        let up = KeyEvent::new(KeyCode::Up, KeyModifiers::NONE);
        let down = KeyEvent::new(KeyCode::Down, KeyModifiers::NONE);
        handle_key(&mut state, &up, &its, &opts);
        assert_eq!(state.cursor, 0, "up at top must clamp, not wrap");
        for _ in 0..10 {
            handle_key(&mut state, &down, &its, &opts);
        }
        assert_eq!(state.cursor, 2, "down at bottom must clamp, not wrap");
    }

    #[test]
    fn typing_appends_to_query_and_resets_cursor() {
        let its = items(5);
        let opts = base_opts();
        let mut state = MultiState {
            cursor: 3,
            ..MultiState::default()
        };
        let key = KeyEvent::new(KeyCode::Char('I'), KeyModifiers::SHIFT);
        handle_key(&mut state, &key, &its, &opts);
        assert_eq!(state.query, "I");
        assert_eq!(state.cursor, 0);
    }

    #[test]
    fn backspace_pops_query_and_resets_cursor() {
        let its = items(5);
        let opts = base_opts();
        let mut state = MultiState {
            query: "ab".to_string(),
            cursor: 2,
            ..MultiState::default()
        };
        let key = KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE);
        handle_key(&mut state, &key, &its, &opts);
        assert_eq!(state.query, "a");
        assert_eq!(state.cursor, 0);
    }

    #[test]
    fn required_swallows_enter_silently_while_empty() {
        let its = items(2);
        let opts = MultiOpts {
            required: true,
            ..base_opts()
        };
        let mut state = MultiState::default();
        let enter = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
        assert_eq!(handle_key(&mut state, &enter, &its, &opts), Action::Redraw);
        state.selected.push("val0".into());
        assert_eq!(handle_key(&mut state, &enter, &its, &opts), Action::Submit);
    }

    #[test]
    fn submitted_collapse_joins_selection_with_no_footer() {
        let t = theme();
        let its = items(3);
        let f = done_frame(&t, "Add agents", &its, &["val1".into(), "val0".into()], false);
        assert_eq!(plain(&f), vec!["◇  Add agents", "│  Item 1, Item 0"]);
        assert!(f[0].starts_with("\x1b[32m◇\x1b[39m"), "symbol: {:?}", f[0]);
        assert!(
            !plain(&f).iter().any(|l| l.contains('└')),
            "submitted frame must not have a bar-end footer"
        );
    }

    #[test]
    fn cancelled_collapse_strikes_the_word_cancelled() {
        let t = theme();
        let its = items(1);
        let f = done_frame(&t, "Add agents", &its, &[], true);
        assert_eq!(plain(&f), vec!["■  Add agents", "│  Cancelled"]);
        assert!(f[0].starts_with("\x1b[31m■\x1b[39m"), "symbol: {:?}", f[0]);
        assert!(
            f[1].contains("\x1b[9m\x1b[2mCancelled\x1b[22m\x1b[29m"),
            "value: {:?}",
            f[1]
        );
    }
}
