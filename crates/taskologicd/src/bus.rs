//! Fan out of daemon to client events, and the registry of who is connected.
//!
//! Every event carries its audience. Private board activity is addressed to
//! the member uids at the moment it happens, so nothing about a private
//! board ever reaches a socket that should not see it. Filtering happens
//! here, never in a client.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use taskologic_core::board::Board;
use taskologic_core::ids::Uid;
use taskologic_proto::ServerMessage;
use tokio::sync::{broadcast, mpsc};

#[derive(Clone, Debug)]
pub enum Audience {
    All,
    Uids(Arc<[Uid]>),
    Uid(Uid),
    /// Members plus every admin. For the fact that a board exists or
    /// changed shape, never for its content.
    UidsOrAdmins(Arc<[Uid]>),
}

impl Audience {
    pub fn for_board(board: &Board) -> Audience {
        if board.is_private {
            let mut uids: Vec<Uid> = board.members.clone();
            if !uids.contains(&board.owner_uid) {
                uids.push(board.owner_uid);
            }
            Audience::Uids(uids.into())
        } else {
            Audience::All
        }
    }

    /// Same as [`Audience::for_board`] but admins always hear about it.
    pub fn for_board_or_admins(board: &Board) -> Audience {
        match Audience::for_board(board) {
            Audience::Uids(u) => Audience::UidsOrAdmins(u),
            other => other,
        }
    }

    pub fn includes(&self, uid: Uid, is_admin: bool) -> bool {
        match self {
            Audience::All => true,
            Audience::Uids(u) => u.contains(&uid),
            Audience::Uid(u) => *u == uid,
            Audience::UidsOrAdmins(u) => is_admin || u.contains(&uid),
        }
    }
}

#[derive(Debug)]
pub struct Outbound {
    pub audience: Audience,
    pub msg: ServerMessage,
}

#[derive(Clone)]
pub struct Bus {
    tx: broadcast::Sender<Arc<Outbound>>,
}

impl Bus {
    pub fn new() -> Bus {
        // A slow client that falls 1024 events behind gets a Lagged error
        // and reloads, it does not stall everyone else.
        Bus {
            tx: broadcast::channel(1024).0,
        }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Arc<Outbound>> {
        self.tx.subscribe()
    }

    pub fn publish(&self, audience: Audience, msg: ServerMessage) {
        // Nobody listening is fine.
        let _ = self.tx.send(Arc::new(Outbound { audience, msg }));
    }
}

impl Default for Bus {
    fn default() -> Self {
        Self::new()
    }
}

pub type ConnId = u64;

#[derive(Clone, Debug)]
pub struct ClientEntry {
    pub uid: Uid,
    pub has_printer: bool,
    pub print_priority: i32,
    /// Direct line to one connection, for print jobs.
    pub tx: mpsc::UnboundedSender<ServerMessage>,
}

#[derive(Default)]
pub struct Registry {
    next: AtomicU64,
    clients: Mutex<HashMap<ConnId, ClientEntry>>,
}

impl Registry {
    pub fn register(&self, entry: ClientEntry) -> ConnId {
        let id = self.next.fetch_add(1, Ordering::Relaxed) + 1;
        self.clients
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(id, entry);
        id
    }

    pub fn unregister(&self, id: ConnId) {
        self.clients
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(&id);
    }

    /// The connection that print jobs for `uid` go to: highest priority
    /// among those with a printer, oldest connection on a tie. Predictable
    /// beats clever.
    pub fn printer_for(&self, uid: Uid) -> Option<ClientEntry> {
        let clients = self.clients.lock().unwrap_or_else(|p| p.into_inner());
        clients
            .iter()
            .filter(|(_, c)| c.uid == uid && c.has_printer)
            .max_by_key(|(id, c)| (c.print_priority, std::cmp::Reverse(**id)))
            .map(|(_, c)| c.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(uid: Uid, has_printer: bool, prio: i32) -> ClientEntry {
        ClientEntry {
            uid,
            has_printer,
            print_priority: prio,
            tx: mpsc::unbounded_channel().0,
        }
    }

    /// Which connection an entry came back from. Takes the lock and gives it
    /// straight back: `printer_for` wants it too, and the mutex is not reentrant.
    fn conn_id_of(r: &Registry, chosen: &ClientEntry) -> Option<ConnId> {
        let clients = r.clients.lock().unwrap();
        clients
            .iter()
            .find(|(_, e)| e.tx.same_channel(&chosen.tx))
            .map(|(id, _)| *id)
    }

    #[test]
    fn printer_routing_is_by_priority_then_age() {
        let r = Registry::default();
        let a = r.register(entry(7, true, 1));
        let _b = r.register(entry(7, false, 99));
        let c = r.register(entry(7, true, 1));
        let _d = r.register(entry(8, true, 50));
        assert_eq!(r.printer_for(7).map(|e| e.print_priority), Some(1));
        // Same priority: the older connection wins.
        assert_eq!(conn_id_of(&r, &r.printer_for(7).unwrap()), Some(a));
        r.unregister(a);
        assert_eq!(conn_id_of(&r, &r.printer_for(7).unwrap()), Some(c));
        assert!(r.printer_for(9).is_none());
    }

    #[test]
    fn audiences() {
        use taskologic_core::board::test_support::board_with_members;
        let mut b = board_with_members(1, &[1, 2]);
        let a = Audience::for_board(&b);
        assert!(a.includes(1, false) && a.includes(2, false) && !a.includes(3, false));
        assert!(
            !a.includes(3, true),
            "content stays with members even for admins"
        );
        let meta = Audience::for_board_or_admins(&b);
        assert!(meta.includes(3, true) && !meta.includes(3, false));
        b.is_private = false;
        assert!(Audience::for_board(&b).includes(3, false));
    }
}
