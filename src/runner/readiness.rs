//! Readiness for one service: polls its command until it passes (or the window
//! times out), or waits out a fixed delay, until shutdown intervenes. On [`Shared`].

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

/// Per-invocation timeout for a single readiness command execution.
const READINESS_CHECK_TIMEOUT: Duration = Duration::from_secs(5);

impl Shared {
    /// Runs the command poll (until it passes, times out, or shutdown) or the
    /// delay timer; a command pass counts only for the child incarnation it probed.
    pub(crate) async fn poll_readiness(
        self: Arc<Self>,
        name: String,
        proc: Arc<Process>,
        mut running: watch::Receiver<Option<u64>>,
    ) {
        let ReadyWhen::ReadinessPass(readiness) = proc.ready_when() else {
            return;
        };
        let env = proc.env();

        // Wait for the first successful launch so spawn time and earlier failed
        // attempts don't eat into the readiness window.
        if !self.await_first_launch(&name, &mut running).await {
            return;
        }

        // A delay is a plain timer; the polling loop below applies only to commands.
        let (command, exit_codes, output, total_timeout, check_interval) = match readiness {
            Readiness::Delay(delay) => {
                self.await_readiness_delay(&name, *delay, &mut running)
                    .await;
                return;
            }
            Readiness::Command {
                command,
                interval,
                timeout,
                exit_codes,
                output,
            } => (
                command.as_str(),
                exit_codes.as_ref(),
                output.as_deref(),
                duration_or(*timeout, DEFAULT_READINESS_TIMEOUT),
                duration_or(*interval, DEFAULT_READINESS_INTERVAL),
            ),
        };

        self.system.debug(format!(
            "{name}: polling readiness command (timeout {}, interval {}): {}",
            humantime::format_duration(total_timeout),
            humantime::format_duration(check_interval),
            command,
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
                run_one_check(&mut running, command, exit_codes, output, env).await
            };

            tokio::select! {
                () = self.token.cancelled() => {
                    self.readiness_cancelled(&name);
                    return;
                }
                () = &mut deadline => {
                    // A manual stop isn't a startup failure: don't shut down the run.
                    if self.is_stopped(&name) {
                        self.readiness_cancelled(&name);
                    } else {
                        self.readiness_timed_out(&name, total_timeout);
                    }
                    return;
                }
                passed = attempt => {
                    // A stop can land mid-check; a stopped service must not release dependents.
                    if self.is_stopped(&name) {
                        self.readiness_cancelled(&name);
                        return;
                    }
                    if passed {
                        self.system.info(format!("{name}: readiness check passed"));
                        self.signal_dep_result(&name, DepState::Ready);
                        return;
                    }
                }
            }
        }
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
            () = sleep(delay) => {
                // A stop or exit can land during the delay; neither may release dependents.
                if self.is_stopped(name) || running.borrow().is_none() {
                    self.readiness_cancelled(name);
                } else {
                    self.system.info(format!("{name}: ready after delay"));
                    self.signal_dep_result(name, DepState::Ready);
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

    /// Reports a readiness timeout (a startup failure) and shuts down. The CAS
    /// gate lets a manual stop that raced the deadline win instead of failing.
    fn readiness_timed_out(self: &Arc<Self>, name: &str, total_timeout: Duration) {
        let settled = self
            .service(name)
            .is_some_and(|svc| svc.finish_if_active(ServiceStatus::Failed(1)));
        if !settled {
            // A concurrent stop/exit already marked it terminal: not a failure.
            self.readiness_cancelled(name);
            return;
        }
        self.system.error(format!(
            "{name}: readiness check timed out after {}, initiating shutdown",
            humantime::format_duration(total_timeout),
        ));
        self.fail_run(name, 1);
    }
}

/// Runs one readiness check, returning true only if it passes AND the child it
/// probed is still the live incarnation (not a since-exited or retried one).
async fn run_one_check(
    running: &mut watch::Receiver<Option<u64>>,
    command: &str,
    exit_codes: Option<&ExitCodes>,
    output: Option<&str>,
    env: &BTreeMap<String, String>,
) -> bool {
    let incarnation = match running.wait_for(Option::is_some).await {
        Ok(incarnation) => *incarnation,
        Err(_) => return false,
    };
    run_readiness_check(command, exit_codes, output, env).await && *running.borrow() == incarnation
}

/// Runs the command in the process's environment; passes if it finishes within
/// [`READINESS_CHECK_TIMEOUT`] with an allowed exit code and matching stdout.
async fn run_readiness_check(
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
    let output_ok =
        output.is_none_or(|needle| String::from_utf8_lossy(&result.stdout).contains(needle));
    code_ok && output_ok
}

/// Resolves a configured interval/timeout: `d` when set and non-zero, else
/// `default`. Zero means "use the default" and guards `interval()` from panicking.
fn duration_or(d: Option<Duration>, default: Duration) -> Duration {
    d.filter(|d| !d.is_zero()).unwrap_or(default)
}
