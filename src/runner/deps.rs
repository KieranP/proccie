use crate::config::{Process, ReadyWhen};

use super::Shared;

/// How a dependency resolved. Each process broadcasts exactly one terminal
/// state to everything waiting on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DepState {
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
    pub(super) async fn wait_for_deps(&self, name: &str, deps: &[String]) -> bool {
        for dep in deps {
            let dep_mode = self
                .config
                .processes()
                .get(dep)
                .map_or(ReadyWhen::Launched, Process::ready_when)
                .verb();

            self.mux
                .debug(format!("{name} waiting for {dep} to {dep_mode}..."));

            let Some(tx) = self.deps.get(dep) else {
                continue;
            };
            let mut rx = tx.subscribe();

            tokio::select! {
                () = self.token.cancelled() => {
                    self.mux.debug(format!("{name} cancelled while waiting for {dep}"));
                    return false;
                }
                result = rx.wait_for(|s| *s != DepState::Pending) => {
                    let ready = result.is_ok_and(|state| *state == DepState::Ready);
                    if !ready {
                        self.mux.warn(format!(
                            "{name}: dependency {dep} failed to become ready"
                        ));
                        return false;
                    }
                }
            }
        }

        true
    }

    /// Broadcasts a dependency result. Only the first call per process has
    /// any effect; later calls are ignored, waking all waiters at once.
    pub(super) fn signal_dep_result(&self, name: &str, state: DepState) {
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
}
