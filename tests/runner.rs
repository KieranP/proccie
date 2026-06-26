//! End-to-end tests for the process runner: exit-code handling, dependency
//! ordering, readiness checks, retries, and filtering.

mod common;

use std::sync::Arc;
use std::time::Duration;

use tempfile::TempDir;

use proccie::config::Config;
use proccie::runner::Runner;

use proccie::logger::LogLevel;

use common::{SharedBuf, build_logger, build_services, wait_for_output, write_config};

const TIMEOUT: Duration = Duration::from_secs(5);

/// Wraps a loaded config in a runner with test defaults, capturing its log output.
fn build_runner(config: Config) -> (Runner, SharedBuf) {
    let (logger, out) = build_logger(&config.names(), LogLevel::Debug);
    let services = build_services(&config, &logger);
    let runner = Runner::new(
        services,
        config.adjacency(),
        Arc::clone(logger.system()),
        Duration::from_secs(2),
    );
    (runner, out)
}

/// Builds a runner from inline TOML, capturing its log output. The returned
/// `TempDir` must stay alive for the duration of the test.
fn make_runner(content: &str) -> (Runner, SharedBuf, TempDir) {
    let (dir, path) = write_config(content);
    let config = Config::load(&path).expect("config loads");
    let (runner, out) = build_runner(config);
    (runner, out, dir)
}

/// Spawns `run()` on a background task, returning its join handle.
fn run_in_background(runner: &Runner) -> tokio::task::JoinHandle<i32> {
    let runner = runner.clone();
    tokio::spawn(async move { runner.run().await })
}

// --- exit-code handling ---

#[tokio::test]
async fn expected_exit_code_does_not_trigger_shutdown() {
    let (runner, out, _dir) = make_runner(
        r#"
[main]
command = "sleep 30"

[task]
command = "echo done"
exit_codes = [0]
"#,
    );

    let handle = run_in_background(&runner);
    assert!(wait_for_output(&out, "completed with expected exit code", TIMEOUT).await);
    assert!(!out.contents().contains("initiating shutdown"));

    runner.shutdown();
    assert_eq!(handle.await.unwrap(), 0);
}

#[tokio::test]
async fn unexpected_exit_triggers_shutdown() {
    let (runner, out, _dir) = make_runner(
        r#"
[main]
command = "sleep 30"

[crasher]
command = "exit 1"
"#,
    );

    let code = runner.run().await;
    assert!(out.contents().contains("initiating shutdown"));
    assert_ne!(code, 0);
}

#[tokio::test]
async fn exit_code_outside_allowed_list_triggers_shutdown() {
    let (runner, out, _dir) = make_runner(
        r#"
[main]
command = "sleep 30"

[task]
command = "exit 2"
exit_codes = [0]
"#,
    );

    let code = runner.run().await;
    assert!(out.contents().contains("initiating shutdown"));
    assert_ne!(code, 0);
}

#[tokio::test]
async fn exit_code_matching_array_entry_is_expected() {
    let (runner, out, _dir) = make_runner(
        r#"
[main]
command = "sleep 30"

[task]
command = "exit 2"
exit_codes = [0, 2]
"#,
    );

    let handle = run_in_background(&runner);
    assert!(wait_for_output(&out, "completed with expected exit code", TIMEOUT).await);
    assert!(!out.contents().contains("initiating shutdown"));

    runner.shutdown();
    assert_eq!(handle.await.unwrap(), 0);
}

#[tokio::test]
async fn process_without_exit_codes_always_triggers_shutdown() {
    let (runner, out, _dir) = make_runner(
        r#"
[main]
command = "sleep 30"

[service]
command = "true"
"#,
    );

    runner.run().await;
    assert!(out.contents().contains("initiating shutdown"));
}

#[tokio::test]
async fn signal_death_reports_the_signal_and_a_shell_convention_code() {
    let (runner, out, _dir) = make_runner(
        r#"
[victim]
command = "kill -KILL $$"
"#,
    );

    let code = runner.run().await;
    assert!(
        out.contents()
            .contains("victim terminated by SIGKILL (code 137)"),
        "{}",
        out.contents()
    );
    assert_eq!(code, 137);
}

#[tokio::test]
async fn signal_death_never_matches_exit_codes() {
    // 128 + signal is only a reporting convention; a signal death is never
    // an expected exit, even when its code appears in exit_codes.
    let (runner, out, _dir) = make_runner(
        r#"
[main]
command = "sleep 30"

[task]
command = "kill -KILL $$"
exit_codes = [137]
"#,
    );

    let code = runner.run().await;
    let output = out.contents();
    assert!(
        output.contains("task terminated by SIGKILL (code 137)"),
        "{output}"
    );
    assert!(output.contains("initiating shutdown"), "{output}");
    assert_eq!(code, 137);
}

#[tokio::test]
async fn all_expected_processes_exit_cleanly() {
    let (runner, _out, _dir) = make_runner(
        r#"
[a]
command = "echo a"
exit_codes = [0]

[b]
command = "echo b"
exit_codes = [0]
"#,
    );
    assert_eq!(runner.run().await, 0);
}

#[tokio::test]
async fn shutdown_is_idempotent() {
    let (runner, out, _dir) = make_runner("[app]\ncommand = \"sleep 30\"\n");

    let handle = run_in_background(&runner);
    assert!(wait_for_output(&out, "starting app: sleep 30", TIMEOUT).await);

    runner.shutdown();
    runner.shutdown();
    runner.shutdown();

    handle.await.unwrap();
}

#[tokio::test]
async fn shutdown_escalates_to_sigkill_for_stubborn_processes() {
    // A process that ignores SIGTERM must still be stopped by the SIGKILL
    // escalation after the shutdown timeout (2s, per `make_runner`).
    let (runner, out, _dir) =
        make_runner("[app]\ncommand = \"trap '' TERM; while true; do sleep 0.2; done\"\n");

    let handle = run_in_background(&runner);
    assert!(wait_for_output(&out, "starting app: trap", TIMEOUT).await);

    runner.shutdown();
    let stopped = tokio::time::timeout(Duration::from_secs(10), handle).await;
    assert!(
        stopped.is_ok(),
        "runner did not stop after SIGKILL escalation"
    );
    assert!(out.contents().contains("SIGKILL"), "{}", out.contents());
}

#[tokio::test]
async fn signals_are_logged_on_the_target_service() {
    // The signal sent to a service is logged on that service's own log (tagged
    // with its name), not as a system message.
    let (runner, out, _dir) = make_runner("[app]\ncommand = \"sleep 30\"\n");

    let handle = run_in_background(&runner);
    assert!(wait_for_output(&out, "starting app", TIMEOUT).await);

    runner.shutdown();
    assert!(wait_for_output(&out, "received SIGTERM", TIMEOUT).await);
    let _ = tokio::time::timeout(TIMEOUT, handle).await;

    let log = out.contents();
    let line = log
        .lines()
        .find(|l| l.contains("received SIGTERM"))
        .expect("a received-SIGTERM line");
    assert!(line.contains("app"), "expected the service tag: {line}");
    assert!(
        !line.contains("system"),
        "should not be a system log: {line}"
    );
}

// --- dependency ordering ---

#[tokio::test]
async fn dependencies_start_before_dependents() {
    let (runner, out, _dir) = make_runner(
        r#"
[db]
command = "echo db-started"
exit_codes = [0]

[web]
command = "echo web-started"
exit_codes = [0]
depends_on = ["db"]
"#,
    );

    runner.run().await;
    let output = out.contents();
    let db = output.find("db-started").expect("db ran");
    let web = output.find("web-started").expect("web ran");
    assert!(db < web);
}

#[tokio::test]
async fn dependency_exiting_with_allowed_code_unblocks_dependent() {
    let (runner, out, _dir) = make_runner(
        r#"
[broken]
command = "exit 1"
exit_codes = [0, 1]

[dependent]
command = "echo dependent-ran"
exit_codes = [0]
depends_on = ["broken"]
"#,
    );

    runner.run().await;
    assert!(out.contents().contains("dependent-ran"));
}

#[tokio::test]
async fn exit_codes_dependency_waits_for_exit() {
    let (runner, out, _dir) = make_runner(
        r#"
[migrate]
command = "sleep 0.5 && echo migrate-done"
exit_codes = [0]

[web]
command = "echo web-started"
exit_codes = [0]
depends_on = ["migrate"]
"#,
    );

    runner.run().await;
    let output = out.contents();
    let migrate = output.find("migrate-done").expect("migrate ran");
    let web = output.find("web-started").expect("web ran");
    assert!(migrate < web);
}

#[tokio::test]
async fn failed_dependency_blocks_dependent() {
    let (runner, out, _dir) = make_runner(
        r#"
[migrate]
command = "exit 2"
exit_codes = [0]

[web]
command = "echo web-started"
exit_codes = [0]
depends_on = ["migrate"]
"#,
    );

    let code = runner.run().await;
    assert!(!out.contents().contains("web-started"));
    assert_ne!(code, 0);
}

#[tokio::test]
async fn bare_dependency_is_ready_on_launch() {
    let (runner, out, _dir) = make_runner(
        r#"
[db]
command = "echo db-launched && sleep 30"

[web]
command = "echo web-started"
exit_codes = [0]
depends_on = ["db"]
"#,
    );

    let handle = run_in_background(&runner);
    assert!(wait_for_output(&out, "web-started", TIMEOUT).await);

    runner.shutdown();
    handle.await.unwrap();
}

// --- readiness ---

#[tokio::test]
async fn dependent_waits_for_readiness_check() {
    let dir = tempfile::tempdir().unwrap();
    let ready = dir.path().join("ready");
    let ready = ready.display().to_string();

    let (runner, out, _cfg_dir) = make_runner(&format!(
        r#"
[api]
command            = "sleep 0.3 && touch {ready} && sleep 30"
readiness.command  = "test -f {ready}"
readiness.interval = 1
readiness.timeout  = 5

[frontend]
command    = "echo frontend-started"
exit_codes = [0]
depends_on = ["api"]
"#,
    ));

    let handle = run_in_background(&runner);
    assert!(wait_for_output(&out, "frontend-started", Duration::from_secs(10)).await);
    assert!(out.contents().contains("readiness check passed"));

    runner.shutdown();
    handle.await.unwrap();
}

#[tokio::test]
async fn readiness_timeout_blocks_dependent_and_fails_the_run() {
    let (runner, out, _dir) = make_runner(
        r#"
[api]
command            = "sleep 30"
readiness.command  = "false"
readiness.interval = 1
readiness.timeout  = 1

[frontend]
command    = "echo frontend-started"
exit_codes = [0]
depends_on = ["api"]
"#,
    );

    // The timeout itself initiates shutdown and taints the exit code.
    let code = runner.run().await;
    let output = out.contents();
    assert!(output.contains("timed out"), "{output}");
    assert!(!output.contains("frontend-started"), "{output}");
    assert_ne!(code, 0);
}

#[tokio::test]
async fn readiness_check_runs_at_least_once_when_timeout_is_short() {
    // timeout (1s) < interval (10s): the immediate first probe must still run.
    let (runner, out, _dir) = make_runner(
        r#"
[api]
command            = "sleep 30"
readiness.command  = "true"
readiness.interval = 10
readiness.timeout  = 1

[frontend]
command    = "echo frontend-started"
exit_codes = [0]
depends_on = ["api"]
"#,
    );

    let handle = run_in_background(&runner);
    assert!(wait_for_output(&out, "frontend-started", TIMEOUT).await);

    runner.shutdown();
    handle.await.unwrap();
}

#[tokio::test]
async fn readiness_check_sees_the_process_environment() {
    let (runner, out, _dir) = make_runner(
        r#"
[api]
command            = "sleep 30"
environment        = { READY_FLAG = "yes" }
readiness.command  = "test \"$READY_FLAG\" = yes"
readiness.interval = 1
readiness.timeout  = 5

[frontend]
command    = "echo frontend-started"
exit_codes = [0]
depends_on = ["api"]
"#,
    );

    let handle = run_in_background(&runner);
    assert!(wait_for_output(&out, "frontend-started", TIMEOUT).await);

    runner.shutdown();
    handle.await.unwrap();
}

// --- output ordering ---

#[tokio::test]
async fn stdout_and_stderr_lines_keep_write_order() {
    // Both streams share one pipe, so interleaved lines stay in write order.
    let (runner, out, _dir) = make_runner(
        r#"
[app]
command    = "echo out1; echo err1 1>&2; echo out2; echo err2 1>&2"
exit_codes = [0]
"#,
    );

    runner.run().await;
    let output = out.contents();
    let pos = |s: &str| {
        output
            .find(s)
            .unwrap_or_else(|| panic!("{s} missing: {output}"))
    };
    assert!(pos("out1") < pos("err1"), "{output}");
    assert!(pos("err1") < pos("out2"), "{output}");
    assert!(pos("out2") < pos("err2"), "{output}");
}

// --- environment ---

#[tokio::test]
async fn inline_environment_reaches_the_process() {
    let (runner, out, _dir) = make_runner(
        r#"
[app]
command = "echo MY_VAR=$MY_VAR"
exit_codes = [0]
environment = { MY_VAR = "hello_from_config" }
"#,
    );

    runner.run().await;
    assert!(out.contents().contains("MY_VAR=hello_from_config"));
}

#[tokio::test]
async fn full_env_merge_order_reaches_the_process() {
    let dir = tempfile::tempdir().unwrap();
    let global = dir.path().join(".env");
    let proc = dir.path().join(".env.app");
    std::fs::write(
        &global,
        "VAR=global\nONLY_GLOBAL=yes\nGFILE_VS_GTABLE=from_gfile\n",
    )
    .unwrap();
    std::fs::write(&proc, "VAR=proc\nONLY_PROC=yes\n").unwrap();

    let toml = format!(
        r#"
env_file    = {:?}
environment = {{ VAR = "global_table", GFILE_VS_GTABLE = "from_gtable", ONLY_GTABLE = "yes" }}

[app]
command     = "echo VAR=$VAR GVG=$GFILE_VS_GTABLE OG=$ONLY_GLOBAL OGT=$ONLY_GTABLE OP=$ONLY_PROC"
exit_codes  = [0]
env_file    = {:?}
environment = {{ VAR = "inline" }}
"#,
        global.display().to_string(),
        proc.display().to_string(),
    );

    let (runner, out, _cfg_dir) = make_runner(&toml);
    runner.run().await;

    let output = out.contents();
    assert!(output.contains("VAR=inline"), "{output}");
    assert!(output.contains("OG=yes"), "{output}");
    assert!(output.contains("GVG=from_gtable"), "{output}");
    assert!(output.contains("OGT=yes"), "{output}");
    assert!(output.contains("OP=yes"), "{output}");
}

// --- retries ---

#[tokio::test]
async fn exhausted_retries_trigger_shutdown() {
    let dir = tempfile::tempdir().unwrap();
    let counter = dir.path().join("count");
    std::fs::write(&counter, "0").unwrap();
    let path = counter.display().to_string();

    let cmd = format!("count=$(cat {path}); count=$((count + 1)); echo $count > {path}; exit 1");
    let (runner, out, _cfg_dir) =
        make_runner(&format!("[task]\ncommand = {cmd:?}\nmax_retries = 2\n"));

    runner.run().await;

    assert_eq!(std::fs::read_to_string(&counter).unwrap().trim(), "3");
    let output = out.contents();
    assert!(output.contains("retry 1/2"), "{output}");
    assert!(output.contains("retry 2/2"), "{output}");
    assert!(output.contains("all 2 retries exhausted"), "{output}");
}

#[tokio::test]
async fn retry_can_succeed_before_exhaustion() {
    let dir = tempfile::tempdir().unwrap();
    let counter = dir.path().join("count");
    std::fs::write(&counter, "0").unwrap();
    let path = counter.display().to_string();

    let cmd = format!(
        "count=$(cat {path}); count=$((count + 1)); echo $count > {path}; \
         if [ $count -eq 1 ]; then exit 1; fi; sleep 30"
    );
    let (runner, out, _cfg_dir) =
        make_runner(&format!("[service]\ncommand = {cmd:?}\nmax_retries = 3\n"));

    let handle = run_in_background(&runner);
    assert!(wait_for_output(&out, "retry 1/3", TIMEOUT).await);
    assert!(!out.contents().contains("all 3 retries exhausted"));

    runner.shutdown();
    handle.await.unwrap();
}

#[tokio::test]
async fn dependent_starts_when_a_dependency_succeeds_on_retry() {
    // A dependency that fails its first attempt but succeeds on retry must
    // not permanently fail its dependents: readiness/exit-code results are
    // only terminal once retries are exhausted.
    let dir = tempfile::tempdir().unwrap();
    let counter = dir.path().join("count");
    std::fs::write(&counter, "0").unwrap();
    let path = counter.display().to_string();

    let cmd = format!(
        "count=$(cat {path}); count=$((count + 1)); echo $count > {path}; \
         if [ $count -eq 1 ]; then exit 1; fi"
    );
    let (runner, out, _cfg_dir) = make_runner(&format!(
        r#"
[migrate]
command     = {cmd:?}
exit_codes  = [0]
max_retries = 2

[web]
command    = "echo web-started"
exit_codes = [0]
depends_on = ["migrate"]
"#,
    ));

    let code = runner.run().await;
    let output = out.contents();
    assert!(output.contains("retry 1/2"), "{output}");
    assert!(output.contains("web-started"), "{output}");
    assert!(!output.contains("initiating shutdown"), "{output}");
    assert_eq!(code, 0);
}

#[tokio::test]
async fn zero_retries_means_no_restart() {
    let (runner, out, _dir) = make_runner("[task]\ncommand = \"exit 1\"\n");

    runner.run().await;
    let output = out.contents();
    assert!(!output.contains("retry"), "{output}");
    assert!(output.contains("initiating shutdown"), "{output}");
}

// --- log files ---

#[tokio::test]
async fn process_output_is_copied_to_its_log_file() {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("app.log");
    let log_path = log.display().to_string();

    // The program emits an ANSI color escape; the log-file copy must be plain.
    let (runner, _out, _cfg) = make_runner(&format!(
        "[app]\ncommand = \"printf '\\\\033[31mhello-log\\\\033[0m\\\\n'\"\nexit_codes = [0]\nlog_file = {log_path:?}\n"
    ));
    runner.run().await;

    let contents = std::fs::read_to_string(&log).expect("log file written");
    assert!(contents.contains("hello-log"), "{contents}");
    // The log-file copy is plain text: the program's own ANSI codes are stripped.
    assert!(!contents.contains('\u{1b}'), "{contents}");
}

#[tokio::test]
async fn unopenable_log_file_warns_but_still_runs() {
    // Parent directory is missing, so opening the log file fails.
    let dir = tempfile::tempdir().unwrap();
    let bad_log = dir.path().join("missing").join("app.log");
    let bad_log = bad_log.display().to_string();

    let (runner, out, _cfg) = make_runner(&format!(
        "[app]\ncommand = \"echo still-running\"\nexit_codes = [0]\nlog_file = {bad_log:?}\n"
    ));
    runner.run().await;

    let logged = out.contents();
    // The service runs despite the bad log path, and the failure is a warning.
    assert!(logged.contains("still-running"), "{logged}");
    assert!(logged.contains("cannot open log file"), "{logged}");
}

// --- filtering integrates with the runner ---

const SUBSET_CONFIG: &str = r#"
[web]
command = "echo web-ran"
exit_codes = [0]

[worker]
command = "echo worker-ran"
exit_codes = [0]
"#;

/// Loads inline TOML, applies `only`/`except` filters, runs to completion,
/// and returns the captured output.
async fn run_filtered(content: &str, only: &[&str], except: &[&str]) -> String {
    let (dir, path) = write_config(content);
    let mut config = Config::load(&path).unwrap();
    let only: Vec<String> = only.iter().map(|s| (*s).to_owned()).collect();
    let except: Vec<String> = except.iter().map(|s| (*s).to_owned()).collect();
    config.filter(&only, &except).unwrap();

    let (runner, out) = build_runner(config);
    runner.run().await;
    drop(dir);
    out.contents()
}

#[tokio::test]
async fn filter_only_runs_the_requested_subset() {
    let output = run_filtered(SUBSET_CONFIG, &["web"], &[]).await;
    assert!(output.contains("web-ran"), "{output}");
    assert!(!output.contains("worker-ran"), "{output}");
}

#[tokio::test]
async fn filter_except_skips_the_excluded_subset() {
    let output = run_filtered(SUBSET_CONFIG, &[], &["worker"]).await;
    assert!(output.contains("web-ran"), "{output}");
    assert!(!output.contains("worker-ran"), "{output}");
}

// --- single-service shutdown (stop_service) ---

#[tokio::test]
async fn stop_service_stops_subtree_without_global_shutdown() {
    // worker depends on web; db is independent. Stopping web must take down
    // web + worker, leave db running, not taint the exit code, and emit the
    // dependent note (an always-shown line, regardless of log level).
    let (runner, out, _dir) = make_runner(
        r#"
[db]
command = "echo db-up; sleep 30"

[web]
command = "echo web-up; sleep 30"

[worker]
command = "echo worker-up; sleep 30"
depends_on = ["web"]
"#,
    );

    let handle = run_in_background(&runner);
    assert!(wait_for_output(&out, "worker-up", TIMEOUT).await);
    assert!(out.contents().contains("db-up"), "{}", out.contents());

    runner.stop_service("web");

    // The dependent note explains the manual stop of the parent.
    assert!(
        wait_for_output(&out, "was manually shut down", Duration::from_secs(5)).await,
        "{}",
        out.contents()
    );

    // proccie keeps running because the independent db is still alive.
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(
        !handle.is_finished(),
        "runner exited despite db still running"
    );

    // A clean global shutdown afterwards; stopped services aren't failures.
    runner.shutdown();
    assert_eq!(handle.await.unwrap(), 0);
}

#[tokio::test]
async fn stopping_a_readiness_service_is_not_a_timeout_failure() {
    // A readiness-mode service that never passes is manually stopped before its
    // window elapses. The stop settles the status first, so the run ends cleanly
    // (code 0) rather than escalating the pending timeout into a run failure.
    let (runner, out, _dir) = make_runner(
        r#"
[api]
command            = "sleep 30"
readiness.command  = "false"
readiness.interval = 1
readiness.timeout  = 30
"#,
    );

    let handle = run_in_background(&runner);
    assert!(wait_for_output(&out, "starting api", TIMEOUT).await);

    runner.stop_service("api");
    let code = tokio::time::timeout(TIMEOUT, handle)
        .await
        .expect("runner stops promptly")
        .unwrap();

    let output = out.contents();
    assert!(!output.contains("timed out"), "{output}");
    assert_eq!(code, 0, "{output}");
}

#[tokio::test]
async fn stop_service_does_not_record_a_failure_code() {
    // Stopping a lone service ends the run with code 0, not a kill code.
    let (runner, out, _dir) = make_runner("[app]\ncommand = \"echo up; sleep 30\"\n");

    let handle = run_in_background(&runner);
    assert!(wait_for_output(&out, "up", TIMEOUT).await);

    runner.stop_service("app");
    assert_eq!(handle.await.unwrap(), 0);
}

#[tokio::test]
async fn stopping_a_dependents_tab_escalates_to_sigkill() {
    // Stopping the parent SIGTERMs the subtree {parent, child}; child ignores
    // TERM. Stopping child's own tab must then force-kill it at once, not wait
    // for the parent stop's scheduled SIGKILL escalation (2s, per `make_runner`).
    let (runner, out, _dir) = make_runner(
        r#"
[parent]
command = "echo parent-up; sleep 30"

[child]
command = "trap '' TERM; echo child-up; while true; do sleep 0.2; done"
depends_on = ["parent"]
"#,
    );

    let handle = run_in_background(&runner);
    assert!(wait_for_output(&out, "child-up", TIMEOUT).await);

    runner.stop_service("parent");
    // Let the subtree SIGTERM land (child ignores it).
    tokio::time::sleep(Duration::from_millis(300)).await;

    runner.stop_service("child");
    // The escalation is immediate, well within the 2s scheduled SIGKILL.
    let stopped = tokio::time::timeout(Duration::from_secs(1), handle).await;
    assert!(
        stopped.is_ok(),
        "dependent was not force-killed promptly: {}",
        out.contents()
    );
    assert!(
        out.contents().contains("force-stopping child"),
        "{}",
        out.contents()
    );
}
