//! Design tokens: the shared sizes every PhazeAI view is built from.
//!
//! Colours live in [`crate::theme::PhazePalette`] because they change with the
//! theme. Sizes don't, so they live here as constants. Use these instead of
//! literal numbers so spacing, corners and type stay consistent across panels.

/// Spacing scale for padding, margins and gaps (logical pixels).
pub mod space {
    pub const XXS: f64 = 2.0;
    pub const XS: f64 = 4.0;
    pub const SM: f64 = 6.0;
    pub const MD: f64 = 8.0;
    pub const LG: f64 = 12.0;
    pub const XL: f64 = 16.0;
    pub const XXL: f64 = 24.0;
}

/// Corner radii.
pub mod radius {
    /// Badges, chips, small inline controls.
    pub const SM: f64 = 4.0;
    /// Buttons, inputs, tiles.
    pub const MD: f64 = 6.0;
    /// Cards, popovers, dialogs.
    pub const LG: f64 = 10.0;
    /// Fully rounded ends.
    pub const PILL: f64 = 999.0;
}

/// Type scale (font sizes in points).
pub mod font {
    /// Hints, metadata, section labels.
    pub const CAPTION: f32 = 11.0;
    /// Dense lists and secondary text.
    pub const BODY: f32 = 12.0;
    /// Default control and panel text.
    pub const UI: f32 = 13.0;
    /// Panel titles.
    pub const TITLE: f32 = 14.0;
    /// Page and dialog headings.
    pub const HEADING: f32 = 16.0;
}

/// Border widths.
pub mod stroke {
    pub const HAIRLINE: f64 = 1.0;
    pub const FOCUS: f64 = 2.0;
}
