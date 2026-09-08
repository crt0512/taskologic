//! Unix socket listener and per connection loop.

use std::sync::Arc;

use chrono::Utc;
use taskologic_core::ids::Uid;
use taskologic_proto::{
    ClientMessage, ErrorBody, ErrorCode, PROTOCOL_VERSION, Request, Response, ServerMessage,
    Welcome, codec,
};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{broadcast, mpsc};

use crate::auth;
use crate::bus::ClientEntry;
use crate::db::repo;
use crate::error::AppError;
use crate::handlers::{Session, handle};
use crate::state::AppState;

pub async fn run(state: Arc<AppState>, listener: UnixListener) {
    loop {
        match listener.accept().await {
            Ok((stream, _)) => {
                let state = state.clone();
                tokio::spawn(async move {
                    if let Err(e) = connection(state, stream).await {
                        tracing::debug!(error = %e, "connection ended");
                    }
                });
            }
            Err(e) => {
                tracing::warn!(error = %e, "accept failed");
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
        }
    }
}

async fn send(w: &mut tokio::net::unix::OwnedWriteHalf, msg: &ServerMessage) -> anyhow::Result<()> {
    let line = codec::encode(msg)?;
    w.write_all(line.as_bytes()).await?;
    Ok(())
}

async fn connection(state: Arc<AppState>, stream: UnixStream) -> anyhow::Result<()> {
    let cred = stream.peer_cred()?;
    let uid = cred.uid();
    let (r, mut w) = stream.into_split();
    let mut lines = BufReader::new(r).lines();

    let identity = match auth::identify(uid, &state.cfg.group) {
        Ok(id) => id,
        Err(e) => {
            tracing::info!(uid, error = %e, "refused connection");
            let msg = ServerMessage::Err {
                id: 0,
                error: ErrorBody::new(ErrorCode::NotTaskologicUser, e.to_string()),
            };
            let _ = send(&mut w, &msg).await;
            return Ok(());
        }
    };

    // First message must be Hello.
    let Some(first) = lines.next_line().await? else {
        return Ok(());
    };
    let first: ClientMessage = match codec::decode(&first) {
        Ok(m) => m,
        Err(e) => {
            let msg = ServerMessage::Err {
                id: 0,
                error: ErrorBody::new(ErrorCode::BadRequest, e.to_string()),
            };
            let _ = send(&mut w, &msg).await;
            return Ok(());
        }
    };
    let Request::Hello(hello) = first.request else {
        let msg = ServerMessage::Err {
            id: first.id,
            error: ErrorBody::new(ErrorCode::ProtocolMismatch, "first message must be hello"),
        };
        let _ = send(&mut w, &msg).await;
        return Ok(());
    };
    if hello.protocol_version != PROTOCOL_VERSION {
        let msg = ServerMessage::Err {
            id: first.id,
            error: ErrorBody::new(
                ErrorCode::ProtocolMismatch,
                format!(
                    "daemon speaks protocol {PROTOCOL_VERSION}, client sent {}",
                    hello.protocol_version
                ),
            ),
        };
        let _ = send(&mut w, &msg).await;
        return Ok(());
    }

    let force_admin = state.cfg.always_admin_uids.contains(&uid);
    let host_tz = state.cfg.host_tz();
    let user = state
        .db
        .with(|c| {
            repo::upsert_user_on_login(c, uid, &identity.username, host_tz, force_admin, Utc::now())
        })
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    let boards = state
        .db
        .with(|c| repo::list_boards_visible(c, uid, user.is_admin))
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    let session = Session {
        uid,
        is_admin: user.is_admin,
    };
    tracing::info!(uid, username = %user.username, client = %hello.client_version, "client connected");

    let (direct_tx, mut direct_rx) = mpsc::unbounded_channel::<ServerMessage>();
    let conn_id = state.registry.register(ClientEntry {
        uid,
        has_printer: hello.has_printer,
        print_priority: hello.print_priority,
        tx: direct_tx,
    });
    let mut events = state.bus.subscribe();

    let welcome = Response::Welcome(Welcome {
        server_version: env!("CARGO_PKG_VERSION").into(),
        protocol_version: PROTOCOL_VERSION,
        user,
        boards,
    });
    send(
        &mut w,
        &ServerMessage::Ok {
            id: first.id,
            response: welcome,
        },
    )
    .await?;
    if hello.has_printer
        && let Err(e) = state.dispatch_print_jobs(uid)
    {
        tracing::warn!(uid, error = %e, "draining print queue on connect");
    }

    let result = serve(
        &state,
        &session,
        &mut lines,
        &mut w,
        &mut direct_rx,
        &mut events,
    )
    .await;
    state.registry.unregister(conn_id);
    tracing::info!(uid, "client disconnected");
    result
}

async fn serve(
    state: &Arc<AppState>,
    session: &Session,
    lines: &mut tokio::io::Lines<BufReader<tokio::net::unix::OwnedReadHalf>>,
    w: &mut tokio::net::unix::OwnedWriteHalf,
    direct: &mut mpsc::UnboundedReceiver<ServerMessage>,
    events: &mut broadcast::Receiver<Arc<crate::bus::Outbound>>,
) -> anyhow::Result<()> {
    loop {
        tokio::select! {
            line = lines.next_line() => {
                let Some(line) = line? else { return Ok(()) };
                if line.trim().is_empty() {
                    continue;
                }
                let reply = match codec::decode::<ClientMessage>(&line) {
                    Ok(msg) => {
                        let id = msg.id;
                        match handle(state, session, msg.request) {
                            Ok(response) => ServerMessage::Ok { id, response },
                            Err(e) => ServerMessage::Err { id, error: error_body(e) },
                        }
                    }
                    Err(e) => ServerMessage::Err { id: 0, error: ErrorBody::new(ErrorCode::BadRequest, e.to_string()) },
                };
                send(w, &reply).await?;
            }
            Some(msg) = direct.recv() => {
                send(w, &msg).await?;
            }
            ev = events.recv() => {
                match ev {
                    Ok(out) => {
                        if out.audience.includes(session.uid, session.is_admin) {
                            send(w, &out.msg).await?;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!(uid = session.uid, skipped = n, "client fell behind on events");
                        send(w, &ServerMessage::Event { event: taskologic_proto::Event::Notice {
                            text: "missed some updates, reload the board".into(),
                            severity: taskologic_proto::Severity::Warning,
                        }}).await?;
                    }
                    Err(broadcast::error::RecvError::Closed) => return Ok(()),
                }
            }
        }
    }
}

fn error_body(e: AppError) -> ErrorBody {
    e.into()
}

/// Bind the socket, replacing a stale file, group readable and writable so
/// only the taskologic group can connect. The group change is best effort, it
/// only works when the daemon runs as root or as a group member.
pub fn bind(path: &std::path::Path, group: &str) -> anyhow::Result<UnixListener> {
    use std::os::unix::fs::PermissionsExt;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    if path.exists() {
        std::fs::remove_file(path)?;
    }
    let listener = UnixListener::bind(path)?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o660))?;
    match nix::unistd::Group::from_name(group) {
        Ok(Some(g)) => {
            if let Err(e) = nix::unistd::chown(path, None, Some(g.gid)) {
                tracing::warn!(error = %e, group, "could not chgrp the socket");
            }
        }
        _ => tracing::warn!(
            group,
            "group does not exist, socket keeps the daemon's group"
        ),
    }
    Ok(listener)
}

#[allow(dead_code)]
fn _uid_is_copy(_: Uid) {}
