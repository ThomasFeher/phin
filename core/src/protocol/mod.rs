//! The protocol half of phin's core: what the daemon writes back to the
//! interface process, and (from plan item 3.4) the interpreter of the commands
//! it reads from it.
//!
//! This is the counterpart of [`core::technical`] for the wire protocol of
//! `doc/architecture/core-protocol.md`: the framing of the lines is in
//! [`proto`] (`proto/src/codec.rs`), the vocabulary in `proto/src/commands.rs`,
//! and this package holds the core side of the stream — the event writer of
//! plan item 3.3. Like [`core::domain`] it performs no I/O of its own: opening
//! the socket or the pipe is the transport of plan item 3.6 in
//! `core/src/technical/transport.rs`, and the writer here only ever hands a
//! finished line to the [`writer::EventSink`] it was given.

pub mod import_work;
pub mod interpret;
pub mod writer;

#[test]
fn protocol_module_compiles() {}
