//! Dependency-graph traversal shared by validation, start ordering, and
//! filtering: one DFS yields a deterministic topological order and detects
//! cycles; [`reachable`] collects transitive dependencies.

use std::collections::{BTreeMap, HashMap, HashSet};

use super::types::Process;

/// DFS node color for cycle detection.
#[derive(Clone, Copy)]
enum Mark {
    /// On the current DFS stack; revisiting one closes a cycle.
    Visiting,
    /// Fully explored.
    Visited,
}

/// Returns the names forming a dependency cycle, if any.
pub fn detect_cycle(processes: &BTreeMap<String, Process>) -> Option<Vec<String>> {
    dfs(processes).err()
}

/// Returns process names in topological order (deps first, ties alphabetical).
/// Callers must have rejected cycles already (validation does).
pub fn topo_order(processes: &BTreeMap<String, Process>) -> Vec<String> {
    dfs(processes).unwrap_or_else(|_| unreachable!("validated config has no cycles"))
}

/// One DFS over the dependency graph: the topological order, or the names
/// forming a cycle. Unknown deps are skipped.
fn dfs(processes: &BTreeMap<String, Process>) -> Result<Vec<String>, Vec<String>> {
    let mut marks: HashMap<&str, Mark> = HashMap::with_capacity(processes.len());
    let mut order = Vec::with_capacity(processes.len());

    // BTreeMap keys are already sorted, giving a stable starting order.
    for name in processes.keys() {
        visit(name, processes, &mut marks, &mut Vec::new(), &mut order)?;
    }

    Ok(order)
}

/// Collects `start` and everything transitively reachable via `depends_on`.
/// Unknown names are kept but contribute no edges.
pub fn reachable(start: &[String], processes: &BTreeMap<String, Process>) -> HashSet<String> {
    let mut seen = HashSet::new();
    let mut stack: Vec<&str> = start.iter().map(String::as_str).collect();

    while let Some(name) = stack.pop() {
        if !seen.insert(name.to_owned()) {
            continue;
        }
        if let Some(proc) = processes.get(name) {
            stack.extend(proc.depends_on.iter().map(String::as_str));
        }
    }

    seen
}

fn visit<'a>(
    name: &'a str,
    processes: &'a BTreeMap<String, Process>,
    marks: &mut HashMap<&'a str, Mark>,
    path: &mut Vec<&'a str>,
    order: &mut Vec<String>,
) -> Result<(), Vec<String>> {
    match marks.get(name).copied() {
        Some(Mark::Visited) => return Ok(()),
        Some(Mark::Visiting) => {
            // Report just the cycle: from the earlier visit of `name` to here.
            let start = path.iter().position(|&n| n == name).unwrap_or(0);
            let cycle = path[start..]
                .iter()
                .chain(std::iter::once(&name))
                .map(|&n| n.to_owned())
                .collect();
            return Err(cycle);
        }
        None => {}
    }

    marks.insert(name, Mark::Visiting);
    path.push(name);

    if let Some(proc) = processes.get(name) {
        let mut deps: Vec<&str> = proc.depends_on.iter().map(String::as_str).collect();
        deps.sort_unstable();
        for dep in deps {
            visit(dep, processes, marks, path, order)?;
        }
    }

    path.pop();
    marks.insert(name, Mark::Visited);
    order.push(name.to_owned());
    Ok(())
}
