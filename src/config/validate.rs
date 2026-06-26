use std::collections::{BTreeMap, HashSet};

use super::error::{ConfigError, ValidationIssue, ValidationIssueKind};
use super::graph;
use super::parse::parse_color;
use super::types::Process;

/// Checks every process for correctness, then detects dependency cycles.
pub fn validate(procs: &BTreeMap<String, Process>) -> Result<(), ConfigError> {
    let mut issues: Vec<ValidationIssue> = Vec::new();
    for (name, proc) in procs {
        validate_process(name, proc, procs, &mut issues);
    }
    check_unique_names(procs, &mut issues);

    if !issues.is_empty() {
        return Err(ConfigError::Validation(issues));
    }

    match graph::detect_cycle(&super::adjacency_of(procs)) {
        Some(cycle) => Err(ConfigError::Cycle(cycle.join(" -> "))),
        None => Ok(()),
    }
}

/// Collects validation issues for one process: required command, exit_codes /
/// readiness rules, dependency sanity, and retry count.
fn validate_process(
    name: &str,
    proc: &Process,
    procs: &BTreeMap<String, Process>,
    issues: &mut Vec<ValidationIssue>,
) {
    let mut issue = |kind: ValidationIssueKind| {
        issues.push(ValidationIssue {
            process: name.to_owned(),
            kind,
        });
    };

    if proc.command.is_empty() {
        issue(ValidationIssueKind::MissingCommand);
    }

    if let Some(readiness) = &proc.readiness {
        if !proc.exit_codes.is_empty() {
            issue(ValidationIssueKind::ExitCodesAndReadiness);
        }
        if readiness.command.is_empty() {
            issue(ValidationIssueKind::EmptyReadinessCommand);
        }
        // Retries fire on exit, not a failed readiness window, so they'd be ignored.
        if proc.max_retries > 0 {
            issue(ValidationIssueKind::RetriesWithReadiness);
        }
    }

    let mut seen = HashSet::new();
    for dep in &proc.depends_on {
        if !seen.insert(dep) {
            issue(ValidationIssueKind::DuplicateDependency(dep.clone()));
        } else if dep == name {
            issue(ValidationIssueKind::SelfDependency);
        } else if !procs.contains_key(dep) {
            issue(ValidationIssueKind::UndefinedDependency(dep.clone()));
        }
    }

    if proc.max_retries < 0 {
        issue(ValidationIssueKind::NegativeMaxRetries);
    }

    if let Some(color) = &proc.color
        && parse_color(color).is_none()
    {
        issue(ValidationIssueKind::InvalidColor(color.clone()));
    }
}

/// Flags any display name shared by more than one process: names label tabs
/// and log prefixes, so a collision makes two processes indistinguishable.
fn check_unique_names(procs: &BTreeMap<String, Process>, issues: &mut Vec<ValidationIssue>) {
    let mut seen: BTreeMap<&str, &str> = BTreeMap::new();
    for (key, proc) in procs {
        let display = proc.display_name(key);
        if seen.insert(display, key).is_some() {
            issues.push(ValidationIssue {
                process: key.clone(),
                kind: ValidationIssueKind::DuplicateName(display.to_owned()),
            });
        }
    }
}
