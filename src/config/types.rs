use std::collections::BTreeMap;
use std::time::Duration;

use serde::Deserialize;
use serde::de::{self, Deserializer, MapAccess, Visitor};

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

/// A readiness check; dependents wait until it completes. In TOML: a table with
/// `command` plus `exit_codes`/`output` (and optional `interval`/`timeout`), or `delay`.
#[derive(Debug, Clone)]
pub enum Readiness {
    /// Poll a command until it passes: exit code in `exit_codes` (when set) and
    /// stdout contains `output` (when set). Validation requires at least one.
    Command {
        command: String,
        interval: Option<Duration>,
        timeout: Option<Duration>,
        exit_codes: Option<ExitCodes>,
        output: Option<String>,
    },
    /// Sleep for a fixed duration, then release dependents.
    Delay(Duration),
}

/// A readiness duration: integer seconds, a positive float, or a humantime
/// string like `"500ms"`. Zero means "unset" (the default applies); negative errors.
struct DurationValue(Option<Duration>);

impl DurationValue {
    /// Wraps a duration, treating zero as "unset" so the configured default applies.
    fn new(d: Duration) -> DurationValue {
        DurationValue((!d.is_zero()).then_some(d))
    }
}

impl<'de> Deserialize<'de> for DurationValue {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct DurationVisitor;

        impl Visitor<'_> for DurationVisitor {
            type Value = DurationValue;

            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("an integer (seconds) or a duration string like \"500ms\"")
            }

            fn visit_i64<E: de::Error>(self, secs: i64) -> Result<DurationValue, E> {
                let secs = u64::try_from(secs).map_err(|_| {
                    de::Error::custom(format!("duration must not be negative (got {secs})"))
                })?;
                Ok(DurationValue::new(Duration::from_secs(secs)))
            }

            fn visit_u64<E: de::Error>(self, secs: u64) -> Result<DurationValue, E> {
                Ok(DurationValue::new(Duration::from_secs(secs)))
            }

            fn visit_str<E: de::Error>(self, s: &str) -> Result<DurationValue, E> {
                parse_duration(s)
                    .map(DurationValue::new)
                    .map_err(de::Error::custom)
            }

            fn visit_f64<E: de::Error>(self, secs: f64) -> Result<DurationValue, E> {
                if secs < 0.0 {
                    return Err(de::Error::custom(format!(
                        "duration must not be negative (got {secs})"
                    )));
                }
                Duration::try_from_secs_f64(secs)
                    .map(DurationValue::new)
                    .map_err(|_| de::Error::custom(format!("invalid duration: {secs}")))
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
                f.write_str(
                    "a readiness command string or a table with a \"command\" or \"delay\" key",
                )
            }

            // A bare string is shorthand for "run this command, ready on exit 0".
            fn visit_str<E: de::Error>(self, value: &str) -> Result<Readiness, E> {
                Ok(Readiness::Command {
                    command: value.to_owned(),
                    interval: None,
                    timeout: None,
                    exit_codes: Some(ExitCodes(vec![0])),
                    output: None,
                })
            }

            fn visit_map<M>(self, mut map: M) -> Result<Readiness, M::Error>
            where
                M: MapAccess<'de>,
            {
                let mut command: Option<String> = None;
                let mut interval: Option<Duration> = None;
                let mut timeout: Option<Duration> = None;
                let mut exit_codes: Option<ExitCodes> = None;
                let mut output: Option<String> = None;
                let mut delay: Option<Duration> = None;

                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "command" => command = Some(map.next_value()?),
                        "interval" => interval = map.next_value::<DurationValue>()?.0,
                        "timeout" => timeout = map.next_value::<DurationValue>()?.0,
                        // Presence is captured here; validation rejects empty values.
                        "exit_codes" => exit_codes = Some(map.next_value()?),
                        "output" => output = Some(map.next_value()?),
                        // Zero is a valid immediate-ready delay, so keep it rather than dropping it.
                        "delay" => {
                            delay = Some(map.next_value::<DurationValue>()?.0.unwrap_or_default());
                        }
                        // Reject unknown keys so typos surface instead of being ignored.
                        _ => {
                            return Err(de::Error::unknown_field(
                                &key,
                                &[
                                    "command",
                                    "interval",
                                    "timeout",
                                    "exit_codes",
                                    "output",
                                    "delay",
                                ],
                            ));
                        }
                    }
                }

                // "delay" is a standalone mode: it can't share the table with command polling.
                if let Some(delay) = delay {
                    if command.is_some()
                        || interval.is_some()
                        || timeout.is_some()
                        || exit_codes.is_some()
                        || output.is_some()
                    {
                        return Err(de::Error::custom(
                            "readiness: \"delay\" cannot be combined with \"command\", \
                             \"interval\", \"timeout\", \"exit_codes\", or \"output\"",
                        ));
                    }
                    return Ok(Readiness::Delay(delay));
                }

                let command = command.ok_or_else(|| {
                    de::Error::custom("readiness: table form requires \"command\" or \"delay\" key")
                })?;

                Ok(Readiness::Command {
                    command,
                    interval,
                    timeout,
                    exit_codes,
                    output,
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
    /// When the readiness check passes: its command succeeds or its delay elapses.
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

    /// Readiness check; dependents wait until its command passes or its delay
    /// elapses. Mutually exclusive with `exit_codes`.
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

    /// Optional display name for the TUI tab and log prefix; the service key
    /// stays the canonical identifier. Falls back to the key when unset.
    #[serde(default)]
    pub name: Option<String>,

    /// Optional prefix/tab color: a named ANSI color (`red`, `bright-green`,
    /// …) or `#rrggbb` hex. Validated at load, parsed on demand by [`color`](Self::color).
    #[serde(default)]
    pub(super) color: Option<String>,

    /// The fully resolved environment, computed during
    /// [`Config::load`](super::Config::load); not from TOML. Private to this module.
    #[serde(skip)]
    pub(super) env: BTreeMap<String, String>,
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

/// Parses a humantime duration (`"10s"`, `"500ms"`); shared by the CLI
/// duration flags and TOML duration values so both render errors the same way.
pub fn parse_duration(s: &str) -> Result<Duration, String> {
    humantime::parse_duration(s).map_err(|e| format!("invalid duration {s:?}: {e}"))
}
