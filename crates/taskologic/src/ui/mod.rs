//! Rendering. `view` reads the app state and draws it, recording the areas
//! mouse handling needs. No decisions are made in here.

pub mod adapter;
pub mod card;
pub mod theme;

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};
use taskologic_core::ids::TaskId;
use taskologic_core::task::Task;

use crate::app::{App, Conn, MIN_HEIGHT, MIN_WIDTH, Overlay};
use crate::forms::task::conflict_lines;
use adapter::{HasScreenCursor, checkbox_at, text_input};
use card::CardCtx;
use theme::Theme;

/// A menu entry: the label as shown, and the key it stands for.
type MenuItem = (&'static str, char);

pub fn view(app: &mut App, f: &mut Frame) {
    let area = f.area();
    if app.too_small() {
        too_small(app, f, area);
        return;
    }
    let t = app.theme();
    f.render_widget(Block::default().style(t.screen()), area);
    match app.conn {
        Conn::Connecting => {
            centered_text(f, area, "Taskologic", "Connecting to the daemon...", &t);
            return;
        }
        Conn::Lost => {
            let text = app
                .conn_error
                .clone()
                .unwrap_or_else(|| "connection lost".into());
            centered_text(
                f,
                area,
                "Taskologic",
                &format!("{text}\n\nPress any key to close the session."),
                &t,
            );
            return;
        }
        Conn::Ready => {}
    }

    let menu_h = if t.touch { 3 } else { 1 };
    let title_h = if t.touch { 3 } else { 1 };
    // While an overlay is up the screen under it is not clickable, so
    // nothing under it may light up under the pointer.
    let bg = if app.overlay.is_some() {
        t.with_mouse(None)
    } else {
        t
    };
    let scan_h = if app.show_scan_button() {
        if t.touch { 3 } else { 1 }
    } else {
        0
    };
    // A blank line above and below the buttons, so the bar, the buttons and
    // the board read as three separate things.
    let [title, _, menu, _, body, scan_row, status] = Layout::vertical([
        Constraint::Length(title_h),
        Constraint::Length(1),
        Constraint::Length(menu_h),
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(scan_h),
        Constraint::Length(1),
    ])
    .areas(area);
    title_bar(app, f, title, &bg);
    menu_bar(app, f, menu, &bg);
    if app.board.is_some() {
        board_body(app, f, body, &bg);
    } else {
        dashboard_body(app, f, body, &bg);
    }
    if scan_h > 0 {
        scan_button(app, f, scan_row, &bg);
    }
    status_bar(app, f, status, &bg);
    overlay(app, f, area, &t);
    // The visual half of the scan bell, for terminals that swallow BEL.
    if app.flash_active() {
        for cell in f.buffer_mut().content.iter_mut() {
            cell.set_style(Style::default().add_modifier(Modifier::REVERSED));
        }
    }
}

fn too_small(app: &App, f: &mut Frame, area: Rect) {
    let text = format!(
        "Terminal too small\n\nCurrent: {}x{}\nRequired: {}x{}\n\nResize the window or press q to quit.",
        app.size.0, app.size.1, MIN_WIDTH, MIN_HEIGHT
    );
    f.render_widget(
        Paragraph::new(text)
            .alignment(Alignment::Center)
            .wrap(Wrap { trim: true }),
        area,
    );
}

fn centered_text(f: &mut Frame, area: Rect, title: &str, text: &str, t: &Theme) {
    let popup = centered(area, 60, 8);
    f.render_widget(Clear, popup);
    let block = surface_block(title, t);
    f.render_widget(
        Paragraph::new(text)
            .block(block)
            .style(t.surface())
            .alignment(Alignment::Center)
            .wrap(Wrap { trim: true }),
        popup,
    );
}

fn surface_block<'a>(title: &'a str, t: &Theme) -> Block<'a> {
    Block::default()
        .borders(Borders::ALL)
        .border_set(t.border_set())
        .border_style(t.surface_border())
        .title(Span::styled(format!(" {title} "), t.surface_title()))
        .style(t.surface())
}

/// The top bar. With board tabs switched on it is the tab strip, so the
/// board name is not written twice; otherwise it names where you are.
fn title_bar(app: &mut App, f: &mut Frame, area: Rect, t: &Theme) {
    f.render_widget(Block::default().style(t.title_bar()), area);
    let who = match &app.user {
        Some(u) if u.is_admin => format!("{} (admin) ", u.username),
        Some(u) => format!("{} ", u.username),
        None => String::new(),
    };
    let who_w = who.chars().count() as u16;
    let right_edge = area.right().saturating_sub(who_w + 1);
    // Its own rect, not the whole bar: a Paragraph repaints everything it is
    // given, which would wipe the highlighted tab behind it.
    let who_area = Rect::new(
        area.right().saturating_sub(who_w),
        area.y + area.height / 2,
        who_w,
        1,
    );
    // Boxed tabs when the bar is tall enough for them.
    let boxed = area.height >= 3;
    let mid = area.y + area.height / 2;
    let clock = clock_stamp(app);
    app.tab_areas.clear();

    if !app.show_tabs() {
        // A bar of its own either side, so the clock reads as its own thing
        // rather than as part of the name or of where you are.
        let stamp = if clock.is_empty() {
            String::new()
        } else {
            format!(" | {clock}")
        };
        let where_ = match &app.board {
            Some(b) => format!(" Taskologic{stamp} | {}", b.detail.board.name),
            None => format!(" Taskologic{stamp} | Dashboard"),
        };
        f.render_widget(
            Paragraph::new(where_).style(t.title_bar()),
            Rect::new(area.x, mid, area.width, 1),
        );
        f.render_widget(Paragraph::new(who).style(t.title_bar()), who_area);
        return;
    }

    let current = app.board.as_ref().map(|b| b.detail.board.id);
    let tabs: Vec<(taskologic_core::ids::BoardId, String)> = app
        .boards
        .iter()
        .filter(|b| b.is_member)
        .map(|b| (b.id, b.name.clone()))
        .collect();
    let label = " Taskologic ";
    f.render_widget(
        Paragraph::new(label).style(t.title_bar().add_modifier(Modifier::BOLD)),
        Rect::new(area.x, mid, label.len() as u16, 1),
    );
    let mut x = area.x + label.len() as u16;
    // The label already ends in a space, so the bar goes straight on. Boxed
    // tabs need an explicit trailing divider: their border belongs to the
    // tab and does not read as part of the date/time indicator.
    let stamp = if clock.is_empty() {
        String::new()
    } else if boxed {
        format!("| {clock} | ")
    } else {
        format!("| {clock} ")
    };
    let stamp_w = stamp.chars().count() as u16;
    // Nothing may run into the username, so a clock without room is left out
    // rather than clipped.
    if stamp_w > 0 && x + stamp_w <= right_edge {
        f.render_widget(
            Paragraph::new(stamp).style(t.title_bar()),
            Rect::new(x, mid, stamp_w, 1),
        );
        x += stamp_w;
    }
    for (id, name) in tabs.iter() {
        // Boxed tabs carry their own separator, flat ones need a bar.
        if !boxed {
            if x + 1 > right_edge {
                break;
            }
            f.render_widget(
                Paragraph::new("|").style(t.title_bar()),
                Rect::new(x, mid, 1, 1),
            );
            x += 1;
        }
        // Two columns for the padding, two more for the border when boxed.
        let w = name.chars().count() as u16 + if boxed { 4 } else { 2 };
        if x + w > right_edge {
            // Say that some tabs did not fit rather than cutting silently.
            if x + 3 <= right_edge {
                f.render_widget(
                    Paragraph::new("...").style(t.title_bar()),
                    Rect::new(x, mid, 3, 1),
                );
            }
            break;
        }
        let rect = Rect::new(x, area.y, w, area.height);
        let base = if Some(*id) == current {
            t.selected()
        } else {
            t.title_bar()
        };
        let style = t.hover_if(base, rect);
        if boxed {
            let block = Block::default()
                .borders(Borders::ALL)
                .border_set(t.border_set())
                .style(style);
            let inner = block.inner(rect);
            f.render_widget(block, rect);
            f.render_widget(Paragraph::new(format!(" {name} ")).style(style), inner);
        } else {
            f.render_widget(Paragraph::new(format!(" {name} ")).style(style), rect);
        }
        app.tab_areas.push((rect, *id));
        x += w;
    }
    f.render_widget(Paragraph::new(who).style(t.title_bar()), who_area);
}

/// The date and clock that sit right of the Taskologic label, with no
/// padding of their own: the two callers sit against different things and
/// pad to suit. Empty unless the user asked for one of them. The client
/// redraws on a 250 ms tick, so the seconds move along on their own and
/// nothing here needs a timer.
fn clock_stamp(app: &App) -> String {
    let Some(user) = app.user.as_ref() else {
        return String::new();
    };
    let now = chrono::Utc::now().with_timezone(&user.timezone);
    let mut parts = Vec::new();
    if user.prefs.ui.show_date {
        parts.push(now.format("%Y-%m-%d").to_string());
    }
    if user.prefs.ui.show_time {
        parts.push(now.format("%H:%M:%S").to_string());
    }
    parts.join(" ")
}

/// What a menu button actually shows. When the shortcut is not a letter of
/// the label, it goes in front, so the drawn width is not the label width.
fn button_text(label: &str, key: char) -> String {
    if label.chars().any(|c| c.eq_ignore_ascii_case(&key)) {
        label.to_string()
    } else {
        format!("{key} {label}")
    }
}

/// The drawn text with its shortcut letter picked out, so a button reads as
/// a button and the key is obvious.
fn button_line(text: &str, key: char, base: Style, key_style: Style) -> Line<'static> {
    let pos = text.char_indices().find(|(_, c)| *c == key).or_else(|| {
        text.char_indices()
            .find(|(_, c)| c.eq_ignore_ascii_case(&key))
    });
    match pos {
        Some((i, c)) => {
            let (before, rest) = text.split_at(i);
            let after: String = rest.chars().skip(1).collect();
            Line::from(vec![
                Span::styled(format!(" {before}"), base),
                Span::styled(c.to_string(), key_style),
                Span::styled(format!("{after} "), base),
            ])
        }
        None => Line::from(Span::styled(format!(" {text} "), base)),
    }
}

/// The buttons that always apply, left to right. Anything that only makes
/// sense with a task selected appears with one, and the rest lives in the
/// More menu so the bar stays short.
fn menu_items(app: &App) -> Vec<MenuItem> {
    let has_task = app
        .board
        .as_ref()
        .is_some_and(|b| b.selected_task().is_some());
    if app.board.is_some() {
        let mut v: Vec<MenuItem> = vec![("Boards", 'b'), ("New task", 'n'), ("Templates", 'T')];
        if has_task {
            v.push(("Edit", 'e'));
            v.push(("Delete", 'd'));
            if app.show_print() {
                v.push(("Print", 'p'));
            }
        }
        v.push(("Help", '?'));
        v
    } else {
        vec![("New board", 'N'), ("Search", '/'), ("Help", '?')]
    }
}

/// What the More menu offers here. Everything that is used now and then
/// rather than constantly.
pub fn more_items(app: &App) -> Vec<(String, char)> {
    let privileged = app.privileged_on_open_board();
    let admin = app.user.as_ref().is_some_and(|u| u.is_admin);
    let mut v: Vec<(String, char)> = Vec::new();
    if app.board.is_some() {
        v.push(("Archive".into(), 'a'));
        v.push(("Members".into(), 'm'));
        v.push(("Repeating tasks".into(), 'R'));
        if privileged {
            v.push(("Columns".into(), 'c'));
            v.push(("Board settings".into(), 'B'));
        }
    } else {
        v.push(("Search".into(), '/'));
    }
    if admin {
        v.push(("Users and admins".into(), 'u'));
    }
    v.push(("Settings".into(), 'S'));
    v.push(("Quit".into(), 'q'));
    v
}

fn menu_bar(app: &mut App, f: &mut Frame, area: Rect, t: &Theme) {
    let items = menu_items(app);
    app.menu_areas.clear();
    let back = app.board.is_some();

    // More sits at the right edge, so reserve its space before laying out.
    let more = "More";
    let more_w = more.chars().count() as u16 + if t.touch { 4 } else { 2 };
    let more_rect = Rect::new(
        area.right().saturating_sub(more_w + 1),
        area.y,
        more_w,
        area.height,
    );
    let limit = more_rect.x.saturating_sub(1);

    let mut x = area.x + 1;
    for (i, (label, key)) in items.iter().enumerate() {
        let mut text = button_text(label, *key);
        if back && i == 0 {
            text = format!("< {text}");
        }
        // The rect must match what gets drawn, or clicks land on the wrong
        // button and the labels overlap. The border needs two more columns.
        let w = text.chars().count() as u16 + if t.touch { 4 } else { 2 };
        if x + w > limit {
            break;
        }
        let rect = Rect::new(x, area.y, w, area.height);
        draw_menu_button(f, rect, &text, *key, t);
        app.menu_areas.push((rect, *key));
        x += w + 1;
    }
    draw_menu_button(f, more_rect, more, 'M', t);
    app.menu_areas.push((more_rect, 'M'));
}

/// One menu button, bordered when there is room for a border.
fn draw_menu_button(f: &mut Frame, rect: Rect, text: &str, key: char, t: &Theme) {
    let base = t.hover_if(t.button(), rect);
    let keys = t.hover_if(t.button_key(), rect);
    if rect.height >= 3 {
        let block = Block::default()
            .borders(Borders::ALL)
            .border_set(t.border_set())
            .style(base);
        let inner = block.inner(rect);
        f.render_widget(block, rect);
        f.render_widget(
            Paragraph::new(button_line(text, key, base, keys)).style(base),
            inner,
        );
    } else {
        f.render_widget(
            Paragraph::new(button_line(text, key, base, keys)).style(base),
            rect,
        );
    }
}

fn dashboard_body(app: &mut App, f: &mut Frame, area: Rect, t: &Theme) {
    // A column of air between the cards and the terminal edges.
    let area = Rect::new(
        area.x + 1,
        area.y,
        area.width.saturating_sub(2),
        area.height,
    );
    let [search_row, list] =
        Layout::vertical([Constraint::Length(3), Constraint::Min(1)]).areas(area);
    let [search, arch] =
        Layout::horizontal([Constraint::Min(1), Constraint::Length(26)]).areas(search_row);
    app.search_area = search;
    f.render_stateful_widget(text_input("Search tasks", t), search, &mut app.search);
    if let Some((x, y)) = app.search.screen_cursor() {
        f.set_cursor_position((x, y));
    }
    let arch_box = Rect::new(arch.x + 1, arch.y + 1, arch.width.saturating_sub(1), 1);
    f.render_stateful_widget(
        checkbox_at("include archived (i)".into(), arch_box, t),
        arch_box,
        &mut app.include_archived,
    );

    let searching = !app.hits.is_empty();
    let heading = if searching {
        "Search results (Esc clears)"
    } else {
        "Boards"
    };
    f.render_widget(
        Paragraph::new(Span::styled(format!(" {heading}"), t.title())),
        Rect::new(list.x, list.y, list.width, 1),
    );
    let list = Rect::new(
        list.x,
        list.y + 1,
        list.width,
        list.height.saturating_sub(1),
    );

    app.board_areas.clear();
    app.hit_areas.clear();
    // One blank line between cards, so they do not run into each other.
    let gap = 1;
    let h = card::max_board_card_height(t.touch);
    let per_page = (list.height / (h + gap)).max(1) as usize;
    let len = if searching {
        app.hits.len()
    } else {
        app.boards.len()
    };
    let sel = if searching {
        app.hit_sel
    } else {
        app.board_sel
    };
    if len == 0 {
        let text = if searching {
            "Nothing matched."
        } else {
            "No boards yet. Press N to create one."
        };
        f.render_widget(Paragraph::new(text).style(t.dim()), list);
        return;
    }
    // Keep the selection on screen. per_page uses the tallest card, so at
    // least that many always fit.
    if sel < app.dash_scroll {
        app.dash_scroll = sel;
    } else if sel >= app.dash_scroll + per_page {
        app.dash_scroll = sel + 1 - per_page;
    }
    app.dash_scroll = app.dash_scroll.min(len.saturating_sub(per_page));

    // One card per row: Up and Down walk the list exactly as it is drawn.
    let mut y = list.y;
    let mut shown = 0;
    for i in app.dash_scroll..len {
        let card_h = if searching {
            h
        } else {
            card::board_card_height(&app.boards[i], t.touch)
        };
        if y + card_h > list.bottom() {
            break;
        }
        let rect = Rect::new(list.x, y, list.width, card_h);
        if searching {
            card::render_hit(f, rect, &app.hits[i], i == sel, t);
            app.hit_areas.push(rect);
        } else {
            card::render_board(f, rect, &app.boards[i], i == sel, t);
            app.board_areas.push(rect);
        }
        y += card_h + gap;
        shown += 1;
    }
    if app.dash_scroll + shown < len {
        let more = format!("{} more below", len - (app.dash_scroll + shown));
        f.render_widget(
            Paragraph::new(more)
                .style(t.dim())
                .alignment(Alignment::Right),
            Rect::new(list.x, list.bottom().saturating_sub(1), list.width, 1),
        );
    }
}

fn board_body(app: &mut App, f: &mut Frame, area: Rect, t: &Theme) {
    let fields = app.card_fields();
    let tz = app
        .user
        .as_ref()
        .map(|u| u.timezone)
        .unwrap_or(chrono_tz::UTC);
    let names: std::collections::HashMap<taskologic_core::ids::Uid, String> = app.users.clone();
    let all: Vec<Task> = app
        .board
        .as_ref()
        .map(|b| b.detail.tasks.clone())
        .unwrap_or_default();
    let finished_col = app
        .board
        .as_ref()
        .map(|b| b.detail.board.finished_col)
        .unwrap_or(taskologic_core::ids::ColumnId(0));
    let deps = move |task: &Task| -> (usize, usize) {
        let total = task.depends_on.len();
        let open = task
            .depends_on
            .iter()
            .filter(|d| match all.iter().find(|x| x.id == **d) {
                Some(x) => !x.is_archived() && x.column_id != finished_col,
                // On another board: assume open, the daemon has the truth.
                None => true,
            })
            .count();
        (open, total)
    };
    let ctx = CardCtx {
        theme: t,
        fields,
        tz,
        names: &|u| names.get(&u).cloned().unwrap_or_else(|| format!("uid {u}")),
        deps: &deps,
        finished_col,
    };

    let Some(b) = &mut app.board else { return };
    b.area = area;
    b.column_areas.clear();
    b.plus_areas.clear();
    b.sort_areas.clear();
    b.up_areas.clear();
    b.down_areas.clear();
    b.task_areas.clear();
    b.left_arrow = None;
    b.right_arrow = None;

    let total = b.column_count();
    if total == 0 {
        f.render_widget(
            Paragraph::new("This board has no columns. Press c to add one.").style(t.dim()),
            area,
        );
        return;
    }
    let min_w = if t.touch { 28 } else { 24 };
    let visible = ((area.width / min_w) as usize).clamp(1, total);
    if b.col < b.col_offset {
        b.col_offset = b.col;
    } else if b.col >= b.col_offset + visible {
        b.col_offset = b.col + 1 - visible;
    }
    b.col_offset = b.col_offset.min(total - visible);

    // Arrow gutters only when there is something off screen.
    let scrolls = visible < total;
    let gutter = if scrolls { 3 } else { 0 };
    let [left_g, strip, right_g] = Layout::horizontal([
        Constraint::Length(gutter),
        Constraint::Min(1),
        Constraint::Length(gutter),
    ])
    .areas(area);
    if scrolls {
        let y = area.y + area.height / 2;
        if b.col_offset > 0 {
            let r = Rect::new(left_g.x, y, 3, 1);
            f.render_widget(Paragraph::new(" < ").style(t.hover_if(t.button(), r)), r);
            b.left_arrow = Some(r);
        }
        if b.col_offset + visible < total {
            let r = Rect::new(right_g.x, y, 3, 1);
            f.render_widget(Paragraph::new(" > ").style(t.hover_if(t.button(), r)), r);
            b.right_arrow = Some(r);
        }
    }

    let slots =
        Layout::horizontal(vec![Constraint::Ratio(1, visible as u32); visible]).split(strip);
    let board = b.detail.board.clone();
    for (slot_idx, slot) in slots.iter().enumerate() {
        let idx = b.col_offset + slot_idx;
        let column = &board.columns[idx];
        let tasks: Vec<Task> = b.tasks_in(idx).into_iter().cloned().collect();
        let selected_col = idx == b.col;
        let hovered = b.hover_col == Some(idx) && b.drag.is_some();
        let roles: String = board
            .roles_of(column.id)
            .iter()
            .map(|r| t.role_glyph(*r))
            .collect::<Vec<_>>()
            .join("");
        let head = format!(
            " {}{}{} ({}) ",
            roles,
            if roles.is_empty() { "" } else { " " },
            column.name,
            tasks.len()
        );
        let border_style = if hovered {
            t.drop_target()
        } else {
            t.column_border(selected_col)
        };
        let sort_label = if column.sort_by_due {
            " by due "
        } else {
            " manual "
        };
        let plus_rect = Rect::new(slot.right().saturating_sub(4), slot.y, 3, 1);
        let sort_rect = Rect::new(
            slot.x + 1,
            slot.bottom().saturating_sub(1),
            sort_label.len() as u16,
            1,
        );
        let block = Block::default()
            .borders(Borders::ALL)
            .border_set(t.border_set())
            .border_style(border_style)
            .title(Span::styled(
                head,
                if selected_col { t.title() } else { t.base() },
            ))
            .title_top(
                Line::from(Span::styled(" + ", t.hover_if(t.button(), plus_rect))).right_aligned(),
            )
            .title_bottom(Line::from(Span::styled(
                sort_label,
                t.hover_if(t.dim(), sort_rect),
            )));
        let inner = block.inner(*slot);
        f.render_widget(block, *slot);
        b.column_areas.push(*slot);
        b.plus_areas.push((idx, plus_rect));
        b.sort_areas.push((idx, sort_rect));

        // Cards are as tall as their own content, so how many fit depends
        // on which ones they are. Each carries a blank line under it, so
        // consecutive cards do not share a border line.
        let gap = 1u16;
        let heights: Vec<u16> = tasks
            .iter()
            .map(|task| card::task_card_height(task, fields, (deps)(task).1, t.touch))
            .collect();
        let fits_from = |start: usize| -> usize {
            let mut used = 0;
            let mut n = 0;
            for h in heights.iter().skip(start) {
                if used + h > inner.height {
                    break;
                }
                used += h + gap;
                n += 1;
            }
            n.max(1)
        };
        while b.col_scroll.len() <= idx {
            b.col_scroll.push(0);
        }
        if selected_col {
            if b.row < b.col_scroll[idx] {
                b.col_scroll[idx] = b.row;
            } else {
                // Scroll down until the selected card is on screen.
                while b.row >= b.col_scroll[idx] + fits_from(b.col_scroll[idx]) {
                    b.col_scroll[idx] += 1;
                }
            }
        }
        b.col_scroll[idx] = b.col_scroll[idx].min(tasks.len().saturating_sub(1).max(0));
        let offset = b.col_scroll[idx];

        let mut areas = Vec::new();
        let mut y = inner.y;
        let mut shown = 0;
        for (i, task) in tasks.iter().enumerate().skip(offset) {
            let h = heights[i];
            if y + h > inner.bottom() {
                break;
            }
            let rect = Rect::new(inner.x, y, inner.width, h);
            let is_selected = selected_col && i == b.row;
            let is_picked = b.picked == Some(task.id) || b.drag.map(|d| d.0) == Some(task.id);
            card::render_task(f, rect, task, is_selected, is_picked, &ctx);
            areas.push((task.id, rect));
            y += h + gap;
            shown += 1;
        }
        b.task_areas.push(areas);

        if offset > 0 {
            let r = Rect::new(
                slot.right().saturating_sub(7),
                slot.bottom().saturating_sub(1),
                3,
                1,
            );
            f.render_widget(Paragraph::new(" ^ ").style(t.hover_if(t.button(), r)), r);
            b.up_areas.push((idx, r));
        }
        if offset + shown < tasks.len() {
            let r = Rect::new(
                slot.right().saturating_sub(4),
                slot.bottom().saturating_sub(1),
                3,
                1,
            );
            f.render_widget(Paragraph::new(" v ").style(t.hover_if(t.button(), r)), r);
            b.down_areas.push((idx, r));
        }
    }
}

/// Full width so it is easy to hit on a touchscreen. Clicking it does what
/// `s` does: toggle manual scan mode.
fn scan_button(app: &mut App, f: &mut Frame, area: Rect, t: &Theme) {
    let text = if app.manual_scan {
        "Scanning, present a barcode now. Click or press s to stop."
    } else {
        "Scan a barcode (s)"
    };
    let base = if app.manual_scan {
        t.selected()
    } else {
        t.button()
    };
    let style = t.hover_if(base, area);
    f.render_widget(Block::default().style(style), area);
    let mid = area.y + area.height / 2;
    f.render_widget(
        Paragraph::new(text)
            .alignment(Alignment::Center)
            .style(style),
        Rect::new(area.x, mid, area.width, 1),
    );
    app.menu_areas.push((area, 's'));
}

fn status_bar(app: &App, f: &mut Frame, area: Rect, t: &Theme) {
    if let Some(toast) = &app.toast {
        f.render_widget(
            Paragraph::new(format!(" {} ", toast.text)).style(t.severity(toast.severity)),
            area,
        );
        return;
    }
    let mut parts: Vec<&str> = Vec::new();
    if app.board.as_ref().is_some_and(|b| b.picked.is_some()) {
        parts.push("arrows move the task");
        parts.push("Space drops it");
        parts.push("Esc cancels");
    } else if app.board.is_some() {
        parts.push("arrows move");
        parts.push("Space picks up");
        parts.push("Enter opens");
        parts.push("d deletes");
    } else {
        parts.push("Enter opens a board");
        parts.push("D deletes it");
        parts.push("/ searches");
    }
    if app.scanner_listening() {
        parts.push("[scanner listening]");
    }
    if app.text_focused() {
        parts.push("Esc leaves the field");
    }
    f.render_widget(
        Paragraph::new(format!(" {}", parts.join("   "))).style(t.dim()),
        area,
    );
}

fn overlay(app: &mut App, f: &mut Frame, area: Rect, t: &Theme) {
    let dep_titles: std::collections::HashMap<TaskId, String> = app
        .board
        .as_ref()
        .map(|b| {
            b.detail
                .tasks
                .iter()
                .map(|x| (x.id, x.title.clone()))
                .collect()
        })
        .unwrap_or_default();
    let users = app.users.clone();
    let name_of = move |uid: taskologic_core::ids::Uid| {
        users
            .get(&uid)
            .cloned()
            .unwrap_or_else(|| format!("uid {uid}"))
    };
    let tz = app
        .user
        .as_ref()
        .map(|u| u.timezone)
        .unwrap_or(chrono_tz::UTC);
    let Some(ov) = &mut app.overlay else { return };
    match ov {
        Overlay::Help => {
            let half = HELP.len().div_ceil(2);
            // Columns sized from the text, so neither overwrites the other.
            // A terminal at the 80 column minimum clips the right one.
            let lw = HELP[..half]
                .iter()
                .map(|l| l.chars().count())
                .max()
                .unwrap_or(0) as u16;
            let rw = HELP[half..]
                .iter()
                .map(|l| l.chars().count())
                .max()
                .unwrap_or(0) as u16;
            let popup = centered(area, lw + rw + 6, half as u16 + 2);
            f.render_widget(Clear, popup);
            let b = surface_block("Keys", t);
            let inner = b.inner(popup);
            f.render_widget(b, popup);
            let [_, left, _, right] = Layout::horizontal([
                Constraint::Length(1),
                Constraint::Length(lw),
                Constraint::Length(2),
                Constraint::Min(1),
            ])
            .areas(inner);
            f.render_widget(
                Paragraph::new(HELP[..half].join("\n")).style(t.surface()),
                left,
            );
            f.render_widget(
                Paragraph::new(HELP[half..].join("\n")).style(t.surface()),
                right,
            );
        }
        Overlay::Confirm { dialog, .. } => dialog.render(f, area, t),
        Overlay::Menu { items, sel, areas } => {
            let w = items
                .iter()
                .map(|(l, _)| l.chars().count())
                .max()
                .unwrap_or(4) as u16
                + 6;
            let row_h = if t.touch { 3 } else { 1 };
            let h = items.len() as u16 * row_h + 2;
            // Under the More button it came from, at the right hand edge.
            let x = area.right().saturating_sub(w + 1);
            let y = (area.y + if t.touch { 4 } else { 2 }).min(area.bottom().saturating_sub(h));
            let popup = Rect::new(x, y, w.min(area.width), h.min(area.height));
            f.render_widget(Clear, popup);
            let block = Block::default()
                .borders(Borders::ALL)
                .border_set(t.border_set())
                .border_style(t.button())
                .style(t.button());
            let inner = block.inner(popup);
            f.render_widget(block, popup);
            areas.clear();
            for (i, (label, key)) in items.iter().enumerate() {
                let top = inner.y + i as u16 * row_h;
                if top + row_h > inner.bottom() {
                    break;
                }
                let rect = Rect::new(inner.x, top, inner.width, row_h);
                let style = if Some(i) == *sel || t.hovered(rect) {
                    t.selected()
                } else {
                    t.button()
                };
                // Fill the whole row so it reads as a button, not a line of text.
                f.render_widget(Block::default().style(style), rect);
                f.render_widget(
                    Paragraph::new(format!(" {key}  {label}")).style(style),
                    Rect::new(rect.x, rect.y + row_h / 2, rect.width, 1),
                );
                areas.push(rect);
            }
        }
        Overlay::Colors { form, .. } => form.render(f, area, t),
        Overlay::Printer { form, .. } => form.render(f, area, t),
        Overlay::TaskForm(form) => form.render(f, area, t),
        Overlay::Settings(form) => form.render(f, area, t),
        Overlay::BoardForm(form) => form.render(f, area, t),
        Overlay::Members(panel) => panel.render(f, area, t),
        Overlay::Columns(panel) => panel.render(f, area, t),
        Overlay::Users(panel) => panel.render(f, area, t),
        Overlay::Templates(panel) => panel.render(f, area, t),
        Overlay::Repeats(panel) => panel.render(f, area, t),
        Overlay::PickColumn {
            title,
            options,
            sel,
            ..
        } => {
            let mut lines: Vec<Line> = vec![
                Line::from(Span::styled(title.clone(), t.surface())),
                Line::from(""),
            ];
            for (i, (_, name)) in options.iter().enumerate() {
                let marker = if i == *sel { "> " } else { "  " };
                let style = if i == *sel { t.selected() } else { t.surface() };
                lines.push(Line::from(Span::styled(format!("{marker}{name}"), style)));
            }
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "Enter picks   Esc goes back",
                t.surface_dim(),
            )));
            let popup = centered(area, 60, lines.len() as u16 + 2);
            f.render_widget(Clear, popup);
            f.render_widget(
                Paragraph::new(lines)
                    .wrap(Wrap { trim: false })
                    .block(surface_block("Remove column", t))
                    .style(t.surface()),
                popup,
            );
        }
        Overlay::Conflict { form, current } => {
            let mut lines = vec![
                "Somebody saved this task while you were editing it.".to_string(),
                String::new(),
            ];
            lines.extend(conflict_lines(form, current, &name_of));
            lines.push(String::new());
            lines.push("[r] reload theirs   [o] overwrite with yours   [Esc] keep editing".into());
            let popup = centered(area, 74, lines.len() as u16 + 2);
            f.render_widget(Clear, popup);
            f.render_widget(
                Paragraph::new(lines.join("\n"))
                    .wrap(Wrap { trim: false })
                    .block(surface_block("Task changed", t))
                    .style(t.surface()),
                popup,
            );
        }
        Overlay::TaskDetail {
            task,
            sel,
            item_areas,
            area: popup_area,
        } => {
            let mut lines = vec![
                Line::from(Span::styled(
                    task.title.clone(),
                    t.surface().add_modifier(Modifier::BOLD),
                )),
                Line::from(Span::styled(
                    format!("version {}", task.version),
                    t.surface_dim(),
                )),
            ];
            if let Some(due) = task.due_at {
                lines.push(Line::from(format!(
                    "Due: {}",
                    due.with_timezone(&tz).format("%Y-%m-%d %H:%M")
                )));
            }
            let assignees: Vec<String> = task.assignees.iter().map(|u| name_of(*u)).collect();
            if !assignees.is_empty() {
                lines.push(Line::from(format!("Assigned to: {}", assignees.join(", "))));
            }
            lines.push(Line::from(format!(
                "Created by: {}",
                name_of(task.created_by)
            )));
            if !task.depends_on.is_empty() {
                lines.push(Line::from("Depends on:"));
                for d in &task.depends_on {
                    let text = match dep_titles.get(d) {
                        Some(t) => format!("  {t}"),
                        None => "  a task on another board".to_string(),
                    };
                    lines.push(Line::from(text));
                }
            }
            if !task.description.trim().is_empty() {
                lines.push(Line::from(""));
                for l in task.description.lines() {
                    lines.push(Line::from(l.to_string()));
                }
            }
            // The checklist starts after everything above; its row rects are
            // recorded so clicks land on the right item.
            let checklist_start = if task.checklist.is_empty() {
                lines.len()
            } else {
                lines.push(Line::from(""));
                let done = task.checklist.iter().filter(|c| c.done).count();
                lines.push(Line::from(Span::styled(
                    format!("Checklist ({done}/{})", task.checklist.len()),
                    t.surface().add_modifier(Modifier::BOLD),
                )));
                let start = lines.len();
                for (i, item) in task.checklist.iter().enumerate() {
                    let mark = if item.done { "[x]" } else { "[ ]" };
                    let style = if i == *sel { t.selected() } else { t.surface() };
                    lines.push(Line::from(Span::styled(
                        format!(" {mark} {}", item.text),
                        style,
                    )));
                }
                lines.push(Line::from(""));
                lines.push(Line::from(Span::styled(
                    "Space ticks   Esc closes",
                    t.surface_dim(),
                )));
                start
            };
            let h = (lines.len() as u16 + 2).max(16);
            let popup = centered(area, 70, h);
            f.render_widget(Clear, popup);
            let block = surface_block("Task", t);
            let inner = block.inner(popup);
            f.render_widget(block, popup);
            // No wrapping: the checklist rows below are hit tested by line
            // number, and a wrapped description would shift them.
            f.render_widget(Paragraph::new(lines).style(t.surface()), inner);
            *popup_area = popup;
            item_areas.clear();
            for i in 0..task.checklist.len() {
                let y = inner.y + (checklist_start + i) as u16;
                if y < inner.bottom() {
                    item_areas.push(Rect::new(inner.x, y, inner.width, 1));
                }
            }
        }
        Overlay::Archive(panel) => panel.render(f, area, t),
    }
}

const HELP: &[&str] = &[
    "Arrows / hjkl   Move selection",
    "Tab / Shift+Tab Next / previous column",
    "Enter           Open task or board",
    "Space           Pick up task, again to drop",
    "Arrows          Move the picked up task",
    "n               New task in current column",
    "e               Edit selected task",
    "d               Delete task, to the archive",
    "a               Archive (v views, D purges)",
    "t               Sort column by due date",
    "/               Search   i  include archived",
    "N               New board (dashboard)",
    "D               Delete board (dashboard)",
    "B               Board settings (owner, admin)",
    "m               Members",
    "c               Columns (owner, admin)",
    "S               Settings",
    "u               Users and admins (admin)",
    "b               Back to the dashboard",
    "M               More menu",
    "s               Manual scan mode, if enabled",
    "p               Print selected task, if enabled",
    "F2              Save the open form",
    "?               This overlay",
    "Esc             Close overlay / cancel",
    "q               Quit (confirm)",
];

pub fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let w = width.min(area.width);
    let h = height.min(area.height);
    Rect::new(
        area.x + (area.width - w) / 2,
        area.y + (area.height - h) / 2,
        w,
        h,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::test_support::{open_board, press, ready_app};
    use crossterm::event::KeyCode;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn render(app: &mut App, w: u16, h: u16) -> String {
        app.size = (w, h);
        let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
        t.draw(|f| view(app, f)).unwrap();
        t.backend().to_string()
    }

    #[test]
    fn too_small_says_what_is_needed() {
        let mut app = ready_app(false);
        let out = render(&mut app, 60, 20);
        assert!(out.contains("Terminal too small"));
        assert!(out.contains("60x20"));
        assert!(out.contains("80x24"));
    }

    #[test]
    fn dashboard_snapshot() {
        let mut app = ready_app(false);
        insta::assert_snapshot!(render(&mut app, 100, 24));
    }

    #[test]
    fn board_snapshot() {
        let mut app = ready_app(true);
        open_board(&mut app);
        press(&mut app, KeyCode::Down);
        insta::assert_snapshot!(render(&mut app, 100, 24));
    }

    #[test]
    fn help_overlay_snapshot() {
        let mut app = ready_app(false);
        press(&mut app, KeyCode::Char('?'));
        insta::assert_snapshot!(render(&mut app, 100, 24));
    }

    /// True where `line` holds a run matching `shape`, in which `d` stands
    /// for any digit and everything else is itself.
    fn contains_shape(line: &str, shape: &str) -> bool {
        let line: Vec<char> = line.chars().collect();
        let shape: Vec<char> = shape.chars().collect();
        line.windows(shape.len()).any(|w| {
            w.iter().zip(&shape).all(|(c, s)| {
                if *s == 'd' {
                    c.is_ascii_digit()
                } else {
                    c == s
                }
            })
        })
    }

    #[test]
    fn the_top_bar_carries_the_date_and_the_clock_when_asked() {
        // A snapshot of a running clock would fail a second later, so this
        // looks at the shape of what was drawn.
        let mut app = ready_app(false);
        if let Some(u) = app.user.as_mut() {
            u.prefs.ui.show_date = true;
            u.prefs.ui.show_time = true;
        }
        let top = |out: String| out.lines().next().unwrap_or_default().to_string();
        let plain = top(render(&mut app, 100, 24));
        assert!(contains_shape(&plain, "dddd-dd-dd"), "{plain}");
        assert!(contains_shape(&plain, "dd:dd:dd"), "{plain}");
        // The other branch of the bar: tabs only show inside a board.
        open_board(&mut app);
        let tabbed = top(render(&mut app, 100, 24));
        assert!(contains_shape(&tabbed, "dddd-dd-dd"), "{tabbed}");
        assert!(contains_shape(&tabbed, "dd:dd:dd"), "{tabbed}");
        assert!(tabbed.contains("Kitchen"), "{tabbed}");
        assert!(tabbed.contains("alice (admin)"), "{tabbed}");
    }

    #[test]
    fn boxed_board_tabs_keep_the_divider_after_the_clock() {
        let mut app = ready_app(false);
        if let Some(u) = app.user.as_mut() {
            u.prefs.ui.touchscreen = true;
            u.prefs.ui.show_date = true;
            u.prefs.ui.show_time = true;
        }
        open_board(&mut app);

        // The boxed title bar is three rows tall; its labels are in the
        // middle row. Keep the clock dynamic while checking its punctuation.
        let out = render(&mut app, 100, 24);
        let middle = out.lines().nth(1).unwrap_or_default();
        assert!(
            contains_shape(middle, "dddd-dd-dd dd:dd:dd | │ Kitchen"),
            "{middle}"
        );
    }

    #[test]
    fn board_name_uses_a_bar_when_tabs_are_disabled() {
        let mut app = ready_app(false);
        if let Some(u) = app.user.as_mut() {
            u.prefs.ui.show_board_tabs = false;
        }
        open_board(&mut app);
        let board_name = app.board.as_ref().unwrap().detail.board.name.clone();

        let out = render(&mut app, 100, 24);
        let top = out.lines().next().unwrap_or_default();
        assert!(top.contains(&format!("Taskologic | {board_name}")), "{top}");
        assert!(!top.contains(" - "), "{top}");
    }

    #[test]
    fn ascii_mode_uses_no_box_drawing() {
        let mut app = ready_app(false);
        app.ascii = true;
        open_board(&mut app);
        let out = render(&mut app, 100, 24);
        assert!(out.is_ascii(), "{out}");
    }

    #[test]
    fn cards_show_due_date_assignees_and_dependencies_but_no_short_id() {
        let mut app = ready_app(false);
        open_board(&mut app);
        // Cards leave out what the task does not have, so give one task
        // everything and check the rest stay bare.
        {
            let b = app.board.as_mut().unwrap();
            let t = &mut b.detail.tasks[0];
            t.due_at = Some(chrono::DateTime::from_timestamp(0, 0).unwrap());
            t.assignees = vec![1];
            let dep = b.detail.tasks[3].id;
            b.detail.tasks[0].depends_on = vec![dep];
        }
        let out = render(&mut app, 100, 24);
        assert!(out.contains("Water the plants"));
        assert!(out.contains("due 1970-01-01"), "{out}");
        assert!(out.contains("for "), "{out}");
        assert!(out.contains("dependencies"), "{out}");
        // Tasks without those fields show only their title.
        assert!(!out.contains("unassigned"), "{out}");
        let short = app.board.as_ref().unwrap().detail.tasks[0]
            .short_id
            .to_string();
        assert!(
            !out.contains(&short),
            "short ids are internal, not for the board: {out}"
        );
    }

    #[test]
    fn mouse_areas_are_recorded_and_clickable() {
        use crate::app::{Cmd, Msg};
        use crossterm::event::{Event, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
        let mut app = ready_app(false);
        render(&mut app, 100, 24);
        assert_eq!(app.board_areas.len(), 2);
        let second = app.board_areas[1];
        let click = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: second.x + 2,
            row: second.y + 1,
            modifiers: KeyModifiers::NONE,
        };
        let cmds = app.update(Msg::Term(Event::Mouse(click)));
        assert!(
            matches!(&cmds[0], Cmd::Send(m) if matches!(m.request, taskologic_proto::Request::GetBoard { board_id } if board_id == taskologic_core::ids::BoardId(2)))
        );
    }

    #[test]
    fn menu_buttons_are_clickable() {
        use crate::app::Msg;
        use crossterm::event::{Event, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
        let mut app = ready_app(false);
        render(&mut app, 100, 24);
        let (rect, key) = *app
            .menu_areas
            .iter()
            .find(|(_, k)| *k == '?')
            .expect("help button");
        assert_eq!(key, '?');
        let click = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: rect.x + 1,
            row: rect.y,
            modifiers: KeyModifiers::NONE,
        };
        app.update(Msg::Term(Event::Mouse(click)));
        assert!(matches!(app.overlay, Some(Overlay::Help)));
    }
}

