use std::collections::BTreeMap;
use std::time::Duration;

use serde::Deserialize;
use serde::de::{self, Deserializer, MapAccess, Visitor};

use super::{DEFAULT_READINESS_INTERVAL, DEFAULT_READINESS_TIMEOUT};

/// Parses a humantime duration (`"10s"`, `"500ms"`); shared by the CLI
/// duration flags and TOML duration values so both render errors the same way.
pub fn parse_duration(s: &str) -> Result<Duration, String> {
    humantime::parse_duration(s).map_err(|e| format!("invalid duration {s:?}: {e}"))
}

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

    /// Reports whether any expected exit codes are configured.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// A readiness check; dependents wait until its command exits 0. In TOML: a
/// bare string, or a table with `command`/`interval`/`timeout` (integer
/// seconds, or a duration string like `"500ms"`).
#[derive(Debug, Clone)]
pub struct Readiness {
    pub command: String,
    pub interval: Option<Duration>,
    pub timeout: Option<Duration>,
}

impl Readiness {
    /// Returns the configured interval, or [`DEFAULT_READINESS_INTERVAL`].
    pub fn interval_or_default(&self) -> Duration {
        self.interval
            .filter(|d| !d.is_zero())
            .unwrap_or(DEFAULT_READINESS_INTERVAL)
    }

    /// Returns the configured timeout, or [`DEFAULT_READINESS_TIMEOUT`].
    pub fn timeout_or_default(&self) -> Duration {
        self.timeout
            .filter(|d| !d.is_zero())
            .unwrap_or(DEFAULT_READINESS_TIMEOUT)
    }
}

/// A readiness duration: bare integer seconds, or a humantime string like
/// `"500ms"` (matching the CLI duration flags). Non-positive values mean
/// "unset" (the default applies) rather than a parse error.
struct DurationValue(Option<Duration>);

impl<'de> Deserialize<'de> for DurationValue {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct DurationVisitor;

        impl<'de> Visitor<'de> for DurationVisitor {
            type Value = DurationValue;

            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("an integer (seconds) or a duration string like \"500ms\"")
            }

            fn visit_i64<E: de::Error>(self, secs: i64) -> Result<DurationValue, E> {
                Ok(DurationValue(
                    (secs > 0).then(|| Duration::from_secs(secs as u64)),
                ))
            }

            fn visit_u64<E: de::Error>(self, secs: u64) -> Result<DurationValue, E> {
                Ok(DurationValue((secs > 0).then(|| Duration::from_secs(secs))))
            }

            fn visit_str<E: de::Error>(self, s: &str) -> Result<DurationValue, E> {
                parse_duration(s)
                    .map(|d| DurationValue((!d.is_zero()).then_some(d)))
                    .map_err(de::Error::custom)
            }

            fn visit_f64<E: de::Error>(self, secs: f64) -> Result<DurationValue, E> {
                // Suggest the user's own value in parseable form when possible.
                let hint = Duration::try_from_secs_f64(secs)
                    .ok()
                    .filter(|d| !d.is_zero())
                    .map_or_else(
                        || "\"500ms\"".to_owned(),
                        |d| format!("\"{}\"", humantime::format_duration(d)),
                    );
                Err(de::Error::custom(format!(
                    "fractional seconds are not supported; use a duration string like {hint}"
                )))
            }
        }

        deserializer.deserialize_any(DurationVisitor)
    }
}

// Readiness is either a bare string or a table, so it needs a custom deserializer.
impl<'de> Deserialize<'de> for Readiness {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct ReadinessVisitor;

        impl<'de> Visitor<'de> for ReadinessVisitor {
            type Value = Readiness;

            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("a readiness command string or a table with a \"command\" key")
            }

            fn visit_str<E: de::Error>(self, value: &str) -> Result<Readiness, E> {
                Ok(Readiness {
                    command: value.to_owned(),
                    interval: None,
                    timeout: None,
                })
            }

            fn visit_map<M>(self, mut map: M) -> Result<Readiness, M::Error>
            where
                M: MapAccess<'de>,
            {
                let mut command: Option<String> = None;
                let mut interval: Option<Duration> = None;
                let mut timeout: Option<Duration> = None;

                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "command" => command = Some(map.next_value()?),
                        "interval" => interval = map.next_value::<DurationValue>()?.0,
                        "timeout" => timeout = map.next_value::<DurationValue>()?.0,
                        // Ignore unknown keys, like the rest of the config.
                        _ => {
                            map.next_value::<de::IgnoredAny>()?;
                        }
                    }
                }

                let command = command.ok_or_else(|| {
                    de::Error::custom("readiness: table form requires \"command\" key")
                })?;

                Ok(Readiness {
                    command,
                    interval,
                    timeout,
                })
            }
        }

        deserializer.deserialize_any(ReadinessVisitor)
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
    /// When the readiness command passes.
    ReadinessPass(&'a Readiness),
}

impl ReadyWhen<'_> {
    /// Describes the release moment for log output ("waiting for X to ...").
    pub fn verb(self) -> &'static str {
        match self {
            ReadyWhen::Launched => "launch",
            ReadyWhen::ExpectedExit(_) => "exit with expected code",
            ReadyWhen::ReadinessPass(_) => "pass readiness check",
        }
    }
}

/// A single process entry from the TOML config.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Process {
    /// Shell command to run (required). Executed via `sh -c`.
    #[serde(default)]
    pub command: String,

    /// Exit codes considered expected; an empty list (default) means any exit
    /// triggers shutdown. Mutually exclusive with `readiness`.
    #[serde(default)]
    pub exit_codes: ExitCodes,

    /// Readiness check; dependents wait until its command exits 0.
    /// Mutually exclusive with `exit_codes`.
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

    /// Maximum number of times to restart this process after it exits with
    /// an error code. A value of 0 (the default) means no retries.
    #[serde(default)]
    pub max_retries: i64,

    /// The fully resolved environment, computed during
    /// [`Config::load`](super::Config::load); not read from TOML. Private so
    /// a `Process` with an unresolved environment can't leave this module.
    #[serde(skip)]
    pub(super) env: BTreeMap<String, String>,
}

impl Process {
    /// Returns the fully resolved environment.
    pub fn env(&self) -> &BTreeMap<String, String> {
        &self.env
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
