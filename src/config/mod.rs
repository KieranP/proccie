//! TOML configuration: parsing, validation, cycle detection, environment
//! resolution, dependency ordering, and filtering. Entry point: [`Config::load`].

mod environment;
mod error;
mod graph;
mod procfile;
mod types;
mod validate;

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

pub use error::{ConfigError, ConfigWarning, ValidationIssue, ValidationIssueKind};
pub use graph::{Adjacency, dependents, reachable, topo_order};
pub use types::{ExitCodes, Process, Readiness, ReadyWhen, parse_duration};

/// Maximum time to wait for a readiness command to succeed before
/// considering the dependency failed.
pub const DEFAULT_READINESS_TIMEOUT: Duration = Duration::from_secs(30);

/// Time between readiness check attempts.
pub const DEFAULT_READINESS_INTERVAL: Duration = Duration::from_secs(1);

/// Config filenames tried in order when no path is given: TOML, then plain.
pub const DEFAULT_CONFIG_FILES: [&str; 2] = ["Procfile.toml", "Procfile"];

/// Sibling env file auto-loaded for a plain Procfile, foreman-style, if it exists.
const DEFAULT_ENV_FILE: &str = ".env";

/// The explicit path if given, else the first [`DEFAULT_CONFIG_FILES`] that exists.
pub fn resolve_path(explicit: Option<&Path>) -> Result<PathBuf, ConfigError> {
    if let Some(path) = explicit {
        return Ok(path.to_path_buf());
    }
    DEFAULT_CONFIG_FILES
        .iter()
        .map(PathBuf::from)
        .find(|path| path.exists())
        .ok_or(ConfigError::NoDefaultConfig)
}

/// A fully-loaded configuration: a name-keyed map of process definitions with
/// resolved environments, backed by a [`BTreeMap`] for alphabetical iteration.
#[derive(Debug, Clone, Default)]
pub struct Config {
    /// Private so mutation stays inside `Config`, preserving the validated
    /// invariants (no cycles, no dangling deps); read via [`Config::processes`].
    processes: BTreeMap<String, Process>,
    /// The file this config was loaded from; read via [`Config::file_path`].
    file_path: PathBuf,
}

impl Config {
    /// Reads, parses, validates, and resolves a TOML config file. Top-level
    /// `env_file`/`environment` keys are allowed; other scalar keys are rejected.
    pub fn load(path: impl AsRef<Path>) -> Result<Config, ConfigError> {
        Self::load_with_env(path, environment::os_env())
    }

    /// Like [`Config::load`], but layering on `base_env` instead of the OS
    /// environment, so tests can pin the inherited layer deterministically.
    pub fn load_with_env(
        path: impl AsRef<Path>,
        base_env: BTreeMap<String, String>,
    ) -> Result<Config, ConfigError> {
        let path = path.as_ref();

        let data = std::fs::read_to_string(path).map_err(|source| ConfigError::Read {
            path: path.to_path_buf(),
            source,
        })?;

        // Relative `env_file` paths resolve against the config file's directory.
        let config_dir = path.parent().unwrap_or_else(|| Path::new(""));

        // TOML by extension; anything else (notably a bare `Procfile`) is plain format.
        let parsed = if is_toml(path) {
            procfile::parse_toml(&data, path)?
        } else {
            let mut parsed = procfile::parse_plain(&data, path)?;
            // foreman convention: apply a sibling `.env` to all processes when present.
            if config_dir.join(DEFAULT_ENV_FILE).exists() {
                parsed.global_env_file = Some(DEFAULT_ENV_FILE.to_owned());
            }
            parsed
        };
        validate::validate(&parsed.processes)?;
        let processes = environment::resolve(parsed, base_env, config_dir)?;

        Ok(Config {
            processes,
            file_path: path.to_path_buf(),
        })
    }

    /// Builds the `name -> depends_on` adjacency map for graph traversal.
    pub fn adjacency(&self) -> Adjacency {
        adjacency_of(&self.processes)
    }

    /// Returns the processes, keyed by name in alphabetical order.
    pub fn processes(&self) -> &BTreeMap<String, Process> {
        &self.processes
    }

    /// The file this config was loaded from.
    pub fn file_path(&self) -> &Path {
        &self.file_path
    }

    /// Non-fatal concerns: a depended-on process with no readiness signal
    /// releases its dependents at launch, which is valid but easy to mistake.
    pub fn warnings(&self) -> Vec<ConfigWarning> {
        let depended_on: HashSet<&str> = self
            .processes
            .values()
            .flat_map(|p| p.depends_on.iter().map(String::as_str))
            .collect();

        self.processes
            .iter()
            .filter(|(name, p)| {
                depended_on.contains(name.as_str())
                    && p.exit_codes.is_empty()
                    && p.readiness.is_none()
            })
            .map(|(name, _)| ConfigWarning::DependedOnReleasesAtLaunch(name.clone()))
            .collect()
    }

    /// Returns all process names in alphabetical order.
    pub fn names(&self) -> Vec<&str> {
        self.processes.keys().map(String::as_str).collect()
    }

    /// The display name (the `name` override, else the key) for each process, in
    /// alphabetical key order. Sizes the log prefix width and the tab labels.
    pub fn display_names(&self) -> Vec<String> {
        self.processes
            .iter()
            .map(|(key, proc)| proc.display_name(key).to_owned())
            .collect()
    }

    /// Restricts the config: `only` keeps the named processes and their deps;
    /// `except` drops them and everything that depends on them. Both, or unknown names, error.
    pub fn filter(&mut self, only: &[String], except: &[String]) -> Result<(), ConfigError> {
        if !only.is_empty() && !except.is_empty() {
            return Err(ConfigError::OnlyAndExcept);
        }

        if let Some(name) = only.iter().find(|n| !self.processes.contains_key(*n)) {
            return Err(ConfigError::UnknownOnly(name.clone()));
        }
        if let Some(name) = except.iter().find(|n| !self.processes.contains_key(*n)) {
            return Err(ConfigError::UnknownExcept(name.clone()));
        }

        if !only.is_empty() {
            let keep = graph::reachable(only, &self.adjacency());
            self.processes.retain(|name, _| keep.contains(name));
        }

        if !except.is_empty() {
            // Drop the named processes and everything that transitively depends on them.
            let mut drop = graph::dependents(except, &self.adjacency());
            drop.extend(except.iter().cloned());
            self.processes.retain(|name, _| !drop.contains(name));
        }

        Ok(())
    }
}

/// Whether a path should be read as TOML: a `.toml` extension, case-insensitive.
fn is_toml(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("toml"))
}

/// Builds a `name -> depends_on` adjacency map from a process table.
pub(crate) fn adjacency_of(procs: &BTreeMap<String, Process>) -> Adjacency {
    procs
        .iter()
        .map(|(name, proc)| (name.clone(), proc.depends_on.clone()))
        .collect()
}
