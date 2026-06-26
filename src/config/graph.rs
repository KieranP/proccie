//! Dependency-graph traversal shared by validation, start ordering, and
//! filtering: one DFS yields a deterministic topological order and detects
//! cycles; [`reachable`] collects transitive dependencies and [`dependents`]
//! their reverse. All operate over a `name -> depends_on` adjacency map.

use std::collections::{BTreeMap, HashMap, HashSet};

/// A `name -> depends_on` adjacency map.
pub type Adjacency = BTreeMap<String, Vec<String>>;

/// DFS node color for cycle detection.
#[derive(Clone, Copy)]
enum Mark {
    /// On the current DFS stack; revisiting one closes a cycle.
    Visiting,
    /// Fully explored.
    Visited,
}

/// Returns the names forming a dependency cycle, if any.
pub fn detect_cycle(adj: &Adjacency) -> Option<Vec<String>> {
    dfs(adj).err()
}

/// Returns process names in topological order (deps first, ties alphabetical).
/// Validation rejects cycles; if one slips through, all names still return (order then arbitrary).
pub fn topo_order(adj: &Adjacency) -> Vec<String> {
    dfs(adj).unwrap_or_else(|_| adj.keys().cloned().collect())
}

/// Collects `start` and everything transitively reachable via `depends_on`.
/// Unknown names are kept but contribute no edges.
pub fn reachable(start: &[String], adj: &Adjacency) -> HashSet<String> {
    let mut seen = HashSet::new();
    let mut stack: Vec<&str> = start.iter().map(String::as_str).collect();

    while let Some(name) = stack.pop() {
        if !seen.insert(name.to_owned()) {
            continue;
        }
        if let Some(deps) = adj.get(name) {
            stack.extend(deps.iter().map(String::as_str));
        }
    }

    seen
}

/// Collects everything that transitively depends on any of `roots` (the reverse
/// of [`reachable`]), excluding the roots themselves.
pub fn dependents(roots: &[String], adj: &Adjacency) -> HashSet<String> {
    // Reverse the edges once: dep -> names that depend on it.
    let mut reverse: HashMap<&str, Vec<&str>> = HashMap::new();
    for (name, deps) in adj {
        for dep in deps {
            reverse.entry(dep.as_str()).or_default().push(name.as_str());
        }
    }

    let root_set: HashSet<&str> = roots.iter().map(String::as_str).collect();
    let mut found = HashSet::new();
    let mut stack: Vec<&str> = roots.iter().map(String::as_str).collect();

    while let Some(name) = stack.pop() {
        if let Some(deps) = reverse.get(name) {
            for &dep in deps {
                if !root_set.contains(dep) && found.insert(dep.to_owned()) {
                    stack.push(dep);
                }
            }
        }
    }

    found
}

/// One DFS over the dependency graph: the topological order, or the names
/// forming a cycle. Unknown deps are skipped.
fn dfs(adj: &Adjacency) -> Result<Vec<String>, Vec<String>> {
    let mut marks: HashMap<&str, Mark> = HashMap::with_capacity(adj.len());
    let mut order = Vec::with_capacity(adj.len());

    // BTreeMap keys are already sorted, giving a stable starting order.
    for name in adj.keys() {
        visit(name, adj, &mut marks, &mut Vec::new(), &mut order)?;
    }

    Ok(order)
}

fn visit<'a>(
    name: &'a str,
    adj: &'a Adjacency,
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

    if let Some(deps) = adj.get(name) {
        let mut deps: Vec<&str> = deps.iter().map(String::as_str).collect();
        deps.sort_unstable();
        for dep in deps {
            visit(dep, adj, marks, path, order)?;
        }
    }

    path.pop();
    marks.insert(name, Mark::Visited);
    order.push(name.to_owned());
    Ok(())
}
