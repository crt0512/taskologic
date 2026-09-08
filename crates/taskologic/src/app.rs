//! Central state and the update function.
//!
//! Elm style: every input is a `Msg`, `update` mutates the state and hands
//! back `Cmd`s for the outside world. Nothing in here does IO, which is what
//! makes the TestBackend tests possible. The client never enforces a rule
//! itself, it hides what it knows the daemon would refuse and otherwise
//! shows the daemon's answer.

use std::collections::HashMap;

use crossterm::event::{
    Event as TermEvent, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent,
    MouseEventKind,
};
use ratatui::layout::{Position, Rect};
use taskologic_core::barcode::{Feed, ScanAction, ScanDetector};
use taskologic_core::board::{Board, ColumnRole};
use taskologic_core::ids::{ColumnId, PrintJobId, TaskId, Uid};
use taskologic_core::prefs::{CardFields, CustomColors, ThemePreset};
use taskologic_core::print::PrintJob;
use taskologic_core::task::{Task, TaskDraft};
use taskologic_core::user::User;
use taskologic_print::DeviceProfile;
use taskologic_proto::{
    BoardChange, BoardDetail, BoardSummary, ClientMessage, ErrorBody, ErrorCode, Event, Hello,
    Request, RequestId, Response, ScanOutcome, SearchHit, ServerMessage, Severity, TaskChange,
};

use crate::forms::archive::{ArchiveOutcome, ArchivePanel};
use crate::forms::board::{BoardForm, BoardOutcome};
use crate::forms::colors::{ColorsForm, ColorsOutcome};
use crate::forms::columns::{ColumnsOutcome, ColumnsPanel};
use crate::forms::confirm::{Confirm, ConfirmOutcome};
use crate::forms::members::{MembersOutcome, MembersPanel};
use crate::forms::printer::{PrinterForm, PrinterOutcome};
use crate::forms::repeats::{RepeatsOutcome, RepeatsPanel};
use crate::forms::settings::{SettingsForm, SettingsOutcome};
use crate::forms::task::{FormMode, FormOutcome, TaskForm};
use crate::forms::templates::{TemplatesOutcome, TemplatesPanel};
use crate::forms::users::{UsersOutcome, UsersPanel};
use crate::scan;
use crate::ui::adapter::{
    CheckboxState, HandleEvent, HasFocus, MouseFlags, Regular, TextInputState,
};
use crate::ui::theme::{ColorMode, Theme};

pub const MIN_WIDTH: u16 = 80;
pub const MIN_HEIGHT: u16 = 24;
const TOAST_MS: u64 = 4000;
const FLASH_MS: u64 = 300;

pub enum Msg {
    Term(TermEvent),
    /// Boxed so the whole enum stays small in the channel.
    Server(Box<ServerMessage>),
    Disconnected(String),
    Tick(u64),
    Printed {
        job_id: PrintJobId,
        error: Option<String>,
    },
    TestPrinted {
        error: Option<String>,
    },
    /// The runtime wrote (or failed to write) the printer profile.
    PrinterSaved {
        path: String,
        error: Option<String>,
    },
    /// What `lpstat -e` reported: CUPS queue names on this machine.
    PrintersDetected {
        queues: Vec<String>,
        error: Option<String>,
    },
    /// How wide the media on `queue` is, per `lpoptions`. None when CUPS
    /// names a size with no dimensions in it, or the queue has gone away.
    MediaDetected {
        queue: String,
        mm: Option<u16>,
    },
}

#[derive(Debug, PartialEq)]
pub enum Cmd {
    Send(ClientMessage),
    Print {
        job_id: PrintJobId,
        job: PrintJob,
    },
    /// Render and spool a job locally without involving the daemon, with
    /// the profile as the printer form has it right now, saved or not.
    TestPrint {
        job: PrintJob,
        profile: DeviceProfile,
    },
    /// Write this printer profile to the client config file. None removes
    /// the printer section.
    SavePrinter(Option<DeviceProfile>),
    /// List the CUPS queues on this machine, off the UI thread.
    DetectPrinters,
    /// Ask CUPS how wide this queue's media is, to seed the paper width.
    DetectMedia(String),
    Bell,
    Quit,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Conn {
    Connecting,
    Ready,
    Lost,
}

pub struct Toast {
    pub text: String,
    pub severity: Severity,
    pub until_ms: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Pending {
    Welcome,
    Boards,
    Users,
    OpenBoard { focus_task: Option<TaskId> },
    RefreshBoard,
    MoveTask { task: TaskId, to: ColumnId },
    DeleteTask,
    PurgeTask,
    DeleteBoard,
    Restore,
    Archived,
    Scan,
    Search,
    Print,
    Ack,
    FormMembers,
    FormDepSearch,
    SaveTask,
    Settings,
    Timezone,
    FormUsers,
    CreateBoardForm,
    BoardOp,
    PanelMembers,
    PanelUsers,
    MemberOp { uid: Uid },
    ColumnOp,
    UsersPanel,
    AdminOp,
    ChecklistOp,
    TemplatesPanel,
    TemplateOp,
    RepeatsPanel,
    StopRepeat,
}

pub enum Overlay {
    Help,
    /// Every yes/no question, answerable by key, mouse or touch.
    Confirm {
        dialog: Box<Confirm>,
        action: ConfirmAction,
        back: Option<Box<Overlay>>,
    },
    TaskForm(Box<TaskForm>),
    Settings(Box<SettingsForm>),
    /// The colour editor, opened from settings and returning to it.
    Colors {
        form: Box<ColorsForm>,
        back: Box<SettingsForm>,
    },
    /// The printer setup, opened from settings and returning to it.
    Printer {
        form: Box<PrinterForm>,
        back: Box<SettingsForm>,
    },
    BoardForm(Box<BoardForm>),
    Members(Box<MembersPanel>),
    Columns(Box<ColumnsPanel>),
    /// One question of the column removal flow.
    PickColumn {
        panel: Box<ColumnsPanel>,
        title: String,
        options: Vec<(ColumnId, String)>,
        sel: usize,
        flow: RemoveFlow,
    },
    Users(Box<UsersPanel>),
    Templates(Box<TemplatesPanel>),
    Repeats(Box<RepeatsPanel>),
    /// The More menu: everything used now and then rather than constantly.
    Menu {
        items: Vec<(String, char)>,
        sel: Option<usize>,
        areas: Vec<Rect>,
    },
    /// A save hit a stale version. The form is kept so nothing typed is lost.
    Conflict {
        form: Box<TaskForm>,
        current: Box<Task>,
    },
    /// The read only task view. The checklist in it is live: items are
    /// selectable and can be ticked. The rects are recorded by the renderer.
    TaskDetail {
        task: Task,
        sel: usize,
        item_areas: Vec<Rect>,
        area: Rect,
    },
    Archive(Box<ArchivePanel>),
}

/// What a confirmed dialog does.
pub enum ConfirmAction {
    Quit,
    Send { request: Request, pending: Pending },
}

/// Answers collected before a column can be removed.
#[derive(Clone, Debug)]
pub struct RemoveFlow {
    pub column: ColumnId,
    pub has_tasks: bool,
    pub destination: Option<ColumnId>,
    pub roles: Vec<ColumnRole>,
    pub replacements: Vec<(ColumnRole, ColumnId)>,
}

impl RemoveFlow {
    fn next_role(&self) -> Option<ColumnRole> {
        self.roles
            .iter()
            .find(|r| !self.replacements.iter().any(|(rr, _)| rr == *r))
            .copied()
    }
}

/// The open board plus everything the view records for mouse hit testing.
pub struct BoardView {
    pub detail: BoardDetail,
    pub col: usize,
    pub row: usize,
    pub picked: Option<TaskId>,
    pub col_offset: usize,
    pub area: Rect,
    pub column_areas: Vec<Rect>,
    /// Per column click targets, as (column index, area).
    pub plus_areas: Vec<(usize, Rect)>,
    pub sort_areas: Vec<(usize, Rect)>,
    pub up_areas: Vec<(usize, Rect)>,
    pub down_areas: Vec<(usize, Rect)>,
    pub left_arrow: Option<Rect>,
    pub right_arrow: Option<Rect>,
    /// First visible card per column.
    pub col_scroll: Vec<usize>,
    pub task_areas: Vec<Vec<(TaskId, Rect)>>,
    pub mouse: MouseFlags,
    pub drag: Option<(TaskId, usize)>,
    pub hover_col: Option<usize>,
}

impl BoardView {
    fn new(detail: BoardDetail) -> Self {
        Self {
            detail,
            col: 0,
            row: 0,
            picked: None,
            col_offset: 0,
            area: Rect::default(),
            column_areas: Vec::new(),
            plus_areas: Vec::new(),
            sort_areas: Vec::new(),
            up_areas: Vec::new(),
            down_areas: Vec::new(),
            left_arrow: None,
            right_arrow: None,
            col_scroll: Vec::new(),
            task_areas: Vec::new(),
            mouse: MouseFlags::default(),
            drag: None,
            hover_col: None,
        }
    }

    pub fn column_count(&self) -> usize {
        self.detail.board.columns.len()
    }

    pub fn column_id(&self, idx: usize) -> Option<ColumnId> {
        self.detail.board.columns.get(idx).map(|c| c.id)
    }

    /// Tasks of a column in display order.
    pub fn tasks_in(&self, idx: usize) -> Vec<&Task> {
        let Some(column) = self.detail.board.columns.get(idx) else {
            return Vec::new();
        };
        let mut v: Vec<&Task> = self
            .detail
            .tasks
            .iter()
            .filter(|t| t.column_id == column.id && !t.is_archived())
            .collect();
        if column.sort_by_due {
            // Tasks without a due date sink to the bottom.
            v.sort_by_key(|t| (t.due_at.is_none(), t.due_at, t.position, t.id));
        } else {
            v.sort_by_key(|t| (t.position, t.id));
        }
        v
    }

    /// Where a card dropped at `y` lands in column `idx`, and the position
    /// value that puts it there.
    fn drop_position(&self, idx: usize, y: u16, moving: TaskId) -> Option<i64> {
        let others: Vec<i64> = self
            .tasks_in(idx)
            .into_iter()
            .filter(|t| t.id != moving)
            .map(|t| t.position)
            .collect();
        if others.is_empty() {
            return Some(0);
        }
        let slot = self.task_areas.get(idx.checked_sub(self.col_offset)?)?;
        let mut index = slot
            .iter()
            .filter(|(id, area)| *id != moving && y >= area.y + area.height / 2)
            .count()
            + self.col_scroll.get(idx).copied().unwrap_or(0);
        index = index.min(others.len());
        Some(match index {
            0 => others[0] - 10,
            i if i >= others.len() => others[others.len() - 1] + 10,
            i => {
                let (before, after) = (others[i - 1], others[i]);
                if after - before > 1 {
                    before + (after - before) / 2
                } else {
                    before + 1
                }
            }
        })
    }

    pub fn selected_task(&self) -> Option<&Task> {
        self.tasks_in(self.col).get(self.row).copied()
    }

    fn clamp(&mut self) {
        let cols = self.column_count();
        if cols == 0 {
            self.col = 0;
            self.row = 0;
            return;
        }
        self.col = self.col.min(cols - 1);
        let n = self.tasks_in(self.col).len();
        self.row = if n == 0 { 0 } else { self.row.min(n - 1) };
    }

    fn select_task(&mut self, id: TaskId) {
        for c in 0..self.column_count() {
            if let Some(r) = self.tasks_in(c).iter().position(|t| t.id == id) {
                self.col = c;
                self.row = r;
                return;
            }
        }
    }

    fn upsert(&mut self, task: Task) {
        match self.detail.tasks.iter_mut().find(|t| t.id == task.id) {
            Some(slot) => *slot = task,
            None => self.detail.tasks.push(task),
        }
        self.clamp();
    }

    fn remove(&mut self, id: TaskId) {
        self.detail.tasks.retain(|t| t.id != id);
        if self.picked == Some(id) {
            self.picked = None;
        }
        self.clamp();
    }

    fn column_at(&self, x: u16, y: u16) -> Option<usize> {
        self.column_areas
            .iter()
            .position(|a| a.contains(Position::new(x, y)))
            .map(|i| i + self.col_offset)
    }

    fn hit(targets: &[(usize, Rect)], x: u16, y: u16) -> Option<usize> {
        targets
            .iter()
            .find(|(_, a)| a.contains(Position::new(x, y)))
            .map(|(i, _)| *i)
    }

    fn task_at(&self, x: u16, y: u16) -> Option<(usize, usize, TaskId)> {
        for (i, col) in self.task_areas.iter().enumerate() {
            for (row, (id, area)) in col.iter().enumerate() {
                if area.contains(Position::new(x, y)) {
                    return Some((i + self.col_offset, row, *id));
                }
            }
        }
        None
    }
}

pub struct App {
    pub size: (u16, u16),
    pub ascii: bool,
    pub conn: Conn,
    pub conn_error: Option<String>,
    pub user: Option<User>,
    pub users: HashMap<Uid, String>,
    pub boards: Vec<BoardSummary>,
    pub board_sel: usize,
    pub board_areas: Vec<Rect>,
    pub dash_scroll: usize,
    pub tab_areas: Vec<(Rect, taskologic_core::ids::BoardId)>,
    pub search: TextInputState,
    pub search_area: Rect,
    pub include_archived: CheckboxState,
    pub hits: Vec<SearchHit>,
    pub hit_sel: usize,
    pub hit_areas: Vec<Rect>,
    pub board: Option<BoardView>,
    pub overlay: Option<Overlay>,
    pub toast: Option<Toast>,
    pub menu_areas: Vec<(Rect, char)>,
    pub printer: Option<DeviceProfile>,
    pub color_mode: ColorMode,
    pub manual_scan: bool,
    /// Screen inverts until this instant, the visual half of the scan bell.
    pub flash_until_ms: u64,
    pub mouse_seen: bool,
    /// Last pointer position, so the theme can light up what is under it.
    pub mouse_pos: Option<(u16, u16)>,
    pub now_ms: u64,
    pending: HashMap<RequestId, Pending>,
    next_id: RequestId,
    scan: ScanDetector,
}

impl App {
    pub fn new(ascii: bool, color_mode: ColorMode, printer: Option<DeviceProfile>) -> App {
        App {
            size: (MIN_WIDTH, MIN_HEIGHT),
            ascii,
            conn: Conn::Connecting,
            conn_error: None,
            user: None,
            users: HashMap::new(),
            boards: Vec::new(),
            board_sel: 0,
            board_areas: Vec::new(),
            dash_scroll: 0,
            tab_areas: Vec::new(),
            search: TextInputState::named("search"),
            search_area: Rect::default(),
            include_archived: CheckboxState::named("include_archived"),
            hits: Vec::new(),
            hit_sel: 0,
            hit_areas: Vec::new(),
            board: None,
            overlay: None,
            toast: None,
            menu_areas: Vec::new(),
            printer,
            color_mode,
            manual_scan: false,
            flash_until_ms: 0,
            mouse_seen: false,
            mouse_pos: None,
            now_ms: 0,
            pending: HashMap::new(),
            next_id: 1,
            scan: ScanDetector::new(),
        }
    }

    pub fn start(&mut self, hello: Hello) -> Vec<Cmd> {
        vec![self.send(Request::Hello(hello), Pending::Welcome)]
    }

    /// Colours and glyphs, rebuilt per frame so a prefs change shows at once.
    /// While the settings form is open it previews what is picked there.
    pub fn theme(&self) -> Theme {
        let (preset, colors) = match &self.overlay {
            Some(Overlay::Settings(form)) => form.preview(),
            Some(Overlay::Colors { back, .. }) => back.preview(),
            Some(Overlay::Printer { back, .. }) => back.preview(),
            _ => match &self.user {
                Some(u) => (u.prefs.ui.theme, u.prefs.ui.custom_colors.clone()),
                None => (ThemePreset::default(), CustomColors::default()),
            },
        };
        Theme::preset(preset, &colors, self.color_mode, self.ascii)
            .with_touch(self.touchscreen())
            .with_mouse(self.mouse_pos)
    }

    pub fn touchscreen(&self) -> bool {
        self.user.as_ref().is_some_and(|u| u.prefs.ui.touchscreen)
    }

    pub fn show_tabs(&self) -> bool {
        self.board.is_some()
            && self
                .user
                .as_ref()
                .is_some_and(|u| u.prefs.ui.show_board_tabs)
    }

    /// What task cards show: the user's own choice, else the board's.
    pub fn card_fields(&self) -> CardFields {
        match self.user.as_ref().and_then(|u| u.prefs.ui.card_fields) {
            Some(f) => f,
            None => self
                .board
                .as_ref()
                .map(|b| b.detail.board.card_fields)
                .unwrap_or_default(),
        }
    }

    pub fn too_small(&self) -> bool {
        self.size.0 < MIN_WIDTH || self.size.1 < MIN_HEIGHT
    }

    pub fn scanner_enabled(&self) -> bool {
        self.user
            .as_ref()
            .is_some_and(|u| u.prefs.ui.scanner_enabled)
    }

    pub fn scanner_manual_only(&self) -> bool {
        self.user
            .as_ref()
            .is_some_and(|u| u.prefs.scanner.manual_only)
    }

    /// The full width scan button shows whenever manual mode could be toggled.
    pub fn show_scan_button(&self) -> bool {
        self.scanner_enabled() && self.scanner_manual_only()
    }

    /// The bell plus a short screen flash, because some terminals swallow BEL.
    fn bell(&mut self) -> Cmd {
        self.flash_until_ms = self.now_ms + FLASH_MS;
        Cmd::Bell
    }

    pub fn flash_active(&self) -> bool {
        self.now_ms < self.flash_until_ms
    }

    /// True while the scan detector is watching the keystroke stream. Never
    /// while a text field has focus, never under an overlay.
    pub fn scanner_listening(&self) -> bool {
        self.scanner_enabled()
            && (!self.scanner_manual_only() || self.manual_scan)
            && self.overlay.is_none()
            && !self.text_focused()
    }

    pub fn text_focused(&self) -> bool {
        self.search.is_focused()
            || self.is_widget_overlay()
            || matches!(&self.overlay, Some(Overlay::Conflict { .. }))
    }

    /// Overlays that own widgets and get raw events.
    fn is_widget_overlay(&self) -> bool {
        matches!(
            self.overlay,
            Some(
                Overlay::Confirm { .. }
                    | Overlay::Menu { .. }
                    | Overlay::Archive(_)
                    | Overlay::TaskForm(_)
                    | Overlay::Settings(_)
                    | Overlay::Colors { .. }
                    | Overlay::Printer { .. }
                    | Overlay::BoardForm(_)
                    | Overlay::Members(_)
                    | Overlay::Columns(_)
                    | Overlay::Users(_)
                    | Overlay::Templates(_)
                    | Overlay::Repeats(_)
            )
        )
    }

    fn user_name(&self, uid: Uid) -> String {
        self.users
            .get(&uid)
            .cloned()
            .unwrap_or_else(|| format!("uid {uid}"))
    }

    fn me(&self) -> Uid {
        self.user.as_ref().map(|u| u.uid).unwrap_or(0)
    }

    /// Owner of the open board, or an admin. What the archive purge and the
    /// board delete are gated on; the daemon decides for real.
    pub fn privileged_on_open_board(&self) -> bool {
        match (&self.user, &self.board) {
            (Some(u), Some(b)) => u.is_admin || b.detail.board.owner_uid == u.uid,
            _ => false,
        }
    }

    pub fn show_print(&self) -> bool {
        self.printer.is_some()
            && self
                .user
                .as_ref()
                .is_some_and(|u| u.prefs.print.show_print_button)
    }

    fn send(&mut self, request: Request, pending: Pending) -> Cmd {
        let id = self.next_id;
        self.next_id += 1;
        self.pending.insert(id, pending);
        Cmd::Send(ClientMessage { id, request })
    }

    pub fn toast(&mut self, severity: Severity, text: impl Into<String>) {
        self.toast = Some(Toast {
            text: text.into(),
            severity,
            until_ms: self.now_ms + TOAST_MS,
        });
    }

    pub fn update(&mut self, msg: Msg) -> Vec<Cmd> {
        match msg {
            Msg::Tick(now) => {
                self.now_ms = now;
                if self.toast.as_ref().is_some_and(|t| t.until_ms <= now) {
                    self.toast = None;
                }
                self.scan.expire(now);
                Vec::new()
            }
            Msg::Disconnected(reason) => {
                self.conn = Conn::Lost;
                self.conn_error = Some(reason);
                Vec::new()
            }
            Msg::Server(msg) => match *msg {
                ServerMessage::Ok { id, response } => {
                    let pending = self.pending.remove(&id);
                    self.on_response(pending, response)
                }
                ServerMessage::Err { id, error } => {
                    let pending = self.pending.remove(&id);
                    self.on_error(pending, error)
                }
                ServerMessage::Event { event } => self.on_event(event),
            },
            Msg::TestPrinted { error } => {
                match error {
                    Some(e) => self.toast(Severity::Error, format!("test print failed: {e}")),
                    None => self.toast(Severity::Success, "test print sent to the printer"),
                }
                Vec::new()
            }
            Msg::PrinterSaved { path, error } => {
                match error {
                    Some(e) => self.toast(Severity::Error, format!("could not write {path}: {e}")),
                    None => self.toast(
                        Severity::Success,
                        format!("printer profile written to {path}"),
                    ),
                }
                Vec::new()
            }
            Msg::MediaDetected { queue, mm } => {
                if let Some(Overlay::Printer { form, .. }) = &mut self.overlay {
                    form.set_media(&queue, mm);
                }
                Vec::new()
            }
            Msg::PrintersDetected { queues, error } => {
                if let Some(Overlay::Printer { form, .. }) = &mut self.overlay {
                    form.set_detected(queues, error);
                }
                Vec::new()
            }
            Msg::Printed { job_id, error } => {
                if let Some(e) = &error {
                    self.toast(Severity::Error, format!("print failed: {e}"));
                }
                vec![self.send(Request::AckPrintJob { job_id, error }, Pending::Ack)]
            }
            Msg::Term(ev) => self.on_term(ev),
        }
    }

    fn on_response(&mut self, pending: Option<Pending>, response: Response) -> Vec<Cmd> {
        match (pending, response) {
            (Some(Pending::Welcome), Response::Welcome(w)) => {
                self.conn = Conn::Ready;
                self.scan = scan::detector_for(&w.user.prefs.scanner);
                self.users.insert(w.user.uid, w.user.username.clone());
                self.user = Some(w.user);
                self.boards = w.boards;
                vec![self.send(Request::ListUsers, Pending::Users)]
            }
            (Some(Pending::Users), Response::Users { users }) => {
                for u in users {
                    self.users.insert(u.uid, u.username);
                }
                Vec::new()
            }
            (Some(Pending::Boards), Response::Boards { boards }) => {
                self.boards = boards;
                self.board_sel = self.board_sel.min(self.boards.len().saturating_sub(1));
                if let Some(b) = &self.board
                    && !self.boards.iter().any(|s| s.id == b.detail.board.id)
                {
                    self.board = None;
                    self.toast(Severity::Warning, "that board is gone");
                }
                Vec::new()
            }
            (Some(Pending::OpenBoard { focus_task }), Response::Board(detail)) => {
                let mut view = BoardView::new(detail);
                if let Some(t) = focus_task {
                    view.select_task(t);
                }
                self.board = Some(view);
                self.hits.clear();
                Vec::new()
            }
            (Some(Pending::RefreshBoard), Response::Board(detail)) => {
                self.refresh_panels(&detail.board);
                if let Some(b) = &mut self.board
                    && b.detail.board.id == detail.board.id
                {
                    b.detail = detail;
                    b.clamp();
                }
                Vec::new()
            }
            (Some(Pending::Restore), Response::Task { task }) => {
                if let Some(b) = &mut self.board
                    && b.detail.board.id == task.board_id
                {
                    b.upsert(task);
                }
                Vec::new()
            }
            (Some(Pending::MoveTask { task: moved, .. }), Response::Task { task }) => {
                if let Some(b) = &mut self.board
                    && b.detail.board.id == task.board_id
                {
                    b.upsert(task);
                    // Keep the cursor on the picked up card, or moving it
                    // twice in a row works on the wrong task.
                    if b.picked == Some(moved) {
                        b.select_task(moved);
                    }
                }
                Vec::new()
            }
            (Some(Pending::ChecklistOp), Response::Task { task }) => {
                if let Some(Overlay::TaskDetail {
                    task: shown, sel, ..
                }) = &mut self.overlay
                    && shown.id == task.id
                {
                    *shown = task.clone();
                    *sel = (*sel).min(shown.checklist.len().saturating_sub(1));
                }
                if let Some(b) = &mut self.board
                    && b.detail.board.id == task.board_id
                {
                    b.upsert(task);
                }
                Vec::new()
            }
            (Some(Pending::FormMembers), Response::Members { members }) => {
                if let Some(Overlay::TaskForm(form)) = &mut self.overlay {
                    form.set_members(&members);
                }
                Vec::new()
            }
            (Some(Pending::FormDepSearch), Response::SearchResults { hits }) => {
                if let Some(Overlay::TaskForm(form)) = &mut self.overlay {
                    form.set_dep_hits(hits);
                }
                Vec::new()
            }
            (Some(Pending::SaveTask), Response::Task { task }) => {
                if matches!(self.overlay, Some(Overlay::TaskForm(_))) {
                    self.overlay = None;
                }
                self.toast(Severity::Success, format!("saved {}", task.title));
                if let Some(b) = &mut self.board
                    && b.detail.board.id == task.board_id
                {
                    b.upsert(task);
                }
                Vec::new()
            }
            (Some(Pending::Settings), Response::Done) => {
                if matches!(self.overlay, Some(Overlay::Settings(_))) {
                    self.overlay = None;
                }
                self.toast(Severity::Success, "settings saved");
                Vec::new()
            }
            (Some(Pending::Timezone), Response::Done) => Vec::new(),
            (Some(Pending::FormUsers), Response::Users { users }) => {
                if let Some(Overlay::BoardForm(form)) = &mut self.overlay {
                    form.set_users(&users);
                }
                Vec::new()
            }
            (Some(Pending::CreateBoardForm), Response::Board(detail)) => {
                self.overlay = None;
                self.toast(
                    Severity::Success,
                    format!("created board {}", detail.board.name),
                );
                self.board = Some(BoardView::new(detail));
                vec![self.send(Request::ListBoards, Pending::Boards)]
            }
            (Some(Pending::BoardOp), Response::Board(detail)) => {
                if matches!(self.overlay, Some(Overlay::BoardForm(_))) {
                    self.overlay = None;
                }
                self.toast(Severity::Success, "board settings saved");
                if let Some(b) = &mut self.board
                    && b.detail.board.id == detail.board.id
                {
                    b.detail = detail;
                    b.clamp();
                }
                vec![self.send(Request::ListBoards, Pending::Boards)]
            }
            (Some(Pending::PanelMembers), Response::Members { members }) => {
                if let Some(Overlay::Members(panel)) = &mut self.overlay {
                    panel.set_members(members);
                }
                Vec::new()
            }
            (Some(Pending::PanelUsers), Response::Users { users }) => {
                if let Some(Overlay::Members(panel)) = &mut self.overlay {
                    panel.set_users(users);
                }
                Vec::new()
            }
            (Some(Pending::MemberOp { .. }), Response::Done) => match &self.overlay {
                Some(Overlay::Members(panel)) => {
                    let board_id = panel.board_id;
                    vec![self.send(Request::ListMembers { board_id }, Pending::PanelMembers)]
                }
                _ => Vec::new(),
            },
            (Some(Pending::ColumnOp), Response::Board(detail)) => {
                let board = detail.board.clone();
                if let Some(b) = &mut self.board
                    && b.detail.board.id == detail.board.id
                {
                    b.detail = detail;
                    b.clamp();
                }
                // Only refresh an open panel. Sort toggles come through here
                // too, and those must not pop the panel open.
                if let Some(Overlay::Columns(panel)) = &mut self.overlay {
                    panel.refresh(&board);
                }
                Vec::new()
            }
            (Some(Pending::UsersPanel), Response::Users { users }) => {
                if let Some(Overlay::Users(panel)) = &mut self.overlay {
                    panel.set_users(users);
                }
                Vec::new()
            }
            (Some(Pending::AdminOp), Response::Done) => {
                if matches!(self.overlay, Some(Overlay::Users(_))) {
                    vec![self.send(Request::ListUsers, Pending::UsersPanel)]
                } else {
                    Vec::new()
                }
            }
            (Some(Pending::Archived), Response::Tasks { tasks }) => {
                let tz = self
                    .user
                    .as_ref()
                    .map(|u| u.timezone)
                    .unwrap_or(chrono_tz::UTC);
                let privileged = self.privileged_on_open_board();
                self.overlay = Some(Overlay::Archive(Box::new(ArchivePanel::new(
                    tasks, privileged, tz,
                ))));
                Vec::new()
            }
            (Some(Pending::TemplatesPanel), Response::Templates { templates }) => {
                let Some(board_id) = self.board.as_ref().map(|b| b.detail.board.id) else {
                    return Vec::new();
                };
                let names: HashMap<Uid, String> = self.users.clone();
                let name_of = move |uid: Uid| {
                    names
                        .get(&uid)
                        .cloned()
                        .unwrap_or_else(|| format!("uid {uid}"))
                };
                match &mut self.overlay {
                    Some(Overlay::Templates(panel)) if panel.board_id == board_id => {
                        panel.set_templates(templates, &name_of);
                    }
                    _ => {
                        let me = self.me();
                        let privileged = self.privileged_on_open_board();
                        let mut panel = TemplatesPanel::new(board_id, me, privileged);
                        panel.set_templates(templates, &name_of);
                        self.overlay = Some(Overlay::Templates(Box::new(panel)));
                    }
                }
                Vec::new()
            }
            (Some(Pending::TemplateOp), Response::Template { template }) => {
                if matches!(self.overlay, Some(Overlay::TaskForm(_))) {
                    self.overlay = None;
                }
                self.toast(
                    Severity::Success,
                    format!("saved template {}", template.name),
                );
                let board_id = template.board_id;
                vec![self.send(Request::ListTemplates { board_id }, Pending::TemplatesPanel)]
            }
            (Some(Pending::TemplateOp), Response::Done) => {
                self.toast(Severity::Info, "template deleted");
                match self.board.as_ref().map(|b| b.detail.board.id) {
                    Some(board_id) => vec![
                        self.send(Request::ListTemplates { board_id }, Pending::TemplatesPanel),
                    ],
                    None => Vec::new(),
                }
            }
            (Some(Pending::RepeatsPanel), Response::Repeats { entries }) => {
                let Some(board_id) = self.board.as_ref().map(|b| b.detail.board.id) else {
                    return Vec::new();
                };
                let tz = self
                    .user
                    .as_ref()
                    .map(|u| u.timezone)
                    .unwrap_or(chrono_tz::UTC);
                match &mut self.overlay {
                    Some(Overlay::Repeats(panel)) if panel.board_id == board_id => {
                        panel.set_entries(entries)
                    }
                    _ => {
                        let mut panel = RepeatsPanel::new(board_id, tz);
                        panel.set_entries(entries);
                        self.overlay = Some(Overlay::Repeats(Box::new(panel)));
                    }
                }
                Vec::new()
            }
            (Some(Pending::StopRepeat), Response::Task { task }) => {
                self.toast(
                    Severity::Success,
                    format!("{} no longer repeats", task.title),
                );
                if let Some(Overlay::Repeats(panel)) = &mut self.overlay {
                    panel.remove(task.id);
                }
                if let Some(b) = &mut self.board
                    && b.detail.board.id == task.board_id
                {
                    b.upsert(task);
                }
                Vec::new()
            }
            (Some(Pending::Search), Response::SearchResults { hits }) => {
                if hits.is_empty() {
                    self.toast(Severity::Info, "no matching tasks");
                }
                self.hits = hits;
                self.hit_sel = 0;
                Vec::new()
            }
            (Some(Pending::Scan), Response::Scan(outcome)) => self.on_scan(outcome),
            (Some(Pending::Print), Response::Done) => {
                self.toast(Severity::Info, "print job queued");
                Vec::new()
            }
            (Some(Pending::DeleteTask), Response::Task { task }) => {
                let days = self
                    .board
                    .as_ref()
                    .map(|b| b.detail.board.purge_deleted_after_secs / 86_400)
                    .unwrap_or(0);
                self.toast(
                    Severity::Info,
                    format!("moved to the archive, restore it from there within {days} days"),
                );
                if let Some(b) = &mut self.board {
                    b.remove(task.id);
                }
                Vec::new()
            }
            (Some(Pending::PurgeTask), Response::Done) => {
                self.toast(Severity::Info, "deleted for good");
                Vec::new()
            }
            (Some(Pending::DeleteBoard), Response::Done) => {
                self.toast(Severity::Info, "board deleted");
                vec![self.send(Request::ListBoards, Pending::Boards)]
            }
            _ => Vec::new(),
        }
    }

    fn on_scan(&mut self, outcome: ScanOutcome) -> Vec<Cmd> {
        match outcome {
            ScanOutcome::Applied {
                task, column_name, ..
            } => {
                self.toast(
                    Severity::Success,
                    format!("{} moved to {column_name}", task.title),
                );
                if let Some(b) = &mut self.board
                    && b.detail.board.id == task.board_id
                {
                    b.upsert(*task);
                }
                Vec::new()
            }
            ScanOutcome::UnknownTask => {
                self.toast(Severity::Error, "scan: unknown task");
                vec![self.bell()]
            }
            ScanOutcome::NoPermission => {
                self.toast(Severity::Error, "scan: you cannot see that task");
                vec![self.bell()]
            }
            ScanOutcome::Refused { reason } => {
                self.toast(Severity::Warning, format!("scan refused: {reason}"));
                vec![self.bell()]
            }
            ScanOutcome::BadScan { reason } => {
                self.toast(Severity::Error, format!("bad scan: {reason}"));
                vec![self.bell()]
            }
            ScanOutcome::BlockedByDependencies { task_id, open } => {
                match self.board.as_ref().map(|b| b.detail.board.finished_col) {
                    Some(to) => self.blocked_dialog(task_id, to, &open),
                    None => self.toast(
                        Severity::Warning,
                        "scan: open dependencies, open the board to override",
                    ),
                }
                vec![self.bell()]
            }
        }
    }

    fn on_error(&mut self, pending: Option<Pending>, error: ErrorBody) -> Vec<Cmd> {
        match (pending, error.code) {
            (Some(Pending::Welcome), _) => {
                self.conn = Conn::Lost;
                self.conn_error = Some(error.reason);
            }
            (Some(Pending::MoveTask { task, to }), ErrorCode::BlockedByDependencies) => {
                let open: Vec<TaskId> = error
                    .detail
                    .and_then(|d| serde_json::from_value(d).ok())
                    .unwrap_or_default();
                self.blocked_dialog(task, to, &open);
            }
            (Some(Pending::SaveTask), ErrorCode::Conflict) => {
                let current: Option<Task> =
                    error.detail.and_then(|d| serde_json::from_value(d).ok());
                match (self.overlay.take(), current) {
                    (Some(Overlay::TaskForm(mut form)), Some(current)) => {
                        form.saving = false;
                        self.overlay = Some(Overlay::Conflict {
                            form,
                            current: Box::new(current),
                        });
                    }
                    (other, _) => {
                        self.overlay = other;
                        self.toast(Severity::Error, error.reason);
                    }
                }
            }
            (Some(Pending::MemberOp { uid }), ErrorCode::MemberHasTasks) => {
                let count = error
                    .detail
                    .and_then(|d| serde_json::from_value::<Vec<TaskId>>(d).ok())
                    .map(|v| v.len())
                    .unwrap_or(0);
                let name = self.user_name(uid);
                if let Some(Overlay::Members(panel)) = self.overlay.take() {
                    let board_id = panel.board_id;
                    let plural = if count == 1 { "" } else { "s" };
                    self.confirm(
                        "Member has tasks",
                        format!("{name} still has {count} task{plural} assigned here.\nUnassign them and remove the member?"),
                        ConfirmAction::Send {
                            request: Request::RemoveMember { board_id, uid, unassign: true },
                            pending: Pending::MemberOp { uid },
                        },
                        Some(Box::new(Overlay::Members(panel))),
                    );
                } else {
                    self.toast(Severity::Error, error.reason);
                }
            }
            (Some(Pending::Settings | Pending::Timezone), _) => {
                if let Some(Overlay::Settings(form)) = &mut self.overlay {
                    form.saving = false;
                    form.error = Some(error.reason);
                } else {
                    self.toast(Severity::Error, error.reason);
                }
            }
            (Some(Pending::BoardOp | Pending::CreateBoardForm), _) => {
                if let Some(Overlay::BoardForm(form)) = &mut self.overlay {
                    form.saving = false;
                    form.error = Some(error.reason);
                } else {
                    self.toast(Severity::Error, error.reason);
                }
            }
            (Some(Pending::MemberOp { .. } | Pending::ColumnOp | Pending::AdminOp), _) => {
                match &mut self.overlay {
                    Some(Overlay::Members(p)) => p.error = Some(error.reason),
                    Some(Overlay::Columns(p)) => p.error = Some(error.reason),
                    Some(Overlay::Users(p)) => p.error = Some(error.reason),
                    _ => self.toast(Severity::Error, error.reason),
                }
            }
            (Some(Pending::SaveTask), _) => {
                if let Some(Overlay::TaskForm(form)) = &mut self.overlay {
                    form.saving = false;
                    form.error = Some(error.reason);
                } else {
                    self.toast(Severity::Error, error.reason);
                }
            }
            (Some(Pending::TemplateOp), _) => match &mut self.overlay {
                Some(Overlay::TaskForm(form)) => {
                    form.saving = false;
                    form.error = Some(error.reason);
                }
                Some(Overlay::Templates(panel)) => panel.error = Some(error.reason),
                _ => self.toast(Severity::Error, error.reason),
            },
            (Some(Pending::StopRepeat), _) => match &mut self.overlay {
                Some(Overlay::Repeats(panel)) => panel.error = Some(error.reason),
                _ => self.toast(Severity::Error, error.reason),
            },
            (_, _) => self.toast(Severity::Error, error.reason),
        }
        Vec::new()
    }

    fn on_event(&mut self, event: Event) -> Vec<Cmd> {
        match event {
            Event::BoardChanged { board_id, change } => {
                let mut cmds = vec![self.send(Request::ListBoards, Pending::Boards)];
                if self
                    .board
                    .as_ref()
                    .is_some_and(|b| b.detail.board.id == board_id)
                {
                    match change {
                        BoardChange::Deleted | BoardChange::AccessRevoked => {
                            self.board = None;
                            self.toast(
                                Severity::Warning,
                                "the board you were on is no longer available",
                            );
                        }
                        _ => cmds
                            .push(self.send(Request::GetBoard { board_id }, Pending::RefreshBoard)),
                    }
                }
                cmds
            }
            Event::TaskChanged { task, change, .. } => {
                if let Some(Overlay::TaskDetail {
                    task: shown, sel, ..
                }) = &mut self.overlay
                    && shown.id == task.id
                {
                    *shown = task.clone();
                    *sel = (*sel).min(shown.checklist.len().saturating_sub(1));
                }
                if let Some(Overlay::Archive(panel)) = &mut self.overlay
                    && self
                        .board
                        .as_ref()
                        .is_some_and(|b| b.detail.board.id == task.board_id)
                {
                    match change {
                        TaskChange::Purged | TaskChange::Restored => panel.remove(task.id),
                        TaskChange::Deleted | TaskChange::Archived => panel.upsert(task.clone()),
                        _ => {}
                    }
                }
                if let Some(b) = &mut self.board
                    && b.detail.board.id == task.board_id
                {
                    match change {
                        TaskChange::Deleted | TaskChange::Archived | TaskChange::Purged => {
                            b.remove(task.id)
                        }
                        _ => b.upsert(task),
                    }
                }
                Vec::new()
            }
            Event::PrintJob { job_id, job } => vec![Cmd::Print { job_id, job }],
            Event::Notice { text, severity } => {
                self.toast(severity, text);
                Vec::new()
            }
            Event::PrefsChanged { prefs } => {
                self.scan = scan::detector_for(&prefs.scanner);
                if let Some(u) = &mut self.user {
                    u.prefs = prefs;
                }
                Vec::new()
            }
        }
    }

    fn on_term(&mut self, ev: TermEvent) -> Vec<Cmd> {
        match ev {
            TermEvent::Resize(w, h) => {
                self.size = (w, h);
                Vec::new()
            }
            TermEvent::Key(k) if k.kind == KeyEventKind::Release => Vec::new(),
            TermEvent::Key(k) => self.on_key(k),
            TermEvent::Mouse(m) => {
                self.mouse_seen = true;
                self.mouse_pos = Some((m.column, m.row));
                self.on_mouse(m)
            }
            _ => Vec::new(),
        }
    }

    fn on_key(&mut self, k: KeyEvent) -> Vec<Cmd> {
        let ctrl_c = k.code == KeyCode::Char('c') && k.modifiers.contains(KeyModifiers::CONTROL);
        if self.too_small() || self.conn == Conn::Lost {
            // Nothing can be drawn properly, so any of the quit keys just quits.
            return if ctrl_c || matches!(k.code, KeyCode::Char('q') | KeyCode::Esc | KeyCode::Enter)
            {
                vec![Cmd::Quit]
            } else {
                Vec::new()
            };
        }
        if ctrl_c {
            self.confirm(
                "Quit",
                "Quit Taskologic? This closes your session.",
                ConfirmAction::Quit,
                None,
            );
            return Vec::new();
        }
        if self.is_widget_overlay() {
            return self.widget_overlay_event(TermEvent::Key(k));
        }
        match &self.overlay {
            Some(Overlay::Conflict { .. }) => return self.conflict_key(k),
            Some(Overlay::PickColumn { .. }) => return self.pick_column_key(k),
            Some(Overlay::TaskDetail { .. }) => return self.task_detail_key(k),
            Some(_) => return self.overlay_key(k),
            None => {}
        }
        if self.search.is_focused() {
            return self.search_key(k);
        }
        if self.scanner_listening()
            && let Some(sk) = scan::scan_key(&k)
        {
            return match self.scan.push(sk, self.now_ms) {
                Feed::Held => Vec::new(),
                Feed::Scan(Ok(p)) => {
                    if let Some(ms) = self.scan.last_scan_duration_ms() {
                        tracing::debug!(duration_ms = ms, "scan captured");
                    }
                    let magic = self
                        .user
                        .as_ref()
                        .map(|u| u.prefs.scanner.magic)
                        .unwrap_or_default();
                    let payload = p.encode(magic);
                    vec![self.send(Request::Scan { payload }, Pending::Scan)]
                }
                Feed::Scan(Err(e)) => {
                    self.toast(Severity::Error, format!("bad scan: {e}"));
                    vec![self.bell()]
                }
                Feed::Pass(keys) => keys
                    .into_iter()
                    .flat_map(|key| self.normal_key(scan::key_event(key)))
                    .collect(),
            };
        }
        self.normal_key(k)
    }

    fn normal_key(&mut self, k: KeyEvent) -> Vec<Cmd> {
        let shift = k.modifiers.contains(KeyModifiers::SHIFT);
        match k.code {
            KeyCode::Char('q') => {
                self.confirm(
                    "Quit",
                    "Quit Taskologic? This closes your session.",
                    ConfirmAction::Quit,
                    None,
                );
                Vec::new()
            }
            KeyCode::Char('?') => {
                self.overlay = Some(Overlay::Help);
                Vec::new()
            }
            KeyCode::Char('/') => {
                self.board = None;
                self.search.focus().set(true);
                Vec::new()
            }
            KeyCode::Char('b') => {
                self.board = None;
                Vec::new()
            }
            KeyCode::Char('M') => {
                let items = crate::ui::more_items(self);
                self.overlay = Some(Overlay::Menu {
                    items,
                    sel: None,
                    areas: Vec::new(),
                });
                Vec::new()
            }
            KeyCode::Char('S') => self.open_settings(),
            KeyCode::Char('s') => {
                if !self.scanner_enabled() {
                    self.toast(
                        Severity::Info,
                        "barcode scanner integration is off in your settings",
                    );
                } else if !self.scanner_manual_only() {
                    self.toast(
                        Severity::Info,
                        "the scanner is always listening on boards and the dashboard",
                    );
                } else {
                    self.manual_scan = !self.manual_scan;
                    let state = if self.manual_scan { "on" } else { "off" };
                    self.toast(Severity::Info, format!("manual scan mode {state}"));
                }
                Vec::new()
            }
            _ if self.board.is_some() => self.board_key(k, shift),
            _ => self.dashboard_key(k),
        }
    }

    fn dashboard_key(&mut self, k: KeyEvent) -> Vec<Cmd> {
        let searching = !self.hits.is_empty();
        let len = if searching {
            self.hits.len()
        } else {
            self.boards.len()
        };
        let sel = if searching {
            &mut self.hit_sel
        } else {
            &mut self.board_sel
        };
        match k.code {
            KeyCode::Up | KeyCode::Char('k') => *sel = sel.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => *sel = (*sel + 1).min(len.saturating_sub(1)),
            KeyCode::Esc if searching => {
                self.hits.clear();
                self.search.clear();
            }
            KeyCode::Enter if searching => {
                if let Some(hit) = self.hits.get(self.hit_sel) {
                    let (board_id, task) = (hit.board_id, hit.task_id);
                    return vec![self.send(
                        Request::GetBoard { board_id },
                        Pending::OpenBoard {
                            focus_task: Some(task),
                        },
                    )];
                }
            }
            KeyCode::Enter => return self.open_selected_board(),
            KeyCode::Char('u') => return self.open_users(),
            KeyCode::Char('i') => {
                self.include_archived.flip_checked();
            }
            KeyCode::Char('D') => {
                if let Some(b) = self.boards.get(self.board_sel) {
                    let privileged = self
                        .user
                        .as_ref()
                        .is_some_and(|u| u.is_admin || u.uid == b.owner_uid);
                    if !privileged {
                        self.toast(
                            Severity::Info,
                            "only the board owner or an admin can delete a board",
                        );
                    } else if b.is_locked {
                        self.toast(
                            Severity::Info,
                            "this board is locked, unlock it in board settings first",
                        );
                    } else {
                        let (board_id, name) = (b.id, b.name.clone());
                        self.confirm(
                            "Delete board",
                            format!("Delete the board \"{name}\" and everything on it?"),
                            ConfirmAction::Send {
                                request: Request::DeleteBoard { board_id },
                                pending: Pending::DeleteBoard,
                            },
                            None,
                        );
                    }
                }
            }
            KeyCode::Char('N') => return self.open_board_form_create(),
            _ => {}
        }
        Vec::new()
    }

    fn open_selected_board(&mut self) -> Vec<Cmd> {
        match self.boards.get(self.board_sel) {
            Some(b) if !b.is_member => {
                self.toast(Severity::Info, "you are not a member of this private board; as an admin you can only delete it (D)");
                Vec::new()
            }
            Some(b) => {
                let board_id = b.id;
                vec![self.send(
                    Request::GetBoard { board_id },
                    Pending::OpenBoard { focus_task: None },
                )]
            }
            None => Vec::new(),
        }
    }

    fn search_key(&mut self, k: KeyEvent) -> Vec<Cmd> {
        match k.code {
            KeyCode::Esc => {
                self.search.focus().set(false);
                Vec::new()
            }
            KeyCode::Enter => {
                self.search.focus().set(false);
                let query = self.search.text().trim().to_string();
                if query.is_empty() {
                    return Vec::new();
                }
                let include_archived = self.include_archived.checked();
                vec![self.send(
                    Request::Search {
                        query,
                        include_archived,
                    },
                    Pending::Search,
                )]
            }
            _ => {
                self.search.handle(&TermEvent::Key(k), Regular);
                Vec::new()
            }
        }
    }

    fn board_key(&mut self, k: KeyEvent, shift: bool) -> Vec<Cmd> {
        let can_print = self.show_print();
        let Some(b) = &mut self.board else {
            return Vec::new();
        };
        let cols = b.column_count();
        let picked = b.picked.is_some();
        match k.code {
            KeyCode::Left | KeyCode::Right | KeyCode::Up | KeyCode::Down if shift => {
                return self.move_picked(k.code);
            }
            // While a task is picked up the arrows carry it, no Shift needed.
            KeyCode::Left | KeyCode::Right | KeyCode::Up | KeyCode::Down if picked => {
                return self.move_picked(k.code);
            }
            KeyCode::Char('h') if picked => return self.move_picked(KeyCode::Left),
            KeyCode::Char('l') if picked => return self.move_picked(KeyCode::Right),
            KeyCode::Char('k') if picked => return self.move_picked(KeyCode::Up),
            KeyCode::Char('j') if picked => return self.move_picked(KeyCode::Down),
            KeyCode::Left | KeyCode::Char('h') | KeyCode::BackTab => {
                b.col = b.col.saturating_sub(1)
            }
            KeyCode::Right | KeyCode::Char('l') | KeyCode::Tab => {
                b.col = (b.col + 1).min(cols.saturating_sub(1))
            }
            KeyCode::Up | KeyCode::Char('k') => b.row = b.row.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => b.row += 1,
            KeyCode::Enter => {
                if let Some(t) = b.selected_task().cloned() {
                    self.open_detail(t);
                }
            }
            KeyCode::Char(' ') => {
                if b.picked.is_some() {
                    b.picked = None;
                } else if let Some(t) = b.selected_task() {
                    b.picked = Some(t.id);
                }
            }
            KeyCode::Esc => {
                if b.picked.is_some() {
                    b.picked = None;
                } else {
                    self.board = None;
                }
            }
            KeyCode::Char('n') => {
                if let Some(column) = b.column_id(b.col) {
                    let board = b.detail.board.id;
                    return self.open_task_form(FormMode::Create { board, column }, None);
                }
            }
            KeyCode::Char('e') => {
                if let Some(t) = b.selected_task().cloned() {
                    return self.open_task_form(
                        FormMode::Edit {
                            task: t.id,
                            version: t.version,
                        },
                        Some(&t),
                    );
                }
            }
            KeyCode::Char('d') => {
                if let Some(t) = b.selected_task() {
                    let (task_id, title) = (t.id, t.title.clone());
                    let days = b.detail.board.purge_deleted_after_secs / 86_400;
                    self.confirm(
                        "Delete task",
                        format!("Move \"{title}\" to the archive?\nIt can be restored from there for {days} days."),
                        ConfirmAction::Send { request: Request::DeleteTask { task_id }, pending: Pending::DeleteTask },
                        None,
                    );
                }
            }
            KeyCode::Char('t') => {
                if let Some(col) = b.detail.board.columns.get(b.col) {
                    let (column_id, sort_by_due) = (col.id, !col.sort_by_due);
                    return vec![self.send(
                        Request::SetColumnSort {
                            column_id,
                            sort_by_due,
                        },
                        Pending::ColumnOp,
                    )];
                }
            }
            KeyCode::Char('B') => return self.open_board_settings(),
            KeyCode::Char('m') => return self.open_members(),
            KeyCode::Char('c') => return self.open_columns(),
            KeyCode::Char('a') => {
                let board_id = b.detail.board.id;
                return vec![self.send(Request::ListArchived { board_id }, Pending::Archived)];
            }
            KeyCode::Char('u') => return self.open_users(),
            KeyCode::Char('T') => {
                let board_id = b.detail.board.id;
                return vec![
                    self.send(Request::ListTemplates { board_id }, Pending::TemplatesPanel),
                ];
            }
            KeyCode::Char('R') => {
                let board_id = b.detail.board.id;
                return vec![self.send(Request::ListRepeats { board_id }, Pending::RepeatsPanel)];
            }
            KeyCode::Char('p') => {
                if !can_print {
                    self.toast(Severity::Info, "no printer configured on this client");
                } else if let Some(t) = b.selected_task() {
                    let task_id = t.id;
                    return vec![self.send(Request::PrintTask { task_id }, Pending::Print)];
                }
            }
            _ => {}
        }
        if let Some(b) = &mut self.board {
            b.clamp();
        }
        Vec::new()
    }

    /// Shift+arrows: move the picked up task between columns or within one.
    fn move_picked(&mut self, code: KeyCode) -> Vec<Cmd> {
        let Some(b) = &mut self.board else {
            return Vec::new();
        };
        let Some(task_id) = b.picked else {
            self.toast(
                Severity::Info,
                "press Space to pick up the selected task first",
            );
            return Vec::new();
        };
        let Some(task) = b.detail.tasks.iter().find(|t| t.id == task_id).cloned() else {
            return Vec::new();
        };
        let current_col = b
            .detail
            .board
            .columns
            .iter()
            .position(|c| c.id == task.column_id)
            .unwrap_or(0);
        let siblings = b.tasks_in(current_col);
        let idx = siblings.iter().position(|t| t.id == task_id).unwrap_or(0);
        let (to_column, position) = match code {
            KeyCode::Left => match b.column_id(current_col.wrapping_sub(1)) {
                Some(c) => (c, None),
                None => return Vec::new(),
            },
            KeyCode::Right => match b.column_id(current_col + 1) {
                Some(c) => (c, None),
                None => return Vec::new(),
            },
            KeyCode::Up if idx > 0 => (task.column_id, Some(siblings[idx - 1].position - 1)),
            KeyCode::Down if idx + 1 < siblings.len() => {
                (task.column_id, Some(siblings[idx + 1].position + 1))
            }
            _ => return Vec::new(),
        };
        vec![self.send(
            Request::MoveTask {
                task_id,
                to_column,
                position,
                override_deps: false,
            },
            Pending::MoveTask {
                task: task_id,
                to: to_column,
            },
        )]
    }

    fn overlay_key(&mut self, _k: KeyEvent) -> Vec<Cmd> {
        let Some(overlay) = self.overlay.take() else {
            return Vec::new();
        };
        match overlay {
            // Any key closes the ones that are only there to be read.
            Overlay::Help => Vec::new(),
            // Widget overlays and the flow dialogs have their own handlers
            // and never reach this function. Put the overlay back untouched.
            other => {
                self.overlay = Some(other);
                Vec::new()
            }
        }
    }

    fn open_detail(&mut self, task: Task) {
        self.overlay = Some(Overlay::TaskDetail {
            task,
            sel: 0,
            item_areas: Vec::new(),
            area: Rect::default(),
        });
    }

    /// The task viewer: Up/Down walk the checklist, Space or Enter tick the
    /// selected item, anything else closes.
    fn task_detail_key(&mut self, k: KeyEvent) -> Vec<Cmd> {
        let Some(Overlay::TaskDetail { task, sel, .. }) = &mut self.overlay else {
            return Vec::new();
        };
        let items = task.checklist.len();
        match k.code {
            KeyCode::Up | KeyCode::Char('k') if items > 0 => {
                *sel = sel.saturating_sub(1);
                Vec::new()
            }
            KeyCode::Down | KeyCode::Char('j') if items > 0 => {
                *sel = (*sel + 1).min(items - 1);
                Vec::new()
            }
            KeyCode::Char(' ') | KeyCode::Enter if items > 0 => {
                let (task_id, index) = (task.id, *sel);
                let done = !task.checklist[index].done;
                vec![self.send(
                    Request::SetChecklistItem {
                        task_id,
                        index,
                        done,
                    },
                    Pending::ChecklistOp,
                )]
            }
            _ => {
                self.overlay = None;
                Vec::new()
            }
        }
    }

    /// Open the task form and ask for the member list to fill the
    /// assignee picker. `task` is the one being edited.
    fn open_task_form(&mut self, mode: FormMode, task: Option<&Task>) -> Vec<Cmd> {
        let tz = self
            .user
            .as_ref()
            .map(|u| u.timezone)
            .unwrap_or(chrono_tz::UTC);
        let board_id = match (&mode, task) {
            (FormMode::Create { board, .. }, _) => *board,
            (FormMode::TemplateNew { board } | FormMode::TemplateEdit { board, .. }, _) => *board,
            (FormMode::Edit { .. }, Some(t)) => t.board_id,
            (FormMode::Edit { .. }, None) => return Vec::new(),
        };
        let form = match task {
            Some(t) => {
                let titles: Vec<(TaskId, String)> = self
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
                TaskForm::edit(t, tz, &|id| {
                    titles
                        .iter()
                        .find(|(i, _)| *i == id)
                        .map(|(_, t)| t.clone())
                })
            }
            None => match mode {
                FormMode::Create { board, column } => TaskForm::create(board, column, tz),
                FormMode::TemplateNew { board } => TaskForm::template_new(board, tz),
                FormMode::Edit { .. } | FormMode::TemplateEdit { .. } => return Vec::new(),
            },
        };
        self.overlay = Some(Overlay::TaskForm(Box::new(form)));
        vec![self.send(Request::ListMembers { board_id }, Pending::FormMembers)]
    }

    fn form_event(&mut self, ev: TermEvent) -> Vec<Cmd> {
        let Some(Overlay::TaskForm(form)) = &mut self.overlay else {
            return Vec::new();
        };
        match form.handle(&ev) {
            FormOutcome::Continue | FormOutcome::Changed => Vec::new(),
            FormOutcome::Cancel => {
                self.overlay = None;
                Vec::new()
            }
            FormOutcome::SearchDeps(query) => {
                vec![self.send(
                    Request::Search {
                        query,
                        include_archived: false,
                    },
                    Pending::FormDepSearch,
                )]
            }
            FormOutcome::Save(draft) => {
                form.saving = true;
                let mode = form.mode.clone();
                self.save_task(mode, draft)
            }
        }
    }

    fn save_task(&mut self, mode: FormMode, draft: TaskDraft) -> Vec<Cmd> {
        let req = match mode {
            FormMode::Create { board, column } => Request::CreateTask {
                board_id: board,
                column_id: Some(column),
                draft,
            },
            FormMode::Edit { task, version } => Request::UpdateTask {
                task_id: task,
                version,
                draft,
            },
            // The template's name is its title; one less field to fill in.
            FormMode::TemplateNew { board } => {
                let name = draft.title.clone();
                return vec![self.send(
                    Request::CreateTemplate {
                        board_id: board,
                        name,
                        draft,
                    },
                    Pending::TemplateOp,
                )];
            }
            FormMode::TemplateEdit { template, .. } => {
                let name = draft.title.clone();
                return vec![self.send(
                    Request::UpdateTemplate {
                        template_id: template,
                        name,
                        draft,
                    },
                    Pending::TemplateOp,
                )];
            }
        };
        vec![self.send(req, Pending::SaveTask)]
    }

    /// The conflict dialog: `r` reloads the form from the server's version,
    /// `o` overwrites it with what is in the form, Esc goes back to editing
    /// against the current version.
    fn conflict_key(&mut self, k: KeyEvent) -> Vec<Cmd> {
        let Some(Overlay::Conflict { mut form, current }) = self.overlay.take() else {
            return Vec::new();
        };
        match k.code {
            KeyCode::Char('r') => {
                let t = *current;
                self.open_task_form(
                    FormMode::Edit {
                        task: t.id,
                        version: t.version,
                    },
                    Some(&t),
                )
            }
            KeyCode::Char('o') => {
                form.set_version(current.version);
                match form.values() {
                    Ok(draft) => {
                        form.saving = true;
                        let mode = form.mode.clone();
                        self.overlay = Some(Overlay::TaskForm(form));
                        self.save_task(mode, draft)
                    }
                    Err(e) => {
                        form.error = Some(e);
                        self.overlay = Some(Overlay::TaskForm(form));
                        Vec::new()
                    }
                }
            }
            _ => {
                form.set_version(current.version);
                self.overlay = Some(Overlay::TaskForm(form));
                Vec::new()
            }
        }
    }

    fn confirm(
        &mut self,
        title: &str,
        text: impl Into<String>,
        action: ConfirmAction,
        back: Option<Box<Overlay>>,
    ) {
        self.overlay = Some(Overlay::Confirm {
            dialog: Box::new(Confirm::new(title, text)),
            action,
            back,
        });
    }

    fn blocked_dialog(&mut self, task_id: TaskId, to: ColumnId, open: &[TaskId]) {
        let titles: Vec<String> = self
            .board
            .as_ref()
            .map(|b| {
                open.iter()
                    .map(|d| match b.detail.tasks.iter().find(|t| t.id == *d) {
                        Some(t) => t.title.clone(),
                        None => "a task on another board".to_string(),
                    })
                    .collect()
            })
            .unwrap_or_default();
        self.confirm(
            "Open dependencies",
            format!(
                "Still open:\n{}\nFinish anyway? The override is recorded.",
                titles.join("\n")
            ),
            ConfirmAction::Send {
                request: Request::MoveTask {
                    task_id,
                    to_column: to,
                    position: None,
                    override_deps: true,
                },
                pending: Pending::MoveTask { task: task_id, to },
            },
            None,
        );
    }

    /// The More menu. Picking an entry presses the key it stands for, so
    /// there is one implementation per action rather than two.
    fn menu_event(&mut self, ev: TermEvent) -> Vec<Cmd> {
        let Some(Overlay::Menu { items, sel, areas }) = &mut self.overlay else {
            return Vec::new();
        };
        let chosen = match ev {
            TermEvent::Key(k) if k.kind != KeyEventKind::Release => match k.code {
                KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('M') => {
                    self.overlay = None;
                    return Vec::new();
                }
                KeyCode::Up | KeyCode::Char('k') => {
                    *sel = Some(sel.map_or(items.len().saturating_sub(1), |i| i.saturating_sub(1)));
                    return Vec::new();
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    *sel = Some(sel.map_or(0, |i| (i + 1).min(items.len().saturating_sub(1))));
                    return Vec::new();
                }
                KeyCode::Enter => sel.and_then(|i| items.get(i)).map(|(_, key)| *key),
                // Typing an entry's own key works too.
                KeyCode::Char(c) => items.iter().find(|(_, key)| *key == c).map(|(_, key)| *key),
                _ => None,
            },
            TermEvent::Mouse(m) => {
                let at = areas
                    .iter()
                    .position(|a| a.contains(Position::new(m.column, m.row)));
                match m.kind {
                    // Follow the pointer, so the highlight means "this is
                    // what you are about to press".
                    MouseEventKind::Moved | MouseEventKind::Drag(_) => {
                        *sel = at;
                        return Vec::new();
                    }
                    MouseEventKind::Down(MouseButton::Left) => match at {
                        Some(i) => {
                            *sel = Some(i);
                            items.get(i).map(|(_, key)| *key)
                        }
                        None => {
                            // A click outside dismisses it.
                            self.overlay = None;
                            return Vec::new();
                        }
                    },
                    _ => None,
                }
            }
            _ => None,
        };
        match chosen {
            Some(key) => {
                self.overlay = None;
                self.normal_key(KeyEvent::new(KeyCode::Char(key), KeyModifiers::NONE))
            }
            None => Vec::new(),
        }
    }

    fn archive_event(&mut self, ev: TermEvent) -> Vec<Cmd> {
        let Some(Overlay::Archive(panel)) = &mut self.overlay else {
            return Vec::new();
        };
        match panel.handle(&ev) {
            ArchiveOutcome::Changed => Vec::new(),
            ArchiveOutcome::Cancel => {
                self.overlay = None;
                Vec::new()
            }
            ArchiveOutcome::Restore(task_id) => {
                vec![self.send(Request::RestoreTask { task_id }, Pending::Restore)]
            }
            ArchiveOutcome::View(task) => {
                self.open_detail(*task);
                Vec::new()
            }
            ArchiveOutcome::Purge(task) => {
                let Some(Overlay::Archive(panel)) = self.overlay.take() else {
                    return Vec::new();
                };
                let (task_id, title) = (task.id, task.title.clone());
                self.confirm(
                    "Delete permanently",
                    format!("Delete \"{title}\" for good? This cannot be undone."),
                    ConfirmAction::Send {
                        request: Request::PurgeTask { task_id },
                        pending: Pending::PurgeTask,
                    },
                    Some(Box::new(Overlay::Archive(panel))),
                );
                Vec::new()
            }
        }
    }

    fn templates_event(&mut self, ev: TermEvent) -> Vec<Cmd> {
        let Some(Overlay::Templates(panel)) = &mut self.overlay else {
            return Vec::new();
        };
        let tz = self
            .user
            .as_ref()
            .map(|u| u.timezone)
            .unwrap_or(chrono_tz::UTC);
        match panel.handle(&ev) {
            TemplatesOutcome::Changed => Vec::new(),
            TemplatesOutcome::Cancel => {
                self.overlay = None;
                Vec::new()
            }
            TemplatesOutcome::New => {
                let board = panel.board_id;
                self.open_task_form(FormMode::TemplateNew { board }, None)
            }
            TemplatesOutcome::Edit(tpl) => {
                let form = TaskForm::template_edit(&tpl, tz);
                self.overlay = Some(Overlay::TaskForm(Box::new(form)));
                vec![self.send(
                    Request::ListMembers {
                        board_id: tpl.board_id,
                    },
                    Pending::FormMembers,
                )]
            }
            TemplatesOutcome::Use(tpl) => {
                // The new task lands in the currently selected column, the
                // same place a plain new task would.
                let Some(b) = &self.board else {
                    return Vec::new();
                };
                let Some(column) = b.detail.board.columns.get(b.col).map(|c| c.id) else {
                    return Vec::new();
                };
                let form = TaskForm::create_from_template(tpl.board_id, column, tz, &tpl);
                self.overlay = Some(Overlay::TaskForm(Box::new(form)));
                vec![self.send(
                    Request::ListMembers {
                        board_id: tpl.board_id,
                    },
                    Pending::FormMembers,
                )]
            }
            TemplatesOutcome::Delete(tpl) => {
                let Some(Overlay::Templates(panel)) = self.overlay.take() else {
                    return Vec::new();
                };
                self.confirm(
                    "Delete template",
                    format!(
                        "Delete the template \"{}\"? Tasks made from it stay.",
                        tpl.name
                    ),
                    ConfirmAction::Send {
                        request: Request::DeleteTemplate {
                            template_id: tpl.id,
                        },
                        pending: Pending::TemplateOp,
                    },
                    Some(Box::new(Overlay::Templates(panel))),
                );
                Vec::new()
            }
        }
    }

    fn repeats_event(&mut self, ev: TermEvent) -> Vec<Cmd> {
        let Some(Overlay::Repeats(panel)) = &mut self.overlay else {
            return Vec::new();
        };
        match panel.handle(&ev) {
            RepeatsOutcome::Changed => Vec::new(),
            RepeatsOutcome::Cancel => {
                self.overlay = None;
                Vec::new()
            }
            RepeatsOutcome::Stop(task_id) => {
                vec![self.send(Request::StopRepeat { task_id }, Pending::StopRepeat)]
            }
        }
    }

    fn confirm_event(&mut self, ev: TermEvent) -> Vec<Cmd> {
        let Some(Overlay::Confirm { dialog, .. }) = &mut self.overlay else {
            return Vec::new();
        };
        let answer = dialog.handle(&ev);
        if answer == ConfirmOutcome::Continue || answer == ConfirmOutcome::Changed {
            return Vec::new();
        }
        let Some(Overlay::Confirm { action, back, .. }) = self.overlay.take() else {
            return Vec::new();
        };
        self.overlay = back.map(|b| *b);
        match (answer, action) {
            (ConfirmOutcome::Yes, ConfirmAction::Quit) => vec![Cmd::Quit],
            (ConfirmOutcome::Yes, ConfirmAction::Send { request, pending }) => {
                vec![self.send(request, pending)]
            }
            _ => Vec::new(),
        }
    }

    fn widget_overlay_event(&mut self, ev: TermEvent) -> Vec<Cmd> {
        match &self.overlay {
            Some(Overlay::Confirm { .. }) => self.confirm_event(ev),
            Some(Overlay::Menu { .. }) => self.menu_event(ev),
            Some(Overlay::Archive(_)) => self.archive_event(ev),
            Some(Overlay::TaskForm(_)) => self.form_event(ev),
            Some(Overlay::Settings(_)) => self.settings_event(ev),
            Some(Overlay::Colors { .. }) => self.colors_event(ev),
            Some(Overlay::Printer { .. }) => self.printer_event(ev),
            Some(Overlay::BoardForm(_)) => self.board_form_event(ev),
            Some(Overlay::Members(_)) => self.members_event(ev),
            Some(Overlay::Columns(_)) => self.columns_event(ev),
            Some(Overlay::Users(_)) => self.users_event(ev),
            Some(Overlay::Templates(_)) => self.templates_event(ev),
            Some(Overlay::Repeats(_)) => self.repeats_event(ev),
            _ => Vec::new(),
        }
    }

    fn open_settings(&mut self) -> Vec<Cmd> {
        let Some(user) = &self.user else {
            return Vec::new();
        };
        let form = SettingsForm::new(user, self.printer.as_ref(), self.mouse_seen);
        self.overlay = Some(Overlay::Settings(Box::new(form)));
        Vec::new()
    }

    fn settings_event(&mut self, ev: TermEvent) -> Vec<Cmd> {
        let Some(Overlay::Settings(form)) = &mut self.overlay else {
            return Vec::new();
        };
        match form.handle(&ev) {
            SettingsOutcome::Continue | SettingsOutcome::Changed => Vec::new(),
            SettingsOutcome::Cancel => {
                self.overlay = None;
                Vec::new()
            }
            SettingsOutcome::EditColors(colors) => {
                let Some(Overlay::Settings(back)) = self.overlay.take() else {
                    return Vec::new();
                };
                self.overlay = Some(Overlay::Colors {
                    form: Box::new(ColorsForm::new(&colors)),
                    back,
                });
                Vec::new()
            }
            SettingsOutcome::Printer => {
                let Some(Overlay::Settings(back)) = self.overlay.take() else {
                    return Vec::new();
                };
                let form = Box::new(PrinterForm::new(self.printer.as_ref()));
                let mut cmds = vec![Cmd::DetectPrinters];
                // Ask about the configured queue's media too, so an existing
                // profile can show what the printer itself is set up for.
                cmds.extend(form.initial_queue().map(Cmd::DetectMedia));
                self.overlay = Some(Overlay::Printer { form, back });
                cmds
            }
            SettingsOutcome::Save { prefs, timezone } => {
                form.saving = true;
                // Apply locally right away; the daemon answer only confirms it.
                self.scan = scan::detector_for(&prefs.scanner);
                if let Some(u) = &mut self.user {
                    u.prefs = prefs.clone();
                    if let Some(tz) = timezone {
                        u.timezone = tz;
                    }
                }
                let mut cmds = vec![self.send(Request::UpdatePrefs { prefs }, Pending::Settings)];
                if let Some(tz) = timezone {
                    cmds.push(self.send(
                        Request::SetTimezone {
                            timezone: tz.name().to_string(),
                        },
                        Pending::Timezone,
                    ));
                }
                cmds
            }
        }
    }

    fn colors_event(&mut self, ev: TermEvent) -> Vec<Cmd> {
        let Some(Overlay::Colors { form, .. }) = &mut self.overlay else {
            return Vec::new();
        };
        let answer = form.handle(&ev);
        match answer {
            ColorsOutcome::Changed => Vec::new(),
            ColorsOutcome::Cancel | ColorsOutcome::Save(_) => {
                let Some(Overlay::Colors { back, .. }) = self.overlay.take() else {
                    return Vec::new();
                };
                let mut back = back;
                if let ColorsOutcome::Save(colors) = answer {
                    back.set_colors(*colors);
                }
                self.overlay = Some(Overlay::Settings(back));
                Vec::new()
            }
        }
    }

    fn printer_event(&mut self, ev: TermEvent) -> Vec<Cmd> {
        let Some(Overlay::Printer { form, .. }) = &mut self.overlay else {
            return Vec::new();
        };
        match form.handle(&ev) {
            PrinterOutcome::Changed => Vec::new(),
            PrinterOutcome::QueueChanged(queue) => vec![Cmd::DetectMedia(queue)],
            PrinterOutcome::Refresh => vec![Cmd::DetectPrinters],
            PrinterOutcome::TestPrint(profile) => match self.test_print_job() {
                Some(job) => vec![Cmd::TestPrint {
                    job,
                    profile: *profile,
                }],
                None => Vec::new(),
            },
            PrinterOutcome::Cancel => {
                let Some(Overlay::Printer { back, .. }) = self.overlay.take() else {
                    return Vec::new();
                };
                self.overlay = Some(Overlay::Settings(back));
                Vec::new()
            }
            PrinterOutcome::Save(profile) => {
                let Some(Overlay::Printer { back, .. }) = self.overlay.take() else {
                    return Vec::new();
                };
                let mut back = back;
                back.set_printer(profile.as_ref());
                self.overlay = Some(Overlay::Settings(back));
                if profile != self.printer {
                    self.printer = profile.clone();
                    vec![Cmd::SavePrinter(profile)]
                } else {
                    Vec::new()
                }
            }
        }
    }

    /// A slip that exercises every field of the local printer profile.
    fn test_print_job(&self) -> Option<PrintJob> {
        let user = self.user.as_ref()?;
        let now = chrono::Utc::now();
        let board = Board {
            id: taskologic_core::ids::BoardId(0),
            name: "Taskologic".into(),
            description: String::new(),
            owner_uid: user.uid,
            is_locked: false,
            is_private: false,
            archive_after_secs: 0,
            purge_deleted_after_secs: 0,
            card_fields: CardFields::default(),
            started_col: ColumnId(2),
            paused_col: ColumnId(3),
            finished_col: ColumnId(4),
            created_at: now,
            columns: Vec::new(),
            members: vec![user.uid],
        };
        let task = Task {
            id: TaskId(0),
            short_id: taskologic_core::ids::ShortId::from_index(0),
            board_id: board.id,
            column_id: ColumnId(1),
            position: 0,
            title: "Test print".into(),
            description: "If you can read this, the printer profile works. Umlauts: äöü ß".into(),
            due_at: Some(now + chrono::TimeDelta::hours(1)),
            created_by: user.uid,
            created_at: now,
            finished_at: None,
            version: 1,
            archived_at: None,
            archived_from_col: None,
            deleted_at: None,
            assignees: vec![user.uid],
            depends_on: Vec::new(),
            checklist: Vec::new(),
            repeat: None,
        };
        let name = user.username.clone();
        let mut job = taskologic_core::print::build_task_job(
            &task,
            &board,
            &[],
            &|_| name.clone(),
            user,
            now,
        );
        // A test print is about the printer, not the prefs: put a barcode on
        // the slip whatever the scanner settings say, so there is something to
        // hold a scanner up to. Text output cannot draw one and prints the
        // payload instead, which still says whether it got that far.
        // One per action, so whichever barcode the slip layout shows carries
        // the sample rather than a code that means something.
        let sym = user.prefs.scanner.format;
        job.barcodes = vec![
            taskologic_core::print::sample_barcode(sym, ScanAction::StartPause),
            taskologic_core::print::sample_barcode(sym, ScanAction::Finish),
        ];
        Some(job)
    }

    fn open_board_form_create(&mut self) -> Vec<Cmd> {
        let me = self.me();
        self.overlay = Some(Overlay::BoardForm(Box::new(BoardForm::create(me))));
        vec![self.send(Request::ListUsers, Pending::FormUsers)]
    }

    fn open_board_settings(&mut self) -> Vec<Cmd> {
        if !self.privileged_on_open_board() {
            self.toast(
                Severity::Info,
                "only the board owner or an admin can change board settings",
            );
            return Vec::new();
        }
        let me = self.me();
        let Some(b) = &self.board else {
            return Vec::new();
        };
        let form = BoardForm::edit(&b.detail.board, me);
        self.overlay = Some(Overlay::BoardForm(Box::new(form)));
        Vec::new()
    }

    fn board_form_event(&mut self, ev: TermEvent) -> Vec<Cmd> {
        let Some(Overlay::BoardForm(form)) = &mut self.overlay else {
            return Vec::new();
        };
        match form.handle(&ev) {
            BoardOutcome::Continue | BoardOutcome::Changed => Vec::new(),
            BoardOutcome::Cancel => {
                self.overlay = None;
                Vec::new()
            }
            BoardOutcome::OpenMembers => self.open_members(),
            BoardOutcome::OpenColumns => self.open_columns(),
            BoardOutcome::Create(req) => {
                form.saving = true;
                vec![self.send(Request::CreateBoard(*req), Pending::CreateBoardForm)]
            }
            BoardOutcome::Update(req) => {
                let req = *req;
                // Going private keeps everyone with a task on as a member. Say
                // who that is before it happens.
                if req.is_private == Some(true)
                    && let Some(b) = &self.board
                {
                    let mut assigned: Vec<Uid> = b
                        .detail
                        .tasks
                        .iter()
                        .filter(|t| !t.is_archived())
                        .flat_map(|t| t.assignees.iter().copied())
                        .collect();
                    assigned.sort_unstable();
                    assigned.dedup();
                    let carry: Vec<String> = b
                        .detail
                        .board
                        .would_lose_access(&assigned)
                        .into_iter()
                        .map(|u| self.user_name(u))
                        .collect();
                    if !carry.is_empty() {
                        self.confirm(
                            "Going private",
                            format!(
                                "These people have tasks here and stay on as members:\n{}\nContinue?",
                                carry.join(", ")
                            ),
                            ConfirmAction::Send { request: Request::UpdateBoard(req), pending: Pending::BoardOp },
                            None,
                        );
                        return Vec::new();
                    }
                }
                if let Some(Overlay::BoardForm(form)) = &mut self.overlay {
                    form.saving = true;
                }
                vec![self.send(Request::UpdateBoard(req), Pending::BoardOp)]
            }
        }
    }

    fn open_members(&mut self) -> Vec<Cmd> {
        let privileged = self.privileged_on_open_board();
        let Some(b) = &self.board else {
            return Vec::new();
        };
        let board_id = b.detail.board.id;
        let panel = MembersPanel::new(&b.detail.board, privileged);
        self.overlay = Some(Overlay::Members(Box::new(panel)));
        vec![
            self.send(Request::ListMembers { board_id }, Pending::PanelMembers),
            self.send(Request::ListUsers, Pending::PanelUsers),
        ]
    }

    fn members_event(&mut self, ev: TermEvent) -> Vec<Cmd> {
        let Some(Overlay::Members(panel)) = &mut self.overlay else {
            return Vec::new();
        };
        let board_id = panel.board_id;
        match panel.handle(&ev) {
            MembersOutcome::Changed => Vec::new(),
            MembersOutcome::Cancel => {
                self.overlay = None;
                Vec::new()
            }
            MembersOutcome::Add(uid) => vec![self.send(
                Request::AddMember { board_id, uid },
                Pending::MemberOp { uid },
            )],
            MembersOutcome::Remove(uid) => {
                vec![self.send(
                    Request::RemoveMember {
                        board_id,
                        uid,
                        unassign: false,
                    },
                    Pending::MemberOp { uid },
                )]
            }
        }
    }

    fn open_columns(&mut self) -> Vec<Cmd> {
        if !self.privileged_on_open_board() {
            self.toast(
                Severity::Info,
                "only the board owner or an admin can change columns",
            );
            return Vec::new();
        }
        let Some(b) = &self.board else {
            return Vec::new();
        };
        self.overlay = Some(Overlay::Columns(Box::new(ColumnsPanel::new(
            &b.detail.board,
        ))));
        Vec::new()
    }

    fn columns_event(&mut self, ev: TermEvent) -> Vec<Cmd> {
        let Some(Overlay::Columns(panel)) = &mut self.overlay else {
            return Vec::new();
        };
        let board_id = panel.board.id;
        match panel.handle(&ev) {
            ColumnsOutcome::Changed => Vec::new(),
            ColumnsOutcome::Cancel => {
                self.overlay = None;
                Vec::new()
            }
            ColumnsOutcome::Add(name) => {
                vec![self.send(Request::AddColumn { board_id, name }, Pending::ColumnOp)]
            }
            ColumnsOutcome::Rename(column_id, name) => {
                vec![self.send(Request::RenameColumn { column_id, name }, Pending::ColumnOp)]
            }
            ColumnsOutcome::Reorder(order) => vec![self.send(
                Request::ReorderColumns { board_id, order },
                Pending::ColumnOp,
            )],
            ColumnsOutcome::SetRole(role, id) => {
                let req = taskologic_proto::UpdateBoard {
                    board_id,
                    name: None,
                    description: None,
                    is_locked: None,
                    is_private: None,
                    archive_after_secs: None,
                    purge_deleted_after_secs: None,
                    card_fields: None,
                    roles: vec![(role, id)],
                };
                vec![self.send(Request::UpdateBoard(req), Pending::ColumnOp)]
            }
            ColumnsOutcome::Remove(id) => self.start_remove_flow(id),
        }
    }

    fn start_remove_flow(&mut self, column: ColumnId) -> Vec<Cmd> {
        let Some(Overlay::Columns(panel)) = self.overlay.take() else {
            return Vec::new();
        };
        let has_tasks = self.board.as_ref().is_some_and(|b| {
            b.detail
                .tasks
                .iter()
                .any(|t| t.column_id == column && !t.is_archived())
        });
        let roles = panel.board.roles_of(column);
        let flow = RemoveFlow {
            column,
            has_tasks,
            destination: None,
            roles,
            replacements: Vec::new(),
        };
        self.advance_remove_flow(panel, flow)
    }

    /// Ask the next open question, or confirm once everything is answered.
    fn advance_remove_flow(&mut self, panel: Box<ColumnsPanel>, flow: RemoveFlow) -> Vec<Cmd> {
        let name = panel
            .board
            .column(flow.column)
            .map(|c| c.name.clone())
            .unwrap_or_default();
        let options: Vec<(ColumnId, String)> = panel
            .board
            .columns
            .iter()
            .filter(|c| c.id != flow.column)
            .map(|c| (c.id, c.name.clone()))
            .collect();
        if flow.has_tasks && flow.destination.is_none() {
            let title = format!("Where should the tasks in \"{name}\" go?");
            self.overlay = Some(Overlay::PickColumn {
                panel,
                title,
                options,
                sel: 0,
                flow,
            });
            return Vec::new();
        }
        if let Some(role) = flow.next_role() {
            let title = format!(
                "\"{name}\" is the {} column. Which column takes over?",
                role.label()
            );
            self.overlay = Some(Overlay::PickColumn {
                panel,
                title,
                options,
                sel: 0,
                flow,
            });
            return Vec::new();
        }
        let request = Request::RemoveColumn {
            column_id: flow.column,
            move_tasks_to: flow.destination,
            replacements: flow.replacements.clone(),
        };
        self.confirm(
            "Remove column",
            format!("Remove the column \"{name}\"?"),
            ConfirmAction::Send {
                request,
                pending: Pending::ColumnOp,
            },
            Some(Box::new(Overlay::Columns(panel))),
        );
        Vec::new()
    }

    fn pick_column_key(&mut self, k: KeyEvent) -> Vec<Cmd> {
        let Some(Overlay::PickColumn {
            panel,
            title,
            options,
            mut sel,
            mut flow,
        }) = self.overlay.take()
        else {
            return Vec::new();
        };
        match k.code {
            KeyCode::Up | KeyCode::Char('k') => sel = sel.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => {
                sel = (sel + 1).min(options.len().saturating_sub(1))
            }
            KeyCode::Esc | KeyCode::Char('q') => {
                self.overlay = Some(Overlay::Columns(panel));
                return Vec::new();
            }
            KeyCode::Enter => {
                if let Some((id, _)) = options.get(sel) {
                    if flow.has_tasks && flow.destination.is_none() {
                        flow.destination = Some(*id);
                    } else if let Some(role) = flow.next_role() {
                        flow.replacements.push((role, *id));
                    }
                }
                return self.advance_remove_flow(panel, flow);
            }
            _ => {}
        }
        self.overlay = Some(Overlay::PickColumn {
            panel,
            title,
            options,
            sel,
            flow,
        });
        Vec::new()
    }

    fn open_users(&mut self) -> Vec<Cmd> {
        if !self.user.as_ref().is_some_and(|u| u.is_admin) {
            self.toast(Severity::Info, "only admins can manage users");
            return Vec::new();
        }
        let me = self.me();
        self.overlay = Some(Overlay::Users(Box::new(UsersPanel::new(me))));
        vec![self.send(Request::ListUsers, Pending::UsersPanel)]
    }

    fn users_event(&mut self, ev: TermEvent) -> Vec<Cmd> {
        let Some(Overlay::Users(panel)) = &mut self.overlay else {
            return Vec::new();
        };
        match panel.handle(&ev) {
            UsersOutcome::Changed => Vec::new(),
            UsersOutcome::Cancel => {
                self.overlay = None;
                Vec::new()
            }
            UsersOutcome::SetAdmin { uid, is_admin } => {
                vec![self.send(Request::SetAdmin { uid, is_admin }, Pending::AdminOp)]
            }
        }
    }

    /// The board changed underneath an open panel or form.
    fn refresh_panels(&mut self, board: &Board) {
        match &mut self.overlay {
            Some(Overlay::Columns(p)) if p.board.id == board.id => p.refresh(board),
            Some(Overlay::BoardForm(f)) if f.board_id() == Some(board.id) => f.refresh(board),
            _ => {}
        }
    }

    fn on_mouse(&mut self, m: MouseEvent) -> Vec<Cmd> {
        if self.too_small() || self.conn != Conn::Ready {
            return Vec::new();
        }
        if self.is_widget_overlay() {
            return self.widget_overlay_event(TermEvent::Mouse(m));
        }
        if let Some(overlay) = &self.overlay {
            // The task viewer answers clicks: an item toggles, outside closes.
            if let Overlay::TaskDetail {
                task,
                item_areas,
                area,
                ..
            } = overlay
            {
                if let MouseEventKind::Down(MouseButton::Left) = m.kind {
                    let pos = Position::new(m.column, m.row);
                    if let Some(i) = item_areas.iter().position(|a| a.contains(pos)) {
                        let task_id = task.id;
                        let done = !task.checklist.get(i).map(|c| c.done).unwrap_or(true);
                        if let Some(Overlay::TaskDetail { sel, .. }) = &mut self.overlay {
                            *sel = i;
                        }
                        return vec![self.send(
                            Request::SetChecklistItem {
                                task_id,
                                index: i,
                                done,
                            },
                            Pending::ChecklistOp,
                        )];
                    }
                    if !area.contains(pos) {
                        self.overlay = None;
                    }
                }
                return Vec::new();
            }
            // Overlays that are only there to be read close on any click.
            let dismissable = matches!(overlay, Overlay::Help | Overlay::PickColumn { .. });
            if dismissable && matches!(m.kind, MouseEventKind::Down(MouseButton::Left)) {
                self.overlay = None;
            }
            return Vec::new();
        }
        let (x, y) = (m.column, m.row);
        if let MouseEventKind::Down(MouseButton::Left) = m.kind
            && let Some((_, key)) = self
                .menu_areas
                .iter()
                .find(|(a, _)| a.contains(Position::new(x, y)))
        {
            let key = *key;
            return self.normal_key(KeyEvent::new(KeyCode::Char(key), KeyModifiers::NONE));
        }
        if let MouseEventKind::Down(MouseButton::Left) = m.kind
            && let Some((_, id)) = self
                .tab_areas
                .iter()
                .find(|(a, _)| a.contains(Position::new(x, y)))
        {
            let board_id = *id;
            if self
                .board
                .as_ref()
                .is_some_and(|b| b.detail.board.id == board_id)
            {
                return Vec::new();
            }
            return vec![self.send(
                Request::GetBoard { board_id },
                Pending::OpenBoard { focus_task: None },
            )];
        }
        if self.board.is_some() {
            return self.board_mouse(m);
        }
        match m.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                self.include_archived.handle(&TermEvent::Mouse(m), Regular);
                if self.search_area.contains(Position::new(x, y)) {
                    self.search.focus().set(true);
                    return Vec::new();
                }
                self.search.focus().set(false);
                if let Some(i) = self
                    .hit_areas
                    .iter()
                    .position(|a| a.contains(Position::new(x, y)))
                {
                    self.hit_sel = i;
                    return self.dashboard_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
                }
                if let Some(i) = self
                    .board_areas
                    .iter()
                    .position(|a| a.contains(Position::new(x, y)))
                {
                    self.board_sel = i;
                    return self.open_selected_board();
                }
                Vec::new()
            }
            MouseEventKind::ScrollUp => {
                self.dashboard_key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE))
            }
            MouseEventKind::ScrollDown => {
                self.dashboard_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE))
            }
            _ => Vec::new(),
        }
    }

    fn board_mouse(&mut self, m: MouseEvent) -> Vec<Cmd> {
        let Some(b) = &mut self.board else {
            return Vec::new();
        };
        let (x, y) = (m.column, m.row);
        // MouseFlags remembers whether the button went down inside the board,
        // so a drag that wanders outside still counts.
        let dragging = b.mouse.drag(b.area, &m);
        // Double click on a card opens it, before the drop logic below eats
        // the second Up event.
        if let Some((col, row, id)) = b.task_at(x, y) {
            let rect = b
                .task_areas
                .get(col - b.col_offset)
                .and_then(|slot| slot.iter().find(|(t, _)| *t == id))
                .map(|(_, r)| *r);
            if let Some(rect) = rect
                && b.mouse.doubleclick(rect, &m)
            {
                b.drag = None;
                b.hover_col = None;
                b.col = col;
                b.row = row;
                if let Some(t) = b.selected_task().cloned() {
                    self.open_detail(t);
                }
                return Vec::new();
            }
        }
        match m.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                // Buttons drawn on the column frames come first.
                if b.left_arrow
                    .is_some_and(|a| a.contains(Position::new(x, y)))
                {
                    b.col = b.col.saturating_sub(1);
                    b.clamp();
                    return Vec::new();
                }
                if b.right_arrow
                    .is_some_and(|a| a.contains(Position::new(x, y)))
                {
                    b.col = (b.col + 1).min(b.column_count().saturating_sub(1));
                    b.clamp();
                    return Vec::new();
                }
                if let Some(idx) = BoardView::hit(&b.up_areas, x, y) {
                    if let Some(s) = b.col_scroll.get_mut(idx) {
                        *s = s.saturating_sub(1);
                    }
                    return Vec::new();
                }
                if let Some(idx) = BoardView::hit(&b.down_areas, x, y) {
                    if let Some(s) = b.col_scroll.get_mut(idx) {
                        *s += 1;
                    }
                    return Vec::new();
                }
                if let Some(idx) = BoardView::hit(&b.sort_areas, x, y) {
                    if let Some(col) = b.detail.board.columns.get(idx) {
                        let (column_id, sort_by_due) = (col.id, !col.sort_by_due);
                        return vec![self.send(
                            Request::SetColumnSort {
                                column_id,
                                sort_by_due,
                            },
                            Pending::ColumnOp,
                        )];
                    }
                    return Vec::new();
                }
                if let Some(idx) = BoardView::hit(&b.plus_areas, x, y) {
                    b.col = idx;
                    b.clamp();
                    if let Some(column) = b.column_id(idx) {
                        let board = b.detail.board.id;
                        return self.open_task_form(FormMode::Create { board, column }, None);
                    }
                    return Vec::new();
                }
                if let Some((col, row, id)) = b.task_at(x, y) {
                    b.col = col;
                    b.row = row;
                    b.drag = Some((id, col));
                } else if let Some(col) = b.column_at(x, y) {
                    b.col = col;
                    b.clamp();
                }
                Vec::new()
            }
            MouseEventKind::Drag(MouseButton::Left) if dragging => {
                b.hover_col = b.column_at(x, y);
                Vec::new()
            }
            MouseEventKind::Up(MouseButton::Left) => {
                let drop = b.drag.take();
                let hover = b.column_at(x, y).or_else(|| b.hover_col.take());
                b.hover_col = None;
                match (drop, hover) {
                    (Some((task_id, from)), Some(to)) => {
                        // Same column means a reorder, so work out where in
                        // the list the card was let go.
                        let position = b.drop_position(to, y, task_id);
                        if to == from && position.is_none() {
                            return Vec::new();
                        }
                        match b.column_id(to) {
                            Some(to_column) => vec![self.send(
                                Request::MoveTask {
                                    task_id,
                                    to_column,
                                    position,
                                    override_deps: false,
                                },
                                Pending::MoveTask {
                                    task: task_id,
                                    to: to_column,
                                },
                            )],
                            None => Vec::new(),
                        }
                    }
                    _ => Vec::new(),
                }
            }
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
                if let Some(col) = b.column_at(x, y) {
                    b.col = col;
                }
                if m.kind == MouseEventKind::ScrollUp {
                    b.row = b.row.saturating_sub(1);
                } else {
                    b.row += 1;
                }
                b.clamp();
                Vec::new()
            }
            _ => Vec::new(),
        }
    }
}

#[cfg(test)]
pub mod test_support {
    use super::*;
    use taskologic_core::board::test_support::board_with_members;
    use taskologic_core::ids::BoardId;
    use taskologic_core::task::test_support::task_on;
    use taskologic_core::user::User;
    use taskologic_proto::Welcome;

    pub fn user(scanner: bool) -> User {
        let mut prefs = taskologic_core::prefs::UserPrefs::default();
        prefs.ui.scanner_enabled = scanner;
        User {
            uid: 1,
            username: "alice".into(),
            is_admin: true,
            timezone: chrono_tz::UTC,
            prefs,
            has_pin: false,
            created_at: chrono::DateTime::from_timestamp(0, 0).unwrap(),
        }
    }

    pub fn ready_app(scanner: bool) -> App {
        // Full colour and unicode, so the snapshots show the real thing.
        let mut app = App::new(false, ColorMode::Full, None);
        app.size = (80, 24);
        let cmds = app.start(Hello {
            protocol_version: 1,
            client_version: "test".into(),
            has_printer: false,
            print_priority: 0,
            capabilities: vec![],
        });
        let Cmd::Send(hello) = &cmds[0] else { panic!() };
        let welcome = Welcome {
            server_version: "test".into(),
            protocol_version: 1,
            user: user(scanner),
            boards: vec![
                BoardSummary {
                    id: BoardId(1),
                    name: "Kitchen".into(),
                    description: "Shopping and chores".into(),
                    owner_uid: 1,
                    is_locked: false,
                    is_private: false,
                    is_member: true,
                    task_count: 3,
                },
                BoardSummary {
                    id: BoardId(2),
                    name: "Secret plans".into(),
                    description: String::new(),
                    owner_uid: 2,
                    is_locked: true,
                    is_private: true,
                    is_member: true,
                    task_count: 0,
                },
            ],
        };
        app.update(server(ServerMessage::Ok {
            id: hello.id,
            response: Response::Welcome(welcome),
        }));
        app
    }

    pub fn open_board(app: &mut App) {
        let board = board_with_members(1, &[1, 2]);
        let mut tasks = Vec::new();
        for (i, (title, col)) in [
            ("Water the plants", 1),
            ("Buy soap", 1),
            ("Dishes", 2),
            ("Taxes", 4),
        ]
        .iter()
        .enumerate()
        {
            let mut t = task_on(&board, 1);
            t.id = TaskId(10 + i as i64);
            t.short_id = taskologic_core::ids::ShortId::from_index(1000 + i as u64);
            t.title = title.to_string();
            t.column_id = ColumnId(*col);
            t.position = i as i64 * 10;
            tasks.push(t);
        }
        let cmds = app.update(Msg::Term(TermEvent::Key(KeyEvent::new(
            KeyCode::Enter,
            KeyModifiers::NONE,
        ))));
        let Some(Cmd::Send(req)) = cmds.first() else {
            panic!("{cmds:?}")
        };
        app.update(server(ServerMessage::Ok {
            id: req.id,
            response: Response::Board(BoardDetail { board, tasks }),
        }));
    }

    pub fn press(app: &mut App, code: KeyCode) -> Vec<Cmd> {
        app.update(Msg::Term(TermEvent::Key(KeyEvent::new(
            code,
            KeyModifiers::NONE,
        ))))
    }

    pub fn server(m: ServerMessage) -> Msg {
        Msg::Server(Box::new(m))
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::*;
    use super::*;
    use taskologic_core::barcode::{Magic, ScanAction, ScanPayload};
    use taskologic_core::ids::{BoardId, ShortId};

    fn payload() -> String {
        ScanPayload {
            action: ScanAction::StartPause,
            short_id: ShortId::parse("K4M9Q2").unwrap(),
        }
        .encode(Magic::Dots)
    }

    #[test]
    fn the_task_viewer_ticks_checklist_items() {
        let mut app = ready_app(false);
        open_board(&mut app);
        {
            let b = app.board.as_mut().unwrap();
            b.detail.tasks[0].checklist = vec![
                taskologic_core::task::ChecklistItem {
                    text: "soil".into(),
                    done: false,
                },
                taskologic_core::task::ChecklistItem {
                    text: "water".into(),
                    done: false,
                },
            ];
        }
        press(&mut app, KeyCode::Enter);
        assert!(matches!(app.overlay, Some(Overlay::TaskDetail { .. })));
        press(&mut app, KeyCode::Down);
        let cmds = press(&mut app, KeyCode::Char(' '));
        let Cmd::Send(req) = &cmds[0] else {
            panic!("{cmds:?}")
        };
        assert!(matches!(
            req.request,
            Request::SetChecklistItem {
                task_id: TaskId(10),
                index: 1,
                done: true
            }
        ));
        // The answer updates the open viewer in place.
        let mut task = app.board.as_ref().unwrap().detail.tasks[0].clone();
        task.checklist[1].done = true;
        task.version += 1;
        app.update(server(ServerMessage::Ok {
            id: req.id,
            response: Response::Task { task },
        }));
        match &app.overlay {
            Some(Overlay::TaskDetail { task, .. }) => assert!(task.checklist[1].done),
            other => panic!("viewer should stay open, got {:?}", other.is_some()),
        }
        // A key that is not part of the checklist closes it.
        press(&mut app, KeyCode::Esc);
        assert!(app.overlay.is_none());
    }

    #[test]
    fn quit_asks_first() {
        let mut app = ready_app(false);
        assert!(press(&mut app, KeyCode::Char('q')).is_empty());
        assert!(matches!(
            app.overlay,
            Some(Overlay::Confirm {
                action: ConfirmAction::Quit,
                ..
            })
        ));
        assert!(press(&mut app, KeyCode::Char('n')).is_empty());
        assert!(app.overlay.is_none());
        press(&mut app, KeyCode::Char('q'));
        assert_eq!(press(&mut app, KeyCode::Char('y')), vec![Cmd::Quit]);
    }

    #[test]
    fn injected_scan_becomes_a_scan_request() {
        let mut app = ready_app(true);
        let mut cmds = Vec::new();
        for ev in scan::inject(&payload(), false) {
            cmds.extend(app.update(Msg::Term(ev)));
        }
        assert_eq!(cmds.len(), 1, "{cmds:?}");
        match &cmds[0] {
            Cmd::Send(ClientMessage {
                request: Request::Scan { payload: p },
                ..
            }) => assert_eq!(p, &payload()),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn scanner_off_means_keys_are_just_keys() {
        let mut app = ready_app(false);
        open_board(&mut app);
        let mut cmds = Vec::new();
        // No 'S' in the payload: with the scanner off these are ordinary
        // keypresses, and 'S' would simply open the settings screen.
        for ev in scan::inject("..1K", false) {
            cmds.extend(app.update(Msg::Term(ev)));
        }
        assert!(cmds.is_empty());
        assert!(app.overlay.is_none());
        // Plain keys still do their normal job, and the scan key explains itself.
        assert!(press(&mut app, KeyCode::Char('s')).is_empty());
        assert!(
            app.toast
                .as_ref()
                .is_some_and(|t| t.text.contains("off in your settings"))
        );
    }

    #[test]
    fn typing_into_the_search_box_never_scans() {
        let mut app = ready_app(true);
        press(&mut app, KeyCode::Char('/'));
        assert!(app.text_focused());
        let mut cmds = Vec::new();
        for ev in scan::inject(&payload(), false) {
            cmds.extend(app.update(Msg::Term(ev)));
        }
        assert!(cmds.is_empty(), "{cmds:?}");
        assert_eq!(app.search.text(), payload());
        let cmds = press(&mut app, KeyCode::Enter);
        assert!(matches!(
            &cmds[0],
            Cmd::Send(ClientMessage {
                request: Request::Search { .. },
                ..
            })
        ));
        assert!(!app.text_focused());
    }

    #[test]
    fn pick_up_and_shift_arrow_sends_a_move() {
        let mut app = ready_app(false);
        open_board(&mut app);
        assert_eq!(
            app.board.as_ref().unwrap().selected_task().unwrap().title,
            "Water the plants"
        );
        press(&mut app, KeyCode::Char(' '));
        let cmds = app.update(Msg::Term(TermEvent::Key(KeyEvent::new(
            KeyCode::Right,
            KeyModifiers::SHIFT,
        ))));
        match &cmds[0] {
            Cmd::Send(ClientMessage {
                request:
                    Request::MoveTask {
                        task_id, to_column, ..
                    },
                ..
            }) => {
                assert_eq!(*task_id, TaskId(10));
                assert_eq!(*to_column, ColumnId(2));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn blocked_move_offers_an_override() {
        let mut app = ready_app(false);
        open_board(&mut app);
        press(&mut app, KeyCode::Char(' '));
        let cmds = app.update(Msg::Term(TermEvent::Key(KeyEvent::new(
            KeyCode::Right,
            KeyModifiers::SHIFT,
        ))));
        let Cmd::Send(req) = &cmds[0] else { panic!() };
        let err = ErrorBody::new(
            ErrorCode::BlockedByDependencies,
            "1 dependencies are still open",
        )
        .with_detail(&vec![TaskId(11)]);
        app.update(server(ServerMessage::Err {
            id: req.id,
            error: err,
        }));
        assert!(matches!(&app.overlay, Some(Overlay::Confirm { .. })));
        let cmds = press(&mut app, KeyCode::Char('y'));
        assert!(matches!(
            &cmds[0],
            Cmd::Send(ClientMessage {
                request: Request::MoveTask {
                    override_deps: true,
                    ..
                },
                ..
            })
        ));
    }

    #[test]
    fn delete_moves_to_the_archive_and_purge_needs_the_owner() {
        let mut app = ready_app(false);
        open_board(&mut app);
        press(&mut app, KeyCode::Char('d'));
        assert!(matches!(app.overlay, Some(Overlay::Confirm { .. })));
        let cmds = press(&mut app, KeyCode::Char('y'));
        let Cmd::Send(req) = &cmds[0] else { panic!() };
        assert!(matches!(
            req.request,
            Request::DeleteTask {
                task_id: TaskId(10)
            }
        ));
        let mut deleted = app.board.as_ref().unwrap().detail.tasks[0].clone();
        deleted.archived_at = Some(chrono::Utc::now());
        deleted.deleted_at = deleted.archived_at;
        app.update(server(ServerMessage::Ok {
            id: req.id,
            response: Response::Task {
                task: deleted.clone(),
            },
        }));
        assert_eq!(app.board.as_ref().unwrap().detail.tasks.len(), 3);
        assert!(
            app.toast
                .as_ref()
                .is_some_and(|t| t.text.contains("30 days"))
        );

        // Archive panel: Enter restores, D deletes for good (owner here).
        let cmds = press(&mut app, KeyCode::Char('a'));
        let Cmd::Send(req) = &cmds[0] else { panic!() };
        app.update(server(ServerMessage::Ok {
            id: req.id,
            response: Response::Tasks {
                tasks: vec![deleted.clone()],
            },
        }));
        assert!(matches!(app.overlay, Some(Overlay::Archive(_))));
        press(&mut app, KeyCode::Char('D'));
        assert!(matches!(app.overlay, Some(Overlay::Confirm { .. })));
        let cmds = press(&mut app, KeyCode::Char('y'));
        assert!(matches!(
            &cmds[0],
            Cmd::Send(ClientMessage {
                request: Request::PurgeTask {
                    task_id: TaskId(10)
                },
                ..
            })
        ));
    }

    #[test]
    fn dashboard_delete_board_respects_lock_and_admin_visibility() {
        let mut app = ready_app(false);
        // Second board is locked and owned by someone else; alice is admin.
        press(&mut app, KeyCode::Down);
        assert!(press(&mut app, KeyCode::Char('D')).is_empty());
        assert!(
            app.toast
                .as_ref()
                .is_some_and(|t| t.text.contains("locked"))
        );
        press(&mut app, KeyCode::Up);
        press(&mut app, KeyCode::Char('D'));
        assert!(matches!(app.overlay, Some(Overlay::Confirm { .. })));
        let cmds = press(&mut app, KeyCode::Char('y'));
        assert!(matches!(
            &cmds[0],
            Cmd::Send(ClientMessage {
                request: Request::DeleteBoard {
                    board_id: BoardId(1)
                },
                ..
            })
        ));
        // A private board the admin is not on cannot be opened.
        app.boards[1].is_member = false;
        app.board_sel = 1;
        assert!(press(&mut app, KeyCode::Enter).is_empty());
        assert!(
            app.toast
                .as_ref()
                .is_some_and(|t| t.text.contains("not a member"))
        );
    }

    #[test]
    fn n_opens_the_task_form_and_saving_creates_the_task() {
        let mut app = ready_app(false);
        open_board(&mut app);
        let cmds = press(&mut app, KeyCode::Char('n'));
        assert!(matches!(app.overlay, Some(Overlay::TaskForm(_))));
        assert!(matches!(
            &cmds[0],
            Cmd::Send(ClientMessage {
                request: Request::ListMembers { .. },
                ..
            })
        ));
        assert!(
            app.text_focused(),
            "scan detection is off while the form is open"
        );
        for c in "Mop".chars() {
            press(&mut app, KeyCode::Char(c));
        }
        let cmds = press(&mut app, KeyCode::F(2));
        match &cmds[0] {
            Cmd::Send(ClientMessage {
                request:
                    Request::CreateTask {
                        column_id, draft, ..
                    },
                id,
            }) => {
                assert_eq!(*column_id, Some(ColumnId(1)));
                assert_eq!(draft.title, "Mop");
                let board = taskologic_core::board::test_support::board_with_members(1, &[1]);
                let mut task = taskologic_core::task::test_support::task_on(&board, 1);
                task.id = TaskId(99);
                task.title = "Mop".into();
                app.update(server(ServerMessage::Ok {
                    id: *id,
                    response: Response::Task { task },
                }));
            }
            other => panic!("{other:?}"),
        }
        assert!(app.overlay.is_none());
        assert_eq!(app.board.as_ref().unwrap().detail.tasks.len(), 5);
    }

    #[test]
    fn edit_conflict_offers_reload_or_overwrite() {
        let mut app = ready_app(false);
        open_board(&mut app);
        press(&mut app, KeyCode::Char('e'));
        assert!(
            matches!(&app.overlay, Some(Overlay::TaskForm(f)) if f.mode == FormMode::Edit { task: TaskId(10), version: 1 })
        );
        let cmds = press(&mut app, KeyCode::F(2));
        let Cmd::Send(req) = &cmds[0] else { panic!() };
        assert!(matches!(
            req.request,
            Request::UpdateTask { version: 1, .. }
        ));
        let mut current = app.board.as_ref().unwrap().detail.tasks[0].clone();
        current.version = 4;
        current.title = "Water the plants twice".into();
        let err = ErrorBody::new(ErrorCode::Conflict, "task changed since you loaded it")
            .with_detail(&current);
        app.update(server(ServerMessage::Err {
            id: req.id,
            error: err,
        }));
        assert!(matches!(app.overlay, Some(Overlay::Conflict { .. })));
        let cmds = press(&mut app, KeyCode::Char('o'));
        let Cmd::Send(req) = &cmds[0] else { panic!() };
        assert!(
            matches!(req.request, Request::UpdateTask { version: 4, .. }),
            "overwrite uses the current version"
        );
        assert!(matches!(app.overlay, Some(Overlay::TaskForm(_))));
    }

    #[test]
    fn settings_save_updates_prefs_locally_and_sends_them() {
        let mut app = ready_app(false);
        press(&mut app, KeyCode::Char('S'));
        assert!(matches!(app.overlay, Some(Overlay::Settings(_))));
        // First checkbox is "board tabs", on by default. Space flips it.
        press(&mut app, KeyCode::Char(' '));
        let cmds = press(&mut app, KeyCode::F(2));
        assert!(
            matches!(&cmds[0], Cmd::Send(ClientMessage { request: Request::UpdatePrefs { prefs }, .. }) if !prefs.ui.show_board_tabs)
        );
        assert!(!app.user.as_ref().unwrap().prefs.ui.show_board_tabs);
    }

    #[test]
    fn new_board_form_creates_with_defaults() {
        let mut app = ready_app(false);
        let cmds = press(&mut app, KeyCode::Char('N'));
        assert!(matches!(
            &cmds[0],
            Cmd::Send(ClientMessage {
                request: Request::ListUsers,
                ..
            })
        ));
        for c in "Garage".chars() {
            press(&mut app, KeyCode::Char(c));
        }
        let cmds = press(&mut app, KeyCode::F(2));
        match &cmds[0] {
            Cmd::Send(ClientMessage {
                request: Request::CreateBoard(req),
                ..
            }) => {
                assert_eq!(req.name, "Garage");
                assert_eq!(req.columns.len(), 4);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn column_removal_asks_for_a_destination_and_role_replacement() {
        let mut app = ready_app(false);
        open_board(&mut app);
        press(&mut app, KeyCode::Char('c'));
        assert!(matches!(app.overlay, Some(Overlay::Columns(_))));
        // Todo (column 1) has tasks and no role: one question, then confirm.
        press(&mut app, KeyCode::Char('d'));
        assert!(
            matches!(&app.overlay, Some(Overlay::PickColumn { options, .. }) if options.len() == 3)
        );
        press(&mut app, KeyCode::Enter);
        assert!(matches!(&app.overlay, Some(Overlay::Confirm { .. })));
        let cmds = press(&mut app, KeyCode::Char('y'));
        match &cmds[0] {
            Cmd::Send(ClientMessage {
                request:
                    Request::RemoveColumn {
                        column_id,
                        move_tasks_to,
                        replacements,
                    },
                ..
            }) => {
                assert_eq!(*column_id, ColumnId(1));
                assert_eq!(*move_tasks_to, Some(ColumnId(2)));
                assert!(replacements.is_empty());
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn removing_a_member_with_tasks_offers_to_unassign() {
        let mut app = ready_app(false);
        open_board(&mut app);
        press(&mut app, KeyCode::Char('m'));
        assert!(matches!(app.overlay, Some(Overlay::Members(_))));
        if let Some(Overlay::Members(panel)) = &mut app.overlay {
            // Bob first: list navigation needs a render to know its row count.
            panel.set_members(vec![
                taskologic_core::user::UserSummary {
                    uid: 2,
                    username: "bob".into(),
                    is_admin: false,
                },
                taskologic_core::user::UserSummary {
                    uid: 1,
                    username: "alice".into(),
                    is_admin: true,
                },
            ]);
        }
        let cmds = press(&mut app, KeyCode::Char('x'));
        let Cmd::Send(req) = &cmds[0] else {
            panic!("{cmds:?}")
        };
        assert!(matches!(
            req.request,
            Request::RemoveMember {
                uid: 2,
                unassign: false,
                ..
            }
        ));
        let err = ErrorBody::new(ErrorCode::MemberHasTasks, "that member still has 1 tasks")
            .with_detail(&vec![TaskId(10)]);
        app.update(server(ServerMessage::Err {
            id: req.id,
            error: err,
        }));
        assert!(matches!(&app.overlay, Some(Overlay::Confirm { .. })));
        let cmds = press(&mut app, KeyCode::Char('y'));
        assert!(matches!(
            &cmds[0],
            Cmd::Send(ClientMessage {
                request: Request::RemoveMember {
                    uid: 2,
                    unassign: true,
                    ..
                },
                ..
            })
        ));
    }

    #[test]
    fn events_update_the_open_board() {
        let mut app = ready_app(false);
        open_board(&mut app);
        let mut moved = app.board.as_ref().unwrap().detail.tasks[0].clone();
        moved.column_id = ColumnId(4);
        app.update(server(ServerMessage::Event {
            event: Event::TaskChanged {
                task: moved,
                change: TaskChange::Moved {
                    from: ColumnId(1),
                    to: ColumnId(4),
                },
                actor: Some(2),
            },
        }));
        let b = app.board.as_ref().unwrap();
        assert_eq!(b.tasks_in(0).len(), 1);
        assert_eq!(b.tasks_in(3).len(), 2);
        let gone = b.detail.tasks[1].clone();
        app.update(server(ServerMessage::Event {
            event: Event::TaskChanged {
                task: gone,
                change: TaskChange::Deleted,
                actor: None,
            },
        }));
        assert_eq!(app.board.as_ref().unwrap().detail.tasks.len(), 3);
        app.update(server(ServerMessage::Event {
            event: Event::BoardChanged {
                board_id: BoardId(1),
                change: BoardChange::AccessRevoked,
            },
        }));
        assert!(app.board.is_none());
    }

    #[test]
    fn print_job_events_turn_into_print_commands_and_acks() {
        let mut app = ready_app(false);
        open_board(&mut app);
        let board = taskologic_core::board::test_support::board_with_members(1, &[1]);
        let task = taskologic_core::task::test_support::task_on(&board, 1);
        let job = taskologic_core::print::build_reminder_job(
            &task,
            &board,
            &user(false),
            chrono::DateTime::from_timestamp(0, 0).unwrap(),
        );
        let cmds = app.update(server(ServerMessage::Event {
            event: Event::PrintJob {
                job_id: PrintJobId(5),
                job: job.clone(),
            },
        }));
        assert_eq!(
            cmds,
            vec![Cmd::Print {
                job_id: PrintJobId(5),
                job
            }]
        );
        let cmds = app.update(Msg::Printed {
            job_id: PrintJobId(5),
            error: Some("out of paper".into()),
        });
        assert!(matches!(
            &cmds[0],
            Cmd::Send(ClientMessage {
                request: Request::AckPrintJob {
                    job_id: PrintJobId(5),
                    error: Some(_)
                },
                ..
            })
        ));
        assert!(
            app.toast
                .as_ref()
                .is_some_and(|t| t.severity == Severity::Error)
        );
    }
}
