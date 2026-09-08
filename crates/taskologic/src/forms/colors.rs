//! The custom colour editor. Twelve fields, each a colour name, a `#rrggbb`
//! value or a palette index. Anything that does not parse falls back to the
//! default theme, and the field says so.

use crossterm::event::{Event, KeyCode, KeyEventKind};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::widgets::{Clear, Paragraph};
use taskologic_core::prefs::CustomColors;

use super::{Row, button_h, button_row, frame_block, popup};
use crate::ui::adapter::{
    ButtonOutcome, ButtonState, Focus, FocusBuilder, HandleEvent, HasFocus, HasScreenCursor,
    Outcome, Regular, TextInputState, field, render_button,
};
use crate::ui::theme::{Theme, parse_color};

#[derive(Debug, PartialEq)]
pub enum ColorsOutcome {
    Changed,
    Cancel,
    Save(Box<CustomColors>),
}

/// Field label and how to read and write it on [`CustomColors`].
type Slot = (
    &'static str,
    fn(&CustomColors) -> &String,
    fn(&mut CustomColors, String),
);

const SLOTS: [Slot; 12] = [
    ("screen", |c| &c.screen, |c, v| c.screen = v),
    ("bar", |c| &c.bar, |c, v| c.bar = v),
    ("surface", |c| &c.surface, |c, v| c.surface = v),
    ("card", |c| &c.card, |c, v| c.card = v),
    ("text", |c| &c.text, |c, v| c.text = v),
    ("muted", |c| &c.muted, |c, v| c.muted = v),
    ("accent", |c| &c.accent, |c, v| c.accent = v),
    ("select", |c| &c.select, |c, v| c.select = v),
    ("button", |c| &c.button, |c, v| c.button = v),
    ("warn", |c| &c.warn, |c, v| c.warn = v),
    ("danger", |c| &c.danger, |c, v| c.danger = v),
    ("ok", |c| &c.ok, |c, v| c.ok = v),
];

pub struct ColorsForm {
    fields: Vec<TextInputState>,
    save: ButtonState,
    cancel: ButtonState,
    reset: ButtonState,
}

impl ColorsForm {
    pub fn new(colors: &CustomColors) -> Self {
        let fields: Vec<TextInputState> = SLOTS
            .iter()
            .map(|(name, get, _)| {
                let mut f = TextInputState::named(name);
                f.set_text(get(colors).clone());
                f
            })
            .collect();
        if let Some(f) = fields.first() {
            f.focus().set(true);
        }
        Self {
            fields,
            save: ButtonState::new(),
            cancel: ButtonState::new(),
            reset: ButtonState::new(),
        }
    }

    fn values(&self) -> CustomColors {
        let mut c = CustomColors::default();
        for (slot, f) in SLOTS.iter().zip(&self.fields) {
            (slot.2)(&mut c, f.text().trim().to_string());
        }
        c
    }

    fn focus(&self) -> Focus {
        let mut b = FocusBuilder::new(None);
        for f in &self.fields {
            b.widget(f);
        }
        b.widget(&self.reset)
            .widget(&self.save)
            .widget(&self.cancel);
        b.build()
    }

    pub fn handle(&mut self, ev: &Event) -> ColorsOutcome {
        let key = match ev {
            Event::Key(k) if k.kind != KeyEventKind::Release => Some(k.code),
            _ => None,
        };
        match key {
            Some(KeyCode::Esc) => return ColorsOutcome::Cancel,
            Some(KeyCode::F(2)) => return ColorsOutcome::Save(Box::new(self.values())),
            _ => {}
        }
        let mut focus = self.focus();
        if focus.handle(ev, Regular) == Outcome::Changed && key.is_some() {
            return ColorsOutcome::Changed;
        }
        if self.save.handle(ev, Regular) == ButtonOutcome::Pressed {
            return ColorsOutcome::Save(Box::new(self.values()));
        }
        if self.cancel.handle(ev, Regular) == ButtonOutcome::Pressed {
            return ColorsOutcome::Cancel;
        }
        if self.reset.handle(ev, Regular) == ButtonOutcome::Pressed {
            let d = CustomColors::default();
            for (slot, f) in SLOTS.iter().zip(&mut self.fields) {
                f.set_text((slot.1)(&d).clone());
            }
            return ColorsOutcome::Changed;
        }
        for f in &mut self.fields {
            f.handle(ev, Regular);
        }
        ColorsOutcome::Changed
    }

    pub fn render(&mut self, f: &mut Frame, area: Rect, t: &Theme) {
        let bh = button_h(t);
        let pad = if t.touch { 2 } else { 0 };
        let p = popup(area, 74, 11 + bh);
        f.render_widget(Clear, p);
        let block = frame_block(
            " Custom colours ",
            " Tab moves   F2 saves   Esc cancels ",
            t,
        );
        let inner = block.inner(p);
        f.render_widget(block, p);
        let mut constraints = vec![Constraint::Length(1); 9];
        constraints.push(Constraint::Length(bh));
        let rows = Layout::vertical(constraints).split(inner);

        f.render_widget(
            Paragraph::new("A colour name (blue, lightcyan), #rrggbb, or a number from 0 to 255.")
                .style(t.surface_dim()),
            rows[0],
        );
        // Three rows of four, each pair as wide as its own label.
        for (chunk, row) in SLOTS.chunks(4).zip(&rows[2..5]) {
            let mut r = Row::new(*row);
            for (i, (name, _, _)) in chunk.iter().enumerate() {
                let idx = SLOTS.iter().position(|s| s.0 == *name).unwrap_or(i);
                f.render_widget(Paragraph::new(*name).style(t.surface_dim()), r.text(name));
                f.render_stateful_widget(field(t), r.take(11), &mut self.fields[idx]);
            }
        }

        // Say which ones the theme will not understand.
        let bad: Vec<&str> = SLOTS
            .iter()
            .zip(&self.fields)
            .filter(|(_, f)| parse_color(f.text().trim()).is_none())
            .map(|(s, _)| s.0)
            .collect();
        if !bad.is_empty() {
            let text = format!("not understood, will use the default: {}", bad.join(", "));
            f.render_widget(Paragraph::new(text).style(t.error()), rows[6]);
        }

        let mut r = Row::new(rows[9]);
        let rs = r.take(super::button_w(" Reset ") + pad);
        render_button(f, rs, " Reset ", &mut self.reset, t);
        let (save, cancel) = button_row(rows[9], " Save ", " Cancel ", t);
        render_button(f, save, " Save ", &mut self.save, t);
        render_button(f, cancel, " Cancel ", &mut self.cancel, t);

        if let Some(pos) = self.fields.iter().filter_map(|f| f.screen_cursor()).next() {
            f.set_cursor_position(pos);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyEvent, KeyModifiers};

    #[test]
    fn round_trips_and_saves() {
        let mut c = CustomColors::default();
        c.accent = "green".into();
        let mut form = ColorsForm::new(&c);
        match form.handle(&Event::Key(KeyEvent::new(
            KeyCode::F(2),
            KeyModifiers::NONE,
        ))) {
            ColorsOutcome::Save(out) => assert_eq!(*out, c),
            other => panic!("{other:?}"),
        }
        assert_eq!(
            form.handle(&Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE))),
            ColorsOutcome::Cancel
        );
    }
}
