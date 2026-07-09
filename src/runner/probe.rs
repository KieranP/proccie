//! A single readiness probe — a shell command or an HTTP request — and running
//! it once. The poll loop in [`readiness`](super::readiness) drives these.

use std::collections::BTreeMap;
use std::process::Stdio;
use std::time::Duration;

use tokio::sync::watch;
use tokio::time::timeout;

use crate::config::{ExitCodes, StatusCodes};
use crate::logger::query_is_case_sensitive;

/// Per-invocation timeout for a single readiness probe (a command run or an
/// HTTP request).
const READINESS_CHECK_TIMEOUT: Duration = Duration::from_secs(5);

/// One polled readiness probe: a shell command or an HTTP request. Owns its
/// config so the poll loop can run it repeatedly without re-reading the config.
pub(crate) enum Probe {
    /// Run a command in the process's environment; pass on an allowed exit code
    /// (when set) and stdout containing `output` (when set).
    Shell {
        cmd: String,
        exit_codes: Option<ExitCodes>,
        output: Option<Needle>,
        env: BTreeMap<String, String>,
    },
    /// GET `url`; pass on a status in `status` and a body containing `output`
    /// (when set).
    Http {
        client: reqwest::Client,
        url: String,
        status: StatusCodes,
        output: Option<Needle>,
    },
}

impl Probe {
    /// A one-line description of the probe for the debug log.
    pub(crate) fn describe(&self) -> String {
        match self {
            Probe::Shell { cmd, .. } => format!("polling readiness command: {cmd}"),
            Probe::Http { url, .. } => format!("polling readiness endpoint: {url}"),
        }
    }

    /// Runs the probe once, returning whether it passed.
    async fn check(&self) -> bool {
        match self {
            Probe::Shell {
                cmd,
                exit_codes,
                output,
                env,
            } => run_shell_check(cmd, exit_codes.as_ref(), output.as_ref(), env).await,
            Probe::Http {
                client,
                url,
                status,
                output,
            } => run_http_check(client, url, status, output.as_ref()).await,
        }
    }
}

/// A readiness output substring with its smart-case decision computed once, so a
/// polled probe doesn't re-derive it every attempt. Matched against ANSI-stripped
/// bytes so a colored banner still matches a plain-text needle; smart-case (like
/// the log search): case-insensitive unless the needle has an uppercase letter.
pub(crate) struct Needle {
    text: String,
    case_sensitive: bool,
}

impl Needle {
    pub(crate) fn new(text: String) -> Needle {
        let case_sensitive = query_is_case_sensitive(&text);
        Needle {
            text,
            case_sensitive,
        }
    }

    /// The needle length in bytes (a caller that strips ANSI itself, e.g. the
    /// streaming output-watch scanner, uses it to size the tail it carries).
    pub(crate) fn byte_len(&self) -> usize {
        self.text.len()
    }

    /// Whether already-ANSI-stripped `bytes` contain the needle (its smart-case
    /// decision applied). The single home of the readiness match rule, shared by
    /// [`matches`](Self::matches) and the streaming output-watch scanner.
    pub(crate) fn contains(&self, stripped: &[u8]) -> bool {
        super::contains_bytes(stripped, self.text.as_bytes(), self.case_sensitive)
    }

    /// Whether `bytes`, once ANSI escapes are stripped, contain the needle.
    fn matches(&self, bytes: &[u8]) -> bool {
        self.contains(&anstream::adapter::strip_bytes(bytes).into_vec())
    }
}

/// Runs one probe, returning true only if it passes AND the child it probed is
/// still the live incarnation (not a since-exited or retried one).
pub(crate) async fn run_one_check(
    running: &mut watch::Receiver<Option<u64>>,
    probe: &Probe,
) -> bool {
    let incarnation = match running.wait_for(Option::is_some).await {
        Ok(incarnation) => *incarnation,
        Err(_) => return false,
    };
    probe.check().await && *running.borrow() == incarnation
}

/// Runs the command in the process's environment; passes if it finishes within
/// [`READINESS_CHECK_TIMEOUT`] with an allowed exit code and matching stdout.
async fn run_shell_check(
    command: &str,
    exit_codes: Option<&ExitCodes>,
    output: Option<&Needle>,
    env: &BTreeMap<String, String>,
) -> bool {
    let mut cmd = super::shell_command(command, env);
    cmd.stdin(Stdio::null())
        .stderr(Stdio::null())
        // On drop (poller aborted or timed out), kill the `sh` rather than orphan it.
        .kill_on_drop(true);
    // Capture stdout only when an output substring must be inspected.
    if output.is_some() {
        cmd.stdout(Stdio::piped());
    } else {
        cmd.stdout(Stdio::null());
    }

    let Ok(child) = cmd.spawn() else {
        return false;
    };

    // On timeout the future drops the child, and kill_on_drop reaps it.
    let Ok(Ok(result)) = timeout(READINESS_CHECK_TIMEOUT, child.wait_with_output()).await else {
        return false;
    };

    // An unset condition passes; when set, the exit code and stdout must both match.
    let code_ok =
        exit_codes.is_none_or(|codes| result.status.code().is_some_and(|c| codes.allows(c)));
    let output_ok = output.is_none_or(|needle| needle.matches(&result.stdout));
    code_ok && output_ok
}

/// Builds the shared client for HTTP probes, bounding each request by
/// [`READINESS_CHECK_TIMEOUT`]. Redirects aren't followed, so `status` reflects
/// the endpoint's own response rather than wherever it points.
pub(crate) fn build_http_client() -> reqwest::Result<reqwest::Client> {
    reqwest::Client::builder()
        .timeout(READINESS_CHECK_TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .build()
}

/// GETs `url`; passes on a status in `status` and, when set, a body containing
/// `output`. A request error (refused, DNS, TLS, timeout) counts as not ready.
async fn run_http_check(
    client: &reqwest::Client,
    url: &str,
    status: &StatusCodes,
    output: Option<&Needle>,
) -> bool {
    let Ok(response) = client.get(url).send().await else {
        return false;
    };
    if !status.allows(response.status().as_u16()) {
        return false;
    }
    // The status matched; when an output substring is required, the body must contain it.
    match output {
        None => true,
        Some(needle) => response
            .bytes()
            .await
            .is_ok_and(|body| needle.matches(&body)),
    }
}
