use std::collections::BTreeMap;
use std::path::Path;

use anstyle::{AnsiColor, Color, RgbColor};
use toml::Value;

use super::error::ConfigError;
use super::types::Process;

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

/// The raw, parsed config before validation and environment resolution.
pub struct Parsed {
    pub global_env_file: Option<String>,
    pub global_env: BTreeMap<String, String>,
    pub processes: BTreeMap<String, Process>,
}

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

/// Decodes TOML into processes, splitting out top-level `env_file`/`environment`.
/// Remaining tables become processes; other scalar keys are rejected.
pub fn parse(data: &str, path: &Path) -> Result<Parsed, ConfigError> {
    let mut table: toml::Table = data.parse().map_err(|source| ConfigError::Toml {
        path: path.to_path_buf(),
        source: Box::new(source),
    })?;

    let global_env_file = take_env_file(&mut table, path)?;
    let global_env = take_environment(&mut table, path)?;

    let mut processes = BTreeMap::new();
    for (name, value) in table {
        if !value.is_table() {
            return Err(ConfigError::UnknownTopLevelKey {
                path: path.to_path_buf(),
                key: name,
            });
        }

        let proc = value.try_into().map_err(|source| ConfigError::Process {
            path: path.to_path_buf(),
            name: name.clone(),
            source: Box::new(source),
        })?;
        processes.insert(name, proc);
    }

    Ok(Parsed {
        global_env_file,
        global_env,
        processes,
    })
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

/// Removes and validates the top-level `env_file` key, if present.
fn take_env_file(table: &mut toml::Table, path: &Path) -> Result<Option<String>, ConfigError> {
    match table.remove("env_file") {
        None => Ok(None),
        Some(Value::String(s)) => Ok(Some(s)),
        Some(_) => Err(ConfigError::EnvFileNotString {
            path: path.to_path_buf(),
        }),
    }
}

/// Removes and validates the top-level `environment` table, if present.
fn take_environment(
    table: &mut toml::Table,
    path: &Path,
) -> Result<BTreeMap<String, String>, ConfigError> {
    let Some(value) = table.remove("environment") else {
        return Ok(BTreeMap::new());
    };

    let Value::Table(entries) = value else {
        return Err(ConfigError::EnvironmentNotTable {
            path: path.to_path_buf(),
        });
    };

    entries
        .into_iter()
        .map(|(key, value)| match value {
            Value::String(s) => Ok((key, s)),
            _ => Err(ConfigError::EnvironmentValueNotString {
                path: path.to_path_buf(),
                key,
            }),
        })
        .collect()
}
