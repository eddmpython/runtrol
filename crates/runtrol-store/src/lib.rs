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
//! No transcripts. No message previews, titles, turn counts, or token counts. Everything a person reads is
//! owned by the provider and read live from the provider's own store. That is not a storage optimization; it
//! is the reason this product is allowed to sit alongside the CLIs it supervises, and it is what lets a
//! subscriber that falls behind be served from the provider's file instead of from a copy runtrol would
//! otherwise have to keep.
//!
//! # Layout
//!
//! - [`open`] the one call site that opens the file, sets the cache, and checks the version
//! - [`schema`] the tables, the key encoding, and the version byte in every type name
//! - [`codec`] the stored row, encoded by hand because the layout is a promise to a file on disk
//! - [`sessions`] reading and writing rows, and the two durability settings and why there are two
//! - [`devices`] paired-device authorization, durable revocation, and fail-closed damage reporting
//! - [`error`] why the database could not be opened, read, or trusted

pub mod codec;
pub mod devices;
pub mod error;
pub mod open;
pub mod schema;
pub mod sessions;

pub use codec::{LiveProcess, SessionRow};
pub use devices::{DeviceRow, ListedDevices};
pub use error::StoreError;
pub use open::{CACHE_BYTES, Store};
pub use schema::{DeviceKey, SCHEMA_VERSION, SessionKey};
pub use sessions::{Cursor, ListedSessions};
