//! Parses a user-configured color string into an `anstyle` color.

use anstyle::{AnsiColor, Color, RgbColor};

/// The 16 named ANSI colors (hyphenated, lowercase) — the canonical name set.
const ANSI_NAMES: [(&str, AnsiColor); 16] = [
    ("black", AnsiColor::Black),
    ("red", AnsiColor::Red),
    ("green", AnsiColor::Green),
    ("yellow", AnsiColor::Yellow),
    ("blue", AnsiColor::Blue),
    ("magenta", AnsiColor::Magenta),
    ("cyan", AnsiColor::Cyan),
    ("white", AnsiColor::White),
    ("bright-black", AnsiColor::BrightBlack),
    ("bright-red", AnsiColor::BrightRed),
    ("bright-green", AnsiColor::BrightGreen),
    ("bright-yellow", AnsiColor::BrightYellow),
    ("bright-blue", AnsiColor::BrightBlue),
    ("bright-magenta", AnsiColor::BrightMagenta),
    ("bright-cyan", AnsiColor::BrightCyan),
    ("bright-white", AnsiColor::BrightWhite),
];

/// Parses a color: a named ANSI color (`red`, `bright-green`, … the 16) or
/// `#rrggbb` hex. Returns `None` for an unrecognized value, which validation reports.
pub fn parse_color(s: &str) -> Option<Color> {
    let value = s.trim();
    if let Some(hex) = value.strip_prefix('#') {
        return parse_hex(hex);
    }

    // Accept either hyphen or underscore separators, case-insensitively.
    let named = value.to_ascii_lowercase().replace('_', "-");
    ANSI_NAMES
        .iter()
        .find(|(name, _)| *name == named)
        .map(|&(_, ansi)| Color::Ansi(ansi))
}

/// Parses a six-digit `rrggbb` hex string into an RGB color.
fn parse_hex(hex: &str) -> Option<Color> {
    // Require exactly six hex digits; `from_str_radix` alone would accept a sign.
    if hex.len() != 6 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let byte = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).ok();
    Some(Color::Rgb(RgbColor(byte(0)?, byte(2)?, byte(4)?)))
}
