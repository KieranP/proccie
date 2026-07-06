//! Readiness for one service: polls a shell command or an HTTP endpoint until
//! it passes (or the window times out), watches the process's own output, or
//! waits out a fixed delay, until shutdown intervenes. On [`Shared`].

use std::collections::BTreeMap;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::watch;
use tokio::time::{MissedTickBehavior, interval, sleep, timeout};

use super::{DepState, Shared};
use crate::config::{
    DEFAULT_READINESS_INTERVAL, DEFAULT_READINESS_TIMEOUT, ExitCodes, Process, Readiness, ReadyWhen,
};
use crate::service::ServiceStatus;

/// Per-invocation timeout for a single readiness probe (a command run or an
/// HTTP request).
const READINESS_CHECK_TIMEOUT: Duration = Duration::from_secs(5);

/// One polled readiness probe: a shell command or an HTTP request. Owns its
/// config so the poll loop can run it repeatedly without re-reading the config.
enum Probe {
    /// Run a command in the process's environment; pass on an allowed exit code
    /// (when set) and stdout containing `output` (when set).
    Shell {
        cmd: String,
        exit_codes: Option<ExitCodes>,
        output: Option<String>,
        env: BTreeMap<String, String>,
    },
    /// GET `url`; pass on a status in `status` and a body containing `output`
    /// (when set).
    Http {
        client: reqwest::Client,
        url: String,
        status: Vec<u16>,
        output: Option<String>,
    },
}

impl Probe {
    /// A one-line description of the probe for the debug log.
    fn describe(&self) -> String {
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
            } => run_shell_check(cmd, exit_codes.as_ref(), output.as_deref(), env).await,
            Probe::Http {
                client,
                url,
                status,
                output,
            } => run_http_check(client, url, status, output.as_deref()).await,
        }
    }
}

impl Shared {
    /// Runs the readiness policy: poll a shell/http probe until it passes, watch
    /// the process's output, or wait out a delay — until it times out or shutdown
    /// intervenes. A probe pass counts only for the child incarnation it probed.
    pub(crate) async fn poll_readiness(
        self: Arc<Self>,
        name: String,
        proc: Arc<Process>,
        mut running: watch::Receiver<Option<u64>>,
        output_matched: Option<watch::Receiver<bool>>,
    ) {
        let ReadyWhen::ReadinessPass(readiness) = proc.ready_when() else {
            return;
        };

        // Wait for the first successful launch so spawn time and earlier failed
        // attempts don't eat into the readiness window.
        if !self.await_first_launch(&name, &mut running).await {
            return;
        }

        // A delay is a plain timer and an output watch waits on the pump; only the
        // shell/http probes drive the polling loop. Their shared interval/timeout
        // defaulting is applied once, after the probe is built.
        let (probe, interval, timeout) = match readiness {
            Readiness::Delay(delay) => {
                self.await_readiness_delay(&name, *delay, &mut running)
                    .await;
                return;
            }
            Readiness::Output { output, timeout } => {
                let total_timeout = duration_or(*timeout, DEFAULT_READINESS_TIMEOUT);
                self.await_output_match(&name, output, total_timeout, &mut running, output_matched)
                    .await;
                return;
            }
            Readiness::Shell {
                cmd,
                interval,
                timeout,
                exit_codes,
                output,
            } => (
                Probe::Shell {
                    cmd: cmd.clone(),
                    exit_codes: exit_codes.clone(),
                    output: output.clone(),
                    env: proc.env().clone(),
                },
                *interval,
                *timeout,
            ),
            Readiness::Http {
                url,
                status,
                output,
                interval,
                timeout,
            } => {
                let client = match build_http_client() {
                    Ok(client) => client,
                    // The probe can never run, so this is a startup failure like a
                    // timeout — shut down rather than orphan the running child.
                    Err(e) => {
                        self.fail_readiness_startup(
                            &name,
                            format!("{name}: cannot build HTTP client: {e}; initiating shutdown"),
                        );
                        return;
                    }
                };
                (
                    Probe::Http {
                        client,
                        url: url.clone(),
                        status: status.0.clone(),
                        output: output.clone(),
                    },
                    *interval,
                    *timeout,
                )
            }
        };

        let total_timeout = duration_or(timeout, DEFAULT_READINESS_TIMEOUT);
        let check_interval = duration_or(interval, DEFAULT_READINESS_INTERVAL);
        self.poll_probe(&name, &probe, total_timeout, check_interval, &mut running)
            .await;
    }

    /// Waits for the first successful launch, returning `false` (after its own
    /// cleanup) if the poll should stop first: cancelled, or it never launched.
    async fn await_first_launch(
        &self,
        name: &str,
        running: &mut watch::Receiver<Option<u64>>,
    ) -> bool {
        tokio::select! {
            () = self.token.cancelled() => {
                self.readiness_cancelled(name);
                false
            }
            result = running.wait_for(Option::is_some) => result.is_ok(),
        }
    }

    /// Sleeps for `delay`, then releases dependents — unless shutdown, a stop, or
    /// the child's exit lands first, which cancels the poll instead of reporting ready.
    async fn await_readiness_delay(
        self: &Arc<Self>,
        name: &str,
        delay: Duration,
        running: &mut watch::Receiver<Option<u64>>,
    ) {
        self.system.debug(format!(
            "{name}: readiness delay of {}",
            humantime::format_duration(delay),
        ));
        tokio::select! {
            () = self.token.cancelled() => self.readiness_cancelled(name),
            // A stop or exit can land during the delay; the gate withholds release then.
            () = sleep(delay) => self.release_if_live(name, running, "ready after delay"),
        }
    }

    /// Waits for the pump to signal the process's output contains `needle`,
    /// releasing dependents on match — unless shutdown, a stop, or timeout lands first.
    async fn await_output_match(
        self: &Arc<Self>,
        name: &str,
        needle: &str,
        total_timeout: Duration,
        running: &mut watch::Receiver<Option<u64>>,
        matched: Option<watch::Receiver<bool>>,
    ) {
        // The link always supplies the channel in output mode; without it, nothing
        // could ever signal a match, so fail rather than hang until the timeout.
        let Some(mut matched) = matched else {
            self.readiness_cancelled(name);
            return;
        };
        self.system.debug(format!(
            "{name}: watching output for readiness (timeout {}): {needle}",
            humantime::format_duration(total_timeout),
        ));

        let deadline = sleep(total_timeout);
        tokio::pin!(deadline);
        loop {
            tokio::select! {
                () = self.token.cancelled() => {
                    self.readiness_cancelled(name);
                    return;
                }
                () = &mut deadline => {
                    self.on_readiness_deadline(name, total_timeout);
                    return;
                }
                changed = matched.changed() => {
                    // Sender gone (output ended without a match): wait out the
                    // deadline or a shutdown; the child's exit drives shutdown anyway.
                    if changed.is_err() {
                        tokio::select! {
                            () = self.token.cancelled() => self.readiness_cancelled(name),
                            () = &mut deadline => self.on_readiness_deadline(name, total_timeout),
                        }
                        return;
                    }
                    if *matched.borrow_and_update() {
                        // A stop or the child's exit can land as the needle appears; the
                        // gate withholds release then.
                        self.release_if_live(name, running, "readiness check passed");
                        return;
                    }
                }
            }
        }
    }

    /// Polls `probe` on `check_interval` until it passes, `total_timeout` elapses,
    /// or shutdown intervenes; a pass counts only while its probed child lives.
    async fn poll_probe(
        self: &Arc<Self>,
        name: &str,
        probe: &Probe,
        total_timeout: Duration,
        check_interval: Duration,
        running: &mut watch::Receiver<Option<u64>>,
    ) {
        self.system.debug(format!(
            "{name}: {} (timeout {}, interval {})",
            probe.describe(),
            humantime::format_duration(total_timeout),
            humantime::format_duration(check_interval),
        ));

        let deadline = sleep(total_timeout);
        tokio::pin!(deadline);
        // The first tick fires at once, so at least one check always runs, even
        // when the timeout is shorter than the interval.
        let mut ticker = interval(check_interval);
        ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);

        loop {
            // Run one check; its pass counts only while its probed child lives.
            let attempt = async {
                ticker.tick().await;
                run_one_check(running, probe).await
            };

            tokio::select! {
                () = self.token.cancelled() => {
                    self.readiness_cancelled(name);
                    return;
                }
                () = &mut deadline => {
                    self.on_readiness_deadline(name, total_timeout);
                    return;
                }
                passed = attempt => {
                    // A stop can land mid-check; a stopped service must not release dependents.
                    if self.is_stopped(name) {
                        self.readiness_cancelled(name);
                        return;
                    }
                    if passed {
                        self.system.info(format!("{name}: readiness check passed"));
                        self.signal_dep_result(name, DepState::Ready);
                        return;
                    }
                }
            }
        }
    }

    /// Reports a cancelled readiness poll: log it and fail dependents.
    fn readiness_cancelled(&self, name: &str) {
        self.system
            .debug(format!("{name}: readiness check cancelled"));
        self.signal_dep_result(name, DepState::Failed);
    }

    /// Releases dependents if this incarnation is still the live child and the
    /// service isn't stopped (logging `ready_msg`); a raced stop/exit cancels instead.
    fn release_if_live(&self, name: &str, running: &watch::Receiver<Option<u64>>, ready_msg: &str) {
        if self.is_stopped(name) || running.borrow().is_none() {
            self.readiness_cancelled(name);
        } else {
            self.system.info(format!("{name}: {ready_msg}"));
            self.signal_dep_result(name, DepState::Ready);
        }
    }

    /// Handles an elapsed readiness deadline: a timeout is a startup failure that
    /// shuts down, unless a manual stop raced it (then cancel, without failing the run).
    fn on_readiness_deadline(self: &Arc<Self>, name: &str, total_timeout: Duration) {
        if self.is_stopped(name) {
            self.readiness_cancelled(name);
        } else {
            self.fail_readiness_startup(
                name,
                format!(
                    "{name}: readiness check timed out after {}, initiating shutdown",
                    humantime::format_duration(total_timeout),
                ),
            );
        }
    }

    /// Settles readiness as a startup failure (logging `reason`) and shuts down.
    /// The CAS gate lets a manual stop or exit that raced win instead — then just
    /// cancel, without failing the run.
    fn fail_readiness_startup(self: &Arc<Self>, name: &str, reason: String) {
        let settled = self
            .service(name)
            .is_some_and(|svc| svc.finish_if_active(ServiceStatus::Failed(1)));
        if !settled {
            self.readiness_cancelled(name);
            return;
        }
        self.system.error(reason);
        self.fail_run(name, 1);
    }
}

/// Runs one probe, returning true only if it passes AND the child it probed is
/// still the live incarnation (not a since-exited or retried one).
async fn run_one_check(running: &mut watch::Receiver<Option<u64>>, probe: &Probe) -> bool {
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
    output: Option<&str>,
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
    let output_ok = output.is_none_or(|needle| stripped_contains(&result.stdout, needle));
    code_ok && output_ok
}

/// Builds the shared client for HTTP probes, bounding each request by
/// [`READINESS_CHECK_TIMEOUT`]. Redirects aren't followed, so `status` reflects
/// the endpoint's own response rather than wherever it points.
fn build_http_client() -> reqwest::Result<reqwest::Client> {
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
    status: &[u16],
    output: Option<&str>,
) -> bool {
    let Ok(response) = client.get(url).send().await else {
        return false;
    };
    if !status.contains(&response.status().as_u16()) {
        return false;
    }
    // The status matched; when an output substring is required, the body must contain it.
    match output {
        None => true,
        Some(needle) => response
            .bytes()
            .await
            .is_ok_and(|body| stripped_contains(&body, needle)),
    }
}

/// Substring test against `bytes` with ANSI escapes stripped first, so a colored
/// banner still matches a plain-text needle. Byte-based, matching the output watch.
fn stripped_contains(bytes: &[u8], needle: &str) -> bool {
    let stripped = anstream::adapter::strip_bytes(bytes).into_vec();
    super::contains_bytes(&stripped, needle.as_bytes())
}

/// Resolves a configured interval/timeout: `d` when set and non-zero, else
/// `default`. Zero means "use the default" and guards `interval()` from panicking.
fn duration_or(d: Option<Duration>, default: Duration) -> Duration {
    d.filter(|d| !d.is_zero()).unwrap_or(default)
}
