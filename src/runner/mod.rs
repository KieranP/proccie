//! Supervises the child-process lifecycle on Tokio: dependency-ordered startup,
//! readiness polling, retries, and graceful shutdown with signal escalation.

mod deps;
mod process;
mod readiness;

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use nix::sys::signal::Signal;
use nix::unistd::Pid;
use tokio::process::Command;
use tokio::sync::watch;
use tokio::task::{Id, JoinSet};
use tokio_util::sync::CancellationToken;

use crate::config::Config;
use crate::mux::Mux;

use deps::DepState;

/// Supervises all child processes. Cheaply cloneable; clones share the same
/// underlying state, so a clone can drive shutdown from a signal handler.
#[derive(Clone)]
pub struct Runner {
    shared: Arc<Shared>,
}

/// State shared across every process task.
struct Shared {
    config: Arc<Config>,
    mux: Arc<Mux>,
    /// Time to wait after SIGTERM before escalating to SIGKILL.
    shutdown_timeout: Duration,
    token: CancellationToken,
    deps: HashMap<String, watch::Sender<DepState>>,
    state: Mutex<State>,
}

/// Mutable runtime state guarded by a single mutex.
struct State {
    /// Live process groups, keyed by process name.
    groups: HashMap<String, Pid>,
    shutting_down: bool,
    /// True once a SIGKILL sweep ran; late-registering groups get SIGKILL too.
    killed: bool,
    exit_code: i32,
}

impl Runner {
    /// Creates a new runner for the given config, logging through `mux`.
    /// `shutdown_timeout` is the SIGTERM-to-SIGKILL grace period.
    pub fn new(config: Arc<Config>, mux: Arc<Mux>, shutdown_timeout: Duration) -> Runner {
        let deps = config
            .processes()
            .keys()
            .map(|name| (name.clone(), watch::channel(DepState::Pending).0))
            .collect();

        Runner {
            shared: Arc::new(Shared {
                config,
                mux,
                shutdown_timeout,
                token: CancellationToken::new(),
                deps,
                state: Mutex::new(State {
                    groups: HashMap::new(),
                    shutting_down: false,
                    killed: false,
                    exit_code: 0,
                }),
            }),
        }
    }

    /// Runs every process in dependency order until all exit, returning
    /// proccie's exit code (the first unexpected non-zero exit, or 0).
    pub async fn run(&self) -> i32 {
        let mut tasks: JoinSet<()> = JoinSet::new();
        let mut names: HashMap<Id, String> = HashMap::new();

        for name in self.shared.config.start_order() {
            let shared = Arc::clone(&self.shared);
            let task_name = name.clone();
            let handle = tasks.spawn(async move {
                shared.run_process(task_name).await;
            });
            names.insert(handle.id(), name);
        }

        // Join as tasks finish so a panic unblocks its dependents immediately.
        while let Some(result) = tasks.join_next_with_id().await {
            if let Err(err) = result
                && let Some(name) = names.get(&err.id())
            {
                self.shared.recover_panic(name);
            }
        }

        self.shared.lock_state().exit_code
    }

    /// Initiates a graceful shutdown. Safe to call repeatedly; only the
    /// first call takes effect.
    pub fn shutdown(&self) {
        self.shared.shutdown();
    }

    /// Immediately sends `SIGKILL` to every process group.
    pub fn force_shutdown(&self) {
        self.shared.force_shutdown();
    }
}

impl Shared {
    /// Locks the shared state, recovering from poisoning so one panicked task
    /// can't cascade and defeat the per-task recovery in [`Runner::run`].
    pub(super) fn lock_state(&self) -> std::sync::MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Graceful shutdown: cancel waiters, `SIGTERM` everything, then
    /// escalate to `SIGKILL` after the timeout.
    fn shutdown(self: &Arc<Self>) {
        {
            let mut state = self.lock_state();
            if state.shutting_down {
                return;
            }
            state.shutting_down = true;
        }

        self.mux.system_log("shutting down all processes...");
        self.token.cancel();
        self.signal_all(Signal::SIGTERM, "SIGTERM");

        let shared = Arc::clone(self);
        let timeout = self.shutdown_timeout;
        tokio::spawn(async move {
            tokio::time::sleep(timeout).await;
            shared.signal_all(Signal::SIGKILL, "SIGKILL (timeout)");
        });
    }

    /// Forced shutdown: `SIGKILL` everything right now.
    fn force_shutdown(&self) {
        self.mux
            .system_log("forced shutdown, sending SIGKILL to all processes...");
        self.signal_all(Signal::SIGKILL, "SIGKILL (forced)");
    }

    /// Sends `sig` to every tracked process group.
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
            self.mux.system_log(format!(
                "sending {label} to {name} (pgid {})",
                pgid.as_raw()
            ));

            match nix::sys::signal::killpg(pgid, sig) {
                Ok(()) | Err(nix::errno::Errno::ESRCH) => {}
                Err(e) => self.mux.system_log(format!("failed to signal {name}: {e}")),
            }
        }
    }

    /// Records the exit code if a non-zero one has not already been set.
    fn set_exit_code(&self, code: i32) {
        let mut state = self.lock_state();
        if state.exit_code == 0 {
            state.exit_code = code;
        }
    }

    /// Terminal failure of one process: fail dependents, record `code`, and
    /// stop the run. Callers log the reason first.
    fn fail_run(self: &Arc<Self>, name: &str, code: i32) {
        self.signal_dep_result(name, DepState::Failed);
        self.set_exit_code(code);
        self.shutdown();
    }

    /// Handles a panicking process task: log it, fail dependents, and shut
    /// down the rest.
    fn recover_panic(self: &Arc<Self>, name: &str) {
        self.mux.system_log(format!("panic in {name} task"));
        self.fail_run(name, 1);
    }
}

/// The one way proccie runs a command: via `sh -c`, with the inherited
/// environment cleared and only the resolved `env` set.
fn shell_command(cmd: &str, env: &BTreeMap<String, String>) -> Command {
    let mut command = Command::new("sh");
    command.arg("-c").arg(cmd).env_clear().envs(env);
    command
}
