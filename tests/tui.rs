//! Tests for the TUI app state: tab cycling, the unread mark, and tab closing.
//! None need a real terminal — they drive `App` and the stores directly.

mod common;

use std::sync::Arc;
use std::time::Duration;

use anstyle::AnsiColor;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use proccie::config::Config;
use proccie::logger::{Destination, LogLevel, Logger, Source};
use proccie::runner::Runner;
use proccie::service::{Service, ServiceStatus};
use proccie::theme::Theme;
use proccie::tui::App;

use common::write_config;

fn color() -> anstyle::Color {
    AnsiColor::Cyan.into()
}

/// Builds an `App` plus the services it wraps, from inline TOML.
fn build_app(toml: &str) -> (App, Arc<[Service]>) {
    let (app, services, _runner) = build_app_runner(toml);
    (app, services)
}

/// Builds an `App`, its services, and a `Runner` over the same service handles
/// (so status changes are visible to both), from inline TOML.
fn build_app_runner(toml: &str) -> (App, Arc<[Service]>, Runner) {
    let (_dir, path) = write_config(toml);
    let config = Config::load(&path).expect("config loads");
    // Store mode so the system and per-service stores share one clock/redraw.
    let logger = Logger::new(
        Destination::Store,
        config.names(),
        LogLevel::Debug,
        Theme::Dark,
    );
    let adjacency = config.adjacency();
    let services =
        Service::build_all(&config, &adjacency, &logger, Theme::Dark).expect("build services");
    let system = Arc::clone(logger.system().store());
    let app = App::new(
        Arc::clone(&services),
        system,
        logger.pad_width(),
        Theme::Dark,
    );
    let runner = Runner::new(
        Arc::clone(&services),
        adjacency,
        Arc::clone(logger.system()),
        Duration::from_secs(1),
    );
    (app, services, runner)
}

/// A Ctrl+C key event.
fn ctrl_c() -> KeyEvent {
    KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)
}

/// A `q` key event.
fn q_key() -> KeyEvent {
    KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE)
}

/// An unmodified character key event.
fn char_key(c: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
}

/// A bare key event for a non-character key (Enter, Esc, arrows, …).
fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

const TWO: &str = "[a]\ncommand = \"true\"\n[b]\ncommand = \"true\"\n";

/// Pushes one line into a service's store (advancing its `total`).
fn emit(svc: &Service) {
    svc.log().push(
        Source {
            tag: svc.label().into(),
            level: None,
        },
        color(),
        "x".into(),
    );
}

#[test]
fn tabs_cycle_and_wrap() {
    let (mut app, _svc) = build_app(TWO);
    // All + a + b.
    assert_eq!(app.tabs().len(), 3);
    assert_eq!(app.active(), 0);

    app.next_tab();
    assert_eq!(app.active(), 1);
    app.next_tab();
    assert_eq!(app.active(), 2);
    app.next_tab();
    assert_eq!(app.active(), 0); // wrap forward

    app.prev_tab();
    assert_eq!(app.active(), 2); // wrap backward
}

#[test]
fn unread_follows_focus_and_scroll() {
    let (mut app, svc) = build_app(TWO);
    let b = &svc[1];

    // Output while All is active: marked seen, so no unread.
    emit(b);
    assert!(app.has_unread(b));
    app.mark_seen();
    assert!(!app.has_unread(b));

    // Focused on `a`, output to `b` raises b's mark.
    app.next_tab(); // active = service a
    emit(b);
    app.mark_seen();
    assert!(app.has_unread(b));

    // Viewing b's tab at the tail clears it.
    app.next_tab(); // active = service b
    app.mark_seen();
    assert!(!app.has_unread(b));

    // Scrolled up on b, new output re-raises the mark.
    app.set_viewport(0);
    app.scroll_up(1); // offset > 0 pauses following
    emit(b);
    app.mark_seen();
    assert!(app.has_unread(b));

    // Back at the bottom, it clears again.
    app.scroll_end();
    app.mark_seen();
    assert!(!app.has_unread(b));
}

#[test]
fn request_quit_marks_the_runner_for_termination() {
    let (_app, _svc, runner) = build_app_runner(TWO);
    assert!(!runner.quit_requested());
    runner.request_quit();
    assert!(runner.quit_requested());
}

#[tokio::test]
async fn request_quit_wakes_the_ui() {
    // The event loop only re-reads the quit flag when it wakes; with no log
    // activity (e.g. a signal after the run finished, at a high log level)
    // request_quit must wake it itself, or the UI would never close.
    let (app, _svc, runner) = build_app_runner(TWO);
    let redraw = app.system.redraw();

    runner.request_quit();
    // The wake stored a permit, so `notified()` resolves at once.
    assert!(
        tokio::time::timeout(Duration::from_millis(200), redraw.notified())
            .await
            .is_ok(),
        "request_quit did not wake the UI"
    );
}

#[test]
fn status_nouns_are_distinct_and_cover_every_state() {
    use std::collections::HashSet;
    // The footer tallies through ServiceStatus::ALL and labels via noun(): ALL
    // must list each state once, and the nouns must be distinct so counts for
    // different states never collapse together.
    let nouns: Vec<&str> = ServiceStatus::ALL.iter().map(|s| s.noun()).collect();
    let unique: HashSet<&str> = nouns.iter().copied().collect();
    assert_eq!(unique.len(), ServiceStatus::ALL.len());
    // The carried code never changes the noun (so e.g. Completed(0)/Completed(2) group).
    assert_eq!(ServiceStatus::Completed(0).noun(), "completed");
    assert_eq!(ServiceStatus::Completed(2).noun(), "completed");
    assert_eq!(ServiceStatus::Failed(1).noun(), "failed");
}

#[test]
fn ctrl_c_quits_proccie_only_once_nothing_is_running() {
    let (mut app, svc, runner) = build_app_runner(TWO);

    // Both services already terminal: nothing is running.
    svc[0].set_status(ServiceStatus::Completed(0));
    svc[1].set_status(ServiceStatus::Stopped);
    assert!(!runner.any_running());

    // Ctrl+C with nothing running asks to quit, but waits for the runner's
    // teardown before actually closing (no forced exit on the first press).
    assert!(!app.handle_key(ctrl_c(), &runner));
    assert!(app.is_exiting());
    assert!(!app.quit);

    // Once the runner reports it finished, the app quits.
    app.on_runner_finished(Some(0));
    assert!(app.quit);
}

#[test]
fn second_ctrl_c_during_teardown_arms_a_forced_exit() {
    let (mut app, svc, runner) = build_app_runner(TWO);

    // Nothing running, and the runner has not yet reported it finished.
    svc[0].set_status(ServiceStatus::Completed(0));
    svc[1].set_status(ServiceStatus::Stopped);
    assert!(!runner.any_running());

    // First Ctrl+C asks to quit (teardown pending), without forcing.
    assert!(!app.handle_key(ctrl_c(), &runner));
    assert!(app.is_exiting());
    assert!(!app.quit);

    // A second Ctrl+C while teardown is still finishing arms the forced hard-exit.
    assert!(app.handle_key(ctrl_c(), &runner));
}

#[tokio::test]
async fn ctrl_c_stops_services_then_stays_open_for_log_review() {
    let (mut app, svc, runner) = build_app_runner(TWO);

    // Services are running: Ctrl+C on the All tab stops them, it does not quit.
    svc[0].set_status(ServiceStatus::Running);
    svc[1].set_status(ServiceStatus::Running);
    app.handle_key(ctrl_c(), &runner);
    assert!(app.is_shutting_down());
    assert!(!app.quit);
    assert!(!app.is_exiting());

    // The services exit and the runner finishes: proccie stays open so the user
    // can read the logs.
    svc[0].set_status(ServiceStatus::Stopped);
    svc[1].set_status(ServiceStatus::Stopped);
    app.on_runner_finished(Some(0));
    assert!(!app.quit);

    // Now that nothing is running, a further Ctrl+C quits.
    app.handle_key(ctrl_c(), &runner);
    assert!(app.quit);
}

#[tokio::test]
async fn q_stops_services_then_exits_once_they_are_down() {
    let (mut app, svc, runner) = build_app_runner(TWO);
    svc[0].set_status(ServiceStatus::Running);
    svc[1].set_status(ServiceStatus::Running);

    // q begins shutting services down and marks proccie to exit, but waits for
    // the services to actually stop first.
    app.handle_key(q_key(), &runner);
    assert!(app.is_shutting_down());
    assert!(app.is_exiting());
    assert!(!app.quit);

    // Once the services stop and the runner finishes, proccie exits.
    svc[0].set_status(ServiceStatus::Stopped);
    svc[1].set_status(ServiceStatus::Stopped);
    app.on_runner_finished(Some(0));
    assert!(app.quit);
}

#[test]
fn search_box_opens_edits_and_commits() {
    let (mut app, _svc, runner) = build_app_runner(TWO);
    assert!(app.search_input().is_none());

    // `s` opens an empty box: it filters nothing until a query is typed.
    app.handle_key(char_key('s'), &runner);
    assert_eq!(app.search_input(), Some(""));
    assert!(app.filter_query().is_none());

    // Printable keys build the query — including ones that are commands normally.
    app.handle_key(char_key('q'), &runner);
    app.handle_key(char_key('x'), &runner);
    assert_eq!(app.search_input(), Some("qx"));
    assert_eq!(app.filter_query(), Some("qx"));
    assert!(!app.is_exiting(), "'q' typed into the box must not quit");

    // Enter commits: the box closes but the filter stays applied.
    app.handle_key(key(KeyCode::Enter), &runner);
    assert!(app.search_input().is_none(), "editor closed after commit");
    assert_eq!(app.filter_query(), Some("qx"), "filter still applied");

    // `s` reopens editing on the committed query; Esc then clears everything.
    app.handle_key(char_key('s'), &runner);
    assert_eq!(app.search_input(), Some("qx"));
    app.handle_key(key(KeyCode::Esc), &runner);
    assert!(app.search_input().is_none());
    assert!(app.filter_query().is_none());
}

#[test]
fn search_query_supports_cursor_editing() {
    let (mut app, _svc, runner) = build_app_runner(TWO);
    app.handle_key(char_key('s'), &runner);
    for c in "abd".chars() {
        app.handle_key(char_key(c), &runner);
    }
    assert_eq!(app.filter_query(), Some("abd"));

    // Left once (before 'd'), insert 'c' → "abcd".
    app.handle_key(key(KeyCode::Left), &runner);
    app.handle_key(char_key('c'), &runner);
    assert_eq!(app.filter_query(), Some("abcd"));

    // Home then Delete drops the first char → "bcd"; Backspace at the start no-ops.
    app.handle_key(key(KeyCode::Home), &runner);
    app.handle_key(key(KeyCode::Delete), &runner);
    assert_eq!(app.filter_query(), Some("bcd"));
    app.handle_key(key(KeyCode::Backspace), &runner);
    assert_eq!(app.filter_query(), Some("bcd"));
}

#[test]
fn cmd_arrows_jump_to_the_ends_of_the_query() {
    let (mut app, _svc, runner) = build_app_runner(TWO);
    app.handle_key(char_key('s'), &runner);
    for c in "bcd".chars() {
        app.handle_key(char_key(c), &runner);
    }

    // Cmd+Left jumps to the front; typing there prepends.
    app.handle_key(KeyEvent::new(KeyCode::Left, KeyModifiers::SUPER), &runner);
    app.handle_key(char_key('a'), &runner);
    assert_eq!(app.filter_query(), Some("abcd"));

    // Cmd+Right jumps to the end; typing there appends.
    app.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::SUPER), &runner);
    app.handle_key(char_key('e'), &runner);
    assert_eq!(app.filter_query(), Some("abcde"));
}

#[test]
fn readline_combos_jump_to_the_ends_of_the_query() {
    let (mut app, _svc, runner) = build_app_runner(TWO);
    app.handle_key(char_key('s'), &runner);
    for c in "bcd".chars() {
        app.handle_key(char_key(c), &runner);
    }

    // Ctrl+A jumps to the front; typing there prepends.
    app.handle_key(
        KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL),
        &runner,
    );
    app.handle_key(char_key('a'), &runner);
    assert_eq!(app.filter_query(), Some("abcd"));

    // Ctrl+E jumps to the end; typing there appends.
    app.handle_key(
        KeyEvent::new(KeyCode::Char('e'), KeyModifiers::CONTROL),
        &runner,
    );
    app.handle_key(char_key('e'), &runner);
    assert_eq!(app.filter_query(), Some("abcde"));
}

#[tokio::test]
async fn committed_filter_lets_commands_through_and_esc_clears() {
    let (mut app, _svc, runner) = build_app_runner(TWO);
    app.handle_key(char_key('s'), &runner);
    app.handle_key(char_key('x'), &runner);
    app.handle_key(key(KeyCode::Enter), &runner); // commit "x"
    assert_eq!(app.filter_query(), Some("x"));

    // With the box shut, `q` is a command again (not typed into the query).
    app.handle_key(q_key(), &runner);
    assert!(
        app.is_exiting(),
        "'q' should quit once the filter is committed"
    );

    // Esc clears the committed filter.
    app.handle_key(key(KeyCode::Esc), &runner);
    assert!(app.filter_query().is_none());
}

#[tokio::test]
async fn search_box_lets_ctrl_c_and_navigation_through() {
    let (mut app, svc, runner) = build_app_runner(TWO);
    app.handle_key(char_key('s'), &runner); // box open, active tab is All

    // Ctrl+C still drives shutdown rather than typing 'c' into the query.
    svc[0].set_status(ServiceStatus::Running);
    svc[1].set_status(ServiceStatus::Running);
    app.handle_key(ctrl_c(), &runner);
    assert!(app.is_shutting_down());
    assert_eq!(
        app.search_input(),
        Some(""),
        "Ctrl+C must not edit the query"
    );

    // Tab still switches tabs while the box is open (navigation falls through).
    let before = app.active();
    app.handle_key(key(KeyCode::Tab), &runner);
    assert_ne!(app.active(), before);
}

#[test]
fn close_only_clean_exited_service_tabs() {
    let (mut app, svc) = build_app(TWO);

    // All tab: closing is a no-op.
    app.close_active();
    assert_eq!(app.tabs().len(), 3);

    // Running service tab: still a no-op.
    app.next_tab(); // service a
    app.close_active();
    assert_eq!(app.tabs().len(), 3);

    // A failure is not closeable.
    svc[0].set_status(ServiceStatus::Failed(1));
    app.close_active();
    assert_eq!(app.tabs().len(), 3);

    // An expected non-zero completion is a success, so it is closeable too.
    svc[0].set_status(ServiceStatus::Completed(2));
    assert!(svc[0].is_closeable());

    // An expected completion closes, and focus moves to the next sibling (b
    // slides into the closed slot) rather than jumping back to All.
    svc[0].set_status(ServiceStatus::Completed(0));
    app.close_active();
    assert_eq!(app.tabs().len(), 2);
    assert_eq!(app.active(), 1);
}
