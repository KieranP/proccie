use crate::config::ReadyWhen;

use super::Shared;

/// How a dependency resolved. Each process broadcasts exactly one terminal
/// state to everything waiting on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DepState {
    /// Not yet resolved.
    Pending,
    /// Ready: launched, exited with an expected code, or passed readiness.
    Ready,
    /// Failed to become ready.
    Failed,
}

impl Shared {
    /// Blocks until all dependencies are ready. Returns `false` if any
    /// dependency failed or a shutdown was requested.
    pub(crate) async fn wait_for_deps(&self, name: &str, deps: &[String]) -> bool {
        for dep in deps {
            if !self.wait_for_dep(name, dep).await {
                return false;
            }
        }
        true
    }

    /// Broadcasts a dependency result. Only the first call per process has
    /// any effect; later calls are ignored, waking all waiters at once.
    pub(crate) fn signal_dep_result(&self, name: &str, state: DepState) {
        if let Some(tx) = self.deps.get(name) {
            tx.send_if_modified(|current| {
                if *current == DepState::Pending {
                    *current = state;
                    true
                } else {
                    false
                }
            });
        }
    }

    /// Blocks until one dependency resolves. Returns `false` if it failed or a
    /// shutdown was requested; an unknown dependency is treated as satisfied.
    async fn wait_for_dep(&self, name: &str, dep: &str) -> bool {
        let dep_mode = self
            .service(dep)
            .map_or(ReadyWhen::Launched, |svc| svc.process().ready_when())
            .verb();
        self.system
            .debug(format!("{name} waiting for {dep} to {dep_mode}..."));

        let Some(tx) = self.deps.get(dep) else {
            return true;
        };
        let mut rx = tx.subscribe();

        tokio::select! {
            () = self.token.cancelled() => {
                self.system.debug(format!("{name} cancelled while waiting for {dep}"));
                false
            }
            result = rx.wait_for(|s| *s != DepState::Pending) => {
                let ready = result.is_ok_and(|state| *state == DepState::Ready);
                // A manual stop already explained itself via a per-tab note.
                if !ready && !self.is_stopped(dep) {
                    self.system
                        .warn(format!("{name}: dependency {dep} failed to become ready"));
                }
                ready
            }
        }
    }
}
