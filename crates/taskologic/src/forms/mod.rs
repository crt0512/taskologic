//! Forms and panels: widget state, focus ring, validation and rendering
//! for one screenful of input. The app owns one as an overlay and forwards
//! raw terminal events to it; it answers with an outcome enum.

pub mod archive;
pub mod board;
pub mod colors;
pub mod columns;
pub mod confirm;
pub mod members;
pub mod printer;
pub mod repeats;
pub mod settings;
pub mod task;
pub mod templates;
pub mod users;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};

use crate::ui::theme::Theme;

/// Places widgets left to right on one line.
pub struct Row {
    x: u16,
    y: u16,
    right: u16,
}

impl Row {
    pub fn new(area: Rect) -> Self {
        Self {
            x: area.x,
            y: area.y,
            right: area.right(),
        }
    }

    /// Enough columns for this text, plus one of gap. Use this instead of
    /// guessing widths: a label that does not fit is a label nobody can read.
    pub fn text(&mut self, s: &str) -> Rect {
        self.take(width_of(s))
    }

    /// The next `w` columns, plus one column of gap.
    pub fn take(&mut self, w: u16) -> Rect {
        let w = w.min(self.right.saturating_sub(self.x));
        let r = Rect::new(self.x, self.y, w, 1);
        self.x = self.x.saturating_add(w).saturating_add(1);
        r
    }

    pub fn rest(&mut self) -> Rect {
        let r = Rect::new(self.x, self.y, self.right.saturating_sub(self.x), 1);
        self.x = self.right;
        r
    }
}

/// Columns a piece of text needs.
pub fn width_of(s: &str) -> u16 {
    s.chars().count() as u16
}

/// Width a checkbox needs: the `[x] ` marker plus its text.
pub fn check_w(text: &str) -> u16 {
    width_of(text) + 4
}

/// Width a button needs for its label. The labels carry their own padding,
/// and a bordered button needs two columns more.
pub fn button_w(label: &str) -> u16 {
    width_of(label)
}

/// Rows a button takes: three when "bigger buttons" is on, one otherwise.
pub fn button_h(t: &Theme) -> u16 {
    if t.touch { 3 } else { 1 }
}

/// A row of buttons along the bottom of a panel, each as wide as its label.
pub fn button_bar(area: Rect, labels: &[&str], t: &Theme) -> Vec<Rect> {
    let pad = if t.touch { 2 } else { 0 };
    let mut out = Vec::new();
    let mut x = area.x;
    for label in labels {
        let w = button_w(label) + pad;
        if x + w > area.right() {
            break;
        }
        out.push(Rect::new(x, area.y, w, area.height));
        x += w + 1;
    }
    out
}

pub fn label(f: &mut Frame, r: Rect, text: &str, t: &Theme) {
    f.render_widget(Paragraph::new(text).style(t.surface_dim()), r);
}

/// Two buttons at the right edge, each as wide as its own label.
pub fn button_row(right: Rect, yes: &str, no: &str, t: &Theme) -> (Rect, Rect) {
    let pad = if t.touch { 2 } else { 0 };
    let (yw, nw) = (button_w(yes) + pad, button_w(no) + pad);
    let [_, a, _, b] = Layout::horizontal([
        Constraint::Min(0),
        Constraint::Length(yw),
        Constraint::Length(2),
        Constraint::Length(nw),
    ])
    .areas(right);
    (a, b)
}

pub fn popup(area: Rect, w: u16, h: u16) -> Rect {
    let w = w.min(area.width);
    let h = h.min(area.height);
    Rect::new(
        area.x + area.width.saturating_sub(w) / 2,
        area.y + area.height.saturating_sub(h) / 2,
        w,
        h,
    )
}

pub fn frame_block<'a>(title: &'a str, hint: &'a str, t: &Theme) -> Block<'a> {
    Block::default()
        .borders(Borders::ALL)
        .border_set(t.border_set())
        .border_style(t.surface_border())
        .title(Span::styled(title.to_string(), t.surface_title()))
        .title_bottom(Line::from(Span::styled(hint.to_string(), t.surface_dim())).right_aligned())
        .style(t.surface())
}

pub fn frame_block_titled<'a>(title: &'a str, t: &Theme) -> Block<'a> {
    Block::default()
        .borders(Borders::ALL)
        .border_set(t.border_set())
        .border_style(t.surface_border())
        .title(Span::styled(format!(" {title} "), t.surface_title()))
        .style(t.surface())
}

pub fn split_label(r: Rect, label_w: u16) -> (Rect, Rect) {
    let [l, w] = Layout::horizontal([Constraint::Length(label_w), Constraint::Min(1)]).areas(r);
    (l, w)
}

/// Days as typed in a form to seconds, or a message.
pub fn parse_days(text: &str, what: &str) -> Result<i64, String> {
    let days: i64 = text
        .trim()
        .parse()
        .map_err(|_| format!("{what} must be a number of days"))?;
    if days < 0 {
        return Err(format!("{what} cannot be negative"));
    }
    Ok(days * 24 * 60 * 60)
}

pub fn days_text(secs: i64) -> String {
    (secs / (24 * 60 * 60)).to_string()
}
