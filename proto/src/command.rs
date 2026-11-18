//! The commands an interface process sends to the core daemon.
//!
//! The vocabulary is exactly the command table of
//! `doc/architecture/core-protocol.md` ("Commands", "All known commands"): verb
//! first, the JSON field `cmd` names the command, and the parameter names the
//! table fixes are the JSON fields of the line. All eight documented commands
//! are modelled; nothing that the doc does not name exists here.
//!
//! [`Command`] is the typed form of one decoded command line.
//! [`command_name`] and [`command_fields`] feed the encoder
//! ([`crate::codec::encode_frame`]); [`command_from_fields`] is called by
//! [`crate::codec::decode_line`] once `v` and `cmd` have been split off.
//!
//! A command name that the table does not contain is the typed error
//! [`crate::codec::CodecError.UnknownCommand`], never a panic, and the daemon
//! answers with an `error` event and stays alive. A field the table does not
//! mention is ignored, which is what lets an older core read a line written by
//! a newer interface.
//!
//! There is no I/O here: the codec turns a text line into values and back.

use serde_json::Map;
use serde_json::Value;

use crate::codec::CodecError;
use crate::codec::put_number;
use crate::codec::put_text;
use crate::codec::take_optional_int;
use crate::codec::take_text;

/// One command line, decoded: the command of the table with its parameters.
#[derive(Debug, Eq, PartialEq)]
pub enum Command {
    /// `version` — the handshake command; `nonce` proves the listener is our
    /// daemon (it is the nonce read from `state/core.lock`).
    Version(String),
    /// `import` — `path` of the one file to run through the importers.
    Import(String),
    /// `report` — `format` (`markdown`, `html`, `pdf`, `ods`) and the optional
    /// `year` the report covers.
    Report(String, Option::<i64>),
    /// `set-category` — `id` of the transaction, the `category` it belongs to
    /// and the `reason` the user gives.
    SetCategory(String, String, String),
    /// `confirm-annotation` — `annotation` plus which link is meant, see
    /// [`ConfirmTarget`]: its `id`, or the reject marker.
    ConfirmAnnotation(String, ConfirmTarget),
    /// `list-pending` — list the low-confidence annotation links.
    ListPending,
    /// `cancel` — cancel the command this connection ran most recently.
    Cancel,
    /// `shutdown` — exit the daemon and remove `state/core.lock`.
    Shutdown,
}

/// What a `confirm-annotation` command refers to.
///
/// The table writes the parameter as `id` | `reject`, so the line carries either
/// the `id` of the pending link the user accepts or the `reject` marker (JSON
/// `true`) for the link the user rejects. A line carrying neither is
/// [`crate::codec::CodecError.MissingField`], a line carrying both is
/// [`crate::codec::CodecError.ConflictingFields`] (the table writes the two
/// as alternatives, so neither may be dropped), and `reject` is only the marker
/// when it is the JSON value `true`.
#[derive(Debug, Eq, PartialEq)]
pub enum ConfirmTarget {
    /// The `id` of the annotation link accepted.
    Id(String),
    /// The annotation link rejected (wire form `"reject":true`).
    Reject,
}

/// The `cmd` name a command is written under.
///
/// Every name here is a row of the command table of
/// `doc/architecture/core-protocol.md`; the encoder writes it as the `cmd`
/// field of the line.
pub fn command_name(command: &Command) -> String {
    match command {
        Command::Version(_) => { return "version".to_string(); }
        Command::Import(_) => { return "import".to_string(); }
        Command::Report(_, _) => { return "report".to_string(); }
        Command::SetCategory(_, _, _) => { return "set-category".to_string(); }
        Command::ConfirmAnnotation(_, _) => { return "confirm-annotation".to_string(); }
        Command::ListPending => { return "list-pending".to_string(); }
        Command::Cancel => { return "cancel".to_string(); }
        Command::Shutdown => { return "shutdown".to_string(); }
    }
}

/// The parameter fields of a command, as the JSON object written on the line
/// besides `v` and `cmd`.
///
/// Optional parameters are written only when they carry a value, so a `report`
/// without `year` simply has no `year` field; the reject form of
/// `confirm-annotation` is the boolean `true` (see [`ConfirmTarget`]).
pub fn command_fields(command: &Command) -> Map::<String, Value> {
    let mut fields = Map::<String, Value>::new();
    match command {
        Command::Version(nonce) => { put_text(&mut fields, "nonce", nonce); }
        Command::Import(path) => { put_text(&mut fields, "path", path); }
        Command::Report(format, year) => {
            put_text(&mut fields, "format", format);
            if let Some(given) = year { put_number(&mut fields, "year", *given); }
        }
        Command::SetCategory(id, category, reason) => {
            put_text(&mut fields, "id", id);
            put_text(&mut fields, "category", category);
            put_text(&mut fields, "reason", reason);
        }
        Command::ConfirmAnnotation(annotation, target) => {
            put_text(&mut fields, "annotation", annotation);
            match target {
                ConfirmTarget::Id(id) => { put_text(&mut fields, "id", id); }
                ConfirmTarget::Reject => { fields.insert("reject".to_string(), Value::Bool(true)); }
            }
        }
        Command::ListPending | Command::Cancel | Command::Shutdown => { }
    }
    return fields;
}

/// Decode the parameter fields of the command named `name`.
///
/// Required parameters missing are [`crate::codec::CodecError.MissingField`]
/// and parameters of the wrong JSON type
/// [`crate::codec::CodecError.WrongType`]; a name the command table does not
/// contain is [`crate::codec::CodecError.UnknownCommand`]. Unknown fields are
/// ignored, so the caller does not have to know every parameter of a newer
/// interface.
pub fn command_from_fields(name: &str, fields: &mut Map::<String, Value>) -> Result::<Command, CodecError> {
    match name {
        "version" => { return Ok(Command::Version(take_text(fields, "nonce")?)); }
        "import" => { return Ok(Command::Import(take_text(fields, "path")?)); }
        "report" => { return Ok(Command::Report(take_text(fields, "format")?, take_optional_int(fields, "year")?)); }
        "set-category" => { return Ok(Command::SetCategory(take_text(fields, "id")?, take_text(fields, "category")?, take_text(fields, "reason")?)); }
        "confirm-annotation" => {
            let annotation = take_text(fields, "annotation")?;
            return confirm_annotation(fields, annotation);
        }
        "list-pending" => { return Ok(Command::ListPending); }
        "cancel" => { return Ok(Command::Cancel); }
        "shutdown" => { return Ok(Command::Shutdown); }
        other => { return Err(CodecError::UnknownCommand(other.to_string())); }
    }
}

/// The `id` | `reject` alternative of the `confirm-annotation` command.
///
/// The command table of `doc/architecture/core-protocol.md` writes the two
/// parameters as alternatives, so a line carrying both is
/// [`crate::codec::CodecError.ConflictingFields`] rather than one form silently
/// dropping the other, a line carrying neither is
/// [`crate::codec::CodecError.MissingField`], and `reject` counts as the
/// reject marker only when it is the JSON value `true`.
fn confirm_annotation(fields: &mut Map::<String, Value>, annotation: String) -> Result::<Command, CodecError> {
    if fields.contains_key("id") && fields.contains_key("reject") { return Err(CodecError::ConflictingFields("id".to_string(), "reject".to_string())); }
    if let Some(marker) = fields.remove("reject") {
        if marker != Value::Bool(true) { return Err(CodecError::WrongType("reject".to_string())); }
        return Ok(Command::ConfirmAnnotation(annotation, ConfirmTarget::Reject));
    }
    if !fields.contains_key("id") { return Err(CodecError::MissingField("id".to_string())); }
    return Ok(Command::ConfirmAnnotation(annotation, ConfirmTarget::Id(take_text(fields, "id")?)));
}
