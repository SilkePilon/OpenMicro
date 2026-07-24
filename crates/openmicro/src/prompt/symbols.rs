//! Glyph sets, matching the symbol table in `docs/tui-style.md`.
//!
//! Unicode is used unless `TERM=linux` — that is the *whole* fallback
//! condition (mirroring `npx skills add` on Unix), so keep any smarter
//! detection out of here. The ASCII column follows the spec table exactly,
//! including its two deliberate non-ASCII entries (`—` for the bar end and
//! `•` for info) because that is what clack itself ships.

/// One glyph set. Static borrows keep frame builders allocation-free for
/// symbols and make the table trivially comparable against the spec in tests.
pub struct Symbols {
    pub step_active: &'static str,
    pub step_submit: &'static str,
    pub step_cancel: &'static str,
    pub step_error: &'static str,
    pub bar_start: &'static str,
    pub bar: &'static str,
    pub bar_end: &'static str,
    pub bar_h: &'static str,
    pub radio_active: &'static str,
    pub radio_inactive: &'static str,
    pub checkbox_selected: &'static str,
    pub checkbox_empty: &'static str,
    pub info: &'static str,
    pub success: &'static str,
    pub warn: &'static str,
    pub error: &'static str,
    pub corner_top_right: &'static str,
    pub connect_left: &'static str,
    pub corner_bottom_right: &'static str,
    // Inline glyphs of the hand-rolled search prompt.
    pub cursor: &'static str,
    pub expanded: &'static str,
    pub collapsed: &'static str,
    pub partial: &'static str,
    pub bullet: &'static str,
    pub ellipsis: &'static str,
    pub arrow_up: &'static str,
    pub arrow_down: &'static str,
}

pub(crate) static UNICODE: Symbols = Symbols {
    step_active: "◆",           // U+25C6
    step_submit: "◇",           // U+25C7
    step_cancel: "■",           // U+25A0
    step_error: "▲",            // U+25B2
    bar_start: "┌",             // U+250C
    bar: "│",                   // U+2502
    bar_end: "└",               // U+2514
    bar_h: "─",                 // U+2500
    radio_active: "●",          // U+25CF
    radio_inactive: "○",        // U+25CB
    checkbox_selected: "◼",     // U+25FC
    checkbox_empty: "◻",        // U+25FB
    info: "●",                  // U+25CF
    success: "◆",               // U+25C6
    warn: "▲",                  // U+25B2
    error: "■",                 // U+25A0
    corner_top_right: "╮",      // U+256E
    connect_left: "├",          // U+251C
    corner_bottom_right: "╯",   // U+256F
    cursor: "❯",                // U+276F
    expanded: "▾",              // U+25BE
    collapsed: "▸",             // U+25B8
    partial: "◐",               // U+25D0
    bullet: "•",                // U+2022
    ellipsis: "…",              // U+2026
    arrow_up: "↑",              // U+2191
    arrow_down: "↓",            // U+2193
};

/// `TERM=linux` fallback. The search-prompt glyphs below the spec table have
/// no specced ASCII forms; these stand-ins keep the layout aligned (all are
/// single-cell except `ellipsis`, whose callers pad by content anyway).
pub(crate) static ASCII: Symbols = Symbols {
    step_active: "*",
    step_submit: "o",
    step_cancel: "x",
    step_error: "x",
    bar_start: "T",
    bar: "|",
    bar_end: "—",
    bar_h: "-",
    radio_active: ">",
    radio_inactive: " ",
    checkbox_selected: "[+]",
    checkbox_empty: "[ ]",
    info: "•",
    success: "*",
    warn: "!",
    error: "x",
    corner_top_right: "+",
    connect_left: "+",
    corner_bottom_right: "+",
    cursor: ">",
    expanded: "v",
    collapsed: ">",
    partial: "o",
    bullet: "-",
    ellipsis: "...",
    arrow_up: "^",
    arrow_down: "v",
};

pub(crate) fn set(unicode: bool) -> &'static Symbols {
    if unicode {
        &UNICODE
    } else {
        &ASCII
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Assert every Unicode glyph by codepoint against the spec table, so a
    /// lookalike glyph (e.g. U+25C8 for U+25C6) cannot sneak in via
    /// copy-paste.
    #[test]
    fn unicode_glyphs_match_spec_codepoints() {
        let cp = |s: &str| s.chars().next().unwrap() as u32;
        assert_eq!(cp(UNICODE.step_active), 0x25C6);
        assert_eq!(cp(UNICODE.step_submit), 0x25C7);
        assert_eq!(cp(UNICODE.step_cancel), 0x25A0);
        assert_eq!(cp(UNICODE.step_error), 0x25B2);
        assert_eq!(cp(UNICODE.bar_start), 0x250C);
        assert_eq!(cp(UNICODE.bar), 0x2502);
        assert_eq!(cp(UNICODE.bar_end), 0x2514);
        assert_eq!(cp(UNICODE.bar_h), 0x2500);
        assert_eq!(cp(UNICODE.radio_active), 0x25CF);
        assert_eq!(cp(UNICODE.radio_inactive), 0x25CB);
        assert_eq!(cp(UNICODE.checkbox_selected), 0x25FC);
        assert_eq!(cp(UNICODE.checkbox_empty), 0x25FB);
        assert_eq!(cp(UNICODE.info), 0x25CF);
        assert_eq!(cp(UNICODE.success), 0x25C6);
        assert_eq!(cp(UNICODE.warn), 0x25B2);
        assert_eq!(cp(UNICODE.error), 0x25A0);
        assert_eq!(cp(UNICODE.corner_top_right), 0x256E);
        assert_eq!(cp(UNICODE.connect_left), 0x251C);
        assert_eq!(cp(UNICODE.corner_bottom_right), 0x256F);
        assert_eq!(cp(UNICODE.cursor), 0x276F);
        assert_eq!(cp(UNICODE.expanded), 0x25BE);
        assert_eq!(cp(UNICODE.collapsed), 0x25B8);
        assert_eq!(cp(UNICODE.partial), 0x25D0);
        assert_eq!(cp(UNICODE.bullet), 0x2022);
        assert_eq!(cp(UNICODE.ellipsis), 0x2026);
        assert_eq!(cp(UNICODE.arrow_up), 0x2191);
        assert_eq!(cp(UNICODE.arrow_down), 0x2193);
    }

    #[test]
    fn ascii_fallbacks_match_spec_table() {
        assert_eq!(ASCII.step_active, "*");
        assert_eq!(ASCII.step_submit, "o");
        assert_eq!(ASCII.step_cancel, "x");
        assert_eq!(ASCII.step_error, "x");
        assert_eq!(ASCII.bar_start, "T");
        assert_eq!(ASCII.bar, "|");
        assert_eq!(ASCII.bar_end, "—");
        assert_eq!(ASCII.bar_h, "-");
        assert_eq!(ASCII.radio_active, ">");
        assert_eq!(ASCII.radio_inactive, " ");
        assert_eq!(ASCII.checkbox_selected, "[+]");
        assert_eq!(ASCII.checkbox_empty, "[ ]");
        assert_eq!(ASCII.info, "•");
        assert_eq!(ASCII.warn, "!");
        assert_eq!(ASCII.error, "x");
        assert_eq!(ASCII.corner_top_right, "+");
        assert_eq!(ASCII.connect_left, "+");
        assert_eq!(ASCII.corner_bottom_right, "+");
    }

    #[test]
    fn set_selects_by_unicode_flag() {
        assert!(std::ptr::eq(set(true), &UNICODE));
        assert!(std::ptr::eq(set(false), &ASCII));
    }
}
