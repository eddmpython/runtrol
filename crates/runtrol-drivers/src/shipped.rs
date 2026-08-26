//! The services this build ships, and nothing else.
//!
//! # Why this list is written by hand
//!
//! An earlier build generated this table from the official ACP Registry, so thirty adapters this product had
//! never measured rode along with the three it had. The sidebar then advertised fifteen of them as installable,
//! which told the operator his machine could grow services he never chose. Which coding services runtrol serves
//! is a product decision, not a snapshot of somebody else's index: a service arrives here when it has been
//! measured and asked for, one line at a time.
//!
//! Adding one is still a manifest and nothing else. The kernel selects code by `kind`, so a new entry that
//! speaks a kind this build already serves needs no code at all.

/// Manifest text for every provider compiled into this binary, in the order the sidebar meets them.
pub const MANIFESTS: &[&str] = &[
    include_str!("../manifests/claude.toml"),
    include_str!("../manifests/codex.toml"),
    include_str!("../manifests/grok.toml"),
];
