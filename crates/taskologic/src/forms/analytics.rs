//! The analytics table: how long tasks actually took.
//!
//! This is a screen, not a dialog. It takes the body of the window the way a
//! board does, keeps the title bar and the menu bar above it, and wears the
//! same colours, because six columns of dates do not fit in a popup and a
//! white sheet over the whole terminal is a shock to look at.
//!
//! Opened from the dashboard it covers every board you can see; opened from a
//! board it starts on that one, and the filter's board picker moves it.
//!
//! Three views share the panel: the table, the filter over it, and one task's
//! recorded history. Which one is showing is [`View`], and Esc walks back out
//! through them. The actions live in the window's menu bar, so what you can
//! do here reads the same way it does everywhere else.

use chrono::{DateTime, NaiveDate, NaiveTime, TimeDelta, TimeZone, Utc};
use chrono_tz::Tz;
use crossterm::event::{Event, KeyCode, KeyEventKind, MouseButton, MouseEventKind};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, ListItem, Paragraph};
use taskologic_core::event::EventKind;
use taskologic_core::ids::{BoardId, ColumnId, TaskId};
use taskologic_core::stats::format_duration;
use taskologic_proto::{AnalyticsFilter, AnalyticsRow, HistoryEntry};

use crate::ui::adapter::{
    CheckboxState, ChoiceState, Focus, FocusBuilder, HandleEvent, HasFocus, HasScreenCursor,
    ListState, Outcome, Regular, TextInputState, checkbox_at, dropdown, dropdown_marker,
    dropdown_popup_hover, field, screen_list,
};
use crate::ui::theme::Theme;

#[derive(Debug, PartialEq)]
pub enum AnalyticsOutcome {
    Changed,
    /// Back to whatever opened it.
    Cancel,
    /// The filter changed; ask the daemon again.
    Reload,
    /// Open this task's history.
    OpenDetail(TaskId),
    /// Take a task in or out of the averages.
    SetExcluded(TaskId, bool),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum View {
    Table,
    Filter,
    Detail,
}

/// How wide the table has to be before it stops abbreviating. Below the
/// first the dates lose their year; below the second the planned columns go,
/// because what a task was measured at matters more than what it was meant
/// to be.
const FULL_DATES_FROM: u16 = 108;
const PLANNED_COLUMNS_FROM: u16 = 76;
/// `104d` at the widest, and `1h 20m /12` for an average with its count.
const TAKEN_W: usize = 7;
const AVERAGE_W: usize = 11;

/// Where the table's columns fall at one particular width.
struct Layout2 {
    title_w: usize,
    date_w: usize,
    planned: bool,
}

pub struct AnalyticsPanel {
    /// The name of the board it was opened on, for the rare case where the
    /// board list has not arrived yet and the picker cannot name its own
    /// choice. The picker itself is what decides the scope.
    scope: String,
    tz: Tz,
    rows: Vec<AnalyticsRow>,
    list: ListState,
    view: View,
    /// The task whose history is showing, and the history itself.
    detail_of: Option<TaskId>,
    history: Vec<HistoryEntry>,
    /// The columns of the board the open history belongs to, so a move can
    /// say where it went.
    history_columns: Vec<(ColumnId, String)>,
    history_list: ListState,
    pub filter: AnalyticsFilter,
    show_unfinished: CheckboxState,
    show_archived: CheckboxState,
    show_deleted: CheckboxState,
    assigned_to_me: CheckboxState,
    created_by_me: CheckboxState,
    repeating_only: CheckboxState,
    /// Boards to narrow to, offered only when the table covers all of them.
    /// Zero means every board, which is what opening it from the dashboard
    /// asked for in the first place.
    boards: Vec<(i64, String)>,
    board_pick: ChoiceState<i64>,
    /// A day each, inclusive, in the reader's own zone. Empty means no bound.
    since_input: TextInputState,
    until_input: TextInputState,
    pub error: Option<String>,
    /// A request is out. The table shows the last answer until the next one.
    pub loading: bool,
}

fn check(name: &'static str, on: bool) -> CheckboxState {
    let mut c = CheckboxState::named(name);
    c.set_checked(on);
    c
}

impl AnalyticsPanel {
    pub fn new(board_id: Option<BoardId>, scope: impl Into<String>, tz: Tz) -> Self {
        let filter = AnalyticsFilter::default();
        let list = ListState::named("analytics");
        list.focus().set(true);
        // Opened from a board the picker starts on it; from the dashboard it
        // starts on "all boards". Either way the picker is what decides from
        // then on, so there is one answer to "which boards am I looking at".
        let mut board_pick = ChoiceState::named("analytics_board");
        board_pick.set_value(board_id.map(|b| b.0).unwrap_or(0));
        Self {
            board_pick,
            scope: scope.into(),
            tz,
            rows: Vec::new(),
            list,
            view: View::Table,
            detail_of: None,
            history: Vec::new(),
            history_columns: Vec::new(),
            history_list: ListState::named("history"),
            show_unfinished: check("show_unfinished", filter.show_unfinished),
            show_archived: check("show_archived", filter.show_archived),
            show_deleted: check("show_deleted", filter.show_deleted),
            assigned_to_me: check("assigned_to_me", filter.assigned_to_me),
            created_by_me: check("created_by_me", filter.created_by_me),
            repeating_only: check("repeating_only", filter.repeating_only),
            boards: Vec::new(),
            since_input: TextInputState::named("analytics_since"),
            until_input: TextInputState::named("analytics_until"),
            filter,
            error: None,
            loading: true,
        }
    }

    /// The boards the reader may narrow to.
    pub fn set_boards(&mut self, boards: Vec<(BoardId, String)>) {
        self.boards = boards.into_iter().map(|(id, name)| (id.0, name)).collect();
    }

    /// Which board the next request should ask for. None is all of them.
    pub fn requested_board(&self) -> Option<BoardId> {
        match self.board_pick.value() {
            0 => None,
            id => Some(BoardId(id)),
        }
    }

    /// What the title bar calls what is on screen, which follows the picker
    /// rather than whatever the table was opened with.
    pub fn scope_label(&self) -> String {
        match self.board_pick.value() {
            0 => "all boards".to_string(),
            id => self
                .boards
                .iter()
                .find(|(b, _)| *b == id)
                .map(|(_, name)| name.clone())
                .unwrap_or_else(|| self.scope.clone()),
        }
    }

    /// What the menu bar offers here. Back comes first and wears a `<`, the
    /// way leaving a board does, so the way out is always in the same corner.
    pub fn menu_items(&self) -> Vec<(String, char)> {
        let back = ("Back".to_string(), 'b');
        match self.view {
            View::Filter => vec![back, ("Apply".into(), 'a')],
            View::Detail => vec![back, ("Exclude".into(), 'x')],
            View::Table => vec![
                back,
                ("Filter".into(), 'f'),
                ("Details".into(), 'd'),
                ("Exclude".into(), 'x'),
            ],
        }
    }

    pub fn set_rows(&mut self, rows: Vec<AnalyticsRow>) {
        self.rows = rows;
        self.loading = false;
        self.error = None;
        self.clamp();
    }

    pub fn set_history(
        &mut self,
        task_id: TaskId,
        entries: Vec<HistoryEntry>,
        columns: Vec<(ColumnId, String)>,
    ) {
        // A late answer for a row the reader has already moved off is stale.
        if self.detail_of != Some(task_id) {
            return;
        }
        self.history = entries;
        self.history_columns = columns;
        self.history_list
            .select((!self.history.is_empty()).then_some(0));
    }

    /// The row the reader is on, if any.
    pub fn selected(&self) -> Option<&AnalyticsRow> {
        self.list.selected().and_then(|i| self.rows.get(i))
    }

    fn clamp(&mut self) {
        let sel = self.list.selected().unwrap_or(0);
        self.list.select(if self.rows.is_empty() {
            None
        } else {
            Some(sel.min(self.rows.len() - 1))
        });
    }

    /// A day in the reader's own zone. `end` picks the last moment of it
    /// rather than the first, so a range of one day covers that whole day.
    fn parse_day(&self, text: impl AsRef<str>, end: bool) -> Result<Option<DateTime<Utc>>, String> {
        let text = text.as_ref().trim();
        if text.is_empty() {
            return Ok(None);
        }
        let day = NaiveDate::parse_from_str(text, "%Y-%m-%d")
            .map_err(|_| "dates must look like 2026-09-07".to_string())?;
        let time = if end {
            NaiveTime::from_hms_opt(23, 59, 59)
        } else {
            NaiveTime::from_hms_opt(0, 0, 0)
        }
        .unwrap_or_default();
        self.tz
            .from_local_datetime(&day.and_time(time))
            .earliest()
            .map(|d| Some(d.with_timezone(&Utc)))
            .ok_or_else(|| "that day does not start in your timezone".to_string())
    }

    /// Take the menu's state as the filter. A date that does not parse leaves
    /// the old range in place and says so, rather than silently widening the
    /// table to everything.
    fn read_filter(&mut self) -> Result<(), String> {
        let since = self.parse_day(self.since_input.text(), false)?;
        let until = self.parse_day(self.until_input.text(), true)?;
        if let (Some(a), Some(b)) = (since, until)
            && a > b
        {
            return Err("the range starts after it ends".into());
        }
        self.filter = AnalyticsFilter {
            show_unfinished: self.show_unfinished.checked(),
            show_archived: self.show_archived.checked(),
            show_deleted: self.show_deleted.checked(),
            assigned_to_me: self.assigned_to_me.checked(),
            created_by_me: self.created_by_me.checked(),
            repeating_only: self.repeating_only.checked(),
            since,
            until,
        };
        Ok(())
    }

    /// Leave the filter menu, applying what it says. A bad date keeps the
    /// menu open with the complaint on it.
    fn apply_filter(&mut self) -> AnalyticsOutcome {
        match self.read_filter() {
            Ok(()) => {
                self.error = None;
                self.view = View::Table;
                self.loading = true;
                AnalyticsOutcome::Reload
            }
            Err(e) => {
                self.error = Some(e);
                AnalyticsOutcome::Changed
            }
        }
    }

    fn focus(&self) -> Focus {
        let mut b = FocusBuilder::new(None);
        match self.view {
            View::Table => {
                b.widget(&self.list);
            }
            View::Filter => {
                b.widget(&self.show_unfinished)
                    .widget(&self.show_archived)
                    .widget(&self.show_deleted)
                    .widget(&self.assigned_to_me)
                    .widget(&self.created_by_me)
                    .widget(&self.repeating_only)
                    .widget(&self.since_input)
                    .widget(&self.until_input)
                    .widget(&self.board_pick);
            }
            View::Detail => {
                b.widget(&self.history_list);
            }
        }
        b.build()
    }

    pub fn handle(&mut self, ev: &Event) -> AnalyticsOutcome {
        let key = match ev {
            Event::Key(k) if k.kind != KeyEventKind::Release => Some(k.code),
            _ => None,
        };
        // Esc walks back out one view at a time rather than closing outright.
        if matches!(key, Some(KeyCode::Esc)) {
            return self.go_back();
        }

        let mut focus = self.focus();
        if focus.handle(ev, Regular) == Outcome::Changed && key.is_some() {
            return AnalyticsOutcome::Changed;
        }

        match self.view {
            View::Table => {
                if key == Some(KeyCode::Char('f')) {
                    self.view = View::Filter;
                    return AnalyticsOutcome::Changed;
                }
                if matches!(key, Some(KeyCode::Char('x'))) {
                    return self.toggle_excluded();
                }
                let open = matches!(key, Some(KeyCode::Enter | KeyCode::Char('d')))
                    || matches!(ev, Event::Mouse(m) if self.double_clicked(m));
                if open {
                    return self.open_detail();
                }
                self.list.handle(ev, Regular);
            }
            View::Filter => {
                for c in [
                    &mut self.show_unfinished,
                    &mut self.show_archived,
                    &mut self.show_deleted,
                    &mut self.assigned_to_me,
                    &mut self.created_by_me,
                    &mut self.repeating_only,
                ] {
                    c.handle(ev, Regular);
                }
                self.since_input.handle(ev, Regular);
                self.until_input.handle(ev, Regular);
                self.board_pick.handle(ev, Regular);
                // Enter applies and goes back, the same as Esc does.
                if key == Some(KeyCode::Enter) {
                    return self.apply_filter();
                }
            }
            View::Detail => {
                if matches!(key, Some(KeyCode::Char('x'))) {
                    return self.toggle_excluded();
                }
                self.history_list.handle(ev, Regular);
            }
        }
        AnalyticsOutcome::Changed
    }

    /// One step back out: the filter applies, the detail closes, and from the
    /// table itself there is nowhere left to go but out.
    pub fn go_back(&mut self) -> AnalyticsOutcome {
        match self.view {
            View::Table => AnalyticsOutcome::Cancel,
            View::Filter => self.apply_filter(),
            View::Detail => {
                self.view = View::Table;
                self.detail_of = None;
                self.history.clear();
                self.history_columns.clear();
                AnalyticsOutcome::Changed
            }
        }
    }

    /// A menu bar button. The bar is the only place these live, so a click on
    /// it has to reach the same code a keystroke does.
    pub fn menu_key(&mut self, key: char) -> AnalyticsOutcome {
        match (self.view, key) {
            (_, 'b') => self.go_back(),
            (View::Filter, 'a') => self.apply_filter(),
            (View::Table, 'f') => {
                self.view = View::Filter;
                AnalyticsOutcome::Changed
            }
            (View::Table, 'd') => self.open_detail(),
            (_, 'x') => self.toggle_excluded(),
            _ => AnalyticsOutcome::Changed,
        }
    }

    fn open_detail(&mut self) -> AnalyticsOutcome {
        match self.selected() {
            Some(row) => {
                let id = row.task_id;
                self.detail_of = Some(id);
                self.history.clear();
                self.view = View::Detail;
                AnalyticsOutcome::OpenDetail(id)
            }
            None => AnalyticsOutcome::Changed,
        }
    }

    fn toggle_excluded(&mut self) -> AnalyticsOutcome {
        match self.selected() {
            Some(row) => AnalyticsOutcome::SetExcluded(row.task_id, !row.excluded),
            None => AnalyticsOutcome::Changed,
        }
    }

    /// A click on the already selected row opens it, which is what a double
    /// click amounts to when the first click did the selecting.
    fn double_clicked(&self, m: &crossterm::event::MouseEvent) -> bool {
        if !matches!(m.kind, MouseEventKind::Down(MouseButton::Left)) {
            return false;
        }
        let pos = ratatui::layout::Position::new(m.column, m.row);
        self.list
            .row_areas
            .iter()
            .position(|r| r.contains(pos))
            .is_some_and(|i| self.list.selected() == Some(i))
    }

    fn stamp(&self, at: Option<DateTime<Utc>>, full: bool) -> String {
        match at {
            Some(d) => {
                let d = d.with_timezone(&self.tz);
                if full {
                    d.format("%Y-%m-%d %H:%M").to_string()
                } else {
                    d.format("%m-%d %H:%M").to_string()
                }
            }
            None => "-".into(),
        }
    }

    fn duration(secs: Option<i64>) -> String {
        match secs {
            Some(s) => format_duration(TimeDelta::seconds(s)),
            // Never started, so there is no number. Zero would be a lie.
            None => "-".into(),
        }
    }

    /// How the columns divide up the width on offer. The header and the rows
    /// both go through this, because a table whose header has drifted a
    /// column off its values is worse than one with no header at all.
    fn columns(width: u16) -> Layout2 {
        let date_w = if width >= FULL_DATES_FROM { 16 } else { 11 };
        let planned = width >= PLANNED_COLUMNS_FROM;
        // Each column is a space and then its field.
        let dates = if planned { 3 } else { 1 };
        let fixed = dates * (date_w + 1) + (TAKEN_W + 1) + (AVERAGE_W + 1);
        Layout2 {
            title_w: (width as usize).saturating_sub(fixed).max(10),
            date_w,
            planned,
        }
    }

    /// One table row, laid out for the width on offer.
    fn row_line(&self, r: &AnalyticsRow, width: u16) -> String {
        let l = Self::columns(width);
        let full = width >= FULL_DATES_FROM;
        let mut title = r.title.clone();
        if r.excluded {
            // The row is still shown, it just says it is not counted.
            title = format!("({title})");
        }
        let title = if title.chars().count() > l.title_w {
            let mut s: String = title.chars().take(l.title_w.saturating_sub(1)).collect();
            s.push('~');
            s
        } else {
            format!("{title:<w$}", w = l.title_w)
        };
        let average = match (r.average_secs, r.samples) {
            (Some(s), n) if n > 0 => format!("{} /{n}", format_duration(TimeDelta::seconds(s))),
            _ => "-".into(),
        };
        let mut out = title;
        if l.planned {
            out.push_str(&format!(
                " {:<w$} {:<w$}",
                self.stamp(r.planned_start, full),
                self.stamp(r.planned_due, full),
                w = l.date_w
            ));
        }
        out.push_str(&format!(
            " {:<dw$} {:>tw$} {:>aw$}",
            self.stamp(r.started_at, full),
            Self::duration(r.time_taken_secs),
            average,
            dw = l.date_w,
            tw = TAKEN_W,
            aw = AVERAGE_W,
        ));
        out.trim_end().to_string()
    }

    fn header(&self, width: u16) -> String {
        let l = Self::columns(width);
        let mut out = format!("{:<w$}", "Task", w = l.title_w);
        if l.planned {
            // A label wider than its column would shove every heading after
            // it out of line with the values below.
            let (start, due) = if l.date_w >= "Planned start".len() {
                ("Planned start", "Planned due")
            } else {
                ("Plan start", "Plan due")
            };
            out.push_str(&format!(" {:<w$} {:<w$}", start, due, w = l.date_w));
        }
        out.push_str(&format!(
            " {:<dw$} {:>tw$} {:>aw$}",
            "Started",
            "Taken",
            "Average",
            dw = l.date_w,
            tw = TAKEN_W,
            aw = AVERAGE_W,
        ));
        out.trim_end().to_string()
    }

    /// What a column was called, for a history line. A column removed since
    /// the move still has to read as a sentence.
    fn column_name(&self, id: ColumnId) -> String {
        self.history_columns
            .iter()
            .find(|(c, _)| *c == id)
            .map(|(_, name)| name.clone())
            .unwrap_or_else(|| "a column since removed".to_string())
    }

    /// One line of a task's history, in words rather than field names.
    fn history_line(&self, e: &HistoryEntry) -> String {
        let who = e.actor.clone().unwrap_or_else(|| "the scheduler".into());
        let when = e.at.with_timezone(&self.tz).format("%Y-%m-%d %H:%M");
        let what = match &e.kind {
            // Rows written before 0.1.11 do not say where it landed.
            EventKind::TaskCreated { column: Some(c) } => {
                format!("created it in {}", self.column_name(*c))
            }
            EventKind::TaskCreated { column: None } => "created it".to_string(),
            EventKind::TaskEdited { changed } if changed.is_empty() => {
                // Everything recorded before 0.1.11 says only this much.
                "edited it".to_string()
            }
            EventKind::TaskEdited { changed } => {
                let fields: Vec<&str> = changed.iter().map(|c| c.label()).collect();
                format!("changed the {}", fields.join(", "))
            }
            EventKind::ChecklistToggled { text, done, .. } => {
                format!("{} \"{text}\"", if *done { "ticked" } else { "unticked" })
            }
            EventKind::TaskMoved { from, to } => format!(
                "moved it from {} to {}",
                self.column_name(*from),
                self.column_name(*to)
            ),
            EventKind::TaskReordered => "reordered it".to_string(),
            EventKind::TaskDeleted => "deleted it".to_string(),
            EventKind::TaskPurged => "purged it".to_string(),
            EventKind::TaskArchived { from } => {
                format!("archived it from {}", self.column_name(*from))
            }
            EventKind::TaskRestored { to } => {
                format!("restored it to {}", self.column_name(*to))
            }
            EventKind::DependencyOverridden { open } => {
                format!("finished it over {} open dependencies", open.len())
            }
            EventKind::ScanApplied { action } => match action {
                taskologic_core::barcode::ScanAction::StartPause => {
                    "scanned it to start or pause".to_string()
                }
                taskologic_core::barcode::ScanAction::Finish => {
                    "scanned it as finished".to_string()
                }
            },
            EventKind::RepeatSpawned { .. } => "spawned it from a repetition".to_string(),
            EventKind::RepeatStopped => "stopped the repetition".to_string(),
            other => other.name().replace('_', " "),
        };
        format!("{when}  {who} {what}")
    }

    pub fn render(&mut self, f: &mut Frame, area: Rect, t: &Theme) {
        // A framed panel on the desktop, the way a board column is, rather
        // than a white sheet over the terminal.
        // The title bar already says which boards this covers, so the frame
        // names the view rather than repeating the scope.
        let title = match self.view {
            View::Table => " How long tasks took ".to_string(),
            View::Filter => " What to show ".to_string(),
            View::Detail => " Task history ".to_string(),
        };
        let hint = match self.view {
            View::Table => " Enter opens a task   x excludes it   Esc goes back ",
            View::Filter => " Space ticks   Enter applies   Esc applies ",
            View::Detail => " x excludes it from the averages   Esc goes back ",
        };
        let block = Block::default()
            .borders(Borders::ALL)
            .border_set(t.border_set())
            .border_style(t.column_border(true))
            .title(Span::styled(title, t.title()))
            .title_bottom(Line::from(Span::styled(hint.to_string(), t.dim())).right_aligned())
            .style(t.screen());
        let inner = block.inner(area);
        f.render_widget(block, area);
        if inner.width == 0 || inner.height == 0 {
            return;
        }

        let [head, body, err] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .areas(inner);

        match self.view {
            View::Table => self.render_table(f, head, body, t),
            View::Filter => self.render_filter(f, head, body, t),
            View::Detail => self.render_detail(f, head, body, t),
        }

        if let Some(e) = &self.error {
            f.render_widget(Paragraph::new(e.clone()).style(t.error()), err);
        }
    }

    fn render_table(&mut self, f: &mut Frame, head: Rect, body: Rect, t: &Theme) {
        f.render_widget(
            Paragraph::new(self.header(head.width)).style(t.title()),
            head,
        );
        let items: Vec<ListItem> = if self.loading {
            vec![ListItem::new("loading...").style(t.dim())]
        } else if self.rows.is_empty() {
            vec![
                ListItem::new("nothing finished yet, or the filter hides it all").style(t.dim()),
            ]
        } else {
            self.rows
                .iter()
                .map(|r| {
                    let line = self.row_line(r, body.width);
                    if r.excluded {
                        ListItem::new(Line::from(Span::styled(line, t.dim())))
                    } else {
                        ListItem::new(line)
                    }
                })
                .collect()
        };
        f.render_stateful_widget(screen_list(items, t), body, &mut self.list);
    }

    fn render_filter(&mut self, f: &mut Frame, head: Rect, body: Rect, t: &Theme) {
        f.render_widget(
            Paragraph::new("Show which tasks").style(t.title()),
            head,
        );
        let rows = Layout::vertical([Constraint::Length(1); 12]).split(body);
        let boxes: [(&mut CheckboxState, &str); 6] = [
            (&mut self.show_unfinished, "not finished yet"),
            (&mut self.show_archived, "archived"),
            (&mut self.show_deleted, "deleted"),
            (&mut self.assigned_to_me, "assigned to me"),
            (&mut self.created_by_me, "created by me"),
            (&mut self.repeating_only, "only from templates or repeats"),
        ];
        for (i, (state, label)) in boxes.into_iter().enumerate() {
            let area = rows[i];
            let w = super::check_w(label).min(area.width);
            let cb = Rect::new(area.x, area.y, w, 1);
            f.render_stateful_widget(checkbox_at(label.into(), cb, t), cb, state);
        }

        // Finished between these two days, inclusive, or created if it never
        // was. Blank either end for no bound.
        let mut r = super::Row::new(rows[7]);
        f.render_widget(
            Paragraph::new("Finished from").style(t.dim()),
            r.text("Finished from"),
        );
        f.render_stateful_widget(field(t), r.take(12), &mut self.since_input);
        f.render_widget(Paragraph::new("to").style(t.dim()), r.text("to"));
        f.render_stateful_widget(field(t), r.take(12), &mut self.until_input);
        f.render_widget(
            Paragraph::new(" YYYY-MM-DD, blank for no limit").style(t.dim()),
            r.rest(),
        );

        // Which boards the table covers. The picker is the only answer to
        // that, whichever screen the reader came from.
        let mut popup = None;
        if !self.boards.is_empty() {
            let mut r = super::Row::new(rows[8]);
            f.render_widget(
                Paragraph::new("Board").style(t.dim()),
                r.text("Board"),
            );
            let mut items = vec![(0i64, "all boards".to_string())];
            items.extend(self.boards.iter().cloned());
            let area = r.take(28);
            let (w, pop) = dropdown(items, area, t);
            f.render_stateful_widget(w, area, &mut self.board_pick);
            dropdown_marker(f, &self.board_pick, t);
            popup = Some((pop, area));
        }

        f.render_widget(
            Paragraph::new(
                "finished tasks are always listed; ticking both \"mine\" boxes shows either",
            )
            .style(t.dim()),
            rows[10],
        );
        if let Some((pop, area)) = popup {
            f.render_stateful_widget(pop, area, &mut self.board_pick);
            dropdown_popup_hover(f, &self.board_pick, t);
        }
        if let Some(p) = [
            self.since_input.screen_cursor(),
            self.until_input.screen_cursor(),
        ]
        .into_iter()
        .flatten()
        .next()
        {
            f.set_cursor_position(p);
        }
    }

    fn render_detail(&mut self, f: &mut Frame, head: Rect, body: Rect, t: &Theme) {
        let row = self
            .detail_of
            .and_then(|id| self.rows.iter().find(|r| r.task_id == id));
        let Some(row) = row else {
            f.render_widget(Paragraph::new("no task selected").style(t.dim()), head);
            return;
        };
        let wall = match row.wall_clock_secs {
            Some(s) => format_duration(TimeDelta::seconds(s)),
            None => "-".into(),
        };
        f.render_widget(
            Paragraph::new(format!(
                "{}  -  {}  -  taken {}, start to finish {}{}",
                row.title,
                row.board_name,
                Self::duration(row.time_taken_secs),
                wall,
                if row.excluded {
                    "  -  left out of the averages"
                } else {
                    ""
                }
            ))
            .style(t.title()),
            head,
        );
        let items: Vec<ListItem> = if self.history.is_empty() {
            vec![ListItem::new("loading...").style(t.dim())]
        } else {
            self.history
                .iter()
                .map(|e| ListItem::new(self.history_line(e)))
                .collect()
        };
        f.render_stateful_widget(screen_list(items, t), body, &mut self.history_list);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use taskologic_core::ids::ShortId;
    use taskologic_proto::TaskState;

    fn row(title: &str) -> AnalyticsRow {
        AnalyticsRow {
            task_id: TaskId(1),
            board_id: BoardId(1),
            board_name: "Kitchen".into(),
            short_id: ShortId::from_index(1),
            title: title.into(),
            state: TaskState::Finished,
            planned_start: DateTime::from_timestamp(1_800_000_000, 0),
            planned_due: DateTime::from_timestamp(1_800_007_200, 0),
            started_at: DateTime::from_timestamp(1_800_000_600, 0),
            time_taken_secs: Some(5_400),
            wall_clock_secs: Some(6_000),
            average_secs: Some(4_800),
            samples: 4,
            excluded: false,
        }
    }

    fn panel() -> AnalyticsPanel {
        AnalyticsPanel::new(Some(BoardId(1)), "Kitchen", chrono_tz::UTC)
    }

    #[test]
    fn a_wide_table_shows_full_dates_and_a_narrow_one_drops_the_planned_columns() {
        let p = panel();
        let r = row("Water plants");
        let wide = p.row_line(&r, 120);
        assert!(wide.contains("2027-01-15"), "full dates when there is room: {wide}");
        assert_eq!(wide.matches("2027-").count(), 3, "planned start, due, started");

        let medium = p.row_line(&r, 90);
        assert!(!medium.contains("2027-"), "the year goes first: {medium}");
        assert!(medium.contains("01-15"), "{medium}");

        // Below the threshold only the column that was measured survives.
        let narrow = p.row_line(&r, 70);
        assert_eq!(narrow.matches("01-15").count(), 1, "{narrow}");
        assert!(narrow.contains("1h 30m"), "the time taken always stays: {narrow}");
    }

    #[test]
    fn the_header_sits_over_the_values_it_names() {
        // The header and the rows are laid out by one function precisely so
        // they cannot drift a column apart from each other.
        let p = panel();
        let r = row("Water plants");
        for width in [130u16, 120, 108, 100, 90, 80, 76, 70, 60] {
            let head = p.header(width);
            let line = p.row_line(&r, width);
            let l = AnalyticsPanel::columns(width);
            assert_eq!(
                head.find("Started"),
                Some(if l.planned {
                    l.title_w + 2 * (l.date_w + 1) + 1
                } else {
                    l.title_w + 1
                }),
                "at {width}: {head}"
            );
            // Each left-aligned column starts where its heading does, with a
            // space in front of it rather than the previous value's last
            // character. Right-aligned numbers may pad further, which is why
            // this checks the columns whose start is fixed.
            let mut at = l.title_w;
            let date_starts = if l.planned { 3 } else { 1 };
            for _ in 0..date_starts {
                at += 1;
                for text in [&head, &line] {
                    let b = text.as_bytes();
                    assert_eq!(b[at - 1], b' ', "no gap at {at} in {width}: {text}");
                    assert_ne!(b[at], b' ', "column starts late at {at} in {width}: {text}");
                }
                at += l.date_w;
            }
            assert!(head.len() <= width as usize, "header overflows at {width}");
            assert!(line.len() <= width as usize, "row overflows at {width}: {line}");
        }
    }

    #[test]
    fn a_task_that_was_never_started_shows_a_dash_not_a_zero() {
        let p = panel();
        let mut r = row("Quick one");
        r.started_at = None;
        r.time_taken_secs = None;
        let line = p.row_line(&r, 120);
        assert!(line.contains('-'), "{line}");
        assert!(!line.contains("0s"), "zero would drag the average down: {line}");
    }

    #[test]
    fn an_average_says_how_many_it_is_over() {
        let p = panel();
        assert!(p.row_line(&row("x"), 120).contains("1h 20m /4"));
        let mut r = row("x");
        r.average_secs = None;
        r.samples = 0;
        let line = p.row_line(&r, 120);
        assert!(line.trim_end().ends_with('-'), "{line}");
    }

    #[test]
    fn an_excluded_row_says_so_in_its_title() {
        let p = panel();
        let mut r = row("Odd one out");
        r.excluded = true;
        assert!(p.row_line(&r, 120).contains("(Odd one out)"));
    }

    #[test]
    fn the_menu_bar_offers_what_the_view_can_actually_do() {
        let mut p = panel();
        p.set_rows(vec![row("Water plants")]);
        let keys = |p: &AnalyticsPanel| -> Vec<char> {
            p.menu_items().into_iter().map(|(_, k)| k).collect()
        };
        // The way out is the first button in every view, wherever you are.
        for view in [View::Table, View::Filter, View::Detail] {
            p.view = view;
            assert_eq!(p.menu_items()[0].0, "Back", "{view:?}");
        }
        p.view = View::Table;
        assert_eq!(keys(&p), vec!['b', 'f', 'd', 'x']);

        // Its buttons reach the same code the keys do.
        assert_eq!(p.menu_key('f'), AnalyticsOutcome::Changed);
        assert_eq!(p.view, View::Filter);
        assert_eq!(keys(&p), vec!['b', 'a'], "nothing to exclude from a filter");
        assert_eq!(p.menu_key('a'), AnalyticsOutcome::Reload);
        assert_eq!(p.view, View::Table);

        assert_eq!(p.menu_key('d'), AnalyticsOutcome::OpenDetail(TaskId(1)));
        assert_eq!(keys(&p), vec!['b', 'x'], "no filter from inside a history");
        assert_eq!(
            p.menu_key('x'),
            AnalyticsOutcome::SetExcluded(TaskId(1), true)
        );
        assert_eq!(p.menu_key('b'), AnalyticsOutcome::Changed);
        assert_eq!(p.view, View::Table);
        assert_eq!(p.menu_key('b'), AnalyticsOutcome::Cancel, "and then out");
    }

    #[test]
    fn the_filter_defaults_to_finished_work_and_esc_applies_it() {
        let mut p = panel();
        assert!(!p.filter.show_unfinished, "unfinished work has no duration");
        assert!(p.filter.show_archived && p.filter.show_deleted);

        // The key the hint bar advertises opens it, not just the button.
        let f = Event::Key(crossterm::event::KeyEvent::new(
            KeyCode::Char('f'),
            crossterm::event::KeyModifiers::NONE,
        ));
        assert_eq!(p.handle(&f), AnalyticsOutcome::Changed);
        assert_eq!(p.view, View::Filter);
        p.show_unfinished.set_checked(true);
        // Esc out of the filter applies it and asks the daemon again.
        let esc = Event::Key(crossterm::event::KeyEvent::new(
            KeyCode::Esc,
            crossterm::event::KeyModifiers::NONE,
        ));
        assert_eq!(p.handle(&esc), AnalyticsOutcome::Reload);
        assert!(p.filter.show_unfinished);
        assert!(p.loading);
        // A second Esc, now on the table, leaves the panel.
        p.loading = false;
        assert_eq!(p.handle(&esc), AnalyticsOutcome::Cancel);
    }

    #[test]
    fn a_date_range_covers_whole_days_in_the_readers_own_zone() {
        let mut p = AnalyticsPanel::new(None, "all boards", chrono_tz::Europe::Berlin);
        p.view = View::Filter;
        p.since_input.set_text("2026-09-07".to_string());
        p.until_input.set_text("2026-09-07".to_string());
        assert_eq!(p.apply_filter(), AnalyticsOutcome::Reload);
        let (since, until) = (p.filter.since.unwrap(), p.filter.until.unwrap());
        // Berlin is two hours ahead in September, so the day starts at 22:00
        // the evening before in UTC and runs just short of 24 hours later.
        assert_eq!(since.to_rfc3339(), "2026-09-06T22:00:00+00:00");
        assert_eq!(until.to_rfc3339(), "2026-09-07T21:59:59+00:00");
        assert!(until > since, "one day is still a range");
    }

    #[test]
    fn a_date_that_does_not_parse_keeps_the_menu_open_and_says_so() {
        let mut p = panel();
        p.view = View::Filter;
        p.since_input.set_text("last tuesday".to_string());
        assert_eq!(p.apply_filter(), AnalyticsOutcome::Changed);
        assert_eq!(p.view, View::Filter, "the reader stays where the typo is");
        assert!(p.error.is_some());
        assert_eq!(p.filter.since, None, "nothing was applied");

        // A range that runs backwards is refused the same way.
        p.since_input.set_text("2026-09-09".to_string());
        p.until_input.set_text("2026-09-07".to_string());
        assert_eq!(p.apply_filter(), AnalyticsOutcome::Changed);
        assert_eq!(p.error.as_deref(), Some("the range starts after it ends"));

        // Putting it right applies and clears the complaint.
        p.until_input.set_text("2026-09-11".to_string());
        assert_eq!(p.apply_filter(), AnalyticsOutcome::Reload);
        assert!(p.error.is_none());
        assert!(p.filter.since.is_some() && p.filter.until.is_some());
    }

    #[test]
    fn the_board_picker_says_what_is_on_screen_and_can_move_it() {
        let boards = vec![
            (BoardId(1), "Kitchen".to_string()),
            (BoardId(2), "Garage".to_string()),
        ];
        // Opened from the dashboard: every board, until the picker says less.
        let mut all = AnalyticsPanel::new(None, "all boards", chrono_tz::UTC);
        all.set_boards(boards.clone());
        assert_eq!(all.requested_board(), None);
        assert_eq!(all.scope_label(), "all boards");
        all.board_pick.set_value(2);
        assert_eq!(all.requested_board(), Some(BoardId(2)));
        assert_eq!(all.scope_label(), "Garage", "the title follows the picker");

        // Opened from a board: it starts there, and the picker can widen it
        // back out to everything or move it to another board.
        let mut one = AnalyticsPanel::new(Some(BoardId(1)), "Kitchen", chrono_tz::UTC);
        one.set_boards(boards);
        assert_eq!(one.requested_board(), Some(BoardId(1)));
        assert_eq!(one.scope_label(), "Kitchen");
        one.board_pick.set_value(0);
        assert_eq!(one.requested_board(), None);
        assert_eq!(one.scope_label(), "all boards");
    }

    #[test]
    fn enter_on_a_row_asks_for_its_history_and_esc_comes_back() {
        let mut p = panel();
        p.set_rows(vec![row("Water plants")]);
        let enter = Event::Key(crossterm::event::KeyEvent::new(
            KeyCode::Enter,
            crossterm::event::KeyModifiers::NONE,
        ));
        assert_eq!(p.handle(&enter), AnalyticsOutcome::OpenDetail(TaskId(1)));
        assert_eq!(p.view, View::Detail);

        // An answer for a different task is stale and ignored.
        p.set_history(TaskId(2), vec![], vec![]);
        assert!(p.history.is_empty());

        let esc = Event::Key(crossterm::event::KeyEvent::new(
            KeyCode::Esc,
            crossterm::event::KeyModifiers::NONE,
        ));
        assert_eq!(p.handle(&esc), AnalyticsOutcome::Changed);
        assert_eq!(p.view, View::Table);
    }

    #[test]
    fn a_move_says_which_columns_it_went_between() {
        let mut p = panel();
        p.detail_of = Some(TaskId(1));
        p.set_history(
            TaskId(1),
            vec![],
            vec![
                (ColumnId(1), "Todo".into()),
                (ColumnId(2), "Doing".into()),
                (ColumnId(4), "Done".into()),
            ],
        );
        let at = DateTime::from_timestamp(1_800_000_000, 0).unwrap();
        let line = |kind| {
            p.history_line(&HistoryEntry {
                at,
                actor: Some("alice".into()),
                kind,
            })
        };
        assert!(
            line(EventKind::TaskMoved {
                from: ColumnId(1),
                to: ColumnId(2),
            })
            .ends_with("alice moved it from Todo to Doing")
        );
        assert!(
            line(EventKind::TaskCreated {
                column: Some(ColumnId(1)),
            })
            .ends_with("alice created it in Todo")
        );
        assert!(
            line(EventKind::TaskArchived { from: ColumnId(4) })
                .ends_with("alice archived it from Done")
        );
        assert!(
            line(EventKind::TaskRestored { to: ColumnId(1) })
                .ends_with("alice restored it to Todo")
        );
        // A column removed since the move still has to read as a sentence.
        assert!(
            line(EventKind::TaskMoved {
                from: ColumnId(9),
                to: ColumnId(2),
            })
            .ends_with("alice moved it from a column since removed to Doing")
        );
    }

    #[test]
    fn history_reads_as_sentences_including_for_rows_older_than_the_feature() {
        let p = panel();
        let at = DateTime::from_timestamp(1_800_000_000, 0).unwrap();
        let line = |kind| {
            p.history_line(&HistoryEntry {
                at,
                actor: Some("alice".into()),
                kind,
            })
        };
        assert!(
            line(EventKind::TaskEdited { changed: vec![] }).ends_with("alice edited it"),
            "an edit from before the detail existed still reads"
        );
        let changed = vec![
            taskologic_core::event::FieldChange::DueAt {
                from: None,
                to: Some(at),
            },
            taskologic_core::event::FieldChange::Checklist,
        ];
        assert!(
            line(EventKind::TaskEdited { changed })
                .ends_with("alice changed the due date, checklist")
        );
        assert!(
            line(EventKind::ChecklistToggled {
                index: 0,
                text: "mop".into(),
                done: true,
            })
            .ends_with(r#"alice ticked "mop""#)
        );
        // A creation from before the column was recorded still reads.
        assert!(
            line(EventKind::TaskCreated { column: None }).ends_with("alice created it"),
            "no column on the row, so none in the sentence"
        );
        // The scheduler is nobody, and says so rather than borrowing a name.
        let job = p.history_line(&HistoryEntry {
            at,
            actor: None,
            kind: EventKind::TaskArchived { from: ColumnId(1) },
        });
        assert!(job.contains("the scheduler archived it"), "{job}");
    }
}
