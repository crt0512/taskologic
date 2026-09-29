//! Forms and panels: widget state, focus ring, validation and rendering
//! for one screenful of input. The app owns one as an overlay and forwards
//! raw terminal events to it; it answers with an outcome enum.

pub mod analytics;
pub mod archive;
pub mod board;
pub mod colors;
pub mod columns;
pub mod confirm;
pub mod members;
pub mod print_rules;
pub mod printer;
pub mod program;
pub mod programs;
pub mod repeats;
pub mod runs;
pub mod settings;
pub mod start_program;
pub mod codes;
pub mod step;
pub mod task;
pub mod templates;
pub mod users;

use crossterm::event::{Event, MouseButton, MouseEventKind};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, ListItem};

use crate::ui::adapter::{ListState, MouseFlags, list};
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

/// Where a list's scroll controls were drawn, for taps.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ListArrows {
    pub up: Option<Rect>,
    pub down: Option<Rect>,
}

/// Rows a touch scroll bar takes.
const TOUCH_BAR_ROWS: u16 = 1;
/// Rows between a bar and the list, drawn as a rule, so the arrow does not
/// read as part of the first or last row.
const TOUCH_BAR_GAP: u16 = 1;

/// Draw a list with its scroll controls, the way a board column has them:
/// outside the list, only in the directions that have something off screen.
/// With bigger buttons on, a bar row and a rule are kept at the top and the
/// bottom of the box whether or not there is anything to scroll to, so the
/// list never moves and a finger that lands where an arrow was hits the
/// rule, not the first or last row; the arrow itself is drawn only when the
/// direction has something. Otherwise a `^` and a `v` on a strip under the
/// list. What was off screen is known from the last frame, so the
/// controls settle one frame after the list does.
pub fn list_with_arrows<'a>(
    f: &mut Frame,
    area: Rect,
    items: Vec<ListItem<'a>>,
    state: &mut ListState,
    t: &Theme,
) -> ListArrows {
    let mut out = ListArrows::default();
    let more_above = state.offset() > 0;
    let more_below = state.offset() + state.page_len() < state.rows();
    let mut list_area = area;
    if t.touch {
        let taken = TOUCH_BAR_ROWS + TOUCH_BAR_GAP;
        if area.height > 2 * taken {
            let top = Rect::new(area.x, area.y, area.width, TOUCH_BAR_ROWS);
            rule(f, Rect::new(area.x, top.bottom(), area.width, TOUCH_BAR_GAP), t);
            if more_above {
                touch_bar(f, top, true, t);
                out.up = Some(top);
            }
            let bottom = Rect::new(area.x, area.bottom() - TOUCH_BAR_ROWS, area.width, TOUCH_BAR_ROWS);
            rule(f, Rect::new(area.x, bottom.y - TOUCH_BAR_GAP, area.width, TOUCH_BAR_GAP), t);
            if more_below {
                touch_bar(f, bottom, false, t);
                out.down = Some(bottom);
            }
            list_area = Rect::new(area.x, area.y + taken, area.width, area.height - 2 * taken);
        }
        f.render_stateful_widget(list(items, t), list_area, state);
        return out;
    }
    // A strip under the list for the glyphs, always there so the list does
    // not jump as it fills.
    if area.height > 1 {
        list_area.height -= 1;
    }
    f.render_stateful_widget(list(items, t), list_area, state);
    let y = area.bottom().saturating_sub(1);
    if more_above && area.width >= 8 {
        let r = Rect::new(area.right().saturating_sub(7), y, 3, 1);
        f.render_widget(Paragraph::new(" /\\ ").style(t.hover_if(t.button(), r)), r);
        out.up = Some(r);
    }
    if more_below && area.width >= 4 {
        let r = Rect::new(area.right().saturating_sub(4), y, 3, 1);
        f.render_widget(Paragraph::new(" \\/ ").style(t.hover_if(t.button(), r)), r);
        out.down = Some(r);
    }
    out
}

/// A horizontal rule the width of `area`, in the theme's border glyph so it
/// matches the frames in ASCII and unicode alike.
fn rule(f: &mut Frame, area: Rect, t: &Theme) {
    let glyph = t.border_set().horizontal_top;
    let line = glyph.repeat(area.width as usize);
    for y in area.y..area.bottom() {
        f.render_widget(
            Paragraph::new(line.clone()).style(t.surface_dim()),
            Rect::new(area.x, y, area.width, 1),
        );
    }
}

/// A full width bar with a chevron drawn over both of its rows, pointing
/// the way the list moves when it is tapped. Plain slashes, so it reads on
/// any terminal the frames do.
fn touch_bar(f: &mut Frame, bar: Rect, up: bool, t: &Theme) {
    let style = t.hover_if(t.button(), bar);
    f.render_widget(Block::default().style(style), bar);
    // An arrowhead over a shaft, or the shaft over the head.
    let rows: [&str; 1] = if up { ["/\\"] } else { ["\\/"] };
    for (i, row) in rows.iter().enumerate() {
        f.render_widget(
            Paragraph::new(*row)
                .alignment(ratatui::layout::Alignment::Center)
                .style(style),
            Rect::new(bar.x, bar.y + i as u16, bar.width, 1),
        );
    }
}

/// A tap on one of the controls moves the selection a page that way, which
/// scrolls the list along with it, and says whether it did.
pub fn list_arrow_tap(ev: &Event, arrows: ListArrows, state: &mut ListState) -> bool {
    let Event::Mouse(m) = ev else { return false };
    if !matches!(m.kind, MouseEventKind::Down(_)) {
        return false;
    }
    let pos = Position::new(m.column, m.row);
    let page = state.page_len().max(1);
    if arrows.up.is_some_and(|r| r.contains(pos)) {
        state.move_up(page);
        return true;
    }
    if arrows.down.is_some_and(|r| r.contains(pos)) {
        state.move_down(page);
        return true;
    }
    false
}

/// The click tracker for a list's rows: the timing tracker a board card
/// uses, plus which row the last click was on, so two quick taps on two
/// different rows are two clicks and not a double click on the second.
#[derive(Debug, Default)]
pub struct RowClicks {
    flags: MouseFlags,
    row: Option<usize>,
}

/// A double click on a list row: the row's index, for the panel to select
/// and act on. Both clicks have to land on the same row.
pub fn list_double_click(ev: &Event, clicks: &mut RowClicks, state: &ListState) -> Option<usize> {
    let Event::Mouse(m) = ev else { return None };
    // The tracker walks down, up, down, up and says yes on the last one, so
    // it has to see both halves of every click.
    if !matches!(m.kind, MouseEventKind::Down(MouseButton::Left) | MouseEventKind::Up(MouseButton::Left)) {
        return None;
    }
    let row = state.row_at_clicked((m.column, m.row))?;
    let area = *state.row_areas.get(row.checked_sub(state.offset())?)?;
    if matches!(m.kind, MouseEventKind::Down(_)) && clicks.row != Some(row) {
        // A different row: whatever was half a double click is forgotten.
        // An event outside the area is how the tracker is told to forget.
        clicks.flags.doubleclick(Rect::default(), m);
        clicks.row = Some(row);
    }
    clicks.flags.doubleclick(area, m).then_some(row)
}

/// A press of a mouse button (or a finger) somewhere other than `window`.
/// Panels that are lists use it to close on a click on the board behind
/// them, which is what the click was for. Nothing before the first draw.
pub fn clicked_outside(ev: &Event, window: Rect) -> bool {
    match ev {
        Event::Mouse(m) if matches!(m.kind, MouseEventKind::Down(_)) => {
            window.width > 0 && !window.contains(Position::new(m.column, m.row))
        }
        _ => false,
    }
}

/// A list entry that is as tall as a button: three rows with the text in
/// the middle when "bigger buttons" is on, so a finger lands on it, one
/// row otherwise.
pub fn tall_item<'a>(text: String, t: &Theme) -> ListItem<'a> {
    if t.touch {
        ListItem::new(vec![Line::raw(""), Line::raw(text), Line::raw("")])
    } else {
        ListItem::new(text)
    }
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
