//! Wire protocol between the daemon and its clients.
//!
//! Newline delimited JSON over a Unix socket. JSON because it is trivially
//! debuggable with socat and because the future browser client wants it
//! anyway. If it ever becomes a bottleneck, swap the codec, the message types
//! stay the same.
//!
//! Requests go client to daemon and get exactly one response carrying the
//! same id. Events go daemon to client unsolicited. Both directions share
//! the domain types from `taskologic-core` rather than duplicating them.

pub mod codec;
pub mod message;

pub use message::*;

/// Bumped on any incompatible change to the message shapes.
pub const PROTOCOL_VERSION: u32 = 1;
