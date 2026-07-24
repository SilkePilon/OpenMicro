//! Clack-style terminal prompt toolkit — the exact look and feel of
//! `npx skills add`, per the extracted spec in `docs/tui-style.md`.
//!
//! The transcript is append-only in the normal buffer (no alternate screen):
//! committed blocks are printed once with a gray gutter, and only the active
//! prompt redraws in place with a cyan (clack) or dim (search prompt)
//! gutter. All frame construction is pure (`state -> Vec<String>`) in the
//! submodules, which is what makes the layout and palette testable with
//! plain `cargo test`; this module owns the environment detection and the
//! ergonomic entry points.
//!
//! ```ignore
//! let mut ui = prompt::Ui::new();
//! ui.intro("openmicro");
//! let fw = ui.select("Firmware?", &options)?; // Err(Cancelled) on Esc/^C
//! let mut sp = ui.spinner();
//! sp.start("Flashing");
//! sp.stop("Flashed");
//! ui.outro("Done");
//! ```

// The menu-driven CLI that consumes this toolkit is being built in a
// parallel change; until it lands, the binary target sees these items as
// unreachable. Remove once `main.rs` is wired up.
#![allow(dead_code)]

mod multiselect;
mod render;
mod select;
mod spinner;
mod style;
mod symbols;
pub(crate) mod term;
mod width;

use std::io::{self, Write};

use crossterm::tty::IsTty;

use render::{LogKind, Theme};

pub use spinner::Spinner;
pub use style::Style;
pub use symbols::Symbols;

/// Returned when the user hits Esc or Ctrl-C at a prompt (and, so callers
/// can't hang a headless run, when stdout is not a terminal). Implements
/// `Error` so `?` folds it into `anyhow` flows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cancelled;

impl std::fmt::Display for Cancelled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("cancelled")
    }
}

impl std::error::Error for Cancelled {}

/// One choice in [`Ui::select`].
#[derive(Clone, Debug)]
pub struct SelectOption<T> {
    pub value: T,
    pub label: String,
    /// Shown dim in parentheses, on the active row only (clack behaviour).
    pub hint: Option<String>,
}

impl<T> SelectOption<T> {
    pub fn new(value: T, label: impl Into<String>) -> Self {
        SelectOption {
            value,
            label: label.into(),
            hint: None,
        }
    }

    pub fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }
}

/// One row of the searchable multi-select.
#[derive(Clone, Debug, Default)]
pub struct Item {
    /// Stable identifier; this is what filtering matches and what
    /// `search_multiselect` returns.
    pub value: String,
    pub label: String,
    /// Shown dim in parentheses after the label.
    pub hint: Option<String>,
    /// Shown in the fixed-height detail pane for the cursor row.
    pub detail: Option<String>,
    /// Accepted for API stability but not yet rendered: the spec documents
    /// the group glyphs (`▾`/`▸`/`◐`) without a layout for group rows, so
    /// inventing one would break the "same exact theme" goal.
    pub group: Option<String>,
}

impl Item {
    pub fn new(value: impl Into<String>, label: impl Into<String>) -> Item {
        Item {
            value: value.into(),
            label: label.into(),
            ..Item::default()
        }
    }

    pub fn with_hint(mut self, hint: impl Into<String>) -> Item {
        self.hint = Some(hint.into());
        self
    }

    pub fn with_detail(mut self, detail: impl Into<String>) -> Item {
        self.detail = Some(detail.into());
        self
    }
}

/// The pinned "always included" block above the searchable list. Rendered
/// with `── {title} ── always included ────────────` (fixed 12 trailing
/// dashes) and bulleted bold rows, truncated at `max_shown` with
/// `…and N more`.
#[derive(Clone, Debug)]
pub struct LockedSection {
    pub title: String,
    pub items: Vec<String>,
    pub max_shown: usize,
}

/// Behaviour knobs for [`Ui::search_multiselect`].
#[derive(Clone, Debug)]
pub struct MultiOpts {
    /// Height of the cursor-centred item window.
    pub max_visible: usize,
    pub locked: Option<LockedSection>,
    /// Title of the `── {title} ────…` header over the searchable list
    /// (e.g. "Additional agents"); `None` drops the header line.
    pub list_title: Option<String>,
    /// When false, the `Search:` line is omitted and typing does nothing.
    pub searchable: bool,
    pub show_detail: bool,
    /// Body rows of the detail pane; fixed so the frame height never
    /// changes as the cursor moves.
    pub detail_lines: usize,
    /// When true, Enter is silently ignored while nothing is selected.
    pub required: bool,
    /// Values pre-selected when the prompt opens.
    pub initial_selected: Vec<String>,
}

impl Default for MultiOpts {
    fn default() -> Self {
        MultiOpts {
            max_visible: 8,
            locked: None,
            list_title: None,
            searchable: true,
            show_detail: false,
            detail_lines: 3,
            required: false,
            initial_selected: Vec::new(),
        }
    }
}

/// The toolkit's handle: owns stdout plus the capability flags decided once
/// at startup (unicode glyphs, colour, tty-ness, width).
pub struct Ui {
    pub(crate) out: io::Stdout,
    pub(crate) style: Style,
    pub(crate) sym: &'static Symbols,
    pub(crate) unicode: bool,
    pub(crate) tty: bool,
    pub(crate) columns: usize,
}

impl Ui {
    /// Detect capabilities the same way `npx skills add` does: glyphs fall
    /// back to ASCII only under `TERM=linux`; ANSI is dropped when stdout is
    /// not a tty or `NO_COLOR` is set (any non-empty value, per the
    /// no-color.org convention).
    pub fn new() -> Ui {
        let unicode = std::env::var("TERM").map(|t| t != "linux").unwrap_or(true);
        let out = io::stdout();
        let tty = out.is_tty();
        let no_color = std::env::var_os("NO_COLOR").is_some_and(|v| !v.is_empty());
        let columns = crossterm::terminal::size()
            .map(|(c, _)| c as usize)
            .unwrap_or(80);
        Ui {
            out,
            style: Style::new(tty && !no_color),
            sym: symbols::set(unicode),
            unicode,
            tty,
            columns,
        }
    }

    /// The active style palette, for callers composing their own text (e.g.
    /// a `bg_cyan` intro badge). Honour it instead of hardcoding ANSI so
    /// `NO_COLOR` keeps working.
    pub fn style(&self) -> Style {
        self.style
    }

    pub fn columns(&self) -> usize {
        self.columns
    }

    /// The glyph set in use, so a caller drawing its own live region (the
    /// agent-activity view) keeps the same gutter as the prompts around it.
    pub fn symbols(&self) -> &'static Symbols {
        self.sym
    }

    pub(crate) fn theme(&self) -> Theme {
        Theme {
            style: self.style,
            sym: self.sym,
        }
    }

    /// Print committed lines (one `\n` each) and flush. All non-interactive
    /// blocks funnel through here.
    fn append(&mut self, lines: Vec<String>) {
        let mut buf = String::new();
        for line in lines {
            buf.push_str(&line);
            buf.push('\n');
        }
        let _ = self.out.write_all(buf.as_bytes());
        let _ = self.out.flush();
    }

    /// The wordmark banner: one blank line, then the given rows in a static
    /// vertical gray gradient (one 256-colour per row, light at the top).
    pub fn banner(&mut self, lines: &[&str]) {
        let frame = render::banner_frame(self.style, lines);
        self.append(frame);
    }

    /// `┌  badge` — opens the rail. The badge is printed verbatim; style it
    /// via [`Ui::style`] if desired.
    pub fn intro(&mut self, badge: &str) {
        let frame = render::intro_frame(&self.theme(), badge);
        self.append(frame);
    }

    /// `└  msg` closing the rail, followed by a blank line.
    pub fn outro(&mut self, msg: &str) {
        let frame = render::outro_frame(&self.theme(), msg);
        self.append(frame);
    }

    /// `└  red(msg)` — the rail's abort ending, followed by a blank line.
    pub fn cancel(&mut self, msg: &str) {
        let frame = render::cancel_frame(&self.theme(), msg);
        self.append(frame);
    }

    pub fn info(&mut self, msg: &str) {
        let frame = render::log_frame(&self.theme(), LogKind::Info, msg);
        self.append(frame);
    }

    pub fn success(&mut self, msg: &str) {
        let frame = render::log_frame(&self.theme(), LogKind::Success, msg);
        self.append(frame);
    }

    pub fn step(&mut self, msg: &str) {
        let frame = render::log_frame(&self.theme(), LogKind::Step, msg);
        self.append(frame);
    }

    pub fn warn(&mut self, msg: &str) {
        let frame = render::log_frame(&self.theme(), LogKind::Warn, msg);
        self.append(frame);
    }

    pub fn error(&mut self, msg: &str) {
        let frame = render::log_frame(&self.theme(), LogKind::Error, msg);
        self.append(frame);
    }

    /// The boxed note whose top-left corner is the step symbol and whose
    /// bottom-left is `├`, so the rail continues into the next step.
    pub fn note(&mut self, title: &str, body: &str) {
        let frame = render::note_frame(&self.theme(), title, body, self.columns);
        self.append(frame);
    }

    /// Single-choice clack select. Returns the chosen value, or
    /// `Err(Cancelled)` on Esc/Ctrl-C (the frame collapses to the red `■`
    /// state first, exactly like the original).
    pub fn select<T: Clone>(
        &mut self,
        message: &str,
        options: &[SelectOption<T>],
    ) -> Result<T, Cancelled> {
        select::run_select(self, message, options)
    }

    /// Yes/no clack confirm; `initial` picks the highlighted side.
    pub fn confirm(&mut self, message: &str, initial: bool) -> Result<bool, Cancelled> {
        select::run_confirm(self, message, initial)
    }

    /// The hand-rolled searchable multi-select (dim gutter, `❯` cursor,
    /// `Selected:` summary). Returns the chosen values in selection order.
    pub fn search_multiselect(
        &mut self,
        message: &str,
        items: &[Item],
        opts: &MultiOpts,
    ) -> Result<Vec<String>, Cancelled> {
        multiselect::run(self, message, items, opts)
    }

    /// A spinner bound to this Ui's theme. Call `start`, then one of
    /// `stop`/`error`/`cancel`; dropping it mid-spin cancels safely.
    pub fn spinner(&mut self) -> Spinner {
        Spinner::new(self.theme(), self.unicode, self.tty, self.columns)
    }
}

impl Default for Ui {
    fn default() -> Self {
        Ui::new()
    }
}
