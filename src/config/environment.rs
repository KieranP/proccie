use std::collections::BTreeMap;

use super::error::ConfigError;
use super::parse::Parsed;
use super::types::Process;

/// Snapshot of the OS environment. Non-UTF-8 vars are skipped rather than panic.
pub fn os_env() -> BTreeMap<String, String> {
    std::env::vars_os()
        .filter_map(|(k, v)| Some((k.into_string().ok()?, v.into_string().ok()?)))
        .collect()
}

/// Resolves each process's environment by layering, lowest precedence first:
/// `base_env`, global `env_file`, global `environment`, per-process `env_file`,
/// then per-process `environment`.
pub fn resolve(
    parsed: Parsed,
    base_env: BTreeMap<String, String>,
) -> Result<BTreeMap<String, Process>, ConfigError> {
    // Shared base layers, built once.
    let mut base = base_env;
    apply_layer(
        &mut base,
        &parsed.global_env_file,
        &parsed.global_env,
        "top-level".to_owned(),
    )?;

    let mut processes = parsed.processes;
    for (name, proc) in &mut processes {
        let mut env = base.clone();
        apply_layer(
            &mut env,
            &proc.env_file,
            &proc.environment,
            format!("process {name:?}"),
        )?;
        proc.env = env;
    }

    Ok(processes)
}

/// Layers one scope onto `env`: its `env_file` (if set), then its explicit
/// `environment` entries. `scope` names the layer in env-file errors.
fn apply_layer(
    env: &mut BTreeMap<String, String>,
    env_file: &Option<String>,
    environment: &BTreeMap<String, String>,
    scope: String,
) -> Result<(), ConfigError> {
    if let Some(path) = non_empty(env_file) {
        env.extend(read_env_file(path).map_err(|source| ConfigError::EnvFile { scope, source })?);
    }
    env.extend(environment.iter().map(|(k, v)| (k.clone(), v.clone())));
    Ok(())
}

/// Treats an absent or empty `env_file` path as "unset", so an explicit
/// `env_file = ""` is ignored rather than read as a (missing) file.
fn non_empty(path: &Option<String>) -> Option<&str> {
    path.as_deref().filter(|p| !p.is_empty())
}

/// Reads a dotenv-style file into a map without mutating the process
/// environment. Variable interpolation is handled by `dotenvy`.
fn read_env_file(path: &str) -> Result<BTreeMap<String, String>, dotenvy::Error> {
    dotenvy::from_path_iter(path)?.collect()
}
