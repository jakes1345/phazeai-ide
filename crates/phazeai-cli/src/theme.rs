use ratatui::style::Color;

#[derive(Clone)]
pub struct Theme {
    pub name: &'static str,
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
