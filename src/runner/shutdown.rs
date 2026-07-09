//! Global teardown: graceful and forced shutdown, the shared SIGKILL grace timer,
//! reaping stray groups, and signalling process groups. Methods on [`Shared`].

use std::sync::Arc;

use nix::sys::signal::{Signal, killpg};
use nix::unistd::Pid;

use super::Shared;

impl Shared {
    /// Graceful shutdown: cancel waiters, `SIGTERM` everything, then
    /// escalate to `SIGKILL` after the timeout.
    pub(crate) fn shutdown(self: &Arc<Self>) {
        {
            let mut state = self.lock_state();
            if state.shutting_down {
                return;
            }
            state.shutting_down = true;
        }

        self.system.info("shutting down all processes...");
        self.token.cancel();
        self.signal_all(Signal::SIGTERM, "SIGTERM");

        self.after_grace(false, |shared| {
            shared.signal_all(Signal::SIGKILL, "SIGKILL (timeout)");
        });
    }

    /// Spawns a task that runs `action` after the shutdown grace. With
    /// `wake_on_cancel`, a global shutdown cuts the wait short (leftovers are
    /// killed at once); otherwise the full grace always elapses first. The single
    /// SIGKILL-timer shape behind the shutdown, stray, and subtree-stop escalations.
    pub(crate) fn after_grace<F>(self: &Arc<Self>, wake_on_cancel: bool, action: F)
    where
        F: FnOnce(&Arc<Self>) + Send + 'static,
    {
        let shared = Arc::clone(self);
        let grace = self.shutdown_timeout;
        tokio::spawn(async move {
            if wake_on_cancel {
                tokio::select! {
                    () = shared.token.cancelled() => {}
                    () = tokio::time::sleep(grace) => {}
                }
            } else {
                tokio::time::sleep(grace).await;
            }
            action(&shared);
        });
    }

    /// Forced shutdown: `SIGKILL` everything right now.
    pub(crate) fn force_shutdown(&self) {
        self.system
            .warn("forced shutdown, sending SIGKILL to all processes...");
        self.signal_all(Signal::SIGKILL, "SIGKILL (forced)");
        // A hard exit may follow before run() returns, so reap leftovers now.
        self.reap_strays();
    }

    /// After the grace, `SIGKILL`s each group in `pids` still registered under its
    /// name — so an instance a restart has since relaunched (a fresh pgid) is left
    /// untouched. The escalation behind both a subtree stop and a mid-stop group.
    pub(crate) fn schedule_sigkill(self: &Arc<Self>, pids: Vec<(String, Pid)>) {
        self.after_grace(false, move |shared| {
            let live: Vec<(String, Pid)> = {
                let state = shared.lock_state();
                pids.into_iter()
                    .filter(|(name, pid)| state.groups.get(name) == Some(pid))
                    .collect()
            };
            for (name, pid) in live {
                shared.signal_group(&name, pid, Signal::SIGKILL, "SIGKILL (timeout)");
            }
        });
    }

    /// SIGKILLs a leftover group after the grace (or at once on shutdown), unless
    /// `reap_strays` already took it.
    pub(crate) fn escalate_stray(self: &Arc<Self>, name: String, group: Pid) {
        self.after_grace(true, move |shared| {
            if shared.lock_state().strays.remove(&group).is_some() {
                shared.kill_stray(&name, group);
            }
        });
    }

    /// SIGKILLs one leftover group; a gone group (it obeyed the SIGTERM) is a no-op.
    fn kill_stray(&self, name: &str, group: Pid) {
        if killpg(group, Signal::SIGKILL).is_ok() {
            self.system
                .warn(format!("{name}: killed leftover background process(es)"));
        }
    }

    /// SIGKILLs any leftover group not yet reaped by its own timer, so none
    /// outlives proccie. Called at run end and on a forced shutdown.
    pub(crate) fn reap_strays(&self) {
        let strays = std::mem::take(&mut self.lock_state().strays);
        for (group, name) in strays {
            self.kill_stray(&name, group);
        }
    }

    /// Sends `sig` to every tracked process group; `label` describes it for the
    /// per-service log (e.g. `SIGKILL (timeout)`).
    fn signal_all(&self, sig: Signal, label: &str) {
        let groups: Vec<(String, Pid)> = {
            let mut state = self.lock_state();
            // Recorded under the snapshot lock, so a group registering after
            // the sweep sees the kill and signals itself (see register_group).
            if sig == Signal::SIGKILL {
                state.killed = true;
            }
            state
                .groups
                .iter()
                .map(|(name, pid)| (name.clone(), *pid))
                .collect()
        };

        for (name, pgid) in groups {
            self.signal_group(&name, pgid, sig, label);
        }
    }

    /// Logs `label` on `name`'s own log, then sends `sig` to its process group.
    /// A gone (ESRCH) or recycled (EPERM) pgid just means the child already died.
    pub(crate) fn signal_group(&self, name: &str, pgid: Pid, sig: Signal, label: &str) {
        if let Some(svc) = self.service(name) {
            svc.logger().warn(format!("received {label}"));
        }
        match killpg(pgid, sig) {
            Ok(()) | Err(nix::errno::Errno::ESRCH | nix::errno::Errno::EPERM) => {}
            Err(e) => self.system.warn(format!("failed to signal {name}: {e}")),
        }
    }
}
