//! A task's own print rules, in a window of their own beside the task form.
//!
//! The task form has no room for a list that grows, so the Printing button
//! on it opens this: the rules the task has, one sentence each, and a row
//! to compose another. Nothing here talks to the daemon; the rules go back
//! to the task form and are saved with the rest of the task.

use std::collections::HashMap;

use crossterm::event::{Event, KeyCode, KeyEventKind, MouseButton, MouseEventKind};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::widgets::{Clear, ListItem, Paragraph};
use taskologic_core::ids::Uid;
use taskologic_core::offset::{MAX_OFFSET_AMOUNT, Offset, OffsetUnit};
use taskologic_core::print::{PrintRule, PrintWhen, Recipients, SlipKind};

use super::{Row, button_bar, button_h, frame_block, popup};
use crate::ui::adapter::{
    ButtonOutcome, ButtonState, ChoiceState, Focus, FocusBuilder, HandleEvent, HasFocus,
    HasScreenCursor, ListState, Outcome, Regular, TextInputState, dropdown, dropdown_marker,
    dropdown_popup_hover, field, list, render_button,
};
use crate::ui::theme::Theme;

#[derive(Debug, PartialEq)]
pub enum PrintRulesOutcome {
    Changed,
    /// Back to the task form with these rules. Closing is never a cancel:
    /// nothing is saved until the task is.
    Done(Vec<PrintRule>),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum WhenChoice {
    #[default]
    OnCreate,
    OnStart,
    BeforeDue,
}

/// Who a composed rule prints for. "Me" is whoever is at the keyboard: a
/// slip for yourself on a task that is somebody else's.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum ToChoice {
    #[default]
    Assignees,
    Creator,
    Me,
}

const WHEN_ITEMS: [(WhenChoice, &str); 3] = [
    (WhenChoice::OnCreate, "when created"),
    (WhenChoice::OnStart, "on start"),
    (WhenChoice::BeforeDue, "before due"),
];
const TO_ITEMS: [(ToChoice, &str); 3] = [
    (ToChoice::Assignees, "assignees"),
    (ToChoice::Creator, "creator"),
    (ToChoice::Me, "me"),
];

pub struct PrintRulesForm {
    me: Uid,
    names: HashMap<Uid, String>,
    /// Whether a group sheet is on offer, which it only is for a program
    /// step that fans out.
    sheets: bool,
    rules: Vec<PrintRule>,
    list: ListState,
    when: ChoiceState<WhenChoice>,
    lead_amount: TextInputState,
    lead_unit: ChoiceState<OffsetUnit>,
    slip: ChoiceState<SlipKind>,
    to: ChoiceState<ToChoice>,
    add: ButtonState,
    remove: ButtonState,
    done: ButtonState,
    pub error: Option<String>,
}

impl PrintRulesForm {
    pub fn new(rules: Vec<PrintRule>, me: Uid, names: HashMap<Uid, String>, sheets: bool) -> Self {
        let mut list = ListState::named("print_rules");
        list.select((!rules.is_empty()).then_some(0));
        let mut when = ChoiceState::named("when");
        when.set_value(WhenChoice::OnStart);
        let mut lead_amount = TextInputState::named("lead_amount");
        lead_amount.set_text("10");
        let mut lead_unit = ChoiceState::named("lead_unit");
        lead_unit.set_value(OffsetUnit::Minutes);
        let mut slip = ChoiceState::named("slip");
        slip.set_value(SlipKind::Task);
        let mut to = ChoiceState::named("to");
        to.set_value(ToChoice::Assignees);
        // Composing is what you came for, so the first field has the focus.
        when.focus().set(true);
        Self {
            me,
            names,
            sheets,
            rules,
            list,
            when,
            lead_amount,
            lead_unit,
            slip,
            to,
            add: ButtonState::new(),
            remove: ButtonState::new(),
            done: ButtonState::new(),
            error: None,
        }
    }

    #[cfg(test)]
    fn rules(&self) -> &[PrintRule] {
        &self.rules
    }

    fn name_of(&self, uid: Uid) -> String {
        self.names
            .get(&uid)
            .cloned()
            .unwrap_or_else(|| format!("uid {uid}"))
    }

    fn focus(&self) -> Focus {
        let mut b = FocusBuilder::new(None);
        b.widget(&self.when);
        if self.when.value() == WhenChoice::BeforeDue {
            b.widget(&self.lead_amount).widget(&self.lead_unit);
        }
        b.widget(&self.slip)
            .widget(&self.to)
            .widget(&self.add)
            .widget(&self.list)
            .widget(&self.remove)
            .widget(&self.done);
        b.build()
    }

    /// The rule the composer row describes, or why it does not describe one.
    fn composed(&self) -> Result<PrintRule, String> {
        let when = match self.when.value() {
            WhenChoice::OnCreate => PrintWhen::OnCreate,
            WhenChoice::OnStart => PrintWhen::OnStart,
            WhenChoice::BeforeDue => {
                let amount: u32 = self
                    .lead_amount
                    .text()
                    .trim()
                    .parse()
                    .map_err(|_| "the lead time needs a whole number".to_string())?;
                if amount > MAX_OFFSET_AMOUNT {
                    return Err("that lead time would land before the calendar starts".into());
                }
                PrintWhen::BeforeDue(Offset {
                    amount,
                    unit: self.lead_unit.value(),
                })
            }
        };
        let to = match self.to.value() {
            ToChoice::Assignees => Recipients::Assignees,
            ToChoice::Creator => Recipients::Creator,
            ToChoice::Me => Recipients::Users(vec![self.me]),
        };
        Ok(PrintRule {
            when,
            slip: self.slip.value(),
            to,
        })
    }

    fn add_rule(&mut self) {
        match self.composed() {
            Ok(rule) => {
                // The same rule twice would print the same slip once anyway,
                // so it is not worth a second line.
                if !self.rules.contains(&rule) {
                    self.rules.push(rule);
                }
                self.list.select(Some(self.rules.len() - 1));
                self.error = None;
            }
            Err(e) => self.error = Some(e),
        }
    }

    fn remove_rule(&mut self, i: usize) {
        if i < self.rules.len() {
            self.rules.remove(i);
            self.list.select(if self.rules.is_empty() {
                None
            } else {
                Some(i.min(self.rules.len() - 1))
            });
        }
    }

    pub fn handle(&mut self, ev: &Event) -> PrintRulesOutcome {
        let key = match ev {
            Event::Key(k) if k.kind != KeyEventKind::Release => Some(k.code),
            _ => None,
        };
        let popup_open = self.when.is_popup_active()
            || self.lead_unit.is_popup_active()
            || self.slip.is_popup_active()
            || self.to.is_popup_active();
        match key {
            Some(KeyCode::Esc) if !popup_open => {
                return PrintRulesOutcome::Done(self.rules.clone());
            }
            Some(KeyCode::F(2)) => return PrintRulesOutcome::Done(self.rules.clone()),
            _ => {}
        }
        let mut focus = self.focus();
        if focus.handle(ev, Regular) == Outcome::Changed && key.is_some() {
            return PrintRulesOutcome::Changed;
        }
        if self.done.handle(ev, Regular) == ButtonOutcome::Pressed {
            return PrintRulesOutcome::Done(self.rules.clone());
        }
        // Enter anywhere on the composer row adds, the same as the button.
        let composing = self.when.is_focused()
            || self.lead_amount.is_focused()
            || self.lead_unit.is_focused()
            || self.slip.is_focused()
            || self.to.is_focused();
        if self.add.handle(ev, Regular) == ButtonOutcome::Pressed
            || (key == Some(KeyCode::Enter) && composing && !popup_open)
        {
            self.add_rule();
            return PrintRulesOutcome::Changed;
        }
        let drop = self.remove.handle(ev, Regular) == ButtonOutcome::Pressed
            || (self.list.is_focused()
                && matches!(
                    key,
                    Some(KeyCode::Delete | KeyCode::Backspace | KeyCode::Char('x'))
                ));
        if drop {
            if let Some(i) = self.list.selected() {
                self.remove_rule(i);
            }
            return PrintRulesOutcome::Changed;
        }
        // Clicking a rule drops it: the list only shows what is there, so
        // there is nothing else a click on a line could mean.
        if let Event::Mouse(m) = ev
            && matches!(m.kind, MouseEventKind::Down(MouseButton::Left))
            && let Some(i) = self
                .list
                .row_areas
                .iter()
                .position(|r| r.contains(Position::new(m.column, m.row)))
        {
            self.remove_rule(i);
            return PrintRulesOutcome::Changed;
        }
        self.when.handle(ev, Regular);
        if self.when.value() == WhenChoice::BeforeDue {
            self.lead_amount.handle(ev, Regular);
            self.lead_unit.handle(ev, Regular);
        }
        self.slip.handle(ev, Regular);
        self.to.handle(ev, Regular);
        self.list.handle(ev, Regular);
        PrintRulesOutcome::Changed
    }

    pub fn render(&mut self, f: &mut Frame, area: Rect, t: &Theme) {
        let bh = button_h(t);
        let pad = if t.touch { 2 } else { 0 };
        let p = popup(area, 76, 16 + bh);
        f.render_widget(Clear, p);
        let block = frame_block(
            " Printing for this task ",
            " Enter adds   x drops the selected rule   Esc goes back ",
            t,
        );
        let inner = block.inner(p);
        f.render_widget(block, p);
        let [note, _, compose, _, heading, rows, err, buttons] = Layout::vertical([
            Constraint::Length(2),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(1),
            Constraint::Length(bh),
        ])
        .areas(inner);
        f.render_widget(
            Paragraph::new(
                "While a task has rules they replace your reminder and auto print settings\n\
                 for it. What each slip looks like is set up with the printer.",
            )
            .style(t.surface_dim()),
            note,
        );

        let mut r = Row::new(compose);
        let when_area = r.take(15);
        let (when_w, when_popup) = dropdown(WHEN_ITEMS, when_area, t);
        f.render_stateful_widget(when_w, when_area, &mut self.when);
        dropdown_marker(f, &self.when, t);
        let mut unit_area = None;
        if self.when.value() == WhenChoice::BeforeDue {
            f.render_stateful_widget(field(t), r.take(6), &mut self.lead_amount);
            let ua = r.take(11);
            let (unit_w, unit_popup) = dropdown(OffsetUnit::ALL.map(|u| (u, u.label())), ua, t);
            f.render_stateful_widget(unit_w, ua, &mut self.lead_unit);
            dropdown_marker(f, &self.lead_unit, t);
            unit_area = Some((ua, unit_popup));
        }
        let slip_area = r.take(16);
        let slips: Vec<(SlipKind, &str)> = SlipKind::ALL
            .into_iter()
            .filter(|s| self.sheets || *s != SlipKind::Sheet)
            .map(|s| (s, s.label()))
            .collect();
        let (slip_w, slip_popup) = dropdown(slips, slip_area, t);
        f.render_stateful_widget(slip_w, slip_area, &mut self.slip);
        dropdown_marker(f, &self.slip, t);
        f.render_widget(Paragraph::new("to").style(t.surface_dim()), r.text("to"));
        let to_area = r.take(12);
        let (to_w, to_popup) = dropdown(TO_ITEMS, to_area, t);
        f.render_stateful_widget(to_w, to_area, &mut self.to);
        dropdown_marker(f, &self.to, t);
        let add = r.take(super::button_w(" Add ") + pad);
        render_button(f, add, " Add ", &mut self.add, t);

        f.render_widget(
            Paragraph::new(format!("Rules ({})", self.rules.len())).style(t.surface_dim()),
            heading,
        );
        let items: Vec<ListItem> = if self.rules.is_empty() {
            vec![
                ListItem::new("none yet, so your own settings decide how this task prints")
                    .style(t.surface_dim()),
            ]
        } else {
            self.rules
                .iter()
                .map(|rule| ListItem::new(rule.summary(&|u| self.name_of(u))))
                .collect()
        };
        f.render_stateful_widget(list(items, t), rows, &mut self.list);
        if let Some(e) = &self.error {
            f.render_widget(Paragraph::new(e.clone()).style(t.error()), err);
        }
        let labels = [" Remove ", " Done "];
        let rects = button_bar(buttons, &labels, t);
        if let Some(r) = rects.first() {
            render_button(f, *r, labels[0], &mut self.remove, t);
        }
        if let Some(r) = rects.get(1) {
            render_button(f, *r, labels[1], &mut self.done, t);
        }

        // Open dropdown lists draw over everything else.
        f.render_stateful_widget(when_popup, when_area, &mut self.when);
        dropdown_popup_hover(f, &self.when, t);
        if let Some((ua, unit_popup)) = unit_area {
            f.render_stateful_widget(unit_popup, ua, &mut self.lead_unit);
            dropdown_popup_hover(f, &self.lead_unit, t);
        }
        f.render_stateful_widget(slip_popup, slip_area, &mut self.slip);
        dropdown_popup_hover(f, &self.slip, t);
        f.render_stateful_widget(to_popup, to_area, &mut self.to);
        dropdown_popup_hover(f, &self.to, t);
        if let Some(pos) = self.lead_amount.screen_cursor() {
            f.set_cursor_position(pos);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyEvent, KeyModifiers};

    fn key(code: KeyCode) -> Event {
        Event::Key(KeyEvent::new(code, KeyModifiers::NONE))
    }

    fn form(rules: Vec<PrintRule>) -> PrintRulesForm {
        PrintRulesForm::new(rules, 7, HashMap::from([(7, "alice".to_string())]), false)
    }

    #[test]
    fn enter_on_the_composer_adds_the_rule_it_describes_once() {
        let mut f = form(Vec::new());
        assert!(f.when.is_focused());
        f.when.set_value(WhenChoice::OnStart);
        f.slip.set_value(SlipKind::Reminder);
        f.handle(&key(KeyCode::Enter));
        f.handle(&key(KeyCode::Enter));
        assert_eq!(
            f.rules(),
            &[PrintRule {
                when: PrintWhen::OnStart,
                slip: SlipKind::Reminder,
                to: Recipients::Assignees,
            }],
            "the same rule twice is one rule"
        );
        // "Me" names whoever is at the keyboard.
        f.to.set_value(ToChoice::Me);
        f.handle(&key(KeyCode::Enter));
        assert_eq!(f.rules()[1].to, Recipients::Users(vec![7]));
        match f.handle(&key(KeyCode::Esc)) {
            PrintRulesOutcome::Done(rules) => assert_eq!(rules.len(), 2),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_lead_time_has_to_be_a_number_and_lands_in_the_rule() {
        let mut f = form(Vec::new());
        f.when.set_value(WhenChoice::BeforeDue);
        f.lead_amount.set_text("soon".to_string());
        f.handle(&key(KeyCode::Enter));
        assert!(f.rules().is_empty());
        assert!(f.error.as_deref().unwrap_or("").contains("whole number"));
        f.lead_amount.set_text("15".to_string());
        f.lead_unit.set_value(OffsetUnit::Minutes);
        f.handle(&key(KeyCode::Enter));
        assert_eq!(
            f.rules()[0].when,
            PrintWhen::BeforeDue(Offset {
                amount: 15,
                unit: OffsetUnit::Minutes
            })
        );
        assert_eq!(f.error, None);
    }

    #[test]
    fn x_on_the_list_drops_the_selected_rule() {
        let rules = vec![
            PrintRule {
                when: PrintWhen::OnCreate,
                slip: SlipKind::Task,
                to: Recipients::Creator,
            },
            PrintRule {
                when: PrintWhen::OnStart,
                slip: SlipKind::Task,
                to: Recipients::Assignees,
            },
        ];
        let mut f = form(rules.clone());
        f.when.focus().set(false);
        f.list.focus().set(true);
        f.list.select(Some(0));
        f.handle(&key(KeyCode::Char('x')));
        assert_eq!(f.rules(), &rules[1..]);
        assert_eq!(f.list.selected(), Some(0));
        f.handle(&key(KeyCode::Char('x')));
        assert!(f.rules().is_empty());
        assert_eq!(f.list.selected(), None);
    }
}
