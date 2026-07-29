//! The runtrol executable. It dispatches on argv and does nothing else.
//!
//! Logic does not go here. When this file grows, the architecture starts moving somewhere the layer
//! gate cannot see it. Commands belong to `runtrol-cli` and the daemon belongs to `runtrol-daemon`.

// The two personalities this one executable links. Dispatch arrives with the command surface it
// dispatches to; until then these lines keep the declared dependency real rather than dead weight.
use runtrol_cli as _;
use runtrol_daemon as _;

fn main() {}
