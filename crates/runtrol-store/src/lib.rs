//! The database. Roughly 200 bytes per session, paired-device authorization, and not one byte of anybody's
//! conversation.
//!
//! # What is stored, and what is deliberately not
//!
//! runtrol stores the pointer it needs to find a session again: which CLI owns it, that CLI's own identifier
//! for it, where it works, when it was seen, and whether a process was serving it. It also stores paired-device
//! public identity, a one-way credential fingerprint, exact approved scope strings, safe display labels, and the
//! approval time. It never stores the bearer credential itself.
//!
//! No transcripts. No message previews, titles, turn counts, or token counts. Conversation content reaches a
//! subscriber only as events from the live provider process and through a small bounded in-memory reconnect window.
//! A subscriber outside that window receives an explicit gap. runtrol never fills it by discovering or reading a
//! provider transcript path.
//!
//! # Layout
//!
//! - [`open`] the one call site that opens the file, sets the cache, and checks the version
//! - [`schema`] the tables, the key encoding, and the version byte in every type name
//! - [`codec`] the stored row, encoded by hand because the layout is a promise to a file on disk
//! - [`sessions`] reading and writing rows, and the two durability settings and why there are two
//! - [`devices`] paired-device authorization, durable revocation, and fail-closed damage reporting
//! - [`integrations`] public Runtime enrollments and exact app grants, without any consumer private key
//! - [`integration_audit`] bounded public Runtime authorization metadata without caller or provider content
//! - [`integration_mutations`] bounded durable mutation ambiguity records without caller input
//! - [`error`] why the database could not be opened, read, or trusted

pub mod codec;
pub mod devices;
pub mod error;
mod integration_audit;
mod integration_mutations;
pub mod integrations;
pub mod open;
pub mod schema;
pub mod sessions;

pub use codec::{LiveProcess, SessionRow};
pub use devices::{DeviceRow, ListedDevices};
pub use error::StoreError;
pub use integration_audit::{
    INTEGRATION_AUDIT_MAX_BYTES, INTEGRATION_AUDIT_MAX_ROW_BYTES, INTEGRATION_AUDIT_MAX_ROWS,
    IntegrationAuditOutcome, IntegrationAuditRow,
};
pub use integration_mutations::{
    INTEGRATION_MUTATION_MAX_ROWS, IntegrationMutationRow, IntegrationMutationState,
};
pub use integrations::{
    EnrollmentRow, EnrollmentState, IntegrationKeyRotation, IntegrationRootRow, IntegrationRow,
};
pub use open::{CACHE_BYTES, Store};
pub use schema::{
    DeviceKey, EnrollmentKey, IntegrationAuditKey, IntegrationKey, IntegrationMutationKey,
    SCHEMA_VERSION, SessionKey,
};
pub use sessions::{Cursor, ListedSessions};
