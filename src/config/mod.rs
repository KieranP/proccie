//! TOML configuration: parsing, validation, cycle detection, environment
//! resolution, dependency ordering, and filtering. Entry point: [`Config::load`].

mod environment;
mod error;
mod graph;
mod parse;
mod types;
mod validate;

use std::collections::{BTreeMap, HashSet};
use std::path::Path;
use std::time::Duration;

pub use error::{ConfigError, ValidationIssue, ValidationIssueKind};
pub use types::{ExitCodes, Process, Readiness, ReadyWhen, parse_duration};

/// Maximum time to wait for a readiness command to succeed before
/// considering the dependency failed.
pub const DEFAULT_READINESS_TIMEOUT: Duration = Duration::from_secs(30);

/// Time between readiness check attempts.
pub const DEFAULT_READINESS_INTERVAL: Duration = Duration::from_secs(1);

/// A fully-loaded configuration: a name-keyed map of process definitions with
/// resolved environments, backed by a [`BTreeMap`] for alphabetical iteration.
#[derive(Debug, Clone, Default)]
pub struct Config {
    /// Private so mutation stays inside `Config`, preserving the validated
    /// invariants (no cycles, no dangling deps); read via [`Config::processes`].
    processes: BTreeMap<String, Process>,
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

        let parsed = parse::parse(&data, path)?;
        validate::validate(&parsed.processes)?;
        let processes = environment::resolve(parsed, base_env)?;

        Ok(Config { processes })
    }

    /// Returns the processes, keyed by name in alphabetical order.
    pub fn processes(&self) -> &BTreeMap<String, Process> {
        &self.processes
    }

    /// Returns all process names in alphabetical order.
    pub fn names(&self) -> Vec<&str> {
        self.processes.keys().map(String::as_str).collect()
    }

    /// Returns process names in deterministic topological order (dependencies
    /// first; ties alphabetical), derived on demand so filtering can't stale it.
    pub fn start_order(&self) -> Vec<String> {
        // Load-time validation rejects cycles, and filtering only removes.
        graph::topo_order(&self.processes)
    }

    /// Restricts the config: `only` keeps the named processes and their deps;
    /// `except` drops them and prunes dangling deps. Both, or unknown names, error.
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
            let keep = graph::reachable(only, &self.processes);
            self.processes.retain(|name, _| keep.contains(name));
        }

        if !except.is_empty() {
            for name in except {
                self.processes.remove(name);
            }
            self.prune_dangling_deps();
        }

        Ok(())
    }

    /// Drops `depends_on` entries that reference processes no longer
    /// present, so the runner never waits on a process that can't start.
    fn prune_dangling_deps(&mut self) {
        let present: HashSet<String> = self.processes.keys().cloned().collect();
        for proc in self.processes.values_mut() {
            proc.depends_on.retain(|dep| present.contains(dep));
        }
    }
}
