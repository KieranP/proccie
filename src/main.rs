//! Command-line entry point for proccie.

use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use clap::{Parser, Subcommand};
use tokio::signal::unix::{Signal, SignalKind, signal};

use proccie::config::{Config, parse_duration};
use proccie::logger::{Destination, LogLevel, Logger, TaggedWriter};
use proccie::runner::Runner;
use proccie::service::Service;
use proccie::theme::Theme;
use proccie::tui;

const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Runs and supervises multiple processes with dependency ordering, readiness
/// checks, and graceful shutdown. A second SIGINT/SIGTERM forces shutdown.
#[derive(Parser)]
#[command(name = "proccie", version, about, long_about = None)]
struct Cli {
    /// Path to the TOML config file.
    #[arg(short = 'f', long = "config", default_value = "Procfile.toml")]
    config: PathBuf,

    /// Shutdown timeout before SIGKILL (e.g. `10s`, `500ms`).
    #[arg(short = 't', long, default_value = "10s", value_parser = parse_duration)]
    timeout: Duration,

    /// Delay after a forced SIGKILL before hard-exiting.
    #[arg(short = 'k', long = "force-delay", default_value = "500ms", value_parser = parse_duration)]
    force_delay: Duration,

    /// Processes to run, with their dependencies (repeatable or comma-separated).
    #[arg(long, value_delimiter = ',')]
    only: Vec<String>,

    /// Processes to exclude, with anything that depends on them (repeatable or comma-separated).
    #[arg(long, value_delimiter = ',')]
    except: Vec<String>,

    /// Minimum severity to log: `debug`, `info`, `warn`, or `error`.
    #[arg(long = "log-level", default_value = "info")]
    log_level: LogLevel,

    /// Disable the interactive TUI and stream plain prefixed output instead.
    #[arg(long = "no-tui")]
    no_tui: bool,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Check that the configuration file is valid, then exit.
    Validate,
}

#[tokio::main]
async fn main() {
    std::process::exit(run().await);
}

async fn run() -> i32 {
    let cli = Cli::parse();

    if let Some(Command::Validate) = cli.command {
        return validate(&cli.config);
    }

    let config = match load_runnable_config(&cli) {
        Ok(config) => config,
        Err(code) => return code,
    };
    warn_all(&config);

    // The TUI launches when stdout is a TTY, unless `--no-tui` forces plain mode.
    let is_tty = std::io::stdout().is_terminal();
    let use_tui = is_tty && !cli.no_tui;
    // Detect the terminal background on a TTY so colors adapt; pipes assume dark.
    // Blocking I/O, so run it off the async worker.
    let theme = if is_tty {
        tokio::task::spawn_blocking(Theme::detect_theme)
            .await
            .unwrap_or_default()
    } else {
        Theme::default()
    };
    let logger = build_logger(&config, &cli, use_tui, theme);

    // Built once, then shared: color resolution borrows it, the runner owns it.
    let adjacency = config.adjacency();
    let services = match Service::build_all(&config, &adjacency, &logger, theme) {
        Ok(services) => services,
        Err(e) => return fail(&e),
    };

    let runner = Runner::new(
        Arc::clone(&services),
        adjacency,
        Arc::clone(logger.system()),
        cli.timeout,
    );

    if let Err(e) = spawn_signal_handler(
        runner.clone(),
        Arc::clone(logger.system()),
        cli.force_delay,
        use_tui,
    ) {
        return fail(&e);
    }

    logger.system().info(format!(
        "proccie {VERSION} starting with {} process(es)",
        config.processes().len()
    ));

    let code = match supervise(&services, &runner, &logger, use_tui, theme, cli.force_delay).await {
        Ok(code) => code,
        Err(e) => return fail(&e),
    };

    logger
        .system()
        .info(format!("proccie exiting (code {code})"));
    code
}

/// Loads the config and reports whether it is valid.
fn validate(path: &Path) -> i32 {
    match Config::load(path) {
        Ok(config) => {
            warn_all(&config);
            println!(
                "{}: valid ({} process(es): {})",
                path.display(),
                config.processes().len(),
                config.names().join(", "),
            );
            0
        }
        Err(e) => fail(&e),
    }
}

/// Loads the config and applies CLI filters, returning the runnable process
/// set or the exit code to fail with.
fn load_runnable_config(cli: &Cli) -> Result<Config, i32> {
    let mut config = match Config::load(&cli.config) {
        Ok(config) => config,
        Err(e) => return Err(fail(&e)),
    };
    if let Err(e) = config.filter(
        &non_empty_trimmed(&cli.only),
        &non_empty_trimmed(&cli.except),
    ) {
        return Err(fail(&e));
    }

    if config.processes().is_empty() {
        eprintln!("error: no processes defined in {}", cli.config.display());
        return Err(1);
    }
    Ok(config)
}

/// Prints each non-fatal config warning to stderr; the config still runs.
fn warn_all(config: &Config) {
    for warning in config.warnings() {
        eprintln!("warning: {warning}");
    }
}

/// Builds the logger: store-backed for the TUI, a plain ANSI stream otherwise,
/// with the prefix width sized to the service display names.
fn build_logger(config: &Config, cli: &Cli, use_tui: bool, theme: Theme) -> Arc<Logger> {
    let dest = if use_tui {
        Destination::Store
    } else {
        Destination::Stream
    };
    let display_names = config.display_names();
    Logger::new(
        dest,
        display_names.iter().map(String::as_str),
        cli.log_level,
        theme,
    )
}

/// Prints an error to stderr and returns the failure exit code.
fn fail(error: &dyn std::error::Error) -> i32 {
    eprintln!("error: {error}");
    1
}

/// Watches for termination signals: the first triggers a graceful shutdown, a
/// second forces an immediate kill. Handlers install up front so failure errors.
fn spawn_signal_handler(
    runner: Runner,
    system: Arc<TaggedWriter>,
    force_delay: Duration,
    use_tui: bool,
) -> std::io::Result<()> {
    let mut sigint = signal(SignalKind::interrupt())?;
    let mut sigterm = signal(SignalKind::terminate())?;

    tokio::spawn(async move {
        let first = next_signal(&mut sigint, &mut sigterm).await;
        // A signal terminates the program; set quit before logging so the redraw sees it.
        runner.request_quit();
        system.info(format!("received signal: {first}"));
        runner.shutdown();

        let second = next_signal(&mut sigint, &mut sigterm).await;
        system.warn(format!(
            "received second signal: {second}, forcing shutdown"
        ));
        runner.force_shutdown();

        tokio::time::sleep(force_delay).await;
        // The forced exit bypasses the TUI's restore, so restore the terminal first.
        if use_tui {
            tui::restore_terminal();
        }
        std::process::exit(1);
    });

    Ok(())
}

/// Runs the supervisor: through the TUI when enabled, else streaming plain output.
async fn supervise(
    services: &Arc<[Service]>,
    runner: &Runner,
    logger: &Logger,
    use_tui: bool,
    theme: Theme,
    force_delay: Duration,
) -> std::io::Result<i32> {
    if use_tui {
        tui::run(services, runner, logger, theme, force_delay).await
    } else {
        Ok(runner.run().await)
    }
}

/// Trims each value and drops the empties from a comma-split CLI flag.
fn non_empty_trimmed(values: &[String]) -> Vec<String> {
    values
        .iter()
        .map(|v| v.trim().to_owned())
        .filter(|v| !v.is_empty())
        .collect()
}

/// Waits for the next SIGINT or SIGTERM, returning its name.
async fn next_signal(sigint: &mut Signal, sigterm: &mut Signal) -> &'static str {
    tokio::select! {
        _ = sigint.recv() => "SIGINT",
        _ = sigterm.recv() => "SIGTERM",
    }
}
