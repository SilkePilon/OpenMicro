#[derive(Clone, Copy, Debug)]
pub struct Style {
    enabled: bool,
}

impl Style {
    pub fn new(enabled: bool) -> Self {
        Style { enabled }
    }

    fn sgr(&self, open: &str, close: &str, s: &str) -> String {
        if self.enabled {
            format!("\x1b[{open}m{s}\x1b[{close}m")
        } else {
            s.to_string()
        }
    }

    pub fn bold(&self, s: &str) -> String {
        self.sgr("1", "22", s)
    }
    pub fn dim(&self, s: &str) -> String {
        self.sgr("2", "22", s)
    }
    pub fn underline(&self, s: &str) -> String {
        self.sgr("4", "24", s)
    }
    pub fn inverse(&self, s: &str) -> String {
        self.sgr("7", "27", s)
    }
    pub fn strikethrough(&self, s: &str) -> String {
        self.sgr("9", "29", s)
    }
    pub fn red(&self, s: &str) -> String {
        self.sgr("31", "39", s)
    }
    pub fn green(&self, s: &str) -> String {
        self.sgr("32", "39", s)
    }
    pub fn yellow(&self, s: &str) -> String {
        self.sgr("33", "39", s)
    }
    pub fn blue(&self, s: &str) -> String {
        self.sgr("34", "39", s)
    }
    pub fn magenta(&self, s: &str) -> String {
        self.sgr("35", "39", s)
    }
    pub fn cyan(&self, s: &str) -> String {
        self.sgr("36", "39", s)
    }
    pub fn gray(&self, s: &str) -> String {
        self.sgr("90", "39", s)
    }
    pub fn bg_cyan(&self, s: &str) -> String {
        self.sgr("46", "49", s)
    }

    pub fn fg256(&self, n: u8, s: &str) -> String {
        self.sgr(&format!("38;5;{n}"), "39", s)
    }
}

pub(crate) const BANNER_GRADIENT: [u8; 6] = [250, 248, 245, 243, 240, 238];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sgr_codes_match_spec_table() {
        let s = Style::new(true);
        assert_eq!(s.bold("x"), "\x1b[1mx\x1b[22m");
        assert_eq!(s.dim("x"), "\x1b[2mx\x1b[22m");
        assert_eq!(s.underline("x"), "\x1b[4mx\x1b[24m");
        assert_eq!(s.inverse("x"), "\x1b[7mx\x1b[27m");
        assert_eq!(s.strikethrough("x"), "\x1b[9mx\x1b[29m");
        assert_eq!(s.red("x"), "\x1b[31mx\x1b[39m");
        assert_eq!(s.green("x"), "\x1b[32mx\x1b[39m");
        assert_eq!(s.yellow("x"), "\x1b[33mx\x1b[39m");
        assert_eq!(s.blue("x"), "\x1b[34mx\x1b[39m");
        assert_eq!(s.magenta("x"), "\x1b[35mx\x1b[39m");
        assert_eq!(s.cyan("x"), "\x1b[36mx\x1b[39m");
        assert_eq!(s.gray("x"), "\x1b[90mx\x1b[39m");
        assert_eq!(s.bg_cyan("x"), "\x1b[46mx\x1b[49m");
        assert_eq!(s.fg256(250, "x"), "\x1b[38;5;250mx\x1b[39m");
    }

    #[test]
    fn disabled_style_is_a_passthrough() {
        let s = Style::new(false);
        assert_eq!(s.cyan("plain"), "plain");
        assert_eq!(s.inverse(" "), " ");
        assert_eq!(s.fg256(250, "row"), "row");
    }

    #[test]
    fn banner_gradient_is_light_to_dark() {
        assert_eq!(BANNER_GRADIENT, [250, 248, 245, 243, 240, 238]);
    }
}
