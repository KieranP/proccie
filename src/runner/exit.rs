//! Classifying how a process exit resolves: settling an expected completion,
//! distinguishing a shutdown/stop from a failure, and the terminal outcomes that
//! end the run. Methods on [`Shared`], driven by [`lifecycle`](super::lifecycle).

use std::process::ExitStatus;
use std::sync::Arc;

use nix::sys::signal::Signal;

use super::Shared;
use crate::config::ReadyWhen;
use crate::service::{Service, ServiceStatus};

/// How a single process execution ended.
pub(crate) enum RunResult {
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
    /// Settles a genuine expected completion's status at exit (atomically, before
    /// output drains); the caller releases dependents once it returns `Some`.
    pub(crate) fn settle_expected(
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
    pub(crate) fn classify_exit(
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

    /// Ends the run on a clean exit of a process that wasn't meant to exit: a
    /// completion (not a failure), but it still stops the rest.
    pub(crate) fn complete_terminally(self: &Arc<Self>, svc: &Service) {
        let name = svc.key();
        self.system
            .info(format!("{name} exited cleanly, initiating shutdown"));
        // CAS so a concurrent manual stop that already marked it Stopped wins.
        svc.finish_if_active(ServiceStatus::Completed(0));
        self.fail_run(name, 0);
    }

    /// Reports a process that exhausted its retries (or never started): record
    /// the failure code and begin shutdown. The code is always non-zero.
    pub(crate) fn fail_terminally(self: &Arc<Self>, svc: &Service, code: i32) {
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
            // A clean (code 0) exit completes only when nothing expected it to stay
            // up; otherwise it's unexpected like any other code.
            None if exit_code == 0
                && !matches!(svc.process().ready_when(), ReadyWhen::ExpectedExit(_)) =>
            {
                RunResult::Completed
            }
            None => {
                self.system
                    .warn(format!("{name} exited with unexpected code {exit_code}"));
                // 0 can't signal a failing run, so an unexpected clean exit fails as 1.
                RunResult::Failed(if exit_code == 0 { 1 } else { exit_code })
            }
        }
    }
}

/// The exit code to report: the process's own code, the shell convention
/// 128 + signal for a signal death, or 1 when the status couldn't be obtained.
pub(crate) fn exit_code_of(status: &std::io::Result<ExitStatus>, signal: Option<i32>) -> i32 {
    match (status, signal) {
        (Ok(s), None) => s.code().unwrap_or(1),
        (Ok(_), Some(sig)) => 128 + sig,
        (Err(_), _) => 1,
    }
}

/// Names a signal for log output (e.g. `SIGSEGV`), falling back to its number.
fn signal_name(sig: i32) -> String {
    Signal::try_from(sig).map_or_else(|_| format!("signal {sig}"), |s| s.as_str().to_owned())
}
