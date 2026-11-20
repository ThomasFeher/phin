//! The core side of the event stream of the core protocol — plan item 3.3.
//!
//! `doc/architecture/core-protocol.md` ("Events", accepted): the interface
//! consumes `progress`, `log`, `result`, `error` and `cancelled`; the core answers
//! one command with exactly one `result` (or `error` / `cancelled`) and `progress`
//! and `log` may appear any number of times in between. This module owns that
//! framing: [`CommandStream`] is the state of the event stream of ONE running
//! command — the command name, the item counter behind the `done`/`total` numbers,
//! the cancel flag of "Cancellation" and whether the terminal event has been
//! written — and [`run`] drives the item loop, calling the caller's work for one
//! item at a time and checking the cancel flag BETWEEN the items, so a cancelled
//! command finishes the current item, discards the partial result and answers
//! `cancelled` instead of a `result`.
//!
//! Framing and the JSON text of every line come from `proto`
//! ([`proto::codec::encode_frame`] over [`proto::event::Event`]), so the wire form
//! of an event is written nowhere else in core: an event the doc does not define
//! cannot be produced here, and a `result` payload is the free map of
//! [`proto::event::Event::Result`], whose reserved keys `proto` refuses with
//! [`proto::codec::CodecError.ReservedField`]. Nothing here is compared byte for
//! byte with a doc example: without the `preserve_order` feature serde_json writes
//! the keys of a frame in lexicographic order, so the tests of this module decode
//! the frames they emitted back with [`proto::codec::decode_line`] and check the
//! typed [`proto::event::Event`].
//!
//! The module performs NO I/O — no file, no socket, no stdout — so it is unit-testable
//! without starting the core (plan rule 4). Where the frames go is the
//! [`EventSink`] the caller injects: plan item 3.4 ("write events to stdout, flush
//! per line") and the transport of plan item 3.6 hand in their own sink, and the
//! tests of this module collect the lines. A sink is injected rather than an
//! internal buffer because the doc wants the interface to see progress WHILE the
//! command runs, which a buffer that is flushed only when the command ends cannot
//! do. Everything that can go wrong is a typed [`WriteError`], never a panic: NOTHING
//! here unwraps a `Result` or an `Option`, and NO number is forced through a cast.
//!
//! Two rules of the doc are held by the state of [`CommandStream`] rather than by
//! a caller's diligence. The terminal event of a command is remembered, so a SECOND
//! one is [`WriteError.AlreadyFinished`] and after it no event of any kind reaches
//! the sink; and once a cancellation was requested the only event still allowed is
//! the `cancelled` event itself — anything else is [`WriteError.CancelRequested`] —
//! which also says that an item the work finished off AFTER the interface asked to
//! stop is not counted and gets no further `progress` line.
//!
//! The third number a command stream holds is the `total` promised at [`start`],
//! which arrives independently of the items [`run`] is handed; nothing at [`start`]
//! can cross-check them, so this module does it where both are known: an item list
//! longer than `total` is refused with [`WriteError.TotalPassed`] BEFORE the first
//! item of [`run`], and so is the item of a directly driven [`progress`] whose
//! `done` number would pass `total`. A `total` LARGER than the items is allowed —
//! an item never started reports nothing — and only that cross-check keeps the
//! `done`/`total` pair of a stream from reporting more items than were promised.
//!
//! A codec refusal is returned, never swallowed: [`run`] then leaves the command
//! OPEN, because the codec builds the whole line BEFORE handing anything over, so
//! no half line is on the stream, and the interpreter of plan item 3.4 ends such a
//! command with the `error` event of [`WriteError.Codec`], an event that carries no
//! free payload and therefore always frames.

use proto::codec::CodecError;
use proto::codec::encode_frame;
use proto::codec::Message;
use proto::event::ErrorReason;
use proto::event::Event;
use serde_json::Map;
use serde_json::Value;

/// Where the frames of one event stream are handed over.
///
/// One method, because a frame of the protocol is already a complete, `\n`-terminated
/// line when the writer hands it over ([`proto::codec::encode_frame`] writes the
/// terminator), so the only thing a stream can be asked for is "take this line, in
/// this order". The sink of plan item 3.4 writes each line to stdout and flushes
/// per line — which is what lets the interface watch `progress` WHILE a command runs —
/// and the sink of the transport of plan item 3.6 writes it to the connection it
/// opened; the tests of this module collect the lines, which is what makes the writer
/// unit-testable without starting the core (plan rule 4); plan item 3.6 is the one
/// that supplies the socket-backed sink of the transport.
///
/// [`EventSink::write_line`] returning nothing is NOT a claim that the write worked
/// out: a sink failure cannot be reported to this module at all, so the writer
/// commits its `progress` counter and marks the command terminal for a frame that
/// may never have reached the interface. An implementation therefore MUST record
/// its own write failure and refuse or drop every later line of the stream, because
/// the writer cannot observe the failure and will already have considered the
/// command finished. That obligation becomes live with plan item 3.4, whose sink
/// lives in `interpret.rs` and writes to a stream the interface may close in the
/// middle of a command; the same obligation applies to the sink of item 3.6.
pub trait EventSink {
    /// Write one complete frame.
    fn write_line(&mut self, line: String);
}

/// Which terminal event of `doc/architecture/core-protocol.md` ("Events") a command
/// stream ended with, and [`Terminal.Open`] while it is still running. The
/// "exactly one" rule of that section is held by this field: every terminal event
/// of [`CommandStream`] is refused once the stream is no longer open, and every
/// event at all is refused once it ended.
#[derive(Debug, Eq, PartialEq, Clone, Copy)]
pub enum Terminal {
    /// The command is running: no terminal event written yet. This value never
    /// escapes as the payload of [`WriteError.AlreadyFinished`], which [`refused`]
    /// produces only from a stream that is no longer open, handing over the terminal
    /// event actually written and never this one; [`Terminal.Open`] is the state of
    /// [`CommandStream`] and the answer of [`terminal_of`] for a `progress` or a
    /// `log` event, so the meaningless "already finished with nothing" error is
    /// unreachable from any path of this module, though it stays constructible
    /// from outside it.
    Open,
    /// Ended with the `result` event.
    Result,
    /// Ended with the `error` event.
    Error,
    /// Ended with the `cancelled` event.
    Cancelled,
}

/// What the work of ONE item came down to. The item loop of [`run`] turns this
/// into the events of the stream, so the work itself stays outside this module
/// (plan item 3.4) while the framing of the stream stays inside it.
#[derive(Debug, Eq, PartialEq)]
pub enum ItemOutcome {
    /// The item was done. The text names the file it worked on for the `file`
    /// field of the `progress` event ("Events": the field is present only when the
    /// command processes files), [`None`] when it did not work on a file.
    Done(Option::<String>),
    /// The command cannot finish: [`run`] ends the stream with the `error` event
    /// of the draft code of `doc/architecture/core-protocol.md` ("Error") and the
    /// message, and no further item is started.
    Failed(ErrorReason, String),
    /// A `cancel` command of the interface ("Cancellation") was seen while this
    /// item was being worked on: the current item is finished off, the partial
    /// result is discarded and the stream answers `cancelled`.
    Cancelled,
}

/// How far the item loop of [`run`] got, which is the outcome of the command: the stream
/// is already terminal when this is returned, so a caller has nothing left to
/// write; the number is how many items were completed before the stream ended.
#[derive(Debug, Eq, PartialEq)]
pub enum RunOutcome {
    /// All items were done and the stream ended with the `result` event.
    Completed(i64),
    /// Cancelled between the items: the stream ended with the `cancelled` event
    /// and no `result` was written ("Cancellation": nothing is half written,
    /// because the command discards the partial result).
    Cancelled(i64),
    /// An item failed: the stream ended with the `error` event. The code and the
    /// message are the ones the work itself chose (see [`ItemOutcome.Failed`]), so
    /// they are not repeated here.
    Failed,
}

/// What can stop an event from being written. Every failure is typed; this module
/// has NO failing path that panics, so a broken event never takes the daemon down
/// with it ("Unknown input" of `doc/architecture/core-protocol.md`: the daemon
/// stays alive).
///
/// Naming note: this type and `core::technical::fs_atomic::WriteError` are two
/// DIFFERENT types with the same name. Plan item 3.4 (`interpret.rs`) is the first
/// module likely to need both — it writes data files and it writes events — and so
/// must alias one of them at its `use` lines instead of importing both bare.
#[derive(Debug, Eq, PartialEq)]
pub enum WriteError {
    /// The stream is still running, but a `cancel` command was already seen, so
    /// the `cancelled` event is the only event that may still be written
    /// ("Cancellation"): a `result`, an `error`, a further `progress` or a further
    /// `log` after the cancel would tell the interface the command is still going.
    CancelRequested,
    /// The stream already ended, so this second event would break the "exactly
    /// one" rule of `doc/architecture/core-protocol.md` ("Events"); the terminal
    /// event already written is carried, so a caller's own bug is diagnosable
    /// without reading the stream back.
    AlreadyFinished(Terminal),
    /// `proto` refused to frame the event, [`proto::codec::CodecError.ReservedField`]
    /// being the realistic case: a payload key named `v`, `event` or `cmd` would
    /// be overwritten by the framing of the line, so the frame is refused instead
    /// of silently losing the field. The codec's own error is kept, NOT swallowed:
    /// the interpreter of plan item 3.4 turns it into the `text` of an `error` event
    /// with [`proto::codec::error_text`], and a refused emission does NOT end the
    /// command, so the terminal event can still be written by the caller.
    Codec(CodecError),
    /// The command was asked to report more items than the `total` promised at
    /// [`start`] allows: [`run`] refuses an item list longer than `total` BEFORE
    /// working on its first item, and [`progress`] refuses the item whose `done`
    /// number would pass `total`, which is what keeps the claim of [`progress`] that
    /// the numbers of a stream cannot pass `total` true by construction even though
    /// [`start`] and [`run`] take `total` and the items as independent inputs.
    /// The payload is the promised `total`; the stream is left [`Terminal.Open`],
    /// like a refused frame, so the caller can still end the command properly.
    TotalPassed(i64),
}

/// The event stream of ONE running command.
///
/// It carries the name of the command, because the `result`, `error` and
/// `cancelled` events of `doc/architecture/core-protocol.md` all name it, and the
/// item counter behind the `done` of `total` `progress` numbers. The counter is
/// private and is raised by no path other than [`progress`] itself, which raises
/// it by one ONLY AFTER a frame was accepted, so a `progress` event cannot report
/// the same number twice or go backwards; and it cannot pass `total` either, which
/// is NOT a property of the counter alone — `total` is set at [`start`] and the
/// items at [`run`], two independent inputs — but of the cross-check of those two
/// in [`run`] and the check of the counter against `total` in [`progress`], both
/// answering [`WriteError.TotalPassed`]; a command with no work items writes no
/// `progress` event at all, which the doc allows ("any number of times in between").
#[derive(Debug, Eq, PartialEq)]
pub struct CommandStream {
    /// The `cmd` field of every event written for this command.
    cmd: String,
    /// The number of items the command has to work on.
    total: i64,
    /// How many items have been done, and therefore the `done` number of the
    /// next `progress` event.
    done: i64,
    /// Whether a `cancel` command was seen for this command.
    cancel_requested: bool,
    /// How the command has ended, [`Terminal.Open`] while it runs.
    terminal: Terminal,
}

/// Start the event stream of one command.
///
/// `total` is the number of items the command will be given; a command that works
/// on nothing is started with a `total` of `0`, which writes no
/// `progress` event, and one item is enough for the non-file commands of the
/// command table. `cmd` is the name the terminal events of the stream carry.
///
/// The rule about `total`: a stream may report FEWER items than it promised —
/// nothing a promise stops is wrong — but never MORE. `total` and the items handed
/// to [`run`] are independent inputs, so this rule is NOT held here but enforced
/// where both meet: [`run`] refuses a `total` smaller than the item list with
/// [`WriteError.TotalPassed`], and [`progress`] refuses the same mistake when a
/// caller drives the loop of a command itself.
pub fn start(cmd: String, total: i64) -> CommandStream {
    return CommandStream { cmd: cmd.to_string(), total: total, done: 0, cancel_requested: false, terminal: Terminal::Open };
}

/// Request the cancellation of this command ("Cancellation").
///
/// The request is remembered, NOT acted on here: the running item is finished off
/// first, because the doc requires that the core "finishes the current item, discards
/// the partial result and answers `cancelled`". [`run`] looks at the flag BETWEEN the
/// items, which is where a `cancel` command of plan item 3.4 lands in a single
/// threaded daemon; a work implementation that notices the cancel itself says so with
/// [`ItemOutcome.Cancelled`], and a cancel requested before the command was even
/// started stops it before its first item.
pub fn request_cancel(stream: &mut CommandStream) {
    stream.cancel_requested = true;
}

/// Whether a cancellation has been requested for this command.
pub fn cancel_requested(stream: &CommandStream) -> bool {
    return stream.cancel_requested;
}

/// Write the `progress` event of the item just done, and raise the item counter.
///
/// `done` is THIS command's counter, so the numbers of one stream cannot repeat,
/// go backwards or pass `total` — they are not supplied by the caller, which is
/// what makes the invariant hold by construction: the number written is always
/// `done` plus one, and the counter is raised only AFTER a frame the
/// sink accepted, so a refused event leaves it where it was. The one thing the
/// counter cannot decide on its own is an item BEYOND the `total` promised at
/// [`start`], and it refuses that one as well, with [`WriteError.TotalPassed`],
/// rather than writing a `done` number greater than the `total` of the line. `file`
/// is the optional `file` field of the doc's progress example; the `cmd` field names
/// this command.
pub fn progress(stream: &mut CommandStream, sink: &mut dyn EventSink, file: Option::<String>) -> Result::<i64, WriteError> {
    let done_now = stream.done + 1;
    // The state of the stream is judged by [`refused`] inside [`frame_of`], and it
    // comes first here too: an ended command or a seen `cancel` is reported as
    // such even when the item would ALSO have passed `total`. What the counter
    // cannot judge on its own is an item BEYOND the `total` promised at [`start`],
    // which is refused here instead of writing a `done` number greater than the
    // `total` of the line.
    if stream.terminal == Terminal::Open && !stream.cancel_requested && done_now > stream.total {
        return Err(WriteError::TotalPassed(stream.total));
    }
    let event = Event::Progress(stream.cmd.to_string(), done_now, stream.total, file);
    match frame_of(stream, event, sink) {
        Err(reason) => { return Err(reason); }
        Ok(()) => { stream.done = done_now; return Ok(done_now); }
    }
}

/// Write one `log` event of this command ("Events": any number of them in between).
///
/// `level` is the doc's `level` field (`info` in the doc's example) and `text` what
/// the user is shown; the text of one log can never forge a second line, because
/// [`proto::codec::encode_frame`] writes exactly one JSON object per line.
pub fn log(stream: &mut CommandStream, sink: &mut dyn EventSink, level: String, text: String) -> Result::<(), WriteError> {
    return frame_of(stream, Event::Log(level.to_string(), text.to_string()), sink);
}

/// End the command with its `result` event: the terminal event of a command that
/// worked (`doc/architecture/core-protocol.md`, "Events": exactly one `result`, or
/// `error` / `cancelled`, per command).
///
/// `cmd` names this command and `payload` holds the remaining top-level fields of
/// the line, which is how the doc's examples come out (`"cmd":"set-category",
/// "affected":183`); an empty payload writes the `cmd` field only. A payload key the
/// framing of a line also writes for itself is refused by `proto` with
/// [`WriteError.Codec`] and leaves the command open, so the terminal event can
/// still be written once the payload is fixed.
pub fn succeed(stream: &mut CommandStream, sink: &mut dyn EventSink, payload: &Map::<String, Value>) -> Result::<(), WriteError> {
    return frame_of(stream, Event::Result(stream.cmd.to_string(), payload.clone()), sink);
}

/// End the command with its `error` event.
///
/// The line carries the `cmd` field naming the command that failed — this is why
/// [`CommandStream`] holds the command name at all, and why a failed item cannot be
/// reported by a caller that lost it — plus the `code` of the draft list of
/// `doc/architecture/core-protocol.md` ("Error": `unknown-command`,
/// `plugin-failed`, `converter-missing`, `io`, `busy`) and the `text` shown to the
/// user, both of which the work of [`CommandWork`] decides.
pub fn fail(stream: &mut CommandStream, sink: &mut dyn EventSink, reason: ErrorReason, text: String) -> Result::<(), WriteError> {
    return frame_of(stream, Event::Error(Some(stream.cmd.to_string()), reason, text.to_string()), sink);
}

/// End the command with its `cancelled` event.
///
/// The one event still allowed once a `cancel` command was seen ("Cancellation"): the
/// `cmd` field names the command whose partial result was discarded. The command
/// name is always known here, so the event always carries it, unlike an `error`
/// event for a line whose command was never resolved.
pub fn cancel(stream: &mut CommandStream, sink: &mut dyn EventSink) -> Result::<(), WriteError> {
    return frame_of(stream, Event::Cancelled(Some(stream.cmd.to_string())), sink);
}

/// The per-item work of a command, supplied by the caller so that this module owns
/// only the framing of the event stream and never the work itself.
///
/// A [`CommandStream`] is handed to the item so that work which notices a `cancel`
/// command of the interface can ask for the cancellation itself
/// ([`request_cancel`]) or show the user something ([`log`]) through the same
/// invariants, the same ones every other event of this command goes through; it is
/// not a way to bypass them, because the stream answers every event it refuses with
/// a typed [`WriteError`] instead.
pub trait CommandWork {
    /// Work on the item `item` and say what it came down to.
    ///
    /// The item is handed the stream AND the sink of the command, which is how
    /// work shows the user something while it is busy: it calls [`log`] of the
    /// module, so its own message goes through the same rules (no `log` after the
    /// command ended or was cancelled) instead of being pushed at the sink directly.
    ///
    /// The outcome is deliberately NOT a per-item value: [`run`] keeps only the
    /// [`ItemOutcome`] and the optional `file` name of an item and throws away
    /// anything else it produced, which is the boundary that keeps a command whose
    /// `result` summarizes its items out of [`run`] until plan item 3.4 widens this
    /// method (or drives the loop through [`start`], [`progress`], [`log`] and
    /// [`succeed`] itself).
    fn do_item(&mut self, stream: &mut CommandStream, sink: &mut dyn EventSink, item: String) -> ItemOutcome;
}

/// Run the item loop of one command, ending its event stream.
///
/// The command is ended here, by the stream and NOT by the caller, so the
/// "exactly one terminal event per command" rule of
/// `doc/architecture/core-protocol.md` ("Events") cannot be forgotten on one path:
/// every item is done in order, `cancel` is looked for BETWEEN the items
/// ("Cancellation": the item already started is finished off, the rest never started),
/// an item that fails ends the command with the `error` event and no further item is
/// started, and running out of items ends it with the `result` event of `payload`.
/// A cancelled command therefore writes no `result`, and every path returns the
/// [`RunOutcome`] of the command, whose stream is already terminal. An item the
/// work finished off AFTER the cancel arrived is neither counted nor followed by a
/// `progress` line, because the doc allows no further `progress` line then, so the
/// last number the interface saw is the last item it may still believe in.
///
/// The boundary of what [`run`] can express, and what plan item 3.4 works around:
/// the item loop collects NO per-item value and takes `payload` immutably up front,
/// so a command whose `result` summarizes the items it worked on — the `affected`
/// of `set-category`, the `path`/`format`/`year` of `report`, the written path of
/// `import`, per `doc/architecture/core-protocol.md` lines 143-176 — cannot be
/// driven through it yet. Item 3.4 will either extend [`CommandWork::do_item`] to
/// hand back per-item data, or drive such a command through [`start`],
/// [`progress`], [`log`] and [`succeed`] with [`cancel_requested`] checked between
/// the items, which is why those remain public.
pub fn run(stream: &mut CommandStream, sink: &mut dyn EventSink, items: &Vec::<String>, payload: &Map::<String, Value>, work: &mut dyn CommandWork) -> Result::<RunOutcome, WriteError> {
    // `total` was set at [`start`] and `items` arrives here, so this is the only
    // place both are known, and nothing else keeps them consistent. A `total`
    // smaller than the item list would otherwise let the counter of the stream
    // report more items than were promised, so the mismatch is refused BEFORE the
    // first item is started, with nothing written and the stream left
    // [`Terminal.Open`]. The two counts are walked down together because this
    // module turns no length into a protocol number and no number into a length.
    let mut promised = stream.total;
    let mut left = items.len();
    while promised > 0 && left > 0 { promised = promised - 1; left = left - 1; }
    if left > 0 {
        return Err(WriteError::TotalPassed(stream.total));
    }
    for index in 0..items.len() {
        if cancel_requested(stream) {
            break;
        }
        match work.do_item(stream, sink, items[index].to_string()) {
            ItemOutcome::Done(file) => {
                if cancel_requested(stream) {
                    // The `cancel` command arrived WHILE this item was worked on:
                    // "Cancellation" says the current item is finished off, and
                    // "Events" allows no further `progress` line once a cancel was
                    // seen, so the item is neither reported nor counted, and the
                    // stream is ended with the `cancelled` event below.
                    break;
                }
                match progress(stream, sink, file) {
                    Err(why) => { return Err(why); }
                    Ok(_) => { }
                }
            }
            ItemOutcome::Cancelled => {
                request_cancel(stream);
                break;
            }
            ItemOutcome::Failed(reason, text) => {
                match fail(stream, sink, reason, text) {
                    Err(why) => {
                        if why == WriteError::CancelRequested {
                            // The interface asked to cancel and the work then reported
                            // a failure: "Cancellation" answers `cancelled`, so the
                            // `error` event is skipped and the after-loop path below
                            // closes the stream, which keeps the "exactly one terminal
                            // event per command" rule on this path too.
                            break;
                        }
                        return Err(why);
                    }
                    Ok(()) => { return Ok(RunOutcome::Failed); }
                }
            }
        }
    }
    if cancel_requested(stream) {
        match cancel(stream, sink) {
            Err(why) => { return Err(why); }
            Ok(()) => { return Ok(RunOutcome::Cancelled(stream.done)); }
        }
    }
    match succeed(stream, sink, payload) {
        Err(why) => { return Err(why); }
        Ok(()) => { return Ok(RunOutcome::Completed(stream.done)); }
    }
}

/// Whether one event may still be written to this command stream at all; it
/// returns [`None`] when the event may be written.
///
/// [`WriteError.AlreadyFinished`] once the terminal event of the command has been
/// written — which is what turns a second `result`, `error` or `cancelled` into a
/// typed error instead of a second line on the stream — and
/// [`WriteError.CancelRequested`] once a `cancel` was seen AND the event asked for
/// is not the `cancelled` event itself, because "Cancellation" allows only that one
/// then.
fn refused(stream: &CommandStream, event: &Event) -> Option::<WriteError> {
    if stream.terminal != Terminal::Open {
        return Some(WriteError::AlreadyFinished(stream.terminal));
    }
    if stream.cancel_requested && terminal_of(event) != Terminal::Cancelled {
        return Some(WriteError::CancelRequested);
    }
    return None;
}

/// Which terminal event one event would end the command with, [`Terminal.Open`] for
/// a `progress` or a `log` event. This is what lets [`frame_of`] close the stream
/// itself, so the terminal event of a command is remembered WITHOUT a second rule
/// that every caller has to remember on top of writing the event.
fn terminal_of(event: &Event) -> Terminal {
    match event {
        Event::Result(_, _) => { return Terminal::Result; }
        Event::Error(_, _, _) => { return Terminal::Error; }
        Event::Cancelled(_) => { return Terminal::Cancelled; }
        _ => { return Terminal::Open; }
    }
}

/// Encode one event into a frame with `proto` and hand the frame to `sink`.
///
/// The codec is the only thing that can refuse a frame — of the events this module
/// writes, only a `result` payload key named `v`, `event` or `cmd` can be refused
/// ([`proto::codec::CodecError.ReservedField`]) — and its own error is returned in
/// [`WriteError.Codec`], never swallowed and never half written to the stream: `encode_frame`
/// builds the whole line BEFORE anything is handed over, so a refused event leaves the
/// stream unchanged and the command can still be ended properly.
fn frame_of(stream: &mut CommandStream, event: Event, sink: &mut dyn EventSink) -> Result::<(), WriteError> {
    match refused(stream, &event) {
        Some(blocked) => { return Err(blocked); }
        None => { }
    }
    let ended = terminal_of(&event);
    let message = Message::Outbound(event);
    match encode_frame(&message) {
        Err(reason) => { return Err(WriteError::Codec(reason)); }
        Ok(line) => { sink.write_line(line); stream.terminal = ended; return Ok(()); }
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
use proto::codec::decode_line;
#[cfg(test)]
use proto::event::event_kind;

/// The frames one event stream wrote, collected instead of written to a stream.
///
/// The collected lines are ALL a test needs: [`events_of`] reads them back with
/// `proto` into the typed events the interface would see, so a test checks protocol
/// values rather than a hand-built JSON string, and never the order of the keys of
/// a frame, which `serde_json` writes lexicographically (so a frame of this module
/// is equal to a `doc/architecture/core-protocol.md` example as a JSON value, but
/// the comparison of a cross-check against the doc is deferred to the doc-reading
/// tests of plan item 3.4 rather than a third copy of that marker-scanning helper).
#[cfg(test)]
struct Collected {
    frames: Vec::<String>,
}

#[cfg(test)]
impl EventSink for Collected {
    /// Remember the frame of one event, rather than writing it to a stream.
    fn write_line(&mut self, line: String) {
        self.frames.push(line.to_string());
    }
}

#[cfg(test)]
fn collector() -> Collected {
    return Collected { frames: Vec::<String>::new() };
}

/// The payload fields of one test's `result` event, written as the remaining top-level
/// fields of the line besides `cmd` ("Events"): [`payload_of`] of `-1` asks for the
/// EMPTY payload, which writes the command name only, and any other number writes
/// the `affected` field of the doc's `set-category` example.
#[cfg(test)]
fn payload_of(affected: i64) -> Map::<String, Value> {
    let mut payload = Map::<String, Value>::new();
    if affected >= 0 {
        payload.insert("affected".to_string(), number_of(affected));
    }
    return payload;
}

#[cfg(test)]
fn number_of(value: i64) -> Value {
    return Value::Number(serde_json::Number::from(value));
}

/// The `file` items of a command, named as the doc's progress example names them.
#[cfg(test)]
fn items_named(count: i64) -> Vec::<String> {
    let mut items = Vec::<String>::new();
    for index in 1..count + 1 {
        items.push(format!("file{index}.pdf"));
    }
    return items;
}

/// The events of the collected frames, decoded with `proto`.
///
/// Every frame must read back as exactly one event of the protocol, which is what
/// the last assertion says: a frame the interface cannot read is a bug of this
/// module, and a `log` text that forged a second line would show up here as one
/// event short of the collected frames.
#[cfg(test)]
fn events_of(collected: &Collected) -> Vec::<Event> {
    let mut events = Vec::<Event>::new();
    for index in 0..collected.frames.len() {
        let read = decode_line(&collected.frames[index]);
        match read {
            Ok(Message::Outbound(event)) => { events.push(event); }
            _ => { }
        }
    }
    assert!(events.len() == collected.frames.len());
    return events;
}

/// The `event` name of every collected event, in the order they were written.
#[cfg(test)]
fn kind_names(events: &Vec::<Event>) -> Vec::<String> {
    let mut names = Vec::<String>::new();
    for index in 0..events.len() {
        names.push(event_kind(&events[index]).to_string());
    }
    return names;
}

/// How many of the event names are the one asked for.
#[cfg(test)]
fn counted(names: &Vec::<String>, wanted: &str) -> i64 {
    let mut found = 0;
    for name in names.iter() {
        if name.to_string() == wanted.to_string() {
            found = found + 1;
        }
    }
    return found;
}

/// How many events of the stream are terminal events ("Events": `result`, `error`,
/// `cancelled`), which is what the "exactly one" rule is checked by.
#[cfg(test)]
fn counted_terminals(events: &Vec::<Event>) -> i64 {
    let mut found = 0;
    for index in 0..events.len() {
        match events[index] {
            Event::Result(_, _) => { found = found + 1; }
            Event::Error(_, _, _) => { found = found + 1; }
            Event::Cancelled(_) => { found = found + 1; }
            _ => { }
        }
    }
    return found;
}

/// The position of the first terminal event of the stream, `-1` when it has none,
/// which is how a test says that a command was NOT ended before its loop ended.
#[cfg(test)]
fn first_terminal(events: &Vec::<Event>) -> i64 {
    let mut position = 0;
    for event in events.iter() {
        match event {
            Event::Result(_, _) => { return position; }
            Event::Error(_, _, _) => { return position; }
            Event::Cancelled(_) => { return position; }
            _ => { }
        }
        position = position + 1;
    }
    return -1;
}

/// The `done` numbers and the `total` numbers of every `progress` event of a stream,
/// in the order they were written, which is what the monotonicity of a command is
/// checked on.
#[cfg(test)]
fn progress_numbers(events: &Vec::<Event>) -> (Vec::<i64>, Vec::<i64>) {
    let mut done = Vec::<i64>::new();
    let mut total = Vec::<i64>::new();
    for event in events.iter() {
        match event {
            Event::Progress(_, item_done, item_total, _) => {
                done.push(*item_done);
                total.push(*item_total);
            }
            _ => { }
        }
    }
    return (done, total);
}

/// How many lines one event stream wrote, counted in the same type the positions
/// of this file's tests are counted in.
#[cfg(test)]
fn line_count(names: &Vec::<String>) -> i64 {
    let mut count = 0;
    for _name in names.iter() {
        count = count + 1;
    }
    return count;
}

/// What the work of one item of [`run`] does, which is all the item loop has to
/// be driven from for a test: the loop itself is not observable from the outside
/// except through the events it writes, so each job below says what one item
/// comes down to and the test reads the stream back.
#[cfg(test)]
enum Job {
    /// The item is done, with no file worked on, so the `progress` event carries
    /// no `file` field ("Events": that field is present only for a command that
    /// processes files).
    JustDone,
    /// The item is done and the item itself is the file it worked on.
    DoneOfItem,
    /// The item is done, and while at it the work shows the user one `log` event.
    DoneWithLog,
    /// The work of the FOURTH item asks for the cancellation of the command and then
    /// finishes that item, which is what "Cancellation" says the core does: the loop
    /// refuses a further `progress` line for it, so the interface last heard "3 of
    /// 10", and no fifth item is started.
    CancelFlaggedByFourth,
    /// The first three items are done; the work of the fourth notices the cancellation
    /// the interface sent, so that item is left unfinished.
    CancelNoticedByFourth,
    /// The second item cannot be done, and the command is not retried.
    FailsAtSecond,
    /// The fourth item both notices the cancellation the interface sent AND reports a
    /// failure, which is the awkward combination: the answer is `cancelled`, never an
    /// `error` event.
    CancelFlaggedAndFailsByFourth,
}

#[cfg(test)]
struct Worker {
    job: Job,
    started: i64,
    logged: i64,
}

#[cfg(test)]
impl CommandWork for Worker {
    /// Do one item of the command, as [`Job`] says.
    fn do_item(&mut self, stream: &mut CommandStream, sink: &mut dyn EventSink, item: String) -> ItemOutcome {
        self.started = self.started + 1;
        match self.job {
            Job::JustDone => { return ItemOutcome::Done(None); }
            Job::DoneOfItem => { return ItemOutcome::Done(Some(item)); }
            Job::DoneWithLog => {
                let message = format!("probe: {item}");
                match log(stream, sink, "info".to_string(), message) {
                    Err(why) => {
                        let refused = what_it_says(why);
                        return ItemOutcome::Failed(ErrorReason::Io, format!("the work of this command could not report anything: {refused}"));
                    }
                    Ok(()) => { }
                }
                self.logged = self.logged + 1;
                return ItemOutcome::Done(Some(item));
            }
            Job::CancelFlaggedByFourth => {
                if self.started == 4 {
                    request_cancel(stream);
                }
                return ItemOutcome::Done(Some(item));
            }
            Job::CancelNoticedByFourth => {
                if self.started == 4 {
                    return ItemOutcome::Cancelled;
                }
                return ItemOutcome::Done(Some(item));
            }
            Job::CancelFlaggedAndFailsByFourth => {
                if self.started == 4 {
                    request_cancel(stream);
                    return ItemOutcome::Failed(ErrorReason::Io, "the file was half read".to_string());
                }
                return ItemOutcome::Done(Some(item));
            }
            Job::FailsAtSecond => {
                if self.started == 2 {
                    return ItemOutcome::Failed(ErrorReason::PluginFailed, "the importer died".to_string());
                }
                return ItemOutcome::Done(Some(item));
            }
        }
    }
}

/// The message one [`WriteError`] carries, so that the work of a test can say what
/// the stream refused.
#[cfg(test)]
fn what_it_says(why: WriteError) -> String {
    match why {
        WriteError::Codec(reason) => { return proto::codec::error_text(&reason); }
        WriteError::AlreadyFinished(ended_with) => { return format!("already ended with {ended_with:?}"); }
        WriteError::CancelRequested => { return "cancellation already requested".to_string(); }
        WriteError::TotalPassed(promised) => { return format!("the stream promised {promised} items").to_string(); }
    }
}

#[test]
fn cancel_requested_after_three_of_ten_items_writes_exactly_one_cancelled_event_and_no_result() {
    // "Cancellation" of doc/architecture/core-protocol.md: the interface sends `cancel`
    // while the command runs, the core finishes the CURRENT item, discards the partial
    // result and answers `cancelled` — so the stream holds ONE terminal event, it is
    // `cancelled`, no `result` is written for the command, and the item loop never
    // starts a FIFTH of the ten items: the work of the FOURTH item asks for the
    // cancellation itself and then finishes that item off, so item four IS started
    // and worked on, but "Events" allows no further `progress` line once a cancel
    // was seen and the item is not counted — which is what the assertions below
    // pin as "finished off, not counted" rather than "never started".
    let mut collected = collector();
    let mut stream = start("import".to_string(), 10);
    let mut worker = Worker { job: Job::CancelFlaggedByFourth, started: 0, logged: 0 };
    let read = run(&mut stream, &mut collected, &items_named(10), &payload_of(-1), &mut worker);
    match read {
        Ok(RunOutcome::Cancelled(done)) => { assert!(done == 3); }
        _ => { assert!(false); }
    }
    // The work of the FOURTH item is what flagged the cancel, so item four was
    // started and finished off and no FIFTH item was: it is not counted and gets
    // no `progress` line, which is why `done` stopped at three of the ten.
    assert!(worker.started == 4);
    let events = events_of(&collected);
    let names = kind_names(&events);
    assert!(counted(&names, "cancelled") == 1);
    assert!(counted(&names, "result") == 0);
    assert!(counted(&names, "error") == 0);
    assert!(counted_terminals(&events) == 1);
    // The `progress` numbers stopped AT the item that was in flight when the cancel
    // was requested: three of the ten promised items, then the terminal event.
    assert!(counted(&names, "progress") == 3);
    let (done_of, total_of) = progress_numbers(&events);
    assert!(done_of.len() == 3);
    assert!(done_of[0] == 1);
    assert!(done_of[1] == 2);
    assert!(done_of[2] == 3);
    for total in total_of.iter() {
        assert!(*total == 10);
    }
    // No terminal event appears before the loop ended.
    assert!(first_terminal(&events) == line_count(&names) - 1);
    // The cancelled event names the command whose partial result was discarded.
    assert!(events[3] == Event::Cancelled(Some("import".to_string())));
}

#[test]
fn cancel_noticed_by_the_work_of_an_item_ends_the_stream_without_a_result() {
    // The other way a cancel reaches the loop ([`ItemOutcome.Cancelled`], for work
    // that reads the `cancel` command of the interface itself): the item it was
    // working on is left unfinished, so the interface saw three items and one
    // `cancelled` event and no `result`.
    let mut collected = collector();
    let mut stream = start("import".to_string(), 10);
    let mut worker = Worker { job: Job::CancelNoticedByFourth, started: 0, logged: 0 };
    let read = run(&mut stream, &mut collected, &items_named(10), &payload_of(-1), &mut worker);
    match read {
        Ok(RunOutcome::Cancelled(done)) => { assert!(done == 3); }
        _ => { assert!(false); }
    }
    let events = events_of(&collected);
    let names = kind_names(&events);
    assert!(counted(&names, "cancelled") == 1);
    assert!(counted(&names, "result") == 0);
    assert!(counted(&names, "progress") == 3);
    assert!(counted_terminals(&events) == 1);
    assert!(first_terminal(&events) == line_count(&names) - 1);
}

#[test]
fn progress_numbers_of_a_full_run_are_strictly_monotonic_and_never_pass_total() {
    // The `done`/`total` pair of the doc's progress example, for a command that
    // ran out of items: the numbers come from the counter of the stream, so they
    // cannot repeat, go backwards or overshoot the number of items promised.
    let mut collected = collector();
    let mut stream = start("import".to_string(), 10);
    let mut worker = Worker { job: Job::DoneOfItem, started: 0, logged: 0 };
    let read = run(&mut stream, &mut collected, &items_named(10), &payload_of(183), &mut worker);
    assert!(read.is_ok());
    let events = events_of(&collected);
    let names = kind_names(&events);
    assert!(counted(&names, "progress") == 10);
    assert!(counted(&names, "result") == 1);
    assert!(counted_terminals(&events) == 1);
    let (done_of, total_of) = progress_numbers(&events);
    assert!(done_of.len() == 10);
    let mut expect = 0;
    for index in 0..done_of.len() {
        expect = expect + 1;
        assert!(done_of[index] == expect);
        assert!(done_of[index] <= total_of[index]);
        assert!(total_of[index] == 10);
    }
    assert!(first_terminal(&events) == line_count(&names) - 1);
    // The `result` event of the run carries the payload the command handed out,
    // read back through `proto` as the fields of the line.
    assert!(events[10] == Event::Result("import".to_string(), payload_of(183)));
}

#[test]
fn an_item_list_longer_than_the_promised_total_is_refused_before_any_item_starts() {
    // The mismatch case of the two INDEPENDENT inputs of [`start`] and [`run`]: a
    // `total` of two for a list of five files cannot be caught at [`start`], so
    // [`run`] refuses it with [`WriteError.TotalPassed`] — NO item of the list is
    // started, NO line is written and the stream stays OPEN, so the caller of
    // plan item 3.4 can still end such a command with its `error` event. The same
    // rule then holds on the path of a caller that drives the loop itself: after
    // the one item a `total` of one allows, the next [`progress`] is refused
    // instead of writing "2 of 1", so no stream can ever report `done` greater
    // than `total`.
    let mut collected = collector();
    let mut stream = start("import".to_string(), 2);
    let mut worker = Worker { job: Job::DoneOfItem, started: 0, logged: 0 };
    assert!(run(&mut stream, &mut collected, &items_named(5), &payload_of(-1), &mut worker) == Err(WriteError::TotalPassed(2)));
    assert!(worker.started == 0);
    assert!(collected.frames.len() == 0);
    assert!(log(&mut stream, &mut collected, "info".to_string(), "nothing was promised to the interface".to_string()) == Ok(()));
    assert!(fail(&mut stream, &mut collected, ErrorReason::Io, "the command was given more items than it promised".to_string()) == Ok(()));
    assert!(counted_terminals(&events_of(&collected)) == 1);

    let mut direct = collector();
    let mut short = start("report".to_string(), 1);
    assert!(progress(&mut short, &mut direct, None) == Ok(1));
    assert!(progress(&mut short, &mut direct, None) == Err(WriteError::TotalPassed(1)));
    assert!(succeed(&mut short, &mut direct, &payload_of(183)) == Ok(()));
    let (done_of, total_of) = progress_numbers(&events_of(&direct));
    assert!(done_of.len() == 1);
    assert!(done_of[0] == 1);
    assert!(total_of[0] == 1);
    assert!(counted_terminals(&events_of(&direct)) == 1);
}

#[test]
fn error_event_carries_the_name_of_the_command_that_failed() {
    // The doc's error example is `{"v":1,"event":"error","cmd":"import",…}`, so the
    // name of the command is checked by DECODING the line the writer emitted and
    // comparing the typed event, not by matching text inside a hand-built string.
    let mut collected = collector();
    let mut stream = start("import".to_string(), 10);
    let mut worker = Worker { job: Job::FailsAtSecond, started: 0, logged: 0 };
    let read = run(&mut stream, &mut collected, &items_named(10), &payload_of(-1), &mut worker);
    match read {
        Ok(RunOutcome::Failed) => { }
        _ => { assert!(false); }
    }
    let events = events_of(&collected);
    let names = kind_names(&events);
    assert!(counted(&names, "error") == 1);
    assert!(counted(&names, "result") == 0);
    assert!(counted(&names, "cancelled") == 0);
    assert!(counted(&names, "progress") == 1);
    assert!(counted_terminals(&events) == 1);
    // The failed item stopped the loop: the third of the ten items was never started.
    assert!(events[1] == Event::Error(Some("import".to_string()), ErrorReason::PluginFailed, "the importer died".to_string()));
    assert!(first_terminal(&events) == line_count(&names) - 1);
}

#[test]
fn a_second_terminal_event_of_one_command_is_a_typed_error_that_writes_no_line() {
    // "Events" (draft, inferred from the Runtime View examples): exactly one `result`,
    // or `error` / `cancelled`, per command. The stream remembers that it ended, so
    // every second terminal event comes back as the typed error naming the terminal
    // event already written, and the sink is not written to a second time.
    let mut collected = collector();
    let mut stream = start("set-category".to_string(), 0);
    assert!(succeed(&mut stream, &mut collected, &payload_of(183)) == Ok(()));
    assert!(succeed(&mut stream, &mut collected, &payload_of(183)) == Err(WriteError::AlreadyFinished(Terminal::Result)));
    assert!(fail(&mut stream, &mut collected, ErrorReason::Busy, "another command holds the daemon".to_string()) == Err(WriteError::AlreadyFinished(Terminal::Result)));
    assert!(cancel(&mut stream, &mut collected) == Err(WriteError::AlreadyFinished(Terminal::Result)));
    assert!(collected.frames.len() == 1);
}

#[test]
fn no_event_of_any_kind_is_written_after_the_command_ended() {
    // A `progress` or a `log` line AFTER the terminal event would tell the interface
    // the command is still running, so they are refused by the same rule, and the
    // one line of the command is the error event of the code the work chose.
    let mut collected = collector();
    let mut stream = start("shutdown".to_string(), 0);
    assert!(fail(&mut stream, &mut collected, ErrorReason::Busy, "another command holds the daemon".to_string()) == Ok(()));
    assert!(progress(&mut stream, &mut collected, None) == Err(WriteError::AlreadyFinished(Terminal::Error)));
    assert!(log(&mut stream, &mut collected, "info".to_string(), "late".to_string()) == Err(WriteError::AlreadyFinished(Terminal::Error)));
    let events = events_of(&collected);
    assert!(collected.frames.len() == 1);
    assert!(counted_terminals(&events) == 1);
    assert!(events[0] == Event::Error(Some("shutdown".to_string()), ErrorReason::Busy, "another command holds the daemon".to_string()));
}

#[test]
fn only_the_cancelled_event_may_follow_a_cancel_request() {
    // "Cancellation": once the interface sent `cancel`, the core finishes the current
    // item and answers `cancelled`; every event that would report work AFTER that —
    // `progress`, `log`, `result`, `error` — is the typed error
    // [`WriteError.CancelRequested`] and writes nothing, while the `cancelled`
    // event itself is still allowed, exactly once.
    let mut collected = collector();
    let mut stream = start("import".to_string(), 10);
    request_cancel(&mut stream);
    assert!(progress(&mut stream, &mut collected, Some("bank_a.pdf".to_string())) == Err(WriteError::CancelRequested));
    assert!(log(&mut stream, &mut collected, "info".to_string(), "still reading".to_string()) == Err(WriteError::CancelRequested));
    assert!(succeed(&mut stream, &mut collected, &payload_of(-1)) == Err(WriteError::CancelRequested));
    assert!(fail(&mut stream, &mut collected, ErrorReason::Io, "half read".to_string()) == Err(WriteError::CancelRequested));
    assert!(collected.frames.len() == 0);
    assert!(cancel(&mut stream, &mut collected) == Ok(()));
    assert!(cancel(&mut stream, &mut collected) == Err(WriteError::AlreadyFinished(Terminal::Cancelled)));
    let events = events_of(&collected);
    assert!(collected.frames.len() == 1);
    assert!(events[0] == Event::Cancelled(Some("import".to_string())));
}

#[test]
fn a_cancel_requested_before_the_items_ends_the_command_without_starting_any_work() {
    // The cancel flag looked at BETWEEN the items by [`run`], on the first turn of
    // the loop: no item of the command is worked on at all, so no `progress` line
    // was written, and the one event of the command is the `cancelled` event.
    let mut collected = collector();
    let mut stream = start("import".to_string(), 10);
    request_cancel(&mut stream);
    let mut worker = Worker { job: Job::DoneOfItem, started: 0, logged: 0 };
    let read = run(&mut stream, &mut collected, &items_named(10), &payload_of(-1), &mut worker);
    assert!(read == Ok(RunOutcome::Cancelled(0)));
    assert!(worker.started == 0);
    assert!(kind_names(&events_of(&collected)) == ["cancelled".to_string()].to_vec());
}

#[test]
fn log_and_progress_lines_may_appear_any_number_of_times_in_between() {
    // "Events": `progress` and `log` may appear any number of times between the
    // start of a command and its one terminal event. Here the work of each item
    // showed the user one line WHILE it ran, so the stream of a three item command
    // alternates log/progress three times and then ends once; a command may also
    // log twice without any item, and end with a `result` of no payload.
    let mut collected = collector();
    let mut stream = start("import".to_string(), 3);
    let mut worker = Worker { job: Job::DoneWithLog, started: 0, logged: 0 };
    assert!(run(&mut stream, &mut collected, &items_named(3), &payload_of(183), &mut worker) == Ok(RunOutcome::Completed(3)));
    assert!(worker.logged == 3);
    assert!(kind_names(&events_of(&collected)) == ["log".to_string(), "progress".to_string(), "log".to_string(), "progress".to_string(), "log".to_string(), "progress".to_string(), "result".to_string()].to_vec());
    let mut plain = collector();
    let mut idle = start("report".to_string(), 0);
    assert!(log(&mut idle, &mut plain, "warn".to_string(), "no converter found".to_string()) == Ok(()));
    assert!(log(&mut idle, &mut plain, "info".to_string(), "trying the fallback".to_string()) == Ok(()));
    assert!(succeed(&mut idle, &mut plain, &payload_of(-1)) == Ok(()));
    assert!(kind_names(&events_of(&plain)) == ["log".to_string(), "log".to_string(), "result".to_string()].to_vec());
    assert!(counted_terminals(&events_of(&plain)) == 1);
}

#[test]
fn every_frame_is_exactly_one_line_ending_in_a_line_feed_and_no_text_forges_a_second_one() {
    // The framing of the protocol ("Transport"): one JSON object per line, `\n`
    // terminated, so a `log` text a plugin produced may hold a line break, a quote
    // or braces and still be ONE line of the stream that reads back as one event.
    let mut collected = collector();
    let mut stream = start("import".to_string(), 2);
    let nasty = "a line break\nand a quote \" and braces {} and a tab\t".to_string();
    assert!(log(&mut stream, &mut collected, "warn".to_string(), nasty.to_string()) == Ok(()));
    let mut worker = Worker { job: Job::DoneOfItem, started: 0, logged: 0 };
    assert!(run(&mut stream, &mut collected, &items_named(2), &payload_of(183), &mut worker) == Ok(RunOutcome::Completed(2)));
    assert!(collected.frames.len() == 4);
    for index in 0..collected.frames.len() {
        let frame = collected.frames[index].to_string();
        assert!(frame.ends_with("\n"));
        let mut breaks = 0;
        for character in frame.chars() {
            if character == '\n' {
                breaks = breaks + 1;
            }
        }
        assert!(breaks == 1);
    }
    let events = events_of(&collected);
    assert!(events[0] == Event::Log("warn".to_string(), nasty));
    assert!(counted_terminals(&events) == 1);
}

#[test]
fn a_command_with_no_items_ends_with_one_result_event_and_no_progress_line() {
    // "any number of times in between" includes ZERO: a command of the command table
    // with no work items at all (a `total` of `0`) ends with its `result` event alone,
    // carrying the payload fields the command hands out.
    let mut collected = collector();
    let mut stream = start("shutdown".to_string(), 0);
    let mut worker = Worker { job: Job::JustDone, started: 0, logged: 0 };
    assert!(run(&mut stream, &mut collected, &Vec::<String>::new(), &payload_of(183), &mut worker) == Ok(RunOutcome::Completed(0)));
    assert!(worker.started == 0);
    assert!(kind_names(&events_of(&collected)) == ["result".to_string()].to_vec());
    assert!(events_of(&collected)[0] == Event::Result("shutdown".to_string(), payload_of(183)));
}

#[test]
fn a_payload_key_the_framing_writes_for_itself_is_the_codecs_own_typed_error() {
    // `proto` refuses a `result` payload key named `v`, `event` or `cmd`, because
    // the framing of the line writes that name for itself; this module passes that
    // error on instead of swallowing it or writing a half line, leaves the command
    // OPEN so the interpreter of plan item 3.4 can still end it properly, and
    // writing a proper payload afterwards still works.
    let mut collected = collector();
    let mut stream = start("report".to_string(), 0);
    assert!(succeed(&mut stream, &mut collected, &reserved_payload()) == Err(WriteError::Codec(CodecError::ReservedField("cmd".to_string()))));
    assert!(collected.frames.len() == 0);
    assert!(succeed(&mut stream, &mut collected, &payload_of(183)) == Ok(()));
    assert!(collected.frames.len() == 1);
    assert!(cancel(&mut stream, &mut collected) == Err(WriteError::AlreadyFinished(Terminal::Result)));
}

#[test]
fn a_failure_reported_after_a_cancel_is_answered_with_the_cancelled_event_only() {
    // The awkward combination: the work of an item notices the cancel AND fails.
    // "Cancellation" says the answer is `cancelled`, so the stream holds exactly one
    // terminal event, no `error` line is written for the failed item, and the item
    // loop stops.
    let mut collected = collector();
    let mut stream = start("import".to_string(), 10);
    let mut worker = Worker { job: Job::CancelFlaggedAndFailsByFourth, started: 0, logged: 0 };
    let read = run(&mut stream, &mut collected, &items_named(10), &payload_of(-1), &mut worker);
    assert!(read == Ok(RunOutcome::Cancelled(3)));
    let events = events_of(&collected);
    let names = kind_names(&events);
    assert!(counted(&names, "cancelled") == 1);
    assert!(counted(&names, "error") == 0);
    assert!(counted(&names, "result") == 0);
    assert!(counted_terminals(&events) == 1);
    assert!(counted(&names, "progress") == 3);
    assert!(events[3] == Event::Cancelled(Some("import".to_string())));
}

#[cfg(test)]
fn reserved_payload() -> Map::<String, Value> {
    let mut payload = Map::<String, Value>::new();
    payload.insert("cmd".to_string(), Value::String("report".to_string()));
    return payload;
}
