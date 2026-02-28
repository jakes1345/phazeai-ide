use floem::peniko::Color;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThemeVariant {
    // Cosmic / PhazeAI originals
    MidnightBlue,
    Cyberpunk,
    Synthwave84,
    Andromeda,
    // Classic dark
    Dark,
    Dracula,
    TokyoNight,
    Monokai,
    NordDark,
    // Hacker
    MatrixGreen,
    RootShell,
    // Light
    Light,
}

impl Default for ThemeVariant {
    fn default() -> Self {
        Self::MidnightBlue
    }
}

impl ThemeVariant {
    pub fn all() -> &'static [ThemeVariant] {
        &[
            ThemeVariant::MidnightBlue,
            ThemeVariant::Cyberpunk,
            ThemeVariant::Synthwave84,
            ThemeVariant::Andromeda,
            ThemeVariant::Dark,
            ThemeVariant::Dracula,
            ThemeVariant::TokyoNight,
            ThemeVariant::Monokai,
            ThemeVariant::NordDark,
            ThemeVariant::MatrixGreen,
            ThemeVariant::RootShell,
            ThemeVariant::Light,
        ]
    }

    pub fn from_str(s: &str) -> Self {
        match s.to_lowercase().replace([' ', '-', '_'], "").as_str() {
            "midnightblue" | "midnight"          => Self::MidnightBlue,
            "cyberpunk" | "cyber"                => Self::Cyberpunk,
            "synthwave84" | "synthwave"          => Self::Synthwave84,
            "andromeda"                          => Self::Andromeda,
            "dracula"                            => Self::Dracula,
            "tokyonight" | "tokyo"               => Self::TokyoNight,
            "monokai"                            => Self::Monokai,
            "norddark" | "nord"                  => Self::NordDark,
            "matrixgreen" | "matrix"             => Self::MatrixGreen,
            "rootshell" | "root"                 => Self::RootShell,
            "light"                              => Self::Light,
            _                                    => Self::Dark,
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            Self::MidnightBlue => "Midnight Blue",
            Self::Cyberpunk    => "Cyberpunk 2077",
            Self::Synthwave84  => "Synthwave '84",
            Self::Andromeda    => "Andromeda",
            Self::Dark         => "Dark",
            Self::Dracula      => "Dracula",
            Self::TokyoNight   => "Tokyo Night",
            Self::Monokai      => "Monokai",
            Self::NordDark     => "Nord Dark",
            Self::MatrixGreen  => "Matrix Green",
            Self::RootShell    => "Root Shell",
            Self::Light        => "Light",
        }
    }
}

/// Raw brand palette — all literal color values live here.
#[derive(Debug, Clone)]
pub struct PhazePalette {
    // Backgrounds
    pub bg_deep: Color,
    pub bg_base: Color,
    pub bg_surface: Color,
    pub bg_panel: Color,
    pub bg_elevated: Color,

    // Text
    pub text_primary: Color,
    pub text_secondary: Color,
    pub text_muted: Color,
    pub text_disabled: Color,

    // Accent
    pub accent: Color,
    pub accent_hover: Color,
    pub accent_dim: Color,

    // Semantic
    pub success: Color,
    pub warning: Color,
    pub error: Color,

    // Borders
    pub border: Color,
    pub border_focus: Color,

    // Selection
    pub selection: Color,

    // Syntax
    pub syn_keyword: Color,
    pub syn_string: Color,
    pub syn_comment: Color,
    pub syn_function: Color,
    pub syn_number: Color,
    pub syn_type: Color,
    pub syn_operator: Color,
    pub syn_macro: Color,

    // Glass effect helpers
    pub glass_bg: Color,
    pub glass_border: Color,
    /// Glow color for box-shadow on active/focused panels
    pub glow: Color,
}

impl PhazePalette {
    /// Cosmic glassmorphism palette — the hero theme.
    /// Electric purple accent on a deep space background.
    pub fn midnight_blue() -> Self {
        Self {
            // Deep space — all backgrounds semi-transparent so the cosmic
            // canvas behind shows through the glass panels.
            bg_deep:        Color::from_rgba8(5,  3,  16,  235), // #050310 deep purple-black
            bg_base:        Color::from_rgba8(0,  0,  0,   0),   // Truly transparent base
            bg_surface:     Color::from_rgba8(13, 11, 30,  185),
            bg_panel:       Color::from_rgba8(10, 9,  22,  145),
            bg_elevated:    Color::from_rgba8(21, 18, 40,  215),

            text_primary:   Color::from_rgb8(215, 220, 255),
            text_secondary: Color::from_rgb8(140, 160, 235),
            text_muted:     Color::from_rgb8(85,  95,  150),
            text_disabled:  Color::from_rgb8(40,  45,  75),

            // Soft blue-white accent — matches screenshot deep space aesthetic
            accent:         Color::from_rgb8(123, 159, 255),
            accent_hover:   Color::from_rgb8(160, 184, 255),
            accent_dim:     Color::from_rgba8(123, 159, 255, 60),

            success:        Color::from_rgb8(72,  230, 150),
            warning:        Color::from_rgb8(255, 200, 60),
            error:          Color::from_rgb8(255, 80,  100),

            border:         Color::from_rgba8(80,  60,  160, 100),
            border_focus:   Color::from_rgba8(120, 90,  255, 200),

            selection:      Color::from_rgba8(80,  60,  160, 60),

            syn_keyword:    Color::from_rgb8(196, 148, 255),
            syn_string:     Color::from_rgb8(80,  220, 150),
            syn_comment:    Color::from_rgb8(85,  95,  150),
            syn_function:   Color::from_rgb8(104, 184, 255),
            syn_number:     Color::from_rgb8(255, 190, 60),
            syn_type:       Color::from_rgb8(80,  230, 210),
            syn_operator:   Color::from_rgb8(140, 160, 235),
            syn_macro:      Color::from_rgb8(255, 136, 80),

            glass_bg:       Color::from_rgba8(8,   5,  20,  165), // Deep purple tint
            glass_border:   Color::from_rgba8(80,  60, 160, 200), // Visible purple border
            glow:           Color::from_rgba8(100, 60, 255, 80),  // Violet/indigo glow
        }
    }

    pub fn dark() -> Self {
        Self {
            bg_deep:        Color::from_rgb8(10, 10, 10),
            bg_base:        Color::from_rgb8(20, 20, 20),
            bg_surface:     Color::from_rgb8(28, 28, 28),
            bg_panel:       Color::from_rgb8(18, 18, 18),
            bg_elevated:    Color::from_rgb8(36, 36, 36),

            text_primary:   Color::from_rgb8(212, 212, 212),
            text_secondary: Color::from_rgb8(160, 160, 160),
            text_muted:     Color::from_rgb8(100, 100, 100),
            text_disabled:  Color::from_rgb8(60,  60,  60),

            accent:         Color::from_rgb8(0,   122, 204),
            accent_hover:   Color::from_rgb8(28,  140, 220),
            accent_dim:     Color::from_rgba8(0,  122, 204, 40),

            success:        Color::from_rgb8(78,  201, 140),
            warning:        Color::from_rgb8(220, 170, 30),
            error:          Color::from_rgb8(244, 71,  71),

            border:         Color::from_rgb8(48,  48,  48),
            border_focus:   Color::from_rgb8(0,   122, 204),

            selection:      Color::from_rgba8(0,  122, 204, 50),

            syn_keyword:    Color::from_rgb8(86,  156, 214),
            syn_string:     Color::from_rgb8(206, 145, 120),
            syn_comment:    Color::from_rgb8(106, 153, 85),
            syn_function:   Color::from_rgb8(220, 220, 170),
            syn_number:     Color::from_rgb8(181, 206, 168),
            syn_type:       Color::from_rgb8(78,  201, 176),
            syn_operator:   Color::from_rgb8(212, 212, 212),
            syn_macro:      Color::from_rgb8(86,  156, 214),

            glass_bg:       Color::from_rgba8(18, 18, 18, 220),
            glass_border:   Color::from_rgba8(80, 80, 80, 185),
            glow:           Color::from_rgba8(0,  122, 204, 60),
        }
    }

    pub fn light() -> Self {
        Self {
            bg_deep:        Color::from_rgb8(240, 240, 245),
            bg_base:        Color::from_rgb8(255, 255, 255),
            bg_surface:     Color::from_rgb8(244, 244, 248),
            bg_panel:       Color::from_rgb8(250, 250, 253),
            bg_elevated:    Color::from_rgb8(255, 255, 255),

            text_primary:   Color::from_rgb8(28,  28,  32),
            text_secondary: Color::from_rgb8(80,  80,  96),
            text_muted:     Color::from_rgb8(140, 140, 158),
            text_disabled:  Color::from_rgb8(190, 190, 205),

            accent:         Color::from_rgb8(88,  66,  225),
            accent_hover:   Color::from_rgb8(66,  44,  205),
            accent_dim:     Color::from_rgba8(88, 66,  225, 30),

            success:        Color::from_rgb8(22,  148, 78),
            warning:        Color::from_rgb8(175, 115, 0),
            error:          Color::from_rgb8(196, 28,  28),

            border:         Color::from_rgb8(210, 210, 222),
            border_focus:   Color::from_rgb8(88,  66,  225),

            selection:      Color::from_rgba8(88, 66,  225, 38),

            syn_keyword:    Color::from_rgb8(0,   0,   180),
            syn_string:     Color::from_rgb8(152, 58,  0),
            syn_comment:    Color::from_rgb8(0,   120, 0),
            syn_function:   Color::from_rgb8(96,  0,   175),
            syn_number:     Color::from_rgb8(0,   96,  48),
            syn_type:       Color::from_rgb8(0,   96,  145),
            syn_operator:   Color::from_rgb8(28,  28,  32),
            syn_macro:      Color::from_rgb8(0,   0,   180),

            glass_bg:       Color::from_rgba8(255, 255, 255, 210),
            glass_border:   Color::from_rgba8(88,  66,  225, 185),
            glow:           Color::from_rgba8(88,  66,  225, 55),
        }
    }

    // ── Cyberpunk 2077 ───────────────────────────────────────────────────────
    pub fn cyberpunk() -> Self {
        Self {
            bg_deep:        Color::from_rgba8(8,   2,   22,  240),
            bg_base:        Color::from_rgba8(13,  2,   33,  255),
            bg_surface:     Color::from_rgba8(35,  10,  65,  215),
            bg_panel:       Color::from_rgba8(22,  6,   48,  200),
            bg_elevated:    Color::from_rgba8(50,  15,  90,  230),

            text_primary:   Color::from_rgb8(0,   255, 255),
            text_secondary: Color::from_rgb8(0,   200, 200),
            text_muted:     Color::from_rgb8(80,  120, 180),
            text_disabled:  Color::from_rgb8(40,  60,  100),

            accent:         Color::from_rgb8(255, 0,   220),
            accent_hover:   Color::from_rgb8(255, 80,  240),
            accent_dim:     Color::from_rgba8(255, 0,  220, 45),

            success:        Color::from_rgb8(0,   255, 120),
            warning:        Color::from_rgb8(255, 240, 0),
            error:          Color::from_rgb8(255, 40,  80),

            border:         Color::from_rgba8(255, 0,   220, 180),
            border_focus:   Color::from_rgba8(255, 240, 0,   220),

            selection:      Color::from_rgba8(255, 240, 0,   60),

            syn_keyword:    Color::from_rgb8(255, 0,   220),
            syn_string:     Color::from_rgb8(0,   255, 180),
            syn_comment:    Color::from_rgb8(80,  100, 160),
            syn_function:   Color::from_rgb8(0,   200, 255),
            syn_number:     Color::from_rgb8(255, 240, 0),
            syn_type:       Color::from_rgb8(255, 100, 220),
            syn_operator:   Color::from_rgb8(0,   255, 255),
            syn_macro:      Color::from_rgb8(255, 150, 0),

            glass_bg:       Color::from_rgba8(8,   2,  22,  190),
            glass_border:   Color::from_rgba8(255, 240, 0,  195), // Yellow border — distinctive
            glow:           Color::from_rgba8(255, 0,  220, 130), // Strong magenta glow
        }
    }

    // ── Synthwave '84 ────────────────────────────────────────────────────────
    pub fn synthwave84() -> Self {
        Self {
            bg_deep:        Color::from_rgba8(22,  8,   45,  245),
            bg_base:        Color::from_rgba8(26,  10,  52,  255),
            bg_surface:     Color::from_rgba8(40,  18,  72,  215),
            bg_panel:       Color::from_rgba8(32,  12,  60,  200),
            bg_elevated:    Color::from_rgba8(55,  25,  90,  230),

            text_primary:   Color::from_rgb8(255, 225, 255),
            text_secondary: Color::from_rgb8(200, 170, 220),
            text_muted:     Color::from_rgb8(120, 90,  155),
            text_disabled:  Color::from_rgb8(70,  50,  100),

            accent:         Color::from_rgb8(252, 86,  255),
            accent_hover:   Color::from_rgb8(255, 120, 255),
            accent_dim:     Color::from_rgba8(252, 86,  255, 50),

            success:        Color::from_rgb8(114, 240, 170),
            warning:        Color::from_rgb8(255, 185, 40),
            error:          Color::from_rgb8(255, 60,  100),

            border:         Color::from_rgba8(252, 86,  255, 60),
            border_focus:   Color::from_rgba8(252, 86,  255, 180),

            selection:      Color::from_rgba8(252, 86,  255, 50),

            syn_keyword:    Color::from_rgb8(252, 86,  255),
            syn_string:     Color::from_rgb8(114, 240, 170),
            syn_comment:    Color::from_rgb8(100, 70,  140),
            syn_function:   Color::from_rgb8(104, 200, 255),
            syn_number:     Color::from_rgb8(255, 185, 40),
            syn_type:       Color::from_rgb8(255, 120, 200),
            syn_operator:   Color::from_rgb8(200, 170, 220),
            syn_macro:      Color::from_rgb8(255, 155, 60),

            glass_bg:       Color::from_rgba8(26,  10,  52,  165),
            glass_border:   Color::from_rgba8(252, 86,  255, 185),
            glow:           Color::from_rgba8(252, 86,  255, 90),
        }
    }

    // ── Andromeda ────────────────────────────────────────────────────────────
    pub fn andromeda() -> Self {
        Self {
            bg_deep:        Color::from_rgb8(18,  18,  26),
            bg_base:        Color::from_rgb8(23,  23,  33),
            bg_surface:     Color::from_rgb8(30,  30,  44),
            bg_panel:       Color::from_rgb8(26,  26,  38),
            bg_elevated:    Color::from_rgb8(38,  38,  55),

            text_primary:   Color::from_rgb8(215, 218, 240),
            text_secondary: Color::from_rgb8(155, 158, 200),
            text_muted:     Color::from_rgb8(95,  98,  140),
            text_disabled:  Color::from_rgb8(55,  58,  90),

            accent:         Color::from_rgb8(120, 100, 255),
            accent_hover:   Color::from_rgb8(150, 130, 255),
            accent_dim:     Color::from_rgba8(120, 100, 255, 45),

            success:        Color::from_rgb8(100, 220, 130),
            warning:        Color::from_rgb8(255, 195, 60),
            error:          Color::from_rgb8(255, 80,  90),

            border:         Color::from_rgba8(100, 80,  200, 120),
            border_focus:   Color::from_rgba8(120, 100, 255, 200),

            selection:      Color::from_rgba8(120, 100, 255, 50),

            syn_keyword:    Color::from_rgb8(200, 140, 255),
            syn_string:     Color::from_rgb8(100, 220, 160),
            syn_comment:    Color::from_rgb8(85,  88,  130),
            syn_function:   Color::from_rgb8(110, 190, 255),
            syn_number:     Color::from_rgb8(255, 195, 60),
            syn_type:       Color::from_rgb8(100, 220, 220),
            syn_operator:   Color::from_rgb8(155, 158, 200),
            syn_macro:      Color::from_rgb8(255, 140, 80),

            glass_bg:       Color::from_rgba8(23,  23,  33,  200),
            glass_border:   Color::from_rgba8(120, 100, 255, 185),
            glow:           Color::from_rgba8(120, 100, 255, 80),
        }
    }

    // ── Dracula ──────────────────────────────────────────────────────────────
    pub fn dracula() -> Self {
        Self {
            bg_deep:        Color::from_rgb8(18,  18,  24),
            bg_base:        Color::from_rgb8(40,  42,  54),
            bg_surface:     Color::from_rgb8(50,  52,  66),
            bg_panel:       Color::from_rgb8(44,  46,  58),
            bg_elevated:    Color::from_rgb8(60,  63,  80),

            text_primary:   Color::from_rgb8(248, 248, 242),
            text_secondary: Color::from_rgb8(190, 190, 200),
            text_muted:     Color::from_rgb8(120, 120, 140),
            text_disabled:  Color::from_rgb8(70,  70,  90),

            accent:         Color::from_rgb8(189, 147, 249),
            accent_hover:   Color::from_rgb8(210, 175, 255),
            accent_dim:     Color::from_rgba8(189, 147, 249, 40),

            success:        Color::from_rgb8(80,  250, 123),
            warning:        Color::from_rgb8(255, 184, 108),
            error:          Color::from_rgb8(255, 85,  85),

            border:         Color::from_rgba8(100, 80,  200, 100),
            border_focus:   Color::from_rgba8(189, 147, 249, 200),

            selection:      Color::from_rgba8(189, 147, 249, 50),

            syn_keyword:    Color::from_rgb8(255, 121, 198),
            syn_string:     Color::from_rgb8(241, 250, 140),
            syn_comment:    Color::from_rgb8(98,  114, 164),
            syn_function:   Color::from_rgb8(80,  250, 123),
            syn_number:     Color::from_rgb8(189, 147, 249),
            syn_type:       Color::from_rgb8(139, 233, 253),
            syn_operator:   Color::from_rgb8(248, 248, 242),
            syn_macro:      Color::from_rgb8(255, 184, 108),

            glass_bg:       Color::from_rgba8(40,  42,  54,  210),
            glass_border:   Color::from_rgba8(189, 147, 249, 185),
            glow:           Color::from_rgba8(189, 147, 249, 80),
        }
    }

    // ── Tokyo Night ──────────────────────────────────────────────────────────
    pub fn tokyo_night() -> Self {
        Self {
            bg_deep:        Color::from_rgb8(13,  17,  23),
            bg_base:        Color::from_rgb8(26,  27,  38),
            bg_surface:     Color::from_rgb8(32,  34,  48),
            bg_panel:       Color::from_rgb8(22,  24,  36),
            bg_elevated:    Color::from_rgb8(40,  44,  60),

            text_primary:   Color::from_rgb8(169, 177, 214),
            text_secondary: Color::from_rgb8(120, 130, 170),
            text_muted:     Color::from_rgb8(75,  85,  120),
            text_disabled:  Color::from_rgb8(45,  52,  80),

            accent:         Color::from_rgb8(122, 162, 247),
            accent_hover:   Color::from_rgb8(150, 185, 255),
            accent_dim:     Color::from_rgba8(122, 162, 247, 40),

            success:        Color::from_rgb8(115, 218, 162),
            warning:        Color::from_rgb8(224, 175, 104),
            error:          Color::from_rgb8(247, 118, 142),

            border:         Color::from_rgba8(80,  100, 180, 110),
            border_focus:   Color::from_rgba8(122, 162, 247, 200),

            selection:      Color::from_rgba8(122, 162, 247, 45),

            syn_keyword:    Color::from_rgb8(187, 154, 247),
            syn_string:     Color::from_rgb8(158, 206, 106),
            syn_comment:    Color::from_rgb8(65,  72,  104),
            syn_function:   Color::from_rgb8(122, 162, 247),
            syn_number:     Color::from_rgb8(255, 158, 100),
            syn_type:       Color::from_rgb8(42,  195, 222),
            syn_operator:   Color::from_rgb8(137, 221, 255),
            syn_macro:      Color::from_rgb8(224, 175, 104),

            glass_bg:       Color::from_rgba8(26,  27,  38,  210),
            glass_border:   Color::from_rgba8(122, 162, 247, 185),
            glow:           Color::from_rgba8(122, 162, 247, 75),
        }
    }

    // ── Monokai ──────────────────────────────────────────────────────────────
    pub fn monokai() -> Self {
        Self {
            bg_deep:        Color::from_rgb8(24,  24,  24),
            bg_base:        Color::from_rgb8(39,  40,  34),
            bg_surface:     Color::from_rgb8(50,  50,  43),
            bg_panel:       Color::from_rgb8(44,  44,  38),
            bg_elevated:    Color::from_rgb8(62,  63,  55),

            text_primary:   Color::from_rgb8(248, 248, 242),
            text_secondary: Color::from_rgb8(190, 192, 180),
            text_muted:     Color::from_rgb8(117, 116, 94),
            text_disabled:  Color::from_rgb8(70,  70,  60),

            accent:         Color::from_rgb8(166, 226, 46),
            accent_hover:   Color::from_rgb8(195, 255, 80),
            accent_dim:     Color::from_rgba8(166, 226, 46,  40),

            success:        Color::from_rgb8(166, 226, 46),
            warning:        Color::from_rgb8(253, 151, 31),
            error:          Color::from_rgb8(249, 38,  114),

            border:         Color::from_rgba8(100, 100, 80,  110),
            border_focus:   Color::from_rgba8(166, 226, 46,  200),

            selection:      Color::from_rgba8(73,  72,  62,  200),

            syn_keyword:    Color::from_rgb8(249, 38,  114),
            syn_string:     Color::from_rgb8(230, 219, 116),
            syn_comment:    Color::from_rgb8(117, 116, 94),
            syn_function:   Color::from_rgb8(166, 226, 46),
            syn_number:     Color::from_rgb8(174, 129, 255),
            syn_type:       Color::from_rgb8(102, 217, 239),
            syn_operator:   Color::from_rgb8(249, 38,  114),
            syn_macro:      Color::from_rgb8(253, 151, 31),

            glass_bg:       Color::from_rgba8(39,  40,  34,  215),
            glass_border:   Color::from_rgba8(166, 226, 46,  180),
            glow:           Color::from_rgba8(166, 226, 46,  70),
        }
    }

    // ── Nord Dark ────────────────────────────────────────────────────────────
    pub fn nord_dark() -> Self {
        Self {
            bg_deep:        Color::from_rgb8(30,  34,  42),
            bg_base:        Color::from_rgb8(46,  52,  64),
            bg_surface:     Color::from_rgb8(59,  66,  82),
            bg_panel:       Color::from_rgb8(52,  58,  72),
            bg_elevated:    Color::from_rgb8(67,  76,  94),

            text_primary:   Color::from_rgb8(236, 239, 244),
            text_secondary: Color::from_rgb8(180, 190, 210),
            text_muted:     Color::from_rgb8(110, 120, 145),
            text_disabled:  Color::from_rgb8(67,  76,  94),

            accent:         Color::from_rgb8(136, 192, 208),
            accent_hover:   Color::from_rgb8(160, 210, 230),
            accent_dim:     Color::from_rgba8(136, 192, 208, 40),

            success:        Color::from_rgb8(163, 190, 140),
            warning:        Color::from_rgb8(235, 203, 139),
            error:          Color::from_rgb8(191, 97,  106),

            border:         Color::from_rgba8(90,  110, 140, 120),
            border_focus:   Color::from_rgba8(136, 192, 208, 200),

            selection:      Color::from_rgba8(136, 192, 208, 45),

            syn_keyword:    Color::from_rgb8(180, 142, 173),
            syn_string:     Color::from_rgb8(163, 190, 140),
            syn_comment:    Color::from_rgb8(76,  86,  106),
            syn_function:   Color::from_rgb8(136, 192, 208),
            syn_number:     Color::from_rgb8(208, 135, 112),
            syn_type:       Color::from_rgb8(143, 188, 187),
            syn_operator:   Color::from_rgb8(236, 239, 244),
            syn_macro:      Color::from_rgb8(235, 203, 139),

            glass_bg:       Color::from_rgba8(46,  52,  64,  210),
            glass_border:   Color::from_rgba8(136, 192, 208, 185),
            glow:           Color::from_rgba8(136, 192, 208, 75),
        }
    }

    // ── Matrix Green ─────────────────────────────────────────────────────────
    pub fn matrix_green() -> Self {
        Self {
            bg_deep:        Color::from_rgba8(0,   8,   0,   245),
            bg_base:        Color::from_rgba8(0,   5,   0,   255),
            bg_surface:     Color::from_rgba8(0,   18,  0,   210),
            bg_panel:       Color::from_rgba8(0,   12,  0,   195),
            bg_elevated:    Color::from_rgba8(0,   28,  0,   230),

            text_primary:   Color::from_rgb8(0,   255, 65),
            text_secondary: Color::from_rgb8(0,   200, 50),
            text_muted:     Color::from_rgb8(0,   110, 28),
            text_disabled:  Color::from_rgb8(0,   55,  14),

            accent:         Color::from_rgb8(0,   255, 65),
            accent_hover:   Color::from_rgb8(100, 255, 100),
            accent_dim:     Color::from_rgba8(0,   255, 65,  40),

            success:        Color::from_rgb8(0,   255, 120),
            warning:        Color::from_rgb8(150, 255, 0),
            error:          Color::from_rgb8(255, 0,   0),

            border:         Color::from_rgba8(0,   255, 65,  90),
            border_focus:   Color::from_rgba8(0,   255, 65,  200),

            selection:      Color::from_rgba8(0,   255, 65,  60),

            syn_keyword:    Color::from_rgb8(0,   255, 150),
            syn_string:     Color::from_rgb8(100, 255, 0),
            syn_comment:    Color::from_rgb8(0,   90,  22),
            syn_function:   Color::from_rgb8(0,   255, 200),
            syn_number:     Color::from_rgb8(0,   200, 255),
            syn_type:       Color::from_rgb8(150, 255, 0),
            syn_operator:   Color::from_rgb8(0,   255, 65),
            syn_macro:      Color::from_rgb8(200, 255, 0),

            glass_bg:       Color::from_rgba8(0,   8,   0,   185),
            glass_border:   Color::from_rgba8(0,   255, 65,  185),
            glow:           Color::from_rgba8(0,   255, 65,  90),
        }
    }

    // ── Root Shell (classic green on black console) ───────────────────────────
    pub fn root_shell() -> Self {
        Self {
            bg_deep:        Color::from_rgb8(0,   0,   0),
            bg_base:        Color::from_rgb8(0,   0,   0),
            bg_surface:     Color::from_rgb8(10,  10,  10),
            bg_panel:       Color::from_rgb8(5,   5,   5),
            bg_elevated:    Color::from_rgb8(18,  18,  18),

            text_primary:   Color::from_rgb8(0,   220, 0),
            text_secondary: Color::from_rgb8(0,   180, 0),
            text_muted:     Color::from_rgb8(0,   100, 0),
            text_disabled:  Color::from_rgb8(0,   50,  0),

            accent:         Color::from_rgb8(0,   255, 0),
            accent_hover:   Color::from_rgb8(80,  255, 80),
            accent_dim:     Color::from_rgba8(0,   255, 0,  30),

            success:        Color::from_rgb8(0,   255, 0),
            warning:        Color::from_rgb8(220, 220, 0),
            error:          Color::from_rgb8(220, 0,   0),

            border:         Color::from_rgba8(0,   180, 0,  90),
            border_focus:   Color::from_rgba8(0,   255, 0,  200),

            selection:      Color::from_rgba8(0,   255, 0,  50),

            syn_keyword:    Color::from_rgb8(0,   255, 0),
            syn_string:     Color::from_rgb8(0,   200, 0),
            syn_comment:    Color::from_rgb8(0,   80,  0),
            syn_function:   Color::from_rgb8(0,   255, 100),
            syn_number:     Color::from_rgb8(0,   200, 200),
            syn_type:       Color::from_rgb8(100, 255, 0),
            syn_operator:   Color::from_rgb8(0,   220, 0),
            syn_macro:      Color::from_rgb8(180, 255, 0),

            glass_bg:       Color::from_rgba8(0,   5,   0,   220),
            glass_border:   Color::from_rgba8(0,   200, 0,   180),
            glow:           Color::from_rgba8(0,   200, 0,   85),
        }
    }
}

/// The active theme — use this throughout all UI components.
#[derive(Debug, Clone)]
pub struct PhazeTheme {
    pub variant: ThemeVariant,
    pub palette: PhazePalette,
}

impl Default for PhazeTheme {
    fn default() -> Self {
        Self::midnight_blue()
    }
}

impl PhazeTheme {
    pub fn midnight_blue() -> Self {
        Self {
            variant: ThemeVariant::MidnightBlue,
            palette: PhazePalette::midnight_blue(),
        }
    }

    pub fn dark() -> Self {
        Self {
            variant: ThemeVariant::Dark,
            palette: PhazePalette::dark(),
        }
    }

    pub fn light() -> Self {
        Self {
            variant: ThemeVariant::Light,
            palette: PhazePalette::light(),
        }
    }

    pub fn from_variant(v: ThemeVariant) -> Self {
        let palette = match v {
            ThemeVariant::MidnightBlue => PhazePalette::midnight_blue(),
            ThemeVariant::Cyberpunk    => PhazePalette::cyberpunk(),
            ThemeVariant::Synthwave84  => PhazePalette::synthwave84(),
            ThemeVariant::Andromeda    => PhazePalette::andromeda(),
            ThemeVariant::Dark         => PhazePalette::dark(),
            ThemeVariant::Dracula      => PhazePalette::dracula(),
            ThemeVariant::TokyoNight   => PhazePalette::tokyo_night(),
            ThemeVariant::Monokai      => PhazePalette::monokai(),
            ThemeVariant::NordDark     => PhazePalette::nord_dark(),
            ThemeVariant::MatrixGreen  => PhazePalette::matrix_green(),
            ThemeVariant::RootShell    => PhazePalette::root_shell(),
            ThemeVariant::Light        => PhazePalette::light(),
        };
        Self { variant: v, palette }
    }

    pub fn from_str(s: &str) -> Self {
        Self::from_variant(ThemeVariant::from_str(s))
    }

    pub fn is_dark(&self) -> bool {
        self.variant != ThemeVariant::Light
    }

    /// True if this theme uses the cosmic glass look (animated nebula canvas).
    pub fn is_cosmic(&self) -> bool {
        matches!(self.variant, ThemeVariant::MidnightBlue | ThemeVariant::Cyberpunk | ThemeVariant::Synthwave84)
    }
}
