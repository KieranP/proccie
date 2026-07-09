//! Key handling for the log viewer: mapping keystrokes to scrolling, tab and
//! search actions, and the stop/restart/quit shutdown semantics.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::runner::Runner;
use crate::tui::search::{Search, SearchAction};

use super::{App, Tab};

impl App {
    /// Handles one key event, driving shutdown through `runner`. Returns whether
    /// the caller should arm a forced hard-exit (a repeat quit while tearing down).
    pub fn handle_key(&mut self, key: KeyEvent, runner: &Runner) -> bool {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        // Ctrl+C always drives shutdown, even with the search box open.
        if ctrl && matches!(key.code, KeyCode::Char('c')) {
            return self.handle_ctrl_c(runner);
        }
        // While editing, most keys edit the query; navigation falls through.
        if self.is_editing_search() && self.edit_search(key) {
            return false;
        }
        match key.code {
            KeyCode::Char('q') => self.request_quit(runner),
            KeyCode::Char('c') => self.close_active(),
            KeyCode::Char('r') => self.restart_active(runner),
            KeyCode::Char('s') => self.open_search(),
            // Esc clears a committed filter (an editing box handles its own Esc).
            KeyCode::Esc => self.clear_search(),
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

    /// Whether the search box is open and capturing keystrokes.
    fn is_editing_search(&self) -> bool {
        self.search.as_ref().is_some_and(Search::is_editing)
    }

    /// Opens the search box, carrying over a committed query so `s` re-edits it,
    /// and follows the newest matches.
    fn open_search(&mut self) {
        let query = self
            .search
            .take()
            .map(Search::into_query)
            .unwrap_or_default();
        self.search = Some(Search::editing(query));
        self.scroll_end();
    }

    /// Clears any search (a committed filter) and returns to the live tail.
    fn clear_search(&mut self) {
        if self.search.take().is_some() {
            self.scroll_end();
        }
    }

    /// Routes one key to the open box and applies its request. Returns whether
    /// the key was consumed (else it falls through so scrolling still works).
    fn edit_search(&mut self, key: KeyEvent) -> bool {
        let action = self
            .search
            .as_mut()
            .map_or(SearchAction::Pass, |search| search.edit(key));
        match action {
            SearchAction::Pass => false,
            SearchAction::Handled => true,
            SearchAction::Refilter => {
                self.scroll_end();
                true
            }
            SearchAction::Close => {
                self.search = None;
                self.scroll_end();
                true
            }
        }
    }

    /// `r` on a service tab: restart that service and its dependents. A no-op on
    /// the All tab, and once the supervisor has finished (nothing left to run).
    fn restart_active(&mut self, runner: &Runner) {
        if self.finished.is_some() {
            return;
        }
        if let Tab::Service(i) = self.active_tab() {
            runner.restart_service(self.services[i].key());
        }
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
