//! The database. Roughly 200 bytes per session, and not one byte of anybody's conversation.
//!
//! # What is stored, and what is deliberately not
//!
//! runtrol stores the pointer it needs to find a session again: which CLI owns it, that CLI's own identifier
//! for it, where it works, when it was seen, and whether a process was serving it. Nothing else.
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
//! - [`error`] why the database could not be opened, read, or trusted

pub mod codec;
pub mod error;
pub mod open;
pub mod schema;
pub mod sessions;

pub use codec::{LiveProcess, SessionRow};
pub use error::StoreError;
pub use open::{CACHE_BYTES, Store};
pub use schema::{SCHEMA_VERSION, SessionKey};
pub use sessions::{Cursor, ListedSessions};
