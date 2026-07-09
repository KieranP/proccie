//! Orchestrates the child-process lifecycle on Tokio: dependency-ordered
//! startup, shared coordination state, and global plus single-service shutdown.

mod control;
mod deps;
mod exit;
mod lifecycle;
mod probe;
mod pump;
mod readiness;
mod shutdown;

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use nix::sys::signal::Signal;
use nix::unistd::Pid;
use tokio::process::Command;
use tokio::sync::{Notify, watch};
use tokio::task::{Id, JoinError, JoinSet};
use tokio_util::sync::CancellationToken;

use crate::config::{Adjacency, topo_order};
use crate::logger::TaggedWriter;
use crate::service::{Service, ServiceStatus};
use crate::sync::MutexExt;

pub(crate) use deps::DepState;

/// The outcome of joining one process task: its id and unit output, or the join
/// error if it panicked. `None` once the `JoinSet` is empty.
type JoinResult = Option<Result<(Id, ()), JoinError>>;

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
    /// The dependency-ordered start order, computed once (the graph is fixed) and
    /// reused for the initial launch and every restart relaunch.
    start_order: Vec<String>,
    state: Mutex<State>,
    /// Wakes the run loop when a restart is requested, so an already-exited
    /// service can relaunch without waiting on some other task to finish.
    restart_notify: Notify,
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
    /// Independent restart batches, each relaunched once none of its members is
    /// still live (the run loop's task-name set is the source of that truth). Kept
    /// separate so one slow-dying subtree can't stall another.
    restart_batches: Vec<HashSet<String>>,
    /// Restarts between requesting the stop and queuing their batch. Keeps the run
    /// loop from ending the run in that window when the last task exits meanwhile.
    restarts_arming: usize,
    /// Set once the run loop has decided to end (no tasks, no restart in flight);
    /// a restart is refused past this so it can't queue a batch into a dead loop.
    ended: bool,
}

impl State {
    /// Whether a restart is in flight: either mid-setup (arming) or queued to
    /// relaunch. The single definition of "still supervising a restart".
    fn restart_in_flight(&self) -> bool {
        self.restarts_arming > 0 || !self.restart_batches.is_empty()
    }
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
        let start_order = topo_order(&adjacency);
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
                start_order,
                state: Mutex::new(State {
                    groups: HashMap::new(),
                    shutting_down: false,
                    killed: false,
                    stopped: HashMap::new(),
                    strays: HashMap::new(),
                    exit_code: 0,
                    restart_batches: Vec::new(),
                    restarts_arming: 0,
                    ended: false,
                }),
                restart_notify: Notify::new(),
            }),
        }
    }

    /// Runs every process in dependency order until all exit, returning
    /// proccie's exit code (the first unexpected non-zero exit, or 0).
    pub async fn run(&self) -> i32 {
        let mut tasks: JoinSet<()> = JoinSet::new();
        let mut names: HashMap<Id, String> = HashMap::new();

        for name in &self.shared.start_order {
            self.spawn_process(&mut tasks, &mut names, name.clone());
        }

        loop {
            // Relaunch any restart batch whose whole subtree has exited (per the
            // live task names); an all-exited batch fires here without a join.
            let live: HashSet<&str> = names.values().map(String::as_str).collect();
            let ready = self.shared.take_ready_restarts(&live);
            for name in ready {
                self.spawn_process(&mut tasks, &mut names, name);
            }
            // Nothing left to supervise — but wait out a restart caught mid-setup
            // rather than ending the run and dropping its queued relaunch.
            if tasks.is_empty() {
                if self.shared.end_run_if_idle() {
                    break;
                }
                self.shared.restart_notify.notified().await;
                continue;
            }

            tokio::select! {
                // A restart request wakes the loop so the relaunch check above runs
                // even when no task happens to finish on its own.
                () = self.shared.restart_notify.notified() => {}
                // Join as tasks finish so a panic unblocks its dependents immediately.
                result = tasks.join_next_with_id() => self.reap_joined(&mut names, result),
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

    /// Restarts one service and its transitive dependents: stops them, then
    /// relaunches once they have all exited. A no-op once the run has finished.
    pub fn restart_service(&self, name: &str) {
        self.shared.restart_service(name);
    }

    /// Spawns one process task, mapping its task id to the service name — used for
    /// panic recovery and, via the value set, to tell which tasks are still live.
    /// Shared by startup and restart.
    fn spawn_process(
        &self,
        tasks: &mut JoinSet<()>,
        names: &mut HashMap<Id, String>,
        name: String,
    ) {
        let shared = Arc::clone(&self.shared);
        let task_name = name.clone();
        let handle = tasks.spawn(async move {
            shared.run_process(task_name).await;
        });
        names.insert(handle.id(), name);
    }

    /// Settles a finished task: drop its name mapping (so it no longer counts as
    /// live) and recover if it panicked (a dead task can't settle its own dependents).
    fn reap_joined(&self, names: &mut HashMap<Id, String>, result: JoinResult) {
        // `None` can't occur (the set is non-empty when polled), but handle it safely.
        let Some(result) = result else { return };
        let id = result.as_ref().map_or_else(JoinError::id, |&(id, ())| id);
        if let Some(name) = names.remove(&id)
            && result.is_err()
        {
            self.shared.recover_panic(&name);
        }
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
        {
            let state = self.shared.lock_state();
            // A live group, or a pending restart (mid-setup or queued): the latter
            // covers the gap between a restarted service exiting and relaunching.
            if !state.groups.is_empty() || state.restart_in_flight() {
                return true;
            }
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
    pub(crate) fn register_group(self: &Arc<Self>, svc: &Service, pgid: Pid) {
        let name = svc.key();
        // `escalate` flags a group that registered after the subtree stop's timer
        // captured pgids: it gets a SIGTERM but nothing else would ever SIGKILL it.
        let (racing_signal, escalate) = {
            let mut state = self.lock_state();
            state.groups.insert(name.to_owned(), pgid);
            if state.killed || matches!(state.stopped.get(name), Some(StopKind::Kill)) {
                (Some(Signal::SIGKILL), false)
            } else if state.shutting_down {
                // The global SIGKILL timer re-reads groups, so it covers this one.
                (Some(Signal::SIGTERM), false)
            } else if state.stopped.contains_key(name) {
                (Some(Signal::SIGTERM), true)
            } else {
                // Mark Running under this lock, so a racing stop isn't lost.
                svc.set_status(ServiceStatus::Running);
                (None, false)
            }
        };
        if let Some(sig) = racing_signal {
            self.signal_group(name, pgid, sig, sig.as_str());
        }
        if escalate {
            self.schedule_sigkill(vec![(name.to_owned(), pgid)]);
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

/// Whether `haystack` contains `needle` as a byte substring, matching exactly
/// when `case_sensitive` and ASCII-folding otherwise. An empty needle never
/// matches (the readiness policy treats one as "no check"). Shared by the
/// output-watch scanner and the shell/http output checks.
fn contains_bytes(haystack: &[u8], needle: &[u8], case_sensitive: bool) -> bool {
    !needle.is_empty() && crate::logger::bytes_contain(haystack, needle, case_sensitive)
}
