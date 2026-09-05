//! The courier: explicit, bounded, opaque messages between Runtrol-managed sessions, without a transport.
//!
//! One managed coding-agent session writes an envelope naming another. The courier admits it to the target's
//! mailbox or refuses it, hands it over exactly once when the target asks, and correlates one reply to one ask.
//! It reads the envelope's framing and nothing else: the body is bytes it charges for, carries, and forgets.
//!
//! What lives here is the mechanical core the `mainPlan/session-fabric` design fixes: the identifiers, the
//! envelope, the delivery states, the ceilings, and the accounting. What does not live here, by construction: a
//! pipe, a clock, a process, a provider name, or any reading of what a body means. The Runtime wires this core to
//! its local named pipe and its managed process tree in the stamps that follow.

mod body;
mod courier;
pub mod env;
mod envelope;
mod id;
mod limits;
mod receipt;
pub mod wire;

pub use body::{BodyTooLarge, BoundedUtf8};
pub use courier::{Courier, Released, Swept};
pub use envelope::{BoundedSessionSet, CallEnvelope, CallKind, PROTOCOL_VERSION, VisitedBound};
pub use id::{CallId, IdError, ManagedSessionId, MessageId, RoomId};
pub use limits::{Limits, UnixMillis};
pub use receipt::{DeliveryState, Receipt, Refusal};
