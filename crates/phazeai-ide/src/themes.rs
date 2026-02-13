use egui::Color32;

#[derive(Clone, Debug, PartialEq)]
pub enum ThemePreset {
    Dark,
    TokyoNight,
    Dracula,
    Nord,
    OneDark,
    Monokai,
    SolarizedDark,
    GruvboxDark,
    CatppuccinMocha,
    Cyberpunk,
    Matrix,
    Light,
    SolarizedLight,
    GruvboxLight,
    CatppuccinLatte,
    GitHubLight,
    HighContrastDark,
    HighContrastLight,
    SpectralPurple,
    MidnightBlue,
    TokyoNightStorm,
    TokyoNightDay,
}

impl ThemePreset {
    pub fn name(&self) -> &'static str {
        match self {
            Self::Dark => "Dark",
            Self::TokyoNight => "Tokyo Night",
            Self::TokyoNightStorm => "Tokyo Night Storm",
            Self::Dracula => "Dracula",
            Self::Nord => "Nord",
            Self::OneDark => "One Dark",
            Self::Monokai => "Monokai",
            Self::SolarizedDark => "Solarized Dark",
            Self::GruvboxDark => "Gruvbox Dark",
            Self::CatppuccinMocha => "Catppuccin Mocha",
            Self::SpectralPurple => "Spectral Purple",
            Self::MidnightBlue => "Midnight Blue",
            Self::Cyberpunk => "Cyberpunk",
            Self::Matrix => "Matrix",
            Self::Light => "Light",
            Self::TokyoNightDay => "Tokyo Night Day",
            Self::SolarizedLight => "Solarized Light",
            Self::GruvboxLight => "Gruvbox Light",
            Self::CatppuccinLatte => "Catppuccin Latte",
            Self::GitHubLight => "GitHub Light",
            Self::HighContrastDark => "High Contrast Dark",
            Self::HighContrastLight => "High Contrast Light",
        }
    }

    pub fn all() -> Vec<Self> {
        vec![
            Self::Dark, Self::TokyoNight, Self::TokyoNightStorm, Self::Dracula,
            Self::Nord, Self::OneDark, Self::Monokai, Self::SolarizedDark,
            Self::GruvboxDark, Self::CatppuccinMocha, Self::SpectralPurple,
            Self::MidnightBlue, Self::Cyberpunk, Self::Matrix,
            Self::Light, Self::TokyoNightDay, Self::SolarizedLight,
            Self::GruvboxLight, Self::CatppuccinLatte, Self::GitHubLight,
            Self::HighContrastDark, Self::HighContrastLight,
        ]
    }
}

#[derive(Clone)]
pub struct ThemeColors {
    pub background: Color32,
    pub background_secondary: Color32,
    pub surface: Color32,
    pub panel: Color32,
    pub text: Color32,
    pub text_secondary: Color32,
    pub text_muted: Color32,
    pub accent: Color32,
    pub accent_hover: Color32,
    pub success: Color32,
    pub warning: Color32,
    pub error: Color32,
    pub border: Color32,
    pub selection: Color32,
    // Syntax
    pub keyword: Color32,
    pub string: Color32,
    pub comment: Color32,
    pub function: Color32,
    pub number: Color32,
    pub type_name: Color32,
}

impl ThemeColors {
    pub fn from_preset(preset: &ThemePreset) -> Self {
        match preset {
            ThemePreset::Dark => Self {
                background: Color32::from_rgb(30, 30, 30),
                background_secondary: Color32::from_rgb(37, 37, 37),
                surface: Color32::from_rgb(50, 50, 50),
                panel: Color32::from_rgb(42, 42, 42),
                text: Color32::from_rgb(220, 220, 220),
                text_secondary: Color32::from_rgb(180, 180, 180),
                text_muted: Color32::from_rgb(120, 120, 120),
                accent: Color32::from_rgb(0, 122, 255),
                accent_hover: Color32::from_rgb(0, 100, 200),
                success: Color32::from_rgb(76, 175, 80),
                warning: Color32::from_rgb(255, 152, 0),
                error: Color32::from_rgb(244, 67, 54),
                border: Color32::from_rgb(60, 60, 60),
                selection: Color32::from_rgba_premultiplied(0, 122, 255, 100),
                keyword: Color32::from_rgb(197, 134, 192),
                string: Color32::from_rgb(206, 145, 120),
                comment: Color32::from_rgb(106, 153, 85),
                function: Color32::from_rgb(220, 220, 170),
                number: Color32::from_rgb(181, 206, 168),
                type_name: Color32::from_rgb(78, 201, 176),
            },
            ThemePreset::TokyoNight | ThemePreset::TokyoNightStorm => Self {
                background: Color32::from_rgb(26, 27, 38),
                background_secondary: Color32::from_rgb(31, 32, 45),
                surface: Color32::from_rgb(41, 42, 58),
                panel: Color32::from_rgb(33, 34, 48),
                text: Color32::from_rgb(169, 177, 214),
                text_secondary: Color32::from_rgb(130, 139, 184),
                text_muted: Color32::from_rgb(86, 95, 137),
                accent: Color32::from_rgb(122, 162, 247),
                accent_hover: Color32::from_rgb(95, 135, 220),
                success: Color32::from_rgb(158, 206, 106),
                warning: Color32::from_rgb(224, 175, 104),
                error: Color32::from_rgb(247, 118, 142),
                border: Color32::from_rgb(52, 53, 74),
                selection: Color32::from_rgba_premultiplied(122, 162, 247, 100),
                keyword: Color32::from_rgb(187, 154, 247),
                string: Color32::from_rgb(158, 206, 106),
                comment: Color32::from_rgb(86, 95, 137),
                function: Color32::from_rgb(122, 162, 247),
                number: Color32::from_rgb(255, 199, 119),
                type_name: Color32::from_rgb(54, 206, 193),
            },
            ThemePreset::Dracula => Self {
                background: Color32::from_rgb(40, 42, 54),
                background_secondary: Color32::from_rgb(48, 50, 64),
                surface: Color32::from_rgb(68, 71, 90),
                panel: Color32::from_rgb(52, 55, 70),
                text: Color32::from_rgb(248, 248, 242),
                text_secondary: Color32::from_rgb(180, 180, 180),
                text_muted: Color32::from_rgb(98, 114, 164),
                accent: Color32::from_rgb(255, 121, 198),
                accent_hover: Color32::from_rgb(255, 89, 178),
                success: Color32::from_rgb(80, 250, 123),
                warning: Color32::from_rgb(241, 250, 140),
                error: Color32::from_rgb(255, 85, 85),
                border: Color32::from_rgb(68, 71, 90),
                selection: Color32::from_rgba_premultiplied(68, 71, 90, 150),
                keyword: Color32::from_rgb(255, 121, 198),
                string: Color32::from_rgb(241, 250, 140),
                comment: Color32::from_rgb(98, 114, 164),
                function: Color32::from_rgb(189, 147, 249),
                number: Color32::from_rgb(255, 184, 108),
                type_name: Color32::from_rgb(139, 233, 253),
            },
            // Default fallback for all other themes - Tokyo Night colors
            _ => Self::from_preset(&ThemePreset::Dark),
        }
    }

    pub fn apply(&self, ctx: &egui::Context) {
        let mut style = (*ctx.style()).clone();
        style.visuals.extreme_bg_color = self.background;
        style.visuals.window_fill = self.surface;
        style.visuals.panel_fill = self.panel;
        style.visuals.code_bg_color = self.background_secondary;
        style.visuals.hyperlink_color = self.accent;
        style.visuals.selection.bg_fill = self.selection;
        style.visuals.selection.stroke.color = self.text;
        style.visuals.window_stroke.color = self.border;
        style.visuals.widgets.noninteractive.bg_fill = self.surface;
        style.visuals.widgets.noninteractive.fg_stroke.color = self.text;
        style.visuals.widgets.inactive.bg_fill = self.background_secondary;
        style.visuals.widgets.inactive.fg_stroke.color = self.text;
        style.visuals.widgets.hovered.bg_fill = self.surface;
        style.visuals.widgets.hovered.fg_stroke.color = self.text;
        style.visuals.widgets.active.bg_fill = self.accent;
        style.visuals.widgets.active.fg_stroke.color = self.text;
        ctx.set_style(style);
    }
}
