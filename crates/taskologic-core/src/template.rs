//! Task templates. A template is a saved [`TaskDraft`] on a board, used to
//! stamp out recurring kinds of work. What it carries beyond the draft lives
//! in [`TemplateOptions`]: rules for prefilling the start and due dates,
//! since a kind of task has a lead time rather than a date, and dependencies
//! on *other templates*, which are stamped out alongside so the new task
//! depends on the tasks they produced.

use std::collections::{HashMap, HashSet};

use chrono::{DateTime, TimeDelta, Utc};
use serde::{Deserialize, Serialize};

use crate::ids::{BoardId, TemplateId, Uid};
use crate::offset::Offset;
use crate::task::TaskDraft;

/// Deriving the due date from how long this kind of task actually takes,
/// rather than from a fixed guess. Until enough of them have been finished
/// to average, the template falls back to its fixed offset.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct FromAverage {
    /// How many finished tasks it takes before the average is trusted. Zero
    /// would mean trusting a single sample, which is how one long afternoon
    /// becomes everybody's estimate.
    pub min_samples: u32,
}

impl Default for FromAverage {
    fn default() -> Self {
        Self {
            min_samples: DEFAULT_MIN_SAMPLES,
        }
    }
}

/// The sample count a new "due from average" rule starts with.
pub const DEFAULT_MIN_SAMPLES: u32 = 3;

/// What a template carries beyond the draft itself.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct TemplateOptions {
    /// Set when the template prefills the start date of the tasks it stamps
    /// out, counted forward from the moment it is used. Stamped out as a
    /// dependency, it counts from when the task needing it starts instead.
    pub start_prefill: Option<Offset>,
    /// The same for the due date. It doubles as the fallback when
    /// `due_from_average` is set but there is not enough history yet.
    pub due_prefill: Option<Offset>,
    /// Set to date the task by how long these usually take instead of by the
    /// fixed offset. Needs `due_prefill` as its fallback and a start date to
    /// count from, so a template with neither gains nothing from it.
    pub due_from_average: Option<FromAverage>,
    /// Other templates on the same board. Using this template stamps those
    /// out too and makes the new task depend on the tasks they produce.
    pub dep_templates: Vec<TemplateId>,
}

impl TemplateOptions {
    /// The start date a task stamped out now begins with.
    pub fn start_for(&self, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
        self.start_prefill?.after(now)
    }

    /// The due date to go with it. `history` is how long these tasks have
    /// taken on average and how many finished ones that average is over,
    /// which only the daemon can know.
    ///
    /// The average is added to the start date, because "this takes two hours"
    /// is a statement about the work, not about the clock. With no start date
    /// there is nothing to add it to and the fixed offset stands.
    pub fn due_for(
        &self,
        now: DateTime<Utc>,
        start: Option<DateTime<Utc>>,
        history: Option<(TimeDelta, u32)>,
    ) -> Option<DateTime<Utc>> {
        if let (Some(rule), Some(start), Some((average, samples))) =
            (self.due_from_average, start, history)
            && samples >= rule.min_samples
            && let Some(from_average) = start.checked_add_signed(average)
        {
            return Some(from_average);
        }
        self.due_prefill?.after(now)
    }
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

/// The moment each dependency template counts its prefills from when `root`
/// is stamped out with a task starting at `root_start`. A dependency's "now"
/// is when the task that needs it starts, so pushing the root task out moves
/// everything it stamps out along with it, level by level. A dependency that
/// several tasks need goes with the earliest of them.
pub fn prefill_anchors(
    options: &HashMap<TemplateId, TemplateOptions>,
    root: TemplateId,
    root_start: DateTime<Utc>,
) -> HashMap<TemplateId, DateTime<Utc>> {
    let deps_of: HashMap<TemplateId, Vec<TemplateId>> = options
        .iter()
        .map(|(id, o)| (*id, o.dep_templates.clone()))
        .collect();
    let mut anchors: HashMap<TemplateId, DateTime<Utc>> = HashMap::new();
    let hand_down =|anchors: &mut HashMap<_, DateTime<Utc>>, from: TemplateId, at| {
        for dep in deps_of.get(&from).into_iter().flatten() {
            anchors
                .entry(*dep)
                .and_modify(|a| *a = (*a).min(at))
                .or_insert(at);
        }
    };
    hand_down(&mut anchors, root, root_start);
    // Dependencies come first in the expansion order, so backwards every
    // task that needs a template has handed its start down before it is read.
    for id in expansion_order(&deps_of, root).into_iter().rev() {
        let anchor = *anchors.entry(id).or_insert(root_start);
        let start = options
            .get(&id)
            .and_then(|o| o.start_for(anchor))
            .unwrap_or(anchor);
        hand_down(&mut anchors, id, start);
    }
    anchors
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

    fn now() -> DateTime<Utc> {
        DateTime::from_timestamp(1_800_000_000, 0).unwrap()
    }

    fn hours(n: u32) -> Offset {
        Offset {
            amount: n,
            unit: crate::offset::OffsetUnit::Hours,
        }
    }

    #[test]
    fn the_prefills_date_a_stamped_task_from_the_moment_it_is_used() {
        let opts = TemplateOptions {
            start_prefill: Some(hours(1)),
            due_prefill: Some(hours(3)),
            ..Default::default()
        };
        let start = opts.start_for(now());
        assert_eq!(start, Some(now() + TimeDelta::hours(1)));
        assert_eq!(
            opts.due_for(now(), start, None),
            Some(now() + TimeDelta::hours(3))
        );

        // A template that prefills neither dates nothing.
        let bare = TemplateOptions::default();
        assert_eq!(bare.start_for(now()), None);
        assert_eq!(bare.due_for(now(), None, None), None);
    }

    #[test]
    fn due_from_average_waits_for_enough_finished_tasks() {
        let opts = TemplateOptions {
            start_prefill: Some(hours(1)),
            due_prefill: Some(hours(3)),
            due_from_average: Some(FromAverage { min_samples: 3 }),
            ..Default::default()
        };
        let start = opts.start_for(now()).unwrap();
        let fixed = now() + TimeDelta::hours(3);
        let averaged = start + TimeDelta::minutes(20);

        // Nothing finished yet, and too little to trust: the fixed offset.
        assert_eq!(opts.due_for(now(), Some(start), None), Some(fixed));
        assert_eq!(
            opts.due_for(now(), Some(start), Some((TimeDelta::minutes(20), 2))),
            Some(fixed)
        );
        // Enough samples: the average, counted from the start date.
        assert_eq!(
            opts.due_for(now(), Some(start), Some((TimeDelta::minutes(20), 3))),
            Some(averaged)
        );
        // No start date to add it to, so the fixed offset stands.
        assert_eq!(
            opts.due_for(now(), None, Some((TimeDelta::minutes(20), 9))),
            Some(fixed)
        );
    }

    #[test]
    fn options_written_before_the_start_date_existed_still_load() {
        // 0.1.10 wrote only a due prefill, under the same field name.
        let old = r#"{"due_prefill":{"amount":2,"unit":"hours"},"dep_templates":[7]}"#;
        let opts: TemplateOptions = serde_json::from_str(old).unwrap();
        assert_eq!(opts.due_prefill, Some(hours(2)));
        assert_eq!(opts.start_prefill, None);
        assert_eq!(opts.due_from_average, None);
        assert_eq!(opts.dep_templates, vec![TemplateId(7)]);
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

    #[test]
    fn dependencies_count_from_when_the_task_needing_them_starts() {
        let opts = |start: Option<Offset>, deps: &[i64]| TemplateOptions {
            start_prefill: start,
            dep_templates: deps.iter().map(|d| TemplateId(*d)).collect(),
            ..Default::default()
        };
        // 1 -> 2, 3; 2 -> 4, 5; 3 -> 4. Only 2 and 4 prefill a start.
        let options: HashMap<_, _> = [
            (1, opts(None, &[2, 3])),
            (2, opts(Some(hours(5)), &[4, 5])),
            (3, opts(None, &[4])),
            (4, opts(Some(hours(1)), &[])),
            (5, opts(None, &[])),
        ]
        .into_iter()
        .map(|(id, o)| (TemplateId(id), o))
        .collect();
        let root_start = now() + TimeDelta::days(3);
        let anchors = prefill_anchors(&options, TemplateId(1), root_start);
        let at = |id| anchors[&TemplateId(id)];

        assert_eq!(at(2), root_start);
        assert_eq!(at(3), root_start);
        // Needed by 2, which starts 5h in, and by 3, which has no start of
        // its own and so starts with the root: the earlier one wins.
        assert_eq!(at(4), root_start);
        // Needed only by 2, so it goes with 2's own start.
        assert_eq!(at(5), root_start + TimeDelta::hours(5));
        assert!(!anchors.contains_key(&TemplateId(1)));
    }
}
