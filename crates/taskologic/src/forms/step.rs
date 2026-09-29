//! One step of a program, edited in a window of its own over the program
//! form. Everything a step says about its task is here: what it looks
//! like, who gets it, what makes it appear, when it starts, its time limit,
//! whether the root waits for it, and what it prints. The root step shows
//! only the fields that mean anything for it.

use crossterm::event::{Event, KeyCode, KeyEventKind, MouseButton, MouseEventKind};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::widgets::{Clear, ListItem, Paragraph};
use taskologic_core::ids::Uid;
use taskologic_core::offset::{MAX_OFFSET_AMOUNT, Offset, OffsetUnit};
use taskologic_core::print::PrintRule;
use taskologic_core::program::{Assign, Question, QuestionKind, ROOT_KEY, StartRule, Step, Trigger};
use taskologic_core::task::{ChecklistItem, validate_title};
use taskologic_core::user::UserSummary;

use super::{Row, button_h, button_row, check_w, frame_block, label, popup, split_label};
use crate::ui::adapter::{
    ButtonOutcome, ButtonState, CheckboxState, ChoiceState, Focus, FocusBuilder, HandleEvent,
    HasFocus, HasScreenCursor, ListState, Navigation, Outcome, Regular, TextAreaState,
    TextInputState, checkbox_at, dropdown, dropdown_marker, dropdown_popup_hover, field, list,
    render_button, text_area, text_area_event,
};
use crate::ui::theme::Theme;

#[derive(Debug, PartialEq)]
pub enum StepOutcome {
    Changed,
    Cancel,
    /// Back to the program form with this step.
    Save(Box<Step>),
    /// Open the print rules window with these rules; it hands them back
    /// through [`StepForm::set_print_rules`]. `sheets` says whether the
    /// step fans out, which is when a group sheet is worth offering.
    EditPrinting { rules: Vec<PrintRule>, sheets: bool },
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum AssignChoice {
    #[default]
    Starter,
    Nobody,
    Pick,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum QuestionChoice {
    #[default]
    None,
    YesNo,
    Choice,
}

const QUESTION_ITEMS: [(QuestionChoice, &str); 3] = [
    (QuestionChoice::None, "asks nothing"),
    (QuestionChoice::YesNo, "asks yes or no"),
    (QuestionChoice::Choice, "asks to pick one"),
];

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum StartChoice {
    #[default]
    Manual,
    WhenRootStarts,
    After,
    DaysLater,
}

const ASSIGN_ITEMS: [(AssignChoice, &str); 3] = [
    (AssignChoice::Starter, "whoever starts it"),
    (AssignChoice::Nobody, "nobody"),
    (AssignChoice::Pick, "these people"),
];
const START_ITEMS: [(StartChoice, &str); 4] = [
    (StartChoice::Manual, "when somebody starts it"),
    (StartChoice::WhenRootStarts, "when the program starts"),
    (StartChoice::After, "some time after it appears"),
    (StartChoice::DaysLater, "days later, at a time"),
];

struct Member {
    uid: Uid,
    name: String,
    check: CheckboxState,
}

pub struct StepForm {
    /// Which step of the program this is. None is a new one.
    pub index: Option<usize>,
    is_root: bool,
    /// The other steps' keys, which the trigger and wait lists offer.
    others: Vec<String>,
    key: TextInputState,
    title: TextInputState,
    description: TextAreaState,
    checklist: TextAreaState,
    assign: ChoiceState<AssignChoice>,
    members: Vec<Member>,
    preselect: Vec<Uid>,
    with_root: CheckboxState,
    root_started: CheckboxState,
    once: CheckboxState,
    /// Other steps whose finishing makes this one, ticked or not.
    after: Vec<(String, bool)>,
    after_list: ListState,
    /// Other steps this one also waits for.
    also: Vec<(String, bool)>,
    also_list: ListState,
    start: ChoiceState<StartChoice>,
    start_amount: TextInputState,
    start_unit: ChoiceState<OffsetUnit>,
    start_days: TextInputState,
    start_time: TextInputState,
    auto_start: CheckboxState,
    limit: CheckboxState,
    limit_amount: TextInputState,
    limit_unit: ChoiceState<OffsetUnit>,
    counts: CheckboxState,
    q_kind: ChoiceState<QuestionChoice>,
    q_text: TextInputState,
    /// One answer per line, for a choice question.
    q_answers: TextAreaState,
    q_default: TextInputState,
    /// The fan-out entries, comma separated, or empty for one task.
    fan_out: TextInputState,
    print_rules: Vec<PrintRule>,
    print_btn: ButtonState,
    save: ButtonState,
    cancel: ButtonState,
    pub error: Option<String>,
}

impl StepForm {
    /// Edit `step`, the `index`th of its program, alongside the `others`.
    pub fn new(index: Option<usize>, step: &Step, others: Vec<String>) -> Self {
        let is_root = step.is_root();
        let mut key = TextInputState::named("key");
        key.set_text(step.key.clone());
        let mut title = TextInputState::named("title");
        title.set_text(step.title.clone());
        let mut description = TextAreaState::named("description");
        description.set_text(&step.description);
        let mut checklist = TextAreaState::named("checklist");
        if !step.checklist.is_empty() {
            let lines: Vec<String> = step
                .checklist
                .iter()
                .map(|c| format!("{} {}", if c.done { "[x]" } else { "[ ]" }, c.text))
                .collect();
            checklist.set_text(lines.join("\n"));
        }
        let mut assign = ChoiceState::named("assign");
        let preselect = match &step.assign {
            Assign::Starter => {
                assign.set_value(AssignChoice::Starter);
                Vec::new()
            }
            Assign::Nobody => {
                assign.set_value(AssignChoice::Nobody);
                Vec::new()
            }
            Assign::Users(uids) => {
                assign.set_value(AssignChoice::Pick);
                uids.clone()
            }
        };
        let check = |name: &str, on: bool| {
            let mut c = CheckboxState::named(name);
            c.set_checked(on);
            c
        };
        let after: Vec<(String, bool)> = others
            .iter()
            .map(|k| {
                let on = step
                    .created
                    .iter()
                    .any(|t| matches!(t, Trigger::Finished { step } if step == k));
                (k.clone(), on)
            })
            .collect();
        let also: Vec<(String, bool)> = others
            .iter()
            .map(|k| (k.clone(), step.also_after.contains(k)))
            .collect();
        let mut start = ChoiceState::named("start");
        let mut start_amount = TextInputState::named("start_amount");
        start_amount.set_text("0");
        let mut start_unit = ChoiceState::named("start_unit");
        start_unit.set_value(OffsetUnit::Minutes);
        let mut start_days = TextInputState::named("start_days");
        start_days.set_text("1");
        let mut start_time = TextInputState::named("start_time");
        start_time.set_text("09:00");
        match step.start {
            StartRule::Manual => {
                start.set_value(StartChoice::Manual);
            }
            StartRule::WhenRootStarts => {
                start.set_value(StartChoice::WhenRootStarts);
            }
            StartRule::AfterTrigger(o) => {
                start.set_value(StartChoice::After);
                start_amount.set_text(o.amount.to_string());
                start_unit.set_value(o.unit);
            }
            StartRule::DaysLaterAt { days, at } => {
                start.set_value(StartChoice::DaysLater);
                start_days.set_text(days.to_string());
                start_time.set_text(at.format("%H:%M").to_string());
            }
        }
        let mut limit_amount = TextInputState::named("limit_amount");
        limit_amount.set_text("30");
        let mut limit_unit = ChoiceState::named("limit_unit");
        limit_unit.set_value(OffsetUnit::Minutes);
        if let Some(o) = step.time_limit {
            limit_amount.set_text(o.amount.to_string());
            limit_unit.set_value(o.unit);
        }
        let mut after_list = ListState::named("after");
        after_list.select((!after.is_empty()).then_some(0));
        let mut also_list = ListState::named("also");
        also_list.select((!also.is_empty()).then_some(0));
        let mut fan_out = TextInputState::named("fan_out");
        if let Some(entries) = &step.fan_out {
            fan_out.set_text(entries.join(", "));
        }
        let mut q_kind = ChoiceState::named("q_kind");
        let mut q_text = TextInputState::named("q_text");
        let mut q_answers = TextAreaState::named("q_answers");
        let mut q_default = TextInputState::named("q_default");
        match &step.question {
            None => {
                q_kind.set_value(QuestionChoice::None);
            }
            Some(q) => {
                q_text.set_text(q.text.clone());
                q_default.set_text(q.default.clone().unwrap_or_default());
                match &q.kind {
                    QuestionKind::YesNo => {
                        q_kind.set_value(QuestionChoice::YesNo);
                    }
                    QuestionKind::Choice(v) => {
                        q_kind.set_value(QuestionChoice::Choice);
                        q_answers.set_text(v.join("\n"));
                    }
                }
            }
        }
        // The root's key is fixed, so the first thing to type is its title.
        if is_root {
            title.focus().set(true);
        } else {
            key.focus().set(true);
        }
        Self {
            index,
            is_root,
            others,
            key,
            title,
            description,
            checklist,
            assign,
            members: Vec::new(),
            preselect,
            with_root: check("with_root", step.created.contains(&Trigger::WithRoot)),
            root_started: check("root_started", step.created.contains(&Trigger::RootStarted)),
            once: check("once", step.once),
            after,
            after_list,
            also,
            also_list,
            start,
            start_amount,
            start_unit,
            start_days,
            start_time,
            auto_start: check("auto_start", step.auto_start),
            limit: check("limit", step.time_limit.is_some()),
            limit_amount,
            limit_unit,
            counts: check("counts", step.counts_toward_root),
            q_kind,
            q_text,
            q_answers,
            q_default,
            fan_out,
            print_rules: step.print.clone(),
            print_btn: ButtonState::new(),
            save: ButtonState::new(),
            cancel: ButtonState::new(),
            error: None,
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

    /// The print rules window closed with these.
    pub fn set_print_rules(&mut self, rules: Vec<PrintRule>) {
        self.print_rules = rules;
    }

    fn picking(&self) -> bool {
        self.assign.value() == AssignChoice::Pick
    }

    fn focus(&self) -> Focus {
        let mut b = FocusBuilder::new(None);
        if !self.is_root {
            b.widget(&self.key);
        }
        b.widget(&self.title);
        b.widget_navigate(&self.description, Navigation::Regular);
        b.widget_navigate(&self.checklist, Navigation::Regular);
        b.widget(&self.assign);
        if self.picking() {
            for m in &self.members {
                b.widget(&m.check);
            }
        }
        if !self.is_root {
            b.widget(&self.with_root)
                .widget(&self.root_started)
                .widget(&self.once)
                .widget(&self.after_list)
                .widget(&self.also_list)
                .widget(&self.start);
            match self.start.value() {
                StartChoice::After => {
                    b.widget(&self.start_amount).widget(&self.start_unit);
                }
                StartChoice::DaysLater => {
                    b.widget(&self.start_days).widget(&self.start_time);
                }
                _ => {}
            }
            if self.start.value() != StartChoice::Manual {
                b.widget(&self.auto_start);
            }
            b.widget(&self.limit);
            if self.limit.checked() {
                b.widget(&self.limit_amount).widget(&self.limit_unit);
            }
            b.widget(&self.counts).widget(&self.q_kind);
            if self.q_kind.value() != QuestionChoice::None {
                b.widget(&self.q_text);
                if self.q_kind.value() == QuestionChoice::Choice {
                    b.widget_navigate(&self.q_answers, Navigation::Regular);
                }
                b.widget(&self.q_default);
            }
            b.widget(&self.fan_out);
        }
        b.widget(&self.print_btn)
            .widget(&self.save)
            .widget(&self.cancel);
        b.build()
    }

    fn toggle(items: &mut [(String, bool)], i: usize) {
        if let Some((_, on)) = items.get_mut(i) {
            *on = !*on;
        }
    }

    pub fn handle(&mut self, ev: &Event) -> StepOutcome {
        let key = match ev {
            Event::Key(k) if k.kind != KeyEventKind::Release => Some(k.code),
            _ => None,
        };
        let popup_open = self.assign.is_popup_active()
            || self.start.is_popup_active()
            || self.start_unit.is_popup_active()
            || self.limit_unit.is_popup_active()
            || self.q_kind.is_popup_active();
        match key {
            Some(KeyCode::Esc) if !popup_open => return StepOutcome::Cancel,
            Some(KeyCode::F(2)) => return self.try_save(),
            _ => {}
        }
        let mut focus = self.focus();
        if focus.handle(ev, Regular) == Outcome::Changed && key.is_some() {
            return StepOutcome::Changed;
        }
        if self.save.handle(ev, Regular) == ButtonOutcome::Pressed {
            return self.try_save();
        }
        if self.cancel.handle(ev, Regular) == ButtonOutcome::Pressed {
            return StepOutcome::Cancel;
        }
        if self.print_btn.handle(ev, Regular) == ButtonOutcome::Pressed {
            return StepOutcome::EditPrinting {
                rules: self.print_rules.clone(),
                sheets: !self.is_root && !self.fan_out.text().trim().is_empty(),
            };
        }
        // The two step lists are ticked, by Space, Enter or a click.
        let tick = matches!(key, Some(KeyCode::Char(' ') | KeyCode::Enter));
        if tick && self.after_list.is_focused() {
            if let Some(i) = self.after_list.selected() {
                Self::toggle(&mut self.after, i);
            }
            return StepOutcome::Changed;
        }
        if tick && self.also_list.is_focused() {
            if let Some(i) = self.also_list.selected() {
                Self::toggle(&mut self.also, i);
            }
            return StepOutcome::Changed;
        }
        if let Event::Mouse(m) = ev
            && matches!(m.kind, MouseEventKind::Down(MouseButton::Left))
        {
            let at = Position::new(m.column, m.row);
            if let Some(i) = self.after_list.row_areas.iter().position(|r| r.contains(at)) {
                Self::toggle(&mut self.after, i);
                return StepOutcome::Changed;
            }
            if let Some(i) = self.also_list.row_areas.iter().position(|r| r.contains(at)) {
                Self::toggle(&mut self.also, i);
                return StepOutcome::Changed;
            }
        }
        if !self.is_root {
            self.key.handle(ev, Regular);
        }
        self.title.handle(ev, Regular);
        text_area_event(&mut self.description, ev);
        text_area_event(&mut self.checklist, ev);
        self.assign.handle(ev, Regular);
        if self.picking() {
            for m in &mut self.members {
                m.check.handle(ev, Regular);
            }
        }
        if !self.is_root {
            self.with_root.handle(ev, Regular);
            self.root_started.handle(ev, Regular);
            self.once.handle(ev, Regular);
            self.after_list.handle(ev, Regular);
            self.also_list.handle(ev, Regular);
            self.start.handle(ev, Regular);
            match self.start.value() {
                StartChoice::After => {
                    self.start_amount.handle(ev, Regular);
                    self.start_unit.handle(ev, Regular);
                }
                StartChoice::DaysLater => {
                    self.start_days.handle(ev, Regular);
                    self.start_time.handle(ev, Regular);
                }
                _ => {}
            }
            if self.start.value() != StartChoice::Manual {
                self.auto_start.handle(ev, Regular);
            }
            self.limit.handle(ev, Regular);
            if self.limit.checked() {
                self.limit_amount.handle(ev, Regular);
                self.limit_unit.handle(ev, Regular);
            }
            self.counts.handle(ev, Regular);
            self.q_kind.handle(ev, Regular);
            if self.q_kind.value() != QuestionChoice::None {
                self.q_text.handle(ev, Regular);
                if self.q_kind.value() == QuestionChoice::Choice {
                    text_area_event(&mut self.q_answers, ev);
                }
                self.q_default.handle(ev, Regular);
            }
            self.fan_out.handle(ev, Regular);
        }
        StepOutcome::Changed
    }

    fn try_save(&mut self) -> StepOutcome {
        match self.values() {
            Ok(step) => {
                self.error = None;
                StepOutcome::Save(Box::new(step))
            }
            Err(e) => {
                self.error = Some(e);
                StepOutcome::Changed
            }
        }
    }

    fn offset_of(amount: &TextInputState, unit: &ChoiceState<OffsetUnit>, what: &str) -> Result<Offset, String> {
        let amount: u32 = amount
            .text()
            .trim()
            .parse()
            .map_err(|_| format!("{what} needs a whole number"))?;
        if amount > MAX_OFFSET_AMOUNT {
            return Err(format!("{what} would land off the calendar"));
        }
        Ok(Offset {
            amount,
            unit: unit.value(),
        })
    }

    /// The step as the fields stand, or the first thing wrong with it.
    pub fn values(&self) -> Result<Step, String> {
        let key = if self.is_root {
            ROOT_KEY.to_string()
        } else {
            let k = self.key.text().trim().to_string();
            if k.is_empty() {
                return Err("the step needs a key, like 1 or 1.1 or trash".into());
            }
            if k == ROOT_KEY {
                return Err(format!("{ROOT_KEY:?} is the root's key"));
            }
            if self.others.contains(&k) {
                return Err(format!("another step already has the key {k:?}"));
            }
            k
        };
        let title = self.title.text().trim().to_string();
        validate_title(&title).map_err(|e| e.to_string())?;
        let assign = match self.assign.value() {
            AssignChoice::Starter => Assign::Starter,
            AssignChoice::Nobody => Assign::Nobody,
            AssignChoice::Pick => {
                let uids: Vec<Uid> = self
                    .members
                    .iter()
                    .filter(|m| m.check.checked())
                    .map(|m| m.uid)
                    .collect();
                if uids.is_empty() {
                    return Err("tick somebody to assign the step to, or pick nobody".into());
                }
                Assign::Users(uids)
            }
        };
        let mut created = Vec::new();
        let (mut also_after, mut start, mut time_limit, mut once, mut counts) =
            (Vec::new(), StartRule::Manual, None, false, true);
        let mut question = None;
        let mut fan_out = None;
        let mut auto_start = false;
        if !self.is_root {
            let entries: Vec<String> = self
                .fan_out
                .text()
                .split(',')
                .map(str::trim)
                .filter(|e| !e.is_empty())
                .map(String::from)
                .collect();
            if !entries.is_empty() {
                fan_out = Some(entries);
            }
            if self.q_kind.value() != QuestionChoice::None {
                let kind = match self.q_kind.value() {
                    QuestionChoice::Choice => QuestionKind::Choice(
                        self.q_answers
                            .text()
                            .lines()
                            .map(str::trim)
                            .filter(|l| !l.is_empty())
                            .map(String::from)
                            .collect(),
                    ),
                    _ => QuestionKind::YesNo,
                };
                let default = self.q_default.text().trim().to_string();
                let q = Question {
                    text: self.q_text.text().trim().to_string(),
                    kind,
                    default: (!default.is_empty()).then_some(default),
                };
                q.validate().map_err(|e| e.to_string())?;
                question = Some(q);
            }
            if self.with_root.checked() {
                created.push(Trigger::WithRoot);
            }
            if self.root_started.checked() {
                created.push(Trigger::RootStarted);
            }
            for (k, on) in &self.after {
                if *on {
                    created.push(Trigger::Finished { step: k.clone() });
                }
            }
            if created.is_empty() {
                return Err("tick at least one thing that makes this step appear".into());
            }
            also_after = self
                .also
                .iter()
                .filter(|(_, on)| *on)
                .map(|(k, _)| k.clone())
                .collect();
            start = match self.start.value() {
                StartChoice::Manual => StartRule::Manual,
                StartChoice::WhenRootStarts => StartRule::WhenRootStarts,
                StartChoice::After => StartRule::AfterTrigger(Self::offset_of(
                    &self.start_amount,
                    &self.start_unit,
                    "the start delay",
                )?),
                StartChoice::DaysLater => StartRule::DaysLaterAt {
                    days: self
                        .start_days
                        .text()
                        .trim()
                        .parse()
                        .map_err(|_| "the number of days must be a whole number".to_string())?,
                    at: chrono::NaiveTime::parse_from_str(self.start_time.text().trim(), "%H:%M")
                        .map_err(|_| "the start time must look like 09:00".to_string())?,
                },
            };
            time_limit = if self.limit.checked() {
                Some(Self::offset_of(
                    &self.limit_amount,
                    &self.limit_unit,
                    "the time limit",
                )?)
            } else {
                None
            };
            auto_start = start != StartRule::Manual && self.auto_start.checked();
            once = self.once.checked();
            counts = self.counts.checked();
        }
        Ok(Step {
            key,
            title,
            description: self.description.text(),
            assign,
            checklist: parse_checklist(&self.checklist.text()),
            created,
            once,
            also_after,
            start,
            auto_start,
            time_limit,
            counts_toward_root: counts,
            question,
            print: self.print_rules.clone(),
            fan_out,
        })
    }

    pub fn render(&mut self, f: &mut Frame, area: Rect, t: &Theme) {
        let bh = button_h(t);
        let pad = if t.touch { 2 } else { 0 };
        let p = popup(area, 78, 31 + bh);
        f.render_widget(Clear, p);
        let title = if self.is_root {
            " The root step "
        } else if self.index.is_some() {
            " Edit step "
        } else {
            " New step "
        };
        let block = frame_block(title, " Tab moves   F2 saves the step   Esc cancels ", t);
        let inner = block.inner(p);
        f.render_widget(block, p);
        let lw = 10u16;
        let member_rows = self.member_rows(inner.width.saturating_sub(lw) as usize).max(1) as u16;
        let gap = Constraint::Length(1);
        let rows = Layout::vertical([
            Constraint::Length(1), // 0 key and title
            gap,
            Constraint::Length(3), // 2 description
            gap,
            Constraint::Length(2), // 4 checklist
            gap,
            Constraint::Length(1),           // 6 assign
            Constraint::Length(member_rows), // 7 members
            gap,
            Constraint::Length(1), // 9 appears
            Constraint::Length(1), // 10 list headings
            Constraint::Length(3), // 11 lists
            gap,
            Constraint::Length(1), // 13 starts
            Constraint::Length(1), // 14 limit and counts
            Constraint::Length(1), // 15 asks
            Constraint::Length(2), // 16 answers and default
            Constraint::Length(1), // 17 fan out
            Constraint::Min(1),    // 18 error
            Constraint::Length(bh), // 19 buttons
        ])
        .split(inner);

        let (l, w) = split_label(rows[0], lw);
        label(f, l, "Key", t);
        let mut r = Row::new(w);
        if self.is_root {
            f.render_widget(
                Paragraph::new("root, the step that stands for the whole program")
                    .style(t.surface_dim()),
                r.rest(),
            );
        } else {
            f.render_stateful_widget(field(t), r.take(10), &mut self.key);
            f.render_widget(Paragraph::new("Title").style(t.surface_dim()), r.text("Title"));
            f.render_stateful_widget(field(t), r.rest(), &mut self.title);
        }
        if self.is_root {
            // The root has no key field, so its title takes the description's
            // first row instead of squeezing in beside the note.
        }
        let (l, w) = split_label(rows[2], lw);
        if self.is_root {
            label(f, l, "Title", t);
            let [tw, dw] = Layout::vertical([Constraint::Length(1), Constraint::Min(1)]).areas(w);
            f.render_stateful_widget(field(t), tw, &mut self.title);
            f.render_stateful_widget(text_area(t), dw, &mut self.description);
        } else {
            label(f, l, "Describe", t);
            f.render_stateful_widget(text_area(t), w, &mut self.description);
        }

        let (l, w) = split_label(rows[4], lw);
        label(f, l, "Checklist", t);
        let [w, hint] = Layout::horizontal([Constraint::Percentage(60), Constraint::Min(1)]).areas(w);
        f.render_stateful_widget(text_area(t), w, &mut self.checklist);
        f.render_widget(
            Paragraph::new(" one item per line").style(t.surface_dim()),
            hint,
        );

        let (l, w) = split_label(rows[6], lw);
        label(f, l, "Assign to", t);
        let mut r = Row::new(w);
        let assign_area = r.take(20);
        let (assign_w, assign_popup) = dropdown(ASSIGN_ITEMS, assign_area, t);
        f.render_stateful_widget(assign_w, assign_area, &mut self.assign);
        dropdown_marker(f, &self.assign, t);
        let (_, w) = split_label(rows[7], lw);
        if self.picking() {
            if self.members.is_empty() {
                f.render_widget(
                    Paragraph::new("loading members...").style(t.surface_dim()),
                    w,
                );
            }
            let mut x = w.x;
            let mut y = w.y;
            for m in &mut self.members {
                let cw = check_w(&m.name) + 1;
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
        }

        let mut popups: Vec<(usize, Rect)> = Vec::new();
        let mut start_popup = None;
        let mut q_popup = None;
        let mut unit_popups = Vec::new();
        if !self.is_root {
            let (l, w) = split_label(rows[9], lw);
            label(f, l, "Appears", t);
            let mut r = Row::new(w);
            let cb = r.take(check_w("with the program"));
            f.render_stateful_widget(
                checkbox_at("with the program".into(), cb, t),
                cb,
                &mut self.with_root,
            );
            let cb = r.take(check_w("when it starts"));
            f.render_stateful_widget(
                checkbox_at("when it starts".into(), cb, t),
                cb,
                &mut self.root_started,
            );
            let cb = r.take(check_w("only once"));
            f.render_stateful_widget(checkbox_at("only once".into(), cb, t), cb, &mut self.once);

            let (_, w) = split_label(rows[10], lw);
            let [lh, rh] =
                Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)]).areas(w);
            f.render_widget(
                Paragraph::new("after these finish (Space ticks)").style(t.surface_dim()),
                lh,
            );
            f.render_widget(
                Paragraph::new("also waits for").style(t.surface_dim()),
                rh,
            );
            let (_, w) = split_label(rows[11], lw);
            let [la, ra] =
                Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)]).areas(w);
            let items = |v: &[(String, bool)]| -> Vec<ListItem> {
                if v.is_empty() {
                    vec![ListItem::new("no other steps yet").style(t.surface_dim())]
                } else {
                    v.iter()
                        .map(|(k, on)| ListItem::new(format!("{} {k}", if *on { "[x]" } else { "[ ]" })))
                        .collect()
                }
            };
            f.render_stateful_widget(list(items(&self.after), t), la, &mut self.after_list);
            f.render_stateful_widget(list(items(&self.also), t), ra, &mut self.also_list);

            let (l, w) = split_label(rows[13], lw);
            label(f, l, "Starts", t);
            let mut r = Row::new(w);
            let start_area = r.take(28);
            let (start_w, sp) = dropdown(START_ITEMS, start_area, t);
            f.render_stateful_widget(start_w, start_area, &mut self.start);
            dropdown_marker(f, &self.start, t);
            start_popup = Some((sp, start_area));
            let units = || OffsetUnit::ALL.map(|u| (u, u.label()));
            match self.start.value() {
                StartChoice::After => {
                    f.render_stateful_widget(field(t), r.take(6), &mut self.start_amount);
                    let ua = r.take(11);
                    let (uw, up) = dropdown(units(), ua, t);
                    f.render_stateful_widget(uw, ua, &mut self.start_unit);
                    dropdown_marker(f, &self.start_unit, t);
                    unit_popups.push((0, ua, up));
                }
                StartChoice::DaysLater => {
                    f.render_stateful_widget(field(t), r.take(4), &mut self.start_days);
                    f.render_widget(Paragraph::new("days at").style(t.surface_dim()), r.text("days at"));
                    // Read in the zone of whoever starts the program.
                    f.render_stateful_widget(field(t), r.take(6), &mut self.start_time);
                }
                _ => {}
            }
            if self.start.value() != StartChoice::Manual {
                let cb = r.take(check_w("by itself"));
                f.render_stateful_widget(
                    checkbox_at("by itself".into(), cb, t),
                    cb,
                    &mut self.auto_start,
                );
            }

            let (l, w) = split_label(rows[14], lw);
            label(f, l, "Limit", t);
            let mut r = Row::new(w);
            // Everything on this row has to fit in the 66 columns beside the
            // label with the limit on, so the words are short ones.
            let cb = r.take(check_w("time limit"));
            f.render_stateful_widget(checkbox_at("time limit".into(), cb, t), cb, &mut self.limit);
            if self.limit.checked() {
                f.render_stateful_widget(field(t), r.take(5), &mut self.limit_amount);
                let ua = r.take(10);
                let (uw, up) = dropdown(units(), ua, t);
                f.render_stateful_widget(uw, ua, &mut self.limit_unit);
                dropdown_marker(f, &self.limit_unit, t);
                unit_popups.push((1, ua, up));
                f.render_widget(
                    Paragraph::new("from start").style(t.surface_dim()),
                    r.text("from start"),
                );
            }
            let cb = r.take(check_w("root waits"));
            f.render_stateful_widget(
                checkbox_at("root waits".into(), cb, t),
                cb,
                &mut self.counts,
            );
            popups.push((0, rows[9]));

            let (l, w) = split_label(rows[15], lw);
            label(f, l, "When done", t);
            let mut r = Row::new(w);
            let qa = r.take(18);
            let (qw, qp) = dropdown(QUESTION_ITEMS, qa, t);
            f.render_stateful_widget(qw, qa, &mut self.q_kind);
            dropdown_marker(f, &self.q_kind, t);
            q_popup = Some((qp, qa));
            if self.q_kind.value() != QuestionChoice::None {
                f.render_stateful_widget(field(t), r.rest(), &mut self.q_text);
            } else {
                f.render_widget(
                    Paragraph::new("a question other steps can wait for the answer to")
                        .style(t.surface_dim()),
                    r.rest(),
                );
            }
            if self.q_kind.value() != QuestionChoice::None {
                let (_, w) = split_label(rows[16], lw);
                let [answers, default] =
                    Layout::horizontal([Constraint::Percentage(55), Constraint::Min(1)]).areas(w);
                if self.q_kind.value() == QuestionChoice::Choice {
                    f.render_stateful_widget(text_area(t), answers, &mut self.q_answers);
                } else {
                    f.render_widget(
                        Paragraph::new("answers: yes and no").style(t.surface_dim()),
                        answers,
                    );
                }
                let mut r = Row::new(Rect::new(default.x + 1, default.y, default.width.saturating_sub(1), 1));
                f.render_widget(Paragraph::new("default").style(t.surface_dim()), r.text("default"));
                f.render_stateful_widget(field(t), r.rest(), &mut self.q_default);
                f.render_widget(
                    Paragraph::new(" used when finished without answering").style(t.surface_dim()),
                    Rect::new(default.x, default.y + 1, default.width, 1),
                );
            }

            let (l, w) = split_label(rows[17], lw);
            label(f, l, "Fan out", t);
            let [fw, hint] =
                Layout::horizontal([Constraint::Percentage(50), Constraint::Min(1)]).areas(w);
            f.render_stateful_widget(field(t), fw, &mut self.fan_out);
            f.render_widget(
                Paragraph::new(" one task per entry, comma separated; {param} in the title")
                    .style(t.surface_dim()),
                hint,
            );
        }

        if let Some(e) = &self.error {
            f.render_widget(Paragraph::new(e.clone()).style(t.error()), rows[18]);
        }
        let printing = format!(" Printing ({}) ", self.print_rules.len());
        let pb = Rect::new(
            rows[19].x,
            rows[19].y,
            (super::button_w(&printing) + pad).min(rows[19].width),
            rows[19].height,
        );
        render_button(f, pb, &printing, &mut self.print_btn, t);
        let (save, cancel) = button_row(rows[19], " Save step ", " Cancel ", t);
        render_button(f, save, " Save step ", &mut self.save, t);
        render_button(f, cancel, " Cancel ", &mut self.cancel, t);

        // Open dropdown lists draw over everything else.
        for (which, ua, up) in unit_popups {
            let state = if which == 0 {
                &mut self.start_unit
            } else {
                &mut self.limit_unit
            };
            f.render_stateful_widget(up, ua, state);
            dropdown_popup_hover(f, state, t);
        }
        if let Some((qp, qa)) = q_popup {
            f.render_stateful_widget(qp, qa, &mut self.q_kind);
            dropdown_popup_hover(f, &self.q_kind, t);
        }
        if let Some((sp, sa)) = start_popup {
            f.render_stateful_widget(sp, sa, &mut self.start);
            dropdown_popup_hover(f, &self.start, t);
        }
        f.render_stateful_widget(assign_popup, assign_area, &mut self.assign);
        dropdown_popup_hover(f, &self.assign, t);

        let cursor = [
            self.key.screen_cursor(),
            self.title.screen_cursor(),
            self.description.screen_cursor(),
            self.checklist.screen_cursor(),
            self.start_amount.screen_cursor(),
            self.start_days.screen_cursor(),
            self.start_time.screen_cursor(),
            self.limit_amount.screen_cursor(),
            self.q_text.screen_cursor(),
            self.q_answers.screen_cursor(),
            self.q_default.screen_cursor(),
            self.fan_out.screen_cursor(),
        ]
        .into_iter()
        .flatten()
        .next();
        if let Some(pos) = cursor {
            f.set_cursor_position(pos);
        }
    }

    fn member_rows(&self, width: usize) -> usize {
        if !self.picking() {
            return 0;
        }
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

/// Checklist lines as typed, the way the task form reads them.
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

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyEvent, KeyModifiers};

    fn key(code: KeyCode) -> Event {
        Event::Key(KeyEvent::new(code, KeyModifiers::NONE))
    }

    #[test]
    fn a_step_round_trips_through_the_form() {
        let step = Step {
            key: "2".into(),
            title: "Trash run".into(),
            description: "every bin".into(),
            assign: Assign::Users(vec![2]),
            checklist: vec![ChecklistItem {
                text: "bags".into(),
                done: false,
            }],
            created: vec![Trigger::Finished { step: "1".into() }, Trigger::RootStarted],
            once: true,
            also_after: vec!["1b".into()],
            start: StartRule::AfterTrigger(Offset {
                amount: 5,
                unit: OffsetUnit::Minutes,
            }),
            time_limit: Some(Offset {
                amount: 30,
                unit: OffsetUnit::Minutes,
            }),
            counts_toward_root: false,
            question: None,
            print: Vec::new(),
            fan_out: Some(vec!["bedroom".into(), "kitchen".into()]),
            auto_start: true,
        };
        let mut form = StepForm::new(Some(2), &step, vec!["1".into(), "1b".into(), "3".into()]);
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
        let back = form.values().unwrap();
        assert_eq!(back.key, "2");
        assert_eq!(back.assign, Assign::Users(vec![2]));
        assert_eq!(back.created, vec![Trigger::RootStarted, Trigger::Finished { step: "1".into() }]);
        assert_eq!(back.also_after, vec!["1b".to_string()]);
        assert_eq!(back.start, step.start);
        assert_eq!(back.time_limit, step.time_limit);
        assert!(back.once && !back.counts_toward_root);
        assert_eq!(back.checklist, step.checklist);
        assert_eq!(back.fan_out, step.fan_out, "comma separated in, a list out");
        assert!(back.auto_start);
        form.fan_out.set_text(" ".to_string());
        assert_eq!(form.values().unwrap().fan_out, None);
    }

    #[test]
    fn a_step_that_nothing_makes_or_with_a_taken_key_is_refused_here() {
        let mut form = StepForm::new(None, &Step::default(), vec!["1".into()]);
        assert!(form.values().unwrap_err().contains("needs a key"));
        form.key.set_text("1".to_string());
        form.title.set_text("Again".to_string());
        assert!(form.values().unwrap_err().contains("already has the key"));
        form.key.set_text("2".to_string());
        assert!(form.values().unwrap_err().contains("makes this step appear"));
        form.with_root.set_checked(true);
        assert_eq!(form.values().unwrap().created, vec![Trigger::WithRoot]);
        // Picking people and ticking none is unfinished, not "nobody".
        form.assign.set_value(AssignChoice::Pick);
        assert!(form.values().unwrap_err().contains("tick somebody"));
    }

    #[test]
    fn a_question_round_trips_and_is_checked_before_it_leaves_the_form() {
        let step = Step {
            key: "1".into(),
            title: "Wash".into(),
            created: vec![Trigger::WithRoot],
            question: Some(Question {
                text: "Last load?".into(),
                kind: QuestionKind::Choice(vec!["yes".into(), "no".into(), "unsure".into()]),
                default: Some("no".into()),
            }),
            ..Default::default()
        };
        let mut form = StepForm::new(Some(1), &step, vec![]);
        assert_eq!(form.values().unwrap().question, step.question);
        form.q_default.set_text("maybe".to_string());
        assert!(form.values().unwrap_err().contains("not one of the answers"));
        form.q_default.set_text(String::new());
        form.q_kind.set_value(QuestionChoice::YesNo);
        let q = form.values().unwrap().question.unwrap();
        assert_eq!(q.kind, QuestionKind::YesNo);
        assert_eq!(q.default, None);
        form.q_kind.set_value(QuestionChoice::None);
        assert_eq!(form.values().unwrap().question, None);
    }

    #[test]
    fn the_limit_row_fits_the_window_at_the_smallest_terminal() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        let step = Step {
            key: "1".into(),
            title: "Wash".into(),
            created: vec![Trigger::WithRoot],
            start: StartRule::DaysLaterAt {
                days: 1,
                at: chrono::NaiveTime::from_hms_opt(9, 0, 0).unwrap(),
            },
            time_limit: Some(Offset {
                amount: 20,
                unit: OffsetUnit::Minutes,
            }),
            ..Default::default()
        };
        let mut form = StepForm::new(Some(1), &step, vec![]);
        let theme = crate::ui::theme::Theme::default();
        let mut term = Terminal::new(TestBackend::new(80, 40)).unwrap();
        term.draw(|f| form.render(f, f.area(), &theme)).unwrap();
        let out = term.backend().to_string();
        let limit = out.lines().find(|l| l.contains("time limit")).expect("the limit row");
        assert!(limit.contains("root waits"), "cut off: {limit}");
        let starts = out.lines().find(|l| l.contains("days later")).expect("the starts row");
        assert!(starts.contains("by itself"), "cut off: {starts}");
    }

    #[test]
    fn the_root_keeps_its_key_and_ignores_the_chain_fields() {
        let root = Step {
            key: ROOT_KEY.into(),
            title: "Clean up".into(),
            ..Default::default()
        };
        let mut form = StepForm::new(Some(0), &root, vec!["1".into()]);
        assert!(form.title.is_focused(), "no key to type for the root");
        form.with_root.set_checked(true);
        let back = form.values().unwrap();
        assert_eq!(back.key, ROOT_KEY);
        assert!(back.created.is_empty(), "the root is made when the program starts");
        assert_eq!(form.handle(&key(KeyCode::Esc)), StepOutcome::Cancel);
    }
}
