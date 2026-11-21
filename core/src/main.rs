//! The core daemon of plan item 3.4: the command interpreter of the core protocol,
//! run on a pipe.
//!
//! What the interface process sends to stdin is read as the abstract command
//! stream of `core/src/protocol/interpret.rs` (plan item 3.4), which keeps the
//! JSON-Lines framing of `proto`, checks every line against the command table, and
//! writes the event lines `doc/architecture/core-protocol.md` ("Events") defines to
//! the sink of `core/src/protocol/writer.rs` (plan item 3.3) — here, stdout, flushed
//! after every line, which is what lets the interface watch `progress` and `log`
//! WHILE a command runs. Which verbs do any work at all is the injected
//! [`interpret::Work`] seam: in this build none of them is wired
//! ([`interpret::WiredNowhere`]), so the commands of the table answer the
//! `error` event of the documented code naming the plan item that wires them.
//!
//! THIS binary does exactly two pieces of I/O, and only because writing the
//! events to stdout and reading the commands from stdin is what the row of plan
//! item 3.4 asks of it: it reads stdin and writes stdout. No socket, no listening
//! port, no `state/core.lock`, no handshake, no idle timeout — the transport of
//! plan item 3.6 (`core/src/technical/transport.rs`) hands the same
//! [`interpret::TextInput`] and the same sink to the same
//! [`interpret::run`] loop for the connection it opens, and plan item 3.5 owns the
//! `version`/`hello` handshake and the nonce of the lock file.
//!
//! A session ends, and BOTH halves of that I/O report their own failure instead
//! of pretending the session ended cleanly: a read of stdin that fails arrives as
//! [`interpret::TextRead.Unreadable`] and stops the loop with
//! [`interpret::Stop.StreamBroken`], and a write of stdout that fails is latched
//! inside the sink. [`exit_code`] turns that pair into the process status — and
//! because the decision is a pure function of those two values, the table is
//! unit-tested here without starting the process, as is the sink, which is
//! generic over the stream it writes to. EOF, a half line left at EOF and
//! `shutdown` exit 0; an input-side read failure or a failed write of stdout
//! exit 1. The exit code is the ONLY place a broken stream is reported, because
//! the stream it is failing to write is the only channel it could be told in.

use core::protocol::interpret;
use core::protocol::writer::EventSink;
use core::technical::transport;
use core::technical::transport_cfg;
use std::io::Write;

/// stdin as the abstract command stream the interpreter reads.
struct StdinText {
    /// Whether the end of stdin was already reached, or a read of it already
    /// failed; after either, there is no more text to hand out, and the difference
    /// between "ended" and "failed" was reported once, when it happened: the
    /// end of the stream is [`interpret::TextRead.Done`] and a FAILED read is
    /// [`interpret::TextRead.Unreadable`], so an I/O error on this stream is
    /// never mistaken for a clean end of input (see [`exit_code`] for what the
    /// difference is worth at the exit).
    pub ended: bool
}

impl interpret::TextInput for StdinText {
    /// Hand out one line of stdin as one chunk of text.
    ///
    /// A line of stdin is the chunk, not the "line" of the protocol: the framing
    /// of `proto` decides where a protocol line ends, and a read that returns
    /// text WITHOUT a trailing `\n` is what the end of the input looks like, which
    /// is how a partial line stays pending instead of being decoded ("Unknown
    /// input" of `doc/architecture/core-protocol.md`).
    fn read_text(&mut self) -> interpret::TextRead {
        if self.ended {
            return interpret::TextRead::Done;
        }
        let mut line = String::new();
        match std::io::stdin().read_line(&mut line) {
            Err(reason) => {
                self.ended = true;
                return interpret::TextRead::Unreadable(format!("{reason}"));
            }
            Ok(read) => {
                if read == 0 {
                    self.ended = true;
                    return interpret::TextRead::Done;
                }
                return interpret::TextRead::Text(line.to_string());
            }
        }
    }
}

/// stdout as the event sink of plan 3.3's writer — generic over the stream the
/// frames go to, which is what makes the latch contract observable in a unit
/// test without starting the process: the daemon's sink is
/// `LatchLines<std::io::Stdout>`, the sink of the test of the latch at the end
/// of this file is `LatchLines<Refusing>`. Plan item 3.6's transport hands its
/// own sink to the same [`interpret::run`] loop, and the same obligation applies
/// to it.
struct LatchLines<W: Write> {
    /// Where the frames go: `std::io::stdout()` for the daemon, the refusing
    /// target `Refusing` of the test of the latch.
    pub out: W,

    /// Set by the FIRST write of this sink that failed, and kept: the doc of
    /// `core/src/protocol/writer.rs` puts that obligation on the sink of plan 3.4,
    /// because plan 3.3's writer has already counted the frame and marks the
    /// command terminal WITHOUT observing the sink, so it cannot know that the
    /// interface never saw the line.
    pub broken: bool
}

impl<W: Write> EventSink for LatchLines<W> {
    /// Write one frame to the stream and flush it.
    ///
    /// `proto`'s [`proto::codec::encode_frame`] already wrote the frame as one
    /// `\n`-terminated line, so the frame goes out as it is and the flush per
    /// line is what makes the stream readable WHILE a command runs. The frame is
    /// written THROUGH the [`std::io::Write`] handle rather than by the standard
    /// `print` macro, BECAUSE THAT macro RAISES when the write ITSELF fails: an
    /// EPIPE would then surface inside it instead of at the flush, and the latch
    /// below assumes a failure shows up where this method can see it. So a
    /// failure of EITHER the write or the flush latches the sink, and from
    /// there every later line of the session is DROPPED, because a line written
    /// under a line the interface never received would be read out of order, and
    /// plan 3.3's writer would not stop it.
    fn write_line(&mut self, line: String) {
        if self.broken {
            // Dropped on purpose: this is the line after the stream broke, and
            // plan 3.3's writer will not stop it (see the field above).
        } else {
            match self.out.write_all(line.as_bytes()) {
                Err(_) => { self.broken = true; }
                Ok(_) => {
                    match self.out.flush() {
                        Err(_) => { self.broken = true; }
                        Ok(_) => { }
                    }
                }
            }
        }
    }
}

/// The status the daemon exits with, decided from the two typed facts about the
/// session that just ended: `stop` is why [`interpret::run`] stopped reading the
/// input, `sink_broken` whether the sink ever failed to write a frame.
///
/// The table is exactly the two clean ends and the two failures. `shutdown` and
/// both ends of the input are CLEAN: `shutdown` because the doc's draft says the
/// daemon answers it with no event and the CLOSED stream is the signal (plan
/// items 3.5/3.6 add the lock file and the idle timeout around that), and a
/// half line left at the end of the input because "Unknown input" forbids
/// resynchronizing on it and nothing can ever complete it. A failure is NOT
/// clean: [`interpret::Stop.StreamBroken`] means a line of the input could not be
/// read — which is what `StdinText` reports for a failing read of stdin — and
/// `sink_broken` means a line of the OUTPUT could not be written. Stdout is the
/// only thing this daemon may write, so both are told with the exit code and
/// not with another line of the stream they are failing to write. Extracted as a
/// function of two values precisely so that the table is a unit test rather than
/// a process that has to be started; [`main`] passes it the values it observed.
fn exit_code(stop: interpret::Stop, sink_broken: bool) -> i32 {
    match stop {
        interpret::Stop::EndOfInput => { }
        interpret::Stop::HalfLineAtEnd(_) => { }
        interpret::Stop::ShutdownRequested => { }
        interpret::Stop::StreamBroken(_) => { return 1; }
    }
    if sink_broken {
        return 1;
    }
    return 0;
}

/// The process's own argv as DATA, [`argv[0]`] FIRST (it is the executable
/// PATH a caller of `transport::connect_or_spawn` needs as the spawn seam's
/// executable — `std::process::self_exe_path` does NOT exist in this toolchain
/// (E0425, a toolchain fact of this row), so the caller's own argv[0] IS the
/// executable path; plan item 6.1's CLI is what wires that call, and the
/// integration tests of 3.6 pass `CARGO_BIN_EXE_core` instead).
///
/// The pipe session of plan items 3.4/3.5 reads NONE of this: `--serve`
/// SELECTS the daemon branch below and the default branch is byte-identical to
/// what item 3.5 left, so its 158 lib + 4 bin tests are UNCHANGED.
fn argv() -> Vec::<String> {
    let mut collected = Vec::<String>::new();
    for held in std::env::args() {
        collected.push(held.to_string());
    }
    return collected;
}

/// The text of one [`transport_cfg::ConfigIssue`] (it carries its complaint as TYPED
/// data WITHOUT a `Display`, like [`core::technical::fs_atomic::WriteError`] in the
/// transport module): rendered here because THIS is where a user sees it.
fn config_refusal_text(issue: &transport_cfg::ConfigIssue) -> String {
    match issue {
        transport_cfg::ConfigIssue::NotReadable(text) => { return format!("the configuration file could not be read: {text}", text = text); }
        transport_cfg::ConfigIssue::NotTOML(text) => { return format!("the configuration file is not TOML: {text}", text = text); }
        transport_cfg::ConfigIssue::WrongType(text) => { return format!("a configuration key holds the wrong type: {text}", text = text); }
        transport_cfg::ConfigIssue::PortOutOfRange(value) => { return format!("the configured port {value} cannot be bound", value = value); }
    }
}

/// One line of DAEMON diagnostics on stderr. The event stream itself stays
/// free of non-protocol output (decision 20): a daemon STARTUP or LOCK failure
/// is not an event — the doc gives the daemon no event for it — and
/// `transport` carries every reason as typed data, which this is the one
/// place allowed to render ("A daemon error is reported on stderr by
/// `main.rs`", the transport module doc). A FAILURE OF THE DIAGNOSTIC ITSELF
/// IS NOT REPORTED: stderr is the channel being complained about.
fn report_stderr(text: String) {
    match std::io::stderr().write_all(text.as_bytes()) {
        Err(_) => { }
        Ok(_) => {
            match std::io::stderr().flush() {
                Err(_) => { }
                Ok(_) => { }
            }
        }
    }
}

/// What removing the lock file on a daemon exit is worth to the USER:
/// [`transport_cfg::Removal`]'s typed outcome; only a FAILURE needs a line, and
/// the line is `data-files.md:143`'s promise breaking ("removed on exit"),
/// WHICH is why the exit below is then NOT clean either.
fn report_removal(removal: transport_cfg::Removal) -> bool {
    match removal {
        transport_cfg::Removal::Removed => { return true; }
        transport_cfg::Removal::WasAbsent => { return true; }
        transport_cfg::Removal::Failed(kind, text) => {
            report_stderr(format!("the lock file of the exiting daemon could not be removed ({why}): {seen}", why = kind.to_string(), seen = text));
            return false;
        }
    }
}

/// The daemon branch of `--serve`: bind the CONFIGURED address (flags first,
/// then the configuration of `core-protocol.md:14` —
/// [`transport::daemon_settings`] IS that precedence), write `state/core.lock`
/// ([`transport::daemon_start`]), serve through [`transport::serve_daemon`]
/// until the idle timeout or `shutdown`, and REMOVE the lock on EVERY exit
/// path. Status: 0 for the two documented clean exits, 1 for a refused
/// start-up (an occupied address is NAMED by the CONFIGURED ADDRESS,
/// `architecture.md:250`), a broken listener, or a lock that stays behind.
fn daemon_status(args: Vec::<String>) -> i32 {
    match transport::daemon_settings(args) {
        (None, issues) => {
            for held in issues {
                match held {
                    transport::DaemonFlagIssue::NotANumber(flag, seen) => { report_stderr(format!("the daemon flag {name} needs an integer, saw [{value}]", name = flag, value = seen)); }
                    transport::DaemonFlagIssue::PortOutOfRange(value) => { report_stderr(format!("the daemon port {value} cannot be bound")); }
                    transport::DaemonFlagIssue::IdleTooSmall(value) => { report_stderr(format!("the daemon idle timeout {value} must be a POSITIVE number of seconds")); }
                    transport::DaemonFlagIssue::ConfigRejected(rejected) => {
                        for bad in rejected {
                            report_stderr(config_refusal_text(&bad));
                        }
                    }
                }
            }
            return 1;
        }
        (Some(settings), _) => {
            let endpoint = transport_cfg::Endpoint { host: settings.host, port: settings.port };
            match transport::daemon_start(&endpoint, &settings.data_dir) {
                transport::DaemonStart::AddressTaken(address, message) => {
                    report_stderr(format!("nothing listens at the configured address [{at}] and this daemon could not bind it either: {why}", at = address, why = message));
                    return 1;
                }
                transport::DaemonStart::LockUnwritten(address, complaint) => {
                    report_stderr(format!("the daemon could not write its lock file for the configured address [{at}]: {why}", at = address, why = complaint));
                    return 1;
                }
                transport::DaemonStart::Ready(handle) => {
                    let mut nothing_wired = interpret::wired_nowhere();
                    let exit = transport::serve_daemon(&handle.listener, &settings.data_dir, settings.idle_seconds, &mut nothing_wired, handle.session);
                    match exit {
                        // The two DOCUMENTED clean exits; a lock left BEHIND
                        // (`Removal::Failed`) breaks `data-files.md:143`
                        // ("removed on exit") and is NOT clean.
                        transport::DaemonExit::IdleExit(removal) => {
                            if !report_removal(removal) {
                                return 1;
                            }
                            return 0;
                        }
                        transport::DaemonExit::ShutdownExit(removal) => {
                            if !report_removal(removal) {
                                return 1;
                            }
                            return 0;
                        }
                        transport::DaemonExit::ListenerBroken(complaint, removal) => {
                            report_stderr(format!("the listener of the daemon broke: {why}", why = complaint));
                            if !report_removal(removal) {
                                return 1;
                            }
                            return 1;
                        }
                    }
                }
            }
        }
    }
}

/// Run the daemon: read the commands of one session from stdin, answer them on
/// stdout, and stop when the stream says to — UNLESS `--serve` is in the
/// argv, WHICH is the daemon mode of plan item 3.6 ([`daemon_status`]).
fn main() {
    let args = argv();
    let mut serve_mode = false;
    for held in args.to_vec() {
        if held == transport::DAEMON_FLAG {
            serve_mode = true;
        }
    }
    if serve_mode {
        std::process::exit(daemon_status(args));
    }
    let mut input = StdinText { ended: false };
    let mut sink: LatchLines<std::io::Stdout> = LatchLines { out: std::io::stdout(), broken: false };
    let mut nothing_wired = interpret::wired_nowhere();
    let how = interpret::run(&mut input, &mut sink, &mut nothing_wired);
    std::process::exit(exit_code(how, sink.broken));
}

#[test]
fn core_daemon_compiles() {}

#[test]
fn a_broken_stream_exits_one_and_a_clean_end_exits_zero() {
    // The exit-code table of [`exit_code`], unit-tested WITHOUT starting the
    // process: the three clean ends of the loop, and the two failures that
    // must not read as one of them. That "a closed stream is the signal" of
    // `shutdown` stays clean while an I/O failure is not is what plan items
    // 3.5 and 3.6 build the handshake and the transport on.
    assert!(exit_code(interpret::Stop::EndOfInput, false) == 0);
    assert!(exit_code(interpret::Stop::HalfLineAtEnd("{\"v\":1,\"cmd\":\"sh".to_string()), false) == 0);
    assert!(exit_code(interpret::Stop::ShutdownRequested, false) == 0);
    // Input side: a read of stdin that FAILED, not one that ended.
    assert!(exit_code(interpret::Stop::StreamBroken("stream read error".to_string()), false) == 1);
    // Output side: the loop stopped for a reason of its own, and the sink latched.
    assert!(exit_code(interpret::Stop::EndOfInput, true) == 1);
    assert!(exit_code(interpret::Stop::StreamBroken("nope".to_string()), true) == 1);
}

/// A stream that refuses EVERY write, recording the length of each write it was
/// asked to take, so that the test of the latch below sees the behaviour of
/// [`LatchLines`] without a process: exactly one line reaches a broken stream,
/// and it is the first. `std::io::Write` hands the bytes of a write as a slice, which is why
/// THIS signature spells `&[u8]` where the rest of the core passes `Vec::<u8>`
/// and `String::as_bytes()`.
#[cfg(test)]
struct Refusing {
    /// The byte length of each write this stream was asked to take, in the order
    /// it was asked. A refused write is RECORDED too — that is the point — and
    /// the three lines of the test differ in length, so the recorded length says
    /// WHICH line got through.
    pub refused: Vec::<usize>
}

#[cfg(test)]
impl Write for Refusing {
    /// The one method [`std::io::Write`] requires: every write is recorded and
    /// refused, and [`std::io::Write.write_all`] — what the sink of
    /// [`LatchLines`] calls — takes the failure straight from it.
    fn write(&mut self, input: &[u8]) -> Result::<usize, std::io::Error> {
        self.refused.push(input.len());
        return Err(std::io::Error::new(std::io::ErrorKind::BrokenPipe, "the stream behind this sink is closed"));
    }

    fn flush(&mut self) -> Result::<(), std::io::Error> {
        return Ok(());
    }
}

#[test]
fn the_sink_latches_at_the_first_line_it_cannot_write() {
    // The obligation plan 3.3's doc puts on the sink of plan 3.4 (see the
    // `broken` field of [`LatchLines`]): a frame that failed to reach the
    // interface must stop every later frame of the same session, because
    // plan 3.3's writer has already counted that command as finished. Dropping
    // the latch therefore HAS to fail this test, which is what makes the
    // contract of the PRODUCTION sink observable.
    let mut sink: LatchLines<Refusing> = LatchLines { out: Refusing { refused: Vec::<usize>::new() }, broken: false };
    sink.write_line("one\n".to_string());
    sink.write_line("twotwo\n".to_string());
    sink.write_line("threethreethree\n".to_string());
    // Asked once, and for the first line only (4 bytes = `"one\n"`).
    assert!(sink.out.refused.len() == 1);
    assert!(sink.out.refused[0] == 4);
    assert!(sink.broken);
}

/// A stream that ACCEPTS every write and REFUSES every flush, recording the
/// length of each write it accepted, in the order it accepted them. This is the
/// shape the real failure of the daemon has: stdout is a pipe the interface has
/// closed, the bytes of the line go into the buffer in front of the pipe
/// WITHOUT complaint, and the EPIPE arrives at the flush AFTER the line — which
/// is exactly the arm of [`LatchLines`] the `Refusing` stream above cannot
/// reach, because it fails at the write and the flush of that sink is never
/// called. What the test below asserts on the recorded lengths is that no
/// line AFTER the failed flush was ever handed out.
#[cfg(test)]
struct FlushRefusing {
    /// The byte length of each write this stream was asked to take and took, in
    /// the order it was asked. A line DROPPED by the latch of [`LatchLines`] is
    /// NOT recorded, so the recorded lengths say how far the session got.
    pub accepted: Vec::<usize>
}

#[cfg(test)]
impl Write for FlushRefusing {
    /// Every write is accepted and recorded; [`std::io::Write.write_all`] of the
    /// sink of [`LatchLines`] therefore reaches its flush call — as it does for
    /// a real buffered stdout — instead of failing at the write.
    fn write(&mut self, input: &[u8]) -> Result::<usize, std::io::Error> {
        self.accepted.push(input.len());
        return Ok(input.len());
    }

    /// Every flush is refused, which is where the EPIPE of a closed pipe shows
    /// up once the line sits in the buffer.
    fn flush(&mut self) -> Result::<(), std::io::Error> {
        return Err(std::io::Error::new(std::io::ErrorKind::BrokenPipe, "the pipe behind this stream was closed while the line was buffered"));
    }
}

#[test]
fn the_sink_latches_at_a_flush_that_fails_on_an_already_written_line() {
    // The other half of the obligation of the `broken` field of [`LatchLines`],
    // and the half the test of the latch above leaves unproven: a line whose
    // BYTES were accepted and whose FLUSH failed latches the sink too, so that
    // the doc of [`LatchLines`] holding that "a failure of EITHER the write or
    // the flush latches" is checked at both ends rather than at one. Dropping
    // the latch from the flush arm therefore HAS to fail this test.
    let mut sink: LatchLines<FlushRefusing> = LatchLines { out: FlushRefusing { accepted: Vec::<usize>::new() }, broken: false };
    sink.write_line("one\n".to_string());
    sink.write_line("twotwo\n".to_string());
    sink.write_line("threethreethree\n".to_string());
    // The first line got through (4 bytes = `"one\n"`) and its flush failed; the
    // second and the third were dropped, so only ONE accepted write exists.
    assert!(sink.out.accepted.len() == 1);
    assert!(sink.out.accepted[0] == 4);
    assert!(sink.broken);
}
