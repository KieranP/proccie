//! Tests for config parsing, validation, ordering, filtering, and
//! environment resolution.

mod common;

use std::collections::BTreeMap;
use std::time::Duration;

use proccie::config::{Config, DEFAULT_READINESS_INTERVAL, DEFAULT_READINESS_TIMEOUT, ExitCodes};

use common::write_config;

/// Loads config from inline TOML, asserting success.
fn load(content: &str) -> Config {
    let (_dir, path) = write_config(content);
    Config::load(&path).expect("config should load")
}

/// Loads config from inline TOML, asserting failure, and returns the
/// rendered error message.
fn load_err(content: &str) -> String {
    let (_dir, path) = write_config(content);
    Config::load(&path)
        .expect_err("config should fail")
        .to_string()
}

#[test]
fn loads_basic_config() {
    let cfg = load(
        r#"
[web]
command = "npm start"

[worker]
command = "bundle exec sidekiq"
exit_codes = [0, 1]
depends_on = ["web"]
"#,
    );

    assert_eq!(cfg.processes().len(), 2);

    let web = &cfg.processes()["web"];
    assert_eq!(web.command, "npm start");
    assert!(web.exit_codes.is_empty());

    let worker = &cfg.processes()["worker"];
    assert!(worker.exit_codes.allows(0) && worker.exit_codes.allows(1));
    assert_eq!(worker.depends_on, ["web"]);
}

#[test]
fn rejects_bare_integer_exit_codes() {
    load_err(
        r#"
[task]
command = "rake db:migrate"
exit_codes = 0
"#,
    );
}

#[test]
fn parses_exit_codes_array() {
    let cfg = load(
        r#"
[task]
command = "run-checks"
exit_codes = [0, 2, 3]
"#,
    );
    let codes = &cfg.processes()["task"].exit_codes;
    assert!(codes.allows(0) && codes.allows(2) && codes.allows(3));
    assert!(!codes.allows(1));
}

#[test]
fn parses_inline_and_subtable_environment() {
    for content in [
        r#"
[web]
command = "npm start"
environment = { PORT = "3000", NODE_ENV = "development" }
"#,
        r#"
[web]
command = "npm start"

[web.environment]
PORT = "3000"
NODE_ENV = "development"
"#,
    ] {
        let cfg = load(content);
        let env = &cfg.processes()["web"].environment;
        assert_eq!(env["PORT"], "3000");
        assert_eq!(env["NODE_ENV"], "development");
    }
}

#[test]
fn missing_file_is_an_error() {
    assert!(Config::load("/nonexistent/path/Procfile.toml").is_err());
}

#[test]
fn rejects_unknown_top_level_key() {
    let err = load_err(
        r#"
bogus = "something"

[web]
command = "npm start"
"#,
    );
    assert!(err.contains("unknown top-level key"), "{err}");
}

#[test]
fn rejects_unknown_process_key() {
    // A typo'd process key (here `exit_code` for `exit_codes`) must error
    // rather than silently misconfigure the process.
    let err = load_err(
        r#"
[web]
command   = "npm start"
exit_code = [0]
"#,
    );
    assert!(err.contains("exit_code"), "{err}");
}

#[test]
fn parses_log_file_option() {
    let cfg = load(
        r#"
[web]
command  = "npm start"
log_file = "tmp/web.log"
"#,
    );
    assert_eq!(
        cfg.processes()["web"].log_file.as_deref(),
        Some("tmp/web.log")
    );

    let cfg = load("[web]\ncommand = \"npm start\"\n");
    assert_eq!(cfg.processes()["web"].log_file, None);
}

// --- readiness ---

#[test]
fn parses_readiness_string_form() {
    let cfg = load(
        r#"
[web]
command   = "npm start"
readiness = "curl -sf http://localhost:3000/health"
"#,
    );
    let r = cfg.processes()["web"]
        .readiness
        .as_ref()
        .expect("readiness");
    assert_eq!(r.command, "curl -sf http://localhost:3000/health");
    // Unspecified interval/timeout fall back to defaults.
    assert_eq!(r.interval_or_default(), DEFAULT_READINESS_INTERVAL);
    assert_eq!(r.timeout_or_default(), DEFAULT_READINESS_TIMEOUT);
}

#[test]
fn parses_readiness_table_form() {
    let cfg = load(
        r#"
[web]
command            = "npm start"
readiness.command  = "curl -sf http://localhost:3000/health"
readiness.interval = 2
readiness.timeout  = 10
"#,
    );
    let r = cfg.processes()["web"]
        .readiness
        .as_ref()
        .expect("readiness");
    assert_eq!(r.command, "curl -sf http://localhost:3000/health");
    assert_eq!(r.interval_or_default().as_secs(), 2);
    assert_eq!(r.timeout_or_default().as_secs(), 10);
}

#[test]
fn readiness_zero_interval_and_timeout_fall_back_to_defaults() {
    // Zero is a valid request for the default rather than a parse error.
    let cfg = load(
        r#"
[web]
command            = "npm start"
readiness.command  = "curl -sf http://localhost:3000/health"
readiness.interval = 0
readiness.timeout  = 0
"#,
    );
    let r = cfg.processes()["web"]
        .readiness
        .as_ref()
        .expect("readiness");
    assert_eq!(r.interval_or_default(), DEFAULT_READINESS_INTERVAL);
    assert_eq!(r.timeout_or_default(), DEFAULT_READINESS_TIMEOUT);
}

#[test]
fn readiness_negative_duration_is_rejected() {
    let err = load_err(
        r#"
[web]
command            = "npm start"
readiness.command  = "curl -sf http://localhost:3000/health"
readiness.interval = -1
"#,
    );
    assert!(err.contains("negative"), "{err}");
}

#[test]
fn fractional_readiness_duration_is_accepted() {
    let cfg = load(
        r#"
[web]
command            = "npm start"
readiness.command  = "curl -sf http://localhost:3000/health"
readiness.interval = 2.5
"#,
    );
    let r = cfg.processes()["web"]
        .readiness
        .as_ref()
        .expect("readiness");
    assert_eq!(r.interval_or_default(), Duration::from_millis(2500));
}

#[test]
fn readiness_table_requires_command() {
    let err = load_err(
        r#"
[web]
command            = "npm start"
readiness.interval = 2
"#,
    );
    assert!(err.contains("requires \"command\""), "{err}");
}

#[test]
fn readiness_table_rejects_unknown_keys() {
    // A typo'd key must surface as an error, not be silently ignored.
    let err = load_err(
        r#"
[web]
command           = "npm start"
readiness.command = "curl -sf http://localhost:3000/health"
readiness.retries = 3
"#,
    );
    assert!(err.contains("retries"), "{err}");
}

#[test]
fn readiness_command_cannot_be_empty() {
    // An empty command must not silently degrade to launch-ready mode.
    let err = load_err(
        r#"
[web]
command   = "npm start"
readiness = ""
"#,
    );
    assert!(err.contains("readiness command cannot be empty"), "{err}");
}

// --- validation ---

#[test]
fn rejects_missing_command() {
    let err = load_err("[web]\nexit_codes = [0]\n");
    assert!(err.contains("missing required key"), "{err}");
}

#[test]
fn rejects_undefined_dependency() {
    let err = load_err("[web]\ncommand = \"npm start\"\ndepends_on = [\"db\"]\n");
    assert!(err.contains("not defined"), "{err}");
}

#[test]
fn rejects_self_dependency() {
    let err = load_err("[web]\ncommand = \"npm start\"\ndepends_on = [\"web\"]\n");
    assert!(err.contains("cannot depend on itself"), "{err}");
}

#[test]
fn rejects_duplicate_display_name() {
    let err = load_err(
        "[web1]\ncommand = \"true\"\nname = \"Web\"\n[web2]\ncommand = \"true\"\nname = \"Web\"\n",
    );
    assert!(
        err.contains("display name \"Web\" is already used"),
        "{err}"
    );
}

#[test]
fn rejects_name_colliding_with_another_key() {
    let err = load_err("[web]\ncommand = \"true\"\nname = \"api\"\n[api]\ncommand = \"true\"\n");
    assert!(
        err.contains("display name \"api\" is already used"),
        "{err}"
    );
}

#[test]
fn rejects_duplicate_dependency() {
    let err = load_err(
        r#"
[db]
command = "echo db"

[web]
command = "npm start"
depends_on = ["db", "db"]
"#,
    );
    assert!(err.contains("duplicate dependency"), "{err}");
}

#[test]
fn rejects_cyclic_dependency() {
    let err = load_err(
        r#"
[a]
command = "echo a"
depends_on = ["b"]

[b]
command = "echo b"
depends_on = ["a"]
"#,
    );
    assert!(err.contains("cycle"), "{err}");
}

#[test]
fn cycle_error_reports_only_the_cycle() {
    // `a` leads into the cycle but is not part of it; the message should name
    // just the cycle (`b -> c -> b`), not the path that reached it.
    let err = load_err(
        r#"
[a]
command = "echo a"
depends_on = ["b"]

[b]
command = "echo b"
depends_on = ["c"]

[c]
command = "echo c"
depends_on = ["b"]
"#,
    );
    assert!(err.contains("b -> c -> b"), "{err}");
    assert!(!err.contains("a ->"), "{err}");
}

#[test]
fn reports_multiple_errors() {
    let err = load_err(
        r#"
[web]
exit_codes = [0]

[worker]
exit_codes = [0]
"#,
    );
    assert!(err.contains("web") && err.contains("worker"), "{err}");
}

#[test]
fn readiness_and_exit_codes_are_mutually_exclusive() {
    let err = load_err(
        r#"
[web]
command    = "npm start"
readiness  = "curl -sf http://localhost:3000/health"
exit_codes = [0]
"#,
    );
    assert!(err.contains("mutually exclusive"), "{err}");
}

#[test]
fn rejects_negative_max_retries() {
    let err = load_err("[web]\ncommand = \"npm start\"\nmax_retries = -1\n");
    assert!(err.contains("max_retries must be non-negative"), "{err}");
}

#[test]
fn max_retries_defaults_to_zero() {
    let cfg = load("[web]\ncommand = \"npm start\"\n");
    assert_eq!(cfg.processes()["web"].max_retries, 0);
}

#[test]
fn rejects_retries_with_readiness() {
    // Retries fire on exit, not a failed readiness window, so the combination is
    // rejected rather than silently ignoring max_retries.
    let err = load_err(
        r#"
[web]
command     = "npm start"
readiness   = "curl -sf http://localhost:3000/health"
max_retries = 3
"#,
    );
    assert!(
        err.contains("\"max_retries\" has no effect with \"readiness\""),
        "{err}"
    );
}

#[test]
fn readiness_with_default_retries_is_allowed() {
    // The default (0 retries) does not conflict with readiness.
    let cfg = load(
        r#"
[web]
command   = "npm start"
readiness = "curl -sf http://localhost:3000/health"
"#,
    );
    assert!(cfg.processes()["web"].readiness.is_some());
}

// --- exit codes helper ---

#[test]
fn exit_codes_allows() {
    assert!(!ExitCodes::default().allows(0));
    assert!(ExitCodes(vec![0]).allows(0));
    assert!(!ExitCodes(vec![0]).allows(1));
    assert!(ExitCodes(vec![0, 1, 2]).allows(2));
    assert!(!ExitCodes(vec![0, 1, 2]).allows(3));
}

// --- ordering and names ---

#[test]
fn start_order_respects_dependencies() {
    let cfg = load(
        r#"
[db]
command = "echo db"

[migrate]
command = "echo migrate"
depends_on = ["db"]

[web]
command = "echo web"
depends_on = ["db", "migrate"]
"#,
    );

    let order = proccie::config::topo_order(&cfg.adjacency());
    let pos = |name: &str| order.iter().position(|n| n == name).unwrap();
    assert!(pos("db") < pos("migrate"));
    assert!(pos("migrate") < pos("web"));
}

#[test]
fn start_order_is_deterministic_and_alphabetical() {
    let cfg = load(
        r#"
[zebra]
command = "echo zebra"

[alpha]
command = "echo alpha"

[middle]
command = "echo middle"
"#,
    );
    assert_eq!(
        proccie::config::topo_order(&cfg.adjacency()),
        ["alpha", "middle", "zebra"]
    );
}

#[test]
fn names_are_sorted() {
    let cfg = load("[web]\ncommand=\"a\"\n[db]\ncommand=\"b\"\n[api]\ncommand=\"c\"\n");
    assert_eq!(cfg.names(), ["api", "db", "web"]);
}

// --- filter ---

#[test]
fn filter_only_keeps_named_process() {
    let mut cfg =
        load("[web]\ncommand=\"a\"\n[worker]\ncommand=\"b\"\n[scheduler]\ncommand=\"c\"\n");
    cfg.filter(&["web".into()], &[]).unwrap();
    assert_eq!(cfg.names(), ["web"]);
}

#[test]
fn filter_only_includes_transitive_dependencies() {
    let mut cfg = load(
        r#"
[db]
command = "postgres"

[migrate]
command = "rake db:migrate"
exit_codes = [0]
depends_on = ["db"]

[web]
command = "npm start"
depends_on = ["migrate"]

[worker]
command = "bundle exec sidekiq"
"#,
    );
    cfg.filter(&["web".into()], &[]).unwrap();
    assert_eq!(cfg.names(), ["db", "migrate", "web"]);
}

#[test]
fn filter_except_removes_named_processes() {
    let mut cfg =
        load("[web]\ncommand=\"a\"\n[worker]\ncommand=\"b\"\n[scheduler]\ncommand=\"c\"\n");
    cfg.filter(&[], &["worker".into(), "scheduler".into()])
        .unwrap();
    assert_eq!(cfg.names(), ["web"]);
}

#[test]
fn filter_except_cascades_to_dependents() {
    let mut cfg = load(
        r#"
[db]
command = "postgres"

[web]
command = "npm start"
depends_on = ["db"]

[worker]
command = "run worker"
depends_on = ["web"]

[lonely]
command = "idle"
"#,
    );
    // Excluding `db` also excludes `web` and `worker` (its transitive dependents),
    // so nothing is left running without a dependency it was wired to need.
    cfg.filter(&[], &["db".into()]).unwrap();
    assert_eq!(cfg.names(), ["lonely"]);
}

#[test]
fn filter_rejects_unknown_and_conflicting_flags() {
    let base = "[web]\ncommand=\"a\"\n[worker]\ncommand=\"b\"\n";

    let mut cfg = load(base);
    assert!(
        cfg.filter(&["nope".into()], &[])
            .unwrap_err()
            .to_string()
            .contains("unknown process")
    );

    let mut cfg = load(base);
    assert!(
        cfg.filter(&[], &["nope".into()])
            .unwrap_err()
            .to_string()
            .contains("unknown process")
    );

    let mut cfg = load(base);
    assert!(
        cfg.filter(&["web".into()], &["worker".into()])
            .unwrap_err()
            .to_string()
            .contains("cannot specify both")
    );
}

#[test]
fn filter_with_no_args_is_a_noop() {
    let mut cfg = load("[web]\ncommand=\"a\"\n[worker]\ncommand=\"b\"\n");
    cfg.filter(&[], &[]).unwrap();
    assert_eq!(cfg.processes().len(), 2);
}

// --- environment resolution ---

/// Reads the resolved env of a single process.
fn env_of(cfg: &Config, name: &str) -> BTreeMap<String, String> {
    cfg.processes()[name].env().clone()
}

#[test]
fn resolves_global_and_per_process_env() {
    let dir = tempfile::tempdir().unwrap();
    let global = dir.path().join(".env");
    let proc = dir.path().join(".env.web");
    std::fs::write(&global, "SHARED_VAR=global_value\n").unwrap();
    std::fs::write(&proc, "WEB_PORT=3000\n").unwrap();

    let toml = format!(
        "env_file = {:?}\n\n[web]\ncommand = \"npm start\"\nenv_file = {:?}\n",
        global.display().to_string(),
        proc.display().to_string(),
    );
    let cfg_path = dir.path().join("Procfile.toml");
    std::fs::write(&cfg_path, toml).unwrap();

    let cfg = Config::load(&cfg_path).unwrap();
    let env = env_of(&cfg, "web");
    assert_eq!(
        env.get("SHARED_VAR").map(String::as_str),
        Some("global_value")
    );
    assert_eq!(env.get("WEB_PORT").map(String::as_str), Some("3000"));
}

#[test]
fn base_env_is_inherited_and_overridden_by_every_config_layer() {
    let dir = tempfile::tempdir().unwrap();
    let global = dir.path().join(".env");
    std::fs::write(&global, "FROM_FILE=file\n").unwrap();

    let toml = format!(
        "env_file = {:?}\n\n[web]\ncommand = \"npm start\"\nenvironment = {{ FROM_INLINE = \"inline\" }}\n",
        global.display().to_string(),
    );
    let cfg_path = dir.path().join("Procfile.toml");
    std::fs::write(&cfg_path, toml).unwrap();

    // An explicit base environment stands in for the OS layer, so its
    // precedence can be pinned without touching the real process env.
    let base = BTreeMap::from([
        ("INHERITED".to_owned(), "from_base".to_owned()),
        ("FROM_FILE".to_owned(), "base_loses".to_owned()),
        ("FROM_INLINE".to_owned(), "base_loses".to_owned()),
    ]);
    let cfg = Config::load_with_env(&cfg_path, base).unwrap();

    let env = env_of(&cfg, "web");
    assert_eq!(env["INHERITED"], "from_base");
    assert_eq!(env["FROM_FILE"], "file");
    assert_eq!(env["FROM_INLINE"], "inline");
}

#[test]
fn relative_env_file_resolves_against_the_config_directory() {
    // A relative env_file must be found next to the Procfile, not in the CWD.
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join(".env"), "FROM_REL=yes\n").unwrap();
    let cfg_path = dir.path().join("Procfile.toml");
    std::fs::write(
        &cfg_path,
        "env_file = \".env\"\n\n[web]\ncommand = \"npm start\"\n",
    )
    .unwrap();

    let cfg = Config::load(&cfg_path).unwrap();
    assert_eq!(
        env_of(&cfg, "web").get("FROM_REL").map(String::as_str),
        Some("yes")
    );
}

#[test]
fn empty_env_file_is_treated_as_unset() {
    // An explicit empty path must be ignored, not read as a missing file.
    let cfg = load(
        r#"
env_file = ""

[web]
command  = "npm start"
env_file = ""
"#,
    );
    // Loads successfully and still inherits the OS environment.
    assert!(cfg.processes().contains_key("web"));
}

#[test]
fn resolves_global_environment_table() {
    let cfg = load(
        r#"
environment = { SHARED_KEY = "shared_val", ANOTHER = "two" }

[web]
command = "npm start"
"#,
    );
    let env = env_of(&cfg, "web");
    assert_eq!(env["SHARED_KEY"], "shared_val");
    assert_eq!(env["ANOTHER"], "two");
}

#[test]
fn env_merge_order_respects_precedence() {
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
command     = "echo hi"
exit_codes  = [0]
env_file    = {:?}
environment = {{ VAR = "inline" }}
"#,
        global.display().to_string(),
        proc.display().to_string(),
    );
    let cfg_path = dir.path().join("Procfile.toml");
    std::fs::write(&cfg_path, toml).unwrap();

    let env = env_of(&Config::load(&cfg_path).unwrap(), "app");
    // Per-process inline table wins outright.
    assert_eq!(env["VAR"], "inline");
    // Global env file value survives.
    assert_eq!(env["ONLY_GLOBAL"], "yes");
    // Global table overrides the global file.
    assert_eq!(env["GFILE_VS_GTABLE"], "from_gtable");
    assert_eq!(env["ONLY_GTABLE"], "yes");
    // Per-process file value survives.
    assert_eq!(env["ONLY_PROC"], "yes");
}

#[test]
fn missing_env_file_is_an_error() {
    let err = load_err("env_file = \"/nonexistent/.env\"\n\n[web]\ncommand = \"npm start\"\n");
    assert!(err.contains("env_file"), "{err}");

    let err = load_err("[web]\ncommand = \"npm start\"\nenv_file = \"/nonexistent/.env.web\"\n");
    assert!(err.contains("env_file"), "{err}");
}

#[test]
fn rejects_non_string_top_level_env_settings() {
    assert!(load_err("env_file = 42\n[web]\ncommand=\"x\"\n").contains("must be a string"));
    assert!(load_err("environment = \"nope\"\n[web]\ncommand=\"x\"\n").contains("must be a table"));
    assert!(
        load_err("[environment]\nGOOD=\"ok\"\nBAD=42\n[web]\ncommand=\"x\"\n")
            .contains("must be a string")
    );
}

// --- display name & color ---

#[test]
fn display_name_falls_back_to_key() {
    let cfg = load("[web]\ncommand = \"x\"\n");
    let web = &cfg.processes()["web"];
    assert_eq!(web.display_name("web"), "web");
    assert!(web.color().is_none());
}

#[test]
fn configured_name_and_named_color_resolve() {
    let cfg = load("[web]\ncommand = \"x\"\nname = \"Web Server\"\ncolor = \"bright-green\"\n");
    let web = &cfg.processes()["web"];
    assert_eq!(web.display_name("web"), "Web Server");
    assert_eq!(web.color(), Some(anstyle::AnsiColor::BrightGreen.into()));
}

#[test]
fn hex_color_resolves_to_rgb() {
    let cfg = load("[web]\ncommand = \"x\"\ncolor = \"#ff8800\"\n");
    let web = &cfg.processes()["web"];
    assert_eq!(
        web.color(),
        Some(anstyle::Color::Rgb(anstyle::RgbColor(0xff, 0x88, 0x00)))
    );
}

#[test]
fn invalid_color_is_rejected() {
    let err = load_err("[web]\ncommand = \"x\"\ncolor = \"chartreuse\"\n");
    assert!(err.contains("invalid color"), "{err}");
    let err = load_err("[web]\ncommand = \"x\"\ncolor = \"#zzzzzz\"\n");
    assert!(err.contains("invalid color"), "{err}");
    // A 6-byte multi-byte value must be rejected, not panic on a char boundary.
    let err = load_err("[web]\ncommand = \"x\"\ncolor = \"#héllo\"\n");
    assert!(err.contains("invalid color"), "{err}");
}

// --- dependency graph: reverse traversal ---

#[test]
fn dependents_collects_transitive_chain() {
    // db <- api <- web, and a standalone cache; stopping db hits api and web.
    let cfg = load(
        r#"
[db]
command = "x"
[api]
command = "x"
depends_on = ["db"]
[web]
command = "x"
depends_on = ["api"]
[cache]
command = "x"
"#,
    );
    let adj = cfg.adjacency();
    let mut deps: Vec<String> = proccie::config::dependents(&["db".to_owned()], &adj)
        .into_iter()
        .collect();
    deps.sort();
    assert_eq!(deps, vec!["api".to_owned(), "web".to_owned()]);

    // A leaf with no dependents yields an empty set.
    assert!(proccie::config::dependents(&["web".to_owned()], &adj).is_empty());
    // cache is independent.
    assert!(proccie::config::dependents(&["cache".to_owned()], &adj).is_empty());
}
