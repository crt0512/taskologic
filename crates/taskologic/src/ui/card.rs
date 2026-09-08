//! Task and board cards. A card is a small bordered box, so the eye can
//! tell one task from the next without counting lines.

use chrono_tz::Tz;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use taskologic_core::ids::Uid;
use taskologic_core::prefs::CardFields;
use taskologic_core::task::Task;
use taskologic_proto::BoardSummary;

use super::theme::Theme;

/// Everything a card needs that is not on the task itself.
pub struct CardCtx<'a> {
    pub theme: &'a Theme,
    pub fields: CardFields,
    pub tz: Tz,
    pub names: &'a dyn Fn(Uid) -> String,
    /// Open dependencies of a task, and how many it has in total.
    pub deps: &'a dyn Fn(&Task) -> (usize, usize),
    pub finished_col: taskologic_core::ids::ColumnId,
}

/// The lines a card shows under its title. A field that has nothing to say
/// is left out rather than printed as "none", so cards stay as small as
/// their content.
fn detail_count(task: &Task, fields: CardFields, deps_total: usize) -> u16 {
    u16::from(fields.due_date && task.due_at.is_some())
        + u16::from(fields.assignees && !task.assignees.is_empty())
        + u16::from(fields.dependencies && deps_total > 0)
        + u16::from(fields.description && !task.description.trim().is_empty())
}

/// Rows this task's card takes, borders included.
pub fn task_card_height(task: &Task, fields: CardFields, deps_total: usize, touch: bool) -> u16 {
    2 + 1 + detail_count(task, fields, deps_total) + u16::from(touch)
}

fn clip(s: &str, width: usize) -> String {
    let n = s.chars().count();
    if n <= width {
        return s.to_string();
    }
    if width <= 1 {
        return s.chars().take(width).collect();
    }
    let mut out: String = s.chars().take(width - 1).collect();
    out.push('~');
    out
}

/// One task, as a bordered box. `state` marks selection and pickup.
pub fn render_task(
    f: &mut Frame,
    area: Rect,
    task: &Task,
    selected: bool,
    picked: bool,
    ctx: &CardCtx,
) {
    let t = ctx.theme;
    let border = if picked {
        t.card_picked()
    } else {
        t.card_border(selected)
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_set(t.border_set())
        .border_style(border)
        .style(t.card());
    let inner = block.inner(area);
    f.render_widget(block, area);
    if inner.width == 0 || inner.height == 0 {
        return;
    }
    let w = inner.width as usize;
    let mut lines: Vec<Line> = Vec::new();

    let title_style = if selected {
        t.card().add_modifier(ratatui::style::Modifier::BOLD)
    } else {
        t.card()
    };
    let mut title = task.title.clone();
    if ctx.fields.short_id {
        title = format!("{} {}", task.short_id, title);
    }
    lines.push(Line::from(Span::styled(clip(&title, w), title_style)));

    if ctx.fields.due_date
        && let Some(d) = task.due_at
    {
        let due = d.with_timezone(&ctx.tz);
        let overdue = task.column_id != ctx.finished_col && d < chrono::Utc::now();
        let mark = if overdue { "! " } else { "" };
        let style = if overdue {
            t.card()
                .patch(t.severity(taskologic_proto::Severity::Warning))
        } else {
            t.card_dim()
        };
        lines.push(Line::from(Span::styled(
            clip(&format!("{mark}due {}", due.format("%Y-%m-%d %H:%M")), w),
            style,
        )));
    }
    if ctx.fields.assignees && !task.assignees.is_empty() {
        let text = format!(
            "for {}",
            task.assignees
                .iter()
                .map(|u| (ctx.names)(*u))
                .collect::<Vec<_>>()
                .join(", ")
        );
        lines.push(Line::from(Span::styled(clip(&text, w), t.card_dim())));
    }
    if ctx.fields.dependencies {
        let (open, total) = (ctx.deps)(task);
        if total > 0 {
            let text = if open == 0 {
                format!("{total} dependencies, all done")
            } else {
                format!("{open} of {total} dependencies open")
            };
            let style = if open > 0 {
                t.card()
                    .patch(t.severity(taskologic_proto::Severity::Warning))
            } else {
                t.card_dim()
            };
            lines.push(Line::from(Span::styled(clip(&text, w), style)));
        }
    }
    if ctx.fields.description
        && let Some(first) = task.description.lines().find(|l| !l.trim().is_empty())
    {
        lines.push(Line::from(Span::styled(clip(first, w), t.card_dim())));
    }

    lines.truncate(inner.height as usize);
    f.render_widget(Paragraph::new(lines).style(t.card()), inner);
}

/// Rows a board card takes. The description only costs a line when there
/// is one.
pub fn board_card_height(board: &BoardSummary, touch: bool) -> u16 {
    2 + 1 + u16::from(!board.description.trim().is_empty()) + u16::from(touch)
}

/// The tallest a board card gets, for reserving space up front.
pub fn max_board_card_height(touch: bool) -> u16 {
    2 + 2 + u16::from(touch)
}

/// One board on the dashboard.
pub fn render_board(f: &mut Frame, area: Rect, board: &BoardSummary, selected: bool, t: &Theme) {
    // A board card opens a board, so it lights up like a button.
    let hover = t.hovered(area);
    let fill = if hover { t.selected() } else { t.card() };
    let dim = if hover { t.selected() } else { t.card_dim() };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_set(t.border_set())
        .border_style(if hover {
            t.selected()
        } else {
            t.card_border(selected)
        })
        .style(fill);
    let inner = block.inner(area);
    f.render_widget(block, area);
    if inner.width == 0 || inner.height == 0 {
        return;
    }
    let w = inner.width as usize;
    let title_style = if selected || hover {
        fill.add_modifier(ratatui::style::Modifier::BOLD)
    } else {
        fill
    };
    let mut flags = vec![format!(
        "{} task{}",
        board.task_count,
        if board.task_count == 1 { "" } else { "s" }
    )];
    if board.is_private {
        flags.push("private".into());
    }
    if board.is_locked {
        flags.push("locked".into());
    }
    if !board.is_member {
        flags.push("no access".into());
    }
    let mut lines = vec![Line::from(vec![
        Span::styled(clip(&board.name, w), title_style),
        Span::styled("  ", dim),
        Span::styled(
            clip(
                &flags.join("  -  "),
                w.saturating_sub(board.name.chars().count() + 2),
            ),
            dim,
        ),
    ])];
    let description = board.description.trim();
    if !description.is_empty() {
        lines.push(Line::from(Span::styled(clip(description, w), dim)));
    }
    f.render_widget(Paragraph::new(lines).style(fill), inner);
}

/// One search hit, as a card.
pub fn render_hit(
    f: &mut Frame,
    area: Rect,
    hit: &taskologic_proto::SearchHit,
    selected: bool,
    t: &Theme,
) {
    let hover = t.hovered(area);
    let fill = if hover { t.selected() } else { t.card() };
    let dim = if hover { t.selected() } else { t.card_dim() };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_set(t.border_set())
        .border_style(if hover {
            t.selected()
        } else {
            t.card_border(selected)
        })
        .style(fill);
    let inner = block.inner(area);
    f.render_widget(block, area);
    if inner.width == 0 || inner.height == 0 {
        return;
    }
    let w = inner.width as usize;
    let title_style = if selected || hover {
        fill.add_modifier(ratatui::style::Modifier::BOLD)
    } else {
        fill
    };
    let mut where_ = format!("{} / {}", hit.board_name, hit.column_name);
    if hit.archived {
        where_.push_str("  (archived)");
    }
    let lines = vec![
        Line::from(Span::styled(clip(&hit.title, w), title_style)),
        Line::from(Span::styled(clip(&where_, w), dim)),
    ];
    f.render_widget(Paragraph::new(lines).style(fill), inner);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn height_follows_what_the_task_actually_has() {
        use taskologic_core::board::test_support::board_with_members;
        use taskologic_core::task::test_support::task_on;
        let board = board_with_members(1, &[1]);
        let mut task = task_on(&board, 1);
        let f = CardFields::default();
        // Title only: two borders and one line.
        assert_eq!(task_card_height(&task, f, 0, false), 3);
        task.due_at = Some(chrono::Utc::now());
        assert_eq!(task_card_height(&task, f, 0, false), 4);
        task.assignees = vec![1];
        assert_eq!(task_card_height(&task, f, 2, false), 6);
        assert_eq!(task_card_height(&task, f, 2, true), 7);
    }

    #[test]
    fn clipping_marks_what_it_cut() {
        assert_eq!(clip("hello", 10), "hello");
        assert_eq!(clip("hello", 5), "hello");
        assert_eq!(clip("hello", 4), "hel~");
        assert_eq!(clip("hello", 1), "h");
    }
}
