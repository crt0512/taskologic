//! Task templates. A template is a saved [`TaskDraft`] on a board, used to
//! stamp out recurring kinds of work. What it carries beyond the draft lives
//! in [`TemplateOptions`]: a rule for prefilling the due date, since a kind
//! of task has a lead time rather than a date, and dependencies on *other
//! templates*, which are stamped out alongside so the new task depends on the
//! tasks they produced.

use std::collections::{HashMap, HashSet};

use chrono::{DateTime, TimeDelta, Utc};
use serde::{Deserialize, Serialize};

use crate::ids::{BoardId, TemplateId, Uid};
use crate::task::TaskDraft;

/// The unit a template's due date offset is counted in.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OffsetUnit {
    Minutes,
    #[default]
    Hours,
    Days,
}

impl OffsetUnit {
    pub const ALL: [OffsetUnit; 3] = [OffsetUnit::Minutes, OffsetUnit::Hours, OffsetUnit::Days];

    pub fn label(self) -> &'static str {
        match self {
            OffsetUnit::Minutes => "minutes",
            OffsetUnit::Hours => "hours",
            OffsetUnit::Days => "days",
        }
    }

    pub fn minutes(self) -> i64 {
        match self {
            OffsetUnit::Minutes => 1,
            OffsetUnit::Hours => 60,
            OffsetUnit::Days => 24 * 60,
        }
    }
}

/// "Prefill current date and time", plus however far ahead of now the due
/// date should land. An amount of zero means exactly now.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct DuePrefill {
    pub amount: u32,
    pub unit: OffsetUnit,
}

impl DuePrefill {
    /// The due date this rule produces, or None if the offset overflows.
    pub fn due_from(self, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
        let minutes = i64::from(self.amount).checked_mul(self.unit.minutes())?;
        now.checked_add_signed(TimeDelta::try_minutes(minutes)?)
    }
}

/// What a template carries beyond the draft itself.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct TemplateOptions {
    /// Set when the template prefills the due date of the tasks it stamps out.
    pub due_prefill: Option<DuePrefill>,
    /// Other templates on the same board. Using this template stamps those
    /// out too and makes the new task depend on the tasks they produce.
    pub dep_templates: Vec<TemplateId>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Template {
    pub id: TemplateId,
    pub board_id: BoardId,
    /// Whoever saved it. Managing the template follows the task rules:
    /// this user, the board owner or an admin.
    pub owner_uid: Uid,
    pub name: String,
    pub draft: TaskDraft,
    #[serde(default)]
    pub options: TemplateOptions,
}

/// The largest prefill offset a template may carry, so a typo cannot push a
/// due date out past what chrono can represent.
pub const MAX_PREFILL_AMOUNT: u32 = 100_000;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TemplateError {
    #[error("a template cannot depend on itself")]
    SelfDependency,
    #[error("template {0} is not on this board")]
    NotOnBoard(TemplateId),
    #[error("depending on template {0} would create a cycle")]
    Cycle(TemplateId),
    #[error("the prefill offset is too large")]
    PrefillTooLarge,
}

/// True if making `template` depend on `new_dep` would create a cycle. The
/// map gives each template's current direct dependencies.
pub fn creates_cycle(
    deps_of: &HashMap<TemplateId, Vec<TemplateId>>,
    template: TemplateId,
    new_dep: TemplateId,
) -> bool {
    if template == new_dep {
        return true;
    }
    // Cycle iff `template` is reachable from `new_dep` by following
    // dependencies.
    let mut seen = HashSet::new();
    let mut stack = vec![new_dep];
    while let Some(t) = stack.pop() {
        if t == template {
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

/// The first dependency in `proposed` that would close a cycle, if any.
pub fn first_cycle(
    deps_of: &HashMap<TemplateId, Vec<TemplateId>>,
    template: TemplateId,
    proposed: &[TemplateId],
) -> Option<TemplateId> {
    // Evaluate against the graph as it will be after the edit, so two new
    // dependencies that only cycle together are still caught.
    let mut graph = deps_of.clone();
    graph.insert(template, Vec::new());
    for dep in proposed {
        if creates_cycle(&graph, template, *dep) {
            return Some(*dep);
        }
        graph.entry(template).or_default().push(*dep);
    }
    None
}

/// Every template that has to be stamped out before `root`, dependencies
/// first and each one only once. `root` itself is not in the list. Assumes
/// the graph is a DAG, which [`first_cycle`] guarantees at save time; a cycle
/// that slipped through is broken rather than looped on.
pub fn expansion_order(
    deps_of: &HashMap<TemplateId, Vec<TemplateId>>,
    root: TemplateId,
) -> Vec<TemplateId> {
    let mut out = Vec::new();
    walk(
        deps_of,
        root,
        &mut HashSet::new(),
        &mut HashSet::new(),
        &mut out,
    );
    // The walk emits `root` after its own dependencies; whoever asked for the
    // order stamps that one out themselves.
    out.pop();
    out
}

fn walk(
    deps_of: &HashMap<TemplateId, Vec<TemplateId>>,
    t: TemplateId,
    done: &mut HashSet<TemplateId>,
    on_stack: &mut HashSet<TemplateId>,
    out: &mut Vec<TemplateId>,
) {
    // Already emitted, or reached again while still being walked, which is
    // the broken cycle.
    if done.contains(&t) || !on_stack.insert(t) {
        return;
    }
    for dep in deps_of.get(&t).into_iter().flatten() {
        walk(deps_of, *dep, done, on_stack, out);
    }
    on_stack.remove(&t);
    done.insert(t);
    out.push(t);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn g(edges: &[(i64, &[i64])]) -> HashMap<TemplateId, Vec<TemplateId>> {
        edges
            .iter()
            .map(|(t, ds)| (TemplateId(*t), ds.iter().map(|d| TemplateId(*d)).collect()))
            .collect()
    }

    fn ids(v: &[TemplateId]) -> Vec<i64> {
        v.iter().map(|t| t.0).collect()
    }

    #[test]
    fn a_prefill_counts_forward_in_its_own_unit() {
        let now = DateTime::from_timestamp(1_800_000_000, 0).unwrap();
        let at = |amount, unit| {
            DuePrefill { amount, unit }
                .due_from(now)
                .map(|d| d.timestamp())
        };
        assert_eq!(at(0, OffsetUnit::Hours), Some(now.timestamp()));
        assert_eq!(at(90, OffsetUnit::Minutes), Some(now.timestamp() + 5_400));
        assert_eq!(at(2, OffsetUnit::Hours), Some(now.timestamp() + 7_200));
        assert_eq!(at(3, OffsetUnit::Days), Some(now.timestamp() + 259_200));
        assert_eq!(at(u32::MAX, OffsetUnit::Days), None, "past what chrono has");
    }

    #[test]
    fn detects_self_direct_and_indirect_cycles() {
        let graph = g(&[(1, &[2]), (2, &[3]), (3, &[])]);
        assert!(creates_cycle(&graph, TemplateId(1), TemplateId(1)));
        assert!(creates_cycle(&graph, TemplateId(2), TemplateId(1)));
        assert!(creates_cycle(&graph, TemplateId(3), TemplateId(1)));
        assert!(!creates_cycle(&graph, TemplateId(1), TemplateId(3)));
        assert!(!creates_cycle(&graph, TemplateId(4), TemplateId(1)));
    }

    #[test]
    fn first_cycle_considers_the_whole_edit() {
        let graph = g(&[(1, &[]), (2, &[]), (3, &[1])]);
        assert_eq!(first_cycle(&graph, TemplateId(1), &[TemplateId(2)]), None);
        assert_eq!(
            first_cycle(&graph, TemplateId(1), &[TemplateId(2), TemplateId(3)]),
            Some(TemplateId(3))
        );
    }

    #[test]
    fn a_diamond_stamps_the_shared_dependency_once_and_first() {
        // a -> b, c; b -> d; c -> d.
        let graph = g(&[(1, &[2, 3]), (2, &[4]), (3, &[4]), (4, &[])]);
        let order = ids(&expansion_order(&graph, TemplateId(1)));
        assert_eq!(order, vec![4, 2, 3]);
        assert_eq!(expansion_order(&graph, TemplateId(4)), vec![]);

        // A cycle the save path should never have allowed is broken, not
        // looped on.
        let bad = g(&[(1, &[2]), (2, &[1])]);
        assert_eq!(ids(&expansion_order(&bad, TemplateId(1))), vec![2]);
    }
}
