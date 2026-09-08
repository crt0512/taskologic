//! The socket lives on its own tasks. The UI loop never touches it, it
//! sends `ClientMessage`s down a channel and gets `Msg`s back.

use taskologic_proto::{ClientMessage, ServerMessage, codec};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::sync::mpsc;

use crate::app::Msg;

pub struct Net {
    pub tx: mpsc::UnboundedSender<ClientMessage>,
}

pub fn spawn(stream: UnixStream, to_app: mpsc::UnboundedSender<Msg>) -> Net {
    let (r, mut w) = stream.into_split();
    let (tx, mut rx) = mpsc::unbounded_channel::<ClientMessage>();

    tokio::spawn(async move {
        let mut lines = BufReader::new(r).lines();
        loop {
            let msg = match lines.next_line().await {
                Ok(Some(line)) => match codec::decode::<ServerMessage>(&line) {
                    Ok(m) => Msg::Server(Box::new(m)),
                    Err(e) => Msg::Disconnected(format!("unreadable message from the daemon: {e}")),
                },
                Ok(None) => Msg::Disconnected("the daemon closed the connection".into()),
                Err(e) => Msg::Disconnected(e.to_string()),
            };
            let fatal = matches!(msg, Msg::Disconnected(_));
            if to_app.send(msg).is_err() || fatal {
                break;
            }
        }
    });

    tokio::spawn(async move {
        while let Some(m) = rx.recv().await {
            let Ok(line) = codec::encode(&m) else {
                continue;
            };
            if w.write_all(line.as_bytes()).await.is_err() {
                break;
            }
        }
    });

    Net { tx }
}
