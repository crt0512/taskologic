//! A yes/no dialog with real buttons, so destructive answers can be given
//! by mouse or touch as well as by key.

use crossterm::event::{Event, KeyCode, KeyEventKind};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph, Wrap};

use super::{button_h, button_row, popup};
use crate::ui::adapter::{
    ButtonOutcome, ButtonState, Focus, FocusBuilder, HandleEvent, HasFocus, Outcome, Regular,
    render_button,
};
use crate::ui::theme::Theme;

#[derive(Debug, PartialEq)]
pub enum ConfirmOutcome {
    Continue,
    Changed,
    Yes,
    No,
}

pub struct Confirm {
    title: String,
    text: String,
    yes_label: String,
    no_label: String,
    yes: ButtonState,
    no: ButtonState,
}

impl Confirm {
    pub fn new(title: impl Into<String>, text: impl Into<String>) -> Self {
        let yes = ButtonState::new();
        let no = ButtonState::new();
        yes.focus().set(true);
        Self {
            title: title.into(),
            text: text.into(),
            yes_label: " Yes ".into(),
            no_label: " No ".into(),
            yes,
            no,
        }
    }

    fn focus(&self) -> Focus {
        let mut b = FocusBuilder::new(None);
        b.widget(&self.yes).widget(&self.no);
        b.build()
    }

    pub fn handle(&mut self, ev: &Event) -> ConfirmOutcome {
        // The ring has to see the event first: a button ignores the mouse
        // until the ring marks it as the thing under the pointer.
        let mut focus = self.focus();
        let moved = focus.handle(ev, Regular);
        if let Event::Key(k) = ev
            && k.kind != KeyEventKind::Release
        {
            match k.code {
                KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => {
                    return ConfirmOutcome::Yes;
                }
                KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc | KeyCode::Char('q') => {
                    return ConfirmOutcome::No;
                }
                KeyCode::Left | KeyCode::Right | KeyCode::Tab => {
                    let on_yes = self.yes.is_focused();
                    self.yes.focus().set(!on_yes);
                    self.no.focus().set(on_yes);
                    return ConfirmOutcome::Changed;
                }
                _ => {}
            }
        }
        if self.yes.handle(ev, Regular) == ButtonOutcome::Pressed {
            return ConfirmOutcome::Yes;
        }
        if self.no.handle(ev, Regular) == ButtonOutcome::Pressed {
            return ConfirmOutcome::No;
        }
        if moved == Outcome::Changed {
            ConfirmOutcome::Changed
        } else {
            ConfirmOutcome::Continue
        }
    }

    pub fn render(&mut self, f: &mut Frame, area: Rect, t: &Theme) {
        // Wide enough for the longest line, within reason, so nothing wraps
        // where it does not have to.
        let longest = self
            .text
            .lines()
            .map(|l| l.chars().count())
            .max()
            .unwrap_or(0) as u16;
        let width = longest.saturating_add(4).clamp(32, 70).min(area.width);
        let inner_w = width.saturating_sub(2).max(1);
        let text_lines: u16 = self
            .text
            .lines()
            .map(|l| (l.chars().count() as u16).div_ceil(inner_w).max(1))
            .sum();
        let height = (text_lines + 3 + button_h(t)).min(area.height);
        let p = popup(area, width, height);
        f.render_widget(Clear, p);
        let block = super::frame_block_titled(&self.title, t);
        let inner = block.inner(p);
        f.render_widget(block, p);
        let [text, buttons] =
            Layout::vertical([Constraint::Min(1), Constraint::Length(button_h(t))]).areas(inner);
        let lines: Vec<Line> = self
            .text
            .lines()
            .map(|l| Line::from(Span::styled(l.to_string(), t.surface())))
            .collect();
        f.render_widget(
            Paragraph::new(lines)
                .wrap(Wrap { trim: false })
                .style(t.surface()),
            text,
        );
        let (y, n) = button_row(buttons, &self.yes_label, &self.no_label, t);
        render_button(f, y, &self.yes_label, &mut self.yes, t);
        render_button(f, n, &self.no_label, &mut self.no, t);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    #[test]
    fn keys_and_clicks_both_answer() {
        let mut c = Confirm::new("Quit", "Really?");
        assert_eq!(
            c.handle(&Event::Key(KeyEvent::new(
                KeyCode::Char('y'),
                KeyModifiers::NONE
            ))),
            ConfirmOutcome::Yes
        );
        assert_eq!(
            c.handle(&Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE))),
            ConfirmOutcome::No
        );

        let t = Theme::default();
        let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
        term.draw(|f| c.render(f, f.area(), &t)).unwrap();
        let area = c.yes.area;
        assert!(area.width > 0, "the button recorded its area");
        let click = |kind| {
            Event::Mouse(MouseEvent {
                kind,
                column: area.x,
                row: area.y,
                modifiers: KeyModifiers::NONE,
            })
        };
        c.handle(&click(MouseEventKind::Down(MouseButton::Left)));
        assert_eq!(
            c.handle(&click(MouseEventKind::Up(MouseButton::Left))),
            ConfirmOutcome::Yes
        );
    }
}
