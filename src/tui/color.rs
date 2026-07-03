//! The one place stored `anstyle` colors map to ratatui, for tabs and log lines.

use anstyle::{AnsiColor, Color as AnsiStyleColor};
use ratatui::style::{Color, Modifier, Style};

use crate::logger::{Emphasis, LogLevel};
use crate::service::ServiceStatus;
use crate::theme::Theme;

/// A service's status-icon color.
pub fn status_color(status: ServiceStatus, theme: Theme) -> Color {
    let accent = match status {
        ServiceStatus::Running | ServiceStatus::Completed(_) => theme.success(),
        ServiceStatus::Failed(_) => theme.error(),
        ServiceStatus::Waiting | ServiceStatus::Stopped => theme.subtle(),
    };
    to_ratatui(accent)
}

/// A system line's style at `level`; shares [`Emphasis`] so stream and TUI can't drift.
pub fn system_style(level: LogLevel, color: Color) -> Style {
    let style = Style::default().fg(color);
    match level.emphasis() {
        Emphasis::Dim => style.add_modifier(Modifier::DIM),
        Emphasis::Normal => style,
        Emphasis::Bold => style.add_modifier(Modifier::BOLD),
    }
}

/// Maps a stored `anstyle::Color` to its ratatui equivalent.
pub fn to_ratatui(color: AnsiStyleColor) -> Color {
    match color {
        AnsiStyleColor::Ansi(c) => ansi_to_ratatui(c),
        AnsiStyleColor::Ansi256(c) => Color::Indexed(c.0),
        AnsiStyleColor::Rgb(c) => Color::Rgb(c.0, c.1, c.2),
    }
}

/// Maps the 16 named ANSI colors; ratatui's `White`/`Gray` split mirrors the
/// normal/bright distinction.
fn ansi_to_ratatui(c: AnsiColor) -> Color {
    match c {
        AnsiColor::Black => Color::Black,
        AnsiColor::Red => Color::Red,
        AnsiColor::Green => Color::Green,
        AnsiColor::Yellow => Color::Yellow,
        AnsiColor::Blue => Color::Blue,
        AnsiColor::Magenta => Color::Magenta,
        AnsiColor::Cyan => Color::Cyan,
        AnsiColor::White => Color::Gray,
        AnsiColor::BrightBlack => Color::DarkGray,
        AnsiColor::BrightRed => Color::LightRed,
        AnsiColor::BrightGreen => Color::LightGreen,
        AnsiColor::BrightYellow => Color::LightYellow,
        AnsiColor::BrightBlue => Color::LightBlue,
        AnsiColor::BrightMagenta => Color::LightMagenta,
        AnsiColor::BrightCyan => Color::LightCyan,
        AnsiColor::BrightWhite => Color::White,
    }
}
