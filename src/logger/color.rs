//! Default tag colors: a palette cycled through for tags without a configured
//! color. Colors are `anstyle::Color`, mapped to a UI color at render time.

use anstyle::AnsiColor;

/// Colors cycled through for tags without a configured color.
pub const PREFIX_COLORS: &[AnsiColor] = &[
    AnsiColor::Cyan,
    AnsiColor::Yellow,
    AnsiColor::Magenta,
    AnsiColor::Green,
    AnsiColor::Blue,
    AnsiColor::Red,
    AnsiColor::BrightCyan,
    AnsiColor::BrightYellow,
    AnsiColor::BrightMagenta,
    AnsiColor::BrightGreen,
];
