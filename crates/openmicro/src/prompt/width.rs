//! Display-width arithmetic for the prompt renderer.
//!
//! The redraw strategy (`ESC[{n}A` + `ESC[J`) only works if `n` is the number
//! of *physical* rows the previous frame occupied. That means counting the
//! rendered width of every line — after stripping ANSI, and treating CJK and
//! emoji as double-width — and dividing by the terminal's column count.
//! Under-counting leaves ghost rows behind on narrow terminals; over-counting
//! eats lines of the transcript above the prompt. `docs/tui-style.md`
//! ("Rendering model") is explicit that this must be wrapped rows, not
//! logical lines.
//!
//! We hand-roll the width table rather than pulling in `unicode-width`: the
//! prompt only needs "is this one of the East Asian Wide/Fullwidth or emoji
//! blocks", and a small range table keeps the crate's dependency set flat.

/// Character ranges rendered double-width by terminals (East Asian
/// Wide/Fullwidth plus the emoji blocks terminals actually double-cell).
/// Condensed from Unicode `EastAsianWidth.txt`; deliberately coarse — an
/// occasional narrow codepoint inside a range costs one blank column, while a
/// missed wide codepoint corrupts the redraw, so we err towards wide.
const WIDE: &[(u32, u32)] = &[
    (0x1100, 0x115F),   // Hangul Jamo leading consonants
    (0x2329, 0x232A),   // angle brackets
    (0x2E80, 0x303E),   // CJK radicals .. CJK symbols & punctuation
    (0x3041, 0x33FF),   // Hiragana .. CJK compatibility
    (0x3400, 0x4DBF),   // CJK extension A
    (0x4E00, 0x9FFF),   // CJK unified ideographs
    (0xA000, 0xA4CF),   // Yi
    (0xA960, 0xA97F),   // Hangul Jamo extended-A
    (0xAC00, 0xD7A3),   // Hangul syllables
    (0xF900, 0xFAFF),   // CJK compatibility ideographs
    (0xFE10, 0xFE19),   // vertical forms
    (0xFE30, 0xFE6F),   // CJK compatibility forms, small forms
    (0xFF00, 0xFF60),   // fullwidth forms
    (0xFFE0, 0xFFE6),   // fullwidth signs
    (0x1B000, 0x1B001), // Kana supplement
    (0x1F004, 0x1F004), // mahjong red dragon
    (0x1F0CF, 0x1F0CF), // playing card joker
    (0x1F18E, 0x1F18E), // AB button
    (0x1F191, 0x1F19A), // squared CL .. squared VS
    (0x1F200, 0x1F2FF), // enclosed ideographic supplement
    (0x1F300, 0x1F64F), // misc pictographs, emoticons
    (0x1F680, 0x1F6FF), // transport & map symbols
    (0x1F900, 0x1F9FF), // supplemental symbols & pictographs
    (0x1FA70, 0x1FAFF), // symbols & pictographs extended-A
    (0x20000, 0x2FFFD), // CJK extension B..F
    (0x30000, 0x3FFFD), // CJK extension G
];

/// Zero-width ranges: combining marks, variation selectors, zero-width
/// spaces/joiners. These attach to the previous cell instead of taking one.
const ZERO: &[(u32, u32)] = &[
    (0x0300, 0x036F), // combining diacritical marks
    (0x1AB0, 0x1AFF), // combining diacritical marks extended
    (0x1DC0, 0x1DFF), // combining diacritical marks supplement
    (0x200B, 0x200F), // zero-width space/joiners, directional marks
    (0x2060, 0x2060), // word joiner
    (0x20D0, 0x20FF), // combining marks for symbols
    (0xFE00, 0xFE0F), // variation selectors
    (0xFE20, 0xFE2F), // combining half marks
    (0xFEFF, 0xFEFF), // BOM / zero-width no-break space
];

fn in_ranges(c: u32, ranges: &[(u32, u32)]) -> bool {
    ranges
        .binary_search_by(|&(lo, hi)| {
            if c < lo {
                std::cmp::Ordering::Greater
            } else if c > hi {
                std::cmp::Ordering::Less
            } else {
                std::cmp::Ordering::Equal
            }
        })
        .is_ok()
}

/// Rendered cell width of a single character: 0, 1, or 2.
pub(crate) fn char_width(c: char) -> usize {
    let u = c as u32;
    if u < 0x20 || (0x7F..0xA0).contains(&u) {
        // Control characters never reach the terminal from our renderer
        // (ANSI is stripped before measuring); count them as zero so a stray
        // one cannot inflate the row estimate.
        0
    } else if in_ranges(u, ZERO) {
        0
    } else if in_ranges(u, WIDE) {
        2
    } else {
        1
    }
}

/// Remove ANSI escape sequences so styled lines measure like their visible
/// text. Handles CSI (`ESC [ ... final`) — the only kind this module emits —
/// plus a defensive skip of any other two-byte escape.
pub(crate) fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            out.push(c);
            continue;
        }
        match chars.peek() {
            Some('[') => {
                chars.next();
                // CSI: parameter/intermediate bytes until a final byte in
                // 0x40..=0x7E terminates the sequence.
                for f in chars.by_ref() {
                    if ('\u{40}'..='\u{7e}').contains(&f) {
                        break;
                    }
                }
            }
            Some(_) => {
                chars.next();
            }
            None => {}
        }
    }
    out
}

/// Rendered width of a (possibly styled) line in terminal cells.
pub(crate) fn display_width(s: &str) -> usize {
    strip_ansi(s).chars().map(char_width).sum()
}

/// Physical rows one logical line occupies at the given terminal width.
/// An empty line still occupies one row — that is what the `\r\n` we print
/// after it produces on screen.
pub(crate) fn wrapped_rows(line: &str, columns: usize) -> usize {
    if columns == 0 {
        return 1;
    }
    let w = display_width(line);
    if w == 0 {
        1
    } else {
        w.div_ceil(columns)
    }
}

/// Total physical rows of a whole frame — the `n` fed to `ESC[{n}A`.
pub(crate) fn frame_rows(lines: &[String], columns: usize) -> usize {
    lines.iter().map(|l| wrapped_rows(l, columns)).sum()
}

/// Greedy word wrap. Words longer than the width are hard-cut mid-word (the
/// spec calls this out for the detail pane: "Greedy word wrap, hard-cut for
/// over-long words"). Widths are display cells, so CJK text wraps correctly.
pub(crate) fn wrap(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut out = Vec::new();
    for raw in text.split('\n') {
        let mut line = String::new();
        let mut line_w = 0usize;
        let mut any = false;
        for word in raw.split_whitespace() {
            any = true;
            let ww = display_width(word);
            if line_w > 0 && line_w + 1 + ww <= width {
                line.push(' ');
                line.push_str(word);
                line_w += 1 + ww;
                continue;
            }
            if line_w > 0 {
                out.push(std::mem::take(&mut line));
                line_w = 0;
            }
            if ww <= width {
                line.push_str(word);
                line_w = ww;
            } else {
                // Hard-cut: emit full-width chunks, keep the tail as the
                // current line so following words continue after it.
                for c in word.chars() {
                    let cw = char_width(c);
                    if line_w + cw > width {
                        out.push(std::mem::take(&mut line));
                        line_w = 0;
                    }
                    line.push(c);
                    line_w += cw;
                }
            }
        }
        if line_w > 0 || !any {
            out.push(line);
        }
    }
    out
}

/// Wrap and clip to `max_lines`, appending `ellipsis` to the last line when
/// text was cut off. Used by the fixed-height detail pane, which must never
/// grow the frame as the cursor moves.
pub(crate) fn wrap_clipped(
    text: &str,
    width: usize,
    max_lines: usize,
    ellipsis: &str,
) -> Vec<String> {
    let lines = wrap(text, width);
    if lines.len() <= max_lines {
        return lines;
    }
    let mut out = lines[..max_lines].to_vec();
    if let Some(last) = out.last_mut() {
        let ell_w = display_width(ellipsis);
        // Make room for the ellipsis without exceeding the pane width.
        while !last.is_empty() && display_width(last) + ell_w > width {
            last.pop();
        }
        last.push_str(ellipsis);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ascii_chars_are_single_width() {
        assert_eq!(display_width("hello"), 5);
    }

    #[test]
    fn cjk_chars_are_double_width() {
        // "你好" is two ideographs → four cells.
        assert_eq!(display_width("你好"), 4);
        assert_eq!(display_width("カタカナ"), 8);
    }

    #[test]
    fn emoji_are_double_width() {
        assert_eq!(display_width("🔥"), 2);
        assert_eq!(display_width("a🚀b"), 4);
    }

    #[test]
    fn combining_marks_are_zero_width() {
        // "e" + combining acute renders as one cell.
        assert_eq!(display_width("e\u{0301}"), 1);
    }

    #[test]
    fn strip_ansi_removes_sgr_sequences() {
        assert_eq!(strip_ansi("\x1b[36m◆\x1b[39m  hi"), "◆  hi");
        assert_eq!(strip_ansi("\x1b[38;5;250mrow\x1b[39m"), "row");
    }

    #[test]
    fn display_width_ignores_ansi() {
        assert_eq!(display_width("\x1b[2mdim\x1b[22m"), 3);
    }

    #[test]
    fn line_exactly_terminal_width_is_one_row() {
        let line = "x".repeat(80);
        assert_eq!(wrapped_rows(&line, 80), 1);
    }

    #[test]
    fn line_one_past_terminal_width_is_two_rows() {
        let line = "x".repeat(81);
        assert_eq!(wrapped_rows(&line, 80), 2);
    }

    #[test]
    fn empty_line_is_one_row() {
        assert_eq!(wrapped_rows("", 80), 1);
    }

    #[test]
    fn wide_chars_wrap_by_cells_not_chars() {
        // 41 ideographs = 82 cells: three rows at 40 columns even though the
        // char count (41) alone would fit in two.
        let line = "宽".repeat(41);
        assert_eq!(wrapped_rows(&line, 40), 3);
    }

    #[test]
    fn frame_rows_sums_wrapped_rows() {
        let lines = vec!["short".to_string(), "y".repeat(85), String::new()];
        assert_eq!(frame_rows(&lines, 80), 1 + 2 + 1);
    }

    #[test]
    fn wrap_is_greedy_and_hard_cuts_long_words() {
        assert_eq!(wrap("one two three", 8), vec!["one two", "three"]);
        // A 12-char word at width 5 is cut into full-width chunks.
        assert_eq!(wrap("abcdefghijkl", 5), vec!["abcde", "fghij", "kl"]);
    }

    #[test]
    fn wrap_preserves_blank_lines() {
        assert_eq!(wrap("a\n\nb", 10), vec!["a", "", "b"]);
    }

    #[test]
    fn wrap_clipped_appends_ellipsis_when_text_remains() {
        let lines = wrap_clipped("one two three four five six", 9, 2, "…");
        assert_eq!(lines.len(), 2);
        assert!(lines[1].ends_with('…'), "clipped line was {:?}", lines[1]);
        assert!(display_width(&lines[1]) <= 9);
    }

    #[test]
    fn wrap_clipped_leaves_short_text_alone() {
        assert_eq!(wrap_clipped("hi", 10, 3, "…"), vec!["hi"]);
    }
}
