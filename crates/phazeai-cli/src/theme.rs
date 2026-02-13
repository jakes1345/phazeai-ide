use ratatui::style::Color;

#[derive(Clone)]
pub struct Theme {
    pub name: &'static str,
    pub bg: Color,
    pub fg: Color,
    pub accent: Color,
    pub muted: Color,
    pub success: Color,
    pub error: Color,
    pub warning: Color,
    pub user_color: Color,
    pub assistant_color: Color,
    pub system_color: Color,
    pub tool_color: Color,
    pub border: Color,
}

impl Theme {
    pub fn dark() -> Self {
        Self {
            name: "dark",
            bg: Color::Rgb(30, 30, 30),
            fg: Color::Rgb(220, 220, 220),
            accent: Color::Rgb(122, 162, 247),
            muted: Color::Rgb(100, 100, 100),
            success: Color::Rgb(158, 206, 106),
            error: Color::Rgb(247, 118, 142),
            warning: Color::Rgb(224, 175, 104),
            user_color: Color::Cyan,
            assistant_color: Color::Green,
            system_color: Color::Yellow,
            tool_color: Color::DarkGray,
            border: Color::Rgb(60, 60, 60),
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
            system_color: Color::Rgb(224, 175, 104),
            tool_color: Color::Rgb(86, 95, 137),
            border: Color::Rgb(52, 53, 74),
        }
    }

    pub fn dracula() -> Self {
        Self {
            name: "dracula",
            bg: Color::Rgb(40, 42, 54),
            fg: Color::Rgb(248, 248, 242),
            accent: Color::Rgb(255, 121, 198),
            muted: Color::Rgb(98, 114, 164),
            success: Color::Rgb(80, 250, 123),
            error: Color::Rgb(255, 85, 85),
            warning: Color::Rgb(241, 250, 140),
            user_color: Color::Rgb(139, 233, 253),
            assistant_color: Color::Rgb(80, 250, 123),
            system_color: Color::Rgb(241, 250, 140),
            tool_color: Color::Rgb(98, 114, 164),
            border: Color::Rgb(68, 71, 90),
        }
    }

    pub fn by_name(name: &str) -> Self {
        match name {
            "tokyo-night" => Self::tokyo_night(),
            "dracula" => Self::dracula(),
            _ => Self::dark(),
        }
    }

    pub fn all_names() -> &'static [&'static str] {
        &["dark", "tokyo-night", "dracula"]
    }
}
