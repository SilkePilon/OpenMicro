
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cancelled;

impl std::fmt::Display for Cancelled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("cancelled")
    }
}

impl std::error::Error for Cancelled {}

#[derive(Clone, Debug)]
pub struct SelectOption<T> {
    pub value: T,
    pub label: String,
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

#[derive(Clone, Debug, Default)]
pub struct Item {
    pub value: String,
    pub label: String,
    pub hint: Option<String>,
    pub detail: Option<String>,
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

#[derive(Clone, Debug)]
pub struct LockedSection {
    pub title: String,
    pub items: Vec<String>,
    pub max_shown: usize,
}

#[derive(Clone, Debug)]
pub struct MultiOpts {
    pub max_visible: usize,
    pub locked: Option<LockedSection>,
    pub list_title: Option<String>,
    pub searchable: bool,
    pub show_detail: bool,
    pub detail_lines: usize,
    pub required: bool,
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

pub struct Ui {
    pub(crate) out: io::Stdout,
    pub(crate) style: Style,
    pub(crate) sym: &'static Symbols,
    pub(crate) unicode: bool,
    pub(crate) tty: bool,
    pub(crate) columns: usize,
}

impl Ui {
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

    pub fn style(&self) -> Style {
        self.style
    }

    pub fn columns(&self) -> usize {
        self.columns
    }

    pub fn symbols(&self) -> &'static Symbols {
        self.sym
    }

    pub(crate) fn theme(&self) -> Theme {
        Theme {
            style: self.style,
            sym: self.sym,
        }
    }

    fn append(&mut self, lines: Vec<String>) {
        let mut buf = String::new();
        for line in lines {
            buf.push_str(&line);
            buf.push('\n');
        }
        let _ = self.out.write_all(buf.as_bytes());
        let _ = self.out.flush();
    }

    pub fn banner(&mut self, lines: &[&str]) {
        let frame = render::banner_frame(self.style, lines);
        self.append(frame);
    }

    pub fn intro(&mut self, badge: &str) {
        let frame = render::intro_frame(&self.theme(), badge);
        self.append(frame);
    }

    pub fn outro(&mut self, msg: &str) {
        let frame = render::outro_frame(&self.theme(), msg);
        self.append(frame);
    }

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

    pub fn warn(&mut self, msg: &str) {
        let frame = render::log_frame(&self.theme(), LogKind::Warn, msg);
        self.append(frame);
    }

    pub fn error(&mut self, msg: &str) {
        let frame = render::log_frame(&self.theme(), LogKind::Error, msg);
        self.append(frame);
    }

    pub fn note(&mut self, title: &str, body: &str) {
        let frame = render::note_frame(&self.theme(), title, body, self.columns);
        self.append(frame);
    }

    pub fn select<T: Clone>(
        &mut self,
        message: &str,
        options: &[SelectOption<T>],
    ) -> Result<T, Cancelled> {
        select::run_select(self, message, options)
    }

    pub fn confirm(&mut self, message: &str, initial: bool) -> Result<bool, Cancelled> {
        select::run_confirm(self, message, initial)
    }

    pub fn search_multiselect(
        &mut self,
        message: &str,
        items: &[Item],
        opts: &MultiOpts,
    ) -> Result<Vec<String>, Cancelled> {
        multiselect::run(self, message, items, opts)
    }

    pub fn spinner(&mut self) -> Spinner {
        Spinner::new(self.theme(), self.unicode, self.tty, self.columns)
    }
}

impl Default for Ui {
    fn default() -> Self {
        Ui::new()
    }
}
