//! Dependency graph rules. Cycles are rejected at creation time, so the graph
//! is always a DAG and finishing checks never loop.

use std::collections::{HashMap, HashSet};

use crate::ids::TaskId;

/// True if making `task` depend on `new_dep` would create a cycle. The map
/// gives each task's current direct dependencies.
pub fn creates_cycle(
    deps_of: &HashMap<TaskId, Vec<TaskId>>,
    task: TaskId,
    new_dep: TaskId,
) -> bool {
    if task == new_dep {
        return true;
    }
    // Cycle iff `task` is reachable from `new_dep` by following dependencies.
    let mut seen = HashSet::new();
    let mut stack = vec![new_dep];
    while let Some(t) = stack.pop() {
        if t == task {
            return true;
        }
        if !seen.insert(t) {
            continue;
        }
        if let Some(next) = deps_of.get(&t) {
            stack.extend(next.iter().copied());
        }
    }
    false
}

/// Check a whole proposed dependency list at once and return the first
/// offender, if any.
pub fn first_cycle(
    deps_of: &HashMap<TaskId, Vec<TaskId>>,
    task: TaskId,
    proposed: &[TaskId],
) -> Option<TaskId> {
    // Evaluate against the graph as it will be after the edit, so two new
    // dependencies that only cycle together are still caught.
    let mut graph = deps_of.clone();
    graph.insert(task, Vec::new());
    for dep in proposed {
        if creates_cycle(&graph, task, *dep) {
            return Some(*dep);
        }
        graph.entry(task).or_default().push(*dep);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn g(edges: &[(i64, &[i64])]) -> HashMap<TaskId, Vec<TaskId>> {
        edges
            .iter()
            .map(|(t, ds)| (TaskId(*t), ds.iter().map(|d| TaskId(*d)).collect()))
            .collect()
    }

    #[test]
    fn detects_self_direct_and_indirect_cycles() {
        let graph = g(&[(1, &[2]), (2, &[3]), (3, &[])]);
        assert!(creates_cycle(&graph, TaskId(1), TaskId(1)));
        assert!(creates_cycle(&graph, TaskId(2), TaskId(1)));
        assert!(creates_cycle(&graph, TaskId(3), TaskId(1)));
        assert!(!creates_cycle(&graph, TaskId(1), TaskId(3)));
        assert!(!creates_cycle(&graph, TaskId(4), TaskId(1)));
    }

    #[test]
    fn first_cycle_considers_the_whole_edit() {
        let graph = g(&[(1, &[]), (2, &[]), (3, &[1])]);
        assert_eq!(first_cycle(&graph, TaskId(1), &[TaskId(2)]), None);
        assert_eq!(
            first_cycle(&graph, TaskId(1), &[TaskId(2), TaskId(3)]),
            Some(TaskId(3))
        );
        // Replacing the list drops old edges: 3 currently depends on 1, and
        // proposing 3 -> [2] while 1 gets [3] is only a cycle if both stand.
        assert_eq!(first_cycle(&graph, TaskId(3), &[TaskId(2)]), None);
    }
}
