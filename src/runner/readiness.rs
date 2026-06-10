use std::collections::BTreeMap;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::watch;
use tokio::time::{MissedTickBehavior, interval, sleep, timeout};

use crate::config::Readiness;

use super::Shared;
use super::deps::DepState;

/// Per-invocation timeout for a single readiness command execution.
const READINESS_CHECK_TIMEOUT: Duration = Duration::from_secs(5);

impl Shared {
    /// Repeatedly runs the readiness command until it succeeds (exit 0),
    /// the timeout elapses, or a shutdown is requested. The timeout window
    /// opens when `running` first reports a successful spawn; checks pause
    /// while no live child exists, and a pass only counts for the child
    /// incarnation it probed, so a stale pass can't release dependents.
    pub(super) async fn poll_readiness(
        self: Arc<Self>,
        name: String,
        readiness: Readiness,
        env: BTreeMap<String, String>,
        mut running: watch::Receiver<Option<u64>>,
    ) {
        // Wait for the first successful launch so spawn time and earlier
        // failed attempts don't eat into the readiness window.
        tokio::select! {
            () = self.token.cancelled() => {
                self.readiness_cancelled(&name);
                return;
            }
            result = running.wait_for(Option::is_some) => {
                // Sender dropped without a launch: the process task already
                // failed its dependents, so there is nothing left to do.
                if result.is_err() {
                    return;
                }
            }
        }

        let total_timeout = readiness.timeout_or_default();
        let check_interval = readiness.interval_or_default();

        self.mux.system_log(format!(
            "{name}: polling readiness command (timeout {}, interval {}): {}",
            humantime::format_duration(total_timeout),
            humantime::format_duration(check_interval),
            readiness.command,
        ));

        let deadline = sleep(total_timeout);
        tokio::pin!(deadline);

        // The first tick fires at once, so at least one check always runs,
        // even when the timeout is shorter than the interval.
        let mut ticker = interval(check_interval);
        ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);

        loop {
            // Run one check, racing it against cancellation and the deadline.
            // A check only starts against a live child, and its pass only
            // counts while that same incarnation is still the live one.
            let attempt = async {
                ticker.tick().await;
                let incarnation = match running.wait_for(Option::is_some).await {
                    Ok(incarnation) => *incarnation,
                    Err(_) => return false,
                };
                run_readiness_check(&readiness.command, &env).await
                    && *running.borrow() == incarnation
            };

            tokio::select! {
                () = self.token.cancelled() => {
                    self.readiness_cancelled(&name);
                    return;
                }
                () = &mut deadline => {
                    self.mux.system_log(format!(
                        "{name}: readiness check timed out after {}, initiating shutdown",
                        humantime::format_duration(total_timeout),
                    ));
                    // A never-ready service is a startup failure, like
                    // exhausted retries.
                    self.fail_run(&name, 1);
                    return;
                }
                passed = attempt => {
                    if passed {
                        self.mux.system_log(format!("{name}: readiness check passed"));
                        self.signal_dep_result(&name, DepState::Ready);
                        return;
                    }
                }
            }
        }
    }

    /// Reports a cancelled readiness poll: log it and fail dependents.
    fn readiness_cancelled(&self, name: &str) {
        self.mux
            .system_log(format!("{name}: readiness check cancelled"));
        self.signal_dep_result(name, DepState::Failed);
    }
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
