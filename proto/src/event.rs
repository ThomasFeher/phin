//! The events the core daemon writes to the interface process.
//!
//! The vocabulary is exactly `doc/architecture/core-protocol.md` ("Events"):
//! the interface consumes `result`, `progress`, `log`, `error` and
//! `cancelled`, the core answers a command with exactly one `result` (or
//! `error` / `cancelled`), and `progress` / `log` may appear any number of
//! times in between. The handshake answer `hello` is also an event — it is the
//! daemon's reply to the `version` command
//! (`doc/architecture/core-protocol.md`, "Handshake").
//!
//! [`Event`] is the typed form of one decoded event line; [`event_kind`] and
//! [`event_fields`] feed the encoder ([`crate::codec::encode_frame`]) and
//! [`event_from_fields`] is called by [`crate::codec::decode_line`] once `v`
//! and `event` have been split off. An event name outside that set is the
//! typed error [`crate::codec::CodecError.UnknownEvent`], never a panic, and
//! fields the doc does not mention are ignored.
//!
//! There is no I/O here: emitting the lines to a stream is the event writer of
//! plan item 3.3 in the core (`core/src/protocol/writer.rs`).

use serde_json::Map;
use serde_json::Value;

use crate::codec::CodecError;
use crate::codec::put_number;
use crate::codec::put_text;
use crate::codec::take_int;
use crate::codec::take_optional_text;
use crate::codec::take_text;

/// One event line, decoded.
#[derive(Debug, Eq, PartialEq)]
pub enum Event {
    /// `hello` — the daemon's handshake answer: `nonce` echoes the nonce the
    /// interface sent, `session` names this daemon instance (informational) and
    /// `version` is the protocol version the daemon speaks. The version
    /// comparison itself belongs to the interpreter of plan item 3.5.
    Hello(String, String, i64),
    /// `progress` — `cmd` of the command running, `done` of `total` items, and
    /// the `file` currently worked on when the command processes files.
    Progress(String, i64, i64, Option::<String>),
    /// `log` — `level` (`info`, `warn`, `error`, …) and the `text` shown to the
    /// user; may appear any number of times while a command runs.
    Log(String, String),
    /// `result` — the terminal event of a successful command: `cmd` names the
    /// command and the remaining JSON fields of the line are its payload
    /// (`affected` for `set-category`, `format`/`year`/`path` for `report`, …),
    /// which the command table of plan item 3.2 defines, so the codec keeps it
    /// as the object's fields instead of hard-coding one shape per command.
    Result(String, Map::<String, Value>),
    /// `error` — the terminal event of a failed command: `code` is one of the
    /// draft error codes of the doc (see [`ErrorReason`]), `text` the message,
    /// and `cmd` the command that failed when it is known.
    Error(Option::<String>, ErrorReason, String),
    /// `cancelled` — the terminal event after a `cancel` command: `cmd` names
    /// the command whose partial result was discarded.
    Cancelled(Option::<String>),
}

/// The `code` of an `error` event.
///
/// The values are the draft list of `doc/architecture/core-protocol.md`
/// ("Error"): `unknown-command`, `plugin-failed`, `converter-missing`, `io`,
/// `busy`. A code the doc does not name is kept verbatim in
/// [`ErrorReason.Other`] so that a newer core is still readable by an older
/// interface instead of being rejected.
#[derive(Debug, Eq, PartialEq)]
pub enum ErrorReason {
    /// The command name is not in the command table.
    UnknownCommand,
    /// An importer or analysis plugin failed or died.
    PluginFailed,
    /// An external tool the architecture names (`pdftotext`, …) is missing.
    ConverterMissing,
    /// A file could not be read or written.
    Io,
    /// The command cannot run because another command holds the daemon.
    Busy,
    /// A code the doc does not name, carried verbatim (`Other(text)`).
    Other(String),
}

/// The `event` name an event is written under.
pub fn event_kind(event: &Event) -> String {
    match event {
        Event::Hello(_, _, _) => { return "hello".to_string(); }
        Event::Progress(_, _, _, _) => { return "progress".to_string(); }
        Event::Log(_, _) => { return "log".to_string(); }
        Event::Result(_, _) => { return "result".to_string(); }
        Event::Error(_, _, _) => { return "error".to_string(); }
        Event::Cancelled(_) => { return "cancelled".to_string(); }
    }
}

/// The wire text of an error code.
pub fn reason_name(reason: &ErrorReason) -> String {
    match reason {
        ErrorReason::UnknownCommand => { return "unknown-command".to_string(); }
        ErrorReason::PluginFailed => { return "plugin-failed".to_string(); }
        ErrorReason::ConverterMissing => { return "converter-missing".to_string(); }
        ErrorReason::Io => { return "io".to_string(); }
        ErrorReason::Busy => { return "busy".to_string(); }
        ErrorReason::Other(name) => { return name.to_string(); }
    }
}

/// The parameter fields of an event, as the JSON object written on the line
/// besides `v` and `event`.
///
/// Optional fields are written only when they carry a value; the payload of a
/// `result` event is written as the remaining top-level fields of the line,
/// which is how the doc's examples come out (`"cmd":"set-category",
/// "affected":183`).
pub fn event_fields(event: &Event) -> Map::<String, Value> {
    let mut fields = Map::<String, Value>::new();
    match event {
        Event::Hello(nonce, session, version) => {
            put_text(&mut fields, "nonce", nonce);
            put_text(&mut fields, "session", session);
            put_number(&mut fields, "version", *version);
        }
        Event::Progress(cmd, done, total, file) => {
            put_text(&mut fields, "cmd", cmd);
            put_number(&mut fields, "done", *done);
            put_number(&mut fields, "total", *total);
            if let Some(name) = file { put_text(&mut fields, "file", name); }
        }
        Event::Log(level, text) => {
            put_text(&mut fields, "level", level);
            put_text(&mut fields, "text", text);
        }
        Event::Result(cmd, payload) => {
            put_text(&mut fields, "cmd", cmd);
            let mut entries = payload.clone();
            fields.append(&mut entries);
        }
        Event::Error(cmd, reason, text) => {
            if let Some(name) = cmd { put_text(&mut fields, "cmd", name); }
            put_text(&mut fields, "code", &reason_name(reason));
            put_text(&mut fields, "text", text);
        }
        Event::Cancelled(cmd) => {
            if let Some(name) = cmd { put_text(&mut fields, "cmd", name); }
        }
    }
    return fields;
}

/// Decode the parameter fields of the event named `name`.
///
/// A required parameter that is missing is [`crate::codec::CodecError.MissingField`]
/// and one of the wrong JSON type
/// [`crate::codec::CodecError.WrongType`]; an event name outside the set the
/// doc lists is [`crate::codec::CodecError.UnknownEvent`]. For a `result` event
/// every field that is not `cmd` becomes the payload, so re-encoding the
/// event writes them again unchanged.
pub fn event_from_fields(name: &str, fields: &mut Map::<String, Value>) -> Result::<Event, CodecError> {
    match name {
        "hello" => { return Ok(Event::Hello(take_text(fields, "nonce")?, take_text(fields, "session")?, take_int(fields, "version")?)); }
        "progress" => { return Ok(Event::Progress(take_text(fields, "cmd")?, take_int(fields, "done")?, take_int(fields, "total")?, take_optional_text(fields, "file")?)); }
        "log" => { return Ok(Event::Log(take_text(fields, "level")?, take_text(fields, "text")?)); }
        "result" => { return Ok(Event::Result(take_text(fields, "cmd")?, fields.clone())); }
        "error" => { return Ok(Event::Error(take_optional_text(fields, "cmd")?, reason_of(take_text(fields, "code")?), take_text(fields, "text")?)); }
        "cancelled" => { return Ok(Event::Cancelled(take_optional_text(fields, "cmd")?)); }
        other => { return Err(CodecError::UnknownEvent(other.to_string())); }
    }
}

/// The [`ErrorReason`] one wire `code` text stands for.
///
/// The five codes named by `doc/architecture/core-protocol.md` ("Error") map
/// to their variants; a code the doc does not name becomes
/// [`ErrorReason.Other`] carrying it verbatim, which is what lets an older
/// interface read the error of a newer core instead of rejecting the line.
fn reason_of(code: String) -> ErrorReason {
    match code.as_str() {
        "unknown-command" => { return ErrorReason::UnknownCommand; }
        "plugin-failed" => { return ErrorReason::PluginFailed; }
        "converter-missing" => { return ErrorReason::ConverterMissing; }
        "io" => { return ErrorReason::Io; }
        "busy" => { return ErrorReason::Busy; }
        other => { return ErrorReason::Other(other.to_string()); }
    }
}
