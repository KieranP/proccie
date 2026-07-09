//! The readiness-check schema and its hand-written TOML deserialization: one
//! mode per table (shell/http/output/delay), a single-or-list HTTP status, and a
//! flexible integer/float/string duration.

use std::time::Duration;

use serde::Deserialize;
use serde::de::{self, Deserializer, MapAccess, Visitor};

use super::process::ExitCodes;
use crate::config::parse_duration;

/// HTTP status codes counted as ready. In TOML: a single integer or a list,
/// e.g. `status = 200` or `status = [200, 204]`. Defaults to `[200]` when unset.
#[derive(Debug, Clone)]
pub struct StatusCodes(pub Vec<u16>);

impl StatusCodes {
    /// Whether the given HTTP status code is in the expected set. Returns `false`
    /// if the set is empty (no status is expected).
    pub fn allows(&self, code: u16) -> bool {
        self.0.contains(&code)
    }

    /// Whether the expected-status-code set is empty (none configured).
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
                        // Zero is a valid immediate-ready delay, so keep it rather than drop it.
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
