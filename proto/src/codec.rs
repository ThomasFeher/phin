//! The JSON-Lines framing of the core protocol, and its version field.
//!
//! This is the transport contract of `doc/architecture/core-protocol.md` ("Transport",
//! accepted): one JSON object per line, `\n` terminated, UTF-8, and the framing
//! is NOT configurable; every line — command and event alike — carries the integer
//! field `v`, the protocol version ("Versioning"), an absent `v` means
//! [`PROTOCOL_VERSION`], and a line whose `v` is higher than this build supports
//! is refused with an `error` event, as is a line that cannot be read at all
//! ("Unknown input"): the daemon answers with an `error` event and stays alive,
//! and a partial line is kept pending rather than decoded or dropped.
//!
//! [`decode_line`] turns one received line into a typed [`Message`];
//! [`encode_frame`] turns a [`crate::command::Command`] or
//! [`crate::event::Event`] back into exactly one `\n`-terminated frame.
//! [`split_lines`] and [`Framing`] are the `\n` framing on the byte/text side,
//! so a chunk that ends mid-line never loses the first half of the line.
//!
//! The module is pure: no socket, no file, no daemon, so encode and decode are
//! unit-testable without starting the core (plan rule 4). Opening the stream is
//! the transport of plan item 3.6 (`core/src/technical/transport.rs`) and writing
//! the events is the writer of plan item 3.3.
//!
//! Every failure is a typed [`CodecError`], never a panic. The message of an
//! error becomes the `text` of the `error` event the daemon writes, which is
//! why [`error_text`] exists here rather than in the core.

use serde_json::Map;
use serde_json::Value;

use crate::command::Command;
use crate::command::command_fields;
use crate::command::command_from_fields;
use crate::command::command_name;
use crate::commands::COMMAND_FIELD;
use crate::commands::EVENT_FIELD;
use crate::commands::PROTOCOL_FIELD;
use crate::commands::reserved_field;
use crate::event::Event;
use crate::event::event_fields;
use crate::event::event_from_fields;
use crate::event::event_kind;

/// The protocol version this build writes and the highest version it accepts
/// on the wire ("Versioning"). The constant is owned by the command vocabulary
/// (see [`crate::commands::PROTOCOL_VERSION`], where plan item 3.5 expects it);
/// this is a re-export so the name `proto::codec::PROTOCOL_VERSION` keeps
/// working and both modules read one value.
pub const PROTOCOL_VERSION: i64 = crate::commands::PROTOCOL_VERSION;

/// What can be wrong with one line of the stream.
#[derive(Debug, Eq, PartialEq)]
pub enum CodecError {
    /// The line is not exactly one JSON object: it does not parse, or it parses
    /// to an array, a bare value or a truncated object. A line the core cannot
    /// read is answered with an `error` event, the daemon stays alive and the
    /// stream is not resynchronized ("Unknown input").
    UnparseableLine,
    /// The line parsed to a JSON object that carries neither `cmd` nor `event`,
    /// so it is neither a command nor an event and cannot be dispatched.
    MissingDiscriminator,
    /// `cmd` names a command the command table does not contain; the name is
    /// carried so the `error` event can name it back to the interface.
    UnknownCommand(String),
    /// `event` names an event outside the set `doc/architecture/core-protocol.md`
    /// ("Events") lists.
    UnknownEvent(String),
    /// A required parameter of a known command or event was absent from the line;
    /// the parameter name is carried so the message can name it.
    MissingField(String),
    /// A parameter of a known command or event had the wrong JSON type.
    WrongType(String),
    /// A parameter of a command frame or a payload key of a `result` event is
    /// named like a field the framing of the line writes for itself (`v`, or
    /// the discriminator `event` / `cmd`), so writing the frame would silently
    /// overwrite it and reading it back would take the value as framing rather
    /// than as data; the name is carried so the `error` event can name it back
    /// (see [`crate::commands::reserved_field`]).
    ReservedField(String),
    /// A command line carried two parameters that the command table writes as
    /// alternatives (`confirm-annotation` takes `id` | `reject` in
    /// `doc/architecture/core-protocol.md`), so one of them would silently
    /// win; both parameter names are carried and neither is dropped.
    ConflictingFields(String, String),
    /// `v` was higher than [`PROTOCOL_VERSION`]; the version read is carried
    /// so the interface can be told which version the daemon cannot speak.
    UnsupportedVersion(i64),
}

/// The message text of a codec error, ready to be the `text` of the `error`
/// event the daemon answers with.
///
/// The text names what is wrong (`unknown command: frob`, as in the error
/// example of `doc/architecture/core-protocol.md`); the `code` of that event is
/// chosen by the interpreter of plan item 3.4, which maps an unknown command to
/// `unknown-command` and an unparseable line to the code it decides on.
pub fn error_text(error: &CodecError) -> String {
    match error {
        CodecError::UnparseableLine => { return "line is not one JSON object".to_string(); }
        CodecError::MissingDiscriminator => { return "line has neither cmd nor event".to_string(); }
        CodecError::UnknownCommand(name) => { return format!("unknown command: {name}"); }
        CodecError::UnknownEvent(name) => { return format!("unknown event: {name}"); }
        CodecError::MissingField(name) => { return format!("missing parameter: {name}"); }
        CodecError::WrongType(name) => { return format!("parameter has the wrong JSON type: {name}"); }
        CodecError::ReservedField(name) => { return format!("parameter is reserved by the framing of a line: {name}"); }
        CodecError::ConflictingFields(first, second) => { return format!("parameters are mutually exclusive: {first} and {second}"); }
        CodecError::UnsupportedVersion(version) => { return format!("unsupported protocol version: {version}"); }
    }
}

/// One decoded line of the stream: a command sent by an interface process, or
/// an event written by the core daemon.
#[derive(Debug, Eq, PartialEq)]
pub enum Message {
    /// A command line, carrying the `cmd` field ("Commands").
    Inbound(Command),
    /// An event line, carrying the `event` field ("Events").
    Outbound(Event),
}

/// Read one complete line of the stream as a message.
///
/// The line may end with `\n` or hold nothing but the JSON object; either way
/// exactly one object is expected. `v` is checked first (see [`read_version`]),
/// then the `event` field makes the line an event and the `cmd` field a command
/// (see [`decode_fields`] for why `event` is checked first); a line with neither
/// is [`CodecError.MissingDiscriminator`]. Fields the interface or the doc does
/// not know are ignored, so a newer peer is still readable; nothing is defaulted,
/// so a missing required parameter is [`CodecError.MissingField`] rather than a
/// silent empty value.
pub fn decode_line(line: &str) -> Result::<Message, CodecError> {
    let mut parsed = serde_json::from_str::<Value>(line).map_err(|_| CodecError::UnparseableLine)?;
    let fields = parsed.as_object_mut().ok_or(CodecError::UnparseableLine)?;
    return decode_fields(fields);
}

/// Read an already parsed JSON object as a command or an event.
///
/// This is what [`decode_line`] does with the object it parsed, and it is public
/// because the interpreter of plan item 3.4 may already hold a parsed object.
///
/// The `event` field decides first, because no command parameter of the command
/// table is named `event`, while `cmd` is NOT a safe discriminator on its own: `doc/architecture/core-protocol.md` writes `cmd` inside `progress`, `result`, `error` and `cancelled` events to name the command the event is about. A line with `cmd` only is a command; a line with neither is [`CodecError.MissingDiscriminator`], and fields neither knows are ignored.
pub fn decode_fields(fields: &mut Map::<String, Value>) -> Result::<Message, CodecError> {
    read_version(fields)?;
    if let Some(held) = fields.remove(EVENT_FIELD) {
        let name = held.as_str().ok_or(CodecError::WrongType(EVENT_FIELD.to_string()))?;
        return Ok(Message::Outbound(event_from_fields(name, fields)?));
    }
    let held = fields.remove(COMMAND_FIELD).ok_or(CodecError::MissingDiscriminator)?;
    let name = held.as_str().ok_or(CodecError::WrongType(COMMAND_FIELD.to_string()))?;
    return Ok(Message::Inbound(command_from_fields(name, fields)?));
}

/// Write a message as one frame of the stream.
///
/// The `\n` terminator is part of the frame, because the framing of the
/// protocol is one JSON object per line, `\n` terminated; the JSON text itself
/// can never contain a line break, so one message is always exactly one line,
/// whatever the text fields hold. `v` is written as [`PROTOCOL_VERSION`], and the
/// keys are written in the order serde_json's map keeps them, which without
/// the `preserve_order` feature is lexicographic: a frame is therefore equal
/// to a `doc/architecture/core-protocol.md` example as a JSON value (same
/// keys, same values) but NOT byte for byte, because the doc writes the keys
/// in protocol order. Re-encoding a decoded message is reproducible; a
/// byte-for-byte comparison against the doc (plan item 7.5) has to
/// canonicalize the key order itself.
///
/// Writing is refused with [`CodecError.ReservedField`] when the message carries
/// a field the framing also writes for itself: a frame is one JSON object, so
/// it can hold each name once, and a parameter of the command table or a payload
/// key of a `result` event named `v` / `event` / `cmd` would be overwritten
/// instead of written (see [`crate::commands::reserved_field`]).
pub fn encode_frame(message: &Message) -> Result::<String, CodecError> {
    match message {
        Message::Inbound(command) => {
            // No parameter of the command table may be named like a field the
            // framing writes for itself: a command frame carrying `event` would
            // be read back as an event, and one carrying `v` or `cmd` would be
            // overwritten by the frame's own version and discriminator.
            let fields = command_fields(command);
            if let Some(reserved) = reserved_field(&fields) {
                return Err(CodecError::ReservedField(reserved));
            }
            return frame_of(fields, COMMAND_FIELD, command_name(command));
        }
        Message::Outbound(event) => {
            if let Some(reserved) = reserved_payload_field(event) {
                return Err(CodecError::ReservedField(reserved));
            }
            return frame_of(event_fields(event), EVENT_FIELD, event_kind(event));
        }
    }
}

/// The payload key of a `result` event that the framing of a line also writes
/// for itself, [`None`] for every other event.
///
/// The payload of a `result` event is written as the remaining fields of the
/// line (`doc/architecture/core-protocol.md`), so a payload key named `v`, `event`
/// or `cmd` would either be read back as framing or be overwritten by the
/// framing of the frame (`cmd` even overwrites the name of the command the
/// result belongs to), which is why the payload is checked before the frame is
/// built instead of being dropped on the floor.
fn reserved_payload_field(event: &Event) -> Option::<String> {
    match event {
        Event::Result(_, payload) => { return reserved_field(payload); }
        _ => { return None; }
    }
}

/// The complete frame of one line: the `v` field, the `discriminator` field
/// holding `name`, and the parameter fields of `fields`. Private to this module
/// because only a command or an event can be the source of a frame. `fields`
/// is taken by value, so the fields of the message are moved into the object
/// that is written out instead of being copied.
///
/// A field of `fields` already named `v` or `discriminator` would be
/// overwritten by the framing, so the frame is refused with
/// [`CodecError.ReservedField`]; a field holding exactly the value the framing
/// writes is harmless and is accepted.
fn frame_of(mut fields: Map::<String, Value>, discriminator: &str, name: String) -> Result::<String, CodecError> {
    let framing = [PROTOCOL_FIELD.to_string(), discriminator.to_string()].to_vec();
    for field in framing {
        if fields.contains_key(&field) {
            return Err(CodecError::ReservedField(field.to_string()));
        }
    }
    fields.insert(discriminator.to_string(), Value::String(name));
    fields.insert(PROTOCOL_FIELD.to_string(), number(PROTOCOL_VERSION));
    return Ok(Value::Object(fields).to_string() + "\n");
}

/// The version field of one line, checked and removed.
///
/// Absent `v` means [`PROTOCOL_VERSION`] ("Versioning", draft: an absent `v`
/// means 1). A `v` that is not an integer is [`CodecError.WrongType`], and one
/// higher than this build supports is
/// [`CodecError.UnsupportedVersion`], which is what the daemon turns into an
/// `error` event naming the version. A lower `v` is accepted: this build reads
/// what older peers wrote.
pub fn read_version(fields: &mut Map::<String, Value>) -> Result::<i64, CodecError> {
    if let Some(held) = fields.remove(PROTOCOL_FIELD) {
        let read = held.as_i64().ok_or(CodecError::WrongType(PROTOCOL_FIELD.to_string()))?;
        if read > PROTOCOL_VERSION { return Err(CodecError::UnsupportedVersion(read)); }
        return Ok(read);
    }
    return Ok(PROTOCOL_VERSION);
}

/// The JSON number for an integer value of a field (`year`, `done`, `total`,
/// the `version` of a `hello` event, and the line's own `v`).
///
/// `serde_json` converts an `i64` to a `Number` through a total conversion
/// (`Number::from`), which cannot fail, so nothing here can panic.
fn number(value: i64) -> Value {
    return Value::Number(serde_json::Number::from(value));
}

/// Write the text of a field of a command or event into `fields`.
///
/// Used by the two writers; the name is given as plain text so the callers do
/// not have to convert every field name into the map's key type themselves.
pub fn put_text(fields: &mut Map::<String, Value>, name: &str, text: &str) {
    fields.insert(name.to_string(), Value::String(text.to_string()));
}

/// Write the integer value of a field of a command or event into `fields`.
pub fn put_number(fields: &mut Map::<String, Value>, name: &str, value: i64) {
    fields.insert(name.to_string(), number(value));
}

/// The text of a required field of a command or event, removed from `fields`.
///
/// Absent is [`CodecError.MissingField`] and a non-string is
/// [`CodecError.WrongType`], so the interface gets the parameter named back.
pub fn take_text(fields: &mut Map::<String, Value>, name: &str) -> Result::<String, CodecError> {
    let held = fields.remove(name).ok_or(CodecError::MissingField(name.to_string()))?;
    return Ok(held.as_str().ok_or(CodecError::WrongType(name.to_string()))?.to_string());
}

/// The text of an optional field of a command or event, removed from `fields`;
/// absent is read as [`None`], a non-string as
/// [`CodecError.WrongType`] — an optional parameter may be left out, never
/// written as the wrong type.
pub fn take_optional_text(fields: &mut Map::<String, Value>, name: &str) -> Result::<Option::<String>, CodecError> {
    if !fields.contains_key(name) { return Ok(None); }
    return Ok(Some(take_text(fields, name)?));
}

/// The integer value of a required field of a command or event, removed from
/// `fields`; absent is [`CodecError.MissingField`] and a non-integer (a float,
/// a string, `null`) is [`CodecError.WrongType`], because the protocol writes
/// numbers as JSON integers.
pub fn take_int(fields: &mut Map::<String, Value>, name: &str) -> Result::<i64, CodecError> {
    let held = fields.remove(name).ok_or(CodecError::MissingField(name.to_string()))?;
    return Ok(held.as_i64().ok_or(CodecError::WrongType(name.to_string()))?);
}

/// The integer value of an optional field of a command or event, removed from
/// `fields` (the optional `year` of a `report` command).
pub fn take_optional_int(fields: &mut Map::<String, Value>, name: &str) -> Result::<Option::<i64>, CodecError> {
    if !fields.contains_key(name) { return Ok(None); }
    return Ok(Some(take_int(fields, name)?));
}

/// The framing state of one stream: the text received after the last complete
/// line, which has not been decoded because it is not a line yet.
///
/// It is empty at the start of a stream and after a chunk that ended with
/// `\n`. The pending text is never dropped and never resynchronized
/// ("Unknown input"): the stream never recovers by skipping the rest of a
/// partial line, it waits for the rest of that line.
#[derive(Debug, Eq, PartialEq)]
pub struct Framing {
    /// The text of the line currently being received.
    pub partial: String,
}

/// Split stream text into the lines it holds and the text left over.
///
/// The first value is the complete, `\n`-terminated lines without their
/// terminator; the second is the text after the last `\n`, which is not a line
/// yet. A text ending in `\n` therefore leaves nothing pending, which is how
/// the "`\n` terminated, one object per line" framing of the doc is read.
pub fn split_lines(text: &str) -> (Vec::<String>, String) {
    let parts: Vec::<String> = text.split('\n').map(|part| part.to_string()).collect();
    return (parts[0 .. parts.len() - 1].to_vec(), parts[parts.len() - 1].to_string());
}

/// Append `chunk` to the pending text of `framing` and hand out the lines this
/// chunk completed, keeping whatever follows the last `\n` pending.
///
/// A chunk that ends mid-line yields the lines before it and leaves the
/// half-line in [`Framing.partial`], so the next chunk continues the same line
/// instead of starting a new one.
pub fn feed(framing: &mut Framing, chunk: &str) -> Vec::<String> {
    framing.partial += chunk;
    let (lines, partial) = split_lines(&framing.partial);
    framing.partial = partial;
    return lines;
}

// ============================================================================
// Tests
// ============================================================================

/// The text of `doc/architecture/core-protocol.md`, read AT TEST TIME — the
/// ONE such reader in `proto` (reviewer finding 5 on plan item 3.6 forbade a
/// third copy of this body).
///
/// The interface doc is the source of truth, so a codec test that hard-coded
/// an example could stay green while the doc and the code disagree. The path
/// is resolved from the `proto` package directory (where `cargo test` runs
/// the test binary, hence `../doc/…`) and from the workspace root
/// (`doc/…`), whichever of the two the binary happens to be started from.
/// PUBLIC within the package (test builds only): `proto/src/commands.rs` and
/// `proto/src/handshake.rs` call this instead of keeping their own copy.
#[cfg(test)]
pub fn read_core_protocol_doc() -> String {
    match std::fs::read_to_string("doc/architecture/core-protocol.md") {
        Ok(text) => { return text; }
        Err(_) => {
            match std::fs::read_to_string("../doc/architecture/core-protocol.md") {
                Ok(text) => { return text; }
                Err(reason) => { panic!("cannot open doc/architecture/core-protocol.md from the proto package directory or from the workspace root: {reason}"); }
            }
        }
    }
}

/// The id of a `<!-- test-example id=<id> -->` marker line. PUBLIC within the
/// package (test builds only) so `proto/src/commands.rs` and `proto/src/handshake.rs`
/// can reuse this module's reading of the doc instead of copying it.
#[cfg(test)]
pub fn marker_id(line: &str) -> String {
    let mut id = String::new();
    let mut reading = false;
    for character in line.chars() {
        if !reading {
            if character == '=' {
                reading = true;
            }
            continue;
        }
        if character == ' ' {
            break;
        }
        id.push(character);
    }
    return id;
}

/// The ids and the JSON lines of every `test-example` of the core protocol doc.
///
/// A marker comment is followed by its example in a `json fence, so the scan
/// keeps the id of the last marker seen until the fenced line is reached; one
/// example per marker, and no example without a marker. PUBLIC within the package
/// (test builds only) for the same reason as [`marker_id`].
#[cfg(test)]
pub fn doc_examples(text: &str) -> (Vec::<String>, Vec::<String>) {
    let mut ids = Vec::<String>::new();
    let mut examples = Vec::<String>::new();
    let (lines, _tail) = split_lines(text);
    let mut pending = String::new();
    let mut inside_fence = false;
    for line in lines {
        if line.contains("<!-- test-example id=") {
            pending = marker_id(&line);
            inside_fence = false;
            continue;
        }
        if pending.len() == 0 {
            continue;
        }
        if line.starts_with("```") {
            inside_fence = !inside_fence;
            continue;
        }
        if inside_fence {
            ids.push(pending.to_string());
            examples.push(line.to_string());
            pending = String::new();
        }
    }
    return (ids, examples);
}

/// The lines of the fenced block that follows the FIRST doc line containing
/// `marker`.
///
/// `doc_examples` above only reaches blocks a `<!-- test-example id=… -->`
/// marker introduces, and the fenced `text` block fixing the format of
/// `state/core.lock` (`doc/architecture/core-protocol.md`, "Lock file and
/// discovery") carries no marker, so the tests of `proto/src/handshake.rs` that
/// pin the lock format need this reading of the doc; it lives HERE because
/// `codec.rs` owns the doc scans of the package (reviewer finding 5 on plan item
/// 3.6: no second doc-reading body elsewhere).
#[cfg(test)]
pub fn fenced_block_after(text: &str, marker: &str) -> Vec::<String> {
    let (lines, _tail) = split_lines(text);
    let mut block = Vec::<String>::new();
    let mut seen_marker = false;
    let mut inside_fence = false;
    for line in lines {
        if !seen_marker {
            if line.contains(marker) {
                seen_marker = true;
            }
            continue;
        }
        if line.starts_with("```") {
            if inside_fence {
                return block;
            }
            inside_fence = true;
            continue;
        }
        if inside_fence {
            block.push(line.to_string());
        }
    }
    return block;
}

/// The frame `frame` written for the message `written` must be exactly one
/// line of the stream and must read back as the message that was written.
#[cfg(test)]
fn assert_frame_is_one_line_and_reads_back_equal(frame: &str, written: &Message, label: &str) {
    let (lines, tail) = split_lines(frame);
    if lines.len() != 1 || tail.len() != 0 {
        panic!("the frame of {label} is not exactly one line: [{frame}]");
    }
    match decode_line(frame) {
        Err(_) => { panic!("the frame of {label} does not read back: [{frame}]"); }
        Ok(back) => {
            if !(back == *written) {
                panic!("the frame of {label} reads back as a different message than written: [{frame}]");
            }
        }
    }
}

/// Encode `message` into a frame and check the frame with
/// [`assert_frame_is_one_line_and_reads_back_equal`].
#[cfg(test)]
fn assert_frame_round_trips(message: Message, label: &str) {
    let frame = encode_frame(&message).unwrap();
    assert_frame_is_one_line_and_reads_back_equal(&frame, &message, label);
}

#[test]
fn unknown_command_is_a_typed_error_not_a_panic() {
    let frob = decode_line("{\"v\":1,\"cmd\":\"frob\",\"path\":\"/x\"}");
    assert!(frob.unwrap_err() == CodecError::UnknownCommand("frob".to_string()));
    // The whole line is still read as far as the codec can: the name is carried
    // so the daemon's `error` event can name the command back (the error example
    // of doc/architecture/core-protocol.md reads "unknown command: frob").
    let text = error_text(&CodecError::UnknownCommand("frob".to_string()));
    assert!(text == "unknown command: frob");
    // A command the table knows, with a required parameter missing, is a
    // different typed error; the daemon stays alive in both cases.
    let import = decode_line("{\"v\":1,\"cmd\":\"import\"}");
    let import_reason = import.unwrap_err();
    assert!(import_reason == CodecError::MissingField("path".to_string()));
    assert!(error_text(&import_reason) == "missing parameter: path");
}

#[test]
fn unparseable_line_is_a_typed_error() {
    // Not JSON at all, JSON that is not an object, two objects on one line, a
    // truncated object (a partial line), an empty line, and a JSON object that
    // is neither a command nor an event. None of them panics and none of them
    // makes the codec skip ahead: a partial line stays what it is (see the
    // framing tests), as doc/architecture/core-protocol.md ("Unknown input")
    // requires.
    assert!(decode_line("nope").unwrap_err() == CodecError::UnparseableLine);
    assert!(decode_line("[1,2]").unwrap_err() == CodecError::UnparseableLine);
    assert!(decode_line("1").unwrap_err() == CodecError::UnparseableLine);
    assert!(decode_line("{\"v\":1,\"cmd\":\"cancel\"} {\"v\":1,\"cmd\":\"cancel\"}").unwrap_err() == CodecError::UnparseableLine);
    assert!(decode_line("{\"v\":1,\"cmd\":").unwrap_err() == CodecError::UnparseableLine);
    assert!(decode_line("").unwrap_err() == CodecError::UnparseableLine);
    assert!(decode_line("{\"v\":1,\"who\":\"nobody\"}").unwrap_err() == CodecError::MissingDiscriminator);
    assert!(decode_line("{\"v\":1,\"cmd\":7}").unwrap_err() == CodecError::WrongType("cmd".to_string()));
    // An unknown event name is the typed error naming it, too.
    assert!(decode_line("{\"v\":1,\"event\":\"sneezed\"}").unwrap_err() == CodecError::UnknownEvent("sneezed".to_string()));
}

#[test]
fn absent_version_means_the_default_and_a_higher_version_is_refused() {
    // "Versioning": every line carries `v`; an absent `v` means 1, and a version
    // this build does not support is rejected instead of half-read.
    let default_line = decode_line("{\"cmd\":\"cancel\"}");
    assert!(default_line.unwrap() == Message::Inbound(Command::Cancel));
    let older_peer = decode_line("{\"v\":0,\"cmd\":\"cancel\"}");
    assert!(older_peer.unwrap() == Message::Inbound(Command::Cancel));
    assert!(PROTOCOL_VERSION == 1);

    let newer_peer = decode_line("{\"v\":2,\"cmd\":\"cancel\"}");
    assert!(newer_peer.unwrap_err() == CodecError::UnsupportedVersion(2));
    let why = error_text(&CodecError::UnsupportedVersion(2));
    assert!(why == "unsupported protocol version: 2");
    // `v` is read and checked before `cmd` / `event` is dispatched, so a line
    // from a newer peer is refused even when the command itself is unknown too.
    assert!(decode_line("{\"v\":9,\"cmd\":\"frob\"}").unwrap_err() == CodecError::UnsupportedVersion(9));
    assert!(decode_line("{\"v\":\"1\",\"cmd\":\"cancel\"}").unwrap_err() == CodecError::WrongType("v".to_string()));
    assert!(decode_line("{\"v\":1.5,\"cmd\":\"cancel\"}").unwrap_err() == CodecError::WrongType("v".to_string()));
    assert!(decode_line("{\"v\":true,\"cmd\":\"cancel\"}").unwrap_err() == CodecError::WrongType("v".to_string()));
}

#[test]
fn fields_the_documentation_does_not_name_are_ignored() {
    // A parameter the command table does not mention must not stop an older
    // core from reading the command it does know.
    let import = decode_line("{\"v\":1,\"cmd\":\"import\",\"path\":\"/x\",\"since\":\"2024\",\"count\":7}");
    let read_import = import.unwrap();
    assert!(read_import == Message::Inbound(Command::Import("/x".to_string())));
    let cancelled = decode_line("{\"v\":1,\"event\":\"cancelled\",\"cmd\":\"import\",\"why\":\"user pressed ctrl c\"}");
    let read_cancelled = cancelled.unwrap();
    assert!(read_cancelled == Message::Outbound(Event::Cancelled(Some("import".to_string()))));
    // Ignoring is one-directional: the writer emits only the documented fields,
    // so the core does not invent fields it does not understand.
    let frame = encode_frame(&read_import).unwrap();
    assert!(!frame.contains("since"));
    assert!(!frame.contains("count"));
}

#[test]
fn multi_line_input_is_split_on_line_feeds_and_a_partial_line_stays_pending() {
    // Framing ("Transport"): one JSON object per line, `\n` terminated; a chunk
    // that ends mid-line must not be decoded and must not be dropped ("Unknown
    // input": the stream never resynchronizes on a partial line).
    let (whole, tail) = split_lines("{\"v\":1,\"cmd\":\"cancel\"}\n{\"v\":1,\"cmd\":\"shutdown\"}\n");
    assert!(whole.len() == 2);
    assert!(tail.len() == 0);
    assert!(decode_line(&whole[0]).unwrap() == Message::Inbound(Command::Cancel));
    assert!(decode_line(&whole[1]).unwrap() == Message::Inbound(Command::Shutdown));

    let (before, half) = split_lines("{\"v\":1,\"cmd\":\"cancel\"}\n{\"v\":1,\"cmd\":\"sh");
    assert!(before.len() == 1);
    assert!(half == "{\"v\":1,\"cmd\":\"sh");

    let mut framing = Framing { partial: String::new() };
    let first = feed(&mut framing, "{\"v\":1,\"cmd\":\"cancel\"}\n{\"v\":1,\"cmd\":\"sh");
    assert!(first.len() == 1);
    assert!(framing.partial == "{\"v\":1,\"cmd\":\"sh");
    assert!(decode_line(&first[0]).unwrap() == Message::Inbound(Command::Cancel));
    let second = feed(&mut framing, "utdown\"}\n");
    assert!(second.len() == 1);
    assert!(framing.partial.len() == 0);
    assert!(decode_line(&second[0]).unwrap() == Message::Inbound(Command::Shutdown));
    // Nothing was skipped between the two feeds: the half-line that was pending
    // was joined with the next chunk, which is what keeps the line intact.
    let empty = feed(&mut framing, "");
    assert!(empty.len() == 0);
}

#[test]
fn unicode_text_survives_decode_and_encode() {
    // UTF-8 is the encoding of the stream ("Transport"): non-ASCII text is
    // written as itself, so a name a user typed is still a name the interface
    // can show, and it still fits on one line.
    let line = "{\"v\":1,\"event\":\"log\",\"level\":\"info\",\"text\":\"probe: café — 1.234.567 € bank_a.pdf\"}";
    let expected = Message::Outbound(Event::Log("info".to_string(), "probe: café — 1.234.567 € bank_a.pdf".to_string()));
    let decoded = decode_line(&line).unwrap();
    assert!(decoded == expected);
    let frame = encode_frame(&decoded).unwrap();
    assert!(frame.contains("café"));
    assert!(frame.contains("€"));
    assert_frame_is_one_line_and_reads_back_equal(&frame, &decoded, "the unicode log event");

    let odd = decode_line("{\"v\":1,\"cmd\":\"import\",\"path\":\"/data/renaming/сашa коштн–100.pdf\"}");
    let read_odd = odd.unwrap();
    let odd_frame = encode_frame(&read_odd).unwrap();
    assert!(odd_frame.contains("сашa"));
    assert_frame_is_one_line_and_reads_back_equal(&odd_frame, &read_odd, "the unicode import path");
}

#[test]
fn a_text_field_cannot_forge_a_second_line() {
    // A `text` / `path` field may hold anything the user or a plugin produced.
    // `encode_frame` escapes it, so one message is always exactly one frame;
    // a stream reader can never be handed a line that is not one object.
    let log = decode_line("{\"v\":1,\"event\":\"log\",\"level\":\"warn\",\"text\":\"a\\nb\"}");
    let read_log = log.unwrap();
    let frame = encode_frame(&read_log).unwrap();
    assert!(!frame.contains("\n\n"));
    let (lines, tail) = split_lines(&frame);
    assert!(lines.len() == 1);
    assert!(tail.len() == 0);
    assert!(decode_line(&frame).unwrap() == read_log);
}

#[test]
fn every_command_of_the_command_table_round_trips() {
    // One value per row of the command table of doc/architecture/core-protocol.md
    // ("Commands"), so the codec is checked against the whole vocabulary, including
    // the two commands the doc gives no example for.
    assert_frame_round_trips(Message::Inbound(Command::Version("6f2c1a48e0b7d9c3".to_string())), "version");
    assert_frame_round_trips(Message::Inbound(Command::Import("/data/statements/2024/bank_a.pdf".to_string())), "import");
    assert_frame_round_trips(Message::Inbound(Command::Report("html".to_string(), Some(2024))), "report with year");
    assert_frame_round_trips(Message::Inbound(Command::Report("ods".to_string(), None)), "report without year");
    assert_frame_round_trips(Message::Inbound(Command::SetCategory("3f9a1c7e".to_string(), "food:supermarket".to_string(), "user".to_string())), "set-category");
    assert_frame_round_trips(Message::Inbound(Command::ConfirmAnnotation("ann00000001".to_string(), crate::command::ConfirmTarget::Id("3f9a1c7e".to_string()))), "confirm-annotation accepted");
    assert_frame_round_trips(Message::Inbound(Command::ConfirmAnnotation("ann00000002".to_string(), crate::command::ConfirmTarget::Reject)), "confirm-annotation rejected");
    assert_frame_round_trips(Message::Inbound(Command::ListPending), "list-pending");
    assert_frame_round_trips(Message::Inbound(Command::Cancel), "cancel");
    assert_frame_round_trips(Message::Inbound(Command::Shutdown), "shutdown");
}

#[test]
fn a_payload_field_the_framing_uses_is_refused_rather_than_dropped() {
    // Plan item 3.1 review obligation, raised by the review of that item: the
    // names the framing writes for itself -- the version field `v`, the
    // discriminator `event` of an outbound line and the discriminator `cmd` of
    // an inbound one -- must NOT be silently droppable as a field of the free
    // payload of a `result` event. Writing such a frame would overwrite the
    // value on the way out and read it back as framing rather than as data,
    // so `encode_frame` refuses with [`CodecError.ReservedField`] naming the
    // field. The names come from the vocabulary of `proto/src/commands.rs`
    // ([`crate::commands::reserved_field`], whose own test is in that module),
    // and the three names iterated below are the constants of that very module
    // ([`PROTOCOL_FIELD`], [`EVENT_FIELD`] and [`COMMAND_FIELD`], imported at
    // the top of this file), so the framing of a line and the vocabulary of the
    // table cannot drift apart into two lists of strings.
    let mut payload = serde_json::Map::<String, serde_json::Value>::new();
    payload.insert("state".to_string(), serde_json::to_value("pending".to_string()).unwrap());
    payload.insert("affected".to_string(), serde_json::to_value(183).unwrap());
    let frame = encode_frame(&Message::Outbound(Event::Result("set-category".to_string(), payload)));
    assert!(frame.is_ok());
    let written = frame.unwrap();
    let read_back = decode_line(&written);
    if !read_back.is_ok() {
        let reason = error_text(&read_back.unwrap_err());
        panic!("the result frame [{written}] carrying the payload fields the interface asked for does not read back ({reason}), so a payload of ordinary names is no longer round-trippable");
    }

    // Each of the three names the framing uses, put into the payload of the only
    // event that carries free fields, is the typed error naming that field --
    // for `v` and `cmd` the frame would lose the value, and for `event` it
    // would be read back as a different kind of message.
    for name in [PROTOCOL_FIELD, EVENT_FIELD, COMMAND_FIELD].to_vec() {
        let mut carrying = serde_json::Map::<String, serde_json::Value>::new();
        carrying.insert(name.to_string(), serde_json::to_value(1).unwrap());
        let refused = encode_frame(&Message::Outbound(Event::Result("report".to_string(), carrying)));
        assert!(refused.unwrap_err() == CodecError::ReservedField(name.to_string()));
    }
}

#[test]
fn every_event_of_the_event_vocabulary_round_trips() {
    // One value per event of doc/architecture/core-protocol.md ("Events"), plus
    // the `hello` handshake answer of the "Handshake" section.
    assert_frame_round_trips(Message::Outbound(Event::Hello("6f2c1a48e0b7d9c3".to_string(), "core-1".to_string(), 1)), "hello");
    assert_frame_round_trips(Message::Outbound(Event::Progress("import".to_string(), 12, 30, Some("bank_a.pdf".to_string()))), "progress with file");
    assert_frame_round_trips(Message::Outbound(Event::Progress("import".to_string(), 0, 0, None)), "progress without file");
    assert_frame_round_trips(Message::Outbound(Event::Log("info".to_string(), "probe: bank_a importer supported".to_string())), "log");
    assert_frame_round_trips(Message::Outbound(Event::Cancelled(Some("import".to_string()))), "cancelled");
    assert_frame_round_trips(Message::Outbound(Event::Cancelled(None)), "cancelled without a command name");
    assert_frame_round_trips(Message::Outbound(Event::Error(Some("import".to_string()), crate::event::ErrorReason::UnknownCommand, "unknown command: frob".to_string())), "error");
    assert_frame_round_trips(Message::Outbound(Event::Error(None, crate::event::ErrorReason::Io, "cannot open transactions.csv".to_string())), "error without a command name");
    assert_frame_round_trips(Message::Outbound(Event::Error(Some("import".to_string()), crate::event::ErrorReason::Other("plugin-versions-mismatch".to_string()), "newer core".to_string())), "error with a code the doc does not name");
    let mut payload = serde_json::Map::<String, serde_json::Value>::new();
    payload.insert("affected".to_string(), serde_json::to_value(183).unwrap());
    assert_frame_round_trips(Message::Outbound(Event::Result("set-category".to_string(), payload)), "result with a payload");
}

#[test]
fn every_test_example_of_the_core_protocol_doc_decodes_and_round_trips() {
    // Definition of Done (doc/implementation/plan.md): "Every protocol and file
    // format in doc/architecture/ matches the code (checked by the doc-reading
    // tests of 3.1 …)". The doc is read here, at test time, so a protocol change
    // that adds or edits a `test-example` marker is caught by this test.
    let text = read_core_protocol_doc();
    let (ids, examples) = doc_examples(&text);
    if examples.len() == 0 {
        panic!("no test-example marker was found in doc/architecture/core-protocol.md: the codec has nothing to be checked against, so either the doc lost its markers or this test reads the wrong file");
    }
    if ids.len() != examples.len() {
        panic!("the marker scan paired an uneven number of ids and examples in doc/architecture/core-protocol.md");
    }
    // Cross-check the scan against a second, independent count of the markers,
    // so the test cannot pass by matching nothing (or by matching one example
    // twice) and silently checking less than the doc marks.
    let (doc_lines, _doc_tail) = split_lines(&text);
    let mut markers = 0;
    for line in doc_lines {
        if line.contains("<!-- test-example id=") { markers = markers + 1; }
    }
    let collected = examples.len();
    if collected != markers {
        panic!("the scan collected {collected} examples for {markers} test-example markers in doc/architecture/core-protocol.md, so one is missing or double counted");
    }
    let mut saw_handshake_request = false;
    let mut saw_handshake_response = false;
    for index in 0..examples.len() {
        let id = ids[index].to_string();
        let example = examples[index].to_string();
        let decoded = decode_line(&example);
        match decoded {
            Err(reason) => {
                let why = error_text(&reason);
                panic!("the doc example {id} does not decode ({why}): [{example}]");
            }
            Ok(message) => {
                // Re-encode, then read the frame again: the value read back
                // must equal the value decoded from the doc's own line.
                let frame = encode_frame(&message).unwrap();
                match decode_line(&frame) {
                    Err(reason) => {
                        let why = error_text(&reason);
                        panic!("the frame written for the doc example {id} does not read back ({why}): [{frame}]");
                    }
                    Ok(back) => {
                        if !(back == message) {
                            panic!("the doc example {id} round-trips to a different message: wrote [{frame}]");
                        }
                    }
                }
                if id == "core-handshake-request" { saw_handshake_request = true; }
                if id == "core-handshake-response" { saw_handshake_response = true; }
            }
        }
    }
    // The scan must have found the handshake pair; otherwise it silently matched
    // something else and the codec would be unchecked.
    if !saw_handshake_request || !saw_handshake_response {
        panic!("the marker scan did not find the core-handshake-request and core-handshake-response examples of doc/architecture/core-protocol.md");
    }
}

#[test]
fn confirm_annotation_takes_either_id_or_reject_and_never_both() {
    // The command table of doc/architecture/core-protocol.md writes the
    // parameters of `confirm-annotation` as `id` | `reject`: the two are
    // alternatives, so a line carrying both is answered with the typed error
    // naming both instead of one form silently dropping the parameter of the
    // other (a silently dropped `id` would also be lost again on re-encode),
    // and a line carrying neither is still an error, not a default.
    let both = decode_line("{\"v\":1,\"cmd\":\"confirm-annotation\",\"annotation\":\"ann00000001\",\"id\":\"3f9a1c7e\",\"reject\":true}");
    assert!(both.unwrap_err() == CodecError::ConflictingFields("id".to_string(), "reject".to_string()));
    let why = error_text(&CodecError::ConflictingFields("id".to_string(), "reject".to_string()));
    assert!(why == "parameters are mutually exclusive: id and reject");
    let neither = decode_line("{\"v\":1,\"cmd\":\"confirm-annotation\",\"annotation\":\"ann00000002\"}");
    assert!(neither.unwrap_err() == CodecError::MissingField("id".to_string()));
    // Each alternative on its own still decodes; `reject` is the reject marker
    // only when it is the JSON value `true`, so `false` is the wrong type.
    let rejected = decode_line("{\"v\":1,\"cmd\":\"confirm-annotation\",\"annotation\":\"ann00000003\",\"reject\":true}");
    assert!(rejected.unwrap() == Message::Inbound(Command::ConfirmAnnotation("ann00000003".to_string(), crate::command::ConfirmTarget::Reject)));
    let accepted = decode_line("{\"v\":1,\"cmd\":\"confirm-annotation\",\"annotation\":\"ann00000004\",\"id\":\"3f9a1c7e\"}");
    assert!(accepted.unwrap() == Message::Inbound(Command::ConfirmAnnotation("ann00000004".to_string(), crate::command::ConfirmTarget::Id("3f9a1c7e".to_string()))));
    let wrong = decode_line("{\"v\":1,\"cmd\":\"confirm-annotation\",\"annotation\":\"ann00000005\",\"reject\":false}");
    assert!(wrong.unwrap_err() == CodecError::WrongType("reject".to_string()));
    // The written form of each alternative is unchanged, so the two errors do
    // not cost the vocabulary its round trip.
    assert_frame_round_trips(Message::Inbound(Command::ConfirmAnnotation("ann00000006".to_string(), crate::command::ConfirmTarget::Reject)), "confirm-annotation rejected");
    assert_frame_round_trips(Message::Inbound(Command::ConfirmAnnotation("ann00000007".to_string(), crate::command::ConfirmTarget::Id("3f9a1c7e".to_string()))), "confirm-annotation accepted");
}
