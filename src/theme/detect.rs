//! Terminal background detection via an OSC 11 query.

use terminal_colorsaurus::{QueryOptions, ThemeMode, theme_mode};

use super::Theme;

impl Theme {
    /// Queries the terminal background via OSC 11 (DA1 fallback); dark on failure.
    pub fn detect_theme() -> Theme {
        match theme_mode(QueryOptions::default()) {
            Ok(ThemeMode::Light) => Theme::Light,
            _ => Theme::Dark,
        }
    }
}
