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

/// HTTP status codes counted as ready. In TOML: a single integer or a list,
/// e.g. `status = 200` or `status = [200, 204]`. Defaults to `[200]` when unset.
#[derive(Debug, Clone)]
pub struct StatusCodes(pub Vec<u16>);

impl StatusCodes {
    /// Reports whether any expected status codes are configured.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// A readiness check; dependents wait until it completes. In TOML: a table
/// selecting one mode (`shell`/`http`/`output`/`delay`) with shared `interval`/`timeout`.
#[derive(Debug, Clone)]
pub enum Readiness {
    /// Poll a shell command until it passes: exit code in `exit_codes` (when set)
    /// and its stdout contains `output` (when set). Validation requires at least one.
    Shell {
        cmd: String,
        interval: Option<Duration>,
        timeout: Option<Duration>,
        exit_codes: Option<ExitCodes>,
        output: Option<String>,
    },
    /// Poll an HTTP endpoint until it responds with a status in `status` and,
    /// when set, a body containing `output`.
    Http {
        url: String,
        status: StatusCodes,
        output: Option<String>,
        interval: Option<Duration>,
        timeout: Option<Duration>,
    },
    /// Watch the process's own output (stdout and stderr), ready once it
    /// contains `output`.
    Output {
        output: String,
        timeout: Option<Duration>,
    },
    /// Sleep for a fixed duration, then release dependents.
    Delay(Duration),
}

/// The `readiness.shell` sub-table (TOML). `interval`/`timeout` are shared across
/// modes, so they live at the readiness top level, not here.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ShellSpec {
    cmd: String,
    #[serde(default)]
    exit_codes: Option<ExitCodes>,
    #[serde(default)]
    output: Option<String>,
}

/// The `readiness.http` sub-table (TOML). `interval`/`timeout` live at the
/// readiness top level.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct HttpSpec {
    url: String,
    #[serde(default)]
    status: Option<StatusCodes>,
    #[serde(default)]
    output: Option<String>,
}

/// A readiness duration: integer seconds, a positive float, or a humantime
/// string like `"500ms"`. Zero means "unset" (the default applies); negative errors.
#[derive(Debug)]
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

// A status is a single code or a list, so it needs a custom deserializer.
impl<'de> Deserialize<'de> for StatusCodes {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct StatusVisitor;

        impl<'de> Visitor<'de> for StatusVisitor {
            type Value = StatusCodes;

            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("an HTTP status code or a list of codes")
            }

            // TOML integers are always i64, so a single visit_i64 covers both the
            // single-code and (via visit_seq) list forms; no visit_u64 is needed.
            fn visit_i64<E: de::Error>(self, code: i64) -> Result<StatusCodes, E> {
                Ok(StatusCodes(vec![status_code(code)?]))
            }

            fn visit_seq<A: de::SeqAccess<'de>>(self, mut seq: A) -> Result<StatusCodes, A::Error> {
                let mut codes = Vec::new();
                while let Some(code) = seq.next_element::<i64>()? {
                    codes.push(status_code(code)?);
                }
                Ok(StatusCodes(codes))
            }
        }

        deserializer.deserialize_any(StatusVisitor)
    }
}

/// Narrows a raw TOML integer to a `u16`; an in-range-but-invalid code (outside
/// 100–599) is named by validation, while a `u16` overflow can only fail here.
fn status_code<E: de::Error>(code: i64) -> Result<u16, E> {
    u16::try_from(code)
        .map_err(|_| de::Error::custom(format!("status code out of range (got {code})")))
}

// Readiness is always a table selecting one mode, with shared interval/timeout.
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
                    "a readiness table with a \"shell\", \"http\", \"output\", or \"delay\" key",
                )
            }

            fn visit_map<M>(self, mut map: M) -> Result<Readiness, M::Error>
            where
                M: MapAccess<'de>,
            {
                let mut shell: Option<ShellSpec> = None;
                let mut http: Option<HttpSpec> = None;
                let mut output: Option<String> = None;
                let mut interval: Option<Duration> = None;
                let mut timeout: Option<Duration> = None;
                let mut delay: Option<Duration> = None;

                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "shell" => shell = Some(map.next_value()?),
                        "http" => http = Some(map.next_value()?),
                        "output" => output = Some(map.next_value()?),
                        // interval/timeout are shared: they apply to whichever mode is chosen.
                        "interval" => interval = map.next_value::<DurationValue>()?.0,
                        "timeout" => timeout = map.next_value::<DurationValue>()?.0,
                        // Zero is a valid immediate-ready delay, so keep it rather than dropping it.
                        "delay" => {
                            delay = Some(map.next_value::<DurationValue>()?.0.unwrap_or_default());
                        }
                        // Reject unknown keys so typos surface instead of being ignored.
                        _ => {
                            return Err(de::Error::unknown_field(
                                &key,
                                &["shell", "http", "output", "interval", "timeout", "delay"],
                            ));
                        }
                    }
                }

                // Exactly one mode may be selected; they are mutually exclusive.
                let selected = u8::from(shell.is_some())
                    + u8::from(http.is_some())
                    + u8::from(output.is_some())
                    + u8::from(delay.is_some());
                if selected > 1 {
                    return Err(de::Error::custom(
                        "readiness: \"shell\", \"http\", \"output\", and \"delay\" are \
                         mutually exclusive",
                    ));
                }

                if let Some(delay) = delay {
                    // A delay is its own complete timer; interval/timeout don't apply.
                    if interval.is_some() || timeout.is_some() {
                        return Err(de::Error::custom(
                            "readiness: \"delay\" cannot be combined with \"interval\" or \
                             \"timeout\"",
                        ));
                    }
                    return Ok(Readiness::Delay(delay));
                }
                if let Some(shell) = shell {
                    return Ok(Readiness::Shell {
                        cmd: shell.cmd,
                        interval,
                        timeout,
                        exit_codes: shell.exit_codes,
                        output: shell.output,
                    });
                }
                if let Some(http) = http {
                    return Ok(Readiness::Http {
                        url: http.url,
                        status: http.status.unwrap_or_else(|| StatusCodes(vec![200])),
                        output: http.output,
                        interval,
                        timeout,
                    });
                }
                if let Some(output) = output {
                    // Nothing is polled, so an interval has no meaning here.
                    if interval.is_some() {
                        return Err(de::Error::custom(
                            "readiness: \"interval\" applies to \"shell\"/\"http\" polling, not \
                             the \"output\" watch",
                        ));
                    }
                    return Ok(Readiness::Output { output, timeout });
                }
                Err(de::Error::custom(
                    "readiness requires \"shell\", \"http\", \"output\", or \"delay\"",
                ))
            }
        }

        deserializer.deserialize_map(ReadinessVisitor)
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
