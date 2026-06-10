//! Tests that drive the compiled binary directly.

mod common;

use std::process::Command;

use common::write_config;

/// Path to the binary under test, provided by Cargo for integration tests.
const BIN: &str = env!("CARGO_BIN_EXE_proccie");

#[test]
fn validate_reports_a_valid_config() {
    let (_dir, path) = write_config(
        r#"
[web]
command = "npm start"

[worker]
command = "bundle exec sidekiq"
"#,
    );

    let output = Command::new(BIN)
        .args(["--config", path.to_str().unwrap(), "validate"])
        .output()
        .expect("run proccie");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("valid"), "{stdout}");
    assert!(stdout.contains("2 process(es)"), "{stdout}");
    assert!(
        stdout.contains("web") && stdout.contains("worker"),
        "{stdout}"
    );
}

#[test]
fn validate_reports_an_invalid_config() {
    let (_dir, path) = write_config("[web]\nexit_codes = [0]\n");

    let output = Command::new(BIN)
        .args(["--config", path.to_str().unwrap(), "validate"])
        .output()
        .expect("run proccie");

    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("error"));
}

#[test]
fn validate_warns_when_a_dependency_releases_at_launch() {
    // `other` is depended on but has no readiness signal, so dependents start
    // the moment it launches -- valid, but worth warning about.
    let (_dir, path) = write_config(
        r#"
[dependencies]
command = "pnpm install"
exit_codes = [0]

[other]
command = "sleep 45"

[frontend]
command = "pnpm dev --open"
depends_on = ["dependencies", "other"]
"#,
    );

    let output = Command::new(BIN)
        .args(["--config", path.to_str().unwrap(), "validate"])
        .output()
        .expect("run proccie");

    // Still valid: warnings do not fail the config.
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("valid"));

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("warning"), "{stderr}");
    assert!(stderr.contains("other"), "{stderr}");
    // `dependencies` sets exit_codes, so it must not be warned about.
    assert!(!stderr.contains("dependencies"), "{stderr}");
}

#[test]
fn validate_reports_a_missing_file() {
    let output = Command::new(BIN)
        .args(["--config", "/nonexistent/Procfile.toml", "validate"])
        .output()
        .expect("run proccie");

    assert_eq!(output.status.code(), Some(1));
}

#[test]
fn run_supervises_processes_to_completion() {
    let (_dir, path) = write_config(
        r#"
[a]
command = "echo a-ran"
exit_codes = [0]

[b]
command = "echo b-ran"
exit_codes = [0]
"#,
    );

    let output = Command::new(BIN)
        .args(["--config", path.to_str().unwrap()])
        .output()
        .expect("run proccie");

    assert!(output.status.success(), "status: {:?}", output.status);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("a-ran") && stdout.contains("b-ran"),
        "{stdout}"
    );
}

#[test]
fn run_exits_with_the_failing_process_code() {
    let (_dir, path) = write_config("[task]\ncommand = \"exit 3\"\n");

    let output = Command::new(BIN)
        .args(["--config", path.to_str().unwrap()])
        .output()
        .expect("run proccie");

    assert_eq!(output.status.code(), Some(3));
}

#[test]
fn only_flag_runs_a_subset() {
    let (_dir, path) = write_config(
        r#"
[web]
command = "echo web-ran"
exit_codes = [0]

[worker]
command = "echo worker-ran"
exit_codes = [0]
"#,
    );

    let output = Command::new(BIN)
        .args(["--config", path.to_str().unwrap(), "--only", "web"])
        .output()
        .expect("run proccie");

    assert!(output.status.success(), "status: {:?}", output.status);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("web-ran"), "{stdout}");
    assert!(!stdout.contains("worker-ran"), "{stdout}");
}

#[test]
fn version_flag_prints_version() {
    let output = Command::new(BIN)
        .arg("--version")
        .output()
        .expect("run proccie");
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains(env!("CARGO_PKG_VERSION")));
}
