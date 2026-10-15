use ratatui::style::Color;

/// How many colours the terminal can show. Detected once at startup from the
/// environment (`NO_COLOR`, `COLORTERM`, `TERM`, …) and used to down-sample
/// the RGB palettes below so limited terminals still get a readable UI.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColorMode {
    /// 24-bit colour.
    TrueColor,
    /// xterm 256-colour palette.
    Ansi256,
    /// The 16 basic ANSI colours.
    Ansi16,
    /// No colour at all (`NO_COLOR` / `--no-color` / dumb terminals). Only
    /// bold / italic / underline / reverse are used.
    None,
}

impl ColorMode {
    /// Detect from the process environment. `force_none` comes from
    /// `--no-color`.
    pub fn detect(force_none: bool) -> Self {
        let get = |k: &str| std::env::var(k).ok();
        Self::from_env(
            force_none,
            get("NO_COLOR"),
            get("COLORTERM"),
            get("TERM"),
            get("TERM_PROGRAM"),
            get("WT_SESSION"),
            cfg!(windows),
        )
    }

    /// Pure detection logic (separated for testing).
    pub fn from_env(
        force_none: bool,
        no_color: Option<String>,
        colorterm: Option<String>,
        term: Option<String>,
        term_program: Option<String>,
        wt_session: Option<String>,
        is_windows: bool,
    ) -> Self {
        // https://no-color.org: any non-empty value disables colour.
        if force_none || no_color.is_some_and(|v| !v.is_empty()) {
            return Self::None;
        }
        let term = term.unwrap_or_default().to_lowercase();
        if term == "dumb" {
            return Self::None;
        }
        let colorterm = colorterm.unwrap_or_default().to_lowercase();
        if colorterm == "truecolor" || colorterm == "24bit" || wt_session.is_some() {
            return Self::TrueColor;
        }
        if term_program.as_deref() == Some("Apple_Terminal") {
            return Self::Ansi256;
        }
        if term.contains("256") {
            return Self::Ansi256;
        }
        if is_windows && term.is_empty() {
            // Windows 10+ console host and Windows Terminal both do 24-bit.
            return Self::TrueColor;
        }
        if term.is_empty() || term == "linux" || term.starts_with("vt") {
            return Self::Ansi16;
        }
        // xterm / screen / tmux without a 256 suffix: assume the common case.
        Self::Ansi256
    }

    /// Map an RGB colour into this mode's palette.
    pub fn map(self, c: Color) -> Color {
        match (self, c) {
            (Self::None, _) => Color::Reset,
            (Self::TrueColor, c) => c,
            (Self::Ansi256, Color::Rgb(r, g, b)) => Color::Indexed(rgb_to_ansi256(r, g, b)),
            (Self::Ansi16, Color::Rgb(r, g, b)) => rgb_to_ansi16(r, g, b),
            (_, c) => c,
        }
    }

    pub fn has_color(self) -> bool {
        self != Self::None
    }
}

/// Nearest xterm-256 index (6x6x6 cube or the 24-step grey ramp).
pub fn rgb_to_ansi256(r: u8, g: u8, b: u8) -> u8 {
    const STEPS: [u8; 6] = [0, 95, 135, 175, 215, 255];
    let nearest_step = |v: u8| -> usize {
        STEPS
            .iter()
            .enumerate()
            .min_by_key(|(_, s)| (i32::from(**s) - i32::from(v)).abs())
            .map(|(i, _)| i)
            .unwrap_or(0)
    };
    let (ri, gi, bi) = (nearest_step(r), nearest_step(g), nearest_step(b));
    let cube = (STEPS[ri], STEPS[gi], STEPS[bi]);
    let cube_idx = 16 + 36 * ri as u8 + 6 * gi as u8 + bi as u8;

    let avg = (u16::from(r) + u16::from(g) + u16::from(b)) / 3;
    let grey_i = if avg < 8 {
        0
    } else {
        ((avg - 8) / 10).min(23) as u8
    };
    let grey_v = 8 + 10 * grey_i;
    let grey_idx = 232 + grey_i;

    let dist = |(x, y, z): (u8, u8, u8)| -> i32 {
        let d = |p: u8, q: u8| (i32::from(p) - i32::from(q)).pow(2);
        d(x, r) + d(y, g) + d(z, b)
    };
    if dist((grey_v, grey_v, grey_v)) < dist(cube) {
        grey_idx
    } else {
        cube_idx
    }
}

/// Nearest of the 16 basic ANSI colours (using typical xterm values).
pub fn rgb_to_ansi16(r: u8, g: u8, b: u8) -> Color {
    const PALETTE: [(Color, (u8, u8, u8)); 16] = [
        (Color::Black, (0, 0, 0)),
        (Color::Red, (205, 0, 0)),
        (Color::Green, (0, 205, 0)),
        (Color::Yellow, (205, 205, 0)),
        (Color::Blue, (0, 0, 238)),
        (Color::Magenta, (205, 0, 205)),
        (Color::Cyan, (0, 205, 205)),
        (Color::Gray, (229, 229, 229)),
        (Color::DarkGray, (127, 127, 127)),
        (Color::LightRed, (255, 0, 0)),
        (Color::LightGreen, (0, 255, 0)),
        (Color::LightYellow, (255, 255, 0)),
        (Color::LightBlue, (92, 92, 255)),
        (Color::LightMagenta, (255, 0, 255)),
        (Color::LightCyan, (0, 255, 255)),
        (Color::White, (255, 255, 255)),
    ];
    let d = |x: u8, y: u8| (i32::from(x) - i32::from(y)).pow(2);
    PALETTE
        .iter()
        .min_by_key(|(_, (pr, pg, pb))| d(*pr, r) + d(*pg, g) + d(*pb, b))
        .map(|(c, _)| *c)
        .unwrap_or(Color::Reset)
}

#[derive(Clone)]
pub struct Theme {
    pub name: &'static str,
    /// Colour capability this theme has been adapted to.
    pub mode: ColorMode,
    // Base (used by consumers for background fill)
    #[allow(dead_code)]
    pub bg: Color,
    pub fg: Color,
    pub accent: Color,
    pub muted: Color,
    // Semantic
    pub success: Color,
    pub error: Color,
    pub warning: Color,
    // Roles
    pub user_color: Color,
    pub assistant_color: Color,
    pub system_color: Color,
    pub tool_color: Color,
    // Structure
    pub border: Color,
    pub border_focused: Color,
    pub surface: Color,
    pub header_bg: Color,
    pub header_fg: Color,
    // Code
    pub code_fg: Color,
    pub code_bg: Color,
    // Input
    pub input_active: Color,
    // Dim variant for subtle elements
    pub dim: Color,
}

impl Theme {
    pub fn dark() -> Self {
        Self {
            mode: ColorMode::TrueColor,
            name: "dark",
            bg: Color::Rgb(24, 24, 28),
            fg: Color::Rgb(220, 220, 220),
            accent: Color::Rgb(122, 162, 247),
            muted: Color::Rgb(90, 90, 100),
            success: Color::Rgb(158, 206, 106),
            error: Color::Rgb(247, 118, 142),
            warning: Color::Rgb(224, 175, 104),
            user_color: Color::Rgb(122, 162, 247),
            assistant_color: Color::Rgb(158, 206, 106),
            system_color: Color::Rgb(140, 140, 155),
            tool_color: Color::Rgb(90, 90, 100),
            border: Color::Rgb(50, 50, 58),
            border_focused: Color::Rgb(80, 80, 100),
            surface: Color::Rgb(32, 32, 38),
            header_bg: Color::Rgb(122, 162, 247),
            header_fg: Color::Rgb(20, 20, 28),
            code_fg: Color::Rgb(180, 210, 180),
            code_bg: Color::Rgb(20, 20, 24),
            input_active: Color::Rgb(122, 162, 247),
            dim: Color::Rgb(60, 60, 68),
        }
    }

    pub fn tokyo_night() -> Self {
        Self {
            mode: ColorMode::TrueColor,
            name: "tokyo-night",
            bg: Color::Rgb(26, 27, 38),
            fg: Color::Rgb(169, 177, 214),
            accent: Color::Rgb(122, 162, 247),
            muted: Color::Rgb(86, 95, 137),
            success: Color::Rgb(158, 206, 106),
            error: Color::Rgb(247, 118, 142),
            warning: Color::Rgb(224, 175, 104),
            user_color: Color::Rgb(122, 162, 247),
            assistant_color: Color::Rgb(158, 206, 106),
            system_color: Color::Rgb(125, 133, 170),
            tool_color: Color::Rgb(86, 95, 137),
            border: Color::Rgb(41, 43, 60),
            border_focused: Color::Rgb(65, 70, 100),
            surface: Color::Rgb(30, 31, 44),
            header_bg: Color::Rgb(122, 162, 247),
            header_fg: Color::Rgb(22, 23, 34),
            code_fg: Color::Rgb(158, 206, 106),
            code_bg: Color::Rgb(22, 23, 32),
            input_active: Color::Rgb(122, 162, 247),
            dim: Color::Rgb(52, 53, 74),
        }
    }

    pub fn dracula() -> Self {
        Self {
            mode: ColorMode::TrueColor,
            name: "dracula",
            bg: Color::Rgb(40, 42, 54),
            fg: Color::Rgb(248, 248, 242),
            accent: Color::Rgb(189, 147, 249),
            muted: Color::Rgb(98, 114, 164),
            success: Color::Rgb(80, 250, 123),
            error: Color::Rgb(255, 85, 85),
            warning: Color::Rgb(241, 250, 140),
            user_color: Color::Rgb(139, 233, 253),
            assistant_color: Color::Rgb(80, 250, 123),
            system_color: Color::Rgb(140, 155, 190),
            tool_color: Color::Rgb(98, 114, 164),
            border: Color::Rgb(55, 58, 74),
            border_focused: Color::Rgb(80, 85, 110),
            surface: Color::Rgb(44, 46, 60),
            header_bg: Color::Rgb(189, 147, 249),
            header_fg: Color::Rgb(40, 42, 54),
            code_fg: Color::Rgb(80, 250, 123),
            code_bg: Color::Rgb(34, 36, 48),
            input_active: Color::Rgb(255, 121, 198),
            dim: Color::Rgb(68, 71, 90),
        }
    }

    pub fn catppuccin_mocha() -> Self {
        Self {
            mode: ColorMode::TrueColor,
            name: "catppuccin",
            bg: Color::Rgb(30, 30, 46),
            fg: Color::Rgb(205, 214, 244),
            accent: Color::Rgb(137, 180, 250),
            muted: Color::Rgb(108, 112, 134),
            success: Color::Rgb(166, 227, 161),
            error: Color::Rgb(243, 139, 168),
            warning: Color::Rgb(249, 226, 175),
            user_color: Color::Rgb(137, 180, 250),
            assistant_color: Color::Rgb(166, 227, 161),
            system_color: Color::Rgb(147, 153, 178),
            tool_color: Color::Rgb(108, 112, 134),
            border: Color::Rgb(49, 50, 68),
            border_focused: Color::Rgb(74, 76, 100),
            surface: Color::Rgb(35, 35, 52),
            header_bg: Color::Rgb(137, 180, 250),
            header_fg: Color::Rgb(30, 30, 46),
            code_fg: Color::Rgb(166, 227, 161),
            code_bg: Color::Rgb(24, 24, 38),
            input_active: Color::Rgb(203, 166, 247),
            dim: Color::Rgb(69, 71, 90),
        }
    }

    pub fn gruvbox() -> Self {
        Self {
            mode: ColorMode::TrueColor,
            name: "gruvbox",
            bg: Color::Rgb(40, 40, 40),
            fg: Color::Rgb(235, 219, 178),
            accent: Color::Rgb(131, 165, 152),
            muted: Color::Rgb(124, 111, 100),
            success: Color::Rgb(184, 187, 38),
            error: Color::Rgb(251, 73, 52),
            warning: Color::Rgb(250, 189, 47),
            user_color: Color::Rgb(131, 165, 152),
            assistant_color: Color::Rgb(184, 187, 38),
            system_color: Color::Rgb(168, 153, 132),
            tool_color: Color::Rgb(124, 111, 100),
            border: Color::Rgb(60, 56, 54),
            border_focused: Color::Rgb(80, 73, 69),
            surface: Color::Rgb(50, 48, 47),
            header_bg: Color::Rgb(250, 189, 47),
            header_fg: Color::Rgb(40, 40, 40),
            code_fg: Color::Rgb(184, 187, 38),
            code_bg: Color::Rgb(32, 32, 32),
            input_active: Color::Rgb(131, 165, 152),
            dim: Color::Rgb(80, 73, 69),
        }
    }

    pub fn nord() -> Self {
        Self {
            mode: ColorMode::TrueColor,
            name: "nord",
            bg: Color::Rgb(46, 52, 64),
            fg: Color::Rgb(216, 222, 233),
            accent: Color::Rgb(136, 192, 208),
            muted: Color::Rgb(76, 86, 106),
            success: Color::Rgb(163, 190, 140),
            error: Color::Rgb(191, 97, 106),
            warning: Color::Rgb(235, 203, 139),
            user_color: Color::Rgb(136, 192, 208),
            assistant_color: Color::Rgb(163, 190, 140),
            system_color: Color::Rgb(129, 140, 160),
            tool_color: Color::Rgb(76, 86, 106),
            border: Color::Rgb(59, 66, 82),
            border_focused: Color::Rgb(76, 86, 106),
            surface: Color::Rgb(52, 58, 72),
            header_bg: Color::Rgb(136, 192, 208),
            header_fg: Color::Rgb(46, 52, 64),
            code_fg: Color::Rgb(163, 190, 140),
            code_bg: Color::Rgb(39, 44, 56),
            input_active: Color::Rgb(136, 192, 208),
            dim: Color::Rgb(67, 76, 94),
        }
    }

    pub fn one_dark() -> Self {
        Self {
            mode: ColorMode::TrueColor,
            name: "one-dark",
            bg: Color::Rgb(40, 44, 52),
            fg: Color::Rgb(171, 178, 191),
            accent: Color::Rgb(97, 175, 239),
            muted: Color::Rgb(92, 99, 112),
            success: Color::Rgb(152, 195, 121),
            error: Color::Rgb(224, 108, 117),
            warning: Color::Rgb(229, 192, 123),
            user_color: Color::Rgb(97, 175, 239),
            assistant_color: Color::Rgb(152, 195, 121),
            system_color: Color::Rgb(130, 137, 150),
            tool_color: Color::Rgb(92, 99, 112),
            border: Color::Rgb(50, 54, 62),
            border_focused: Color::Rgb(70, 76, 88),
            surface: Color::Rgb(44, 48, 56),
            header_bg: Color::Rgb(97, 175, 239),
            header_fg: Color::Rgb(40, 44, 52),
            code_fg: Color::Rgb(152, 195, 121),
            code_bg: Color::Rgb(34, 38, 46),
            input_active: Color::Rgb(97, 175, 239),
            dim: Color::Rgb(60, 63, 70),
        }
    }

    pub fn by_name(name: &str) -> Self {
        match name.to_lowercase().as_str() {
            "tokyo-night" | "tokyo_night" => Self::tokyo_night(),
            "dracula" => Self::dracula(),
            "catppuccin" | "catppuccin-mocha" => Self::catppuccin_mocha(),
            "gruvbox" => Self::gruvbox(),
            "nord" => Self::nord(),
            "one-dark" | "onedark" | "one_dark" => Self::one_dark(),
            _ => Self::dark(),
        }
    }

    /// Down-sample every colour to what the terminal supports.
    pub fn with_mode(mut self, mode: ColorMode) -> Self {
        let m = |c: Color| mode.map(c);
        self.mode = mode;
        self.bg = m(self.bg);
        self.fg = m(self.fg);
        self.accent = m(self.accent);
        self.muted = m(self.muted);
        self.success = m(self.success);
        self.error = m(self.error);
        self.warning = m(self.warning);
        self.user_color = m(self.user_color);
        self.assistant_color = m(self.assistant_color);
        self.system_color = m(self.system_color);
        self.tool_color = m(self.tool_color);
        self.border = m(self.border);
        self.border_focused = m(self.border_focused);
        self.surface = m(self.surface);
        self.header_bg = m(self.header_bg);
        self.header_fg = m(self.header_fg);
        self.code_fg = m(self.code_fg);
        self.code_bg = m(self.code_bg);
        self.input_active = m(self.input_active);
        self.dim = m(self.dim);
        self
    }

    /// Map an arbitrary RGB colour (e.g. from syntax highlighting) into this
    /// theme's colour mode.
    pub fn map_color(&self, c: Color) -> Color {
        self.mode.map(c)
    }

    /// Whether `name` is a known theme (case-insensitive, with aliases).
    pub fn exists(name: &str) -> bool {
        matches!(
            name.to_lowercase().as_str(),
            "dark"
                | "tokyo-night"
                | "tokyo_night"
                | "dracula"
                | "catppuccin"
                | "catppuccin-mocha"
                | "gruvbox"
                | "nord"
                | "one-dark"
                | "onedark"
                | "one_dark"
        )
    }

    pub fn all_names() -> &'static [&'static str] {
        &[
            "dark",
            "tokyo-night",
            "dracula",
            "catppuccin",
            "gruvbox",
            "nord",
            "one-dark",
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn detect(no_color: Option<&str>, colorterm: Option<&str>, term: Option<&str>) -> ColorMode {
        ColorMode::from_env(
            false,
            no_color.map(String::from),
            colorterm.map(String::from),
            term.map(String::from),
            None,
            None,
            false,
        )
    }

    #[test]
    fn no_color_wins() {
        assert_eq!(detect(Some("1"), Some("truecolor"), None), ColorMode::None);
        // Empty NO_COLOR does not disable colour (per no-color.org).
        assert_eq!(
            detect(Some(""), Some("truecolor"), None),
            ColorMode::TrueColor
        );
        assert_eq!(
            ColorMode::from_env(
                true,
                None,
                Some("truecolor".into()),
                None,
                None,
                None,
                false
            ),
            ColorMode::None
        );
    }

    #[test]
    fn detects_capabilities() {
        assert_eq!(
            detect(None, Some("24bit"), Some("xterm")),
            ColorMode::TrueColor
        );
        assert_eq!(
            detect(None, None, Some("xterm-256color")),
            ColorMode::Ansi256
        );
        assert_eq!(detect(None, None, Some("linux")), ColorMode::Ansi16);
        assert_eq!(detect(None, None, Some("dumb")), ColorMode::None);
        assert_eq!(
            ColorMode::from_env(false, None, None, None, None, None, true),
            ColorMode::TrueColor
        );
    }

    #[test]
    fn ansi256_mapping() {
        assert_eq!(rgb_to_ansi256(0, 0, 0), 16);
        assert_eq!(rgb_to_ansi256(255, 255, 255), 231);
        assert_eq!(rgb_to_ansi256(255, 0, 0), 196);
        // Mid grey lands on the grey ramp.
        assert!((232..=255).contains(&rgb_to_ansi256(128, 128, 128)));
    }

    #[test]
    fn ansi16_mapping() {
        assert_eq!(rgb_to_ansi16(250, 10, 10), Color::LightRed);
        assert_eq!(rgb_to_ansi16(10, 10, 10), Color::Black);
    }

    #[test]
    fn with_mode_maps_all_colours() {
        let t = Theme::dark().with_mode(ColorMode::None);
        assert_eq!(t.fg, Color::Reset);
        assert_eq!(t.accent, Color::Reset);
        let t = Theme::dark().with_mode(ColorMode::Ansi256);
        assert!(matches!(t.accent, Color::Indexed(_)));
    }

    #[test]
    fn theme_lookup() {
        assert!(Theme::exists("Tokyo-Night"));
        assert!(!Theme::exists("nope"));
        assert_eq!(Theme::by_name("nord").name, "nord");
    }
}
