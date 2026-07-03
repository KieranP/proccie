use std::collections::BTreeMap;
use std::path::Path;

use toml::Value;

use super::error::ConfigError;
use super::types::Process;

/// The raw, parsed config before validation and environment resolution.
pub struct Parsed {
    pub global_env_file: Option<String>,
    pub global_env: BTreeMap<String, String>,
    pub processes: BTreeMap<String, Process>,
}

/// Decodes TOML into processes, splitting out top-level `env_file`/`environment`.
/// Remaining tables become processes; other scalar keys are rejected.
pub fn parse(data: &str, path: &Path) -> Result<Parsed, ConfigError> {
    let mut table: toml::Table = data.parse().map_err(|source| ConfigError::Toml {
        path: path.to_path_buf(),
        source: Box::new(source),
    })?;

    let global_env_file = take_env_file(&mut table, path)?;
    let global_env = take_environment(&mut table, path)?;

    let mut processes = BTreeMap::new();
    for (name, value) in table {
        if !value.is_table() {
            return Err(ConfigError::UnknownTopLevelKey {
                path: path.to_path_buf(),
                key: name,
            });
        }

        let proc = value.try_into().map_err(|source| ConfigError::Process {
            path: path.to_path_buf(),
            name: name.clone(),
            source: Box::new(source),
        })?;
        processes.insert(name, proc);
    }

    Ok(Parsed {
        global_env_file,
        global_env,
        processes,
    })
}

/// Removes and validates the top-level `env_file` key, if present.
fn take_env_file(table: &mut toml::Table, path: &Path) -> Result<Option<String>, ConfigError> {
    match table.remove("env_file") {
        None => Ok(None),
        Some(Value::String(s)) => Ok(Some(s)),
        Some(_) => Err(ConfigError::EnvFileNotString {
            path: path.to_path_buf(),
        }),
    }
}

/// Removes and validates the top-level `environment` table, if present.
fn take_environment(
    table: &mut toml::Table,
    path: &Path,
) -> Result<BTreeMap<String, String>, ConfigError> {
    let Some(value) = table.remove("environment") else {
        return Ok(BTreeMap::new());
    };

    let Value::Table(entries) = value else {
        return Err(ConfigError::EnvironmentNotTable {
            path: path.to_path_buf(),
        });
    };

    entries
        .into_iter()
        .map(|(key, value)| match value {
            Value::String(s) => Ok((key, s)),
            _ => Err(ConfigError::EnvironmentValueNotString {
                path: path.to_path_buf(),
                key,
            }),
        })
        .collect()
}
