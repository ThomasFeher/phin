//! The command interpreter of the core daemon — plan item 3.4.
//!
//! `doc/architecture/core-protocol.md` ("Commands", "Events", "Unknown input")
//! is implemented here on the reading side of the stream: [`run`] takes an abstract
//! INPUT stream and the [`writer::EventSink`] of plan item 3.3, keeps the `\n`
//! framing of plan item 3.1 ([`proto::codec::feed`], [`proto::codec::Framing`]) so
//! a chunk that stops in the middle of a line stays pending instead of being
//! decoded or dropped, decodes each line with
//! [`proto::codec::decode_line`] — which validates it against the command table of
//! plan item 3.2 and reads its `v` field — and answers every line the interface
//! sends with the ONE terminal event of plan item 3.3, while the loop carries on.
//!
//! What the interpreter does with each kind of line, which is the whole of its
//! contract:
//!
//! * a command of the table is dispatched through the injected [`Work`] seam, so
//!   the phases that give a verb its behaviour (import 4.8, report 5.8,
//!   `set-category` 6.2, `confirm-annotation`/`list-pending` 6.3) plug in there and
//!   nothing else in this module changes;
//! * a line the codec refuses — an unknown command, an unparseable or truncated
//!   line, a line with neither `cmd` nor `event`, a missing or wrongly typed
//!   parameter, a reserved or conflicting parameter, a `v` higher than
//!   `proto::commands::PROTOCOL_VERSION` — is answered with one `error` event and
//!   the loop CONTINUES ("Unknown input": the daemon stays alive);
//! * an event line arriving from the interface (`{"event":…}`) is not a command
//!   at all and is answered the same way — typed, never a panic;
//! * `cancel` answers one `cancelled` event through plan 3.3's cancel-flag path
//!   (see [`cancel_last`]);
//! * `shutdown` ends the loop without an event (see [`serve`]).
//!
//! No I/O happens in this module: the stream of the interface is handed in as a
//! [`TextInput`] and every line out goes to the [`EventSink`], which is what makes
//! the loop unit-testable without starting the core (plan rule 4). The
//! [`StringInput`] harness IS that abstraction — the tests script a session with
//! it, and the transport of plan item 3.6 hands in the line stream of its own
//! connection; opening the socket, writing `state/core.lock`, the `version`/`hello`
//! handshake, the idle timeout and `ping`/`quit` belong to plan items 3.5 and 3.6,
//! NOT here. `core/src/main.rs` supplies the real pair: stdin as a [`TextInput`],
//! stdout as a flushing sink.
//!
//! ## The `code` of every refusal, and why
//!
//! `doc/architecture/core-protocol.md` ("Error", draft) names five codes:
//! `unknown-command`, `plugin-failed`, `converter-missing`, `io`, `busy`. The doc
//! is the source of truth and is not edited by this item, so a refusal only ever
//! answers with one of those five, never with a code of this module's invention
//! ([`proto::event::ErrorReason`] keeps such a code verbatim in `Other`, which is
//! how an OLDER interface reads a NEWER core, not a licence to invent one here):
//!
//! | line received | `code` | `cmd` |
//! |---|---|---|
//! | `CodecError.UnknownCommand(name)` | `unknown-command` | the unknown verb |
//! | any other `CodecError`, an event line | `unknown-command` | absent |
//! | a verb whose phase is not wired (see [`WiredNowhere`]) | see the table in that method | the verb |
//!
//! Of the five, `unknown-command` ("the command name is not in the command
//! table") is the only one that tells the interface "this line is not a command
//! this daemon can run"; the doc's own error example pairs that code with the
//! codec's own message ("unknown command: frob"), which is what
//! [`proto::codec::error_text`] supplies here. A `cmd` field is written ONLY
//! where the line's command is identified at all: an event line and
//! `CodecError.UnknownEvent` carry no command name, and for
//! `MissingField`/`WrongType`/`ReservedField`/`ConflictingFields`/
//! `UnsupportedVersion` the codec refused the line BEFORE resolving a verb, so the
//! optional `cmd` of the `error` event is left out rather than guessed at — the
//! doc says `cmd` is the command "when it is known". A malformed parameter of a
//! KNOWN verb is the one case where that code is a stretch: the doc names no code
//! for a bad parameter, so the message carries the precise complaint and a later
//! item that amends the doc may sharpen the code.

use proto::codec::CodecError;
use proto::codec::decode_line;
use proto::codec::encode_frame;
use proto::codec::error_text;
use proto::codec::feed;
use proto::codec::Framing;
use proto::codec::Message;
use proto::command::Command;
use proto::command::command_name;
use proto::event::ErrorReason;
use proto::event::Event;
use proto::event::event_kind;
use super::writer;
use super::writer::EventSink;

/// One read of the interface's side of the stream.
#[derive(Debug, Eq, PartialEq)]
pub enum TextRead {
    /// Text the stream handed out. It is not cut at line ends here on purpose:
    /// [`proto::codec::feed`] of plan 3.1 decides where the line ends are, which
    /// is what lets a chunk that stops in the middle of a line leave its first
    /// half pending ("Unknown input": the stream never resynchronizes on a
    /// partial line, it waits for the rest of it).
    Text(String),
    /// The stream has no more text: end of file, closed connection, a script
    /// used up. It is its own variant, so "nothing more to read" is never confused
    /// with an empty chunk (which is possible and says nothing at all).
    Done,
    /// The stream itself could not be read; the text is the message for the user.
    /// An unreadable stream is not an event: nothing can be answered for it, so
    /// [`run`] answers the FAILURE by stopping with [`Stop.StreamBroken`]. The
    /// SCRIPTED [`StringInput`] below never produces it — a script cannot fail
    /// to be read, only run out — but a REAL stream does: the stdin of
    /// `core/src/main.rs` reports a failed read here, which is why the variant
    /// is live in this build and not merely constructible.
    Unreadable(String),
}

/// The abstract stream of commands the interpreter reads.
pub trait TextInput {
    /// Hand out the next text of the stream, or say that there is none.
    fn read_text(&mut self) -> TextRead;
}

/// A scripted session: the text the tests want the interface to send.
///
/// PUBLIC and NOT gated on `#[cfg(test)]` on purpose, although no production
/// path of THIS item uses it: it is the harness the LATER items of the plan
/// reuse, since the transport tests of plan item 3.6 script a session of
/// [`run`] through this same type instead of opening a socket, and item 3.5
/// drives the `version`/`hello` handshake through it — and a test module of
/// another package CANNOT reach a `#[cfg(test)]` item of this one, so gating
/// it would force those items to write their own [`TextInput`].
#[derive(Debug, Eq, PartialEq)]
pub struct StringInput {
    /// The chunks still to be handed out, one read per entry. A chunk holds its
    /// own `\n` when it is a whole line of the stream; the last entry of a script
    /// may deliberately leave a partial line pending (see the test of that).
    pub lines: Vec::<String>
}

/// The harness holding the scripted chunks `lines` — public for the same reason
/// as [`StringInput`]: the transport tests of plan item 3.6 and the handshake
/// tests of item 3.5 build their sessions with it.
pub fn scripted(lines: Vec::<String>) -> StringInput {
    return StringInput { lines: lines.to_vec() };
}

impl TextInput for StringInput {
    fn read_text(&mut self) -> TextRead {
        if self.lines.len() == 0 {
            return TextRead::Done;
        }
        let next = self.lines.remove(0);
        return TextRead::Text(next);
    }
}

/// What the work of one command came down to.
#[derive(Debug, Eq, PartialEq)]
pub enum Answered {
    /// The work wrote the event stream of the command itself, terminal event
    /// included — the way the phases that give a verb its behaviour drive it, since
    /// they know the `total` of items of plan 3.3's [`writer::CommandStream`] and this
    /// interpreter does not.
    EndedWith,
    /// The work has nothing to show, and says so with the documented `code` and
    /// the message the interpreter writes as the command's terminal `error` event.
    AnswerWith(ErrorReason, String),
}

/// The behaviour of the commands the interpreter dispatches, injected so that a
/// later phase plugs in its own work without any change in this module.
///
/// Note for the reviewer: a wired phase drives plan 3.3's primitives itself
/// ([`writer::start`], [`writer::progress`], [`writer::log`], [`writer::succeed`]
/// and the cancel flag), which is what the test work of this module does, BECAUSE
/// [`writer::run`] and [`writer::CommandWork`] of plan 3.3 are still a TEST-ONLY
/// convenience here: [`writer::CommandWork.do_item`][`writer::CommandWork`] hands
/// back no per-item value and the `payload` of [`writer::run`] is fixed BEFORE the
/// items run, so a command whose `result` summarizes its items — `affected` of
/// `set-category`, the `path`/`format`/`year` of `report` — cannot be driven through
/// it without editing `core/src/protocol/writer.rs`, which is outside the files
/// this item may touch. Widening it belongs to the item that needs it (4.8, 6.2).
pub trait Work {
    /// Answer the command `command` of the table, and say what it came down to.
    fn do_command(&mut self, command: Command, sink: &mut dyn EventSink) -> Answered;
}

/// Why the loop of [`run`] stopped reading the stream.
#[derive(Debug, Eq, PartialEq)]
pub enum Stop {
    /// The stream ended with nothing pending — end of file, the connection of plan
    /// 3.6 closed, a script used up — and nothing was answered FOR the end of
    /// input itself: the exit is clean, which is what the interface of plan 6.1
    /// reads as an ordinary end of session.
    EndOfInput,
    /// The stream ended with a partial line still pending. "Unknown input" of
    /// `doc/architecture/core-protocol.md` says the stream never resynchronizes on
    /// a partial line, and at the end of the input NOTHING can ever complete it, so
    /// the tail is neither decoded, nor answered, nor repaired: the exit is clean
    /// just the same, and the text is the tail that was left pending only so that
    /// the caller and the tests can tell the two clean ends apart.
    HalfLineAtEnd(String),
    /// The interface sent `shutdown` of the command table, and the doc's draft says
    /// the daemon does not answer it: the closed stream is the signal. Removing
    /// `state/core.lock` and the idle timeout of the same section belong to plan
    /// items 3.5/3.6, which is why the loop only stops here.
    ShutdownRequested,
    /// No event line could be written for one line of the input, OR the input
    /// itself could not be read: [`run`] answers [`TextRead.Unreadable`] with
    /// this variant, which is how the stdin of `core/src/main.rs` reports a
    /// read error and what its exit code 1 is for, so the variant is LIVE in
    /// this build, not merely constructible. What this build cannot produce is
    /// the OTHER source of it — the text is the codec's own message or plan
    /// 3.3's own refusal, kept rather than swallowed (see [`refusal_text`]), and
    /// neither arises from the events this build writes: `encode_frame` refuses
    /// only a `result` payload key named `v`/`event`/`cmd`, and a refusal of
    /// plan 3.3's writer needs a stream that already ended or was already
    /// cancelled, which the streams started here never are.
    StreamBroken(String),
}

/// What one line of the input stream left the loop to do. Private to this module:
/// the loop of [`run`] is the only thing that reads it.
#[derive(Debug, Eq, PartialEq)]
enum Step {
    /// Keep reading. The text is the command this stream most recently ran,
    /// [`String::new`] when it ran none, which is as far as
    /// `doc/architecture/core-protocol.md` ("Cancellation") lets a later `cancel`
    /// line say WHAT it cancels: "the command that the same connection most recently
    /// sent". Carried on every path so no line can silently forget it.
    CarriedOn(String),
    /// `shutdown` was read, and its section of the doc answers it with no event at
    /// all.
    StoppedShutdown,
    /// The event line this line of the input calls for could not be written; the
    /// text is why, and reaches the daemon's exit (see [`Stop.StreamBroken`]).
    StoppedBroken(String),
}

/// Read one session of commands and answer every line of it.
///
/// The framing, the decoding and the vocabulary check are the modules that own
/// them: [`proto::codec::feed`] keeps a half line pending between reads
/// ([`Framing`] holds it), [`proto::codec::decode_line`] reads one line into a
/// typed [`proto::codec::Message`] and has already checked it against the command
/// table of plan 3.2, including the `v` field, and the terminal events go through
/// plan 3.3's [`writer::CommandStream`], so the "exactly one terminal event per
/// command" rule of `doc/architecture/core-protocol.md` ("Events") holds on the
/// refusal paths of this interpreter as well as on the ones a command reaches by
/// doing work. Every line the interface sends gets AT MOST one event line here
/// (`shutdown` gets none), every command gets exactly one, and the loop keeps
/// reading after an error, which is what "the daemon stays alive" means.
pub fn run(input: &mut dyn TextInput, sink: &mut dyn EventSink, work: &mut dyn Work) -> Stop {
    let mut framing = Framing { partial: String::new() };
    let mut most_recent = String::new();
    loop {
        match input.read_text() {
            TextRead::Unreadable(why) => { return Stop::StreamBroken(why); }
            TextRead::Done => {
                if framing.partial.len() > 0 {
                    return Stop::HalfLineAtEnd(framing.partial);
                }
                return Stop::EndOfInput;
            }
            TextRead::Text(chunk) => {
                let lines = feed(&mut framing, &chunk);
                for index in 0..lines.len() {
                    match handle_line(lines[index].to_string(), sink, work, most_recent.to_string()) {
                        Step::CarriedOn(name) => { most_recent = name; }
                        Step::StoppedShutdown => { return Stop::ShutdownRequested; }
                        Step::StoppedBroken(why) => { return Stop::StreamBroken(why); }
                    }
                }
            }
        }
    }
}

/// Answer one line of the stream: `line` is the whole line as framing completed
/// it, `most_recent` the command this stream most recently ran.
fn handle_line(line: String, sink: &mut dyn EventSink, work: &mut dyn Work, most_recent: String) -> Step {
    match decode_line(&line) {
        Err(reason) => {
            // "Unknown input": an unknown command, an unparseable or truncated line,
            // a parameter the table refuses and a `v` this build does not speak all
            // come out of the codec as one typed refusal, all are answered with the
            // ONE `error` event of `unknown-command` and the codec's own message,
            // and the loop carries on.
            match answer_line(sink, blame_of(&reason), ErrorReason::UnknownCommand, error_text(&reason)) {
                Some(why) => { return Step::StoppedBroken(why); }
                None => { return Step::CarriedOn(most_recent); }
            }
        }
        Ok(held) => {
            match held {
                Message::Inbound(command) => { return serve(command, sink, work, most_recent); }
                Message::Outbound(event) => {
                    // An event line is what the daemon writes, never what the
                    // interface sends, and it is NOT a command the interpreter can
                    // run: answered as the doc's "Unknown input" answers an unreadable
                    // line, and the loop carries on. `proto` read the line fine, so
                    // the event name is put in the message through
                    // [`proto::event::event_kind`] rather than refused.
                    let complaint = format!("the line is an event, which the interface does not send: {kind}", kind = event_kind(&event));
                    match answer_line(sink, None, ErrorReason::UnknownCommand, complaint) {
                        Some(why) => { return Step::StoppedBroken(why); }
                        None => { return Step::CarriedOn(most_recent); }
                    }
                }
            }
        }
    }
}

/// Dispatch a command the command table of plan 3.2 knows.
///
/// [`proto::command::command_name`] reads the name back out of the table row the
/// typed command stands for, so the `cmd` field of every event of this command
/// comes from the vocabulary and is never spelled here; the name is what
/// `doc/architecture/core-protocol.md` ("Cancellation") lets a `cancel` line of the
/// same connection refer to, which is why it is remembered for the whole stream.
fn serve(command: Command, sink: &mut dyn EventSink, work: &mut dyn Work, most_recent: String) -> Step {
    // `shutdown` and `cancel` are answered by this interpreter rather than by the
    // [`Work`] seam, because what they change is what the DAEMON does, not what one
    // command does; and neither is implemented as a FEATURE here, which plan items
    // 3.5 (the `version`/`hello` handshake, the nonce of `state/core.lock`) and 3.6
    // (the transport, removing the lock file, the idle timeout) own: `shutdown` only
    // stops the loop, and its section of the doc says the daemon does not answer it.
    // There is no `ping` and no `quit` anywhere in this build.
    match command {
        Command::Shutdown => { return Step::StoppedShutdown; }
        Command::Cancel => { return cancel_last(sink, most_recent); }
        _ => { }
    }
    let verb = command_name(&command);
    match work.do_command(command, sink) {
        // The phase that wired this verb wrote its own event stream, terminal event
        // included; nothing more is owed to the command, and nothing here can check
        // that claim, which is why the claim is the seam's, not a caller's guess.
        Answered::EndedWith => { return Step::CarriedOn(verb); }
        Answered::AnswerWith(reason, text) => {
            match answer_line(sink, Some(verb.to_string()), reason, text) {
                Some(why) => { return Step::StoppedBroken(why); }
                None => { return Step::CarriedOn(verb); }
            }
        }
    }
}

/// Answer a `cancel` line of the interface with the one `cancelled` event
/// `doc/architecture/core-protocol.md` ("Cancellation") calls for.
///
/// The doc lets `cancel` refer to nothing more than "the command that the same
/// connection most recently sent", and in a daemon that serializes its commands
/// ("Sharing") the `cancel` line of one stream can only be read AFTER the command
/// it refers to has already been answered — no command of this interpreter is
/// still running between two reads of its own input — so the name of that command
/// is all that is known here, and the event names exactly that. The event still
/// goes through plan 3.3's cancel-flag path rather than being framed by hand
/// ([`writer::request_cancel`] sets the flag, and afterwards
/// [`writer::cancel`] is the only event the stream still accepts), which is the
/// state the work of a later phase (4.8's importers, 5.8's renderers) leaves its
/// own [`writer::CommandStream`] in when IT reads the `cancel` line between items
/// and checks [`writer::cancel_requested`] there.
fn cancel_last(sink: &mut dyn EventSink, most_recent: String) -> Step {
    if most_recent.len() == 0 {
        // Nothing was ever run on this connection, so no partial result was
        // discarded, and the doc's `cancelled` event carries the `cmd` field only
        // when the command is known ([`proto::event::Event.Cancelled`] takes it
        // optionally). There is no command stream to close either — a stream belongs
        // to a command — so the line is framed by `proto` and handed to the same
        // sink, which is the only case in this module where an event line does not
        // go through plan 3.3's writer.
        match write_event(sink, Event::Cancelled(None)) {
            Some(why) => { return Step::StoppedBroken(why); }
            None => { return Step::CarriedOn(most_recent); }
        }
    }
    let mut stream = writer::start(most_recent.to_string(), 0);
    writer::request_cancel(&mut stream);
    match writer::cancel(&mut stream, sink) {
        Err(why) => { return Step::StoppedBroken(refusal_text(why)); }
        Ok(_) => { return Step::CarriedOn(most_recent.to_string()); }
    }
}

/// Write the one `error` event that answers a line this build cannot run.
///
/// The polarity of every emitter of this module, stated once and used everywhere:
/// [`None`] when the line was written, the reason it could not be otherwise, so the
/// loop can report why it gave up instead of swallowing a refusal. The `cmd` field
/// is written only where the line's command is identified at all (see
/// [`blame_of`]): a command of the table HAS a command stream, so its event goes
/// through plan 3.3's writer and the rule of "exactly one terminal event per
/// command" holds of it too, while a line whose command was never resolved has no
/// stream, and its line comes from [`proto::codec::encode_frame`] alone.
fn answer_line(sink: &mut dyn EventSink, cmd: Option::<String>, reason: ErrorReason, text: String) -> Option::<String> {
    match cmd {
        // A command of the table HAS a command stream, so its terminal event
        // goes through plan 3.3's [`writer::fail`], which is how the doc's
        // "exactly one terminal event per command" rule and the `cmd` field of
        // an `error` event hold on this path as well.
        Some(verb) => {
            let mut stream = writer::start(verb, 0);
            match writer::fail(&mut stream, sink, reason, text.to_string()) {
                Err(why) => { return Some(refusal_text(why)); }
                Ok(_) => { return None; }
            }
        }
        // A line whose command was never resolved has no stream at all, so the
        // optional `cmd` of the doc's `error` event is left out and the line is
        // framed by `proto` alone (see [`write_event`]).
        None => { return write_event(sink, Event::Error(None, reason, text.to_string())); }
    }
}

/// Frame one event line with [`proto::codec::encode_frame`] and hand it to the sink.
/// [`None`] when it was written, the codec's own message otherwise. This is where
/// the lines that belong to NO command stream are written — see [`answer_line`]
/// and [`cancel_last`] — and it bypasses nothing of the protocol: the framing of
/// the line is `proto`'s own, and only the state of a command is plan 3.3's.
fn write_event(sink: &mut dyn EventSink, event: Event) -> Option::<String> {
    let message = Message::Outbound(event);
    match encode_frame(&message) {
        Err(reason) => { return Some(error_text(&reason)); }
        Ok(frame) => { sink.write_line(frame); return None; }
    }
}

/// The command name the `error` event of a refused line may name back: the verb of
/// [`proto::codec::CodecError.UnknownCommand`], which is what the doc's error
/// example is about and what the task's clause asks the event to name.
///
/// The 9 variants of [`proto::codec::CodecError`] are all answered with the ONE
/// documented code (see the table at the top of this module), and the codec's own
/// message through [`proto::codec::error_text`] says which of them it was:
/// | refusal of [`proto::codec::decode_line`] | what the `error` event carries |
/// |---|---|
/// | `UnknownCommand(name)` | `unknown-command`, `cmd` = the unknown verb, message "unknown command: {name}" |
/// | `UnparseableLine` | `unknown-command`, no `cmd`, message "line is not one JSON object" — the doc's "Unknown input" answers an unparseable line with an error event and keeps the daemon alive, and of the five codes only this one says "this line is not a command I can run" |
/// | `MissingDiscriminator` | `unknown-command`, no `cmd`: a line with neither `cmd` nor `event` names no verb, so nothing may be guessed into `cmd` |
/// | `UnknownEvent(name)` | `unknown-command`, no `cmd`: an event name is not a command, and an event line arriving from the interface is not input this daemon runs |
/// | `MissingField(name)`, `WrongType(name)`, `ReservedField(name)`, `ConflictingFields(a, b)` | `unknown-command`, no `cmd`: the codec refused the line BEFORE resolving a verb, so the optional `cmd` stays out and the message names the parameter. This is the one mapping the reviewer should challenge: the doc names no code for a malformed parameter of a KNOWN command, and inventing one is forbidden here because `doc/architecture/core-protocol.md` ("Error") is the source of truth and is not this item's to edit |
/// | `UnsupportedVersion(kept)` | `unknown-command`, no `cmd`, message "unsupported protocol version: {kept}" — the doc ("Versioning") requires the `error` event for a `v` this build does not speak but names no code for it |
/// 
fn blame_of(reason: &CodecError) -> Option::<String> {
    match reason {
        CodecError::UnknownCommand(name) => { return Some(name.to_string()); }
        _ => { return None; }
    }
}

/// The message of a refusal of plan 3.3's writer, kept so that the reason an
/// event line did not reach the interface survives to the exit of the daemon
/// instead of being swallowed. Naming note: this is
/// [`writer::WriteError`][`core::protocol::writer::WriteError`], NOT the identically
/// named type of `core/src/technical/fs_atomic.rs`; this module touches no file
/// and so never imports that one, and the writer's type is always spelled with
/// its module path here, which is what an item that touches BOTH (4.8, 6.2) has
/// to alias at its `use` lines.
fn refusal_text(why: writer::WriteError) -> String {
    match why {
        writer::WriteError::CancelRequested => { return "the command stream was already asked to cancel".to_string(); }
        writer::WriteError::AlreadyFinished(ended) => { return format!("the command stream already ended with {ended:?}"); }
        writer::WriteError::Codec(reason) => { return error_text(&reason); }
        writer::WriteError::TotalPassed(promised) => { return format!("the stream promised {promised} items"); }
    }
}

/// The work of the commands this build dispatches, which is NONE of them yet.
///
/// [`run`] is handed a [`Work`] and this is what `core/src/main.rs` hands it today:
/// every verb the command table knows but this build has no behaviour for answers
/// its terminal `error` event with a message naming the plan item that wires it,
/// so the interface is told what is missing instead of being hung or ignored, and a
/// later phase plugs its own [`Work`] implementation in at that one call site
/// (`import` 4.8, `report` 5.8, `set-category` 6.2, `confirm-annotation` and
/// `list-pending` 6.3, the `version` handshake 3.5) with no change in this module.
/// The `code` of each answer is one of the five of `doc/architecture/core-protocol.md`
/// ("Error") — never a code of this module's invention — and the reason for each
/// is the table below:
///
/// | command | `code` | why that code |
/// |---|---|---|
/// | `import` | `plugin-failed` | the importers of plan 4.4/4.8 are the plugins the doc means by that code ("an importer or analysis plugin failed or died"): in this build there is no importer to run, so the command fails as the importer failing fails |
/// | `report` | `plugin-failed` | the analysis of the report is the analysis plugin of plan 5.5 and the renderers of 5.6/5.7 are what plan 5.8 wires; the first stage of that chain is a plugin, and it cannot run |
/// | `set-category` | `io` | the override and the learned rule the command writes are data files of plan 6.2's store wiring, and `io` is the doc's code for "a file could not be read or written" |
/// | `confirm-annotation` | `io` | the confirmed annotation link is a data file of plan 6.3's store wiring, for the same reason |
/// | `list-pending` | `io` | the pending links plan 6.3 lists are read from that same store |
/// | `version` | `unknown-command` | the doc's handshake answer `hello` belongs to plan 3.5, so THIS build gives the handshake no answer; of the five codes this is the only one that tells the interface "this verb is not served here", and per the doc's discovery rules an interface that gets no handshake answer treats the listener as not-our-daemon rather than half-trusting it |
///
/// `cancel` and `shutdown` are absent from that table because this interpreter
/// answers them itself, before the seam is ever asked (see [`serve`]).
#[derive(Debug, Eq, PartialEq)]
pub struct WiredNowhere { }

/// The work of plan item 3.4: no verb wired, so every command of the table is
/// refused with the documented code of the table above.
pub fn wired_nowhere() -> WiredNowhere {
    return WiredNowhere { };
}

impl Work for WiredNowhere {
    fn do_command(&mut self, command: Command, _sink: &mut dyn EventSink) -> Answered {
        match command {
            Command::Version(nonce) => {
                return Answered::AnswerWith(ErrorReason::UnknownCommand, format!("the handshake of plan item 3.5 is not wired in this build, so no `hello` answers the nonce {nonce}"));
            }
            Command::Import(path) => {
                return Answered::AnswerWith(ErrorReason::PluginFailed, format!("no importer is wired for {path} yet: plan item 4.8 connects the importers to the interpreter"));
            }
            // The optional `year` of the command is deliberately not echoed
            // back: reading it means turning an absent parameter into a text,
            // and the refusal is about the analysis of `format`, not the year.
            Command::Report(format, _) => {
                return Answered::AnswerWith(ErrorReason::PluginFailed, format!("no analysis or renderer is wired for {format} yet: plan items 5.1-5.8 connect them to the interpreter"));
            }
            Command::SetCategory(id, category, reason) => {
                return Answered::AnswerWith(ErrorReason::Io, format!("the override of {id} to {category} (the reason the user gave: {reason}) is written to data files plan item 6.2 wires into the interpreter, and this build can neither read nor write them"));
            }
            // The `id` | `reject` alternative of the row of the command table
            // is not echoed either: the refusal is about the LINK, which is a
            // data file either way (see [`WiredNowhere`]'s table).
            Command::ConfirmAnnotation(annotation, _) => {
                return Answered::AnswerWith(ErrorReason::Io, format!("the annotation link {link} is a data file plan item 6.3 wires into the interpreter, and this build can neither read nor write it", link = annotation.to_string()));
            }
            Command::ListPending => {
                return Answered::AnswerWith(ErrorReason::Io, "the pending annotation links of plan item 6.3 are read from the store, which this build does not wire into the interpreter".to_string());
            }
            // `cancel` and `shutdown` never reach the seam ([`serve`] answers them
            // before it is asked), so this arm only closes the vocabulary: the
            // message says so, and no code of the doc is claimed for a command
            // nobody asks about here.
            _ => {
                return Answered::AnswerWith(ErrorReason::UnknownCommand, "this interpreter answers the cancel and shutdown commands itself, so the work seam is never asked about them".to_string());
            }
        }
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
use proto::event::reason_name;

/// The frames one session wrote, collected instead of written to a stream.
///
/// The collected lines are ALL a test needs: [`events_of`] reads them back with
/// `proto` into the typed events the interface would see, so a test asserts the
/// WHOLE sequence of protocol values rather than the text of a frame, and never
/// the order of the keys of a line, which `serde_json` writes lexicographically
/// (as in `core/src/protocol/writer.rs`).
#[cfg(test)]
#[derive(Debug, Eq, PartialEq)]
struct Frames {
    pub frames: Vec::<String>
}

#[cfg(test)]
fn collecting() -> Frames {
    return Frames { frames: Vec::<String>::new() };
}

#[cfg(test)]
impl EventSink for Frames {
    fn write_line(&mut self, line: String) {
        self.frames.push(line.to_string());
    }
}

/// The events of the collected frames, decoded back with `proto` in the order
/// they were written. A frame the interface cannot read is a bug of this module
/// and is reported as such rather than skipped, which is also what catches a
/// text field that forged a second line: the count of events would fall short
/// of the count of frames.
#[cfg(test)]
fn events_of(frames: &Vec::<String>) -> Vec::<Event> {
    let mut events = Vec::<Event>::new();
    for index in 0..frames.len() {
        match decode_line(&frames[index]) {
            Ok(Message::Outbound(event)) => { events.push(event); }
            _ => { panic!("the frame [{frame}] of the interpreter does not read back as one event of the protocol", frame = frames[index].to_string()); }
        }
    }
    if events.len() != frames.len() {
        panic!("{count_events} events were decoded from {count_frames} frames, so one frame of the session was not one event", count_events = events.len(), count_frames = frames.len());
    }
    return events;
}

/// The `event` name of every event of a session, in the order they were written.
#[cfg(test)]
fn kinds_of(events: &Vec::<Event>) -> Vec::<String> {
    let mut names = Vec::<String>::new();
    for index in 0..events.len() {
        names.push(event_kind(&events[index]).to_string());
    }
    return names;
}

/// How many events of a session are terminal ("Events": `result`, `error`,
/// `cancelled`), which is how the "exactly one per command" rule is checked.
#[cfg(test)]
fn terminals_of(events: &Vec::<Event>) -> i64 {
    let mut found = 0;
    for event in events.iter() {
        match event {
            Event::Result(_, _) => { found = found + 1; }
            Event::Error(_, _, _) => { found = found + 1; }
            Event::Cancelled(_) => { found = found + 1; }
            _ => { }
        }
    }
    return found;
}

/// The `code` of every `error` event of a session, as the text the interface
/// reads, in the order they were written; [`proto::event::reason_name`] turns
/// the typed code back into that text, which is what lets a test check the
/// documented code and not an invented one.
#[cfg(test)]
fn codes_of(events: &Vec::<Event>) -> Vec::<String> {
    let mut codes = Vec::<String>::new();
    for event in events.iter() {
        match event {
            Event::Error(_, reason, _) => { codes.push(reason_name(&reason)); }
            _ => { }
        }
    }
    return codes;
}

/// The `cmd` field of every `error` event of a session, [`String::new`] where
/// the line named no command at all.
#[cfg(test)]
fn blamed_of(events: &Vec::<Event>) -> Vec::<String> {
    let mut names = Vec::<String>::new();
    for event in events.iter() {
        match event {
            Event::Error(cmd, _, _) => {
                match cmd {
                    Some(name) => { names.push(name.to_string()); }
                    None => { names.push(String::new()); }
                }
            }
            _ => { }
        }
    }
    return names;
}

#[test]
fn a_scripted_session_yields_the_asserted_event_sequence() {
    // Plan item 3.4's success check, in one session: a scripted
    // [`StringInput`] of six lines of the stream, and the ASSERTED sequence of
    // events, each emitted line read back with [`decode_line`] and compared as a
    // typed [`proto::event::Event`] in order. The lines are one command the
    // interpreter refuses because its phase is not wired (`set-category`, 6.2),
    // the `cancel` line that follows it, an unknown verb, a line that is not one
    // JSON object, `shutdown` (which the doc says the daemon does not ANSWER), and
    // a last command AFTER `shutdown` that must produce nothing at all.
    let mut out = collecting();
    let mut input = scripted(["{\"v\":1,\"cmd\":\"set-category\",\"id\":\"3f9a1c7e\",\"category\":\"food:supermarket\",\"reason\":\"user\"}\n".to_string(), "{\"v\":1,\"cmd\":\"cancel\"}\n".to_string(), "{\"v\":1,\"cmd\":\"frob\"}\n".to_string(), "nope\n".to_string(), "{\"v\":1,\"cmd\":\"shutdown\"}\n".to_string(), "{\"v\":1,\"cmd\":\"list-pending\"}\n".to_string()].to_vec());
    let mut worker = wired_nowhere();
    let stopped = run(&mut input, &mut out, &mut worker);
    assert!(stopped == Stop::ShutdownRequested);
    let events = events_of(&out.frames);
    assert!(kinds_of(&events) == ["error".to_string(), "cancelled".to_string(), "error".to_string(), "error".to_string()].to_vec());
    // The refused command names itself, the code is the documented `io` of the
    // data files plan 6.2 wires, and the message names the phase (see the table
    // in [`WiredNowhere`]).
    assert!(events[0] == Event::Error(Some("set-category".to_string()), ErrorReason::Io, "the override of 3f9a1c7e to food:supermarket (the reason the user gave: user) is written to data files plan item 6.2 wires into the interpreter, and this build can neither read nor write them".to_string()));
    // "Cancellation": the event names the command this connection most recently
    // sent, which is the most that the doc lets `cancel` refer to.
    assert!(events[1] == Event::Cancelled(Some("set-category".to_string())));
    // "Unknown input": the unknown verb is named back on the `cmd` field, with
    // the codec's own message.
    assert!(events[2] == Event::Error(Some("frob".to_string()), ErrorReason::UnknownCommand, "unknown command: frob".to_string()));
    // ...and a line whose command was never resolved carries no `cmd` at all.
    assert!(events[3] == Event::Error(None, ErrorReason::UnknownCommand, "line is not one JSON object".to_string()));
    assert!(codes_of(&events) == ["io".to_string(), "unknown-command".to_string(), "unknown-command".to_string()].to_vec());
    assert!(blamed_of(&events) == ["set-category".to_string(), "frob".to_string(), String::new()].to_vec());
    // Three commands got a terminal event (set-category, frob, the unreadable
    // line); `cancel` is itself a command of the table and its terminal event is
    // the `cancelled` one, and `shutdown`/`list-pending` after it got none.
    assert!(terminals_of(&events) == 4);
    assert!(out.frames.len() == 4);
}

#[test]
fn an_unknown_command_answers_one_error_event_naming_the_verb_and_the_loop_continues() {
    // The clause of plan item 3.4 that a refusal must not end the session: the
    // `error` event is exactly ONE, its code the documented
    // `unknown-command` and its `cmd` the unknown verb the codec carried
    // ([`blame_of`]), and the `cancel` line sent AFTER it is still answered
    // and the line sent AFTER it is still answered: the `cancel` command of the
    // table got its own event, which is what proves the loop carried on.
    let mut out = collecting();
    let mut input = scripted(["{\"v\":1,\"cmd\":\"frob\",\"path\":\"/x\"}\n".to_string(), "{\"v\":1,\"cmd\":\"cancel\"}\n".to_string()].to_vec());
    let mut worker = wired_nowhere();
    let stopped = run(&mut input, &mut out, &mut worker);
    assert!(stopped == Stop::EndOfInput);
    let events = events_of(&out.frames);
    assert!(kinds_of(&events) == ["error".to_string(), "cancelled".to_string()].to_vec());
    assert!(events[0] == Event::Error(Some("frob".to_string()), ErrorReason::UnknownCommand, "unknown command: frob".to_string()));
    // The `cancel` line still got its own terminal event, which is what proves
    // the loop carried on — and it carries NO `cmd`: `frob` was never a command
    // of the table, so nothing was ever started on this connection whose partial
    // result this `cancel` could discard, and only a command the interpreter
    // resolved is a candidate for "the command that the same connection most
    // recently sent" (see [`serve`]).
    assert!(events[1] == Event::Cancelled(None));
    assert!(terminals_of(&events) == 2);
}

#[test]
fn end_of_input_exits_cleanly_without_an_event_for_the_end_itself() {
    // The clause of plan item 3.4 that EOF is a clean exit, checked on both
    // shapes of it: a stream that ends AT a line feed, and an empty session.
    // Nothing is answered FOR the end of input — no `error`, no `result`, no
    // `cancelled` — and the loop RETURNS rather than waiting for more.
    let mut out = collecting();
    let mut input = scripted(["{\"v\":1,\"cmd\":\"list-pending\"}\n".to_string()].to_vec());
    let mut worker = wired_nowhere();
    let stopped = run(&mut input, &mut out, &mut worker);
    assert!(stopped == Stop::EndOfInput);
    let events = events_of(&out.frames);
    assert!(kinds_of(&events) == ["error".to_string()].to_vec());
    assert!(events[0] == Event::Error(Some("list-pending".to_string()), ErrorReason::Io, "the pending annotation links of plan item 6.3 are read from the store, which this build does not wire into the interpreter".to_string()));
    let mut empty = collecting();
    let mut nothing = scripted(Vec::<String>::new());
    let stopped_empty = run(&mut nothing, &mut empty, &mut worker);
    assert!(stopped_empty == Stop::EndOfInput);
    assert!(empty.frames.len() == 0);
}

#[test]
fn a_half_line_stays_pending_until_the_rest_of_it_arrives() {
    // "Unknown input" of `doc/architecture/core-protocol.md`: the stream never
    // resynchronizes on a partial line, it WAITS for the rest of it — which is
    // what [`proto::codec::feed`] and [`Framing`] do inside [`run`]: the two
    // chunks below are ONE line of the stream, so exactly one event answers it
    // and nothing in between is decoded or skipped.
    let mut out = collecting();
    let mut input = scripted(["{\"v\":1,\"cmd\":\"".to_string(), "cancel\"}\n".to_string()].to_vec());
    let mut worker = wired_nowhere();
    let stopped = run(&mut input, &mut out, &mut worker);
    assert!(stopped == Stop::EndOfInput);
    let events = events_of(&out.frames);
    assert!(kinds_of(&events) == ["cancelled".to_string()].to_vec());
    // Nothing was run before it, so there is no command whose partial result
    // was discarded, and the doc's `cancelled` event carries no `cmd` then.
    assert!(events[0] == Event::Cancelled(None));
}

#[test]
fn a_partial_line_left_at_end_of_input_exits_without_an_answer() {
    // The end of the input can never complete a half line, and the doc forbids
    // resynchronizing on it, so the tail is left EXACTLY as it is: not decoded,
    // not answered, not repaired, and the exit is clean. [`Stop.HalfLineAtEnd`]
    // carries the tail so the caller and the tests can tell the two clean ends
    // apart, which is the choice this item makes for that case.
    let mut out = collecting();
    let mut input = scripted(["{\"v\":1,\"cmd\":\"cancel\"}\n".to_string(), "{\"v\":1,\"cmd\":\"sh".to_string()].to_vec());
    let mut worker = wired_nowhere();
    let stopped = run(&mut input, &mut out, &mut worker);
    assert!(stopped == Stop::HalfLineAtEnd("{\"v\":1,\"cmd\":\"sh".to_string()));
    let events = events_of(&out.frames);
    assert!(kinds_of(&events) == ["cancelled".to_string()].to_vec());
    assert!(terminals_of(&events) == 1);
}
#[cfg(test)]
use serde_json::Map;
#[cfg(test)]
use serde_json::Value;

/// The `text` of every `error` event of a session, in the order they were written,
/// which is what lets a test check that the user is told WHICH phase wires the
/// verb it asked for (see the table in [`WiredNowhere`]).
#[cfg(test)]
fn texts_of(events: &Vec::<Event>) -> Vec::<String> {
    let mut texts = Vec::<String>::new();
    for event in events.iter() {
        match event {
            Event::Error(_, _, text) => { texts.push(text.to_string()); }
            _ => { }
        }
    }
    return texts;
}

#[test]
fn a_protocol_version_this_build_does_not_speak_answers_one_error_event_and_the_loop_carries_on() {
    // "Versioning" of `doc/architecture/core-protocol.md`: a line whose `v` is
    // higher than this build supports is refused with an `error` event; the
    // refusal is `proto`'s own typed `CodecError.UnsupportedVersion`, read here
    // like every other refusal, and the loop still reads the NEXT line, which
    // answers `cancel` with its own event.
    let mut out = collecting();
    let mut input = scripted(["{\"v\":9,\"cmd\":\"cancel\"}\n".to_string(), "{\"v\":1,\"cmd\":\"cancel\"}\n".to_string()].to_vec());
    let mut worker = wired_nowhere();
    let stopped = run(&mut input, &mut out, &mut worker);
    assert!(stopped == Stop::EndOfInput);
    let events = events_of(&out.frames);
    assert!(kinds_of(&events) == ["error".to_string(), "cancelled".to_string()].to_vec());
    assert!(events[0] == Event::Error(None, ErrorReason::UnknownCommand, "unsupported protocol version: 9".to_string()));
    assert!(terminals_of(&events) == 2);
}

#[test]
fn an_event_line_sent_by_the_interface_is_answered_and_never_panics() {
    // An event line is what the DAEMON writes ("Events"), never what the
    // interface sends, so it is not a command this interpreter can run: the
    // line is answered with the `error` event of "Unknown input" naming the
    // event it was given, the command AFTER it is still served, and nothing
    // panicked on the way.
    let mut out = collecting();
    let mut input = scripted(["{\"v\":1,\"event\":\"log\",\"level\":\"info\",\"text\":\"from the interface\"}\n".to_string(), "{\"v\":1,\"cmd\":\"list-pending\"}\n".to_string()].to_vec());
    let mut worker = wired_nowhere();
    let stopped = run(&mut input, &mut out, &mut worker);
    assert!(stopped == Stop::EndOfInput);
    let events = events_of(&out.frames);
    assert!(kinds_of(&events) == ["error".to_string(), "error".to_string()].to_vec());
    assert!(events[0] == Event::Error(None, ErrorReason::UnknownCommand, "the line is an event, which the interface does not send: log".to_string()));
    assert!(events[1] == Event::Error(Some("list-pending".to_string()), ErrorReason::Io, "the pending annotation links of plan item 6.3 are read from the store, which this build does not wire into the interpreter".to_string()));
    assert!(terminals_of(&events) == 2);
}

#[test]
fn every_frame_is_one_line_ending_in_a_line_feed_and_no_text_field_forges_another() {
    // The framing of "Transport": one JSON object per line, `\n` terminated. A
    // command name may hold a line break, a quote, braces and a tab, and the
    // `error` event `proto` writes for it still has to be ONE line of the stream
    // that reads back as ONE event — which is what the count of frames and of
    // the line feeds inside the frame checks, and what [`events_of`] would
    // catch by a frame that does not read back at all.
    let mut out = collecting();
    let mut input = scripted(["{\"v\":1,\"cmd\":\"fro\\nb\\\"{}\\tquux\"}\n".to_string()].to_vec());
    let mut worker = wired_nowhere();
    let stopped = run(&mut input, &mut out, &mut worker);
    assert!(stopped == Stop::EndOfInput);
    assert!(out.frames.len() == 1);
    let frame = out.frames[0].to_string();
    assert!(frame.ends_with("\n"));
    let mut breaks = 0;
    for character in frame.chars() {
        if character == '\n' {
            breaks = breaks + 1;
        }
    }
    assert!(breaks == 1);
    let events = events_of(&out.frames);
    assert!(events[0] == Event::Error(Some("fro\nb\"{}\tquux".to_string()), ErrorReason::UnknownCommand, "unknown command: fro\nb\"{}\tquux".to_string()));
}

/// The work of a phase that IS wired, which is how plan items 4.8-6.3 plug into
/// the [`Work`] seam: the command stream and the `total` of items belong to the
/// work (this interpreter cannot know how many files an import found), so such
/// work drives plan 3.3's [`writer::CommandStream`] itself — the `progress` lines
/// and all — and then tells the interpreter that it already ended the command,
/// [`Answered.EndedWith`], so the interpreter writes nothing more for it.
#[cfg(test)]
struct Importer {
    pub started: i64
}

#[cfg(test)]
fn importing() -> Importer {
    return Importer { started: 0 };
}

#[cfg(test)]
/// The empty payload of a `result` event: the doc's "remaining fields of the
/// line", none here, so the line carries `cmd` alone.
fn empty_payload() -> Map::<String, Value> {
    return Map::<String, Value>::new();
}

#[cfg(test)]
impl Work for Importer {
    fn do_command(&mut self, command: Command, sink: &mut dyn EventSink) -> Answered {
        self.started = self.started + 1;
        match command {
            Command::Import(path) => {
                let mut stream = writer::start("import".to_string(), 2);
                match writer::progress(&mut stream, sink, Some(path.to_string())) {
                    Err(why) => { return Answered::AnswerWith(ErrorReason::Io, refusal_text(why)); }
                    Ok(_) => { }
                }
                match writer::progress(&mut stream, sink, Some(path.to_string())) {
                    Err(why) => { return Answered::AnswerWith(ErrorReason::Io, refusal_text(why)); }
                    Ok(_) => { }
                }
                match writer::succeed(&mut stream, sink, &empty_payload()) {
                    Err(why) => { return Answered::AnswerWith(ErrorReason::Io, refusal_text(why)); }
                    Ok(_) => { return Answered::EndedWith; }
                }
            }
            _ => {
                return Answered::AnswerWith(ErrorReason::Busy, "the work of this test answers the import command only".to_string());
            }
        }
    }
}

#[test]
fn several_commands_in_a_row_each_get_exactly_one_terminal_event() {
    // The "exactly one" rule of "Events" over a SESSION rather than over one
    // command: two commands driven by the work seam of a wired phase end with
    // their `result`, and the `cancel` line ends with its `cancelled` — so
    // the terminal events of the session are exactly THREE for three commands,
    // each one the last event of its own command, while the `progress` lines
    // of the wired work appear any number of times IN BETWEEN.
    let mut out = collecting();
    let mut input = scripted(["{\"v\":1,\"cmd\":\"import\",\"path\":\"bank_a.pdf\"}\n".to_string(), "{\"v\":1,\"cmd\":\"import\",\"path\":\"bank_b.pdf\"}\n".to_string(), "{\"v\":1,\"cmd\":\"cancel\"}\n".to_string()].to_vec());
    let mut worker = importing();
    let stopped = run(&mut input, &mut out, &mut worker);
    assert!(stopped == Stop::EndOfInput);
    assert!(worker.started == 2);
    let events = events_of(&out.frames);
    assert!(kinds_of(&events) == ["progress".to_string(), "progress".to_string(), "result".to_string(), "progress".to_string(), "progress".to_string(), "result".to_string(), "cancelled".to_string()].to_vec());
    assert!(terminals_of(&events) == 3);
    assert!(events[2] == Event::Result("import".to_string(), empty_payload()));
    assert!(events[6] == Event::Cancelled(Some("import".to_string())));
    let mut plain = collecting();
    let mut idle = scripted(Vec::<String>::new());
    let stopped_idle = run(&mut idle, &mut plain, &mut worker);
    assert!(stopped_idle == Stop::EndOfInput);
    assert!(plain.frames.len() == 0);
}

#[test]
fn every_command_of_the_table_is_refused_with_a_documented_code_naming_its_phase() {
    // The whole vocabulary of plan 3.2 run through this build's [`Work`]
    // implementation: each verb answers ONE terminal event, its `code` is one of
    // the FIVE codes `doc/architecture/core-protocol.md` ("Error") names — read
    // back as the text the interface sees, so no code of this module's invention
    // can slip in — and the message names the plan item that wires the verb,
    // which is the seam's own table (see [`WiredNowhere`]). The two
    // alternatives of `confirm-annotation` are both sent, since the row of the
    // command table writes them as `id` | `reject`.
    let mut out = collecting();
    let mut input = scripted(["{\"v\":1,\"cmd\":\"version\",\"nonce\":\"6f2c1a48e0b7d9c3\"}\n".to_string(), "{\"v\":1,\"cmd\":\"import\",\"path\":\"/data/statements/2024/bank_a.pdf\"}\n".to_string(), "{\"v\":1,\"cmd\":\"report\",\"format\":\"html\",\"year\":2024}\n".to_string(), "{\"v\":1,\"cmd\":\"report\",\"format\":\"ods\"}\n".to_string(), "{\"v\":1,\"cmd\":\"set-category\",\"id\":\"3f9a1c7e\",\"category\":\"food:supermarket\",\"reason\":\"user\"}\n".to_string(), "{\"v\":1,\"cmd\":\"confirm-annotation\",\"annotation\":\"ann00000001\",\"id\":\"3f9a1c7e\"}\n".to_string(), "{\"v\":1,\"cmd\":\"confirm-annotation\",\"annotation\":\"ann00000002\",\"reject\":true}\n".to_string(), "{\"v\":1,\"cmd\":\"list-pending\"}\n".to_string()].to_vec());
    let mut worker = wired_nowhere();
    let stopped = run(&mut input, &mut out, &mut worker);
    assert!(stopped == Stop::EndOfInput);
    let events = events_of(&out.frames);
    assert!(kinds_of(&events) == ["error".to_string(), "error".to_string(), "error".to_string(), "error".to_string(), "error".to_string(), "error".to_string(), "error".to_string(), "error".to_string()].to_vec());
    assert!(terminals_of(&events) == 8);
    assert!(codes_of(&events) == ["unknown-command".to_string(), "plugin-failed".to_string(), "plugin-failed".to_string(), "plugin-failed".to_string(), "io".to_string(), "io".to_string(), "io".to_string(), "io".to_string()].to_vec());
    assert!(blamed_of(&events) == ["version".to_string(), "import".to_string(), "report".to_string(), "report".to_string(), "set-category".to_string(), "confirm-annotation".to_string(), "confirm-annotation".to_string(), "list-pending".to_string()].to_vec());
    let texts = texts_of(&events);
    // Every answer says WHICH plan item wires the verb, so the interface is told
    // what is missing rather than being left without an answer.
    for index in 0..texts.len() {
        let text = texts[index].to_string();
        if index == 0 { assert!(text.contains("3.5")); }
        if index == 1 { assert!(text.contains("4.8")); }
        if index == 2 || index == 3 { assert!(text.contains("5.1-5.8")); }
        if index == 4 { assert!(text.contains("6.2")); }
        if index == 5 || index == 6 || index == 7 { assert!(text.contains("6.3")); }
    }
    // The `format` the interface asked to report in IS echoed back, and the
    // optional `year` deliberately is not (see [`WiredNowhere`]).
    assert!(texts[2].contains("html"));
    assert!(texts[3].contains("ods"));
    assert!(texts[2].contains("2024") == false);
}

/// The sink of a test that FAILS on the `fails_at`-th line written to it, which
/// is the shape of the sink `core/src/main.rs` ships for stdout, where a failed
/// write surfaces at the flush of that line: it records its FIRST failure, and
/// every later line of the session is DROPPED, because plan 3.3's writer has
/// already counted the frame and marked the command terminal without ever
/// observing the sink (see the doc of [`writer::EventSink`]).
#[cfg(test)]
struct Breaking {
    pub frames: Vec::<String>,
    pub attempts: i64,
    pub fails_at: i64,
    pub broken: bool,
    pub dropped: i64
}

#[cfg(test)]
fn breaking(fails_at: i64) -> Breaking {
    return Breaking { frames: Vec::<String>::new(), attempts: 0, fails_at: fails_at, broken: false, dropped: 0 };
}

#[cfg(test)]
impl EventSink for Breaking {
    fn write_line(&mut self, line: String) {
        self.attempts = self.attempts + 1;
        if self.broken {
            self.dropped = self.dropped + 1;
        } else {
            if self.attempts == self.fails_at {
                self.broken = true;
            } else {
                self.frames.push(line.to_string());
            }
        }
    }
}

#[test]
fn a_sink_that_fails_latches_and_drops_every_later_line_of_the_session() {
    // The obligation plan item 3.3 put on the sink of plan item 3.4, checked on
    // the same shape the daemon ships: the SECOND line of a three line session
    // fails, and the THIRD is DROPPED rather than written under a line the
    // interface never received — while the loop of [`run`] still reads the whole
    // session to the end, because a sink failure is the sink's own to remember
    // and the writer cannot see it.
    let mut out = breaking(2);
    let mut input = scripted(["{\"v\":1,\"cmd\":\"list-pending\"}\n".to_string(), "{\"v\":1,\"cmd\":\"list-pending\"}\n".to_string(), "{\"v\":1,\"cmd\":\"list-pending\"}\n".to_string()].to_vec());
    let mut worker = wired_nowhere();
    let stopped = run(&mut input, &mut out, &mut worker);
    assert!(stopped == Stop::EndOfInput);
    assert!(out.attempts == 3);
    assert!(out.broken == true);
    assert!(out.dropped == 1);
    assert!(out.frames.len() == 1);
    let events = events_of(&out.frames);
    assert!(events[0] == Event::Error(Some("list-pending".to_string()), ErrorReason::Io, "the pending annotation links of plan item 6.3 are read from the store, which this build does not wire into the interpreter".to_string()));
}
