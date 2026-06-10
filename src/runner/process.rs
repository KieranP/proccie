use std::fs::OpenOptions;
use std::io::{LineWriter, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::process::ExitStatusExt;
use std::process::{ExitStatus, Stdio};
use std::sync::Arc;

use nix::sys::signal::{Signal, killpg};
use nix::unistd::Pid;
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::net::unix::pipe;
use tokio::process::Child;
use tokio::sync::watch;

use crate::config::{Process, ReadyWhen};
use crate::mux::{Mux, PrefixWriter, StreamWriter};

use super::Shared;
use super::deps::DepState;

/// Permission mode for per-process log files.
const LOG_FILE_PERMS: u32 = 0o600;

/// How a single process execution ended.
enum RunResult {
    /// Exited with an expected code.
    Expected,
    /// Exited with an unexpected code (or never spawned), to propagate if terminal.
    Failed(i32),
    /// Exited because a shutdown was in progress.
    Shutdown,
}

impl Shared {
    /// Drives one process: wait for dependencies, then launch it with retries,
    /// failing dependents and shutting down once retries are exhausted.
    pub(super) async fn run_process(self: &Arc<Self>, name: String) {
        let Some(proc) = self.config.processes().get(&name).cloned() else {
            return;
        };

        if !self.wait_for_deps(&name, &proc.depends_on).await || self.token.is_cancelled() {
            self.signal_dep_result(&name, DepState::Failed);
            return;
        }

        let log_file = match self.open_process_log(&name, &proc) {
            Ok(file) => file,
            Err(()) => return,
        };
        let writer = self.mux.prefix_writer(&name, log_file);

        // Start the readiness poller once; its window opens at the first
        // successful launch and spans every retry. `running` carries the live
        // child's incarnation (the attempt number) or `None` while no child
        // runs, so a check can tell the child it probed is gone (or replaced).
        let readiness = if let ReadyWhen::ReadinessPass(readiness) = proc.ready_when() {
            let (tx, rx) = watch::channel(None);
            let task = tokio::spawn(Arc::clone(self).poll_readiness(
                name.clone(),
                readiness.clone(),
                proc.env().clone(),
                rx,
            ));
            Some((tx, task))
        } else {
            None
        };

        self.run_attempts(&name, &proc, &writer, readiness.as_ref().map(|(tx, _)| tx))
            .await;

        // Stop the poller; if a check is still in flight, kill_on_drop reaps it.
        if let Some((_, task)) = readiness {
            task.abort();
        }
    }

    /// Launches the process with retries, failing dependents and shutting down
    /// once retries are exhausted.
    async fn run_attempts(
        self: &Arc<Self>,
        name: &str,
        proc: &Process,
        writer: &Arc<PrefixWriter>,
        running: Option<&watch::Sender<Option<u64>>>,
    ) {
        let max_attempts = proc.max_retries.max(0).saturating_add(1);

        for attempt in 1..=max_attempts {
            if self.token.is_cancelled() {
                self.signal_dep_result(name, DepState::Failed);
                return;
            }
            if attempt > 1 {
                self.mux.warn(format!(
                    "{name}: retry {}/{}",
                    attempt - 1,
                    proc.max_retries
                ));
            }

            match self
                .run_once(name, proc, writer, running, attempt as u64)
                .await
            {
                RunResult::Expected | RunResult::Shutdown => return,
                RunResult::Failed(_) if attempt < max_attempts => {
                    continue;
                }
                RunResult::Failed(code) => {
                    self.fail_terminally(name, proc, code);
                    return;
                }
            }
        }
    }

    /// Runs the command once: spawn, stream output, release dependents, wait,
    /// then classify how it exited. `incarnation` identifies this attempt to
    /// the readiness poller.
    async fn run_once(
        self: &Arc<Self>,
        name: &str,
        proc: &Process,
        writer: &Arc<PrefixWriter>,
        running: Option<&watch::Sender<Option<u64>>>,
        incarnation: u64,
    ) -> RunResult {
        self.mux.info(format!("starting {name}: {}", proc.command));
        let (mut child, output) = match self.spawn_child(proc) {
            Ok(pair) => pair,
            Err(e) => {
                self.mux.error(format!("failed to start {name}: {e}"));
                // A process that never launched still failed the run.
                return RunResult::Failed(1);
            }
        };

        let pgid = Pid::from_raw(child.id().expect("spawned child has a pid") as i32);
        self.register_group(name, pgid);
        set_liveness(running, Some(incarnation));

        // One pump drains the shared pipe, preserving stdout/stderr write order.
        let out_task = tokio::spawn(pump(
            output,
            writer.stream(),
            Arc::clone(&self.mux),
            format!("{name} output"),
        ));
        self.release_dependents(name, proc);

        let status = child.wait().await;
        set_liveness(running, None);

        // The child is reaped, so its pgid may be recycled: drop the group at
        // once so no later signal sweep can hit an unrelated process.
        let is_shutdown = self.deregister_group(name);

        // Drain the remaining output and flush the trailing partial line.
        let _ = out_task.await;

        self.classify_exit(name, proc, &status, is_shutdown)
    }

    /// Builds and spawns the command in its own process group (pgid == pid).
    /// stdout and stderr share one pipe so interleaved lines keep their write
    /// order; the returned receiver is its read end.
    fn spawn_child(&self, proc: &Process) -> std::io::Result<(Child, pipe::Receiver)> {
        let (tx, rx) = pipe::pipe()?;
        let stdout = tx.into_blocking_fd()?;
        let stderr = stdout.try_clone()?;
        let child = super::shell_command(&proc.command, proc.env())
            .stdout(Stdio::from(stdout))
            .stderr(Stdio::from(stderr))
            .process_group(0)
            .spawn()?;
        Ok((child, rx))
    }

    /// Releases dependents the moment a launch-ready process launches; the
    /// other release moments (expected exit, readiness pass) are handled elsewhere.
    fn release_dependents(&self, name: &str, proc: &Process) {
        if matches!(proc.ready_when(), ReadyWhen::Launched) {
            self.signal_dep_result(name, DepState::Ready);
        }
    }

    /// Records the child's process group, signalling it at once if a shutdown
    /// began before registration (so no SIGTERM or SIGKILL sweep misses it).
    fn register_group(&self, name: &str, pgid: Pid) {
        let racing_signal = {
            let mut state = self.lock_state();
            state.groups.insert(name.to_owned(), pgid);
            if state.killed {
                Some(Signal::SIGKILL)
            } else if state.shutting_down {
                Some(Signal::SIGTERM)
            } else {
                None
            }
        };
        if let Some(sig) = racing_signal {
            let _ = killpg(pgid, sig);
        }
    }

    /// Drops the child's process group and reports whether a shutdown is underway.
    fn deregister_group(&self, name: &str) -> bool {
        let mut state = self.lock_state();
        state.groups.remove(name);
        state.shutting_down
    }

    /// Logs and classifies the exit, signalling readiness for an expected
    /// code. Signal deaths report the shell convention, 128 + signal number.
    fn classify_exit(
        &self,
        name: &str,
        proc: &Process,
        status: &std::io::Result<ExitStatus>,
        is_shutdown: bool,
    ) -> RunResult {
        if is_shutdown {
            self.mux.debug(format!("{name} exited (shutdown)"));
            return RunResult::Shutdown;
        }

        let signal = status.as_ref().ok().and_then(ExitStatusExt::signal);
        let exit_code = match (status, signal) {
            (Ok(s), None) => s.code().unwrap_or(1),
            (Ok(_), Some(sig)) => 128 + sig,
            (Err(_), _) => 1,
        };

        // Only a genuine exit can be expected: a signal death never matches,
        // even when its 128+N reporting code appears in exit_codes.
        if status.is_ok()
            && signal.is_none()
            && let ReadyWhen::ExpectedExit(codes) = proc.ready_when()
            && codes.allows(exit_code)
        {
            self.mux.info(format!(
                "{name} completed with expected exit code {exit_code}"
            ));
            self.signal_dep_result(name, DepState::Ready);
            return RunResult::Expected;
        }

        match signal {
            Some(sig) => self.mux.warn(format!(
                "{name} terminated by {} (code {exit_code})",
                signal_name(sig)
            )),
            None => self
                .mux
                .warn(format!("{name} exited with unexpected code {exit_code}")),
        }
        RunResult::Failed(exit_code)
    }

    /// Reports a process that exhausted its retries: fail dependents, record its
    /// exit code, and begin shutdown.
    fn fail_terminally(self: &Arc<Self>, name: &str, proc: &Process, code: i32) {
        if proc.max_retries > 0 {
            self.mux.error(format!(
                "{name}: all {} retries exhausted, initiating shutdown",
                proc.max_retries
            ));
        } else {
            self.mux
                .error(format!("{name} failed, initiating shutdown"));
        }
        self.fail_run(name, code);
    }

    /// Opens the per-process log file if configured; on failure, fails the
    /// process and starts shutdown, returning `Err`.
    fn open_process_log(
        self: &Arc<Self>,
        name: &str,
        proc: &Process,
    ) -> Result<Option<Box<dyn Write + Send>>, ()> {
        let Some(path) = &proc.log_file else {
            return Ok(None);
        };
        match open_log_file(path) {
            // LineWriter coalesces each prefixed line into one write.
            Ok(file) => Ok(Some(
                Box::new(LineWriter::new(file)) as Box<dyn Write + Send>
            )),
            Err(e) => {
                self.mux
                    .error(format!("failed to open log file for {name}: {e}"));
                self.fail_run(name, 1);
                Err(())
            }
        }
    }
}

/// Copies a child output stream into its line buffer until EOF or a read error,
/// flushing the trailing partial line and reporting any error.
async fn pump<R: AsyncRead + Unpin>(
    mut reader: R,
    stream: StreamWriter,
    mux: Arc<Mux>,
    label: String,
) {
    let mut buf = [0u8; 8192];
    loop {
        match reader.read(&mut buf).await {
            Ok(0) => break,
            Ok(n) => stream.write(&buf[..n]),
            Err(e) => {
                mux.warn(format!("error reading {label}: {e}"));
                break;
            }
        }
    }
    stream.flush();
}

/// Names a signal for log output (e.g. `SIGSEGV`), falling back to its number.
fn signal_name(sig: i32) -> String {
    Signal::try_from(sig).map_or_else(|_| format!("signal {sig}"), |s| s.as_str().to_owned())
}

/// Reports the live child's incarnation to the readiness poller:
/// `Some(attempt)` while that attempt's child runs, `None` once it exits.
fn set_liveness(running: Option<&watch::Sender<Option<u64>>>, incarnation: Option<u64>) {
    if let Some(tx) = running {
        tx.send_replace(incarnation);
    }
}

/// Opens a per-process log file for appending, creating it if needed.
fn open_log_file(path: &str) -> std::io::Result<std::fs::File> {
    OpenOptions::new()
        .create(true)
        .append(true)
        .mode(LOG_FILE_PERMS)
        .open(path)
}
