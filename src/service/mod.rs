//! The per-service object: config, identity, resolved color, and tagged log
//! writer. The runner and TUI both read everything off one `Service` handle.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use anstyle::Color;

use crate::config::{Adjacency, Config, Process, topo_order};
use crate::logger::{LogStore, Logger, TaggedWriter};
use crate::sync::MutexExt;
use crate::theme::Theme;

/// A service's lifecycle status, driving the tab icon and the close gate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ServiceStatus {
    /// Defined but not yet launched: still waiting on dependencies (or startup).
    Waiting,
    Running,
    /// Finished as expected (an allowed exit code, which may be non-zero); the
    /// tab is closeable.
    Completed(i32),
    /// Exited with an unexpected code (a failure).
    Failed(i32),
    /// Stopped by a manual or shutdown kill.
    Stopped,
}

impl ServiceStatus {
    /// Every lifecycle state in display order — the single source for tallying
    /// and labelling (carried codes are placeholders, compared by variant).
    pub const ALL: [ServiceStatus; 5] = [
        ServiceStatus::Running,
        ServiceStatus::Waiting,
        ServiceStatus::Completed(0),
        ServiceStatus::Failed(0),
        ServiceStatus::Stopped,
    ];

    /// The tab-bar status glyph, keyed to lifecycle (a borrowed constant so the
    /// common path allocates nothing per render).
    pub fn icon(self) -> &'static str {
        match self {
            ServiceStatus::Running => "●",
            ServiceStatus::Waiting => "○",
            ServiceStatus::Completed(_) => "✓",
            ServiceStatus::Failed(_) => "✗",
            ServiceStatus::Stopped => "■",
        }
    }

    /// The lifecycle noun for footer tallies (the code, if any, shows via
    /// [`code`](Self::code)).
    pub fn noun(self) -> &'static str {
        match self {
            ServiceStatus::Running => "running",
            ServiceStatus::Waiting => "waiting",
            ServiceStatus::Completed(_) => "completed",
            ServiceStatus::Failed(_) => "failed",
            ServiceStatus::Stopped => "stopped",
        }
    }

    /// The exit/failure code to show beside the icon, if noteworthy.
    pub fn code(self) -> Option<i32> {
        match self {
            ServiceStatus::Completed(c) if c != 0 => Some(c),
            ServiceStatus::Failed(c) => Some(c),
            _ => None,
        }
    }

    /// Whether the service is still waiting to start or running.
    pub fn is_active(self) -> bool {
        matches!(self, ServiceStatus::Waiting | ServiceStatus::Running)
    }

    /// Whether the service has reached a terminal state.
    pub fn is_terminal(self) -> bool {
        !self.is_active()
    }
}

/// One service: its config, identity, color, lifecycle status, and tagged
/// writer (which owns the log store and the optional log file).
pub struct Service {
    key: String,
    process: Arc<Process>,
    label: String,
    color: Color,
    /// A dimmed, background-adaptive neutral for per-service notes.
    note_color: Color,
    /// Lifecycle status, shared between the process task (writer) and the TUI.
    status: Mutex<ServiceStatus>,
    logger: Arc<TaggedWriter>,
}

impl Service {
    /// Builds one `Service` per defined process: resolves colors once, opens
    /// each log file, and gives each a tagged writer over its own store.
    pub fn build_all(
        config: &Config,
        adjacency: &Adjacency,
        logger: &Logger,
        theme: Theme,
    ) -> std::io::Result<Arc<[Service]>> {
        let colors = resolve_colors(config, adjacency, theme);
        config
            .processes()
            .iter()
            .map(|(key, proc)| {
                let label = proc.display_name(key).to_owned();
                let color = colors[key];
                let logger = open_writer(logger, &label, color, proc.log_file.as_deref())?;

                Ok(Service {
                    key: key.clone(),
                    process: Arc::new(proc.clone()),
                    color,
                    note_color: theme.faint(),
                    status: Mutex::new(ServiceStatus::Waiting),
                    logger,
                    label,
                })
            })
            .collect::<std::io::Result<Vec<Service>>>()
            .map(Arc::from)
    }

    /// The canonical identity (config key), used by deps, filters, and the runner.
    pub fn key(&self) -> &str {
        &self.key
    }

    /// The cosmetic display name for the tab and log prefix.
    pub fn label(&self) -> &str {
        &self.label
    }

    /// The resolved prefix/tab color (shared with the log lines).
    pub fn color(&self) -> Color {
        self.color
    }

    /// This service's tagged writer (write output, log diagnostics).
    pub fn logger(&self) -> &Arc<TaggedWriter> {
        &self.logger
    }

    /// The writer's log store (the reader side for the TUI).
    pub fn log(&self) -> &Arc<LogStore> {
        self.logger.store()
    }

    /// The wrapped process config (command, env, deps, readiness, …).
    pub fn process(&self) -> &Arc<Process> {
        &self.process
    }

    /// Total lines ever pushed (eviction-stable), for the unread mark.
    pub fn total(&self) -> u64 {
        self.log().total()
    }

    /// The current lifecycle status.
    pub fn status(&self) -> ServiceStatus {
        *self.status.lock_recover()
    }

    /// Whether this service finished as expected and its tab can be closed.
    pub fn is_closeable(&self) -> bool {
        matches!(self.status(), ServiceStatus::Completed(_))
    }

    /// Records the lifecycle status and wakes the UI.
    pub fn set_status(&self, status: ServiceStatus) {
        *self.status.lock_recover() = status;
        self.log().wake();
    }

    /// Atomically records a terminal `status` only from a non-terminal state
    /// (`Waiting`/`Running`), so the first of a concurrent stop/exit wins.
    pub fn finish_if_active(&self, status: ServiceStatus) -> bool {
        let mut current = self.status.lock_recover();
        if current.is_active() {
            *current = status;
            drop(current);
            self.log().wake();
            true
        } else {
            false
        }
    }

    /// Marks the service `Stopped` only from a non-terminal state (atomic), so a
    /// stop can't clobber a terminal status a concurrent exit just set.
    pub fn stop_if_active(&self) -> bool {
        self.finish_if_active(ServiceStatus::Stopped)
    }

    /// Emits a per-service note (an always-shown explanatory line, e.g. the
    /// manual-shutdown message); the writer routes it per destination.
    pub fn note(&self, msg: impl AsRef<str>) {
        self.logger.note(self.note_color, msg);
    }
}

/// Resolves each service's color: its configured `color`, else the next slot of
/// the theme's palette, assigned walking start order (not alphabetical key order).
fn resolve_colors(config: &Config, adjacency: &Adjacency, theme: Theme) -> BTreeMap<String, Color> {
    let palette = theme.palette();
    let mut idx = 0;
    let processes = config.processes();
    topo_order(adjacency)
        .into_iter()
        .filter_map(|key| {
            // Skip a name that is only a dependency (no process): never indexed.
            let proc = processes.get(&key)?;
            let color = proc.color().unwrap_or_else(|| {
                let color = palette[idx % palette.len()];
                idx += 1;
                color.into()
            });
            Some((key, color))
        })
        .collect()
}

/// Opens `label`'s tagged writer. A configured log file that can't be opened
/// must not sink the service: warn and fall back to logging without a file.
fn open_writer(
    logger: &Logger,
    label: &str,
    color: Color,
    log_file: Option<&str>,
) -> std::io::Result<Arc<TaggedWriter>> {
    logger.tagged_writer(label, color, log_file).or_else(|e| {
        logger.system().warn(format!(
            "{label}: cannot open log file {}: {e}; logging without a file",
            log_file.unwrap_or_default()
        ));
        logger.tagged_writer(label, color, None)
    })
}
