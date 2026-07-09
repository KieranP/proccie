//! A process entry from the Procfile: its command, environment, dependencies,
//! and the policy for when it releases its dependents.

use std::collections::BTreeMap;

use serde::Deserialize;

use super::readiness::Readiness;

/// Exit codes considered expected for a process; an empty set means any exit
/// triggers shutdown. In TOML: an array of integers, e.g. `exit_codes = [0, 1]`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(transparent)]
pub struct ExitCodes(pub Vec<i32>);

impl ExitCodes {
    /// Reports whether the given exit code is in the expected set. Returns
    /// `false` if the set is empty (no exits are expected).
    pub fn allows(&self, code: i32) -> bool {
        self.0.contains(&code)
    }

    /// Whether the expected-exit-code set is empty (none configured).
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// When a process releases its dependents, carrying that policy's config so
/// release sites match exhaustively instead of re-reading the raw fields.
#[derive(Debug, Clone, Copy)]
pub enum ReadyWhen<'a> {
    /// The moment the process launches.
    Launched,
    /// When the process exits with an expected code.
    ExpectedExit(&'a ExitCodes),
    /// When the readiness check passes: a shell/HTTP probe succeeds, the watched
    /// output appears, or the delay elapses.
    ReadinessPass(&'a Readiness),
}

impl ReadyWhen<'_> {
    /// Describes the release moment for log output ("waiting for X to ...").
    pub fn verb(self) -> &'static str {
        match self {
            ReadyWhen::Launched => "launch",
            ReadyWhen::ExpectedExit(_) => "exit with expected code",
            ReadyWhen::ReadinessPass(Readiness::Delay(_)) => "become ready after its delay",
            ReadyWhen::ReadinessPass(_) => "pass readiness check",
        }
    }
}

/// A single process entry from the TOML config.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Process {
    /// Shell command to run (required). Executed via `sh -c`.
    #[serde(default)]
    pub command: String,

    /// Exit codes considered expected; empty (default) means any exit triggers
    /// shutdown. A code outside a non-empty list fails, 0 included. Excludes `readiness`.
    #[serde(default)]
    pub exit_codes: ExitCodes,

    /// Readiness check; dependents wait until its shell/http probe passes, its
    /// output appears, or its delay elapses. Mutually exclusive with `exit_codes`.
    #[serde(default)]
    pub readiness: Option<Readiness>,

    /// Process names that must be ready before this one starts.
    #[serde(default)]
    pub depends_on: Vec<String>,

    /// Additional environment variables for this process, merged on top of
    /// the inherited environment.
    #[serde(default)]
    pub environment: BTreeMap<String, String>,

    /// Optional file path; when set, all output is also written here without
    /// ANSI color codes, in addition to the console.
    #[serde(default)]
    pub log_file: Option<String>,

    /// Optional path to a dotenv-style file applied to this process.
    #[serde(default)]
    pub env_file: Option<String>,

    /// Maximum number of times to restart this process after an unexpected
    /// exit (clean or not). A value of 0 (the default) means no retries.
    #[serde(default)]
    pub max_retries: i64,

    /// Optional display name for the TUI tab and log prefix; the service key
    /// stays the canonical identifier. Falls back to the key when unset.
    #[serde(default)]
    pub name: Option<String>,

    /// Optional prefix/tab color: a named ANSI color (`red`, `bright-green`,
    /// …) or `#rrggbb` hex. Validated at load, parsed on demand by [`color`](Self::color).
    #[serde(default)]
    pub(in crate::config) color: Option<String>,

    /// The fully resolved environment, computed during
    /// [`Config::load`](crate::config::Config::load); not from TOML.
    #[serde(skip)]
    pub(in crate::config) env: BTreeMap<String, String>,
}

impl Process {
    /// Returns the fully resolved environment.
    pub fn env(&self) -> &BTreeMap<String, String> {
        &self.env
    }

    /// Returns the configured display name, or `key` when unset. The key
    /// remains the canonical identifier; this is purely cosmetic.
    pub fn display_name<'a>(&'a self, key: &'a str) -> &'a str {
        self.name.as_deref().unwrap_or(key)
    }

    /// Returns the configured prefix/tab color, if any (parsed; validated at load).
    pub fn color(&self) -> Option<anstyle::Color> {
        self.color.as_deref().and_then(crate::theme::parse_color)
    }

    /// Returns when this process releases its dependents; validation
    /// guarantees `exit_codes` and `readiness` are mutually exclusive.
    pub fn ready_when(&self) -> ReadyWhen<'_> {
        if !self.exit_codes.is_empty() {
            ReadyWhen::ExpectedExit(&self.exit_codes)
        } else if let Some(readiness) = &self.readiness {
            ReadyWhen::ReadinessPass(readiness)
        } else {
            ReadyWhen::Launched
        }
    }
}
