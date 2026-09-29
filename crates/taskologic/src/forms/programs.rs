//! The programs panel: the programs saved on one board. Start begins a run,
//! New and Edit open the program editor, Runs lists what is going, Delete
//! asks first. Managing follows the template rules and the daemon is the
//! authority; this panel only pre-checks to say why.

use crossterm::event::{Event, KeyCode, KeyEventKind};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::widgets::{Clear, ListItem, Paragraph};
use taskologic_core::ids::{BoardId, Uid};
use taskologic_core::program::Program;

use super::{button_bar, button_h, clicked_outside, frame_block, popup, tall_item};
use crate::ui::adapter::{
    ButtonOutcome, ButtonState, Focus, FocusBuilder, HandleEvent, HasFocus, ListState, Outcome,
    Regular, list, render_button,
};
use crate::ui::theme::Theme;

#[derive(Debug, PartialEq)]
pub enum ProgramsOutcome {
    Changed,
    Cancel,
    New,
    Start(Box<Program>),
    Edit(Box<Program>),
    Delete(Box<Program>),
    Runs,
    /// Print the program's own control codes.
    PrintCode(Box<Program>),
}

pub struct ProgramsPanel {
    pub board_id: BoardId,
    me: Uid,
    /// Where the window was last drawn; a click anywhere else closes it.
    area: Rect,
    privileged: bool,
    programs: Vec<(Program, String)>,
    list: ListState,
    start_btn: ButtonState,
    new_btn: ButtonState,
    edit_btn: ButtonState,
    runs_btn: ButtonState,
    code_btn: ButtonState,
    delete_btn: ButtonState,
    close_btn: ButtonState,
    pub error: Option<String>,
}

impl ProgramsPanel {
    pub fn new(board_id: BoardId, me: Uid, privileged: bool) -> Self {
        let list = ListState::named("programs");
        list.focus().set(true);
        Self {
            area: Rect::default(),
            board_id,
            me,
            privileged,
            programs: Vec::new(),
            list,
            start_btn: ButtonState::new(),
            new_btn: ButtonState::new(),
            edit_btn: ButtonState::new(),
            runs_btn: ButtonState::new(),
            code_btn: ButtonState::new(),
            delete_btn: ButtonState::new(),
            close_btn: ButtonState::new(),
            error: None,
        }
    }

    pub fn set_programs(&mut self, programs: Vec<Program>, name_of: &dyn Fn(Uid) -> String) {
        self.programs = programs
            .into_iter()
            .map(|p| {
                let n = name_of(p.owner_uid);
                (p, n)
            })
            .collect();
        let sel = self.list.selected().unwrap_or(0);
        self.list.select(if self.programs.is_empty() {
            None
        } else {
            Some(sel.min(self.programs.len() - 1))
        });
    }

    fn selected(&self) -> Option<&Program> {
        self.list
            .selected()
            .and_then(|i| self.programs.get(i))
            .map(|(p, _)| p)
    }

    fn manageable(&mut self) -> Option<Program> {
        let p = self.selected()?.clone();
        if self.privileged || p.owner_uid == self.me {
            Some(p)
        } else {
            self.error = Some(
                "only the program creator, the board owner or an admin can change or delete this program"
                    .into(),
            );
            None
        }
    }

    fn focus(&self) -> Focus {
        let mut b = FocusBuilder::new(None);
        b.widget(&self.list)
            .widget(&self.start_btn)
            .widget(&self.new_btn)
            .widget(&self.edit_btn)
            .widget(&self.runs_btn)
            .widget(&self.code_btn)
            .widget(&self.delete_btn)
            .widget(&self.close_btn);
        b.build()
    }

    pub fn handle(&mut self, ev: &Event) -> ProgramsOutcome {
        let key = match ev {
            Event::Key(k) if k.kind != KeyEventKind::Release => Some(k.code),
            _ => None,
        };
        if matches!(key, Some(KeyCode::Esc | KeyCode::Char('q'))) || clicked_outside(ev, self.area) {
            return ProgramsOutcome::Cancel;
        }
        let mut focus = self.focus();
        if focus.handle(ev, Regular) == Outcome::Changed && key.is_some() {
            return ProgramsOutcome::Changed;
        }
        if self.close_btn.handle(ev, Regular) == ButtonOutcome::Pressed {
            return ProgramsOutcome::Cancel;
        }
        if self.new_btn.handle(ev, Regular) == ButtonOutcome::Pressed
            || key == Some(KeyCode::Char('n'))
        {
            return ProgramsOutcome::New;
        }
        if self.runs_btn.handle(ev, Regular) == ButtonOutcome::Pressed
            || key == Some(KeyCode::Char('r'))
        {
            return ProgramsOutcome::Runs;
        }
        if self.code_btn.handle(ev, Regular) == ButtonOutcome::Pressed
            || key == Some(KeyCode::Char('c'))
        {
            return match self.selected() {
                Some(p) => ProgramsOutcome::PrintCode(Box::new(p.clone())),
                None => ProgramsOutcome::Changed,
            };
        }
        let start = self.start_btn.handle(ev, Regular) == ButtonOutcome::Pressed
            || (key == Some(KeyCode::Enter) && self.list.is_focused());
        if start {
            return match self.selected() {
                Some(p) => ProgramsOutcome::Start(Box::new(p.clone())),
                None => ProgramsOutcome::Changed,
            };
        }
        if self.edit_btn.handle(ev, Regular) == ButtonOutcome::Pressed
            || key == Some(KeyCode::Char('e'))
        {
            return match self.manageable() {
                Some(p) => ProgramsOutcome::Edit(Box::new(p)),
                None => ProgramsOutcome::Changed,
            };
        }
        if self.delete_btn.handle(ev, Regular) == ButtonOutcome::Pressed {
            return match self.manageable() {
                Some(p) => ProgramsOutcome::Delete(Box::new(p)),
                None => ProgramsOutcome::Changed,
            };
        }
        self.list.handle(ev, Regular);
        ProgramsOutcome::Changed
    }

    pub fn render(&mut self, f: &mut Frame, area: Rect, t: &Theme) {
        // The same window as a task's, so the panels line up.
        let p = popup(area, 78, 32 + button_h(t));
        self.area = p;
        f.render_widget(Clear, p);
        let block = frame_block(
            " Programs ",
            " Enter starts the selected one   Esc closes ",
            t,
        );
        let inner = block.inner(p);
        f.render_widget(block, p);
        let [l, note, err, buttons] = Layout::vertical([
            Constraint::Min(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(button_h(t)),
        ])
        .areas(inner);
        let items: Vec<ListItem> = if self.programs.is_empty() {
            vec![
                ListItem::new("no programs on this board yet, New writes one")
                    .style(t.surface_dim()),
            ]
        } else {
            self.programs
                .iter()
                .map(|(p, owner)| {
                    let steps = p.steps.len().saturating_sub(1);
                    tall_item(
                        format!(
                            "{:<28} {steps} step{}  by {owner}",
                            p.name,
                            if steps == 1 { "" } else { "s" }
                        ),
                        t,
                    )
                })
                .collect()
        };
        f.render_stateful_widget(list(items, t), l, &mut self.list);
        f.render_widget(
            Paragraph::new("a program is a chain of tasks; starting one makes its root task")
                .style(t.surface_dim()),
            note,
        );
        if let Some(e) = &self.error {
            f.render_widget(Paragraph::new(e.clone()).style(t.error()), err);
        }
        let labels = [" Start ", " New ", " Edit ", " Runs ", " Code ", " Delete ", " Close "];
        let rects = button_bar(buttons, &labels, t);
        let states = [
            &mut self.start_btn,
            &mut self.new_btn,
            &mut self.edit_btn,
            &mut self.runs_btn,
            &mut self.code_btn,
            &mut self.delete_btn,
            &mut self.close_btn,
        ];
        for ((r, label), state) in rects.iter().zip(labels).zip(states) {
            render_button(f, *r, label, state, t);
        }
    }
}
