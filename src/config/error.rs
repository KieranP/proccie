use std::path::PathBuf;

/// Errors produced while loading, validating, or filtering a config.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("reading config {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("parsing config {path}: {source}")]
    Toml {
        path: PathBuf,
        #[source]
        source: Box<toml::de::Error>,
    },

    #[error("parsing config {path}: top-level env_file must be a string")]
    EnvFileNotString { path: PathBuf },

    #[error("parsing config {path}: top-level environment must be a table")]
    EnvironmentNotTable { path: PathBuf },

    #[error("parsing config {path}: top-level environment value for {key:?} must be a string")]
    EnvironmentValueNotString { path: PathBuf, key: String },

    #[error("parsing config {path}: unknown top-level key {key:?} (expected a process table)")]
    UnknownTopLevelKey { path: PathBuf, key: String },

    #[error("parsing config {path}: process {name:?}: {source}")]
    Process {
        path: PathBuf,
        name: String,
        #[source]
        source: Box<toml::de::Error>,
    },

    #[error("config validation failed:\n  {}", format_issues(.0))]
    Validation(Vec<ValidationIssue>),

    #[error("dependency cycle detected: {0}")]
    Cycle(String),

    #[error("{scope} env_file: {source}")]
    EnvFile {
        scope: String,
        #[source]
        source: dotenvy::Error,
    },

    #[error("cannot specify both --only and --except")]
    OnlyAndExcept,

    #[error("--only: unknown process {0:?}")]
    UnknownOnly(String),

    #[error("--except: unknown process {0:?}")]
    UnknownExcept(String),
}

/// One validation problem, tied to the process that triggered it.
#[derive(Debug, thiserror::Error)]
#[error("process {process:?}: {kind}")]
pub struct ValidationIssue {
    pub process: String,
    pub kind: ValidationIssueKind,
}

/// The validation rule a process broke.
#[derive(Debug, thiserror::Error)]
pub enum ValidationIssueKind {
    #[error("missing required key \"command\"")]
    MissingCommand,

    #[error("\"exit_codes\" and \"readiness\" are mutually exclusive")]
    ExitCodesAndReadiness,

    #[error("readiness command cannot be empty")]
    EmptyReadinessCommand,

    #[error("duplicate dependency {0:?}")]
    DuplicateDependency(String),

    #[error("cannot depend on itself")]
    SelfDependency,

    #[error("depends on {0:?}, which is not defined")]
    UndefinedDependency(String),

    #[error("max_retries must be non-negative")]
    NegativeMaxRetries,
}

/// Indents each issue onto its own line for the `Validation` message.
fn format_issues(issues: &[ValidationIssue]) -> String {
    issues
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("\n  ")
}
