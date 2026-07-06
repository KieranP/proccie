//! Orchestrates the child-process lifecycle on Tokio: dependency-ordered
//! startup, shared coordination state, and global plus single-service shutdown.

mod deps;
mod lifecycle;
mod readiness;

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use nix::sys::signal::{Signal, killpg};
use nix::unistd::Pid;
use tokio::process::Command;
use tokio::sync::watch;
use tokio::task::{Id, JoinSet};
use tokio_util::sync::CancellationToken;

use crate::config::{Adjacency, dependents, topo_order};
use crate::logger::TaggedWriter;
use crate::service::{Service, ServiceStatus};
use crate::sync::MutexExt;

pub(crate) use deps::DepState;

/// Supervises all child processes. Cheaply cloneable; clones share the same
/// underlying state, so a clone can drive shutdown from a signal handler.
#[derive(Clone)]
pub struct Runner {
    shared: Arc<Shared>,
}

/// State shared across every process task. Fields read by the [`lifecycle`] and
/// [`readiness`] tasks are `pub(crate)`.
pub(crate) struct Shared {
    pub(crate) services: Arc<[Service]>,
    by_key: HashMap<String, usize>,
    adjacency: Adjacency,
    pub(crate) system: Arc<TaggedWriter>,
    /// Time to wait after SIGTERM before escalating to SIGKILL.
    shutdown_timeout: Duration,
    pub(crate) token: CancellationToken,
    /// Set when the program itself should terminate (a received signal), so the
    /// UI closes rather than staying open — distinct from stopping services.
    quit: AtomicBool,
    deps: HashMap<String, watch::Sender<DepState>>,
    state: Mutex<State>,
}

/// How a stopped service's live (or late-registering) group should be signalled.
#[derive(Clone, Copy, PartialEq, Eq)]
enum StopKind {
    /// Graceful: SIGTERM now, SIGKILL after the grace period.
    Term,
    /// Forced (a repeat stop): SIGKILL immediately.
    Kill,
}

/// Mutable runtime state guarded by a single mutex.
pub(crate) struct State {
    /// Live process groups, keyed by process name.
    groups: HashMap<String, Pid>,
    shutting_down: bool,
    /// True once a SIGKILL sweep ran; late-registering groups get SIGKILL too.
    killed: bool,
    /// Services stopped manually (or via a subtree stop) and how to signal them:
    /// their kills aren't failures, so they don't taint the exit code or shut down.
    stopped: HashMap<String, StopKind>,
    /// Leftover groups (a reaped leader's members) pending a SIGKILL, keyed by pgid.
    strays: HashMap<Pid, String>,
    exit_code: i32,
}

impl Runner {
    /// Creates a runner from the built services and their dependency adjacency,
    /// logging through `system`. `shutdown_timeout` is the SIGTERM-to-SIGKILL grace.
    pub fn new(
        services: Arc<[Service]>,
        adjacency: Adjacency,
        system: Arc<TaggedWriter>,
        shutdown_timeout: Duration,
    ) -> Runner {
        let by_key = services
            .iter()
            .enumerate()
            .map(|(i, svc)| (svc.key().to_owned(), i))
            .collect();
        let deps = services
            .iter()
            .map(|svc| (svc.key().to_owned(), watch::channel(DepState::Pending).0))
            .collect();

        Runner {
            shared: Arc::new(Shared {
                services,
                by_key,
                adjacency,
                system,
                shutdown_timeout,
                token: CancellationToken::new(),
                quit: AtomicBool::new(false),
                deps,
                state: Mutex::new(State {
                    groups: HashMap::new(),
                    shutting_down: false,
                    killed: false,
                    stopped: HashMap::new(),
                    strays: HashMap::new(),
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

        for name in topo_order(&self.shared.adjacency) {
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

        // Every task is done; kill any leftovers not yet reaped by their timers.
        self.shared.reap_strays();

        let (code, was_shutdown) = {
            let state = self.shared.lock_state();
            (state.exit_code, state.shutting_down)
        };
        // Confirm completion (the "shutting down…" opener otherwise has no close);
        // distinguish a shutdown from every process exiting on its own.
        if was_shutdown {
            self.shared.system.info("shutdown complete");
        } else {
            self.shared.system.info("all processes exited");
        }
        code
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

    /// Stops one service and its transitive dependents without a global shutdown.
    /// A repeat call escalates that subtree to SIGKILL immediately.
    pub fn stop_service(&self, name: &str) {
        self.shared.stop_service(name);
    }

    /// Requests that the program terminate (a received signal): the UI closes
    /// once it observes this, rather than staying open for log review.
    pub fn request_quit(&self) {
        self.shared.quit.store(true, Ordering::Relaxed);
        // Wake the UI so it observes the flag even with no log activity.
        self.shared.system.store().wake();
    }

    /// Whether termination has been requested via [`request_quit`](Self::request_quit).
    pub fn quit_requested(&self) -> bool {
        self.shared.quit.load(Ordering::Relaxed)
    }

    /// Whether any service still has a live process group or is yet to reach a
    /// terminal state — i.e. there is still something to stop.
    pub fn any_running(&self) -> bool {
        if !self.shared.lock_state().groups.is_empty() {
            return true;
        }
        self.shared
            .services
            .iter()
            .any(|svc| svc.status().is_active())
    }
}

impl Shared {
    /// Locks the shared state, recovering from poisoning so one panicked task
    /// can't cascade and defeat the per-task recovery in [`Runner::run`].
    pub(crate) fn lock_state(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock_recover()
    }

    /// Looks up a service by its canonical key.
    pub(crate) fn service(&self, key: &str) -> Option<&Service> {
        self.by_key.get(key).map(|&i| &self.services[i])
    }

    /// Whether `name` has been manually stopped (so its kill isn't a failure).
    pub(crate) fn is_stopped(&self, name: &str) -> bool {
        self.lock_state().stopped.contains_key(name)
    }

    /// Records the child's process group and marks it `Running`; if a shutdown/stop
    /// began first, signals it at once and leaves the status the stop set.
    pub(crate) fn register_group(&self, svc: &Service, pgid: Pid) {
        let name = svc.key();
        let racing_signal = {
            let mut state = self.lock_state();
            state.groups.insert(name.to_owned(), pgid);
            if state.killed || matches!(state.stopped.get(name), Some(StopKind::Kill)) {
                Some(Signal::SIGKILL)
            } else if state.shutting_down || state.stopped.contains_key(name) {
                Some(Signal::SIGTERM)
            } else {
                // Mark Running under this lock, so a racing stop isn't lost.
                svc.set_status(ServiceStatus::Running);
                None
            }
        };
        if let Some(sig) = racing_signal {
            self.signal_group(name, pgid, sig, sig.as_str());
        }
    }

    /// Drops the child's process group, returning its pgid (to sweep members the
    /// leader left behind) and whether it exited via shutdown or a manual stop.
    pub(crate) fn deregister_group(&self, name: &str) -> (Option<Pid>, bool) {
        let mut state = self.lock_state();
        let pgid = state.groups.remove(name);
        (
            pgid,
            state.shutting_down || state.stopped.contains_key(name),
        )
    }

    /// Terminal failure of one process: fail dependents, record `code`, and
    /// stop the run. Callers log the reason first.
    pub(crate) fn fail_run(self: &Arc<Self>, name: &str, code: i32) {
        self.signal_dep_result(name, DepState::Failed);
        self.set_exit_code(code);
        self.shutdown();
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

        self.system.info("shutting down all processes...");
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
        self.system
            .warn("forced shutdown, sending SIGKILL to all processes...");
        self.signal_all(Signal::SIGKILL, "SIGKILL (forced)");
        // A hard exit may follow before run() returns, so reap leftovers now.
        self.reap_strays();
    }

    /// SIGKILLs a leftover group after the grace (or at once on shutdown), unless
    /// `reap_strays` already took it.
    fn escalate_stray(self: &Arc<Self>, name: String, group: Pid) {
        let shared = Arc::clone(self);
        let grace = self.shutdown_timeout;
        tokio::spawn(async move {
            tokio::select! {
                () = shared.token.cancelled() => {}
                () = tokio::time::sleep(grace) => {}
            }
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
    fn reap_strays(&self) {
        let strays = std::mem::take(&mut self.lock_state().strays);
        for (group, name) in strays {
            self.kill_stray(&name, group);
        }
    }

    /// Stops `name` and its transitive dependents. First call: mark `stopped`, note
    /// it, unblock waiters, `SIGTERM` live groups, schedule `SIGKILL`. Repeat: `SIGKILL` now.
    fn stop_service(self: &Arc<Self>, name: &str) {
        if self.service(name).is_none() {
            return;
        }

        let targets = self.subtree(name);
        let (already, pids) = self.mark_subtree_stopped(name, &targets);

        if already {
            self.system.warn(format!(
                "force-stopping {name} and its dependents (SIGKILL)"
            ));
            for (target, pid) in &pids {
                self.signal_group(target, *pid, Signal::SIGKILL, "SIGKILL (forced)");
            }
            return;
        }

        self.system
            .info(format!("stopping {name} and its dependents..."));
        self.note_subtree_stopped(name, &targets);
        for (target, pid) in &pids {
            self.signal_group(target, *pid, Signal::SIGTERM, "SIGTERM");
        }
        self.schedule_subtree_sigkill(targets);
    }

    /// `name` plus everything that transitively depends on it.
    fn subtree(&self, name: &str) -> Vec<String> {
        let root = name.to_owned();
        let mut targets: Vec<String> = dependents(std::slice::from_ref(&root), &self.adjacency)
            .into_iter()
            .collect();
        targets.push(root);
        targets
    }

    /// Marks the subtree `stopped` (escalating to SIGKILL on a repeat stop of
    /// `name`); returns whether `name` was already stopped, plus the live groups.
    fn mark_subtree_stopped(&self, name: &str, targets: &[String]) -> (bool, Vec<(String, Pid)>) {
        let mut state = self.lock_state();
        // Already stopped (as its own root or as a dependent) means a repeat: escalate to SIGKILL.
        let already = state.stopped.contains_key(name);
        let kind = if already {
            StopKind::Kill
        } else {
            StopKind::Term
        };
        for target in targets {
            state.stopped.insert(target.clone(), kind);
        }
        let pids = targets
            .iter()
            .filter_map(|t| state.groups.get(t).map(|pid| (t.clone(), *pid)))
            .collect();
        (already, pids)
    }

    /// Marks each live target `Stopped`, annotates its tab with why, and
    /// unblocks anything still waiting on it.
    fn note_subtree_stopped(&self, name: &str, targets: &[String]) {
        for target in targets {
            // Skip targets that already exited; the atomic CAS keeps their status.
            if let Some(svc) = self.service(target)
                && svc.stop_if_active()
            {
                if target == name {
                    svc.note("Manually shut down.");
                } else {
                    svc.note(format!(
                        "Parent process `{name}` was manually shut down. Shutting down dependent process…"
                    ));
                }
            }
            self.signal_dep_result(target, DepState::Failed);
        }
    }

    /// After the shutdown grace period, `SIGKILL`s any of `targets` still alive.
    fn schedule_subtree_sigkill(self: &Arc<Self>, targets: Vec<String>) {
        let shared = Arc::clone(self);
        let timeout = self.shutdown_timeout;
        tokio::spawn(async move {
            tokio::time::sleep(timeout).await;
            // Snapshot live (name, pgid) pairs under the lock, then signal each.
            let groups: Vec<(String, Pid)> = {
                let state = shared.lock_state();
                targets
                    .iter()
                    .filter_map(|t| state.groups.get(t).map(|pid| (t.clone(), *pid)))
                    .collect()
            };
            for (name, pid) in groups {
                shared.signal_group(&name, pid, Signal::SIGKILL, "SIGKILL (timeout)");
            }
        });
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
    fn signal_group(&self, name: &str, pgid: Pid, sig: Signal, label: &str) {
        if let Some(svc) = self.service(name) {
            svc.logger().warn(format!("received {label}"));
        }
        match killpg(pgid, sig) {
            Ok(()) | Err(nix::errno::Errno::ESRCH | nix::errno::Errno::EPERM) => {}
            Err(e) => self.system.warn(format!("failed to signal {name}: {e}")),
        }
    }

    /// Records the exit code if a non-zero one has not already been set.
    fn set_exit_code(&self, code: i32) {
        let mut state = self.lock_state();
        if state.exit_code == 0 {
            state.exit_code = code;
        }
    }

    /// Handles a panicking process task: settle it, kill and drop the child the
    /// dead task left behind, fail dependents, then shut down the rest.
    fn recover_panic(self: &Arc<Self>, name: &str) {
        self.system.error(format!("panic in {name} task"));
        if let Some(svc) = self.service(name) {
            svc.finish_if_active(ServiceStatus::Failed(1));
        }
        // The dead task can't reap its child: SIGKILL the group before dropping it.
        let pgid = self.lock_state().groups.get(name).copied();
        if let Some(pgid) = pgid {
            self.signal_group(name, pgid, Signal::SIGKILL, "SIGKILL (panic)");
        }
        // The group was already SIGKILL'd above, so no self-exit sweep is needed.
        let _ = self.deregister_group(name);
        self.fail_run(name, 1);
    }
}

/// Runs a command via `sh -c` with the inherited environment cleared to only
/// the resolved `env`. Shared by process spawn and readiness checks.
fn shell_command(cmd: &str, env: &BTreeMap<String, String>) -> Command {
    let mut command = Command::new("sh");
    command.arg("-c").arg(cmd).env_clear().envs(env);
    command
}

/// Whether `haystack` contains `needle` as a byte substring. An empty needle
/// never matches (and guards `windows(0)`, which would panic). Shared by the
/// output-watch scanner and the shell/http output checks.
fn contains_bytes(haystack: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty()
        && needle.len() <= haystack.len()
        && haystack.windows(needle.len()).any(|w| w == needle)
}
