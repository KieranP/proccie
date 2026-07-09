//! A service's lifecycle status and the display helpers (icon, noun, code) the
//! TUI reads off it.

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
