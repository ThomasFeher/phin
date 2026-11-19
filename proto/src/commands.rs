//! The command vocabulary of the core protocol as one data-driven table.
//!
//! `doc/architecture/core-protocol.md` ("Versioning") says the vocabulary is
//! data driven: "command names and their parameters are described in a table that
//! both sides read, so adding a command does not require changes in the
//! interface layer", and the "All known commands" table of that doc IS that
//! table — the interface doc is the source of truth, and this module is the
//! code-side copy of it. One row per documented command: the `cmd` name, its
//! parameters as the JSON fields of the line (name, wire type, required or
//! optional), and the pair of parameters of which exactly one may be present
//! (`confirm-annotation`'s `id` | `reject`, which the doc writes as
//! alternatives, so neither may be silently dropped).
//!
//! [`command_rows`] is the table, seeded with exactly the eight commands the
//! doc lists and nothing else; [`command_names`] and [`parameter_names`] hand
//! out what a row holds, which is what the tests of plan item 3.2 compare
//! against the doc. [`check_parameters`] is the vocabulary check of one command
//! line, and it runs on the way in: the line an interface sends on its way in
//! (through [`crate::codec::decode_line`], which reaches it via
//! [`crate::command::command_from_fields`]). The way out is held to the same
//! table without calling this function -- no production emit path does yet:
//! [`crate::command::command_fields`] can only write the names a row holds, so
//! it cannot invent a command the interface cannot pronounce (plan items
//! 3.3-3.6 are the consumers that will check parameters on their way out). A parameter the row does not name is ignored, which is what lets
//! an older core read a line written by a newer interface; a required parameter
//! that is missing is [`crate::codec::CodecError.MissingField`] naming that
//! parameter, one of the wrong JSON type is
//! [`crate::codec::CodecError.WrongType`], a command name the table does not
//! contain is [`crate::codec::CodecError.UnknownCommand`], and the two
//! alternatives of a row are never both present
//! ([`crate::codec::CodecError.ConflictingFields`]).
//!
//! [`take_fields`] reads the parameters of a row out of an object addressed by
//! command name, [`text_value`], [`optional_int_value`] and [`take_alternative`]
//! read a value of that row back as the value the row's [`ValueKind`] declares
//! for it, and [`put_fields`] writes values back out under the names of that
//! row; all of them find the row by its own name, so a row appended to the table
//! validates, takes and puts with no change anywhere else in `proto` or in the
//! core — the property the tests of this module check.
//! [`crate::command::Command`] is a second, typed view of the same vocabulary,
//! which is why [`row_name`] and [`position_of`] relate a typed command to a row
//! by position in the table, why neither the command names nor the parameter
//! names of the vocabulary are written anywhere but in this module, and why
//! [`every_typed_command_writes_the_parameters_of_the_row_it_names`] checks
//! every row of the table against the typed view it stands for, in both
//! directions — a position of one view drifting onto another row would write a
//! command name its values do not belong to, and read back as another command.
//!
//! [`PROTOCOL_VERSION`] lives here, with the vocabulary it belongs to
//! ("Versioning": every line carries the version it was written with, which is
//! the version this build speaks); `codec` re-exports the name as
//! [`crate::codec::PROTOCOL_VERSION`] so the version field of a line and the
//! vocabulary it carries cannot disagree. [`reserved_field`] names the fields
//! the framing of a line writes for itself (`v`, and the discriminators `event`
//! and `cmd`), which no parameter of the table and no payload key of a `result`
//! event may use, because one frame is one JSON object and carries each of
//! them exactly once.
//!
//! There is no I/O here: the table matches values against names, and moving
//! bytes to and from a stream is the transport of plan item 3.6 in the core.

use serde_json::Map;
use serde_json::Value;

use crate::codec::CodecError;

/// The protocol version this build of `proto` writes and the highest version
/// it accepts on a line ("Versioning": a line whose `v` is higher than this
/// build supports is rejected, an absent `v` means this version, and the
/// handshake compares versions before any command is sent).
///
/// This is the constant plan item 3.5 expects here as the protocol version of
/// `proto`; `codec` keeps the name [`crate::codec::PROTOCOL_VERSION`] as a
/// re-export of this value, so the version field of a line and the vocabulary
/// it carries cannot disagree.
pub const PROTOCOL_VERSION: i64 = 1;

/// The field every line of the stream carries: the protocol version.
pub const PROTOCOL_FIELD: &str = "v";

/// The discriminator field of an event line.
pub const EVENT_FIELD: &str = "event";

/// The discriminator field of a command line, which
/// `doc/architecture/core-protocol.md` also puts inside the `progress`, `result`,
/// `error` and `cancelled` events to name the command the event is about.
pub const COMMAND_FIELD: &str = "cmd";

/// The JSON type one parameter value has on the wire.
#[derive(Debug, Eq, PartialEq, Clone)]
pub enum ValueKind {
    /// A JSON string, the type of every parameter of the documented table except
    /// `report`'s `year` and `confirm-annotation`'s `reject`.
    Text,
    /// A JSON integer (`year` of `report`); a float, a string or `null` is the
    /// wrong type, because the doc writes protocol numbers as integers.
    Integer,
    /// The JSON value `true`: the `reject` marker of `confirm-annotation` counts
    /// as the reject form only when it is `true`, so `false` is a wrong type
    /// rather than a silently accepted value.
    RejectMarker,
}

/// One parameter of one command: the name of the JSON field on the line, the
/// type of its value, and whether a command line is invalid without it.
#[derive(Debug, Eq, PartialEq, Clone)]
pub struct Parameter {
    /// Name of the field, exactly as the doc's command table writes it.
    pub name: String,
    /// Type the value must have.
    pub kind: ValueKind,
    /// Whether the parameter must be present. The members of the `either` pair
    /// of a row are optional here, because the pair decides together which one
    /// of the two must be present (see [`CommandSpec.either`]).
    pub required: bool,
}

/// One row of the command table: a command name, its parameters in the order
/// the doc's table lists them, and at most one pair of alternatives.
#[derive(Debug, Eq, PartialEq, Clone)]
pub struct CommandSpec {
    /// The `cmd` name of the command, the discriminator field of the line.
    pub name: String,
    /// The parameters of the command in the order of the doc's table; empty for
    /// the commands the doc writes with `–`, which take no parameter.
    pub parameters: Vec::<Parameter>,
    /// The two parameters of which exactly one must be present, as the doc
    /// writes them: `confirm-annotation` takes `id` | `reject`, so a line
    /// carrying both is [`crate::codec::CodecError.ConflictingFields`] (one of
    /// them would silently win) and a line carrying neither is
    /// [`crate::codec::CodecError.MissingField`] naming the parameter the doc
    /// lists first. [`None`] when the command has no alternative.
    pub either: Option::<(String, String)>,
}

/// The command table: one row per command of the "All known commands" table of
/// `doc/architecture/core-protocol.md` ("Commands"), in the doc's order, and
/// nothing else (plan item 3.2: seeded with exactly the documented commands).
///
/// A row is pure data: appending one to the list this function returns makes
/// that command checkable, readable and writable by name with no other change
/// in `proto` or in the core, which is what the tests of this module check.
/// Appending a row does NOT reach the typed [`crate::command::Command`] view of
/// the vocabulary, which is a separate description of the same table (see
/// [`position_of`] and the note in `proto/src/command.rs`).
pub fn command_rows() -> Vec::<CommandSpec> {
    return [row("version", [parameter("nonce", ValueKind::Text, true)].to_vec(), None), row("import", [parameter("path", ValueKind::Text, true)].to_vec(), None), row("report", [parameter("format", ValueKind::Text, true), parameter("year", ValueKind::Integer, false)].to_vec(), None), row("set-category", [parameter("id", ValueKind::Text, true), parameter("category", ValueKind::Text, true), parameter("reason", ValueKind::Text, true)].to_vec(), None), row("confirm-annotation", [parameter("annotation", ValueKind::Text, true), parameter("id", ValueKind::Text, false), parameter("reject", ValueKind::RejectMarker, false)].to_vec(), Some(("id".to_string(), "reject".to_string()))), row("list-pending", Vec::<Parameter>::new(), None), row("cancel", Vec::<Parameter>::new(), None), row("shutdown", Vec::<Parameter>::new(), None)].to_vec();
}

/// One row of the command table: its `cmd` name, its parameters in the order
/// the doc's table lists them, and its pair of alternatives, [`None`] when the
/// command has none.
fn row(name: &str, parameters: Vec::<Parameter>, either: Option::<(String, String)>) -> CommandSpec {
    return CommandSpec { name: name.to_string(), parameters: parameters, either: either };
}

/// One parameter of a row: its name as the doc's table writes it, the wire type
/// the doc gives it ([`ValueKind`]), and whether a command line is invalid
/// without it (the members of an `either` pair are optional one by one, because
/// the pair as a whole is what must be present — see [`CommandSpec.either`]).
fn parameter(name: &str, kind: ValueKind, required: bool) -> Parameter {
    return Parameter { name: name.to_string(), kind: kind, required: required };
}

/// The `cmd` name of every row of `rows`, in table order.
pub fn command_names(rows: &Vec::<CommandSpec>) -> Vec::<String> {
    return rows.iter().map(|held| -> String { return held.name.to_string(); }).collect();
}

/// The parameter names of the row `row`, in the order of the row.
pub fn parameter_names(row: &CommandSpec) -> Vec::<String> {
    return row.parameters.iter().map(|held| -> String { return held.name.to_string(); }).collect();
}



/// The position in the command table of the row named `name`, [`None`] when the
/// table holds no such row.
///
/// The table finds the position from its own rows, so a caller that addresses a
/// row by position cannot pair a line with another row's parameters, and a row
/// appended to the table is addressable by name and by position with no other
/// change.
pub fn position_of(rows: &Vec::<CommandSpec>, name: String) -> Option::<usize> {
    return rows.iter().position(|held| -> bool { return held.name == name; });
}

/// The `cmd` name of the row of the command table at `position`.
///
/// [`String::new`] when the table holds no row at that position: a typed
/// command with no row in the vocabulary then has no name, and
/// [`crate::codec::encode_frame`] writes no `cmd` field for it, so the line
/// [`crate::codec::decode_line`] reads is refused as
/// [`crate::codec::CodecError.MissingDiscriminator`] rather than answering for
/// a command nobody named.
pub fn row_name(rows: &Vec::<CommandSpec>, position: usize) -> String {
    if let Some(row) = rows.get(position) { return row.name.to_string(); }
    return String::new();
}

/// Check the parameter fields of one command line against the row `row`.
///
/// A parameter the line carries must have the wire type the row declares, a
/// required parameter the line does not carry is an error naming that parameter,
/// and the `either` pair of the row must be present exactly once: both present
/// is [`crate::codec::CodecError.ConflictingFields`] and neither present is
/// [`crate::codec::CodecError.MissingField`] naming the parameter the doc lists
/// first. Fields the row does not name are ignored, so a line written by a
/// newer interface is still readable.
///
/// What is guaranteed today is the way in: the parameters of a line an
/// interface sends are validated against the row. The way out is covered too,
/// but NOT by a call to this function — no production emit path calls it yet:
/// [`put_fields`] writes parameters under the names of the row and nothing else,
/// so a writer cannot invent a name outside the table, and the tests of
/// [`put_fields`] pin that. This function stays public because plan items
/// 3.3-3.6 (the event writer and the interpreter) are its other consumers and
/// will call it for parameters on their way out. Takes `fields` by reference,
/// so a caller can read the values out of the same object afterwards (see
/// [`take_fields`]).
pub fn check_parameters(row: &CommandSpec, fields: &Map::<String, Value>) -> Result::<(), CodecError> {
    for parameter in &row.parameters {
        match fields.get(&parameter.name) {
            Some(held) => { if !value_matches(parameter.kind.clone(), held) { return Err(CodecError::WrongType(parameter.name.to_string())); } }
            None => { if parameter.required { return Err(CodecError::MissingField(parameter.name.to_string())); } }
        }
    }
    if let Some(pair) = &row.either {
        if fields.contains_key(&pair.0) && fields.contains_key(&pair.1) { return Err(CodecError::ConflictingFields(pair.0.to_string(), pair.1.to_string())); }
        if !fields.contains_key(&pair.0) && !fields.contains_key(&pair.1) { return Err(CodecError::MissingField(pair.0.to_string())); }
    }
    return Ok(());
}

/// The parameters of the row of the command table named `name`, taken out of
/// `fields` in the order of that row, as one value per parameter.
///
/// [`check_parameters`] runs first, so every value taken has already been
/// checked against the row, and every field the row does not name is left in
/// `fields` (unknown fields are ignored, not read). A parameter the line does
/// not carry is read as `Value::Null`, the absent marker [`put_fields`] also
/// skips: the doc's vocabulary has no parameter whose value may be JSON `null`
/// (see [`ValueKind`]), so `null` among the values read can only ever mean
/// "this line does not carry this parameter", which is what lets the typed
/// read of [`crate::command`] tell an absent optional parameter from a value
/// without inventing a type of its own. A `name` the table does not contain is
/// [`crate::codec::CodecError.UnknownCommand`].
pub fn take_fields(rows: &Vec::<CommandSpec>, fields: &mut Map::<String, Value>, name: String) -> Result::<Vec::<Value>, CodecError> {
    let row = row_named(&rows, name.to_string()).ok_or(CodecError::UnknownCommand(name.to_string()))?;
    check_parameters(&row, fields)?;
    let mut taken = Vec::<Value>::new();
    for parameter in &row.parameters {
        match fields.remove(&parameter.name) {
            Some(held) => { taken.push(held); }
            None => { taken.push(Value::Null); }
        }
    }
    return Ok(taken);
}

/// Write the parameter values `values` of the command named `name` into
/// `fields`, under the parameter names of that row of the command table: `values[i]`
/// belongs to `parameters[i]`, both in the order of the row, and a value of
/// `Value::Null` leaves the parameter out of the line, which is how an optional
/// parameter without a value and an alternative that was not taken are written
/// — a `report` frame without a year has no `year` field at all, and a rejected
/// annotation has no `id` field. Values for a `name` the table does not contain
/// are not written at all: the table names the fields of a frame, so writing a
/// frame invents no field of its own.
pub fn put_fields(fields: &mut Map::<String, Value>, rows: &Vec::<CommandSpec>, name: String, values: &Vec::<Value>) {
    if let Some(row) = row_named(&rows, name.to_string()) {
        for held in row.parameters.iter().enumerate() {
            if let Some(carried) = values.get(held.0) {
                if *carried != Value::Null { fields.insert(held.1.name.to_string(), carried.clone()); }
            }
        }
    }
}

/// The wire value of an optional parameter of a command: the JSON value it
/// holds, or [`Value::Null`], the absent marker [`put_fields`] leaves out of the
/// frame. `serde_json`'s `Number::from` is total for an `i64` (decision 14 of
/// the plan item), so writing a number cannot fail and nothing here can fail
/// either.
pub fn optional_number(value: Option::<i64>) -> Value {
    match value {
        Some(given) => { return Value::Number(serde_json::Number::from(given)); }
        None => { return Value::Null; }
    }
}

/// Which alternative of the `either` pair of the command named `name` the values
/// `taken` (taken by [`take_fields`]) carry: the name of the parameter of the
/// pair the line carries, the value it holds, and the [`ValueKind`] the row
/// declares for it, which is what tells the reject marker of
/// `confirm-annotation` from the `id` the user accepts without the caller
/// having to know either name.
///
/// A row with no pair, or a line that carries neither of the two, is the typed
/// error [`crate::codec::CodecError.MissingField`] naming the parameter the doc
/// lists first — the same error [`check_parameters`] has already raised for
/// such a line, so [`crate::command`] cannot reach it through a line a peer
/// sends, only through a row and a typed command disagreeing.
pub fn take_alternative(rows: &Vec::<CommandSpec>, taken: &Vec::<Value>, name: String) -> Result::<(String, Value, ValueKind), CodecError> {
    let row = row_named(&rows, name.to_string()).ok_or(CodecError::UnknownCommand(name.to_string()))?;
    let pair = row.either.ok_or(CodecError::MissingField("parameter".to_string()))?;
    for held in row.parameters.iter().enumerate() {
        if held.1.name != pair.0 && held.1.name != pair.1 { continue; }
        if let Some(carried) = taken.get(held.0) {
            if *carried != Value::Null { return Ok((held.1.name.to_string(), carried.clone(), held.1.kind.clone())); }
        }
    }
    return Err(CodecError::MissingField(pair.0.to_string()));
}

/// The name the vocabulary gives the parameter at `index` of the command named
/// `name`, or `"parameter"` when the table names no such parameter (the table
/// names only its own parameters, so an unexpected position is reported as a
/// nameless parameter rather than as a name the table never writes).
///
/// Used by the read side of the table ([`text_value`] and [`optional_int_value`])
/// to name the parameter a value is missing from or wrongly typed in the message
/// it answers with, so the read path never writes a parameter name of its own.
fn parameter_name(rows: &Vec::<CommandSpec>, name: String, index: usize) -> String {
    for row in rows {
        if row.name != name { continue; }
        match row.parameters.get(index) {
            Some(parameter) => { return parameter.name.to_string(); }
            None => { return "parameter".to_string(); }
        }
    }
    return "parameter".to_string();
}

/// The text value of the parameter at `index` of the command named `name`,
/// among the values [`take_fields`] took: a parameter the line does not carry
/// (`Value::Null`, the absent marker, or no value at that position) is
/// [`crate::codec::CodecError.MissingField`] and a value that is not a JSON
/// string is [`crate::codec::CodecError.WrongType`], each naming the parameter
/// the row holds at that position through [`parameter_name`].
///
/// [`check_parameters`] has already refused such a line, so these two errors
/// mean a row of the table and the typed view of [`crate::command`] disagree;
/// a peer cannot provoke them. This is the read side of the table, which is
/// why the typed read of [`crate::command`] holds no field extractor of its own.
pub fn text_value(rows: &Vec::<CommandSpec>, name: String, taken: &Vec::<Value>, index: usize) -> Result::<String, CodecError> {
    let named = parameter_name(&rows, name, index);
    let held = taken.get(index).ok_or(CodecError::MissingField(named.to_string()))?;
    if *held == Value::Null { return Err(CodecError::MissingField(named.to_string())); }
    return Ok(held.as_str().ok_or(CodecError::WrongType(named.to_string()))?.to_string());
}

/// The integer value of the optional parameter at `index` of the command named
/// `name` among the values [`take_fields`] took — the [`ValueKind.Integer`]
/// parameter `year` of `report`: [`None`] when the line does not carry it, the
/// JSON integer the row declares for it when it does, and a value that is not an
/// integer is [`crate::codec::CodecError.WrongType`] naming the parameter the
/// row holds at that position.
pub fn optional_int_value(rows: &Vec::<CommandSpec>, name: String, taken: &Vec::<Value>, index: usize) -> Result::<Option::<i64>, CodecError> {
    let named = parameter_name(&rows, name, index);
    if let Some(held) = taken.get(index) {
        if *held != Value::Null { return Ok(Some(held.as_i64().ok_or(CodecError::WrongType(named.to_string()))?)); }
    }
    return Ok(None);
}

/// The first field of `fields` whose name the framing of a line writes for
/// itself ([`PROTOCOL_FIELD`], the version of every line, and the two
/// discriminators [`EVENT_FIELD`] and [`COMMAND_FIELD`]), [`None`] when `fields`
/// uses none of them.
///
/// A frame is one JSON object, so it can carry each of these names once: a
/// parameter of the command table or a payload key of a `result` event named
/// like one of them would either be read back as framing instead of as data or
/// be overwritten by the framing of the frame — and since the payload of a
/// `result` event is written as the remaining fields of the line, a `cmd` key
/// among them would overwrite the name of the command the result belongs to.
/// `codec`'s encoder refuses such a message with
/// [`crate::codec::CodecError.ReservedField`] instead of writing a frame that
/// cannot be read back, so nothing is silently dropped.
pub fn reserved_field(fields: &Map::<String, Value>) -> Option::<String> {
    for name in [PROTOCOL_FIELD, EVENT_FIELD, COMMAND_FIELD].to_vec() {
        if fields.contains_key(name) { return Some(name.to_string()); }
    }
    return None;
}

/// The row of the command table named `name`, [`None`] when the table holds no
/// such row. The one lookup of the table by name; every other function of this
/// module goes through it.
fn row_named(rows: &Vec::<CommandSpec>, name: String) -> Option::<CommandSpec> {
    return rows.iter().find(|held| -> bool { return held.name == name; }).cloned();
}

/// The JSON value `held` actually has, checked against the [`ValueKind`] the
/// row declares for that parameter.
fn value_matches(kind: ValueKind, held: &Value) -> bool {
    match kind {
        ValueKind::Text => { return held.as_str().is_some(); }
        ValueKind::Integer => { return held.as_i64().is_some(); }
        ValueKind::RejectMarker => { return *held == Value::Bool(true); }
    }
}

// ============================================================================
// Tests
// ============================================================================

/// The text of `doc/architecture/core-protocol.md`, read AT TEST TIME.
///
/// The interface doc is the source of the command table, so a test that
/// hard-coded the names could stay green while the doc and the table disagree.
/// The path is resolved from the `proto` package directory (where the test
/// binary runs, hence `../doc/…`) and from the workspace root (`doc/…`),
/// whichever of the two the binary happens to be started from (as in
/// `proto/src/codec.rs`).
#[cfg(test)]
fn read_core_protocol_doc() -> String {
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

/// The cells of one row of a markdown table, split on the ` | ` that separates
/// them. The command table of the doc escapes the bar of an alternative as
/// `\|`, which is NOT ` | `, so `confirm-annotation`'s "`id` \| `reject`" stays
/// ONE cell and reaches the parser as the text of that cell.
#[cfg(test)]
fn table_cells(line: &String) -> Vec::<String> {
    return line.to_string().replace(" | ", "\n").split('\n').map(|cell| -> String { return cell.to_string(); }).collect();
}

/// The backtick-quoted words of `text`, in the order they are written: the
/// names of the parameters of a cell of the command table, or the name of the
/// command itself.
#[cfg(test)]
fn quoted_words(text: &String) -> Vec::<String> {
    let mut words = Vec::<String>::new();
    let mut word = String::new();
    let mut inside = false;
    for character in text.to_string().chars() {
        if character == '`' {
            if inside {
                words.push(word);
                word = String::new();
            }
            inside = !inside;
            continue;
        }
        if inside {
            word.push(character);
        }
    }
    return words;
}

/// One row of the command table of `doc/architecture/core-protocol.md`
/// ("Commands"), read AT TEST TIME: the `cmd` name of the command, the names of
/// its parameters in the order the doc lists them, the names the doc marks
/// `optional`, and the two names the doc separates with an escaped bar (the
/// alternatives of the row, if it has any).
#[cfg(test)]
#[derive(Debug, Eq, PartialEq)]
struct DocCommandRow {
    pub name: String,
    pub parameters: Vec::<String>,
    pub optional: Vec::<String>,
    pub alternatives: Option::<(String, String)>,
}

/// The command table of the doc, as `Vec::<DocCommandRow>` in the order the doc
/// lists its commands, from the header row `| Command | Parameters | Meaning |`
/// to the last row that starts a table cell.
#[cfg(test)]
fn doc_command_table(text: &str) -> Vec::<DocCommandRow> {
    let mut rows = Vec::<DocCommandRow>::new();
    let (lines, _tail) = crate::codec::split_lines(&text);
    let mut reading = false;
    for line in lines {
        if !reading {
            if line.starts_with("| Command | Parameters |") {
                reading = true;
            }
            continue;
        }
        if line.contains("---") {
            continue;
        }
        if !line.starts_with("|") {
            break;
        }
        let cells = table_cells(&line);
        if cells.len() < 2 {
            continue;
        }
        let name = &cells[0];
        let given = &cells[1];
        let alternatives: Vec::<String> = given.split("\\|").map(|part| -> String { return part.to_string(); }).collect();
        let written = quoted_words(&alternatives[0]);
        let mut parameters = Vec::<String>::new();
        for word in &written {
            parameters.push(word.to_string());
        }
        let mut pair = None;
        if alternatives.len() > 1 {
            let tail = quoted_words(&alternatives[1]);
            // The parameters of the row continue past the escaped bar of the
            // alternative, so the words of the tail are parameters of the row
            // too, in the order the doc writes them.
            for word in &tail {
                parameters.push(word.to_string());
            }
            if written.len() > 0 && tail.len() > 0 {
                pair = Some((written[written.len() - 1].to_string(), tail[0].to_string()));
            }
        }
        let mut optional = Vec::<String>::new();
        for word in &parameters {
            let marked = format!("optional `{}`", word);
            if given.contains(&marked) {
                optional.push(word.to_string());
            }
        }
        let names = quoted_words(name);
        if names.len() == 0 {
            continue;
        }
        rows.push(DocCommandRow { name: names[0].to_string(), parameters: parameters, optional: optional, alternatives: pair });
    }
    return rows;
}

#[test]
fn command_table_holds_exactly_the_commands_of_the_documented_table() {
    // Plan item 3.2's table is seeded with "exactly the documented commands": the
    // doc's own table is the source of truth, so this test reads it AT TEST TIME
    // and compares every row of it with the row of [`command_rows`] at the same
    // position -- the `cmd` name, the parameter names in the order the doc lists
    // them, the optional parameters (the [`Parameter.required`] flags of the
    // row, read back through [`optional_parameter_names`], a helper of this
    // test), and the alternative pair of [`CommandSpec.either`]. A command
    // added to, renamed in, retyped or added to, renamed in, retyped or
    // dropped from either side fails here, naming the row that disagrees.
    let text = read_core_protocol_doc();
    let documented = doc_command_table(&text);
    let rows = command_rows();
    if documented.len() == 0 {
        panic!("the command table of doc/architecture/core-protocol.md was not read: the cross-check has nothing to compare against, so either the doc lost its table or this test reads the wrong file");
    }
    if documented.len() != rows.len() {
        let count_doc = documented.len();
        let count_table = rows.len();
        panic!("the doc lists {count_doc} commands and the command table of proto/src/commands.rs holds {count_table} rows, so one side drifted: the doc has {documented:?} and the table has {rows:?}");
    }
    let mut position = 0;
    for entry in &documented {
        let row = rows.get(position);
        if row == None {
            panic!("the command table holds no row at position {position} of the doc's table, which lists {entry:?}");
        }
        let spec = row.unwrap();
        if entry.name != spec.name {
            panic!("position {position} of the command table is named [{named}] while the doc's table names it [{documented_as}]", named = spec.name, documented_as = entry.name);
        }
        if entry.parameters != parameter_names(&spec) {
            panic!("the parameters of the command [{name}] are {doc_parameters:?} in the doc and {table_parameters:?} in the command table", name = entry.name, doc_parameters = entry.parameters, table_parameters = parameter_names(&spec));
        }
        let mut optional = entry.optional.clone();
        match &entry.alternatives {
            Some(pair) => {
                // The members of an alternative pair are optional one by one: the
                // pair as a whole is what a line must carry (see
                // [`CommandSpec.either`]), so the doc's `optional` entries plus
                // the two members of the pair are the optional parameters the
                // table holds.
                let (first, second) = pair;
                optional.push(first.to_string());
                optional.push(second.to_string());
            }
            None => { }
        }
        if optional != optional_parameter_names(&spec) {
            panic!("the optional parameters of the command [{name}] are {doc_optional:?} in the doc (its `optional` entries plus its alternative pair) and {table_optional:?} in the command table", name = entry.name, doc_optional = optional, table_optional = optional_parameter_names(&spec));
        }
        if entry.alternatives != spec.either {
            panic!("the command [{name}] takes {doc_pair:?} in the doc and {table_pair:?} in the command table, so the alternative pair drifted", name = entry.name, doc_pair = entry.alternatives, table_pair = spec.either);
        }
        position = position + 1;
    }
}

/// The names of the parameters of the row `row` a command line may leave out,
/// read back out of the row's own [`Parameter.required`] flags: the table stores
/// one `required` flag per parameter instead of a separate list of optional
/// names, so the test cross-checks the doc's `optional` markers against the flag
/// of the parameter itself (the two members of the row's `either` pair carry
/// `required = false` one by one because the pair as a whole is what must be
/// present — see [`CommandSpec.either`]).
#[cfg(test)]
fn optional_parameter_names(row: &CommandSpec) -> Vec::<String> {
    let mut names = Vec::<String>::new();
    for parameter in &row.parameters {
        if !parameter.required { names.push(parameter.name.to_string()); }
    }
    return names;
}

/// The row of a command the documented vocabulary does not have: `archive`,
/// taking the `path` of the file to archive. Written with the same private
/// constructors [`row`] and [`text`] that seed [`command_rows`], so the test of
/// the append property adds DATA and nothing else.
#[cfg(test)]
fn extra_row() -> CommandSpec {
    return row("archive", [parameter("path", ValueKind::Text, true)].to_vec(), None);
}

#[test]
fn appending_a_row_to_the_command_table_validates_the_new_command_with_no_other_change() {
    // Plan item 3.2's success check: "test appends a new command to the table and
    // it validates with no other code change". What makes this check HONEST rather
    // than a tautology: the table is DATA ([`command_rows`] returns a
    // `Vec::<CommandSpec>`), every function that reads or writes the parameters
    // of a command takes its rows as a parameter, and the test hands those
    // functions the documented table PLUS one row it appends itself -- so a
    // command the production table does not know is validated, read and written
    // by the SAME production code path, with not one line of production code
    // changed, added or specialized for it. The appended command is NOT in
    // [`command_rows`], which the test asserts, so the row really is the only
    // thing that was added; and the test never touches the typed
    // [`crate::command::Command`] view, because a row of the table alone does
    // NOT reach that view (see the note on [`command_rows`] and in
    // `proto/src/command.rs`: the typed view is a second description of the
    // same vocabulary, and a new documented command still needs one there).
    let extra = extra_row();
    let documented_rows = command_rows();
    let mut rows = command_rows().to_vec();
    rows.push(extra.clone());
    assert!(documented_rows.len() == 8);
    assert!(rows.len() == 9);
    assert!(position_of(&rows, "archive".to_string()) != None);

    // A complete command line for the appended command validates against its row.
    let mut fields = Map::<String, Value>::new();
    fields.insert("path".to_string(), Value::String("/data/2024/march.pdf".to_string()));
    assert!(check_parameters(&extra, &fields).is_ok());
    // The same table reads its parameters in the order of the row...
    let read = take_fields(&rows, &mut fields, "archive".to_string());
    assert!(read.is_ok());
    let taken = read.unwrap();
    assert!(taken.len() == 1);
    assert!(taken[0] == Value::String("/data/2024/march.pdf".to_string()));
    // ...and writes them back out under the names of that same row.
    let mut written = Map::<String, Value>::new();
    put_fields(&mut written, &rows, "archive".to_string(), &taken);
    assert!(written.contains_key("path"));
    assert!(written.len() == 1);
    // The position and the name of the new row come from the table, not from a
    // caller that knows the vocabulary by heart.
    assert!(position_of(&rows, "archive".to_string()).unwrap() == 8);
    assert!(row_name(&rows, 8) == "archive".to_string());
    assert!(parameter_names(&extra) == ["path".to_string()].to_vec());
    assert!(optional_parameter_names(&extra) == Vec::<String>::new());
    // A line for the appended command that leaves the required parameter out is
    // the typed error naming that parameter -- still with no other change.
    let mut nothing = Map::<String, Value>::new();
    assert!(take_fields(&rows, &mut nothing, "archive".to_string()).unwrap_err() == CodecError::MissingField("path".to_string()));
    // A line carrying a parameter the appended row does not declare is ignored,
    // exactly as it is for a documented command.
    let mut more = Map::<String, Value>::new();
    more.insert("path".to_string(), Value::String("/x".to_string()));
    more.insert("until".to_string(), Value::String("2025".to_string()));
    assert!(check_parameters(&extra, &more).is_ok());
    assert!(take_fields(&rows, &mut more, "archive".to_string()).unwrap().len() == 1);
    assert!(more.contains_key("until"));
    // The typed [`crate::command::Command`] view of the vocabulary is the SECOND
    // description this check does NOT reach: reading the very same line for
    // the appended command back through the production codec is still
    // [`crate::codec::CodecError.UnknownCommand`], so a row of the table alone
    // validates a command without inventing it in the typed view.
    let archived = crate::codec::decode_line("{\"v\":1,\"cmd\":\"archive\",\"path\":\"/x\"}");
    assert!(archived.unwrap_err() == CodecError::UnknownCommand("archive".to_string()));
}

#[test]
fn a_missing_required_parameter_is_the_typed_error_naming_that_parameter() {
    // The vocabulary is what reports an incomplete command line, so the name of
    // the parameter that is missing comes out of the table, for every kind of
    // parameter the table declares (plan item 3.2's check: at least two commands
    // plus an optional parameter that must NOT be an error):
    let rows = command_rows();
    let mut nothing = Map::<String, Value>::new();
    // `import` without the `path` of the file to import.
    assert!(take_fields(&rows, &mut nothing, "import".to_string()).unwrap_err() == CodecError::MissingField("path".to_string()));
    // `version` without the `nonce` the doc requires of it.
    let mut version_fields = Map::<String, Value>::new();
    version_fields.insert("id".to_string(), Value::String("c1".to_string()));
    assert!(take_fields(&rows, &mut version_fields, "version".to_string()).unwrap_err() == CodecError::MissingField("nonce".to_string()));
    // `set-category` with the annotation and the reason but without the
    // `category` the annotation gets.
    let mut category_fields = Map::<String, Value>::new();
    category_fields.insert("id".to_string(), Value::String("a1".to_string()));
    category_fields.insert("reason".to_string(), Value::String("missed plate".to_string()));
    assert!(take_fields(&rows, &mut category_fields, "set-category".to_string()).unwrap_err() == CodecError::MissingField("category".to_string()));
    // `report` without the `format` to report in.
    let mut year_fields = Map::<String, Value>::new();
    year_fields.insert("year".to_string(), Value::Number(serde_json::Number::from(-2025)));
    assert!(take_fields(&rows, &mut year_fields, "report".to_string()).unwrap_err() == CodecError::MissingField("format".to_string()));
    // `confirm-annotation` with the annotation but without either of the
    // alternatives of [`CommandSpec.either`]: the error names the first one the
    // doc lists, so the interface is told what to send.
    let mut target_fields = Map::<String, Value>::new();
    target_fields.insert("annotation".to_string(), Value::String("a1".to_string()));
    assert!(check_parameters(&confirm_annotation_row(&rows), &target_fields).unwrap_err() == CodecError::MissingField(confirm_annotation_row(&rows).either.unwrap().0.to_string()));
    // `cancel` takes no parameter at all (the doc lists its Parameters as `–`),
    // so a line that carries nothing beyond the framing is complete: the table
    // holds an empty parameter list for it and requires nothing of it.
    let mut cancel_fields = Map::<String, Value>::new();
    cancel_fields.insert("id".to_string(), Value::String("c9".to_string()));
    assert!(take_fields(&rows, &mut cancel_fields, "cancel".to_string()).unwrap() == Vec::<Value>::new());
    assert!(cancel_fields.contains_key("id"));
    assert!(parameter_names(&cancel_row(&rows)) == Vec::<String>::new());
    // A parameter name that is not a parameter of the row is never reported as
    // missing: the doc allows extra fields, so only the parameters the row
    // declares are required of it.
    let mut extra_fields = Map::<String, Value>::new();
    extra_fields.insert("path".to_string(), Value::String("/x".to_string()));
    extra_fields.insert("unknown".to_string(), Value::Number(serde_json::Number::from(1)));
    assert!(check_parameters(&import_row(&rows), &extra_fields).is_ok());
    // An ordinary parameter present but holding the WRONG JSON type is the
    // typed error naming that parameter, for a required and for an optional
    // one alike: `import` whose `path` is the number 7 instead of a string, and
    // `report` whose optional `year` is the string "2024" instead of an integer
    // (`format` is carried, so the reported problem is the type and not an
    // absence the row would otherwise hit first).
    let mut path_as_number = Map::<String, Value>::new();
    path_as_number.insert("path".to_string(), Value::Number(serde_json::Number::from(7)));
    assert!(check_parameters(&import_row(&rows), &path_as_number).unwrap_err() == CodecError::WrongType("path".to_string()));
    let mut year_as_text = Map::<String, Value>::new();
    year_as_text.insert("format".to_string(), Value::String("json".to_string()));
    year_as_text.insert("year".to_string(), Value::String("2024".to_string()));
    assert!(check_parameters(&report_row(&rows), &year_as_text).unwrap_err() == CodecError::WrongType("year".to_string()));
}

/// The row of the command `confirm-annotation`, looked up in the table by name
/// (the test of the parameter checks above needs the row itself to call
/// [`check_parameters`] with, the way [`take_fields`] does).
#[cfg(test)]
fn confirm_annotation_row(rows: &Vec::<CommandSpec>) -> CommandSpec {
    return row_named(rows, "confirm-annotation".to_string()).unwrap();
}

/// The row of the command `report`, looked up in the table by name: the row of
/// the command the doc gives one optional parameter to.
#[cfg(test)]
fn report_row(rows: &Vec::<CommandSpec>) -> CommandSpec {
    return row_named(rows, "report".to_string()).unwrap();
}

/// The row of the command `cancel`, looked up in the table by name: the row of
/// a command the doc gives no parameters to.
#[cfg(test)]
fn cancel_row(rows: &Vec::<CommandSpec>) -> CommandSpec {
    return row_named(rows, "cancel".to_string()).unwrap();
}

/// The row of the command `import`, looked up in the table by name.
#[cfg(test)]
fn import_row(rows: &Vec::<CommandSpec>) -> CommandSpec {
    return row_named(rows, "import".to_string()).unwrap();
}

#[test]
fn an_optional_parameter_of_the_table_may_be_left_out_of_a_line() {
    // An alternative of [`CommandSpec.either`] and a parameter the doc marks
    // `optional` are the two ways a parameter of a row can be absent, and
    // plan item 3.2 requires that the optional one is NOT an error: `report`
    // asks for the `format` to report in and leaves the `year` to report on
    // out, so a line without `year` is complete and a line without `format`
    // is the typed error naming `format` (see the test above).
    let rows = command_rows();
    let mut fields = Map::<String, Value>::new();
    fields.insert("format".to_string(), Value::String("summary".to_string()));
    assert!(check_parameters(&report_row(&rows), &fields).is_ok());
    // The absent parameter is read as a value the codec does not write back
    // out, so a round trip keeps a line as sparse as the line was.
    let read = take_fields(&rows, &mut fields, "report".to_string());
    assert!(read.is_ok());
    let taken = read.unwrap();
    assert!(taken.len() == 2);
    assert!(taken[0] == Value::String("summary".to_string()));
    assert!(taken[1] == Value::Null);
    let mut written = Map::<String, Value>::new();
    put_fields(&mut written, &rows, "report".to_string(), &taken);
    assert!(written.contains_key("format"));
    assert!(!written.contains_key("year"));
    assert!(written.len() == 1);
    // With the year present the same row reads both parameters and writes both.
    let mut both = Map::<String, Value>::new();
    both.insert("format".to_string(), Value::String("csv".to_string()));
    both.insert("year".to_string(), Value::Number(serde_json::Number::from(2025)));
    let values = take_fields(&rows, &mut both, "report".to_string()).unwrap();
    assert!(values.len() == 2);
    put_fields(&mut written, &rows, "report".to_string(), &values);
    assert!(written.len() == 2);
    assert!(written.contains_key("year"));
    // The optional parameters of the row are the data of the table, not a
    // rule of the reader: the [`Parameter.required`] flag of the row says the
    // same for `report` as the doc does, and
    // [`command_table_holds_exactly_the_commands_of_the_documented_table`]
    // keeps that claim checked against the doc.
    assert!(optional_parameter_names(&report_row(&rows)) == ["year".to_string()].to_vec());
}

/// One value per row of the command table, written as the typed commands of
/// `proto/src/command.rs`: the Rust variant names are not wire vocabulary, so
/// writing them here adds no name the table does not already hold, and the
/// wire name each one is written under comes out of the table itself.
#[cfg(test)]
fn typed_view() -> Vec::<crate::command::Command> {
    let mut typed = Vec::<crate::command::Command>::new();
    typed.push(crate::command::Command::Version("6f2c1a48e0b7d9c3".to_string()));
    typed.push(crate::command::Command::Import("/data/statements/2024/bank_a.pdf".to_string()));
    typed.push(crate::command::Command::Report("html".to_string(), Some(2024)));
    typed.push(crate::command::Command::SetCategory("3f9a1c7e".to_string(), "food:supermarket".to_string(), "user".to_string()));
    typed.push(crate::command::Command::ConfirmAnnotation("ann00000001".to_string(), crate::command::ConfirmTarget::Id("3f9a1c7e".to_string())));
    typed.push(crate::command::Command::ConfirmAnnotation("ann00000002".to_string(), crate::command::ConfirmTarget::Reject));
    typed.push(crate::command::Command::ListPending);
    typed.push(crate::command::Command::Cancel);
    typed.push(crate::command::Command::Shutdown);
    return typed;
}

#[test]
fn every_typed_command_writes_the_parameters_of_the_row_it_names() {
    // [`crate::command`] relates a typed command to its row by POSITION, so
    // nothing but this test keeps the two views of the vocabulary keyed the
    // same way: for every typed command, the `cmd` name
    // [`crate::command::command_name`] takes out of the table, the fields
    // [`crate::command::command_fields`] writes, and the frame
    // [`crate::codec::encode_frame`] writes and [`crate::codec::decode_line`]
    // reads back must all belong to ONE row. A position of the typed view
    // drifting onto another row — the hazard a position-keyed typed view has
    // and a name-keyed one does not — is caught twice here: the fields written
    // would not be parameters of the row the command is named under, and the
    // frame would read back as another command. Cross-checking the table
    // against the doc (see the test above) cannot see that, because it leaves
    // the table itself unchanged.
    let rows = command_rows();
    let typed = typed_view();
    let mut names = Vec::<String>::new();
    for command in typed {
        let name = crate::command::command_name(&command);
        let place = position_of(&rows, name.to_string());
        if place == None {
            let carried = name.to_string();
            panic!("the typed view writes a command under [{carried}], which the command table holds no row for, so a position of the typed view stands for a row the vocabulary does not have");
        }
        let row = &rows.get(place.unwrap()).unwrap();
        names.push(name.to_string());
        let fields = crate::command::command_fields(&command);
        // No field the typed command writes may be outside the parameters of
        // the row it is named under...
        let declared = parameter_names(row);
        for key in fields.keys() {
            if !declared.contains(&key.to_string()) {
                let written_as = name.to_string();
                let carried = key.to_string();
                panic!("the typed command written under [{written_as}] writes the field [{carried}], which the row of [{written_as}] does not declare as a parameter, so that typed command stands for another row of the table");
            }
        }
        // ...and no required parameter of that row may be left out of it.
        for parameter in &row.parameters {
            if !parameter.required { continue; }
            if !fields.contains_key(&parameter.name) {
                let written_as = name.to_string();
                let carried = parameter.name.to_string();
                panic!("the typed command written under [{written_as}] leaves the required parameter [{carried}] of the row of [{written_as}] out, so that typed command stands for another row of the table");
            }
        }
        // And the frame of the command reads back as the same command, which is
        // what a row cannot survive being read as another command's value for.
        let message = crate::codec::Message::Inbound(command);
        let written = crate::codec::encode_frame(&message).unwrap();
        if crate::codec::decode_line(&written).unwrap() != message {
            panic!("the frame [{written}] the codec writes for the typed command named [{name}] reads back as a different command, so the row that typed command stands for is not the row its values belong to");
        }
    }
    // Every row of the table is named by at least one typed command, and no
    // typed command is written under a name outside the table: the distinct
    // names the typed view writes are exactly the names of the rows, as
    // [`command_names`] reads them out of the table. `confirm-annotation`
    // appears twice in [`typed_view`] because its row has two alternatives and
    // both must be checked against that one row, so the names are reduced to
    // the distinct ones before they are compared.
    let mut distinct = Vec::<String>::new();
    for held in names {
        if distinct.contains(&held.to_string()) { continue; }
        distinct.push(held.to_string());
    }
    distinct.sort();
    let mut documented = command_names(&rows);
    documented.sort();
    if documented != distinct {
        panic!("the rows of the command table are named {documented:?} while the typed view of proto/src/command.rs is written under {distinct:?}, so one row of the vocabulary has no typed command");
    }
}

/// The keys of the JSON object of one line of the doc, as the fields that line
/// carries. The framing fields `v`, `event` and `cmd` are left out: `proto/src/codec.rs`
/// reads them before the parameters of a message, and [`reserved_field`] reports
/// them when a caller tries to use one of them as a payload field.
#[cfg(test)]
fn object_fields(line: &str) -> Vec::<String> {
    let parsed = serde_json::from_str::<Value>(&line);
    if !parsed.is_ok() {
        panic!("cannot parse the doc line [{line}] as a JSON object, so the cross-check of the documented examples cannot name the fields the line carries");
    }
    let read = parsed.unwrap();
    let mut names = Vec::<String>::new();
    match read.as_object() {
        Some(object) => {
            for key in object.keys() {
                let name = key.to_string();
                if name == PROTOCOL_FIELD || name == EVENT_FIELD || name == COMMAND_FIELD {
                    continue;
                }
                names.push(name);
            }
        }
        None => {
            panic!("the doc line [{line}] is not a JSON object, so the cross-check of the documented examples has no fields to name");
        }
    }
    return names;
}

/// The id of a `<!-- test-example id=<id> -->` marker line of the doc: the
/// text between the `=` of the marker and the end of the id.
#[cfg(test)]
fn marker_id(line: &str) -> String {
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

/// The ids and the JSON lines of every `test-example` of the core protocol doc:
/// a marker comment is followed by its example inside a fence, so the scan
/// keeps the id of the last marker seen until the fenced line is reached, one
/// example per marker and no example without a marker (as the test helper of
/// `proto/src/codec.rs` does).
#[cfg(test)]
fn doc_test_examples(text: &str) -> (Vec::<String>, Vec::<String>) {
    let mut ids = Vec::<String>::new();
    let mut examples = Vec::<String>::new();
    let (lines, _tail) = crate::codec::split_lines(&text);
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

#[test]
fn every_documented_command_example_carries_the_parameters_of_its_table_row() {
    // The second half of the cross-check of this module: the doc shows the
    // commands of its vocabulary as `test-example` lines, and each of those
    // lines must carry the parameters the row of the table declares for its
    // command -- no field that the row does not declare, and no required
    // parameter of the row left out. If the doc renamed a parameter of a
    // command line without renaming it in the table, or the table renamed one
    // without the doc, this test names the command and the field that
    // disagree. The command is decoded with the codec, and its name is read
    // back through the typed view of `proto/src/command.rs`, so the claim
    // covers the wire form a caller actually sends, not a paraphrase of it.
    let text = read_core_protocol_doc();
    let rows = command_rows();
    let (ids, examples) = doc_test_examples(&text);
    // The scan above must not silently drop a test-example of the doc, so the
    // examples it found are cross-checked with the marker lines themselves,
    // exactly as the test of `proto/src/codec.rs` does.
    let (lines, _tail) = crate::codec::split_lines(&text);
    let mut markers = 0;
    for line in lines {
        if line.contains("<!-- test-example id=") {
            markers = markers + 1;
        }
    }
    let found = examples.len();
    if found != markers {
        panic!("the scan of the test-examples of the doc read {found} examples for {markers} test-example markers the doc holds, so one example is missing or double counted and the cross-check below would not see every documented command line");
    }
    let mut commands = 0;
    let mut position = 0;
    while position < ids.len() {
        let id = &ids[position].to_string();
        let example = &examples[position].to_string();
        position = position + 1;
        if !id.starts_with("core-command-") {
            continue;
        }
        commands = commands + 1;
        let decoded = crate::codec::decode_line(&example);
        if !decoded.is_ok() {
            let reason = crate::codec::error_text(&decoded.unwrap_err());
            panic!("the doc example [{id}] does not decode ({reason}): [{example}]");
        }
        let message = decoded.unwrap();
        match message {
            crate::codec::Message::Inbound(command) => {
                let name = crate::command::command_name(&command);
                let place = position_of(&rows, name.to_string());
                if place == None {
                    let carried = name.to_string();
                    panic!("the command line of the doc example [{id}] carries the command [{carried}], which the command table of proto/src/commands.rs holds no row for, so the row of the command the doc shows is missing from the table");
                }
                let row = &rows.get(place.unwrap()).unwrap();
                let given = object_fields(example);
                let declared = parameter_names(row);
                let names = format!("{:?}", declared);
                for field in &given {
                    if !declared.contains(&field.to_string()) {
                        let carried = field.to_string();
                        panic!("the doc example [{id}] carries the field [{carried}], which the command table does not declare as a parameter of the command [{name}]: the row declares {names}");
                    }
                }
                for parameter in &row.parameters {
                    if !parameter.required {
                        continue;
                    }
                    let required = parameter.name.to_string();
                    let mut carried_by_example = false;
                    for field in &given {
                        if field.to_string() == required {
                            carried_by_example = true;
                        }
                    }
                    if !carried_by_example {
                        panic!("the doc example [{id}] leaves the required parameter [{required}] of the command [{name}] out, so the doc and the table do not agree on what the command takes");
                    }
                }
            }
            _ => {
                panic!("the doc example [{id}] decoded to a message that is not an inbound command line, so the cross-check cannot compare its fields with the parameters of a row: [{example}]");
            }
        }
    }
    if commands == 0 {
        panic!("the doc holds no test-example of an inbound command line, so the cross-check of the documented command lines with the command table has nothing to compare: the doc lost its examples of commands or renamed their ids");
    }
}

#[test]
fn the_protocol_version_is_one_constant_read_by_the_codec_and_the_vocabulary() {
    // Plan item 3.2 moves the version constant into this module, next to the
    // vocabulary whose lines carry it, and `proto/src/codec.rs` re-exports it
    // as `crate::codec::PROTOCOL_VERSION`: the version the codec writes as
    // [`PROTOCOL_FIELD`] of every frame and the version a line must carry are
    // therefore ONE number that cannot drift, and plan item 3.5 exposes this
    // constant as the protocol version of `proto`. Decision 14 of
    // `02_impl.md`: an integer, so the comparison `codec` makes needs no cast.
    assert!(crate::codec::PROTOCOL_VERSION == PROTOCOL_VERSION);
    // The doc, "Versioning": a line without `v` means version 1, which is the
    // value the codec fills in for a missing one and the value this module
    // holds. A line of a version this module does not hold is the typed error
    // naming the version it carries.
    assert!(PROTOCOL_VERSION == 1);
    let refused = crate::codec::error_text(&CodecError::UnsupportedVersion(PROTOCOL_VERSION + 1));
    assert!(refused == "unsupported protocol version: 2");
    let line = "{\"v\":1,\"cmd\":\"shutdown\",\"id\":\"c1\"}";
    let decoded = crate::codec::decode_line(&line);
    if !decoded.is_ok() {
        let reason = crate::codec::error_text(&decoded.unwrap_err());
        panic!("the line of version {PROTOCOL_VERSION} that the codec is asked to read does not decode ({reason}), so the version this module holds and the version the codec reads no longer agree about a line a caller sends");
    }
    // The frame `codec` writes for that line carries the version of this module
    // as [`PROTOCOL_FIELD`], which is what makes the two names one claim.
    let message = decoded.unwrap();
    let frame = crate::codec::encode_frame(&message);
    if !frame.is_ok() {
        let reason = crate::codec::error_text(&frame.unwrap_err());
        panic!("the codec cannot write the frame of the line it just read ({reason}), so the vocabulary and the framing cannot agree on the version field of a frame");
    }
    let reparsed = serde_json::from_str::<Value>(&frame.unwrap()).unwrap();
    let expected = serde_json::to_value(PROTOCOL_VERSION).unwrap();
    assert!(reparsed.get(PROTOCOL_FIELD).unwrap().eq(&expected));
}

#[test]
fn a_payload_field_the_framing_uses_is_the_typed_error_naming_it() {
    // Plan item 3.1 review obligation: the names the framing uses -- the
    // version field [`PROTOCOL_FIELD`], the discriminator [`EVENT_FIELD`] of an
    // outbound message and the discriminator [`COMMAND_FIELD`] of an inbound
    // one -- must NOT be silently dropped when a caller uses one of them to
    // carry a payload. The table owns those names, so it answers which one a
    // set of fields carries; `proto/src/codec.rs` turns the answer into
    // [`CodecError.ReservedField`] instead of writing a frame whose payload
    // key would be overwritten on the way out (see the test of `codec`).
    let mut carrying = Map::<String, Value>::new();
    carrying.insert("state".to_string(), Value::String("pending".to_string()));
    assert!(reserved_field(&carrying) == None);
    carrying.insert(PROTOCOL_FIELD.to_string(), Value::Number(serde_json::Number::from(2)));
    assert!(reserved_field(&carrying).unwrap() == PROTOCOL_FIELD);
    let mut event = Map::<String, Value>::new();
    event.insert(EVENT_FIELD.to_string(), Value::String("progress".to_string()));
    assert!(reserved_field(&event).unwrap() == EVENT_FIELD);
    let mut command = Map::<String, Value>::new();
    command.insert(COMMAND_FIELD.to_string(), Value::String("report".to_string()));
    assert!(reserved_field(&command).unwrap() == COMMAND_FIELD);
    // The names are held here, next to the rows of the vocabulary, because
    // the doc is what names them: its "Versioning" section names `v` and its
    // message table names `event` and `cmd`, and
    // [`command_table_holds_exactly_the_commands_of_the_documented_table`]
    // reads the table of commands from that same doc.
    assert!(PROTOCOL_FIELD == "v");
    assert!(EVENT_FIELD == "event");
    assert!(COMMAND_FIELD == "cmd");
}
