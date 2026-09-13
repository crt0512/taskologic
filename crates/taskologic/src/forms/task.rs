//! The task form, for creating and editing.
//!
//! Every field the README lists is here: title, multi line description,
//! due date and time, a reminder override, dependencies across visible
//! boards, assignees limited to members, and repetition. The form validates
//! with the same rules as `taskologic_core`, but the daemon is still the
//! authority on the save.
//!
//! In template mode the due and dependency rows become the template's own:
//! a rule for prefilling the due date, and other templates on the board to
//! stamp out alongside.

use chrono::{DateTime, NaiveDate, NaiveDateTime, NaiveTime, TimeZone, Utc, Weekday};
use chrono_tz::Tz;
use crossterm::event::{Event, KeyCode, KeyEventKind, MouseButton, MouseEventKind};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::widgets::{Clear, ListItem, Paragraph};
use taskologic_core::ids::{BoardId, ColumnId, TaskId, TemplateId, Uid};
use taskologic_core::offset::{MAX_OFFSET_AMOUNT, Offset, OffsetUnit};
use taskologic_core::prefs::{parse_reminder_hours, reminder_hours_text};
use taskologic_core::repeat::{Repeat, RepeatSpec};
use taskologic_core::task::{ChecklistItem, Task, TaskDraft, validate_title};
use taskologic_core::template::{
    DEFAULT_MIN_SAMPLES, FromAverage, Template, TemplateError, TemplateOptions,
};
use taskologic_core::user::UserSummary;
use taskologic_proto::SearchHit;

use super::{Row, button_h, button_row, frame_block, popup};
use crate::ui::adapter::{
    ButtonOutcome, ButtonState, CheckboxState, ChoiceState, Focus, FocusBuilder, HandleEvent,
    HasFocus, HasScreenCursor, ListState, Navigation, Outcome, Regular, TextAreaState,
    TextInputState, checkbox_at, dropdown, dropdown_marker, dropdown_popup_hover, field, list,
    render_button, text_area, text_area_event,
};
use crate::ui::theme::Theme;

/// Which unit dropdown an open list belongs to. The lists are drawn after
/// everything else so they sit on top, by which point the borrow of the form
/// that produced them is long gone.
#[derive(Clone, Copy, PartialEq, Eq)]
enum DropdownKind {
    StartPrefill,
    DuePrefill,
    RepeatStart,
    RepeatDue,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RepeatKind {
    #[default]
    None,
    EveryDays,
    Weekdays,
    DayOfMonth,
    FixedDate,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FormMode {
    Create {
        board: BoardId,
        column: ColumnId,
        /// Set when the form was stamped from a template, so saving goes
        /// through the daemon's template path and the template's own
        /// dependency templates get stamped out too.
        from_template: Option<TemplateId>,
    },
    Edit {
        task: TaskId,
        version: u64,
    },
    /// A template being written. The title doubles as the template name,
    /// and the due and dependency rows carry the template's own: a prefill
    /// rule rather than a date, other templates rather than tasks.
    TemplateNew {
        board: BoardId,
    },
    TemplateEdit {
        board: BoardId,
        template: TemplateId,
    },
}

/// Everything a save carries: the draft, plus the options that only a
/// template has.
#[derive(Debug, PartialEq)]
pub struct TaskSave {
    pub draft: TaskDraft,
    pub options: TemplateOptions,
}

#[derive(Debug, PartialEq)]
pub enum FormOutcome {
    Continue,
    Changed,
    Save(Box<TaskSave>),
    Cancel,
    /// The dependency picker wants search results for this query.
    SearchDeps(String),
}

struct Member {
    uid: Uid,
    name: String,
    check: CheckboxState,
}

pub struct TaskForm {
    pub mode: FormMode,
    tz: Tz,
    /// The task being edited, excluded from its own dependency picker.
    self_task: Option<TaskId>,
    /// Assignees to tick once the member list arrives.
    preselect: Vec<Uid>,
    /// Dependency templates to tick once the template list arrives.
    preselect_templates: Vec<TemplateId>,
    title: TextInputState,
    description: TextAreaState,
    /// One item per line. `[x]` at the start marks it done; plain lines are
    /// open items, so typing a quick list needs no markers at all.
    checklist: TextAreaState,
    /// When work on this should begin, read the same way the due field is.
    start: TextInputState,
    insert_now_start: ButtonState,
    due: TextInputState,
    /// Drops the current date and time into the due field.
    insert_now: ButtonState,
    /// Template mode: the start date rule the stamped tasks begin with.
    start_prefill: CheckboxState,
    start_prefill_amount: TextInputState,
    start_prefill_unit: ChoiceState<OffsetUnit>,
    /// Template mode: the due date rule the stamped tasks start with.
    prefill: CheckboxState,
    prefill_amount: TextInputState,
    prefill_unit: ChoiceState<OffsetUnit>,
    /// Template mode: date the due field by how long these usually take,
    /// falling back to the fixed offset until enough have been finished.
    from_average: CheckboxState,
    min_samples: TextInputState,
    /// The reminder lead times for this task alone, overriding the user's
    /// defaults. One pair per date, the way the settings have them.
    remind_start_override: CheckboxState,
    remind_start_hours: TextInputState,
    remind_override: CheckboxState,
    remind_hours: TextInputState,
    members: Vec<Member>,
    dep_search: TextInputState,
    dep_hits: Vec<SearchHit>,
    dep_hits_list: ListState,
    deps: Vec<(TaskId, String)>,
    deps_list: ListState,
    /// Template mode: the other templates on this board, and whether this
    /// one depends on them.
    dep_templates: Vec<(TemplateId, String, bool)>,
    dep_templates_list: ListState,
    repeat_kind: ChoiceState<RepeatKind>,
    repeat_from: Option<NaiveDate>,
    /// What each copy a repetition makes is dated, counted from the moment
    /// it fires. Unticked leaves that date off the copy.
    repeat_start: CheckboxState,
    repeat_start_amount: TextInputState,
    repeat_start_unit: ChoiceState<OffsetUnit>,
    repeat_due: CheckboxState,
    repeat_due_amount: TextInputState,
    repeat_due_unit: ChoiceState<OffsetUnit>,
    every_n: TextInputState,
    weekdays: Vec<(Weekday, CheckboxState)>,
    day_of_month: TextInputState,
    fixed_date: TextInputState,
    at_time: TextInputState,
    save: ButtonState,
    cancel: ButtonState,
    pub error: Option<String>,
    /// A save is in flight. Input is ignored until the daemon answers.
    pub saving: bool,
}

const WEEKDAYS: [Weekday; 7] = [
    Weekday::Mon,
    Weekday::Tue,
    Weekday::Wed,
    Weekday::Thu,
    Weekday::Fri,
    Weekday::Sat,
    Weekday::Sun,
];

fn weekday_label(w: Weekday) -> &'static str {
    match w {
        Weekday::Mon => "Mon",
        Weekday::Tue => "Tue",
        Weekday::Wed => "Wed",
        Weekday::Thu => "Thu",
        Weekday::Fri => "Fri",
        Weekday::Sat => "Sat",
        Weekday::Sun => "Sun",
    }
}

impl TaskForm {
    fn blank(mode: FormMode, tz: Tz) -> Self {
        let mut at_time = TextInputState::named("at_time");
        at_time.set_text("09:00");
        let mut every_n = TextInputState::named("every_n");
        every_n.set_text("1");
        let mut repeat_kind = ChoiceState::named("repeat_kind");
        repeat_kind.set_value(RepeatKind::None);
        // Ticking the box alone means "now", which the offset says as zero.
        let mut prefill_amount = TextInputState::named("prefill_amount");
        prefill_amount.set_text("0");
        let mut prefill_unit = ChoiceState::named("prefill_unit");
        prefill_unit.set_value(OffsetUnit::Hours);
        let mut start_prefill_amount = TextInputState::named("start_prefill_amount");
        start_prefill_amount.set_text("0");
        let mut start_prefill_unit = ChoiceState::named("start_prefill_unit");
        start_prefill_unit.set_value(OffsetUnit::Hours);
        let mut min_samples = TextInputState::named("min_samples");
        min_samples.set_text(DEFAULT_MIN_SAMPLES.to_string());
        // A repetition dates its copies from the moment it fires, so zero is
        // the sensible starting point for both offsets.
        let mut repeat_start_amount = TextInputState::named("repeat_start_amount");
        repeat_start_amount.set_text("0");
        let mut repeat_start_unit = ChoiceState::named("repeat_start_unit");
        repeat_start_unit.set_value(OffsetUnit::Hours);
        let mut repeat_due_amount = TextInputState::named("repeat_due_amount");
        repeat_due_amount.set_text("1");
        let mut repeat_due_unit = ChoiceState::named("repeat_due_unit");
        repeat_due_unit.set_value(OffsetUnit::Days);
        let title = TextInputState::named("title");
        title.focus().set(true);
        Self {
            mode,
            tz,
            self_task: None,
            preselect: Vec::new(),
            preselect_templates: Vec::new(),
            title,
            description: TextAreaState::named("description"),
            checklist: TextAreaState::named("checklist"),
            start: TextInputState::named("start"),
            insert_now_start: ButtonState::new(),
            due: TextInputState::named("due"),
            insert_now: ButtonState::new(),
            start_prefill: CheckboxState::named("start_prefill"),
            start_prefill_amount,
            start_prefill_unit,
            prefill: CheckboxState::named("prefill"),
            prefill_amount,
            prefill_unit,
            from_average: CheckboxState::named("from_average"),
            min_samples,
            remind_start_override: CheckboxState::named("remind_start_override"),
            remind_start_hours: TextInputState::named("remind_start_hours"),
            remind_override: CheckboxState::named("remind_override"),
            remind_hours: TextInputState::named("remind_hours"),
            members: Vec::new(),
            dep_search: TextInputState::named("dep_search"),
            dep_hits: Vec::new(),
            dep_hits_list: ListState::named("dep_hits"),
            deps: Vec::new(),
            deps_list: ListState::named("deps"),
            dep_templates: Vec::new(),
            dep_templates_list: ListState::named("dep_templates"),
            repeat_kind,
            repeat_from: None,
            repeat_start: CheckboxState::named("repeat_start"),
            repeat_start_amount,
            repeat_start_unit,
            repeat_due: CheckboxState::named("repeat_due"),
            repeat_due_amount,
            repeat_due_unit,
            every_n,
            weekdays: WEEKDAYS
                .iter()
                .map(|w| (*w, CheckboxState::named(weekday_label(*w))))
                .collect(),
            day_of_month: TextInputState::named("day_of_month"),
            fixed_date: TextInputState::named("fixed_date"),
            at_time,
            save: ButtonState::new(),
            cancel: ButtonState::new(),
            error: None,
            saving: false,
        }
    }

    pub fn create(board: BoardId, column: ColumnId, tz: Tz) -> Self {
        Self::blank(
            FormMode::Create {
                board,
                column,
                from_template: None,
            },
            tz,
        )
    }

    /// A new task prefilled from a template. The save goes through the
    /// daemon's template path, so the template's own dependency templates
    /// are stamped out alongside; they are not shown here.
    pub fn create_from_template(board: BoardId, column: ColumnId, tz: Tz, tpl: &Template) -> Self {
        let mut f = Self::blank(
            FormMode::Create {
                board,
                column,
                from_template: Some(tpl.id),
            },
            tz,
        );
        f.apply_draft(&tpl.draft);
        // The rules are the template's, the dates they land on are this
        // moment, and the user can still edit them before saving. The daemon
        // works the due date out again on save, where it can consult the
        // history the "from average" rule needs; what is shown here is the
        // fixed offset, which is what that rule falls back to anyway.
        let now = Utc::now();
        let stamp = |field: &mut TextInputState, at: Option<DateTime<Utc>>| {
            if let Some(at) = at {
                field.set_text(at.with_timezone(&tz).format("%Y-%m-%d %H:%M").to_string());
            }
        };
        stamp(&mut f.start, tpl.options.start_for(now));
        stamp(&mut f.due, tpl.options.due_prefill.and_then(|p| p.after(now)));
        f
    }

    pub fn template_new(board: BoardId, tz: Tz) -> Self {
        Self::blank(FormMode::TemplateNew { board }, tz)
    }

    pub fn template_edit(tpl: &Template, tz: Tz) -> Self {
        let mut f = Self::blank(
            FormMode::TemplateEdit {
                board: tpl.board_id,
                template: tpl.id,
            },
            tz,
        );
        f.apply_draft(&tpl.draft);
        f.preselect_templates = tpl.options.dep_templates.clone();
        if let Some(p) = tpl.options.start_prefill {
            f.start_prefill.set_checked(true);
            f.start_prefill_amount.set_text(p.amount.to_string());
            f.start_prefill_unit.set_value(p.unit);
        }
        if let Some(p) = tpl.options.due_prefill {
            f.prefill.set_checked(true);
            f.prefill_amount.set_text(p.amount.to_string());
            f.prefill_unit.set_value(p.unit);
        }
        if let Some(a) = tpl.options.due_from_average {
            f.from_average.set_checked(true);
            f.min_samples.set_text(a.min_samples.to_string());
        }
        f
    }

    /// The templates on this board that this one may depend on, with the
    /// ones it already depends on ticked. The form excludes itself.
    pub fn set_template_choices(&mut self, templates: &[Template]) {
        let me = match self.mode {
            FormMode::TemplateEdit { template, .. } => Some(template),
            _ => None,
        };
        self.dep_templates = templates
            .iter()
            .filter(|t| Some(t.id) != me)
            .map(|t| {
                (
                    t.id,
                    t.name.clone(),
                    self.preselect_templates.contains(&t.id),
                )
            })
            .collect();
        self.dep_templates_list
            .select((!self.dep_templates.is_empty()).then_some(0));
    }

    fn toggle_dep_template(&mut self, i: usize) {
        if let Some((_, _, on)) = self.dep_templates.get_mut(i) {
            *on = !*on;
        }
    }

    fn is_template(&self) -> bool {
        matches!(
            self.mode,
            FormMode::TemplateNew { .. } | FormMode::TemplateEdit { .. }
        )
    }

    /// Fill the fields from a draft: title, description, checklist, the
    /// assignee preselection, the reminder override and the repetition. Due
    /// date and dependencies stay empty, this is only used for templates.
    fn apply_draft(&mut self, draft: &TaskDraft) {
        self.preselect = draft.assignees.clone();
        self.title.set_text(draft.title.clone());
        self.description.set_text(&draft.description);
        self.apply_reminders(draft.reminder_start_minutes, draft.reminder_due_minutes);
        if !draft.checklist.is_empty() {
            let text: Vec<String> = draft
                .checklist
                .iter()
                .map(|c| format!("{} {}", if c.done { "[x]" } else { "[ ]" }, c.text))
                .collect();
            self.checklist.set_text(&text.join("\n"));
        }
        if let Some(spec) = &draft.repeat {
            self.apply_repeat(spec);
        }
    }

    /// An override that is set means its box is ticked: the stored minutes
    /// come back as the hours the user typed.
    fn apply_reminders(&mut self, start: Option<u32>, due: Option<u32>) {
        if let Some(m) = start {
            self.remind_start_override.set_checked(true);
            self.remind_start_hours.set_text(reminder_hours_text(m));
        }
        if let Some(m) = due {
            self.remind_override.set_checked(true);
            self.remind_hours.set_text(reminder_hours_text(m));
        }
    }

    fn apply_repeat(&mut self, spec: &RepeatSpec) {
        self.at_time.set_text(spec.at.format("%H:%M").to_string());
        if let Some(o) = spec.start_rule {
            self.repeat_start.set_checked(true);
            self.repeat_start_amount.set_text(o.amount.to_string());
            self.repeat_start_unit.set_value(o.unit);
        }
        if let Some(o) = spec.due_rule {
            self.repeat_due.set_checked(true);
            self.repeat_due_amount.set_text(o.amount.to_string());
            self.repeat_due_unit.set_value(o.unit);
        }
        match &spec.rule {
            Repeat::EveryDays { every, from } => {
                self.repeat_kind.set_value(RepeatKind::EveryDays);
                self.every_n.set_text(every.to_string());
                self.repeat_from = Some(*from);
            }
            Repeat::Weekdays { days } => {
                self.repeat_kind.set_value(RepeatKind::Weekdays);
                for (w, c) in &mut self.weekdays {
                    c.set_checked(days.contains(w));
                }
            }
            Repeat::DayOfMonth { day } => {
                self.repeat_kind.set_value(RepeatKind::DayOfMonth);
                self.day_of_month.set_text(day.to_string());
            }
            Repeat::FixedDate { date } => {
                self.repeat_kind.set_value(RepeatKind::FixedDate);
                self.fixed_date
                    .set_text(date.format("%Y-%m-%d").to_string());
            }
        }
    }

    /// Prefilled from an existing task. `dep_titles` resolves dependency ids
    /// to something readable; unknown ones show as their id.
    pub fn edit(task: &Task, tz: Tz, dep_titles: &dyn Fn(TaskId) -> Option<String>) -> Self {
        let mut f = Self::blank(
            FormMode::Edit {
                task: task.id,
                version: task.version,
            },
            tz,
        );
        f.self_task = Some(task.id);
        f.preselect = task.assignees.clone();
        f.title.set_text(task.title.clone());
        f.description.set_text(&task.description);
        if !task.checklist.is_empty() {
            let text: Vec<String> = task
                .checklist
                .iter()
                .map(|c| format!("{} {}", if c.done { "[x]" } else { "[ ]" }, c.text))
                .collect();
            f.checklist.set_text(&text.join("\n"));
        }
        if let Some(start) = task.start_at {
            f.start
                .set_text(start.with_timezone(&tz).format("%Y-%m-%d %H:%M").to_string());
        }
        if let Some(due) = task.due_at {
            f.due
                .set_text(due.with_timezone(&tz).format("%Y-%m-%d %H:%M").to_string());
        }
        f.apply_reminders(task.reminder_start_minutes, task.reminder_due_minutes);
        f.deps = task
            .depends_on
            .iter()
            .map(|d| (*d, dep_titles(*d).unwrap_or_else(|| format!("task {d}"))))
            .collect();
        if let Some(spec) = &task.repeat {
            f.apply_repeat(spec);
        }
        f
    }

    /// After a conflict the user chose to overwrite: save against the
    /// version that is current now.
    pub fn set_version(&mut self, version: u64) {
        if let FormMode::Edit { task, .. } = self.mode {
            self.mode = FormMode::Edit { task, version };
        }
    }

    pub fn set_members(&mut self, members: &[UserSummary]) {
        self.members = members
            .iter()
            .map(|m| {
                let mut check = CheckboxState::named(&m.username);
                check.set_checked(self.preselect.contains(&m.uid));
                Member {
                    uid: m.uid,
                    name: m.username.clone(),
                    check,
                }
            })
            .collect();
    }

    pub fn set_dep_hits(&mut self, hits: Vec<SearchHit>) {
        self.dep_hits = hits
            .into_iter()
            .filter(|h| {
                Some(h.task_id) != self.self_task
                    && !self.deps.iter().any(|(id, _)| *id == h.task_id)
            })
            .collect();
        self.dep_hits_list.select(if self.dep_hits.is_empty() {
            None
        } else {
            Some(0)
        });
        if !self.dep_hits.is_empty() {
            self.dep_search.focus().set(false);
            self.dep_hits_list.focus().set(true);
        }
    }

    fn remove_dep(&mut self, i: usize) {
        if i < self.deps.len() {
            self.deps.remove(i);
            self.deps_list.select(if self.deps.is_empty() {
                None
            } else {
                Some(i.min(self.deps.len() - 1))
            });
        }
    }

    fn add_dep(&mut self, i: usize) {
        if i < self.dep_hits.len() {
            let h = self.dep_hits.remove(i);
            self.deps
                .push((h.task_id, format!("{} / {}", h.board_name, h.title)));
            self.dep_hits_list.select(if self.dep_hits.is_empty() {
                None
            } else {
                Some(i.min(self.dep_hits.len() - 1))
            });
        }
    }

    fn focus(&self) -> Focus {
        let mut b = FocusBuilder::new(None);
        // Textareas default to keeping Tab as input, which traps keyboard
        // users. Nobody needs literal tabs in a description.
        b.widget(&self.title);
        b.widget_navigate(&self.description, Navigation::Regular);
        b.widget_navigate(&self.checklist, Navigation::Regular);
        if self.is_template() {
            b.widget(&self.start_prefill);
            if self.start_prefill.checked() {
                b.widget(&self.start_prefill_amount)
                    .widget(&self.start_prefill_unit);
            }
            b.widget(&self.prefill);
            if self.prefill.checked() {
                b.widget(&self.prefill_amount).widget(&self.prefill_unit);
            }
            b.widget(&self.from_average);
            if self.from_average.checked() {
                b.widget(&self.min_samples);
            }
        } else {
            b.widget(&self.start).widget(&self.insert_now_start);
            b.widget(&self.due).widget(&self.insert_now);
        }
        b.widget(&self.remind_start_override);
        if self.remind_start_override.checked() {
            b.widget(&self.remind_start_hours);
        }
        b.widget(&self.remind_override);
        if self.remind_override.checked() {
            b.widget(&self.remind_hours);
        }
        for m in &self.members {
            b.widget(&m.check);
        }
        if self.is_template() {
            b.widget(&self.dep_templates_list);
        } else {
            b.widget(&self.dep_search)
                .widget(&self.dep_hits_list)
                .widget(&self.deps_list);
        }
        b.widget(&self.repeat_kind);
        match self.repeat_kind.value() {
            RepeatKind::None => {}
            RepeatKind::EveryDays => {
                b.widget(&self.every_n);
            }
            RepeatKind::Weekdays => {
                for (_, c) in &self.weekdays {
                    b.widget(c);
                }
            }
            RepeatKind::DayOfMonth => {
                b.widget(&self.day_of_month);
            }
            RepeatKind::FixedDate => {
                b.widget(&self.fixed_date);
            }
        }
        if self.repeat_kind.value() != RepeatKind::None {
            b.widget(&self.at_time);
            b.widget(&self.repeat_start);
            if self.repeat_start.checked() {
                b.widget(&self.repeat_start_amount)
                    .widget(&self.repeat_start_unit);
            }
            b.widget(&self.repeat_due);
            if self.repeat_due.checked() {
                b.widget(&self.repeat_due_amount)
                    .widget(&self.repeat_due_unit);
            }
        }
        b.widget(&self.save).widget(&self.cancel);
        b.build()
    }

    pub fn handle(&mut self, ev: &Event) -> FormOutcome {
        if self.saving {
            return FormOutcome::Continue;
        }
        let key = match ev {
            Event::Key(k) if k.kind != KeyEventKind::Release => Some(k.code),
            _ => None,
        };
        // An open dropdown eats Esc to close itself.
        let popup_open = self.repeat_kind.is_popup_active()
            || self.prefill_unit.is_popup_active()
            || self.start_prefill_unit.is_popup_active()
            || self.repeat_start_unit.is_popup_active()
            || self.repeat_due_unit.is_popup_active();
        match key {
            Some(KeyCode::Esc) if !popup_open => return FormOutcome::Cancel,
            Some(KeyCode::F(2)) => return self.try_save(),
            _ => {}
        }

        // The ring handles Tab and click to focus. A consumed key is done.
        let mut focus = self.focus();
        if focus.handle(ev, Regular) == Outcome::Changed && key.is_some() {
            return FormOutcome::Changed;
        }

        if self.save.handle(ev, Regular) == ButtonOutcome::Pressed {
            return self.try_save();
        }
        if self.cancel.handle(ev, Regular) == ButtonOutcome::Pressed {
            return FormOutcome::Cancel;
        }
        // Written the way `parse_when` reads it back, so the field stays
        // editable rather than turning into a magic value.
        let now_text = || {
            Utc::now()
                .with_timezone(&self.tz)
                .format("%Y-%m-%d %H:%M")
                .to_string()
        };
        if self.insert_now.handle(ev, Regular) == ButtonOutcome::Pressed {
            self.due.set_text(now_text());
            return FormOutcome::Changed;
        }
        if self.insert_now_start.handle(ev, Regular) == ButtonOutcome::Pressed {
            self.start.set_text(now_text());
            return FormOutcome::Changed;
        }

        // Template dependencies are ticked, not searched for.
        if self.dep_templates_list.is_focused()
            && matches!(key, Some(KeyCode::Char(' ') | KeyCode::Enter))
        {
            if let Some(i) = self.dep_templates_list.selected() {
                self.toggle_dep_template(i);
            }
            return FormOutcome::Changed;
        }
        if let Event::Mouse(m) = ev
            && matches!(m.kind, MouseEventKind::Down(MouseButton::Left))
            && let Some(i) = self
                .dep_templates_list
                .row_areas
                .iter()
                .position(|r| r.contains(ratatui::layout::Position::new(m.column, m.row)))
        {
            self.toggle_dep_template(i);
            return FormOutcome::Changed;
        }

        // Dependency picker: Enter searches or picks, Delete drops.
        if key == Some(KeyCode::Enter) {
            if self.dep_search.is_focused() {
                let q = self.dep_search.text().trim().to_string();
                return if q.is_empty() {
                    FormOutcome::Changed
                } else {
                    FormOutcome::SearchDeps(q)
                };
            }
            if self.dep_hits_list.is_focused() {
                if let Some(i) = self.dep_hits_list.selected() {
                    self.add_dep(i);
                }
                return FormOutcome::Changed;
            }
        }
        if matches!(
            key,
            Some(KeyCode::Delete | KeyCode::Backspace | KeyCode::Char('x'))
        ) && self.deps_list.is_focused()
        {
            if let Some(i) = self.deps_list.selected() {
                self.remove_dep(i);
            }
            return FormOutcome::Changed;
        }
        // Clicking a chosen dependency drops it. The list is only there to
        // show what is picked, so there is nothing else a click could mean.
        if let Event::Mouse(m) = ev
            && matches!(m.kind, MouseEventKind::Down(MouseButton::Left))
            && let Some(i) = self
                .deps_list
                .row_areas
                .iter()
                .position(|r| r.contains(ratatui::layout::Position::new(m.column, m.row)))
        {
            self.remove_dep(i);
            return FormOutcome::Changed;
        }
        // Clicking a search result adds it, the same as Enter.
        if let Event::Mouse(m) = ev
            && matches!(m.kind, MouseEventKind::Down(MouseButton::Left))
            && let Some(i) = self
                .dep_hits_list
                .row_areas
                .iter()
                .position(|r| r.contains(ratatui::layout::Position::new(m.column, m.row)))
        {
            self.add_dep(i);
            return FormOutcome::Changed;
        }

        // Everything else goes to the widgets. Each one ignores keys unless
        // it has focus and mouse events outside its area.
        self.title.handle(ev, Regular);
        text_area_event(&mut self.description, ev);
        text_area_event(&mut self.checklist, ev);
        if self.is_template() {
            self.start_prefill.handle(ev, Regular);
            if self.start_prefill.checked() {
                self.start_prefill_amount.handle(ev, Regular);
                self.start_prefill_unit.handle(ev, Regular);
            }
            self.prefill.handle(ev, Regular);
            if self.prefill.checked() {
                self.prefill_amount.handle(ev, Regular);
                self.prefill_unit.handle(ev, Regular);
            }
            self.from_average.handle(ev, Regular);
            if self.from_average.checked() {
                self.min_samples.handle(ev, Regular);
            }
            self.dep_templates_list.handle(ev, Regular);
        } else {
            self.start.handle(ev, Regular);
            self.due.handle(ev, Regular);
            self.dep_search.handle(ev, Regular);
            self.dep_hits_list.handle(ev, Regular);
            self.deps_list.handle(ev, Regular);
        }
        self.remind_start_override.handle(ev, Regular);
        if self.remind_start_override.checked() {
            self.remind_start_hours.handle(ev, Regular);
        }
        self.remind_override.handle(ev, Regular);
        if self.remind_override.checked() {
            self.remind_hours.handle(ev, Regular);
        }
        for m in &mut self.members {
            m.check.handle(ev, Regular);
        }
        self.repeat_kind.handle(ev, Regular);
        match self.repeat_kind.value() {
            RepeatKind::None => {}
            RepeatKind::EveryDays => {
                self.every_n.handle(ev, Regular);
            }
            RepeatKind::Weekdays => {
                for (_, c) in &mut self.weekdays {
                    c.handle(ev, Regular);
                }
            }
            RepeatKind::DayOfMonth => {
                self.day_of_month.handle(ev, Regular);
            }
            RepeatKind::FixedDate => {
                self.fixed_date.handle(ev, Regular);
            }
        }
        if self.repeat_kind.value() != RepeatKind::None {
            self.at_time.handle(ev, Regular);
            self.repeat_start.handle(ev, Regular);
            if self.repeat_start.checked() {
                self.repeat_start_amount.handle(ev, Regular);
                self.repeat_start_unit.handle(ev, Regular);
            }
            self.repeat_due.handle(ev, Regular);
            if self.repeat_due.checked() {
                self.repeat_due_amount.handle(ev, Regular);
                self.repeat_due_unit.handle(ev, Regular);
            }
        }
        FormOutcome::Changed
    }

    fn try_save(&mut self) -> FormOutcome {
        match self.values() {
            Ok(save) => {
                self.error = None;
                FormOutcome::Save(Box::new(save))
            }
            Err(e) => {
                self.error = Some(e);
                FormOutcome::Changed
            }
        }
    }

    /// The draft and template options as the fields stand, or the first
    /// validation problem.
    pub fn values(&self) -> Result<TaskSave, String> {
        let title = self.title.text().trim().to_string();
        validate_title(&title).map_err(|e| e.to_string())?;
        // A template carries a prefill rule rather than a date, and other
        // templates rather than task dependencies; those fields are hidden,
        // this only makes the draft say the same.
        let (start_at, due_at) = if self.is_template() {
            (None, None)
        } else {
            let start = parse_when(self.start.text().trim(), self.tz, "start")?;
            let due = parse_when(self.due.text().trim(), self.tz, "due")?;
            // The daemon refuses this too. Saying so here saves a round trip
            // and points at the field rather than at the save.
            if let (Some(s), Some(d)) = (start, due)
                && s > d
            {
                return Err("the start date cannot be after the due date".into());
            }
            (start, due)
        };
        let assignees = self
            .members
            .iter()
            .filter(|m| m.check.checked())
            .map(|m| m.uid)
            .collect();
        let depends_on = if self.is_template() {
            Vec::new()
        } else {
            self.deps.iter().map(|(id, _)| *id).collect()
        };
        let checklist = parse_checklist(&self.checklist.text());
        let repeat = self.repeat_spec()?;
        let reminder_start_minutes = reminder_of(
            &self.remind_start_override,
            &self.remind_start_hours,
            "start",
        )?;
        let reminder_due_minutes =
            reminder_of(&self.remind_override, &self.remind_hours, "due")?;
        let due_prefill = self.offset_of(&self.prefill, &self.prefill_amount, &self.prefill_unit)?;
        let due_from_average = if self.from_average.checked() {
            if due_prefill.is_none() {
                return Err(
                    "the average needs a plain due prefill to fall back on before there                      is enough history"
                        .into(),
                );
            }
            Some(FromAverage {
                min_samples: self
                    .min_samples
                    .text()
                    .trim()
                    .parse()
                    .map_err(|_| "samples needs a whole number".to_string())?,
            })
        } else {
            None
        };
        let options = TemplateOptions {
            start_prefill: self.offset_of(
                &self.start_prefill,
                &self.start_prefill_amount,
                &self.start_prefill_unit,
            )?,
            due_prefill,
            due_from_average,
            dep_templates: self
                .dep_templates
                .iter()
                .filter(|(_, _, on)| *on)
                .map(|(id, _, _)| *id)
                .collect(),
        };
        Ok(TaskSave {
            draft: TaskDraft {
                title,
                description: self.description.text(),
                start_at,
                due_at,
                reminder_start_minutes,
                reminder_due_minutes,
                assignees,
                depends_on,
                checklist,
                repeat,
            },
            options,
        })
    }

    /// An amount and unit pair, when its box is ticked.
    fn offset_of(
        &self,
        on: &CheckboxState,
        amount: &TextInputState,
        unit: &ChoiceState<OffsetUnit>,
    ) -> Result<Option<Offset>, String> {
        if !on.checked() {
            return Ok(None);
        }
        let amount: u32 = amount
            .text()
            .trim()
            .parse()
            .map_err(|_| "the offset needs a whole number".to_string())?;
        if amount > MAX_OFFSET_AMOUNT {
            return Err(TemplateError::PrefillTooLarge.to_string());
        }
        Ok(Some(Offset {
            amount,
            unit: unit.value(),
        }))
    }

    fn repeat_spec(&self) -> Result<Option<RepeatSpec>, String> {
        let rule = match self.repeat_kind.value() {
            RepeatKind::None => return Ok(None),
            RepeatKind::EveryDays => {
                let every: u32 = self
                    .every_n
                    .text()
                    .trim()
                    .parse()
                    .map_err(|_| "repeat every N days needs a number".to_string())?;
                let from = self
                    .repeat_from
                    .unwrap_or_else(|| Utc::now().with_timezone(&self.tz).date_naive());
                Repeat::EveryDays { every, from }
            }
            RepeatKind::Weekdays => Repeat::Weekdays {
                days: self
                    .weekdays
                    .iter()
                    .filter(|(_, c)| c.checked())
                    .map(|(w, _)| *w)
                    .collect(),
            },
            RepeatKind::DayOfMonth => {
                let day: u32 = self
                    .day_of_month
                    .text()
                    .trim()
                    .parse()
                    .map_err(|_| "day of month needs a number".to_string())?;
                Repeat::DayOfMonth { day }
            }
            RepeatKind::FixedDate => {
                let date = NaiveDate::parse_from_str(self.fixed_date.text().trim(), "%Y-%m-%d")
                    .map_err(|_| "fixed date must look like 2026-12-24".to_string())?;
                Repeat::FixedDate { date }
            }
        };
        let at = NaiveTime::parse_from_str(self.at_time.text().trim(), "%H:%M")
            .map_err(|_| "repeat time must look like 09:00".to_string())?;
        let spec = RepeatSpec {
            rule,
            at,
            tz: self.tz,
            start_rule: self.offset_of(
                &self.repeat_start,
                &self.repeat_start_amount,
                &self.repeat_start_unit,
            )?,
            due_rule: self.offset_of(
                &self.repeat_due,
                &self.repeat_due_amount,
                &self.repeat_due_unit,
            )?,
        };
        spec.validate().map_err(|e| e.to_string())?;
        Ok(Some(spec))
    }

    pub fn render(&mut self, f: &mut Frame, area: Rect, t: &Theme) {
        let bh = button_h(t);
        let p = popup(area, 78, 32 + bh);
        f.render_widget(Clear, p);
        let title = match self.mode {
            FormMode::Create { .. } => " New task ",
            FormMode::Edit { .. } => " Edit task ",
            FormMode::TemplateNew { .. } => " New template ",
            FormMode::TemplateEdit { .. } => " Edit template ",
        };
        let hint = if self.saving {
            " saving... "
        } else {
            " Tab moves   F2 saves   Esc cancels "
        };
        let block = frame_block(title, hint, t);
        let inner = block.inner(p);
        f.render_widget(block, p);

        let label_w = 12u16;
        let member_rows = self
            .member_rows(inner.width.saturating_sub(label_w) as usize)
            .max(1) as u16;
        // A blank row between fields: they carry their own background, so
        // without one the boxes run into each other.
        let gap = Constraint::Length(1);
        let rows = Layout::vertical([
            Constraint::Length(1), // 0  title
            gap,
            Constraint::Length(4), // 2  description
            gap,
            Constraint::Length(3), // 4  checklist
            gap,
            Constraint::Length(1), // 6  start
            Constraint::Length(1), // 7  due
            gap,
            Constraint::Length(1), // 9  reminder
            gap,
            Constraint::Length(member_rows), // 11 assignees
            gap,
            Constraint::Length(1), // 13 dependency search
            Constraint::Length(3), // 14 dependency lists
            gap,
            Constraint::Length(1),  // 16 repeat rule
            Constraint::Length(1),  // 17 repeat detail
            Constraint::Length(1),  // 18 what the copies are dated
            Constraint::Min(1),     // 19 error
            Constraint::Length(bh), // 20 buttons
        ])
        .split(inner);
        let split = |r: Rect| -> (Rect, Rect) {
            let [l, w] =
                Layout::horizontal([Constraint::Length(label_w), Constraint::Min(1)]).areas(r);
            (l, w)
        };

        let (l, w) = split(rows[0]);
        super::label(f, l, "Title", t);
        f.render_stateful_widget(field(t), w, &mut self.title);

        let (l, w) = split(rows[2]);
        super::label(f, l, "Description", t);
        f.render_stateful_widget(text_area(t), w, &mut self.description);

        let (l, w) = split(rows[4]);
        super::label(f, l, "Checklist", t);
        let [w, hint] =
            Layout::horizontal([Constraint::Percentage(60), Constraint::Min(1)]).areas(w);
        f.render_stateful_widget(text_area(t), w, &mut self.checklist);
        f.render_widget(
            Paragraph::new(" one item per line,\n [x] marks it done").style(t.surface_dim()),
            hint,
        );

        // Unit lists open over the rows below them, so they are drawn last.
        let mut unit_popups: Vec<(_, Rect, DropdownKind)> = Vec::new();
        let units = || OffsetUnit::ALL.map(|u| (u, u.label()));
        let pad = if t.touch { 2 } else { 0 };

        let (l, w) = split(rows[6]);
        super::label(f, l, "Start", t);
        let mut r = Row::new(w);
        if self.is_template() {
            let cb = r.take(super::check_w("now plus"));
            f.render_stateful_widget(
                checkbox_at("now plus".into(), cb, t),
                cb,
                &mut self.start_prefill,
            );
            if self.start_prefill.checked() {
                f.render_stateful_widget(field(t), r.take(5), &mut self.start_prefill_amount);
                let unit_area = r.take(11);
                let (unit_w, popup) = dropdown(units(), unit_area, t);
                f.render_stateful_widget(unit_w, unit_area, &mut self.start_prefill_unit);
                dropdown_marker(f, &self.start_prefill_unit, t);
                unit_popups.push((popup, unit_area, DropdownKind::StartPrefill));
            }
        } else {
            f.render_stateful_widget(field(t), r.take(17), &mut self.start);
            let btn = r.take(super::button_w(" Insert current ") + pad);
            render_button(f, btn, " Insert current ", &mut self.insert_now_start, t);
            f.render_widget(
                Paragraph::new(" when work should begin").style(t.surface_dim()),
                r.rest(),
            );
        }

        let (l, w) = split(rows[7]);
        super::label(f, l, "Due", t);
        let mut r = Row::new(w);
        if self.is_template() {
            let cb = r.take(super::check_w("now plus"));
            f.render_stateful_widget(
                checkbox_at("now plus".into(), cb, t),
                cb,
                &mut self.prefill,
            );
            if self.prefill.checked() {
                f.render_stateful_widget(field(t), r.take(5), &mut self.prefill_amount);
                let unit_area = r.take(11);
                let (unit_w, popup) = dropdown(units(), unit_area, t);
                f.render_stateful_widget(unit_w, unit_area, &mut self.prefill_unit);
                dropdown_marker(f, &self.prefill_unit, t);
                unit_popups.push((popup, unit_area, DropdownKind::DuePrefill));
            }
            let cb = r.take(super::check_w("or start plus average of"));
            f.render_stateful_widget(
                checkbox_at("or start plus average of".into(), cb, t),
                cb,
                &mut self.from_average,
            );
            if self.from_average.checked() {
                f.render_stateful_widget(field(t), r.take(4), &mut self.min_samples);
                f.render_widget(
                    Paragraph::new("finished").style(t.surface_dim()),
                    r.rest(),
                );
            }
        } else {
            f.render_stateful_widget(field(t), r.take(17), &mut self.due);
            let btn = r.take(super::button_w(" Insert current ") + pad);
            render_button(f, btn, " Insert current ", &mut self.insert_now, t);
            f.render_widget(
                Paragraph::new(" YYYY-MM-DD HH:MM").style(t.surface_dim()),
                r.rest(),
            );
        }

        let (l, w) = split(rows[9]);
        super::label(f, l, "Remind", t);
        let mut r = Row::new(w);
        let cb = r.take(super::check_w("start"));
        f.render_stateful_widget(
            checkbox_at("start".into(), cb, t),
            cb,
            &mut self.remind_start_override,
        );
        if self.remind_start_override.checked() {
            f.render_stateful_widget(field(t), r.take(5), &mut self.remind_start_hours);
            f.render_widget(Paragraph::new("h,").style(t.surface_dim()), r.text("h,"));
        }
        let cb = r.take(super::check_w("due"));
        f.render_stateful_widget(
            checkbox_at("due".into(), cb, t),
            cb,
            &mut self.remind_override,
        );
        if self.remind_override.checked() {
            f.render_stateful_widget(field(t), r.take(5), &mut self.remind_hours);
            f.render_widget(
                Paragraph::new("h before").style(t.surface_dim()),
                r.rest(),
            );
        } else if !self.remind_start_override.checked() {
            f.render_widget(
                Paragraph::new("your settings decide").style(t.surface_dim()),
                r.rest(),
            );
        }

        let (l, w) = split(rows[11]);
        super::label(f, l, "Assignees", t);
        if self.members.is_empty() {
            f.render_widget(
                Paragraph::new("loading members...").style(t.surface_dim()),
                w,
            );
        }
        let mut x = w.x;
        let mut y = w.y;
        for m in &mut self.members {
            let cw = super::check_w(&m.name) + 1;
            if x + cw > w.right() && x > w.x {
                x = w.x;
                y += 1;
            }
            if y >= w.bottom() {
                break;
            }
            let cb = Rect::new(x, y, cw.min(w.right() - x), 1);
            f.render_stateful_widget(checkbox_at(m.name.clone(), cb, t), cb, &mut m.check);
            x += cw;
        }

        let (l, w) = split(rows[13]);
        super::label(f, l, "Depends on", t);
        if self.is_template() {
            f.render_widget(
                Paragraph::new("Space ticks the templates this one needs").style(t.surface_dim()),
                w,
            );
        } else {
            let [w, hint] =
                Layout::horizontal([Constraint::Percentage(50), Constraint::Min(1)]).areas(w);
            f.render_stateful_widget(field(t), w, &mut self.dep_search);
            f.render_widget(
                Paragraph::new(
                    " Enter searches, click a result to add, click a picked one to drop",
                )
                .style(t.surface_dim()),
                hint,
            );
        }

        let (_, w) = split(rows[14]);
        if self.is_template() {
            let items: Vec<ListItem> = if self.dep_templates.is_empty() {
                vec![ListItem::new("no other templates on this board").style(t.surface_dim())]
            } else {
                self.dep_templates
                    .iter()
                    .map(|(_, name, on)| {
                        ListItem::new(format!("{} {name}", if *on { "[x]" } else { "[ ]" }))
                    })
                    .collect()
            };
            f.render_stateful_widget(list(items, t), w, &mut self.dep_templates_list);
        } else {
            let [hits, chosen] =
                Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)])
                    .areas(w);
            let hit_items: Vec<ListItem> = if self.dep_hits.is_empty() {
                vec![ListItem::new("no results").style(t.surface_dim())]
            } else {
                self.dep_hits
                    .iter()
                    .map(|h| ListItem::new(format!("{} / {}", h.board_name, h.title)))
                    .collect()
            };
            f.render_stateful_widget(list(hit_items, t), hits, &mut self.dep_hits_list);
            let chosen_items: Vec<ListItem> = if self.deps.is_empty() {
                vec![ListItem::new("no dependencies").style(t.surface_dim())]
            } else {
                self.deps
                    .iter()
                    .map(|(_, title)| ListItem::new(format!("x  {title}")))
                    .collect()
            };
            f.render_stateful_widget(list(chosen_items, t), chosen, &mut self.deps_list);
        }

        let (l, w) = split(rows[16]);
        super::label(f, l, "Repeat", t);
        let items = [
            (RepeatKind::None, "never"),
            (RepeatKind::EveryDays, "every N days"),
            (RepeatKind::Weekdays, "on weekdays"),
            (RepeatKind::DayOfMonth, "day of month"),
            (RepeatKind::FixedDate, "on a fixed date"),
        ];
        let [w, _] = Layout::horizontal([Constraint::Length(20), Constraint::Min(0)]).areas(w);
        let (repeat_w, repeat_popup) = dropdown(items, w, t);
        f.render_stateful_widget(repeat_w, w, &mut self.repeat_kind);
        dropdown_marker(f, &self.repeat_kind, t);
        let repeat_area = w;

        let (_, w) = split(rows[17]);
        match self.repeat_kind.value() {
            RepeatKind::None => {}
            RepeatKind::EveryDays => {
                let mut r = Row::new(w);
                f.render_widget(
                    Paragraph::new("every").style(t.surface_dim()),
                    r.text("every"),
                );
                f.render_stateful_widget(field(t), r.take(4), &mut self.every_n);
                f.render_widget(
                    Paragraph::new("days at").style(t.surface_dim()),
                    r.text("days at"),
                );
                f.render_stateful_widget(field(t), r.take(6), &mut self.at_time);
            }
            RepeatKind::Weekdays => {
                let mut r = Row::new(w);
                for (wd, c) in &mut self.weekdays {
                    let label = weekday_label(*wd);
                    let cb = r.take(super::check_w(label));
                    f.render_stateful_widget(checkbox_at(label.into(), cb, t), cb, c);
                }
                f.render_widget(Paragraph::new("at").style(t.surface_dim()), r.text("at"));
                f.render_stateful_widget(field(t), r.take(6), &mut self.at_time);
            }
            RepeatKind::DayOfMonth => {
                let mut r = Row::new(w);
                f.render_widget(Paragraph::new("day").style(t.surface_dim()), r.text("day"));
                f.render_stateful_widget(field(t), r.take(4), &mut self.day_of_month);
                f.render_widget(
                    Paragraph::new("of the month at").style(t.surface_dim()),
                    r.text("of the month at"),
                );
                f.render_stateful_widget(field(t), r.take(6), &mut self.at_time);
            }
            RepeatKind::FixedDate => {
                let mut r = Row::new(w);
                f.render_widget(Paragraph::new("on").style(t.surface_dim()), r.text("on"));
                f.render_stateful_widget(field(t), r.take(11), &mut self.fixed_date);
                f.render_widget(Paragraph::new("at").style(t.surface_dim()), r.text("at"));
                f.render_stateful_widget(field(t), r.take(6), &mut self.at_time);
            }
        }

        if self.repeat_kind.value() != RepeatKind::None {
            let (l, w) = split(rows[18]);
            super::label(f, l, "Copies get", t);
            let mut r = Row::new(w);
            let cb = r.take(super::check_w("start"));
            f.render_stateful_widget(
                checkbox_at("start".into(), cb, t),
                cb,
                &mut self.repeat_start,
            );
            if self.repeat_start.checked() {
                f.render_stateful_widget(field(t), r.take(4), &mut self.repeat_start_amount);
                let unit_area = r.take(10);
                let (unit_w, popup) = dropdown(units(), unit_area, t);
                f.render_stateful_widget(unit_w, unit_area, &mut self.repeat_start_unit);
                dropdown_marker(f, &self.repeat_start_unit, t);
                unit_popups.push((popup, unit_area, DropdownKind::RepeatStart));
            }
            let cb = r.take(super::check_w("due"));
            f.render_stateful_widget(
                checkbox_at("due".into(), cb, t),
                cb,
                &mut self.repeat_due,
            );
            if self.repeat_due.checked() {
                f.render_stateful_widget(field(t), r.take(4), &mut self.repeat_due_amount);
                let unit_area = r.take(10);
                let (unit_w, popup) = dropdown(units(), unit_area, t);
                f.render_stateful_widget(unit_w, unit_area, &mut self.repeat_due_unit);
                dropdown_marker(f, &self.repeat_due_unit, t);
                unit_popups.push((popup, unit_area, DropdownKind::RepeatDue));
            }
            f.render_widget(
                Paragraph::new("after each firing").style(t.surface_dim()),
                r.rest(),
            );
        }

        if let Some(e) = &self.error {
            f.render_widget(Paragraph::new(e.clone()).style(t.error()), rows[19]);
        }

        let (save, cancel) = button_row(rows[20], " Save ", " Cancel ", t);
        render_button(f, save, " Save ", &mut self.save, t);
        render_button(f, cancel, " Cancel ", &mut self.cancel, t);
        // Open dropdown lists draw over everything else.
        for (popup, unit_area, which) in unit_popups {
            let state = match which {
                DropdownKind::StartPrefill => &mut self.start_prefill_unit,
                DropdownKind::DuePrefill => &mut self.prefill_unit,
                DropdownKind::RepeatStart => &mut self.repeat_start_unit,
                DropdownKind::RepeatDue => &mut self.repeat_due_unit,
            };
            f.render_stateful_widget(popup, unit_area, state);
            dropdown_popup_hover(f, state, t);
        }
        f.render_stateful_widget(repeat_popup, repeat_area, &mut self.repeat_kind);
        dropdown_popup_hover(f, &self.repeat_kind, t);

        let cursor = [
            self.title.screen_cursor(),
            self.description.screen_cursor(),
            self.checklist.screen_cursor(),
            self.start.screen_cursor(),
            self.due.screen_cursor(),
            self.start_prefill_amount.screen_cursor(),
            self.prefill_amount.screen_cursor(),
            self.min_samples.screen_cursor(),
            self.remind_start_hours.screen_cursor(),
            self.remind_hours.screen_cursor(),
            self.repeat_start_amount.screen_cursor(),
            self.repeat_due_amount.screen_cursor(),
            self.dep_search.screen_cursor(),
            self.every_n.screen_cursor(),
            self.day_of_month.screen_cursor(),
            self.fixed_date.screen_cursor(),
            self.at_time.screen_cursor(),
        ]
        .into_iter()
        .flatten()
        .next();
        if let Some(p) = cursor {
            f.set_cursor_position(p);
        }
    }

    fn member_rows(&self, width: usize) -> usize {
        let mut rows = 1;
        let mut x = 0;
        for m in &self.members {
            let cw = m.name.chars().count() + 5;
            if x + cw > width && x > 0 {
                rows += 1;
                x = 0;
            }
            x += cw;
        }
        rows
    }
}

/// Checklist lines as typed. A `[x]` prefix marks an item done, a `[ ]`
/// prefix is optional, blank lines are skipped.
fn parse_checklist(text: &str) -> Vec<ChecklistItem> {
    text.lines()
        .filter_map(|l| {
            let l = l.trim();
            let (done, rest) =
                if let Some(r) = l.strip_prefix("[x]").or_else(|| l.strip_prefix("[X]")) {
                    (true, r)
                } else if let Some(r) = l.strip_prefix("[ ]") {
                    (false, r)
                } else {
                    (false, l)
                };
            let text = rest.trim().to_string();
            (!text.is_empty()).then_some(ChecklistItem { text, done })
        })
        .collect()
}

/// "2026-09-07 14:30" or "2026-09-07" (09:00 assumed), in the user's zone.
fn parse_when(text: &str, tz: Tz, which: &str) -> Result<Option<DateTime<Utc>>, String> {
    if text.is_empty() {
        return Ok(None);
    }
    let naive = NaiveDateTime::parse_from_str(text, "%Y-%m-%d %H:%M")
        .or_else(|_| {
            NaiveDate::parse_from_str(text, "%Y-%m-%d")
                .map(|d| d.and_hms_opt(9, 0, 0).unwrap_or_default())
        })
        .map_err(|_| format!("{which} date must look like 2026-09-07 14:30"))?;
    let local = tz
        .from_local_datetime(&naive)
        .earliest()
        .ok_or_else(|| "that time does not exist in your timezone".to_string())?;
    Ok(Some(local.with_timezone(&Utc)))
}

/// The hours a ticked reminder box carries. Ticking the box and leaving the
/// field blank is unfinished rather than "use the default", so it is an error
/// and not a silent None.
fn reminder_of(
    on: &CheckboxState,
    hours: &TextInputState,
    which: &str,
) -> Result<Option<u32>, String> {
    if !on.checked() {
        return Ok(None);
    }
    match parse_reminder_hours(hours.text())? {
        Some(m) => Ok(Some(m)),
        None => Err(format!(
            "the {which} reminder needs a number of hours, fractions allowed"
        )),
    }
}

/// Lines for the conflict dialog: every field where the form differs from
/// what is on the server now.
pub fn conflict_lines(
    form: &TaskForm,
    current: &Task,
    name_of: &dyn Fn(Uid) -> String,
) -> Vec<String> {
    let mut out = Vec::new();
    let draft = match form.values() {
        Ok(s) => s.draft,
        Err(e) => return vec![format!("your form is not valid yet: {e}")],
    };
    if draft.title != current.title {
        out.push(format!(
            "Title: yours \"{}\", theirs \"{}\"",
            draft.title, current.title
        ));
    }
    if draft.description.trim() != current.description.trim() {
        out.push("Description: both changed".into());
    }
    let show_date = |d: Option<DateTime<Utc>>| {
        d.map(|d| {
            d.with_timezone(&form.tz)
                .format("%Y-%m-%d %H:%M")
                .to_string()
        })
        .unwrap_or_else(|| "none".into())
    };
    if draft.start_at != current.start_at {
        out.push(format!(
            "Start: yours {}, theirs {}",
            show_date(draft.start_at),
            show_date(current.start_at)
        ));
    }
    if draft.due_at != current.due_at {
        out.push(format!(
            "Due: yours {}, theirs {}",
            show_date(draft.due_at),
            show_date(current.due_at)
        ));
    }
    let show_lead = |m: Option<u32>| {
        m.map(|m| format!("{} hours before", reminder_hours_text(m)))
            .unwrap_or_else(|| "the default".into())
    };
    if draft.reminder_start_minutes != current.reminder_start_minutes {
        out.push(format!(
            "Start reminder: yours {}, theirs {}",
            show_lead(draft.reminder_start_minutes),
            show_lead(current.reminder_start_minutes)
        ));
    }
    if draft.reminder_due_minutes != current.reminder_due_minutes {
        out.push(format!(
            "Due reminder: yours {}, theirs {}",
            show_lead(draft.reminder_due_minutes),
            show_lead(current.reminder_due_minutes)
        ));
    }
    let mut a = draft.assignees.clone();
    let mut b = current.assignees.clone();
    a.sort_unstable();
    b.sort_unstable();
    if a != b {
        let names = |v: &[Uid]| v.iter().map(|u| name_of(*u)).collect::<Vec<_>>().join(", ");
        out.push(format!(
            "Assignees: yours [{}], theirs [{}]",
            names(&a),
            names(&b)
        ));
    }
    let mut a = draft.depends_on.clone();
    let mut b = current.depends_on.clone();
    a.sort();
    b.sort();
    if a != b {
        out.push("Dependencies: both changed".into());
    }
    if draft.checklist != current.checklist {
        out.push("Checklist: both changed".into());
    }
    if draft.repeat != current.repeat {
        out.push("Repetition: both changed".into());
    }
    if out.is_empty() {
        out.push("No field differs from the current version, saving again is safe.".into());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyEvent, KeyModifiers};
    use taskologic_core::board::test_support::board_with_members;
    use taskologic_core::task::test_support::task_on;

    fn key(code: KeyCode) -> Event {
        Event::Key(KeyEvent::new(code, KeyModifiers::NONE))
    }

    fn type_str(form: &mut TaskForm, s: &str) {
        for c in s.chars() {
            form.handle(&key(KeyCode::Char(c)));
        }
    }

    fn template(id: i64, name: &str, options: TemplateOptions) -> Template {
        Template {
            id: TemplateId(id),
            board_id: BoardId(1),
            owner_uid: 1,
            name: name.into(),
            draft: TaskDraft {
                title: name.into(),
                ..Default::default()
            },
            options,
        }
    }

    #[test]
    fn checklist_lines_parse_and_round_trip() {
        assert_eq!(parse_checklist(""), vec![]);
        let items = parse_checklist("balloons\n[x] cake\n[ ] candles\n\n  [X] invites  ");
        assert_eq!(
            items,
            vec![
                ChecklistItem {
                    text: "balloons".into(),
                    done: false
                },
                ChecklistItem {
                    text: "cake".into(),
                    done: true
                },
                ChecklistItem {
                    text: "candles".into(),
                    done: false
                },
                ChecklistItem {
                    text: "invites".into(),
                    done: true
                },
            ]
        );
        // Editing a task shows the same markers back.
        let board = board_with_members(1, &[1]);
        let mut task = task_on(&board, 1);
        task.checklist = items;
        let form = TaskForm::edit(&task, chrono_tz::UTC, &|_| None);
        assert_eq!(form.values().unwrap().draft.checklist, task.checklist);
    }

    #[test]
    fn empty_title_is_refused_and_a_valid_form_saves() {
        let mut form = TaskForm::create(BoardId(1), ColumnId(1), chrono_tz::Europe::Berlin);
        assert_eq!(form.handle(&key(KeyCode::F(2))), FormOutcome::Changed);
        assert!(form.error.as_deref().unwrap_or("").contains("title"));
        type_str(&mut form, "Water plants");
        match form.handle(&key(KeyCode::F(2))) {
            FormOutcome::Save(s) => assert_eq!(s.draft.title, "Water plants"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn dates_are_local_and_the_time_is_optional() {
        assert_eq!(parse_when("", chrono_tz::UTC, "due").unwrap(), None);
        let d = parse_when("2026-09-07 09:00", chrono_tz::Europe::Berlin, "due")
            .unwrap()
            .unwrap();
        assert_eq!(d.to_rfc3339(), "2026-09-07T07:00:00+00:00");
        let d = parse_when("2026-09-07", chrono_tz::UTC, "due")
            .unwrap()
            .unwrap();
        assert_eq!(d.to_rfc3339(), "2026-09-07T09:00:00+00:00");
        // The message names the field that is wrong, not just "a date".
        let e = parse_when("tomorrow", chrono_tz::UTC, "start").unwrap_err();
        assert!(e.starts_with("start date must look like"), "{e}");
    }

    #[test]
    fn a_task_due_before_it_starts_is_refused_without_a_round_trip() {
        let mut form = TaskForm::create(BoardId(1), ColumnId(1), chrono_tz::UTC);
        form.title.set_text("Paint the shed".to_string());
        form.start.set_text("2026-09-08 09:00".to_string());
        form.due.set_text("2026-09-07 09:00".to_string());
        assert_eq!(
            form.values().unwrap_err(),
            "the start date cannot be after the due date"
        );
        // Either date on its own is fine.
        form.due.set_text(String::new());
        let draft = form.values().unwrap().draft;
        assert!(draft.start_at.is_some() && draft.due_at.is_none());
    }

    #[test]
    fn edit_prefills_and_round_trips() {
        let board = board_with_members(1, &[1, 2]);
        let mut task = task_on(&board, 1);
        task.title = "Taxes".into();
        task.description = "Q3\nand Q4".into();
        task.assignees = vec![2];
        task.depends_on = vec![TaskId(77)];
        task.due_at = Some(
            DateTime::parse_from_rfc3339("2026-09-07T07:00:00Z")
                .unwrap()
                .with_timezone(&Utc),
        );
        task.repeat = Some(RepeatSpec {
            rule: Repeat::Weekdays {
                days: vec![Weekday::Mon, Weekday::Fri],
            },
            at: NaiveTime::from_hms_opt(8, 30, 0).unwrap(),
            tz: chrono_tz::Europe::Berlin,
            start_rule: None,
            due_rule: None,
        });
        let mut form = TaskForm::edit(&task, chrono_tz::Europe::Berlin, &|_| {
            Some("Buy stamps".into())
        });
        form.set_members(&[
            UserSummary {
                uid: 1,
                username: "alice".into(),
                is_admin: true,
            },
            UserSummary {
                uid: 2,
                username: "bob".into(),
                is_admin: false,
            },
        ]);
        let draft = form.values().unwrap().draft;
        assert_eq!(draft.title, "Taxes");
        assert_eq!(draft.description, "Q3\nand Q4");
        assert_eq!(draft.assignees, vec![2]);
        assert_eq!(draft.depends_on, vec![TaskId(77)]);
        assert_eq!(draft.due_at, task.due_at);
        assert_eq!(draft.repeat, task.repeat);
        assert_eq!(
            form.mode,
            FormMode::Edit {
                task: task.id,
                version: task.version
            }
        );
        assert!(
            conflict_lines(&form, &task, &|u| format!("u{u}"))
                .iter()
                .any(|l| l.contains("No field differs"))
        );
        task.title = "Taxes, urgently".into();
        task.assignees = vec![1];
        let lines = conflict_lines(&form, &task, &|u| format!("u{u}"));
        assert!(lines.iter().any(|l| l.starts_with("Title:")), "{lines:?}");
        assert!(
            lines.iter().any(|l| l.starts_with("Assignees:")),
            "{lines:?}"
        );
    }

    #[test]
    fn template_mode_prefills_and_never_emits_due_or_deps() {
        let tpl = Template {
            id: TemplateId(5),
            board_id: BoardId(1),
            owner_uid: 1,
            name: "Weekly clean".into(),
            draft: TaskDraft {
                title: "Weekly clean".into(),
                description: "kitchen and hallway".into(),
                assignees: vec![2],
                checklist: vec![ChecklistItem {
                    text: "mop".into(),
                    done: true,
                }],
                repeat: Some(RepeatSpec {
                    rule: Repeat::Weekdays {
                        days: vec![Weekday::Mon],
                    },
                    at: NaiveTime::from_hms_opt(8, 0, 0).unwrap(),
                    tz: chrono_tz::UTC,
                    start_rule: None,
                    due_rule: None,
                }),
                ..Default::default()
            },
            options: TemplateOptions::default(),
        };
        let mut form = TaskForm::template_edit(&tpl, chrono_tz::UTC);
        form.set_members(&[
            UserSummary {
                uid: 1,
                username: "alice".into(),
                is_admin: false,
            },
            UserSummary {
                uid: 2,
                username: "bob".into(),
                is_admin: false,
            },
        ]);
        let draft = form.values().unwrap().draft;
        assert_eq!(draft.title, "Weekly clean");
        assert_eq!(draft.assignees, vec![2]);
        assert_eq!(draft.checklist, tpl.draft.checklist);
        assert_eq!(draft.repeat, tpl.draft.repeat);
        assert_eq!(draft.due_at, None);
        assert!(draft.depends_on.is_empty());
        // Using a template starts a create that remembers where it came
        // from, with the same content.
        let form = TaskForm::create_from_template(BoardId(1), ColumnId(1), chrono_tz::UTC, &tpl);
        assert_eq!(
            form.mode,
            FormMode::Create {
                board: BoardId(1),
                column: ColumnId(1),
                from_template: Some(TemplateId(5))
            }
        );
        assert_eq!(
            form.values().unwrap().draft.description,
            "kitchen and hallway"
        );
    }

    #[test]
    fn a_click_past_the_end_of_the_description_does_not_panic() {
        use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        let mut form = TaskForm::create(BoardId(1), ColumnId(1), chrono_tz::UTC);
        let theme = crate::ui::theme::Theme::default();
        let mut term = Terminal::new(TestBackend::new(100, 30)).unwrap();
        term.draw(|f| form.render(f, f.area(), &theme)).unwrap();
        // Click well to the right of the empty first line, then type. This
        // used to leave the cursor past the line end and panic on insert.
        let area = form.description.inner;
        let click = |kind| {
            Event::Mouse(MouseEvent {
                kind,
                column: area.x + 20,
                row: area.y,
                modifiers: KeyModifiers::NONE,
            })
        };
        form.handle(&click(MouseEventKind::Down(MouseButton::Left)));
        form.handle(&click(MouseEventKind::Up(MouseButton::Left)));
        type_str(&mut form, "hi");
        assert!(form.description.text().contains("hi"));
    }

    #[test]
    fn dependency_picker_searches_then_adds() {
        let mut form = TaskForm::create(BoardId(1), ColumnId(1), chrono_tz::UTC);
        // Tab from title: description, checklist, start, its Insert current,
        // due, its Insert current, the two reminder boxes, then (no members
        // yet) the search box.
        for _ in 0..9 {
            form.handle(&key(KeyCode::Tab));
        }
        assert!(form.dep_search.is_focused());
        type_str(&mut form, "soap");
        assert_eq!(
            form.handle(&key(KeyCode::Enter)),
            FormOutcome::SearchDeps("soap".into())
        );
        form.set_dep_hits(vec![SearchHit {
            task_id: TaskId(5),
            board_id: BoardId(1),
            board_name: "Kitchen".into(),
            column_name: "Todo".into(),
            title: "Buy soap".into(),
            archived: false,
        }]);
        assert!(form.dep_hits_list.is_focused());
        form.handle(&key(KeyCode::Enter));
        assert_eq!(form.deps.len(), 1);
        assert!(form.dep_hits.is_empty());
        type_str(&mut form, "");
        assert_eq!(form.handle(&key(KeyCode::Esc)), FormOutcome::Cancel);
    }

    #[test]
    fn insert_current_fills_a_due_date_the_form_reads_back() {
        let mut form = TaskForm::create(BoardId(1), ColumnId(1), chrono_tz::Europe::Berlin);
        type_str(&mut form, "Feed the cat");
        // Tab from title: description, checklist, start, its own button,
        // due, then the button beside it.
        for _ in 0..6 {
            form.handle(&key(KeyCode::Tab));
        }
        assert!(form.insert_now.is_focused());
        form.handle(&key(KeyCode::Enter));
        let due = form.values().unwrap().draft.due_at.expect("a due date");
        assert!((Utc::now() - due).num_minutes().abs() <= 1);

        // The start field has a button of its own that fills only itself.
        let mut form = TaskForm::create(BoardId(1), ColumnId(1), chrono_tz::Europe::Berlin);
        type_str(&mut form, "Feed the cat");
        for _ in 0..4 {
            form.handle(&key(KeyCode::Tab));
        }
        assert!(form.insert_now_start.is_focused());
        form.handle(&key(KeyCode::Enter));
        let draft = form.values().unwrap().draft;
        assert!((Utc::now() - draft.start_at.expect("a start date")).num_minutes().abs() <= 1);
        assert_eq!(draft.due_at, None, "one button fills one field");
    }

    #[test]
    fn a_reminder_override_is_typed_in_hours_and_saved_in_minutes() {
        let mut form = TaskForm::create(BoardId(1), ColumnId(1), chrono_tz::UTC);
        type_str(&mut form, "Take the bins out");
        // Tab from title: description, checklist, start, its Insert current,
        // due, its Insert current, the start reminder box, then the due one.
        for _ in 0..8 {
            form.handle(&key(KeyCode::Tab));
        }
        assert!(form.remind_override.is_focused());
        form.handle(&key(KeyCode::Char(' ')));
        form.handle(&key(KeyCode::Tab));
        assert!(form.remind_hours.is_focused());
        type_str(&mut form, "1.5");
        assert_eq!(form.values().unwrap().draft.reminder_due_minutes, Some(90));
        // Ticked and blank is unfinished, not "use the default".
        form.remind_hours.set_text("");
        assert!(form.values().is_err());

        // The start lead time is its own switch and its own field.
        form.remind_hours.set_text("1.5");
        form.remind_start_override.set_checked(true);
        form.remind_start_hours.set_text("0.5");
        let draft = form.values().unwrap().draft;
        assert_eq!(draft.reminder_start_minutes, Some(30));
        assert_eq!(draft.reminder_due_minutes, Some(90));
    }

    #[test]
    fn a_changed_reminder_shows_up_in_the_conflict_dialog() {
        let board = board_with_members(1, &[1]);
        let mut task = task_on(&board, 1);
        task.reminder_due_minutes = Some(90);
        let form = TaskForm::edit(&task, chrono_tz::UTC, &|_| None);
        assert_eq!(form.values().unwrap().draft.reminder_due_minutes, Some(90));
        task.reminder_due_minutes = None;
        let lines = conflict_lines(&form, &task, &|u| format!("u{u}"));
        assert!(
            lines
                .contains(&"Due reminder: yours 1.5 hours before, theirs the default".to_string()),
            "{lines:?}"
        );
    }

    #[test]
    fn a_template_keeps_its_prefill_rule_and_the_templates_it_needs() {
        let tpl = template(
            1,
            "Order parts",
            TemplateOptions {
                due_prefill: Some(Offset {
                    amount: 2,
                    unit: OffsetUnit::Days,
                }),
                dep_templates: vec![TemplateId(2)],
                ..Default::default()
            },
        );
        let mut form = TaskForm::template_edit(&tpl, chrono_tz::UTC);
        form.set_template_choices(&[
            tpl.clone(),
            template(2, "Empty the van", TemplateOptions::default()),
            template(3, "Book the lift", TemplateOptions::default()),
        ]);
        // A template is never offered as a dependency of itself.
        assert_eq!(form.dep_templates.len(), 2);
        assert_eq!(form.values().unwrap().options, tpl.options);
        // The list only knows how far it can move once it has been drawn, so
        // arrow keys do nothing until the form has been through a render.
        let theme = crate::ui::theme::Theme::default();
        let mut term = ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 30)).unwrap();
        term.draw(|f| form.render(f, f.area(), &theme)).unwrap();
        // Tab from title: description, checklist, the start prefill box, the
        // due prefill box with its amount and unit, the from-average box and
        // the two reminder boxes, then the template list.
        for _ in 0..10 {
            form.handle(&key(KeyCode::Tab));
        }
        assert!(form.dep_templates_list.is_focused());
        form.handle(&key(KeyCode::Down));
        form.handle(&key(KeyCode::Char(' ')));
        assert_eq!(
            form.values().unwrap().options.dep_templates,
            vec![TemplateId(2), TemplateId(3)]
        );
    }

    #[test]
    fn using_a_template_with_a_prefill_rule_fills_the_due_field() {
        let tpl = template(
            4,
            "Weekly order",
            TemplateOptions {
                due_prefill: Some(Offset {
                    amount: 3,
                    unit: OffsetUnit::Hours,
                }),
                dep_templates: Vec::new(),
                ..Default::default()
            },
        );
        let form = TaskForm::create_from_template(
            BoardId(1),
            ColumnId(1),
            chrono_tz::Europe::Berlin,
            &tpl,
        );
        let due = form.values().unwrap().draft.due_at.expect("a due date");
        let ahead = (due - Utc::now()).num_minutes();
        assert!((179..=180).contains(&ahead), "{ahead} minutes ahead");
        // Without the rule the due field stays empty.
        let plain = template(5, "Ad hoc", TemplateOptions::default());
        let form = TaskForm::create_from_template(BoardId(1), ColumnId(1), chrono_tz::UTC, &plain);
        assert_eq!(form.values().unwrap().draft.due_at, None);
    }
}
