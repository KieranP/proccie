//! Runs one service as an OS process: spawn in its own group, pump output,
//! retry on failure, and classify the exit. Methods on [`Shared`].

use std::os::unix::process::ExitStatusExt;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use std::time::Duration;

use nix::sys::signal::{Signal, killpg};
use nix::unistd::Pid;
use tokio::net::unix::pipe;
use tokio::process::Child;
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tokio::time::sleep;

use super::exit::{RunResult, exit_code_of};
use super::probe::Needle;
use super::pump::{OutputWatch, drain_output, pump};
use super::{DepState, Shared};
use crate::config::{Process, Readiness, ReadyWhen};
use crate::service::Service;

/// Backoff between a failed attempt and its retry, so a process that crashes
/// immediately can't spin through its retries with no pause.
const RETRY_DELAY: Duration = Duration::from_millis(500);

/// The readiness poller's handoff to the run: the live child's incarnation sender,
/// and — in output-watch mode — the needle and the signal the pump trips on match.
pub(crate) struct ReadinessLink {
    running: watch::Sender<Option<u64>>,
    output: Option<OutputWatch>,
}

impl Shared {
    /// Drives one service: wait for dependencies, then launch it with retries,
    /// failing dependents and shutting down once retries are exhausted.
    pub(crate) async fn run_process(self: &Arc<Self>, name: String) {
        let Some(svc) = self.service(&name) else {
            return;
        };

        let deps_ok = self
            .wait_for_deps(svc.key(), &svc.process().depends_on)
            .await;
        // Can't start (stopped, failed dep, or shutdown): mark it terminal and fail its waiters.
        if self.is_stopped(svc.key()) || !deps_ok || self.token.is_cancelled() {
            self.abandon(svc);
            return;
        }

        // Split the link: the run keeps the liveness sender for its whole life, but
        // hands the output-watch sender to the pump so its EOF drops the last sender.
        let (running, output, task) = match self.spawn_readiness_poller(svc) {
            Some((ReadinessLink { running, output }, task)) => (Some(running), output, Some(task)),
            None => (None, None, None),
        };
        self.run_attempts(svc, running.as_ref(), output).await;

        // Stop the poller; if a check is still in flight, kill_on_drop reaps it.
        if let Some(task) = task {
            task.abort();
        }
    }

    /// Marks a service that never (re)started terminal — `Stopped`, unless a
    /// concurrent exit already set a terminal status — and fails its dependents.
    fn abandon(&self, svc: &Service) {
        svc.stop_if_active();
        self.signal_dep_result(svc.key(), DepState::Failed);
    }

    /// Spawns the readiness poller for a readiness-mode service (`None` otherwise),
    /// returning the link it shares with each run and its task handle.
    fn spawn_readiness_poller(
        self: &Arc<Self>,
        svc: &Service,
    ) -> Option<(ReadinessLink, JoinHandle<()>)> {
        let ReadyWhen::ReadinessPass(readiness) = svc.process().ready_when() else {
            return None;
        };
        let (tx, rx) = watch::channel(None);
        // Output-watch mode needs a channel the pump trips on match; other modes don't.
        let (output, matched_rx) = match readiness {
            Readiness::Output { output, .. } => {
                let (matched, matched_rx) = watch::channel(false);
                let watch = OutputWatch {
                    needle: Needle::new(output.clone()),
                    matched,
                };
                (Some(watch), Some(matched_rx))
            }
            _ => (None, None),
        };
        // Share the process by refcount rather than deep-cloning its env for the poll's lifetime.
        let task = tokio::spawn(Arc::clone(self).poll_readiness(
            svc.key().to_owned(),
            Arc::clone(svc.process()),
            rx,
            matched_rx,
        ));
        Some((
            ReadinessLink {
                running: tx,
                output,
            },
            task,
        ))
    }

    /// Launches the process with retries, failing dependents and shutting down
    /// once retries are exhausted.
    async fn run_attempts(
        self: &Arc<Self>,
        svc: &Service,
        running: Option<&watch::Sender<Option<u64>>>,
        mut output: Option<OutputWatch>,
    ) {
        let proc = svc.process();
        let name = svc.key();
        let max_attempts = proc.max_retries.saturating_add(1);

        for attempt in 1..=max_attempts {
            if self.token.is_cancelled() || self.is_stopped(name) {
                self.abandon(svc);
                return;
            }
            if attempt > 1 && !self.delay_retry(svc, attempt).await {
                self.abandon(svc);
                return;
            }

            // The watch is single-use: readiness (the only output-watch user) never
            // retries, so a later attempt would leave it `None` — the pump won't scan.
            match self
                .run_once(svc, running, output.take(), attempt.cast_unsigned())
                .await
            {
                RunResult::Expected | RunResult::Shutdown => return,
                // An unexpected exit — clean or not — is retried while attempts remain.
                RunResult::Completed | RunResult::Failed(_) if attempt < max_attempts => {}
                // Retries exhausted: the success/failure split was settled at exit.
                RunResult::Completed => {
                    self.complete_terminally(svc);
                    return;
                }
                RunResult::Failed(code) => {
                    self.fail_terminally(svc, code);
                    return;
                }
            }
        }
    }

    /// Announces retry `attempt` and waits out the backoff. Returns `false` if a
    /// stop or shutdown landed during the wait (the caller should abandon).
    async fn delay_retry(&self, svc: &Service, attempt: i64) -> bool {
        let name = svc.key();
        let max_retries = svc.process().max_retries;
        self.system
            .warn(format!("{name}: retry {}/{max_retries}", attempt - 1));
        svc.note(format!(
            "Process exited; retrying ({}/{max_retries})…",
            attempt - 1
        ));
        // Back off before retrying; bail out if a stop/shutdown lands.
        tokio::select! {
            () = self.token.cancelled() => {}
            () = sleep(RETRY_DELAY) => {}
        }
        !(self.token.is_cancelled() || self.is_stopped(name))
    }

    /// Runs the command once: spawn, stream output, release dependents, wait, then
    /// classify the exit. `incarnation` identifies this attempt to the readiness poller.
    async fn run_once(
        self: &Arc<Self>,
        svc: &Service,
        running: Option<&watch::Sender<Option<u64>>>,
        output_watch: Option<OutputWatch>,
        incarnation: u64,
    ) -> RunResult {
        let proc = svc.process();
        let name = svc.key();
        self.system
            .info(format!("starting {name}: {}", proc.command));
        let (mut child, output) = match Self::spawn_child(proc) {
            Ok(pair) => pair,
            Err(e) => {
                self.system.error(format!("failed to start {name}: {e}"));
                // A process that never launched still failed the run.
                return RunResult::Failed(1);
            }
        };

        let Some(pid) = child.id() else {
            // Unreachable (a fresh child has a pid), but never panic per the rules.
            self.system
                .error(format!("failed to start {name}: spawned child has no pid"));
            return RunResult::Failed(1);
        };
        let group = Pid::from_raw(pid.cast_signed());
        // register_group marks it Running under the state lock, so a racing stop isn't lost.
        self.register_group(svc, group);
        set_liveness(running, Some(incarnation));

        // One pump drains the shared pipe in write order; `read` counts bytes for the drain.
        let read = Arc::new(AtomicU64::new(0));
        let mut out_task = tokio::spawn(pump(
            output,
            Arc::clone(svc.logger()),
            Arc::clone(&self.system),
            format!("{name} output"),
            Arc::clone(&read),
            // In output-watch mode the pump scans the stream and trips the poller.
            output_watch,
        ));
        self.release_dependents(svc);

        let status = child.wait().await;
        let signal = status.as_ref().ok().and_then(ExitStatusExt::signal);
        let exit_code = exit_code_of(&status, signal);

        // The wait reaped the child; drop its (recyclable) pgid first.
        let (group, is_shutdown) = self.deregister_group(name);
        set_liveness(running, None);

        // Sweep any members the reaped leader left behind.
        if let Some(group) = group {
            self.sweep_group(name, group);
        }

        // Settle the completion status, then release dependents *before* the
        // drain so a lingering grandchild's open pipe can't delay their start.
        let settled = self.settle_expected(svc, status.is_ok(), signal, exit_code);
        if settled.is_some() {
            self.signal_dep_result(name, DepState::Ready);
        }

        drain_output(&mut out_task, &read, svc).await;

        match settled {
            Some(result) => result,
            None => self.classify_exit(svc, signal, exit_code, is_shutdown),
        }
    }

    /// Spawns the command in its own process group (pgid == pid); stdout and stderr
    /// share one pipe so interleaved lines keep write order. Returns the pipe's read end.
    fn spawn_child(proc: &Process) -> std::io::Result<(Child, pipe::Receiver)> {
        let (tx, rx) = pipe::pipe()?;
        let stdout = tx.into_blocking_fd()?;
        let stderr = stdout.try_clone()?;
        let child = super::shell_command(&proc.command, proc.env())
            // No stdin: a child mustn't detect a TTY and enter interactive mode.
            .stdin(Stdio::null())
            .stdout(Stdio::from(stdout))
            .stderr(Stdio::from(stderr))
            .process_group(0)
            // Reap the child on drop if the task unwinds before it's registered/killed.
            .kill_on_drop(true)
            .spawn()?;
        Ok((child, rx))
    }

    /// Releases dependents the moment a launch-ready process launches; the
    /// other release moments (expected exit, readiness pass) are handled elsewhere.
    fn release_dependents(&self, svc: &Service) {
        if matches!(svc.process().ready_when(), ReadyWhen::Launched) {
            self.signal_dep_result(svc.key(), DepState::Ready);
        }
    }

    /// SIGTERMs a reaped leader's leftover group members, then schedules their
    /// SIGKILL. An empty group (the common case) is a no-op.
    fn sweep_group(self: &Arc<Self>, name: &str, group: Pid) {
        // An error means the group is already empty: nothing was left behind.
        if killpg(group, Signal::SIGTERM).is_err() {
            return;
        }
        self.system.debug(format!(
            "{name}: terminating leftover background process(es)"
        ));
        self.lock_state().strays.insert(group, name.to_owned());
        self.escalate_stray(name.to_owned(), group);
    }
}

/// Reports the live child's incarnation to the readiness poller:
/// `Some(attempt)` while that attempt's child runs, `None` once it exits.
fn set_liveness(running: Option<&watch::Sender<Option<u64>>>, incarnation: Option<u64>) {
    if let Some(tx) = running {
        tx.send_replace(incarnation);
    }
}
