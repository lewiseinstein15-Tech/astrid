//! Library target for the `astrid` binary crate: exposes the session turn
//! loop (and the event pump it depends on) to embedders — the UniFFI bridge
//! in `astrid-ffi` drives the SAME turn pipeline the TUI ships.
pub mod eventlog;
pub mod session;
