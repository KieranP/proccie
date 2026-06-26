//! Readiness polling for one service: runs its readiness command until it
//! passes, the window times out, or shutdown intervenes. Methods on [`Shared`].

use std::collections::BTreeMap;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::watch;
use tokio::time::{MissedTickBehavior, interval, sleep, timeout};

use super::{DepState, Shared};
use crate::config::{Process, ReadyWhen};
use crate::service::ServiceStatus;

/// Per-invocation timeout for a single readiness command execution.
const READINESS_CHECK_TIMEOUT: Duration = Duration::from_secs(5);

impl Shared {
    /// Runs the readiness command until exit 0, timeout (window opens at first
    /// spawn), or shutdown; a pass counts only for the child incarnation it probed.
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

        let total_timeout = readiness.timeout_or_default();
        let check_interval = readiness.interval_or_default();
        self.system.debug(format!(
            "{name}: polling readiness command (timeout {}, interval {}): {}",
            humantime::format_duration(total_timeout),
            humantime::format_duration(check_interval),
            readiness.command,
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
                run_one_check(&mut running, &readiness.command, env).await
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
    env: &BTreeMap<String, String>,
) -> bool {
    let incarnation = match running.wait_for(Option::is_some).await {
        Ok(incarnation) => *incarnation,
        Err(_) => return false,
    };
    run_readiness_check(command, env).await && *running.borrow() == incarnation
}

/// Executes the readiness command with the process's resolved environment,
/// returning true if it exits with code 0 within [`READINESS_CHECK_TIMEOUT`].
async fn run_readiness_check(command: &str, env: &BTreeMap<String, String>) -> bool {
    let mut cmd = super::shell_command(command, env);
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        // On drop (poller aborted mid-check), kill the `sh` rather than orphan it.
        .kill_on_drop(true);

    let Ok(mut child) = cmd.spawn() else {
        return false;
    };

    match timeout(READINESS_CHECK_TIMEOUT, child.wait()).await {
        Ok(Ok(status)) => status.success(),
        Ok(Err(_)) => false,
        Err(_) => {
            // Timed out; kill the lingering check process.
            let _ = child.start_kill();
            false
        }
    }
}
