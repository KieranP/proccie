//! Runs one service as an OS process: spawn in its own group, pump output,
//! retry on failure, and classify the exit. Methods on [`Shared`].

use std::os::unix::process::ExitStatusExt;
use std::process::{ExitStatus, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use nix::sys::signal::{Signal, killpg};
use nix::unistd::Pid;
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::net::unix::pipe;
use tokio::process::Child;
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tokio::time::sleep;

use super::{DepState, Shared};
use crate::config::{Process, ReadyWhen};
use crate::logger::TaggedWriter;
use crate::service::{Service, ServiceStatus};

/// Backoff between a failed attempt and its retry, so a process that crashes
/// immediately can't spin through its retries with no pause.
const RETRY_DELAY: Duration = Duration::from_millis(500);

/// Idle grace while draining output after a child exits: the pump is abandoned
/// once no further output arrives for this long (a lingering grandchild's open pipe).
const OUTPUT_DRAIN_GRACE: Duration = Duration::from_secs(2);

/// Absolute cap on draining, so a grandchild that keeps *writing* (never idle)
/// can't drain forever and hang the run; the idle grace handles the common case.
const OUTPUT_DRAIN_MAX: Duration = Duration::from_secs(10);

/// Chunks buffered between the async reader and the blocking stream writer; a
/// stalled consumer backs up through this to the pipe rather than growing unbounded.
const PUMP_QUEUE_DEPTH: usize = 256;

/// How a single process execution ended.
enum RunResult {
    /// Exited with an expected code.
    Expected,
    /// Exited cleanly (code 0) but wasn't configured to — retried like a failure,
    /// then terminal (as a completion, not a failure) once attempts are exhausted.
    Completed,
    /// Exited with an unexpected code (or never spawned), to propagate if terminal.
    Failed(i32),
    /// Exited because a shutdown or a manual stop was in progress.
    Shutdown,
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
        // Can't start (manual stop, failed dependency, or shutdown): mark it terminal and fail its waiters.
        if self.is_stopped(svc.key()) || !deps_ok || self.token.is_cancelled() {
            self.abandon(svc);
            return;
        }

        let readiness = self.spawn_readiness_poller(svc);
        self.run_attempts(svc, readiness.as_ref().map(|(tx, _)| tx))
            .await;

        // Stop the poller; if a check is still in flight, kill_on_drop reaps it.
        if let Some((_, task)) = readiness {
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
    /// returning its liveness sender (the live child's incarnation) and task handle.
    fn spawn_readiness_poller(
        self: &Arc<Self>,
        svc: &Service,
    ) -> Option<(watch::Sender<Option<u64>>, JoinHandle<()>)> {
        if !matches!(svc.process().ready_when(), ReadyWhen::ReadinessPass(_)) {
            return None;
        }
        let (tx, rx) = watch::channel(None);
        // Share the process by refcount rather than deep-cloning its env for the poll's lifetime.
        let task = tokio::spawn(Arc::clone(self).poll_readiness(
            svc.key().to_owned(),
            Arc::clone(svc.process()),
            rx,
        ));
        Some((tx, task))
    }

    /// Launches the process with retries, failing dependents and shutting down
    /// once retries are exhausted.
    async fn run_attempts(
        self: &Arc<Self>,
        svc: &Service,
        running: Option<&watch::Sender<Option<u64>>>,
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

            match self.run_once(svc, running, attempt.cast_unsigned()).await {
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
        // register_group marks the service Running under the state lock, so a racing stop isn't lost.
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

    /// Settles a genuine expected completion's status at exit (atomically, before
    /// output drains); the caller releases dependents once it returns `Some`.
    fn settle_expected(
        &self,
        svc: &Service,
        status_ok: bool,
        signal: Option<i32>,
        exit_code: i32,
    ) -> Option<RunResult> {
        let name = svc.key();
        // Not expected if signal-killed or stopped; the CAS lets a racing stop win.
        if status_ok
            && signal.is_none()
            && !self.is_stopped(name)
            && let ReadyWhen::ExpectedExit(codes) = svc.process().ready_when()
            && codes.allows(exit_code)
            && svc.finish_if_active(ServiceStatus::Completed(exit_code))
        {
            self.system.info(format!(
                "{name} completed with expected exit code {exit_code}"
            ));
            return Some(RunResult::Expected);
        }
        None
    }

    /// Classifies a non-completion exit: a shutdown or manual stop (including one
    /// that landed during the output drain) isn't a failure; otherwise it is.
    fn classify_exit(
        &self,
        svc: &Service,
        signal: Option<i32>,
        exit_code: i32,
        is_shutdown: bool,
    ) -> RunResult {
        let name = svc.key();

        if is_shutdown || self.is_stopped(name) {
            self.system.debug(format!("{name} exited (shutdown)"));
            // CAS like every terminal write, so an already-settled status survives.
            svc.finish_if_active(ServiceStatus::Stopped);
            return RunResult::Shutdown;
        }

        self.report_unexpected_exit(svc, signal, exit_code)
    }

    /// Classifies a non-allowed exit: a signal death or an exit outside a
    /// configured `exit_codes` set (0 included) fails; else a clean exit completes.
    fn report_unexpected_exit(
        &self,
        svc: &Service,
        signal: Option<i32>,
        exit_code: i32,
    ) -> RunResult {
        let name = svc.key();
        match signal {
            Some(sig) => {
                self.system.warn(format!(
                    "{name} terminated by {} (code {exit_code})",
                    signal_name(sig)
                ));
                RunResult::Failed(exit_code)
            }
            // Clean exit completes only without `exit_codes`; with them, 0 isn't
            // in the set here, so fail as code 1 (0 can't signal a failing run).
            None if exit_code == 0 => {
                if matches!(svc.process().ready_when(), ReadyWhen::ExpectedExit(_)) {
                    self.system
                        .warn(format!("{name} exited with unexpected code {exit_code}"));
                    RunResult::Failed(1)
                } else {
                    RunResult::Completed
                }
            }
            None => {
                self.system
                    .warn(format!("{name} exited with unexpected code {exit_code}"));
                RunResult::Failed(exit_code)
            }
        }
    }

    /// Ends the run on a clean exit of a process that wasn't meant to exit: a
    /// completion (not a failure), but it still stops the rest.
    fn complete_terminally(self: &Arc<Self>, svc: &Service) {
        let name = svc.key();
        self.system
            .info(format!("{name} exited cleanly, initiating shutdown"));
        // CAS so a concurrent manual stop that already marked it Stopped wins.
        svc.finish_if_active(ServiceStatus::Completed(0));
        self.fail_run(name, 0);
    }

    /// Reports a process that exhausted its retries (or never started): record
    /// the failure code and begin shutdown. The code is always non-zero.
    fn fail_terminally(self: &Arc<Self>, svc: &Service, code: i32) {
        debug_assert!(
            code != 0,
            "fail_terminally is for failures; code 0 is a completion"
        );
        let proc = svc.process();
        let name = svc.key();
        if proc.max_retries > 0 {
            self.system.error(format!(
                "{name}: all {} retries exhausted, initiating shutdown",
                proc.max_retries
            ));
        } else {
            self.system
                .error(format!("{name} failed, initiating shutdown"));
        }
        // CAS so a concurrent manual stop that already marked it Stopped wins.
        svc.finish_if_active(ServiceStatus::Failed(code));
        self.fail_run(name, code);
    }
}

/// Reports the live child's incarnation to the readiness poller:
/// `Some(attempt)` while that attempt's child runs, `None` once it exits.
fn set_liveness(running: Option<&watch::Sender<Option<u64>>>, incarnation: Option<u64>) {
    if let Some(tx) = running {
        tx.send_replace(incarnation);
    }
}

/// Copies a child output stream into the service's writer until EOF or a read
/// error, flushing the trailing partial line. `read` counts bytes pumped.
async fn pump<R: AsyncRead + Unpin>(
    mut reader: R,
    writer: Arc<TaggedWriter>,
    system: Arc<TaggedWriter>,
    label: String,
    read: Arc<AtomicU64>,
) {
    let mut buf = [0u8; 8192];

    if writer.is_store_mode() {
        // Store writes only touch memory, so run them inline on this task.
        while let Some(n) = read_step(&mut reader, &mut buf, &system, &label).await {
            read.fetch_add(n as u64, Ordering::Relaxed);
            writer.write(&buf[..n]);
        }
        writer.flush();
        return;
    }

    // Stream writes can block on a stalled consumer, so one blocking task drains a
    // bounded channel; a full channel backs pressure up to the pipe (and the child).
    let (tx, mut rx) = tokio::sync::mpsc::channel::<Vec<u8>>(PUMP_QUEUE_DEPTH);
    let sink_writer = Arc::clone(&writer);
    let sink = tokio::task::spawn_blocking(move || {
        while let Some(chunk) = rx.blocking_recv() {
            sink_writer.write(&chunk);
        }
        sink_writer.flush();
    });
    while let Some(n) = read_step(&mut reader, &mut buf, &system, &label).await {
        read.fetch_add(n as u64, Ordering::Relaxed);
        // A closed channel means the sink died; nothing more can be written.
        if tx.send(buf[..n].to_vec()).await.is_err() {
            break;
        }
    }
    // Dropping `tx` closes the channel, ending the sink (which does the final flush).
    drop(tx);
    let _ = sink.await;
}

/// One read into `buf`: returns the chunk length, or `None` on EOF or a (logged)
/// read error. The shared read/EOF/error step for both pump modes.
async fn read_step<R: AsyncRead + Unpin>(
    reader: &mut R,
    buf: &mut [u8],
    system: &TaggedWriter,
    label: &str,
) -> Option<usize> {
    match reader.read(buf).await {
        Ok(0) => None,
        Ok(n) => Some(n),
        Err(e) => {
            system.warn(format!("error reading {label}: {e}"));
            None
        }
    }
}

/// The exit code to report: the process's own code, the shell convention
/// 128 + signal for a signal death, or 1 when the status couldn't be obtained.
fn exit_code_of(status: &std::io::Result<ExitStatus>, signal: Option<i32>) -> i32 {
    match (status, signal) {
        (Ok(s), None) => s.code().unwrap_or(1),
        (Ok(_), Some(sig)) => 128 + sig,
        (Err(_), _) => 1,
    }
}

/// Drains the pump after the child exits, abandoning it once output stalls (a
/// lingering grandchild's idle pipe) or the absolute [`OUTPUT_DRAIN_MAX`] cap.
async fn drain_output(out_task: &mut JoinHandle<()>, read: &AtomicU64, svc: &Service) {
    let deadline = tokio::time::Instant::now() + OUTPUT_DRAIN_MAX;
    loop {
        let before = read.load(Ordering::Relaxed);
        // Wait up to the idle grace, but never past the absolute cap.
        let step =
            OUTPUT_DRAIN_GRACE.min(deadline.saturating_duration_since(tokio::time::Instant::now()));
        if tokio::time::timeout(step, &mut *out_task).await.is_ok() {
            return;
        }
        // Keep draining while output still flows and the cap allows it.
        if read.load(Ordering::Relaxed) != before && tokio::time::Instant::now() < deadline {
            continue;
        }
        out_task.abort();
        // Store mode flushes inline (abort skips it); the stream sink flushes on close.
        if svc.logger().is_store_mode() {
            svc.logger().flush();
        }
        return;
    }
}

/// Names a signal for log output (e.g. `SIGSEGV`), falling back to its number.
fn signal_name(sig: i32) -> String {
    Signal::try_from(sig).map_or_else(|_| format!("signal {sig}"), |s| s.as_str().to_owned())
}
