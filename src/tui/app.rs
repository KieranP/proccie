//! UI state for the tabbed log viewer: open tabs, active tab, per-tab scroll,
//! and per-service "seen" counts; reads logs through `Service` handles.

use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use crate::logger::LogStore;
use crate::runner::Runner;
use crate::service::Service;

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// A tab in the view: the combined All view, or one service (by index).
#[derive(Clone, Copy)]
pub enum Tab {
    All,
    Service(usize),
}

/// Per-tab scroll position; offset 0 means following the tail.
#[derive(Clone, Copy, Default)]
pub struct Scroll {
    pub offset_from_bottom: usize,
}

impl Scroll {
    /// Whether the view is pinned to the tail (newest lines).
    pub fn is_following(&self) -> bool {
        self.offset_from_bottom == 0
    }
}

/// The whole UI state.
pub struct App {
    pub services: Arc<[Service]>,
    pub system: Arc<LogStore>,
    pub prefix_width: usize,
    /// Open tabs, always starting with [`Tab::All`].
    tabs: Vec<Tab>,
    /// Per-tab scroll, parallel to `tabs`.
    scroll: Vec<Scroll>,
    active: usize,
    /// Highest `total` the user has actually viewed, per service key.
    seen: HashMap<String, u64>,
    /// Inner viewport height, refreshed each frame for scroll clamping.
    viewport: usize,
    pub quit: bool,
    /// proccie's exit code once the runner has finished.
    pub finished: Option<i32>,
    /// Whether a global stop has issued SIGTERM, so a repeat escalates to SIGKILL.
    global_stopped: bool,
    /// Whether the user asked to quit proccie (all services already down).
    quit_requested: bool,
}

impl App {
    /// Builds the initial state: an All tab plus one tab per service.
    pub fn new(services: Arc<[Service]>, system: Arc<LogStore>, prefix_width: usize) -> App {
        let mut tabs = vec![Tab::All];
        let mut scroll = vec![Scroll::default()];
        for i in 0..services.len() {
            tabs.push(Tab::Service(i));
            scroll.push(Scroll::default());
        }
        let seen = services.iter().map(|s| (s.key().to_owned(), 0)).collect();

        App {
            services,
            system,
            prefix_width,
            tabs,
            scroll,
            active: 0,
            seen,
            viewport: 0,
            quit: false,
            finished: None,
            global_stopped: false,
            quit_requested: false,
        }
    }

    /// Whether a global stop is in progress (drives the footer hint).
    pub fn is_shutting_down(&self) -> bool {
        self.global_stopped
    }

    /// Whether the user has asked to quit and proccie is finishing teardown.
    pub fn is_exiting(&self) -> bool {
        self.quit_requested
    }

    /// Records the runner's exit code; quits now if the user already asked to.
    pub fn on_runner_finished(&mut self, code: Option<i32>) {
        self.finished = code;
        if self.quit_requested {
            self.quit = true;
        }
    }

    /// The open tabs, in display order.
    pub fn tabs(&self) -> &[Tab] {
        &self.tabs
    }

    /// The index of the active tab.
    pub fn active(&self) -> usize {
        self.active
    }

    /// The active tab.
    pub fn active_tab(&self) -> Tab {
        self.tabs[self.active]
    }

    /// The service of the active tab, if it is a service tab.
    pub fn active_service(&self) -> Option<&Service> {
        match self.tabs[self.active] {
            Tab::Service(i) => Some(&self.services[i]),
            Tab::All => None,
        }
    }

    /// The active tab's scroll position.
    pub fn active_scroll(&self) -> Scroll {
        self.scroll[self.active]
    }

    /// Records the inner viewport height for scroll clamping.
    pub fn set_viewport(&mut self, height: usize) {
        self.viewport = height;
    }

    /// Pulls the active tab's scroll back within range, so eviction (or a
    /// shrunken viewport) can't strand the view past the top of the content.
    pub fn clamp_scroll(&mut self) {
        // Following the tail (offset 0) is always in range; skip the store-locking scan.
        if self.active_scroll().is_following() {
            return;
        }
        let max = self.max_offset();
        if self.active_scroll().offset_from_bottom > max {
            self.set_offset(max);
        }
    }

    /// Whether a service has output the user has not yet seen.
    pub fn has_unread(&self, svc: &Service) -> bool {
        svc.total() > self.seen.get(svc.key()).copied().unwrap_or(0)
    }

    /// Advances the seen counts once per frame, per the focus/scroll rules.
    pub fn mark_seen(&mut self) {
        match self.tabs[self.active] {
            // All shows everything: every service is considered seen.
            Tab::All => {
                for svc in self.services.iter() {
                    if let Some(seen) = self.seen.get_mut(svc.key()) {
                        *seen = svc.total();
                    }
                }
            }
            // A service tab at the tail marks only that service seen; scrolled
            // up, it leaves `seen` behind so new lines re-raise the mark.
            Tab::Service(i) => {
                if self.scroll[self.active].is_following() {
                    let svc = &self.services[i];
                    if let Some(seen) = self.seen.get_mut(svc.key()) {
                        *seen = svc.total();
                    }
                }
            }
        }
    }

    /// A cheap hash of everything `render` reads, so the event loop can skip a
    /// repaint when nothing visible changed (e.g. off-screen log activity).
    pub fn render_fingerprint(&self) -> u64 {
        let mut h = DefaultHasher::new();
        self.active.hash(&mut h);
        self.scroll[self.active].offset_from_bottom.hash(&mut h);
        self.viewport.hash(&mut h);
        self.finished.hash(&mut h);
        // Footer state, so a stop/quit repaints even before any service reacts.
        self.global_stopped.hash(&mut h);
        self.quit_requested.hash(&mut h);
        self.tabs.len().hash(&mut h);
        // Tab bar: each service's status icon and unread mark.
        for svc in self.services.iter() {
            svc.status().hash(&mut h);
            self.has_unread(svc).hash(&mut h);
        }
        // Active tab content: the line count feeding the visible window.
        match self.tabs[self.active] {
            Tab::All => self
                .all_stores()
                .map(|s| s.total())
                .sum::<u64>()
                .hash(&mut h),
            Tab::Service(i) => self.services[i].total().hash(&mut h),
        }
        h.finish()
    }

    /// Cycles to the next tab.
    pub fn next_tab(&mut self) {
        self.active = (self.active + 1) % self.tabs.len();
    }

    /// Cycles to the previous tab.
    pub fn prev_tab(&mut self) {
        self.active = (self.active + self.tabs.len() - 1) % self.tabs.len();
    }

    /// The stores feeding the All view, in render order: each service, then system.
    pub(crate) fn all_stores(&self) -> impl Iterator<Item = &Arc<LogStore>> {
        self.services
            .iter()
            .map(Service::log)
            .chain(std::iter::once(&self.system))
    }

    /// Scrolls up `n` lines (pausing follow).
    pub fn scroll_up(&mut self, n: usize) {
        let offset = self.active_scroll().offset_from_bottom + n;
        self.set_offset(offset.min(self.max_offset()));
    }

    /// Scrolls down `n` lines; reaching the bottom resumes follow.
    pub fn scroll_down(&mut self, n: usize) {
        let offset = self.active_scroll().offset_from_bottom.saturating_sub(n);
        self.set_offset(offset);
    }

    /// Jumps to the top (oldest lines).
    pub fn scroll_home(&mut self) {
        self.set_offset(self.max_offset());
    }

    /// Jumps to the bottom and resumes follow.
    pub fn scroll_end(&mut self) {
        self.set_offset(0);
    }

    /// Closes the active tab if it is a cleanly-finished service; otherwise a
    /// no-op (All, running, failed, or stopped services stay).
    pub fn close_active(&mut self) {
        let Tab::Service(i) = self.tabs[self.active] else {
            return;
        };
        if !self.services[i].is_closeable() {
            return;
        }
        let key = self.services[i].key().to_owned();
        self.tabs.remove(self.active);
        self.scroll.remove(self.active);
        self.seen.remove(&key);
        // The next tab slid into this slot; clamp only if we closed the last.
        self.active = self.active.min(self.tabs.len() - 1);
    }

    /// Handles one key event, driving shutdown through `runner`. Returns whether
    /// the caller should arm a forced hard-exit (a repeat quit while tearing down).
    pub fn handle_key(&mut self, key: KeyEvent, runner: &Runner) -> bool {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Char('c') if ctrl => return self.handle_ctrl_c(runner),
            KeyCode::Char('q') => self.request_quit(runner),
            KeyCode::Char('c') => self.close_active(),
            // Some terminals send Shift+Tab as Tab+SHIFT rather than BackTab.
            KeyCode::Tab if key.modifiers.contains(KeyModifiers::SHIFT) => self.prev_tab(),
            KeyCode::BackTab => self.prev_tab(),
            KeyCode::Tab => self.next_tab(),
            KeyCode::Up => self.scroll_up(1),
            KeyCode::Down => self.scroll_down(1),
            KeyCode::PageUp => self.scroll_up(self.viewport.max(1)),
            KeyCode::PageDown => self.scroll_down(self.viewport.max(1)),
            KeyCode::Home => self.scroll_home(),
            KeyCode::End => self.scroll_end(),
            _ => {}
        }
        false
    }

    /// Number of lines available in the active tab (for scroll clamping).
    fn content_len(&self) -> usize {
        match self.tabs[self.active] {
            Tab::All => self.all_stores().map(|s| s.len()).sum(),
            Tab::Service(i) => self.services[i].log().len(),
        }
    }

    /// The furthest the active tab can scroll up given the viewport.
    fn max_offset(&self) -> usize {
        self.content_len().saturating_sub(self.viewport)
    }

    /// Sets the active tab's scroll offset, deriving follow (offset 0 == tail).
    fn set_offset(&mut self, offset: usize) {
        self.scroll[self.active].offset_from_bottom = offset;
    }

    /// `q`: stop every service (SIGTERM, then SIGKILL after the timeout) and
    /// exit proccie once they have all shut down. A repeat press force-kills.
    fn request_quit(&mut self, runner: &Runner) {
        self.quit_requested = true;
        if runner.any_running() {
            self.escalate_global_stop(runner);
        } else if self.finished.is_some() {
            self.quit = true;
        }
    }

    /// Ctrl+C: stops services while any run (staying open for review), else quits;
    /// returns whether to arm a forced hard-exit.
    fn handle_ctrl_c(&mut self, runner: &Runner) -> bool {
        if runner.any_running() {
            match self.active_tab() {
                Tab::All => self.escalate_global_stop(runner),
                Tab::Service(i) => runner.stop_service(self.services[i].key()),
            }
            false
        } else {
            self.request_exit()
        }
    }

    /// All-tab Ctrl+C: SIGTERM every service, then SIGKILL on a repeat. proccie
    /// stays open so the user can read the logs afterwards.
    fn escalate_global_stop(&mut self, runner: &Runner) {
        if self.global_stopped {
            runner.force_shutdown();
        } else {
            runner.shutdown();
            self.global_stopped = true;
        }
    }

    /// Ctrl+C with nothing left running: quit proccie. Returns `true` to force a
    /// hard exit when a repeat press lands while teardown is still finishing.
    fn request_exit(&mut self) -> bool {
        if self.quit_requested {
            return true;
        }
        self.quit_requested = true;
        // Runner already done: exit now; else quit when teardown finishes.
        if self.finished.is_some() {
            self.quit = true;
        }
        false
    }
}
