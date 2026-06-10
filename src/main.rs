//! Command-line entry point for proccie.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use clap::{Parser, Subcommand};
use tokio::signal::unix::{Signal, SignalKind, signal};

use proccie::config::{Config, parse_duration};
use proccie::mux::Mux;
use proccie::runner::Runner;

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

    /// Processes to exclude (repeatable or comma-separated).
    #[arg(long, value_delimiter = ',')]
    except: Vec<String>,

    /// Show system log lines.
    #[arg(long)]
    debug: bool,

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

    // Strip ANSI styling automatically when stdout isn't a terminal.
    let stdout = anstream::AutoStream::auto(std::io::stdout());
    let width = Mux::prefix_width(config.names());
    let mux = Mux::new(stdout, width, cli.debug);
    let count = config.processes().len();
    let runner = Runner::new(Arc::new(config), Arc::clone(&mux), cli.timeout);

    spawn_signal_handler(runner.clone(), Arc::clone(&mux), cli.force_delay);

    mux.system_log(format!(
        "proccie {VERSION} starting with {count} process(es)"
    ));
    let code = runner.run().await;
    mux.system_log(format!("proccie exiting (code {code})"));
    code
}

/// Loads the config and applies CLI filters, returning the runnable process
/// set or the exit code to fail with.
fn load_runnable_config(cli: &Cli) -> Result<Config, i32> {
    let mut config = Config::load(&cli.config).map_err(|e| fail(&e))?;
    config
        .filter(
            &non_empty_trimmed(&cli.only),
            &non_empty_trimmed(&cli.except),
        )
        .map_err(|e| fail(&e))?;

    if config.processes().is_empty() {
        eprintln!("error: no processes defined in {}", cli.config.display());
        return Err(1);
    }
    Ok(config)
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

/// Prints an error to stderr and returns the failure exit code.
fn fail(error: &dyn std::error::Error) -> i32 {
    eprintln!("error: {error}");
    1
}

/// Prints each non-fatal config warning to stderr; the config still runs.
fn warn_all(config: &Config) {
    for warning in config.warnings() {
        eprintln!("warning: {warning}");
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

/// Watches for termination signals: the first triggers a graceful shutdown,
/// a second forces an immediate kill and hard exit.
fn spawn_signal_handler(runner: Runner, mux: Arc<Mux>, force_delay: Duration) {
    tokio::spawn(async move {
        let mut sigint = signal(SignalKind::interrupt()).expect("install SIGINT handler");
        let mut sigterm = signal(SignalKind::terminate()).expect("install SIGTERM handler");

        let first = next_signal(&mut sigint, &mut sigterm).await;
        mux.system_log(format!("received signal: {first}"));
        runner.shutdown();

        let second = next_signal(&mut sigint, &mut sigterm).await;
        mux.system_log(format!(
            "received second signal: {second}, forcing shutdown"
        ));
        runner.force_shutdown();

        tokio::time::sleep(force_delay).await;
        std::process::exit(1);
    });
}

/// Waits for the next SIGINT or SIGTERM, returning its name.
async fn next_signal(sigint: &mut Signal, sigterm: &mut Signal) -> &'static str {
    tokio::select! {
        _ = sigint.recv() => "SIGINT",
        _ = sigterm.recv() => "SIGTERM",
    }
}
