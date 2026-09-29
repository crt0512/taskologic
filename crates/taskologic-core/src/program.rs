//! Programs: a chain of tasks written once and started as often as needed.
//!
//! A program is a list of steps. Every step says what its task looks like,
//! when it appears, when it starts, how long it should take, whether the
//! root waits for it and what it prints. No step creates another: a step
//! says what it is waiting for, and ordering, branching and loops all fall
//! out of that. Starting a program makes a root task and a *run*, and from
//! then on the daemon feeds every move on the run's tasks through [`react`],
//! which answers with what to create, date and finish next. The daemon
//! applies the answer inside the same transaction as the move; nothing here
//! touches a database or a clock.

use chrono::{DateTime, NaiveTime, Utc};
use chrono_tz::Tz;
use serde::{Deserialize, Deserializer, Serialize};

use crate::barcode::ScanAction;
use crate::board::Board;
use crate::ids::{BoardId, ColumnId, ProgramId, RunId, ShortId, TaskId, Uid};
use crate::offset::{MAX_OFFSET_AMOUNT, Offset};
use crate::template::DEFAULT_MIN_SAMPLES;
use crate::print::{PrintRule, SlipKind};
use crate::repeat::local_to_utc;
use crate::task::{ChecklistItem, TaskDraft, TaskError, validate_print_rules, validate_title};

/// The key of the step that stands for the whole program. Its task is made
/// when the program starts and finishes itself when the steps that count
/// are done.
pub const ROOT_KEY: &str = "root";

pub const MAX_NAME_CHARS: usize = 120;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Program {
    pub id: ProgramId,
    /// Six characters a barcode can name the program by.
    pub short_id: ShortId,
    pub board_id: BoardId,
    /// Whoever saved it. Managing the program follows the template rules:
    /// this user, the board owner or an admin.
    pub owner_uid: Uid,
    pub name: String,
    pub description: String,
    pub steps: Vec<Step>,
    /// How many finished tasks of a step there have to be before the board
    /// shows an estimate for that step (the root's is the whole run). One
    /// long afternoon must not become everybody's estimate, so it starts at
    /// [`DEFAULT_MIN_SAMPLES`]; a program that runs rarely may want fewer.
    #[serde(default = "default_min_samples")]
    pub min_samples: u32,
}

fn default_min_samples() -> u32 {
    DEFAULT_MIN_SAMPLES
}

/// What a client sends to save a program: everything but the ids.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ProgramDraft {
    pub name: String,
    pub description: String,
    pub steps: Vec<Step>,
    pub min_samples: u32,
}

impl Default for ProgramDraft {
    fn default() -> Self {
        Self {
            name: String::new(),
            description: String::new(),
            steps: Vec::new(),
            min_samples: DEFAULT_MIN_SAMPLES,
        }
    }
}

impl Program {
    pub fn draft(&self) -> ProgramDraft {
        ProgramDraft {
            name: self.name.clone(),
            description: self.description.clone(),
            steps: self.steps.clone(),
            min_samples: self.min_samples,
        }
    }
}

/// One step of a program. Every field has a default, so a saved program
/// keeps loading when a field is added.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Step {
    /// How other steps refer to this one: "1", "1.1", "trash". The root is
    /// the step whose key is [`ROOT_KEY`].
    pub key: String,
    pub title: String,
    pub description: String,
    pub assign: Assign,
    pub checklist: Vec<ChecklistItem>,
    /// Any one of these makes the step's task. Ignored on the root, which
    /// is made when the program starts.
    pub created: Vec<Trigger>,
    /// A trigger does nothing while the run already has a task for this
    /// step. Off, every firing makes another.
    pub once: bool,
    /// Extra steps this one waits for besides the one that created it: their
    /// latest tasks go into the dependency list too.
    pub also_after: Vec<String>,
    pub start: StartRule,
    /// The task moves itself into the started column when its start date
    /// arrives, instead of waiting for a person.
    pub auto_start: bool,
    /// Counted from the moment the task enters the started column, into its
    /// due date. Never invalidates anything; it drives prints and stats.
    pub time_limit: Option<Offset>,
    /// Whether the root waits for this step before finishing itself.
    pub counts_toward_root: bool,
    /// Asked when the step's task finishes. Other steps can wait for an
    /// answer, which is how a program branches.
    pub question: Option<Question>,
    /// Copied onto every task this step makes.
    pub print: Vec<PrintRule>,
    /// One task per entry instead of one task, each with the entry in place
    /// of `{param}` in its title and description. "After this step" then
    /// means after the last of them.
    pub fan_out: Option<Vec<String>>,
}

impl Default for Step {
    fn default() -> Self {
        Self {
            key: String::new(),
            title: String::new(),
            description: String::new(),
            assign: Assign::default(),
            checklist: Vec::new(),
            created: Vec::new(),
            once: false,
            also_after: Vec::new(),
            start: StartRule::default(),
            auto_start: false,
            time_limit: None,
            counts_toward_root: true,
            question: None,
            print: Vec::new(),
            fan_out: None,
        }
    }
}

/// What stands in for the entry of a fan-out step in its texts.
pub const PARAM: &str = "{param}";

impl Step {
    pub fn is_root(&self) -> bool {
        self.key == ROOT_KEY
    }

    /// The entries this step makes a task for: its fan-out list, or one
    /// task with nothing to substitute.
    pub fn params(&self) -> Vec<Option<String>> {
        match &self.fan_out {
            Some(v) if !v.is_empty() => v.iter().map(|p| Some(p.clone())).collect(),
            _ => vec![None],
        }
    }
}

/// A question a step asks as its task finishes. The answer is what other
/// steps wait for; without an answer the run holds and the root waits.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Question {
    pub text: String,
    pub kind: QuestionKind,
    /// Used when the task is finished without an answer: by the task form,
    /// by a plain finish scan. None means the question stays open until
    /// somebody answers it.
    #[serde(default)]
    pub default: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuestionKind {
    /// Two answers, printed as the Y and N codes.
    YesNo,
    /// Two to eight answers of your own, printed as the codes 1 to 8.
    Choice(Vec<String>),
}

pub const YES: &str = "yes";
pub const NO: &str = "no";

impl Question {
    /// The answers, in the order their codes print.
    pub fn answers(&self) -> Vec<String> {
        match &self.kind {
            QuestionKind::YesNo => vec![YES.to_string(), NO.to_string()],
            QuestionKind::Choice(v) => v.clone(),
        }
    }

    /// The answer as it is stored and matched, if `text` names one. Case
    /// and surrounding blanks do not count.
    pub fn canonical(&self, text: &str) -> Option<String> {
        let wanted = text.trim().to_lowercase();
        self.answers()
            .into_iter()
            .find(|a| a.trim().to_lowercase() == wanted)
    }

    /// The scan action that stands for the `i`th answer on a slip.
    pub fn barcode_action(&self, i: usize) -> ScanAction {
        match self.kind {
            QuestionKind::YesNo if i == 0 => ScanAction::Yes,
            QuestionKind::YesNo => ScanAction::No,
            QuestionKind::Choice(_) => ScanAction::Choice((i + 1).min(usize::from(ScanAction::MAX_CHOICES)) as u8),
        }
    }

    /// What a scanned answer code means for this question, if anything.
    pub fn answer_for(&self, action: ScanAction) -> Option<String> {
        match (&self.kind, action) {
            (QuestionKind::YesNo, ScanAction::Yes) => Some(YES.to_string()),
            (QuestionKind::YesNo, ScanAction::No) => Some(NO.to_string()),
            (QuestionKind::Choice(v), ScanAction::Choice(n)) => {
                v.get(usize::from(n).checked_sub(1)?).cloned()
            }
            _ => None,
        }
    }

    pub fn validate(&self) -> Result<(), ProgramError> {
        if self.text.trim().is_empty() {
            return Err(ProgramError::Question("the question needs a text".into()));
        }
        if let QuestionKind::Choice(v) = &self.kind {
            if v.len() < 2 || v.len() > usize::from(ScanAction::MAX_CHOICES) {
                return Err(ProgramError::Question(
                    "a choice question needs between two and eight answers".into(),
                ));
            }
            let mut seen: Vec<String> = Vec::new();
            for a in v {
                let a = a.trim().to_lowercase();
                if a.is_empty() {
                    return Err(ProgramError::Question("an answer cannot be empty".into()));
                }
                if seen.contains(&a) {
                    return Err(ProgramError::Question(format!("the answer {a:?} is there twice")));
                }
                seen.push(a);
            }
        }
        if let Some(d) = &self.default
            && self.canonical(d).is_none()
        {
            return Err(ProgramError::Question(format!(
                "the default {d:?} is not one of the answers"
            )));
        }
        Ok(())
    }
}

/// Who a step's task is assigned to.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Assign {
    /// Whoever started the program.
    #[default]
    Starter,
    Users(Vec<Uid>),
    Nobody,
}

/// What makes a step's task appear.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Trigger {
    /// Together with the root, when the program starts.
    WithRoot,
    /// When the root task enters the started column.
    RootStarted,
    /// When a task of that step enters the finished column.
    Finished { step: String },
    /// When a task of that step is answered so. Case does not count.
    Answered { step: String, answer: String },
}

/// What a step's task is dated when it is made.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StartRule {
    /// No start date, a human moves it.
    #[default]
    Manual,
    /// Dated the moment the root enters the started column. A step made
    /// after that starts at once.
    WhenRootStarts,
    /// So long after the event that made it. Zero means immediately.
    AfterTrigger(Offset),
    /// So many days after the event that made it, at this time of day in
    /// the starter's zone: one day is tomorrow morning, zero is that same
    /// day at that time, even when the time has passed.
    DaysLaterAt { days: u32, at: NaiveTime },
}

impl<'de> Deserialize<'de> for StartRule {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        // Programs saved while the rule was fixed at one day wrote
        // `next_day_at`; they keep loading as a day of one.
        #[derive(Deserialize)]
        #[serde(rename_all = "snake_case")]
        enum Wire {
            Manual,
            WhenRootStarts,
            AfterTrigger(Offset),
            NextDayAt(NaiveTime),
            DaysLaterAt { days: u32, at: NaiveTime },
        }
        Ok(match Wire::deserialize(d)? {
            Wire::Manual => StartRule::Manual,
            Wire::WhenRootStarts => StartRule::WhenRootStarts,
            Wire::AfterTrigger(o) => StartRule::AfterTrigger(o),
            Wire::NextDayAt(at) => StartRule::DaysLaterAt { days: 1, at },
            Wire::DaysLaterAt { days, at } => StartRule::DaysLaterAt { days, at },
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ProgramError {
    #[error("program name cannot be empty")]
    EmptyName,
    #[error("program name is too long, {MAX_NAME_CHARS} characters at most")]
    NameTooLong,
    #[error("a program needs a step with the key \"{ROOT_KEY}\"")]
    NoRoot,
    #[error("an estimate needs at least one finished task to come from")]
    NoSamples,
    #[error("every step needs a key")]
    EmptyKey,
    #[error("the key {0:?} is used twice")]
    DuplicateKey(String),
    #[error("step {0}: {1}")]
    Title(String, TaskError),
    #[error("step {step} refers to a step {refers:?} that does not exist")]
    UnknownStep { step: String, refers: String },
    #[error("step {0} waits for the root, but the root finishing ends the run")]
    WaitsForRoot(String),
    #[error("step {0} has nothing that makes it appear")]
    NeverAppears(String),
    #[error("step {0} has an offset that would land off the calendar")]
    OffsetTooLarge(String),
    #[error("step {0}: uid {1} is not a member of this board and cannot be assigned")]
    AssigneeNotMember(String, Uid),
    #[error("step {0}: {1}")]
    Print(String, TaskError),
    #[error("{0}")]
    Question(String),
    #[error("the root cannot ask a question, its finishing ends the run")]
    RootQuestion,
    #[error("step {step} waits for {other} to be answered {answer:?}, which is not one of its answers")]
    NoSuchAnswer {
        step: String,
        other: String,
        answer: String,
    },
    #[error("step {0} fans out over nothing: give it entries or turn the fan-out off")]
    EmptyFanOut(String),
}

/// Everything that can be checked with the board in hand. Whether a named
/// uid is a Taskologic user at all is the daemon's to ask.
pub fn validate(draft: &ProgramDraft, board: &Board) -> Result<(), ProgramError> {
    let name = draft.name.trim();
    if name.is_empty() {
        return Err(ProgramError::EmptyName);
    }
    if name.chars().count() > MAX_NAME_CHARS {
        return Err(ProgramError::NameTooLong);
    }
    if !draft.steps.iter().any(Step::is_root) {
        return Err(ProgramError::NoRoot);
    }
    if draft.min_samples == 0 {
        return Err(ProgramError::NoSamples);
    }
    let mut keys: Vec<&str> = Vec::new();
    for step in &draft.steps {
        let key = step.key.trim();
        if key.is_empty() {
            return Err(ProgramError::EmptyKey);
        }
        if keys.contains(&key) {
            return Err(ProgramError::DuplicateKey(key.to_string()));
        }
        keys.push(key);
    }
    let known = |k: &str| draft.steps.iter().any(|s| s.key.trim() == k);
    for step in &draft.steps {
        let key = step.key.trim().to_string();
        validate_title(&step.title).map_err(|e| ProgramError::Title(key.clone(), e))?;
        let refers: Vec<&String> = step
            .created
            .iter()
            .filter_map(|t| match t {
                Trigger::Finished { step } | Trigger::Answered { step, .. } => Some(step),
                _ => None,
            })
            .chain(step.also_after.iter())
            .collect();
        for r in refers {
            if r == ROOT_KEY {
                return Err(ProgramError::WaitsForRoot(key));
            }
            if !known(r) {
                return Err(ProgramError::UnknownStep {
                    step: key,
                    refers: r.clone(),
                });
            }
        }
        // Waiting for an answer the other step cannot give would wait forever.
        for t in &step.created {
            if let Trigger::Answered { step: other, answer } = t {
                let asked = draft
                    .steps
                    .iter()
                    .find(|s| s.key.trim() == other)
                    .and_then(|s| s.question.as_ref());
                if asked.is_none_or(|q| q.canonical(answer).is_none()) {
                    return Err(ProgramError::NoSuchAnswer {
                        step: key,
                        other: other.clone(),
                        answer: answer.clone(),
                    });
                }
            }
        }
        if let Some(q) = &step.question {
            if step.is_root() {
                return Err(ProgramError::RootQuestion);
            }
            q.validate()?;
        }
        if let Some(entries) = &step.fan_out
            && entries.iter().all(|e| e.trim().is_empty())
        {
            return Err(ProgramError::EmptyFanOut(key));
        }
        if !step.is_root() && step.created.is_empty() {
            return Err(ProgramError::NeverAppears(key));
        }
        let too_large = |o: Offset| o.amount > MAX_OFFSET_AMOUNT;
        if step.time_limit.is_some_and(too_large)
            || matches!(step.start, StartRule::AfterTrigger(o) if too_large(o))
            || matches!(step.start, StartRule::DaysLaterAt { days, .. } if days > MAX_OFFSET_AMOUNT)
        {
            return Err(ProgramError::OffsetTooLarge(key));
        }
        if let Assign::Users(uids) = &step.assign
            && let Some(u) = uids.iter().find(|u| !board.is_member(**u))
        {
            return Err(ProgramError::AssigneeNotMember(key, *u));
        }
        validate_print_rules(&step.print, board, step.fan_out.is_some())
            .map_err(|e| ProgramError::Print(key, e))?;
    }
    Ok(())
}

/// One started copy of a program. It carries its own copy of the steps, so
/// editing or deleting the program afterwards changes nothing here.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Run {
    pub id: RunId,
    /// None once the program it came from was deleted.
    pub program_id: Option<ProgramId>,
    pub board_id: BoardId,
    pub root_task: TaskId,
    pub started_by: Uid,
    /// Where the root was made and where every step lands.
    pub column_id: ColumnId,
    pub steps: Vec<Step>,
    pub created_at: DateTime<Utc>,
    /// When the root entered the started column.
    pub started_at: Option<DateTime<Utc>>,
    /// When the root finished, by itself or by hand.
    pub finished_at: Option<DateTime<Utc>>,
    pub cancelled_at: Option<DateTime<Utc>>,
}

impl Run {
    /// Still reacting to what happens to its tasks.
    pub fn is_active(&self) -> bool {
        self.finished_at.is_none() && self.cancelled_at.is_none()
    }
}

/// How a task belongs to a run, carried on the task so a board can say
/// which step a card is, ask its question and mark one still waiting.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StepRef {
    pub run: RunId,
    pub step: String,
    pub iteration: u32,
    /// The step's question, as the run has it.
    #[serde(default)]
    pub question: Option<Question>,
    /// Finished, asked, and nobody has answered yet.
    #[serde(default)]
    pub pending_question: bool,
}

impl StepRef {
    pub fn is_root(&self) -> bool {
        self.step == ROOT_KEY
    }

    /// The short mark a card carries: the key, with the go at it when the
    /// step has run more than once.
    pub fn label(&self) -> String {
        if self.iteration > 1 {
            format!("{}#{}", self.step, self.iteration)
        } else {
            self.step.clone()
        }
    }
}

/// One task a run has made, as much of it as the engine needs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StepTask {
    pub step: String,
    pub iteration: u32,
    /// The fan-out entry this task is for, when the step fans out.
    pub param: Option<String>,
    pub task_id: TaskId,
    /// In the finished column or archived.
    pub done: bool,
    /// Deleted by hand. The step counts as skipped: its triggers never
    /// fire and the root does not wait for it.
    pub skipped: bool,
    pub counts: bool,
    /// Finished with a question nobody has answered. The root waits.
    pub pending_answer: bool,
}

/// What just happened to a run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RunEvent {
    /// The root task was made; the program has started.
    Created,
    /// The root task entered the started column.
    RootStarted,
    /// One of the run's tasks entered the finished column, with the answer
    /// to its question when whoever finished it gave one.
    Finished {
        task: TaskId,
        answer: Option<String>,
    },
    /// A question on a finished task was answered afterwards.
    Answered { task: TaskId, answer: String },
    /// One of the run's tasks was deleted.
    Deleted { task: TaskId },
}

/// A task the engine wants made.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Creation {
    /// Index into the run's steps.
    pub step: usize,
    pub iteration: u32,
    /// The fan-out entry this task is for. Every task of one go at a
    /// fan-out step shares the iteration and differs here.
    pub param: Option<String>,
    pub start_at: Option<DateTime<Utc>>,
    /// The task that triggered it, when one did, and the latest task of
    /// every step it also waits for.
    pub depends_on: Vec<TaskId>,
    pub counts: bool,
}

/// Everything the daemon has to do after an event.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Plan {
    pub create: Vec<Creation>,
    /// Tasks waiting for the root to start, to be dated now. The daemon
    /// dates only those that have no start date yet.
    pub set_start: Vec<TaskId>,
    /// Every step that counts is done: the root finishes itself.
    pub finish_root: bool,
    /// The root itself was finished or deleted by hand. The run is over and
    /// nothing in it reacts to anything again.
    pub end_run: bool,
    /// This task finished with a question and no answer: mark it asked and
    /// wait for one.
    pub ask: Option<TaskId>,
    /// This task's question was answered so, whether by whoever finished it,
    /// by its default or by a later answer: record it.
    pub answered: Option<(TaskId, String)>,
}

/// Decide what an event means for a run. `tasks` is everything the run has
/// made so far, `started` whether the root has entered the started column,
/// `tz` the starter's zone for "days later at" dates.
pub fn react(
    steps: &[Step],
    tasks: &[StepTask],
    started: bool,
    event: &RunEvent,
    now: DateTime<Utc>,
    tz: Tz,
) -> Plan {
    let mut plan = Plan::default();
    let step_of = |id: TaskId| tasks.iter().find(|t| t.task_id == id);
    let def_of = |key: &str| steps.iter().find(|s| s.key == key);

    // What the event fires, and which task the new ones wait for. A finish
    // with an answer, or with a question that has a default, fires the
    // answer's triggers in the same breath; without either the question
    // stays open and the run waits.
    let mut fired: Vec<Trigger> = Vec::new();
    let trigger_task: Option<TaskId> = match event {
        RunEvent::Created => {
            fired.push(Trigger::WithRoot);
            None
        }
        RunEvent::RootStarted => {
            fired.push(Trigger::RootStarted);
            None
        }
        RunEvent::Finished { task, answer } => match step_of(*task) {
            Some(t) if t.step == ROOT_KEY => {
                plan.end_run = true;
                return plan;
            }
            Some(t) => {
                // A step that fans out is finished when the last of its
                // tasks is; before that the others are still going.
                let group_open = tasks.iter().any(|o| {
                    o.step == t.step
                        && o.iteration == t.iteration
                        && o.task_id != t.task_id
                        && !o.done
                        && !o.skipped
                });
                if !group_open {
                    fired.push(Trigger::Finished {
                        step: t.step.clone(),
                    });
                }
                if let Some(q) = def_of(&t.step).and_then(|s| s.question.as_ref()) {
                    let given = answer
                        .as_deref()
                        .or(q.default.as_deref())
                        .and_then(|a| q.canonical(a));
                    match given {
                        Some(a) => {
                            fired.push(Trigger::Answered {
                                step: t.step.clone(),
                                answer: a.clone(),
                            });
                            plan.answered = Some((*task, a));
                        }
                        None => plan.ask = Some(*task),
                    }
                }
                Some(*task)
            }
            None => None,
        },
        RunEvent::Answered { task, answer } => match step_of(*task) {
            Some(t) => {
                let canonical = def_of(&t.step)
                    .and_then(|s| s.question.as_ref())
                    .and_then(|q| q.canonical(answer));
                match canonical {
                    Some(a) => {
                        fired.push(Trigger::Answered {
                            step: t.step.clone(),
                            answer: a.clone(),
                        });
                        plan.answered = Some((*task, a));
                        Some(*task)
                    }
                    // Not an answer the question takes: nothing happens.
                    None => None,
                }
            }
            None => None,
        },
        RunEvent::Deleted { task } => match step_of(*task) {
            Some(t) if t.step == ROOT_KEY => {
                plan.end_run = true;
                return plan;
            }
            _ => None,
        },
    };
    let started = started || *event == RunEvent::RootStarted;
    let matches = |t: &Trigger| match t {
        Trigger::Answered { step, answer } => fired.iter().any(|f| {
            matches!(f, Trigger::Answered { step: s, answer: a } if s == step && a.eq_ignore_ascii_case(answer))
        }),
        other => fired.contains(other),
    };

    if !fired.is_empty() {
        for (index, step) in steps.iter().enumerate() {
            // One task per step per event, however many of its triggers fired.
            if step.is_root() || !step.created.iter().any(matches) {
                continue;
            }
            let existing = tasks.iter().filter(|t| t.step == step.key);
            if step.once && existing.clone().any(|t| !t.skipped) {
                continue;
            }
            let iteration = existing.map(|t| t.iteration).max().unwrap_or(0) + 1;
            let mut depends_on: Vec<TaskId> = trigger_task.into_iter().collect();
            for key in &step.also_after {
                if let Some(latest) = tasks
                    .iter()
                    .filter(|t| &t.step == key && !t.skipped)
                    .max_by_key(|t| t.iteration)
                {
                    depends_on.push(latest.task_id);
                }
            }
            depends_on.dedup();
            let start_at = match step.start {
                StartRule::Manual => None,
                StartRule::WhenRootStarts => started.then_some(now),
                StartRule::AfterTrigger(offset) => offset.after(now),
                StartRule::DaysLaterAt { days, at } => days_later_at(now, days, at, tz),
            };
            for param in step.params() {
                plan.create.push(Creation {
                    step: index,
                    iteration,
                    param,
                    start_at,
                    depends_on: depends_on.clone(),
                    counts: step.counts_toward_root,
                });
            }
        }
    }

    if *event == RunEvent::RootStarted {
        plan.set_start = tasks
            .iter()
            .filter(|t| !t.done && !t.skipped && t.step != ROOT_KEY)
            .filter(|t| {
                steps
                    .iter()
                    .any(|s| s.key == t.step && s.start == StartRule::WhenRootStarts)
            })
            .map(|t| t.task_id)
            .collect();
    }

    // The root finishes itself once it has started and nothing that counts
    // is still open, this event's outcome included. Before the root starts
    // the steps have not begun, so a program whose steps only appear later
    // must not close the moment it is made.
    if started {
        let open = tasks.iter().any(|t| {
            let done = t.done || matches!(event, RunEvent::Finished { task, .. } if *task == t.task_id);
            let skipped =
                t.skipped || matches!(event, RunEvent::Deleted { task } if *task == t.task_id);
            // A finished task whose question nobody answered is still owed
            // something, and so is the one this event just asked.
            let answered_now = plan.answered.as_ref().is_some_and(|(id, _)| *id == t.task_id);
            let waiting = (t.pending_answer && !answered_now) || plan.ask == Some(t.task_id);
            t.step != ROOT_KEY && t.counts && !skipped && (!done || waiting)
        });
        let root_done = tasks.iter().any(|t| t.step == ROOT_KEY && t.done);
        plan.finish_root = !open && !root_done && plan.create.iter().all(|c| !c.counts);
    }
    plan
}

/// `days` days on from the moment `now`, at `time` in `tz`: one is tomorrow.
fn days_later_at(now: DateTime<Utc>, days: u32, time: NaiveTime, tz: Tz) -> Option<DateTime<Utc>> {
    let date = now
        .with_timezone(&tz)
        .date_naive()
        .checked_add_days(chrono::Days::new(u64::from(days)))?;
    Some(local_to_utc(tz, date.and_time(time)))
}

/// The task a step makes, as the daemon creates it. `starter` is whoever
/// started the run; assignees who are not on the board are left out.
/// `param` is the fan-out entry, put in place of `{param}` in the texts.
pub fn draft_for(
    step: &Step,
    starter: Uid,
    board: &Board,
    start_at: Option<DateTime<Utc>>,
    depends_on: Vec<TaskId>,
    param: Option<&str>,
) -> TaskDraft {
    let fill = |text: &str| match param {
        Some(p) if text.contains(PARAM) => text.replace(PARAM, p),
        // A fan-out step whose title never says where it goes gets the
        // entry appended, or every task of the group would read the same.
        Some(p) if text == step.title.trim() => format!("{text} {p}"),
        _ => text.to_string(),
    };
    let assignees = match &step.assign {
        Assign::Starter => vec![starter],
        Assign::Users(uids) => uids
            .iter()
            .copied()
            .filter(|u| board.is_member(*u))
            .collect(),
        Assign::Nobody => Vec::new(),
    };
    TaskDraft {
        title: fill(step.title.trim()),
        description: fill(&step.description),
        start_at,
        due_at: None,
        reminder_start_minutes: None,
        reminder_due_minutes: None,
        assignees,
        depends_on,
        checklist: step
            .checklist
            .iter()
            .map(|c| ChecklistItem {
                text: c.text.clone(),
                done: false,
            })
            .collect(),
        repeat: None,
        // The group sheet is the run's to print, once for the whole group;
        // a single task cannot act on that rule and does not carry it.
        print_rules: step
            .print
            .iter()
            .filter(|r| r.slip != SlipKind::Sheet)
            .cloned()
            .collect(),
        auto_start: step.auto_start,
    }
}

/// One readable line for a step's start rule.
pub fn start_rule_summary(rule: StartRule) -> String {
    match rule {
        StartRule::Manual => "when somebody starts it".into(),
        StartRule::WhenRootStarts => "when the program starts".into(),
        StartRule::AfterTrigger(o) if o.amount == 0 => "immediately".into(),
        StartRule::AfterTrigger(o) => format!("{} {} later", o.amount, o.unit.label()),
        StartRule::DaysLaterAt { days: 0, at } => format!("that day at {}", at.format("%H:%M")),
        StartRule::DaysLaterAt { days: 1, at } => format!("next day at {}", at.format("%H:%M")),
        StartRule::DaysLaterAt { days, at } => format!("{days} days later at {}", at.format("%H:%M")),
    }
}

/// One readable line for what makes a step appear.
pub fn triggers_summary(triggers: &[Trigger]) -> String {
    if triggers.is_empty() {
        return "never".into();
    }
    triggers
        .iter()
        .map(|t| match t {
            Trigger::WithRoot => "with the program".to_string(),
            Trigger::RootStarted => "when the program starts".to_string(),
            Trigger::Finished { step } => format!("after {step}"),
            Trigger::Answered { step, answer } => format!("when {step} is answered {answer}"),
        })
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::test_support::board_with_members;
    use crate::offset::OffsetUnit;

    fn minutes(n: u32) -> Offset {
        Offset {
            amount: n,
            unit: OffsetUnit::Minutes,
        }
    }

    fn step(key: &str, created: Vec<Trigger>) -> Step {
        Step {
            key: key.into(),
            title: format!("Step {key}"),
            created,
            ..Default::default()
        }
    }

    fn after(key: &str) -> Trigger {
        Trigger::Finished { step: key.into() }
    }

    /// The apartment example without the wash question and the room
    /// fan-out: root, gather (1), trash (2), two rooms (3a, 3b) and the
    /// laundry the next day (4), which the root does not wait for.
    fn apartment() -> Vec<Step> {
        vec![
            Step {
                key: ROOT_KEY.into(),
                title: "Clean up".into(),
                ..Default::default()
            },
            Step {
                start: StartRule::WhenRootStarts,
                time_limit: Some(minutes(20)),
                ..step("1", vec![Trigger::WithRoot])
            },
            Step {
                once: true,
                start: StartRule::AfterTrigger(minutes(0)),
                time_limit: Some(minutes(30)),
                ..step("2", vec![after("1")])
            },
            step("3a", vec![after("2")]),
            step("3b", vec![after("2")]),
            Step {
                counts_toward_root: false,
                start: StartRule::DaysLaterAt {
                    days: 1,
                    at: NaiveTime::from_hms_opt(9, 0, 0).unwrap(),
                },
                ..step("4", vec![after("1")])
            },
        ]
    }

    fn now() -> DateTime<Utc> {
        DateTime::from_timestamp(1_800_000_000, 0).unwrap()
    }

    fn made(step: &str, iteration: u32, id: i64, counts: bool) -> StepTask {
        StepTask {
            step: step.into(),
            iteration,
            param: None,
            task_id: TaskId(id),
            done: false,
            skipped: false,
            counts,
            pending_answer: false,
        }
    }

    fn last_load() -> Question {
        Question {
            text: "Is this the last load?".into(),
            kind: QuestionKind::YesNo,
            default: None,
        }
    }

    /// The wash loop of the example: 1 asks, "no" makes another cycle
    /// (1.1, which asks again), "yes" makes the final hang up (1.2).
    fn laundry() -> Vec<Step> {
        let asks = |key: &str, created: Vec<Trigger>| Step {
            question: Some(last_load()),
            ..step(key, created)
        };
        let answered = |k: &str, a: &str| Trigger::Answered {
            step: k.into(),
            answer: a.into(),
        };
        vec![
            Step {
                key: ROOT_KEY.into(),
                title: "Laundry".into(),
                ..Default::default()
            },
            asks("1", vec![Trigger::WithRoot]),
            asks("1.1", vec![answered("1", "no"), answered("1.1", "no")]),
            step("1.2", vec![answered("1", "yes"), answered("1.1", "yes")]),
            Step {
                once: true,
                ..step("2", vec![after("1")])
            },
        ]
    }

    #[test]
    fn a_program_is_checked_step_by_step() {
        let board = board_with_members(1, &[1, 2]);
        let mut d = ProgramDraft {
            name: "Clean up".into(),
            steps: apartment(),
            ..Default::default()
        };
        assert_eq!(validate(&d, &board), Ok(()));
        d.name = " ".into();
        assert_eq!(validate(&d, &board), Err(ProgramError::EmptyName));
        d.name = "Clean up".into();
        d.steps.remove(0);
        assert_eq!(validate(&d, &board), Err(ProgramError::NoRoot));
        d.steps = apartment();
        d.steps[2].created = vec![after("9")];
        assert_eq!(
            validate(&d, &board),
            Err(ProgramError::UnknownStep {
                step: "2".into(),
                refers: "9".into()
            })
        );
        d.steps[2].created = vec![];
        assert_eq!(validate(&d, &board), Err(ProgramError::NeverAppears("2".into())));
        d.steps[2].created = vec![after(ROOT_KEY)];
        assert_eq!(validate(&d, &board), Err(ProgramError::WaitsForRoot("2".into())));
        d.steps = apartment();
        d.steps[3].key = "3b".into();
        assert_eq!(validate(&d, &board), Err(ProgramError::DuplicateKey("3b".into())));
        d.steps = apartment();
        d.steps[1].assign = Assign::Users(vec![2, 9]);
        assert_eq!(
            validate(&d, &board),
            Err(ProgramError::AssigneeNotMember("1".into(), 9))
        );
        d.steps = apartment();
        d.steps[1].title = " ".into();
        assert!(matches!(validate(&d, &board), Err(ProgramError::Title(k, _)) if k == "1"));
    }

    #[test]
    fn starting_makes_the_steps_that_come_with_the_root_and_the_root_waits() {
        let steps = apartment();
        let plan = react(&steps, &[], false, &RunEvent::Created, now(), chrono_tz::UTC);
        assert_eq!(plan.create.len(), 1, "only step 1 comes with the root");
        let c = &plan.create[0];
        assert_eq!((c.step, c.iteration, c.counts), (1, 1, true));
        assert_eq!(c.start_at, None, "it waits for the root to start");
        assert!(c.depends_on.is_empty());
        assert!(!plan.finish_root, "the root has not even started");

        // The root starting dates step 1 and creates nothing else here.
        let tasks = vec![made(ROOT_KEY, 1, 10, false), made("1", 1, 11, true)];
        let plan = react(&steps, &tasks, false, &RunEvent::RootStarted, now(), chrono_tz::UTC);
        assert!(plan.create.is_empty());
        assert_eq!(plan.set_start, vec![TaskId(11)]);
        assert!(!plan.finish_root, "step 1 is still open");
    }

    #[test]
    fn finishing_a_step_makes_what_waited_for_it_with_the_right_dates_and_links() {
        let steps = apartment();
        let tasks = vec![made(ROOT_KEY, 1, 10, false), made("1", 1, 11, true)];
        let berlin = chrono_tz::Europe::Berlin;
        let plan = react(
            &steps,
            &tasks,
            true,
            &RunEvent::Finished {
                task: TaskId(11),
                answer: None,
            },
            now(),
            berlin,
        );
        assert_eq!(plan.create.len(), 2, "trash and the laundry");
        let trash = &plan.create[0];
        assert_eq!(trash.step, 2);
        assert_eq!(trash.depends_on, vec![TaskId(11)], "waits for what made it");
        assert_eq!(trash.start_at, Some(now()), "zero after the trigger is now");
        assert!(trash.counts);
        let laundry = &plan.create[1];
        assert_eq!(laundry.step, 5);
        assert!(!laundry.counts);
        let start = laundry.start_at.expect("dated");
        let local = start.with_timezone(&berlin);
        assert_eq!(local.format("%H:%M").to_string(), "09:00");
        assert_eq!(
            local.date_naive(),
            now().with_timezone(&berlin).date_naive().succ_opt().unwrap()
        );
        assert!(!plan.finish_root, "trash still has to happen");
    }

    #[test]
    fn once_stops_a_second_copy_but_not_a_step_that_repeats() {
        let steps = apartment();
        // Step 1 finished a second time, somehow: trash exists and is once.
        let tasks = vec![
            made(ROOT_KEY, 1, 10, false),
            made("1", 1, 11, true),
            made("2", 1, 12, true),
            made("4", 1, 14, false),
        ];
        let plan = react(
            &steps,
            &tasks,
            true,
            &RunEvent::Finished {
                task: TaskId(11),
                answer: None,
            },
            now(),
            chrono_tz::UTC,
        );
        let made_steps: Vec<(usize, u32)> = plan.create.iter().map(|c| (c.step, c.iteration)).collect();
        assert_eq!(made_steps, vec![(5, 2)], "no second trash run, a second laundry");
    }

    #[test]
    fn the_root_finishes_when_everything_that_counts_is_done_or_skipped() {
        let steps = apartment();
        let mut tasks = vec![
            made(ROOT_KEY, 1, 10, false),
            made("1", 1, 11, true),
            made("2", 1, 12, true),
            made("3a", 1, 13, true),
            made("3b", 1, 14, true),
            made("4", 1, 15, false),
        ];
        for t in &mut tasks[1..4] {
            t.done = true;
        }
        // The last room finishing closes the run, laundry or no laundry.
        let plan = react(
            &steps,
            &tasks,
            true,
            &RunEvent::Finished {
                task: TaskId(14),
                answer: None,
            },
            now(),
            chrono_tz::UTC,
        );
        assert!(plan.create.is_empty());
        assert!(plan.finish_root);
        assert!(!plan.end_run);
        // Deleting it instead counts as skipping it, with the same result.
        let plan = react(
            &steps,
            &tasks,
            true,
            &RunEvent::Deleted { task: TaskId(14) },
            now(),
            chrono_tz::UTC,
        );
        assert!(plan.finish_root);
        // A room still open holds the root back.
        tasks[3].done = false;
        let plan = react(
            &steps,
            &tasks,
            true,
            &RunEvent::Finished {
                task: TaskId(14),
                answer: None,
            },
            now(),
            chrono_tz::UTC,
        );
        assert!(!plan.finish_root);
    }

    #[test]
    fn a_step_can_also_wait_for_another_that_did_not_make_it() {
        let mut steps = apartment();
        // The laundry waits for the first room too.
        steps[5].also_after = vec!["3a".into()];
        let tasks = vec![
            made(ROOT_KEY, 1, 10, false),
            made("1", 1, 11, true),
            made("3a", 1, 13, true),
            made("3a", 2, 16, true),
        ];
        let plan = react(
            &steps,
            &tasks,
            true,
            &RunEvent::Finished {
                task: TaskId(11),
                answer: None,
            },
            now(),
            chrono_tz::UTC,
        );
        let laundry = plan.create.iter().find(|c| c.step == 5).unwrap();
        assert_eq!(
            laundry.depends_on,
            vec![TaskId(11), TaskId(16)],
            "the trigger and the latest room"
        );
    }

    #[test]
    fn the_root_finishing_or_going_by_hand_ends_the_run() {
        let steps = apartment();
        let tasks = vec![made(ROOT_KEY, 1, 10, false), made("1", 1, 11, true)];
        let plan = react(
            &steps,
            &tasks,
            true,
            &RunEvent::Finished {
                task: TaskId(10),
                answer: None,
            },
            now(),
            chrono_tz::UTC,
        );
        assert_eq!(
            plan,
            Plan {
                end_run: true,
                ..Default::default()
            }
        );
        let plan = react(
            &steps,
            &tasks,
            true,
            &RunEvent::Deleted { task: TaskId(10) },
            now(),
            chrono_tz::UTC,
        );
        assert!(plan.end_run);
        // A task the run does not know is not its business.
        let plan = react(
            &steps,
            &tasks,
            true,
            &RunEvent::Finished {
                task: TaskId(99),
                answer: None,
            },
            now(),
            chrono_tz::UTC,
        );
        assert!(plan.create.is_empty() && !plan.end_run);
    }

    #[test]
    fn a_finish_with_an_answer_fires_both_the_finish_and_the_answer() {
        let steps = laundry();
        let tasks = vec![made(ROOT_KEY, 1, 10, false), made("1", 1, 11, true)];
        let plan = react(
            &steps,
            &tasks,
            true,
            &RunEvent::Finished {
                task: TaskId(11),
                answer: Some("NO".into()),
            },
            now(),
            chrono_tz::UTC,
        );
        let made_steps: Vec<usize> = plan.create.iter().map(|c| c.step).collect();
        assert_eq!(made_steps, vec![2, 4], "another cycle, and the trash run");
        assert_eq!(plan.answered, Some((TaskId(11), "no".into())), "stored lower case");
        assert_eq!(plan.ask, None);
        assert!(!plan.finish_root);

        // The loop: 1.1 answered "no" makes 1.1 again, a second go.
        let tasks = vec![
            made(ROOT_KEY, 1, 10, false),
            made("1", 1, 11, true),
            made("1.1", 1, 12, true),
            made("2", 1, 13, true),
        ];
        let plan = react(
            &steps,
            &tasks,
            true,
            &RunEvent::Finished {
                task: TaskId(12),
                answer: Some("no".into()),
            },
            now(),
            chrono_tz::UTC,
        );
        assert_eq!(plan.create.len(), 1);
        assert_eq!((plan.create[0].step, plan.create[0].iteration), (2, 2));
        assert_eq!(plan.create[0].depends_on, vec![TaskId(12)]);
    }

    #[test]
    fn a_finish_without_an_answer_asks_and_the_root_waits_until_it_comes() {
        let steps = laundry();
        let tasks = vec![made(ROOT_KEY, 1, 10, false), made("1", 1, 11, true)];
        let plan = react(
            &steps,
            &tasks,
            true,
            &RunEvent::Finished {
                task: TaskId(11),
                answer: None,
            },
            now(),
            chrono_tz::UTC,
        );
        assert_eq!(plan.ask, Some(TaskId(11)));
        assert_eq!(plan.answered, None);
        let made_steps: Vec<usize> = plan.create.iter().map(|c| c.step).collect();
        assert_eq!(made_steps, vec![4], "the trash run does not wait for the answer");
        assert!(!plan.finish_root, "an unanswered question keeps the root open");

        // Later the answer comes: the branch it names appears and the
        // question stops holding the root, which now waits on 1.2 and 2.
        let mut tasks = tasks.clone();
        tasks[1].done = true;
        tasks[1].pending_answer = true;
        tasks.push(made("2", 1, 13, true));
        let plan = react(
            &steps,
            &tasks,
            true,
            &RunEvent::Answered {
                task: TaskId(11),
                answer: "Yes".into(),
            },
            now(),
            chrono_tz::UTC,
        );
        assert_eq!(plan.create.iter().map(|c| c.step).collect::<Vec<_>>(), vec![3]);
        assert_eq!(plan.answered, Some((TaskId(11), "yes".into())));
        // An answer the question does not take does nothing at all.
        let plan = react(
            &steps,
            &tasks,
            true,
            &RunEvent::Answered {
                task: TaskId(11),
                answer: "maybe".into(),
            },
            now(),
            chrono_tz::UTC,
        );
        assert_eq!(plan, Plan::default());
    }

    #[test]
    fn a_default_answers_for_whoever_did_not() {
        let mut steps = laundry();
        steps[1].question.as_mut().unwrap().default = Some("yes".into());
        let tasks = vec![made(ROOT_KEY, 1, 10, false), made("1", 1, 11, true)];
        let plan = react(
            &steps,
            &tasks,
            true,
            &RunEvent::Finished {
                task: TaskId(11),
                answer: None,
            },
            now(),
            chrono_tz::UTC,
        );
        assert_eq!(plan.answered, Some((TaskId(11), "yes".into())));
        assert_eq!(plan.create.iter().map(|c| c.step).collect::<Vec<_>>(), vec![3, 4]);
    }

    #[test]
    fn questions_know_their_answers_codes_and_limits() {
        let q = last_load();
        assert_eq!(q.answers(), vec!["yes", "no"]);
        assert_eq!(q.canonical(" Yes "), Some("yes".into()));
        assert_eq!(q.canonical("maybe"), None);
        assert_eq!(q.barcode_action(0), ScanAction::Yes);
        assert_eq!(q.barcode_action(1), ScanAction::No);
        assert_eq!(q.answer_for(ScanAction::No), Some("no".into()));
        assert_eq!(q.answer_for(ScanAction::Choice(1)), None, "not a choice question");
        let pick = Question {
            text: "Which room?".into(),
            kind: QuestionKind::Choice(vec!["Kitchen".into(), "Bath".into()]),
            default: Some("bath".into()),
        };
        assert_eq!(pick.validate(), Ok(()));
        assert_eq!(pick.barcode_action(1), ScanAction::Choice(2));
        assert_eq!(pick.answer_for(ScanAction::Choice(2)), Some("Bath".into()));
        assert_eq!(pick.answer_for(ScanAction::Choice(3)), None);
        assert_eq!(pick.answer_for(ScanAction::Yes), None);
        let one = Question {
            kind: QuestionKind::Choice(vec!["only".into()]),
            ..pick.clone()
        };
        assert!(one.validate().is_err(), "two answers at least");
        let stray = Question {
            default: Some("hallway".into()),
            ..pick.clone()
        };
        assert!(stray.validate().is_err(), "the default has to be an answer");

        // A program that waits for an answer nobody can give is refused.
        let board = board_with_members(1, &[1]);
        let mut d = ProgramDraft {
            name: "Laundry".into(),
            steps: laundry(),
            ..Default::default()
        };
        assert_eq!(validate(&d, &board), Ok(()));
        d.steps[3].created = vec![Trigger::Answered {
            step: "1".into(),
            answer: "maybe".into(),
        }];
        assert!(matches!(validate(&d, &board), Err(ProgramError::NoSuchAnswer { .. })));
        d.steps = laundry();
        d.steps[0].question = Some(last_load());
        assert_eq!(validate(&d, &board), Err(ProgramError::RootQuestion));
    }

    #[test]
    fn a_steps_draft_carries_its_content_and_the_starter() {
        let board = board_with_members(1, &[1, 2]);
        let mut s = apartment()[1].clone();
        s.checklist = vec![ChecklistItem {
            text: "socks".into(),
            done: true,
        }];
        let d = draft_for(&s, 2, &board, Some(now()), vec![TaskId(5)], None);
        assert_eq!(d.title, "Step 1");
        assert_eq!(d.assignees, vec![2]);
        assert_eq!(d.start_at, Some(now()));
        assert_eq!(d.depends_on, vec![TaskId(5)]);
        assert!(!d.checklist[0].done, "a fresh task starts unticked");
        s.assign = Assign::Users(vec![1, 9]);
        assert_eq!(draft_for(&s, 2, &board, None, vec![], None).assignees, vec![1]);
        s.assign = Assign::Nobody;
        assert!(draft_for(&s, 2, &board, None, vec![], None).assignees.is_empty());
    }

    #[test]
    fn a_step_that_fans_out_makes_one_task_per_entry_and_is_finished_with_the_last() {
        let mut steps = apartment();
        steps[3].fan_out = Some(vec!["bedroom".into(), "kitchen".into()]);
        steps[3].title = "Clean the {param}".into();
        let tasks = vec![
            made(ROOT_KEY, 1, 10, false),
            made("1", 1, 11, true),
            made("2", 1, 12, true),
        ];
        let plan = react(
            &steps,
            &tasks,
            true,
            &RunEvent::Finished {
                task: TaskId(12),
                answer: None,
            },
            now(),
            chrono_tz::UTC,
        );
        let rooms: Vec<(usize, u32, Option<String>)> = plan
            .create
            .iter()
            .filter(|c| c.step == 3)
            .map(|c| (c.step, c.iteration, c.param.clone()))
            .collect();
        assert_eq!(
            rooms,
            vec![
                (3, 1, Some("bedroom".into())),
                (3, 1, Some("kitchen".into()))
            ],
            "one go, two tasks"
        );
        let board = board_with_members(1, &[1]);
        let d = draft_for(&steps[3], 1, &board, None, vec![], Some("bedroom"));
        assert_eq!(d.title, "Clean the bedroom");
        let mut plain = steps[3].clone();
        plain.title = "Clean".into();
        assert_eq!(
            draft_for(&plain, 1, &board, None, vec![], Some("kitchen")).title,
            "Clean kitchen",
            "a title that never says where gets told"
        );

        // A step waiting on the rooms waits for both of them.
        steps.push(step("5", vec![after("3a")]));
        let mut tasks = tasks;
        tasks.push(StepTask {
            param: Some("bedroom".into()),
            ..made("3a", 1, 13, true)
        });
        tasks.push(StepTask {
            param: Some("kitchen".into()),
            ..made("3a", 1, 14, true)
        });
        let plan = react(
            &steps,
            &tasks,
            true,
            &RunEvent::Finished {
                task: TaskId(13),
                answer: None,
            },
            now(),
            chrono_tz::UTC,
        );
        assert!(plan.create.is_empty(), "the kitchen is still going");
        tasks[3].done = true;
        let plan = react(
            &steps,
            &tasks,
            true,
            &RunEvent::Finished {
                task: TaskId(14),
                answer: None,
            },
            now(),
            chrono_tz::UTC,
        );
        assert_eq!(plan.create.iter().map(|c| c.step).collect::<Vec<_>>(), vec![6]);

        // Fanning out over nothing is refused, and a sheet needs a fan-out.
        let mut d = ProgramDraft {
            name: "Rooms".into(),
            steps: steps.clone(),
            ..Default::default()
        };
        assert_eq!(validate(&d, &board), Ok(()));
        d.steps[3].fan_out = Some(vec![" ".into()]);
        assert_eq!(validate(&d, &board), Err(ProgramError::EmptyFanOut("3a".into())));
        d.steps[3].fan_out = None;
        d.steps[3].print = vec![PrintRule {
            when: crate::print::PrintWhen::OnCreate,
            slip: crate::print::SlipKind::Sheet,
            to: Default::default(),
        }];
        assert!(matches!(validate(&d, &board), Err(ProgramError::Print(_, _))));
    }

    #[test]
    fn a_next_day_rule_saved_before_the_days_field_still_loads() {
        let nine = NaiveTime::from_hms_opt(9, 0, 0).unwrap();
        let old: StartRule = serde_json::from_str(r#"{"next_day_at":"09:00:00"}"#).unwrap();
        assert_eq!(old, StartRule::DaysLaterAt { days: 1, at: nine });
        let json = serde_json::to_string(&StartRule::DaysLaterAt { days: 3, at: nine }).unwrap();
        assert_eq!(json, r#"{"days_later_at":{"days":3,"at":"09:00:00"}}"#);
        assert_eq!(serde_json::from_str::<StartRule>(&json).unwrap(), StartRule::DaysLaterAt { days: 3, at: nine });
        assert_eq!(serde_json::from_str::<StartRule>(r#""manual""#).unwrap(), StartRule::Manual);
    }

    #[test]
    fn days_later_counts_calendar_days_in_the_starter_zone() {
        let tz: Tz = "Europe/Zurich".parse().unwrap();
        // 23:30 in Zurich on the 10th is 21:30 UTC.
        let now = "2026-03-10T21:30:00Z".parse::<DateTime<Utc>>().unwrap();
        let nine = NaiveTime::from_hms_opt(9, 0, 0).unwrap();
        let at = |d| days_later_at(now, d, nine, tz).unwrap().to_rfc3339();
        assert_eq!(at(0), "2026-03-10T08:00:00+00:00", "that same local day, already passed");
        assert_eq!(at(1), "2026-03-11T08:00:00+00:00");
        assert_eq!(at(30), "2026-04-09T07:00:00+00:00", "across the clock change");
    }

    #[test]
    fn summaries_read_like_sentences() {
        assert_eq!(start_rule_summary(StartRule::Manual), "when somebody starts it");
        assert_eq!(start_rule_summary(StartRule::AfterTrigger(minutes(0))), "immediately");
        assert_eq!(start_rule_summary(StartRule::AfterTrigger(minutes(45))), "45 minutes later");
        let nine = NaiveTime::from_hms_opt(9, 0, 0).unwrap();
        assert_eq!(
            start_rule_summary(StartRule::DaysLaterAt { days: 1, at: nine }),
            "next day at 09:00"
        );
        assert_eq!(
            start_rule_summary(StartRule::DaysLaterAt { days: 0, at: nine }),
            "that day at 09:00"
        );
        assert_eq!(
            start_rule_summary(StartRule::DaysLaterAt { days: 3, at: nine }),
            "3 days later at 09:00"
        );
        assert_eq!(triggers_summary(&[]), "never");
        assert_eq!(
            triggers_summary(&[Trigger::WithRoot, after("1")]),
            "with the program, after 1"
        );
    }
}
