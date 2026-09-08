use taskologic_core::board::BoardError;
use taskologic_core::ids::TaskId;
use taskologic_core::permission::Denial;
use taskologic_core::task::{Task, TaskError};
use taskologic_core::transition::MoveError;
use taskologic_proto::{ErrorBody, ErrorCode};

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("{0}")]
    NotFound(String),
    #[error("{0}")]
    Denied(Denial),
    #[error("{0}")]
    BadRequest(String),
    #[error("task changed since you loaded it")]
    Conflict { current: Box<Task> },
    #[error("{} dependencies are still open", open.len())]
    Blocked { open: Vec<TaskId> },
    #[error("that member still has {} tasks assigned on this board", .0.len())]
    MemberHasTasks(Vec<TaskId>),
    #[error("{0}")]
    Protocol(String),
    #[error("database error: {0}")]
    Db(#[from] rusqlite::Error),
    #[error("{0}")]
    Internal(#[from] anyhow::Error),
}

impl AppError {
    pub fn bad(msg: impl Into<String>) -> Self {
        AppError::BadRequest(msg.into())
    }
}

impl From<Denial> for AppError {
    fn from(d: Denial) -> Self {
        match d {
            // Answer as if the board does not exist. Do not leak that it does.
            Denial::BoardNotVisible => AppError::NotFound("board not found".into()),
            other => AppError::Denied(other),
        }
    }
}

impl From<BoardError> for AppError {
    fn from(e: BoardError) -> Self {
        AppError::BadRequest(e.to_string())
    }
}

impl From<TaskError> for AppError {
    fn from(e: TaskError) -> Self {
        AppError::BadRequest(e.to_string())
    }
}

impl From<MoveError> for AppError {
    fn from(e: MoveError) -> Self {
        match e {
            MoveError::BlockedByDependencies { open } => AppError::Blocked { open },
            other => AppError::BadRequest(other.to_string()),
        }
    }
}

impl From<serde_json::Error> for AppError {
    fn from(e: serde_json::Error) -> Self {
        AppError::Internal(e.into())
    }
}

impl From<AppError> for ErrorBody {
    fn from(e: AppError) -> Self {
        match e {
            AppError::NotFound(r) => ErrorBody::new(ErrorCode::NotFound, r),
            AppError::Denied(d) => ErrorBody::new(ErrorCode::PermissionDenied, d.reason()),
            AppError::BadRequest(r) => ErrorBody::new(ErrorCode::BadRequest, r),
            AppError::Conflict { current } => {
                ErrorBody::new(ErrorCode::Conflict, "task changed since you loaded it")
                    .with_detail(&*current)
            }
            AppError::Blocked { open } => ErrorBody::new(
                ErrorCode::BlockedByDependencies,
                format!("{} dependencies are still open", open.len()),
            )
            .with_detail(&open),
            AppError::MemberHasTasks(tasks) => ErrorBody::new(
                ErrorCode::MemberHasTasks,
                format!(
                    "that member still has {} tasks assigned on this board",
                    tasks.len()
                ),
            )
            .with_detail(&tasks),
            AppError::Protocol(r) => ErrorBody::new(ErrorCode::ProtocolMismatch, r),
            AppError::Db(e) => {
                tracing::error!(error = %e, "database error");
                ErrorBody::new(ErrorCode::Internal, "database error, see the daemon log")
            }
            AppError::Internal(e) => {
                tracing::error!(error = %e, "internal error");
                ErrorBody::new(ErrorCode::Internal, "internal error, see the daemon log")
            }
        }
    }
}
