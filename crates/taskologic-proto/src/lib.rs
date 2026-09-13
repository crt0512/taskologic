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
///
/// 2 is 0.1.11. The single `reminder_minutes` on a task became one field per
/// date, so a 0.1.10 client saving a task against this daemon would silently
/// drop the reminder override rather than keep it. Refusing the connection
/// says so instead; `make update` installs both halves together anyway.
pub const PROTOCOL_VERSION: u32 = 2;
