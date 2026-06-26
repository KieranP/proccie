//! Log severity for leveled messages: ordering, labels, and the
//! console/structured styling each level renders with.

use anstyle::{AnsiColor, Style};

/// Severity of a leveled log message. A line is emitted only when its level is
/// at or above the configured threshold; ordering follows the declaration order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum LogLevel {
    Debug,
    Info,
    Warn,
    Error,
}

/// How a level's text is emphasized, independent of color; the single source
/// of the dim/normal/bold policy for every renderer (stream and TUI).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Emphasis {
    Dim,
    Normal,
    Bold,
}

impl LogLevel {
    /// Every level, lowest to highest.
    pub const ALL: [LogLevel; 4] = [
        LogLevel::Debug,
        LogLevel::Info,
        LogLevel::Warn,
        LogLevel::Error,
    ];

    /// The uppercase label shown in the log prefix (e.g. `INFO`).
    pub fn label(self) -> &'static str {
        match self {
            LogLevel::Debug => "DEBUG",
            LogLevel::Info => "INFO",
            LogLevel::Warn => "WARN",
            LogLevel::Error => "ERROR",
        }
    }

    /// The dim/normal/bold emphasis for lines at this level.
    pub fn emphasis(self) -> Emphasis {
        match self {
            LogLevel::Debug | LogLevel::Info => Emphasis::Dim,
            LogLevel::Warn => Emphasis::Normal,
            LogLevel::Error => Emphasis::Bold,
        }
    }

    /// The console style for lines at this level: a colored base for warn/error
    /// plus the level's [`Emphasis`].
    pub fn style(self) -> Style {
        let base = match self {
            LogLevel::Warn => Style::new().fg_color(Some(AnsiColor::Yellow.into())),
            LogLevel::Error => Style::new().fg_color(Some(AnsiColor::Red.into())),
            LogLevel::Debug | LogLevel::Info => Style::new(),
        };
        match self.emphasis() {
            Emphasis::Dim => base.dimmed(),
            Emphasis::Normal => base,
            Emphasis::Bold => base.bold(),
        }
    }

    /// The representative prefix color for this level, used when storing
    /// structured system lines for the TUI.
    pub fn color(self) -> anstyle::Color {
        match self {
            LogLevel::Debug => AnsiColor::BrightBlack.into(),
            LogLevel::Info => AnsiColor::White.into(),
            LogLevel::Warn => AnsiColor::Yellow.into(),
            LogLevel::Error => AnsiColor::Red.into(),
        }
    }
}

impl std::str::FromStr for LogLevel {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "debug" => Ok(LogLevel::Debug),
            "info" => Ok(LogLevel::Info),
            "warn" | "warning" => Ok(LogLevel::Warn),
            "error" => Ok(LogLevel::Error),
            _ => Err(format!(
                "invalid log level '{s}' (expected one of: debug, info, warn, error)"
            )),
        }
    }
}

impl std::fmt::Display for LogLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.label())
    }
}
