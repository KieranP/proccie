//! Per-background colors: the service palette, the neutrals, and the accents.

use anstyle::{Ansi256Color, AnsiColor, Color};

use super::Theme;

/// Dark background: bright hues first; normals (dimmer on black) trail.
const DARK: &[AnsiColor] = &[
    AnsiColor::BrightRed,
    AnsiColor::BrightYellow,
    AnsiColor::BrightGreen,
    AnsiColor::BrightCyan,
    AnsiColor::BrightMagenta,
    AnsiColor::BrightBlue,
    AnsiColor::Red,
    AnsiColor::Yellow,
    AnsiColor::Green,
    AnsiColor::Cyan,
];

/// Light background: saturated normals first; brights (washed out on white) trail.
const LIGHT: &[AnsiColor] = &[
    AnsiColor::Red,
    AnsiColor::Yellow,
    AnsiColor::Green,
    AnsiColor::Cyan,
    AnsiColor::Magenta,
    AnsiColor::Blue,
    AnsiColor::BrightRed,
    AnsiColor::BrightYellow,
    AnsiColor::BrightGreen,
    AnsiColor::BrightCyan,
];

impl Theme {
    /// The per-service palette.
    pub fn palette(self) -> &'static [AnsiColor] {
        match self {
            Theme::Dark => DARK,
            Theme::Light => LIGHT,
        }
    }

    /// A subdued but legible neutral: notes, info, dividers, idle status.
    pub fn subtle(self) -> Color {
        match self {
            Theme::Dark => Color::Ansi(AnsiColor::White),
            Theme::Light => Color::Ansi(AnsiColor::BrightBlack),
        }
    }

    /// The faintest neutral (debug), below [`subtle`](Self::subtle); a 256-color
    /// mid-gray on light, where no 16-color slot fits.
    pub fn faint(self) -> Color {
        match self {
            Theme::Dark => Color::Ansi(AnsiColor::BrightBlack),
            Theme::Light => Color::Ansi256(Ansi256Color(245)),
        }
    }

    /// A running or completed service (green).
    pub fn success(self) -> Color {
        match self {
            Theme::Dark => Color::Ansi(AnsiColor::BrightGreen),
            Theme::Light => Color::Ansi(AnsiColor::Green),
        }
    }

    /// Warnings, and the unread-output marker (yellow).
    pub fn warning(self) -> Color {
        match self {
            Theme::Dark => Color::Ansi(AnsiColor::BrightYellow),
            Theme::Light => Color::Ansi(AnsiColor::Yellow),
        }
    }

    /// Errors (red).
    pub fn error(self) -> Color {
        match self {
            Theme::Dark => Color::Ansi(AnsiColor::BrightRed),
            Theme::Light => Color::Ansi(AnsiColor::Red),
        }
    }
}
