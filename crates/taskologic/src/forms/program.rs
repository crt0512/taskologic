//! The program editor: a name, a description and the list of steps. Each
//! step is edited in its own window, `forms::step`, which hands the step
//! back here. Saving sends the whole program; the daemon checks it against
//! the board and says what is wrong, if anything.

use crossterm::event::{Event, KeyCode, KeyEventKind};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::widgets::{Clear, ListItem, Paragraph};
use taskologic_core::ids::{BoardId, ProgramId};
use taskologic_core::template::DEFAULT_MIN_SAMPLES;
use taskologic_core::program::{
    Program, ProgramDraft, ROOT_KEY, Step, start_rule_summary, triggers_summary,
};

use super::{Row, button_bar, button_h, button_row, frame_block, label, popup, split_label};
use crate::ui::adapter::{
    ButtonOutcome, ButtonState, Focus, FocusBuilder, HandleEvent, HasFocus, HasScreenCursor,
    ListState, Outcome, Regular, TextInputState, field, list, render_button,
};
use crate::ui::theme::Theme;

#[derive(Debug, PartialEq)]
pub enum ProgramOutcome {
    Continue,
    Changed,
    Cancel,
    /// Open the step window. `index` None is a new step.
    EditStep { index: Option<usize>, step: Box<Step> },
    Save(Box<ProgramDraft>),
}

pub struct ProgramForm {
    pub board_id: BoardId,
    /// None while creating.
    pub program_id: Option<ProgramId>,
    name: TextInputState,
    description: TextInputState,
    /// Finished tasks of a step before the board shows its estimate.
    min_samples: TextInputState,
    steps: Vec<Step>,
    list: ListState,
    add: ButtonState,
    edit: ButtonState,
    remove: ButtonState,
    up: ButtonState,
    down: ButtonState,
    save: ButtonState,
    cancel: ButtonState,
    pub error: Option<String>,
    /// A save is in flight. Input is ignored until the daemon answers.
    pub saving: bool,
}

impl ProgramForm {
    fn blank(board_id: BoardId, program_id: Option<ProgramId>, steps: Vec<Step>) -> Self {
        let name = TextInputState::named("name");
        name.focus().set(true);
        let mut list = ListState::named("steps");
        list.select((!steps.is_empty()).then_some(0));
        Self {
            board_id,
            program_id,
            name,
            description: TextInputState::named("description"),
            min_samples: {
                let mut s = TextInputState::named("min_samples");
                s.set_text(DEFAULT_MIN_SAMPLES.to_string());
                s
            },
            steps,
            list,
            add: ButtonState::new(),
            edit: ButtonState::new(),
            remove: ButtonState::new(),
            up: ButtonState::new(),
            down: ButtonState::new(),
            save: ButtonState::new(),
            cancel: ButtonState::new(),
            error: None,
            saving: false,
        }
    }

    /// A new program, with the root step already there: every program has
    /// one, and it is the first thing to give a title.
    pub fn create(board_id: BoardId) -> Self {
        Self::blank(
            board_id,
            None,
            vec![Step {
                key: ROOT_KEY.into(),
                ..Default::default()
            }],
        )
    }

    pub fn edit(program: &Program) -> Self {
        let mut f = Self::blank(program.board_id, Some(program.id), program.steps.clone());
        f.name.set_text(program.name.clone());
        f.description.set_text(program.description.clone());
        f.min_samples.set_text(program.min_samples.to_string());
        f
    }

    /// The step window closed with `step` for the `index`th slot, or a new
    /// one. A step whose key another one takes is not added; the step
    /// window refuses that first, this is only the last line.
    pub fn set_step(&mut self, index: Option<usize>, step: Step) {
        match index {
            Some(i) if i < self.steps.len() => self.steps[i] = step,
            _ => {
                self.steps.push(step);
                self.list.select(Some(self.steps.len() - 1));
            }
        }
    }

    /// Every key but the one at `index`, for the step window's lists.
    pub fn other_keys(&self, index: Option<usize>) -> Vec<String> {
        self.steps
            .iter()
            .enumerate()
            .filter(|(i, s)| Some(*i) != index && !s.is_root())
            .map(|(_, s)| s.key.clone())
            .collect()
    }

    fn focus(&self) -> Focus {
        let mut b = FocusBuilder::new(None);
        b.widget(&self.name)
            .widget(&self.description)
            .widget(&self.min_samples)
            .widget(&self.list)
            .widget(&self.add)
            .widget(&self.edit)
            .widget(&self.remove)
            .widget(&self.up)
            .widget(&self.down)
            .widget(&self.save)
            .widget(&self.cancel);
        b.build()
    }

    fn selected(&self) -> Option<usize> {
        self.list.selected().filter(|i| *i < self.steps.len())
    }

    fn remove_selected(&mut self) {
        let Some(i) = self.selected() else { return };
        if self.steps[i].is_root() {
            self.error = Some("the root step cannot be removed, every program has one".into());
            return;
        }
        let gone = self.steps.remove(i).key;
        // Nothing may go on waiting for a step that is not there.
        for s in &mut self.steps {
            s.created.retain(|t| !matches!(t, taskologic_core::program::Trigger::Finished { step } if *step == gone));
            s.also_after.retain(|k| *k != gone);
        }
        self.list.select(if self.steps.is_empty() {
            None
        } else {
            Some(i.min(self.steps.len() - 1))
        });
    }

    fn shift(&mut self, delta: isize) {
        let Some(i) = self.selected() else { return };
        let j = i as isize + delta;
        if j < 0 || j as usize >= self.steps.len() {
            return;
        }
        self.steps.swap(i, j as usize);
        self.list.select(Some(j as usize));
    }

    pub fn handle(&mut self, ev: &Event) -> ProgramOutcome {
        if self.saving {
            return ProgramOutcome::Continue;
        }
        let key = match ev {
            Event::Key(k) if k.kind != KeyEventKind::Release => Some(k.code),
            _ => None,
        };
        match key {
            Some(KeyCode::Esc) => return ProgramOutcome::Cancel,
            Some(KeyCode::F(2)) => return self.try_save(),
            _ => {}
        }
        let mut focus = self.focus();
        if focus.handle(ev, Regular) == Outcome::Changed && key.is_some() {
            return ProgramOutcome::Changed;
        }
        if self.save.handle(ev, Regular) == ButtonOutcome::Pressed {
            return self.try_save();
        }
        if self.cancel.handle(ev, Regular) == ButtonOutcome::Pressed {
            return ProgramOutcome::Cancel;
        }
        let on_list = self.list.is_focused();
        if self.add.handle(ev, Regular) == ButtonOutcome::Pressed
            || (on_list && key == Some(KeyCode::Char('n')))
        {
            return ProgramOutcome::EditStep {
                index: None,
                step: Box::new(Step::default()),
            };
        }
        if self.edit.handle(ev, Regular) == ButtonOutcome::Pressed
            || (on_list && key == Some(KeyCode::Enter))
        {
            return match self.selected() {
                Some(i) => ProgramOutcome::EditStep {
                    index: Some(i),
                    step: Box::new(self.steps[i].clone()),
                },
                None => ProgramOutcome::Changed,
            };
        }
        if self.remove.handle(ev, Regular) == ButtonOutcome::Pressed
            || (on_list && matches!(key, Some(KeyCode::Delete | KeyCode::Char('x'))))
        {
            self.remove_selected();
            return ProgramOutcome::Changed;
        }
        if self.up.handle(ev, Regular) == ButtonOutcome::Pressed
            || (on_list && key == Some(KeyCode::Char('K')))
        {
            self.shift(-1);
            return ProgramOutcome::Changed;
        }
        if self.down.handle(ev, Regular) == ButtonOutcome::Pressed
            || (on_list && key == Some(KeyCode::Char('J')))
        {
            self.shift(1);
            return ProgramOutcome::Changed;
        }
        self.name.handle(ev, Regular);
        self.description.handle(ev, Regular);
        self.min_samples.handle(ev, Regular);
        self.list.handle(ev, Regular);
        ProgramOutcome::Changed
    }

    fn try_save(&mut self) -> ProgramOutcome {
        match self.values() {
            Ok(draft) => {
                self.error = None;
                ProgramOutcome::Save(Box::new(draft))
            }
            Err(e) => {
                self.error = Some(e);
                ProgramOutcome::Changed
            }
        }
    }

    /// The draft as the form stands. The daemon does the real checking; the
    /// two things caught here are the ones that need no board to see.
    pub fn values(&self) -> Result<ProgramDraft, String> {
        let name = self.name.text().trim().to_string();
        if name.is_empty() {
            return Err("the program needs a name".into());
        }
        if self.steps.iter().find(|s| s.is_root()).is_none_or(|r| r.title.trim().is_empty()) {
            return Err("give the root step a title, it is the task the program starts with".into());
        }
        let min_samples: u32 = self
            .min_samples
            .text()
            .trim()
            .parse()
            .ok()
            .filter(|n| *n >= 1)
            .ok_or_else(|| "the estimate needs a whole number of finished tasks, at least 1".to_string())?;
        Ok(ProgramDraft {
            name,
            description: self.description.text().trim().to_string(),
            steps: self.steps.clone(),
            min_samples,
        })
    }

    fn line(step: &Step) -> String {
        let what = if step.is_root() {
            "made when the program starts".to_string()
        } else {
            format!(
                "appears {}, starts {}{}{}",
                triggers_summary(&step.created),
                start_rule_summary(step.start),
                if step.question.is_some() { ", asks" } else { "" },
                if step.counts_toward_root { "" } else { ", root does not wait" }
            )
        };
        format!("{:<7} {:<24} {what}", step.key, clip(&step.title, 24))
    }

    pub fn render(&mut self, f: &mut Frame, area: Rect, t: &Theme) {
        let bh = button_h(t);
        // Most of the screen, whatever its size, with three cells of desktop
        // around it: a program is read as a list and wants the room.
        let p = popup(area, area.width.saturating_sub(6), area.height.saturating_sub(6));
        f.render_widget(Clear, p);
        let title = if self.program_id.is_some() {
            " Edit program "
        } else {
            " New program "
        };
        let hint = if self.saving {
            " saving... "
        } else {
            " Enter edits a step   n adds one   F2 saves   Esc cancels "
        };
        let block = frame_block(title, hint, t);
        let inner = block.inner(p);
        f.render_widget(block, p);
        let [name_row, desc_row, est_row, heading, steps, step_buttons, err, buttons] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(3),
            Constraint::Length(bh),
            Constraint::Length(1),
            Constraint::Length(bh),
        ])
        .areas(inner);
        let lw = 12;
        let (l, w) = split_label(name_row, lw);
        label(f, l, "Name", t);
        f.render_stateful_widget(field(t), w, &mut self.name);
        let (l, w) = split_label(desc_row, lw);
        label(f, l, "Description", t);
        f.render_stateful_widget(field(t), w, &mut self.description);
        let (l, w) = split_label(est_row, lw);
        label(f, l, "Estimate", t);
        let mut r = Row::new(w);
        f.render_widget(Paragraph::new("after").style(t.surface_dim()), r.text("after"));
        f.render_stateful_widget(field(t), r.take(4), &mut self.min_samples);
        f.render_widget(
            Paragraph::new("finished tasks of a step, the board shows how long it usually takes")
                .style(t.surface_dim()),
            r.rest(),
        );
        f.render_widget(
            Paragraph::new("Steps, in the order they are listed. Every program has a root step.")
                .style(t.surface_dim()),
            heading,
        );
        let items: Vec<ListItem> = self.steps.iter().map(|s| ListItem::new(Self::line(s))).collect();
        f.render_stateful_widget(list(items, t), steps, &mut self.list);
        let labels = [" Add step ", " Edit ", " Remove ", " Up ", " Down "];
        let rects = button_bar(step_buttons, &labels, t);
        let states = [
            &mut self.add,
            &mut self.edit,
            &mut self.remove,
            &mut self.up,
            &mut self.down,
        ];
        for ((r, label), state) in rects.iter().zip(labels).zip(states) {
            render_button(f, *r, label, state, t);
        }
        if let Some(e) = &self.error {
            f.render_widget(Paragraph::new(e.clone()).style(t.error()), err);
        }
        let (save, cancel) = button_row(buttons, " Save ", " Cancel ", t);
        render_button(f, save, " Save ", &mut self.save, t);
        render_button(f, cancel, " Cancel ", &mut self.cancel, t);
        if let Some(pos) = [
            self.name.screen_cursor(),
            self.description.screen_cursor(),
            self.min_samples.screen_cursor(),
        ]
            .into_iter()
            .flatten()
            .next()
        {
            f.set_cursor_position(pos);
        }
    }
}

fn clip(s: &str, width: usize) -> String {
    if s.chars().count() <= width {
        return s.to_string();
    }
    let mut out: String = s.chars().take(width.saturating_sub(1)).collect();
    out.push('~');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyEvent, KeyModifiers};
    use taskologic_core::program::Trigger;

    fn key(code: KeyCode) -> Event {
        Event::Key(KeyEvent::new(code, KeyModifiers::NONE))
    }

    #[test]
    fn a_new_program_starts_with_a_root_and_needs_a_name_and_a_root_title() {
        let mut form = ProgramForm::create(BoardId(1));
        assert_eq!(form.steps.len(), 1);
        assert!(form.steps[0].is_root());
        assert!(form.values().unwrap_err().contains("needs a name"));
        form.name.set_text("Clean up".to_string());
        assert!(form.values().unwrap_err().contains("root step a title"));
        form.set_step(
            Some(0),
            Step {
                key: ROOT_KEY.into(),
                title: "Clean up the flat".into(),
                ..Default::default()
            },
        );
        let draft = form.values().unwrap();
        assert_eq!(draft.name, "Clean up");
        assert_eq!(draft.steps[0].title, "Clean up the flat");
    }

    #[test]
    fn the_editor_takes_the_screen_but_for_three_cells_around() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        let mut form = ProgramForm::create(BoardId(1));
        for (w, h) in [(100u16, 40u16), (80, 24), (160, 60)] {
            let theme = crate::ui::theme::Theme::default();
            let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
            term.draw(|f| form.render(f, f.area(), &theme)).unwrap();
            let out = term.backend().to_string();
            let lines: Vec<&str> = out.lines().collect();
            let framed: Vec<usize> = lines
                .iter()
                .enumerate()
                .filter(|(_, l)| l.contains('│') || l.contains('─'))
                .map(|(y, _)| y)
                .collect();
            assert_eq!((framed[0], *framed.last().unwrap()), (3, h as usize - 4), "{w}x{h}:\n{out}");
            // The test backend quotes each line; count in characters past it.
            let top = lines[3].trim_matches('"');
            let first = top.chars().take_while(|c| c.is_whitespace()).count();
            let last = top.trim_end().chars().count() - 1;
            assert_eq!((first, last), (3, w as usize - 4), "{w}x{h}:\n{out}");
        }
    }

    #[test]
    fn removing_a_step_drops_what_waited_for_it_and_never_the_root() {
        let mut form = ProgramForm::create(BoardId(1));
        form.set_step(
            None,
            Step {
                key: "1".into(),
                title: "One".into(),
                created: vec![Trigger::WithRoot],
                ..Default::default()
            },
        );
        form.set_step(
            None,
            Step {
                key: "2".into(),
                title: "Two".into(),
                created: vec![Trigger::Finished { step: "1".into() }],
                also_after: vec!["1".into()],
                ..Default::default()
            },
        );
        assert_eq!(form.other_keys(Some(2)), vec!["1".to_string()]);
        form.list.select(Some(1));
        form.remove_selected();
        assert_eq!(form.steps.len(), 2);
        assert!(form.steps[1].created.is_empty(), "nothing waits for a step that is gone");
        assert!(form.steps[1].also_after.is_empty());
        form.list.select(Some(0));
        form.remove_selected();
        assert_eq!(form.steps.len(), 2, "the root stays");
        assert!(form.error.as_deref().unwrap_or("").contains("root"));
        assert_eq!(form.handle(&key(KeyCode::Esc)), ProgramOutcome::Cancel);
    }
}
