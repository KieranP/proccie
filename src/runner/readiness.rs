//! Readiness for one service: polls a shell command or an HTTP endpoint until
//! it passes (or the window times out), watches the process's own output, or
//! waits out a fixed delay, until shutdown intervenes. On [`Shared`].

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::watch;
use tokio::time::{MissedTickBehavior, interval, sleep};

use super::probe::{Needle, Probe, build_http_client, run_one_check};
use super::{DepState, Shared};
use crate::config::{
    DEFAULT_READINESS_INTERVAL, DEFAULT_READINESS_TIMEOUT, Process, Readiness, ReadyWhen,
};
use crate::service::ServiceStatus;

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

        // A delay is a plain timer and an output watch waits on the pump; only shell/http
        // probes drive the polling loop (their interval/timeout defaulting applied below).
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
                    output: output.clone().map(Needle::new),
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
                        status: status.clone(),
                        output: output.clone().map(Needle::new),
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

/// Resolves a configured interval/timeout: `d` when set and non-zero, else
/// `default`. Zero means "use the default" and guards `interval()` from panicking.
fn duration_or(d: Option<Duration>, default: Duration) -> Duration {
    d.filter(|d| !d.is_zero()).unwrap_or(default)
}
