//! The commands an interface process sends to the core daemon.
//!
//! The vocabulary is exactly the command table of
//! `doc/architecture/core-protocol.md` ("Commands", "All known commands"): verb
//! first, the JSON field `cmd` names the command, and the parameter names the
//! table fixes are the JSON fields of the line. All eight documented commands
//! are modelled; nothing that the doc does not name exists here.
//!
//! [`Command`] is the TYPED VIEW of one row of that table, which plan item 3.2
//! keeps as data in [`crate::commands`]: [`crate::commands::take_fields`] checks
//! the fields of a line against its row and takes them in the order of that row
//! (which is what [`command_from_fields`] hands to the typed read), the read
//! side of the table ([`crate::commands::text_value`] and
//! [`crate::commands::optional_int_value`]) converts those values by the wire
//! type the row declares for them, and
//! [`crate::commands::put_fields`] writes a frame under the parameter names of a
//! row, so the encoder of [`crate::codec::encode_frame`] can only write names
//! the table holds. Consequently NEITHER the command names NOR the parameter
//! names of the vocabulary, and no field presence or type check, appear in this
//! module: a typed command relates to its row by POSITION in the table (see
//! [`command_parameters`]), and
//! [`crate::commands::row_name`] reads the name back from the row.
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
use crate::commands::CommandSpec;
use crate::commands::command_rows;
use crate::commands::ValueKind;
use crate::commands::optional_int_value;
use crate::commands::optional_number;
use crate::commands::position_of;
use crate::commands::put_fields;
use crate::commands::row_name;
use crate::commands::take_alternative;
use crate::commands::take_fields;
use crate::commands::text_value;

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
/// The row of `crate::commands::command_rows` for `confirm-annotation` writes
/// the parameters of the command as `id` | `reject` (see
/// `doc/architecture/core-protocol.md`, and [`crate::commands::CommandSpec.either`]
/// which carries that pair), so the line carries either the `id` of the link
/// the user accepts or the `reject` marker (JSON `true`) for the link the user
/// rejects. A line carrying neither is
/// [`crate::codec::CodecError.MissingField`], a line carrying both is
/// [`crate::codec::CodecError.ConflictingFields`] (the table writes the two as
/// alternatives, so neither may be dropped), and `reject` is only the marker
/// when it is the JSON value `true` (the [`crate::commands::ValueKind`] the row
/// declares for that parameter).
#[derive(Debug, Eq, PartialEq)]
pub enum ConfirmTarget {
    /// The `id` of the annotation link accepted.
    Id(String),
    /// The annotation link rejected (wire form `"reject":true`).
    Reject,
}

/// The `cmd` name a command is written under: the name of the row of the
/// command table the typed command is the view of, read from that row by
/// [`crate::commands::row_name`] at the position [`command_parameters`] gives
/// for the command.
///
/// The name is not written here: [`command_parameters`] says only which row of
/// [`crate::commands::command_rows`] the typed command stands for, in the order
/// `doc/architecture/core-protocol.md` lists its commands, and the row holds
/// the name the encoder writes as the `cmd` field of the line.
pub fn command_name(command: &Command) -> String {
    return row_name(&command_rows(), command_parameters(command).0);
}

/// The parameter fields of a command, as the JSON object written on the line
/// besides `v` and `cmd`: the parameter names of the row of the command table
/// the command stands for, filled by [`crate::commands::put_fields`] with the
/// values of the typed command in the order of that row (see
/// [`command_parameters`]).
///
/// Optional parameters are written only when they carry a value, so a `report`
/// without `year` simply has no `year` field, and the reject form of
/// `confirm-annotation` is the boolean `true` (see [`ConfirmTarget`]).
pub fn command_fields(command: &Command) -> Map::<String, Value> {
    let mut fields = Map::<String, Value>::new();
    let parameters = command_parameters(command);
    put_fields(&mut fields, &command_rows(), row_name(&command_rows(), parameters.0), &parameters.1);
    return fields;
}

/// Which row of the command table a typed command is the view of — the position
/// of that row in the order of the table — and the values of the parameters of
/// that row, in the order of the row, as the JSON values the row declares for
/// them; `Value::Null` is the absent marker
/// [`crate::commands::put_fields`] leaves out of the line, which is how the
/// optional `year` of a `report` without a year and the alternative of
/// `confirm-annotation` that was not taken are written. No parameter name is
/// written here — the position of a value in this list is the position of that
/// parameter in the row.
///
/// This is the only relation this module makes between a typed command and the
/// table on the way out; the way in is [`read_typed_command`], which is keyed
/// by the same positions and reads each value through the read side of the
/// table.
fn command_parameters(command: &Command) -> (usize, Vec::<Value>) {
    match command {
        Command::Version(nonce) => { return (0, [Value::String(nonce.to_string())].to_vec()); }
        Command::Import(path) => { return (1, [Value::String(path.to_string())].to_vec()); }
        Command::Report(format, year) => { return (2, [Value::String(format.to_string()), optional_number(*year)].to_vec()); }
        Command::SetCategory(id, category, reason) => { return (3, [Value::String(id.to_string()), Value::String(category.to_string()), Value::String(reason.to_string())].to_vec()); }
        Command::ConfirmAnnotation(annotation, target) => {
            match target {
                ConfirmTarget::Id(id) => { return (4, [Value::String(annotation.to_string()), Value::String(id.to_string()), Value::Null].to_vec()); }
                ConfirmTarget::Reject => { return (4, [Value::String(annotation.to_string()), Value::Null, Value::Bool(true)].to_vec()); }
            }
        }
        Command::ListPending => { return (5, Vec::<Value>::new()); }
        Command::Cancel => { return (6, Vec::<Value>::new()); }
        Command::Shutdown => { return (7, Vec::<Value>::new()); }
    }
}

/// Decode the parameter fields of the command named `name`.
///
/// [`crate::commands::take_fields`] is the vocabulary check of plan item 3.2:
/// it refuses a `name` the command table does not contain as
/// [`crate::codec::CodecError.UnknownCommand`], checks the fields against the
/// row the line named (a required parameter that is missing is
/// [`crate::codec::CodecError.MissingField`] naming that parameter, a value of
/// the wrong JSON type is [`crate::codec::CodecError.WrongType`], and the
/// `id` | `reject` alternative of `confirm-annotation` must be present exactly
/// once), and takes the parameters out of `fields` in the order of that row.
/// Fields the row does not name are ignored, so the caller does not have to
/// know every parameter of a newer interface. The typed command is then built
/// from the row's values in the order of the row.
pub fn command_from_fields(name: &str, fields: &mut Map::<String, Value>) -> Result::<Command, CodecError> {
    let read_name = name.to_string();
    let taken = take_fields(&command_rows(), fields, read_name.to_string())?;
    return read_typed_command(&command_rows(), read_name, &taken);
}

/// The typed command of the row named `name`, built from the parameter values
/// [`crate::commands::take_fields`] took for it, addressed by position in the
/// order of that row.
///
/// The table has already checked presence and wire type, so the values read
/// here are the row's own parameters, converted by the read side of the table
/// ([`crate::commands::text_value`] and
/// [`crate::commands::optional_int_value`]), which name a parameter that is
/// nevertheless absent or of the wrong shape by what the row holds at that
/// position; this module names none of them. A row of the table this build has
/// no typed view of is answered with the typed error an unknown command name is
/// answered with, because this build cannot carry such a command out.
fn read_typed_command(rows: &Vec::<CommandSpec>, name: String, taken: &Vec::<Value>) -> Result::<Command, CodecError> {
    match position_of(&rows, name.clone()) {
        None => { return Err(CodecError::UnknownCommand(name.to_string())); }
        Some(position) => {
            match position {
                0 => { return Ok(Command::Version(text_value(&rows, name, taken, 0)?)); }
                1 => { return Ok(Command::Import(text_value(&rows, name, taken, 0)?)); }
                2 => { return Ok(Command::Report(text_value(&rows, name.clone(), taken, 0)?, optional_int_value(&rows, name, taken, 1)?)); }
                3 => { return Ok(Command::SetCategory(text_value(&rows, name.clone(), taken, 0)?, text_value(&rows, name.clone(), taken, 1)?, text_value(&rows, name, taken, 2)?)); }
                4 => { return confirm_annotation(&rows, name, taken); }
                5 => { return Ok(Command::ListPending); }
                6 => { return Ok(Command::Cancel); }
                7 => { return Ok(Command::Shutdown); }
                _ => { return Err(CodecError::UnknownCommand(name.to_string())); }
            }
        }
    }
}

/// The typed form of the `id` | `reject` alternative of the `confirm-annotation`
/// command: the annotation the command is about, plus which link is meant.
///
/// [`crate::commands::take_alternative`] says which parameter of the row's
/// `either` pair the line carries, together with the
/// [`crate::commands::ValueKind`] the row declares for it, so the reject marker
/// (JSON `true`) and the accepted `id` are told apart BY THE TABLE, without this
/// module naming either parameter. [`crate::commands::check_parameters`] has
/// already refused a line carrying both of them
/// ([`crate::codec::CodecError.ConflictingFields`]) or neither of them
/// ([`crate::codec::CodecError.MissingField`] naming the parameter the doc lists
/// first, which is the error [`crate::commands::take_alternative`] answers with
/// here too), and a value that is neither the marker nor a text is the typed
/// error naming the parameter the line carried it under.
fn confirm_annotation(rows: &Vec::<CommandSpec>, name: String, taken: &Vec::<Value>) -> Result::<Command, CodecError> {
    let annotation = text_value(&rows, name.clone(), taken, 0)?;
    let carried = take_alternative(&rows, taken, name)?;
    match carried.2 {
        ValueKind::RejectMarker => { return Ok(Command::ConfirmAnnotation(annotation, ConfirmTarget::Reject)); }
        _ => {
            let link = carried.1.as_str().ok_or(CodecError::WrongType(carried.0.to_string()))?;
            return Ok(Command::ConfirmAnnotation(annotation, ConfirmTarget::Id(link.to_string())));
        }
    }
}
