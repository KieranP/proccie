//! Per-service control without a global shutdown: stopping a service and its
//! dependents, and restarting them (stop, then relaunch once the subtree has
//! quiesced). Methods on [`Shared`].

use std::collections::HashSet;
use std::sync::Arc;

use nix::sys::signal::Signal;
use nix::unistd::Pid;

use crate::config::dependents;
use crate::service::ServiceStatus;

use super::{DepState, Shared, StopKind};

/// Why a subtree is being stopped. A restart stops exactly like a manual stop;
/// only the log and tab wording differ, and the run loop relaunches afterward.
#[derive(Clone, Copy)]
enum StopReason {
    Manual,
    Restart,
}

impl StopReason {
    /// The (plain, forced) verbs for this stop reason.
    fn verbs(self) -> (&'static str, &'static str) {
        match self {
            StopReason::Manual => ("stopping", "force-stopping"),
            StopReason::Restart => ("restarting", "force-restarting"),
        }
    }

    /// The first-stop announcement (info level).
    fn announce(self, name: &str) -> String {
        format!("{} {name} and its dependents...", self.verbs().0)
    }

    /// The repeat-stop escalation to SIGKILL (warn level).
    fn escalate(self, name: &str) -> String {
        format!("{} {name} and its dependents (SIGKILL)", self.verbs().1)
    }

    /// The per-tab note for a stopped member (`root` is the named service itself).
    /// A non-root member may depend on `name` only transitively, so it names `name`
    /// as the upstream trigger rather than claiming it as a direct dependency.
    fn note(self, name: &str, root: bool) -> String {
        match (self, root) {
            (StopReason::Manual, true) => "Manually shut down.".to_owned(),
            (StopReason::Manual, false) => format!(
                "Upstream service `{name}` was manually shut down. Shutting down dependent process…"
            ),
            (StopReason::Restart, true) => "Restarting…".to_owned(),
            (StopReason::Restart, false) => {
                format!("Upstream service `{name}` is restarting; restarting…")
            }
        }
    }
}

impl Shared {
    /// Ends the run only if nothing is left to supervise: no task and no restart in
    /// flight. Sets `ended` under the same lock so a concurrent [`restart_service`]
    /// either arms before this (and is seen here) or is refused after (returns
    /// `true` to break the run loop). Called only when the `JoinSet` is empty.
    pub(crate) fn end_run_if_idle(&self) -> bool {
        let mut state = self.lock_state();
        if state.restart_in_flight() {
            return false;
        }
        state.ended = true;
        true
    }

    /// Claims and returns (in start order) every restart batch that has fully
    /// quiesced (no member still among `live`). Empty until then; a global
    /// shutdown discards all batches.
    pub(crate) fn take_ready_restarts(&self, live: &HashSet<&str>) -> Vec<String> {
        // Relaunch in dependency order (the fixed start order, filtered to the batch).
        let ready = self.claim_ready_restarts(live);
        self.start_order
            .iter()
            .filter(|name| ready.contains(*name))
            .cloned()
            .collect()
    }

    /// Stops one service and its transitive dependents without a global shutdown.
    pub(crate) fn stop_service(self: &Arc<Self>, name: &str) {
        if self.service(name).is_some() {
            self.stop_subtree(name, StopReason::Manual);
        }
    }

    /// Restarts one service and its transitive dependents: stops the subtree, then
    /// queues it to relaunch once every member has exited (the run loop fires the
    /// batch). Refused once a global shutdown or the run's end is underway.
    pub(crate) fn restart_service(self: &Arc<Self>, name: &str) {
        if self.service(name).is_none() {
            return;
        }
        // Check-and-arm under one lock: the run loop sets `ended` under the same
        // lock, so a racing restart either arms first or is cleanly refused.
        {
            let mut state = self.lock_state();
            if state.shutting_down || state.ended {
                return;
            }
            state.restarts_arming += 1;
        }
        let targets = self.stop_subtree(name, StopReason::Restart);
        self.queue_restart(targets);
    }

    /// Claims every batch whose members have all exited (none still among `live`),
    /// re-arming each (clearing its stop mark, resetting its gate and status) under
    /// one lock so a claimed batch never looks idle, and returns their union. A
    /// batch with a still-live member is left queued; a shutdown discards them all.
    fn claim_ready_restarts(&self, live: &HashSet<&str>) -> HashSet<String> {
        let mut state = self.lock_state();
        if state.shutting_down {
            state.restart_batches.clear();
            return HashSet::new();
        }
        let mut claimed = HashSet::new();
        let mut pending = Vec::new();
        for batch in std::mem::take(&mut state.restart_batches) {
            if batch.iter().any(|n| live.contains(n.as_str())) {
                pending.push(batch);
            } else {
                // Every prior task has exited, so no stale signal can race the reset.
                for name in &batch {
                    state.stopped.remove(name);
                    self.reset_for_restart(name);
                }
                claimed.extend(batch);
            }
        }
        state.restart_batches = pending;
        claimed
    }

    /// Re-arms a restarting service: dependency gate back to `Pending` (clearing the
    /// `Stopped` a stop set) and status back to `Waiting`, so the relaunched task
    /// starts a clean lifecycle. Called under the state lock (touches only the gate
    /// channel and the status atom).
    fn reset_for_restart(&self, name: &str) {
        if let Some(tx) = self.deps.get(name) {
            tx.send_replace(DepState::Pending);
        }
        if let Some(svc) = self.service(name) {
            svc.set_status(ServiceStatus::Waiting);
        }
    }

    /// Stops `name`'s subtree and returns it. First stop: mark `stopped`, note it,
    /// unblock waiters, `SIGTERM` live groups, schedule `SIGKILL`. Repeat: `SIGKILL`
    /// now. Shared by [`stop_service`](Self::stop_service) and the restart path.
    fn stop_subtree(self: &Arc<Self>, name: &str, reason: StopReason) -> Vec<String> {
        let targets = self.subtree(name);
        let (already, pids) = self.mark_subtree_stopped(name, &targets);
        if already {
            self.system.warn(reason.escalate(name));
            for (target, pid) in &pids {
                self.signal_group(target, *pid, Signal::SIGKILL, "SIGKILL (forced)");
            }
        } else {
            self.system.info(reason.announce(name));
            self.note_subtree(name, &targets, reason);
            for (target, pid) in &pids {
                self.signal_group(target, *pid, Signal::SIGTERM, "SIGTERM");
            }
            self.schedule_sigkill(pids);
        }
        targets
    }

    /// Queues a just-stopped subtree as its own batch and clears the arming mark,
    /// waking the run loop to fire it once every member has exited (the wake covers
    /// an all-exited subtree, which no join would surface).
    fn queue_restart(&self, targets: Vec<String>) {
        {
            let mut state = self.lock_state();
            merge_restart_batch(&mut state.restart_batches, targets);
            state.restarts_arming -= 1;
        }
        self.restart_notify.notify_one();
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

    /// Marks each live subtree member `Stopped`, annotates its tab with why (per
    /// `reason`), and flips its dependency gate to `Stopped` so a member parked
    /// waiting on an upstream dependency observes its own stop at once.
    fn note_subtree(&self, name: &str, targets: &[String], reason: StopReason) {
        for target in targets {
            // Skip targets that already exited; the atomic CAS keeps their status.
            if let Some(svc) = self.service(target)
                && svc.stop_if_active()
            {
                svc.note(reason.note(name, target == name));
            }
            self.signal_dep_result(target, DepState::Stopped);
        }
    }
}

/// Adds `targets` as a restart batch, folding in any pending batch that overlaps
/// it so an overlapping restart relaunches together rather than deadlocking on a
/// member the other batch owns. Disjoint batches stay independent.
fn merge_restart_batch(batches: &mut Vec<HashSet<String>>, targets: Vec<String>) {
    let mut merged: HashSet<String> = targets.into_iter().collect();
    let mut kept = Vec::with_capacity(batches.len());
    for batch in std::mem::take(batches) {
        if batch.iter().any(|n| merged.contains(n)) {
            merged.extend(batch);
        } else {
            kept.push(batch);
        }
    }
    kept.push(merged);
    *batches = kept;
}
