//! Protocol definitions for phin (core protocol, plugin protocol, file formats).
//!
//! `codec`, `command` and `event` are the codec of the core protocol as
//! `doc/architecture/core-protocol.md` fixes it (plan item 3.1): JSON-Lines —
//! one JSON object per line, `\n` terminated, UTF-8 — carrying the integer
//! protocol version `v` on every line, with typed commands, typed events and
//! typed errors for an unknown command or an unparseable line. The interface
//! doc is the source of truth; the tests of plan item 3.1 read that file and
//! decode and round-trip every message it marks as a `test-example`.
//!
//! No I/O: the codec turns a text line into values and back, so it is unit-testable
//! without starting the core (plan rule 4); opening the stream is transport,
//! which lives in the core (`core/src/technical/`, plan item 3.6).

pub mod codec;
pub mod command;
pub mod event;

#[test]
fn proto_package_compiles() {}
