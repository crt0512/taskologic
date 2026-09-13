use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use taskologic_core::barcode::ScanAction;
use taskologic_core::board::{Board, ColumnRole};
use taskologic_core::ids::{BoardId, ColumnId, PrintJobId, TaskId, TemplateId, Uid};
use taskologic_core::prefs::{CardFields, UserPrefs};
use taskologic_core::print::PrintJob;
use taskologic_core::task::{Task, TaskDraft};
use taskologic_core::template::{Template, TemplateOptions};
use taskologic_core::user::{User, UserSummary};

pub type RequestId = u64;

/// Client to daemon envelope.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ClientMessage {
    pub id: RequestId,
    pub request: Request,
}

/// Daemon to client. Responses carry the request id, events do not.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMessage {
    Ok { id: RequestId, response: Response },
    Err { id: RequestId, error: ErrorBody },
    Event { event: Event },
}

/// First message on every connection. Auth is SO_PEERCRED, so there are no
/// credentials in here, only what the client is and what it can do.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Hello {
    pub protocol_version: u32,
    pub client_version: String,
    /// Whether this client has a printer configured. Print jobs only go to
    /// clients that can render them.
    pub has_printer: bool,
    /// Higher wins when the same user has several clients connected.
    pub print_priority: i32,
    /// Free form, for future negotiation. Unknown entries are ignored.
    #[serde(default)]
    pub capabilities: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Welcome {
    pub server_version: String,
    pub protocol_version: u32,
    pub user: User,
    pub boards: Vec<BoardSummary>,
}

/// Enough for the dashboard and the board switcher.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BoardSummary {
    pub id: BoardId,
    pub name: String,
    pub description: String,
    pub owner_uid: Uid,
    pub is_locked: bool,
    pub is_private: bool,
    /// False only for admins looking at a private board they are not on.
    /// They can unlock or delete it, nothing else.
    pub is_member: bool,
    pub task_count: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BoardDetail {
    pub board: Board,
    /// Live tasks only, archived ones come from `ListArchived`.
    pub tasks: Vec<Task>,
}

/// Columns are named up front and the designated ones referenced by index
/// into that list, since they have no ids yet.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateBoard {
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub columns: Vec<String>,
    pub started_col: usize,
    pub paused_col: usize,
    pub finished_col: usize,
    pub archive_after_secs: i64,
    /// How long deleted tasks stay in the archive before they are purged.
    #[serde(default = "default_purge_secs")]
    pub purge_deleted_after_secs: i64,
    #[serde(default)]
    pub card_fields: CardFields,
    pub is_private: bool,
    pub is_locked: bool,
    #[serde(default)]
    pub members: Vec<Uid>,
}

fn default_purge_secs() -> i64 {
    taskologic_core::board::DEFAULT_PURGE_DELETED_AFTER_SECS
}

/// Partial update, None leaves a field alone.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateBoard {
    pub board_id: BoardId,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub is_locked: Option<bool>,
    #[serde(default)]
    pub is_private: Option<bool>,
    #[serde(default)]
    pub archive_after_secs: Option<i64>,
    #[serde(default)]
    pub purge_deleted_after_secs: Option<i64>,
    #[serde(default)]
    pub card_fields: Option<CardFields>,
    #[serde(default)]
    pub roles: Vec<(ColumnRole, ColumnId)>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Request {
    Hello(Hello),
    ListBoards,
    GetBoard {
        board_id: BoardId,
    },
    CreateBoard(CreateBoard),
    UpdateBoard(UpdateBoard),
    DeleteBoard {
        board_id: BoardId,
    },

    AddColumn {
        board_id: BoardId,
        name: String,
    },
    RenameColumn {
        column_id: ColumnId,
        name: String,
    },
    ReorderColumns {
        board_id: BoardId,
        order: Vec<ColumnId>,
    },
    /// Show a column by due date instead of its manual order.
    SetColumnSort {
        column_id: ColumnId,
        sort_by_due: bool,
    },
    RemoveColumn {
        column_id: ColumnId,
        move_tasks_to: Option<ColumnId>,
        #[serde(default)]
        replacements: Vec<(ColumnRole, ColumnId)>,
    },

    ListMembers {
        board_id: BoardId,
    },
    AddMember {
        board_id: BoardId,
        uid: Uid,
    },
    /// `unassign` is the answer to "they still have tasks, what now". False
    /// with open assignments is refused with the list of tasks.
    RemoveMember {
        board_id: BoardId,
        uid: Uid,
        unassign: bool,
    },
    /// Everyone in the taskologic group, for public board assignee pickers.
    ListUsers,
    /// Admins only. The first user ever to log in is an admin, this is how
    /// they hand it on.
    SetAdmin {
        uid: Uid,
        is_admin: bool,
    },

    /// `column_id` None means the board's first column.
    CreateTask {
        board_id: BoardId,
        column_id: Option<ColumnId>,
        draft: TaskDraft,
    },
    /// `version` must match the current row or the save is rejected.
    UpdateTask {
        task_id: TaskId,
        version: u64,
        draft: TaskDraft,
    },
    MoveTask {
        task_id: TaskId,
        to_column: ColumnId,
        /// None appends at the bottom.
        position: Option<i64>,
        #[serde(default)]
        override_deps: bool,
    },
    /// Moves the task to the archive marked as deleted. Anyone who can see
    /// the board can restore it until the retention period purges it.
    DeleteTask {
        task_id: TaskId,
    },
    /// Tick or untick one checklist item. No version check: it flips one
    /// bool, and whoever ticked last is right.
    SetChecklistItem {
        task_id: TaskId,
        index: usize,
        done: bool,
    },
    RestoreTask {
        task_id: TaskId,
    },
    /// Removes an archived task for good. Owner and admins only.
    PurgeTask {
        task_id: TaskId,
    },
    ListArchived {
        board_id: BoardId,
    },

    ListTemplates {
        board_id: BoardId,
    },
    /// Any member can save a template. The draft's due date and dependencies
    /// are still ignored: a template carries a due date *prefill rule* and
    /// dependencies on other *templates* instead, both in `options`.
    CreateTemplate {
        board_id: BoardId,
        name: String,
        draft: TaskDraft,
        #[serde(default)]
        options: TemplateOptions,
    },
    /// Template creator, board owner or admin.
    UpdateTemplate {
        template_id: TemplateId,
        name: String,
        draft: TaskDraft,
        #[serde(default)]
        options: TemplateOptions,
    },
    /// Stamp a task out of a template. The template's dependency templates
    /// are stamped out first, in dependency order, and the new task depends
    /// on the tasks they produced. The draft is what the user had in the
    /// form, so an edit before saving still counts.
    CreateFromTemplate {
        template_id: TemplateId,
        column_id: Option<ColumnId>,
        draft: TaskDraft,
    },
    /// Template creator, board owner or admin.
    DeleteTemplate {
        template_id: TemplateId,
    },

    /// Active repetitions on one board with their next fire time.
    ListRepeats {
        board_id: BoardId,
    },
    /// Turn a repetition off. Any board member; it is working the board the
    /// same way moving a task is.
    StopRepeat {
        task_id: TaskId,
    },

    /// Raw barcode contents as the scanner typed them.
    Scan {
        payload: String,
    },
    Search {
        query: String,
        #[serde(default)]
        include_archived: bool,
    },
    PrintTask {
        task_id: TaskId,
    },
    /// Report the outcome of rendering and spooling a `PrintJob` event.
    AckPrintJob {
        job_id: PrintJobId,
        error: Option<String>,
    },
    UpdatePrefs {
        prefs: UserPrefs,
    },
    /// IANA zone name. Timestamps are stored in UTC and rendered in this.
    SetTimezone {
        timezone: String,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Response {
    Welcome(Welcome),
    Boards {
        boards: Vec<BoardSummary>,
    },
    Board(BoardDetail),
    Task {
        task: Task,
    },
    Tasks {
        tasks: Vec<Task>,
    },
    Members {
        members: Vec<UserSummary>,
    },
    Users {
        users: Vec<UserSummary>,
    },
    SearchResults {
        hits: Vec<SearchHit>,
    },
    Scan(ScanOutcome),
    Template {
        template: Template,
    },
    Templates {
        templates: Vec<Template>,
    },
    Repeats {
        entries: Vec<RepeatEntry>,
    },
    /// Plain acknowledgement.
    Done,
}

/// One row of the repeating tasks list.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RepeatEntry {
    pub task: Task,
    /// None when the rule is exhausted, a fixed date that has passed.
    pub next_fire_at: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchHit {
    pub task_id: TaskId,
    pub board_id: BoardId,
    pub board_name: String,
    pub column_name: String,
    pub title: String,
    pub archived: bool,
}

/// Every scan gets one of these. The client gives distinct feedback per
/// variant, a scan that silently does nothing is worse than useless.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum ScanOutcome {
    Applied {
        task: Box<Task>,
        action: ScanAction,
        moved_to: ColumnId,
        column_name: String,
    },
    UnknownTask,
    /// Includes tasks on private boards the scanner cannot see.
    NoPermission,
    /// Finished already, archived, and so on.
    Refused {
        reason: String,
    },
    BadScan {
        reason: String,
    },
    /// Finishing by barcode hit open dependencies. The client can offer the
    /// override through a normal `MoveTask`.
    BlockedByDependencies {
        task_id: TaskId,
        open: Vec<TaskId>,
    },
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    BadRequest,
    /// Also the answer for private boards the caller cannot see.
    NotFound,
    PermissionDenied,
    /// Stale task version. `detail` carries the current task.
    Conflict,
    /// `detail` carries the open dependency ids.
    BlockedByDependencies,
    /// Removing a member who still has tasks without saying what to do.
    /// `detail` carries the task ids.
    MemberHasTasks,
    ProtocolMismatch,
    NotTaskologicUser,
    Internal,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ErrorBody {
    pub code: ErrorCode,
    /// Always says why, never just "no".
    pub reason: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<serde_json::Value>,
}

impl ErrorBody {
    pub fn new(code: ErrorCode, reason: impl Into<String>) -> Self {
        Self {
            code,
            reason: reason.into(),
            detail: None,
        }
    }

    pub fn with_detail<T: Serialize>(mut self, detail: &T) -> Self {
        self.detail = serde_json::to_value(detail).ok();
        self
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Info,
    Success,
    Warning,
    Error,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "change", rename_all = "snake_case")]
pub enum BoardChange {
    Created,
    Settings,
    Columns,
    Members,
    Deleted,
    /// You are no longer a member. Drop it from the dashboard.
    AccessRevoked,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "change", rename_all = "snake_case")]
pub enum TaskChange {
    Created,
    Edited,
    Moved {
        from: ColumnId,
        to: ColumnId,
    },
    Reordered,
    /// In the archive now, marked as deleted.
    Deleted,
    Archived,
    Restored,
    /// Gone for good.
    Purged,
}

/// Unsolicited daemon to client messages. Clients only receive events for
/// boards they can see, the daemon filters, never the client.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    BoardChanged {
        board_id: BoardId,
        change: BoardChange,
    },
    /// `task` is the state after the change, or the last state for a delete.
    TaskChanged {
        task: Task,
        change: TaskChange,
        actor: Option<Uid>,
    },
    /// Render this on the local printer and answer with `AckPrintJob`.
    PrintJob {
        job_id: PrintJobId,
        job: PrintJob,
    },
    Notice {
        text: String,
        severity: Severity,
    },
    /// Prefs changed from another client of the same user.
    PrefsChanged {
        prefs: UserPrefs,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codec::{decode, encode};
    use taskologic_core::board::test_support::board_with_members;
    use taskologic_core::task::test_support::task_on;

    #[test]
    fn hello_is_internally_tagged() {
        let msg = ClientMessage {
            id: 1,
            request: Request::Hello(Hello {
                protocol_version: 1,
                client_version: "0.1.0".into(),
                has_printer: false,
                print_priority: 0,
                capabilities: vec![],
            }),
        };
        let line = encode(&msg).unwrap();
        assert!(line.ends_with('\n'));
        assert!(line.contains("\"type\":\"hello\""), "{line}");
        assert_eq!(decode::<ClientMessage>(&line).unwrap(), msg);
    }

    #[test]
    fn unit_variants_are_just_a_type() {
        let line = encode(&ServerMessage::Ok {
            id: 7,
            response: Response::Done,
        })
        .unwrap();
        assert_eq!(
            line.trim(),
            r#"{"type":"ok","id":7,"response":{"type":"done"}}"#
        );
        let line = encode(&ClientMessage {
            id: 2,
            request: Request::ListBoards,
        })
        .unwrap();
        assert_eq!(line.trim(), r#"{"id":2,"request":{"type":"list_boards"}}"#);
    }

    #[test]
    fn events_and_errors_round_trip() {
        let board = board_with_members(1, &[1]);
        let task = task_on(&board, 1);
        let ev = ServerMessage::Event {
            event: Event::TaskChanged {
                task: task.clone(),
                change: TaskChange::Moved {
                    from: ColumnId(1),
                    to: ColumnId(2),
                },
                actor: Some(1),
            },
        };
        let line = encode(&ev).unwrap();
        assert_eq!(decode::<ServerMessage>(&line).unwrap(), ev);

        let err = ServerMessage::Err {
            id: 3,
            error: ErrorBody::new(ErrorCode::Conflict, "task changed since you opened it")
                .with_detail(&task),
        };
        let line = encode(&err).unwrap();
        let back: ServerMessage = decode(&line).unwrap();
        assert_eq!(back, err);
        let plain = encode(&ErrorBody::new(ErrorCode::NotFound, "board not found")).unwrap();
        assert!(
            !plain.contains("detail"),
            "absent detail is omitted: {plain}"
        );
    }

    #[test]
    fn default_fields_can_be_omitted_on_the_wire() {
        let line =
            r#"{"id":9,"request":{"type":"move_task","task_id":5,"to_column":2,"position":null}}"#;
        let msg: ClientMessage = decode(line).unwrap();
        assert_eq!(
            msg.request,
            Request::MoveTask {
                task_id: TaskId(5),
                to_column: ColumnId(2),
                position: None,
                override_deps: false
            }
        );
        let line = r#"{"id":9,"request":{"type":"search","query":"plants"}}"#;
        let msg: ClientMessage = decode(line).unwrap();
        assert_eq!(
            msg.request,
            Request::Search {
                query: "plants".into(),
                include_archived: false
            }
        );
        let line = r#"{"id":9,"request":{"type":"create_template","board_id":1,"name":"Weekly","draft":{"title":"Water plants"}}}"#;
        let msg: ClientMessage = decode(line).unwrap();
        assert_eq!(
            msg.request,
            Request::CreateTemplate {
                board_id: BoardId(1),
                name: "Weekly".into(),
                draft: TaskDraft {
                    title: "Water plants".into(),
                    ..Default::default()
                },
                options: TemplateOptions::default(),
            }
        );
    }

    #[test]
    fn stamping_a_task_out_of_a_template_round_trips() {
        let msg = ClientMessage {
            id: 4,
            request: Request::CreateFromTemplate {
                template_id: TemplateId(2),
                column_id: Some(ColumnId(3)),
                draft: TaskDraft {
                    title: "Water plants".into(),
                    reminder_minutes: Some(90),
                    ..Default::default()
                },
            },
        };
        let line = encode(&msg).unwrap();
        assert!(line.contains(r#""type":"create_from_template""#), "{line}");
        assert_eq!(decode::<ClientMessage>(&line).unwrap(), msg);
    }

    #[test]
    fn oversized_lines_are_refused() {
        let huge = "x".repeat(crate::codec::MAX_LINE_BYTES + 1);
        assert!(matches!(
            decode::<ClientMessage>(&huge),
            Err(crate::codec::CodecError::TooLong(_))
        ));
    }
}
