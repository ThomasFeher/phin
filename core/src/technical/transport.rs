//! The daemon transport and discovery of plan item 3.6 — connect-or-spawn.
//!
//! This is the transport the row of the plan puts under `core/src/technical/`
//! ("opening the stream is transport, which lives in the core", `proto`'s own
//! module doc): `state/core.lock` is written and removed, `phin.toml` is read,
//! a TCP stream is bound on `127.0.0.1`, the handshake is performed **before any
//! command**, and the socket pair is fed to the interpreter of plan items 3.4/3.5
//! (`interpret::run_session` of `core/src/protocol/interpret.rs` plus the
//! `EventSink` of plan 3.3) instead of to stdin/stdout, which
//! `core/src/main.rs` keeps for the pipe session of those items.
//!
//! # The other half of the row (plan sub-row 3.6b, the USER's decision 38)
//!
//! The CONFIG/LOCK/ENDPOINT/NONCE half of this row — the address names,
//! the `phin.toml` reading (`endpoint_from_config`), the `state/core.lock`
//! file (`lock_path`/`read_lock`/`write_lock`/`remove_lock`) and the nonce
//! mint (`mint_nonce`) — lives in the sibling module
//! `core/src/technical/transport_cfg.rs` with its OWN tests; what stays
//! HERE is the connection gate, the daemon side, the spawn seam and
//! `connect_or_spawn`, and this module RE-EXPORTS those names (see the
//! `pub use` block below) only until sub-row 3.6c rewires the two callers
//! that still name them through `transport::` (`core/src/main.rs` and
//! `core/tests/transport_lifecycle.rs`).
//!
//! # What the interface docs fix
//!
//! * **Framing and carrier** (`doc/architecture/core-protocol.md:8-16`): JSON-Lines
//!   over a TCP stream on `127.0.0.1`, one connection per session; the framing
//!   belongs to `proto::codec` and is reused, never re-implemented here.
//! * **Address** (`core-protocol.md:14`): `core.address` and `core.port` in the
//!   TOML configuration, default `127.0.0.1:47623`; only the address is configurable.
//! * **Lock file** (`core-protocol.md:18-28`, `data-files.md:143`):
//!   `state/core.lock` inside the data directory, holding the address the daemon
//!   listens on plus its nonce; written at daemon start-up, removed on exit. The
//!   text itself is encoded and parsed by `proto/src/handshake.rs`, which is
//!   `proto`'s half of this row.
//! * **Discovery** (`architecture.md:247-250`): a lock file that READS ->
//!   connect to the CONFIGURED address and handshake; success means REUSE and
//!   the lock is NOT removed on exit; a MISSING lock, a lock whose text gives
//!   NO usable nonce (a failed handshake by construction) or a STALE one
//!   (nothing listens at the endpoint) -> start a core daemon which binds
//!   the configured address, writes the lock and serves it; a lock that
//!   EXISTS but CANNOT BE READ (`transport_cfg`'s typed
//!   [`LockReading.Unreadable`]) is a TYPED refusal NAMING THE CONFIGURED
//!   ADDRESS with NO SPAWN — the doc's rule is that an EXISTING lock means
//!   CONNECT-and-HANDSHAKE, and NOTHING here may declare its writer GONE;
//!   a port held by an unrelated process fails the handshake and is an error
//!   NAMING THE CONFIGURED ADDRESS and no command is ever sent to it;
//!   process-id liveness is never the test, and this module never inspects
//!   a pid: the handshake IS the test.
//! * **Sharing** (`architecture.md:365`): several interfaces share one daemon and
//!   the core serializes commands. In this build the daemon serves ONE connection
//!   at a time, so the sharing rule holds BY CONSTRUCTION: one session's
//!   interpreter loop writes its command's whole event block to its own socket
//!   before the daemon reads another connection. See the deferred item below.
//! * **Shutdown** (`core-protocol.md:192-201`, `:16`): the `shutdown` command
//!   exits the daemon and removes the lock; the idle timeout does the same.
//! * **Unknown input** (`core-protocol.md:203-209`): an unparseable line is
//!   answered by the interpreter and the daemon stays alive; a partial line is
//!   never resynchronized — which is also why the pending text of a peer is kept
//!   pending here, and capped (see the bound below).
//!
//! # Decisions this module had to make (doc is silent, so they are RECORDED)
//!
//! * **Idle timeout is a typed parameter, not a config key.** `core-protocol.md:16`
//!   and `architecture.md:366` call it "configurable, default 15 minutes", but
//!   `data-files.md:146-152` lists the configuration keys and there is NO
//!   timeout key, and plan rule 8 forbids inventing one. It is
//!   [`DEFAULT_IDLE_SECONDS`] passed down as a value, and the daemon CHILD learns
//!   it through its own argv ([--idle-timeout=...]). A doc amendment + USER
//!   decision is owed for where the value is configured.
//! * **The address precedence.** `core.address` may be written as the `host:port`
//!   text the lock file uses; `core.port` then OVERRIDES the port inside it,
//!   because two keys cannot both be "the address" of `core-protocol.md:14`.
//! * **The lock file is read for the nonce only** (`core-protocol.md:37`, `:41-43`):
//!   the address inside it is compared nowhere in this build — the doc's own rule
//!   is that a lock disagreeing with the configuration FAILS the handshake, and
//!   the handshake fails by itself when the configured address holds a daemon
//!   with another nonce. Carrying the lock address into the error text is left
//!   to a later item.
//! * **The daemon mints its own nonce** (`architecture.md:249`: the daemon writes
//!   the lock, the interface READS it), from its own pid, the endpoint and the
//!   lock path, hashed with this package's own [`crate::technical::id`]
//!   helpers — decision 33, which also dropped `proto`'s digest-to-nonce route
//!   (it implied an undocumented channel from the interface to the child it
//!   spawns). The 16-hex-char SHAPE rule stays in `proto::handshake` as a
//!   recorded invention matching the doc example.
//! * **The session id of a connection** is derived from the lock nonce
//!   (`proto::handshake::session_id_of`) and handed to
//!   `interpret::run_session`, replacing `interpret::DEFAULT_SESSION` — decision
//!   28. `core-protocol.md:74` shows the informational value `core-1`, so a
//!   doc amendment is owed (recorded in `proto/src/handshake.rs` too).
//! * **The transport owns the handshake-before-commands obligation** (decision
//!   29/30): the interpreter cannot see ordering across a session boundary, so
//!   [`open_session`] writes the handshake frame, reads exactly ONE answer line,
//!   and hands out the connection ONLY if `proto::handshake::check_handshake_answer`
//!   answers [`proto::handshake::HandshakeCheck.Echoed`]. Nothing else is ever
//!   written to a peer that failed that check, which is what makes
//!   "never sends commands to an unknown process" (`architecture.md:250`)
//!   observable in the tests.
//! * **The pending line is bounded here.** `proto::codec::Framing.partial` is
//!   unbounded by design (the codec stays pure), so a peer streaming without `\n`
//!   would grow it forever: [`PENDING_MAX_BYTES`] is this transport's own
//!   resource limit and exceeding it DROPS the connection with a typed reason
//!   (an invented limit, recorded; the docs fix no maximum line length).
//! * **The executable of the daemon child is DATA** (`std::process::self_exe_path`
//!   does not exist): the caller hands in its own `argv[0]`, and the
//!   integration tests hand in `CARGO_BIN_EXE_core`. The daemon mode is selected
//!   in `core/src/main.rs` by the flag `--serve` — an invented flag, recorded
//!   here and in `main.rs`, which plan 6.1's CLI will adopt; the pipe session of
//!   plan items 3.4/3.5 stays the DEFAULT and its tests are unchanged.
//! * **No stderr diagnostics in this module.** The failure reason travels as
//!   typed data; `core/src/main.rs`'s daemon branch decides what a user sees, and
//!   decision 20 keeps the event stream itself free of non-protocol output.
//! * **A daemon error is reported on stderr by `main.rs`** and the exit status is
//!   the pipe session's table, which already returns 1 for a broken stream.
//!
//! # Deferred, and who owes it
//!
//! * Concurrent serving of several open sessions (threads and daemon-wide shared
//!   state): see the sharing paragraph above — the interleaving test of
//!   `core/tests/transport_lifecycle.rs` pins the ORDER that is achievable HERE
//!   and records what is NOT.
//! * Cancelling a command of a shared session, `cancel` across sessions, and the
//!   wired verbs of plan 4.x.

use std::io::Read;
use std::io::Write;
use std::net::TcpListener;
use std::net::TcpStream;
use std::time::Duration;

use crate::protocol::interpret;
use crate::protocol::writer::EventSink;
use crate::technical::fs_atomic;
use proto::codec::error_text;
use proto::handshake::LockRecord;
use proto::handshake::check_handshake_answer;
use proto::handshake::HandshakeCheck;
use proto::handshake::is_wellformed_nonce;
use proto::handshake::session_id_of;
use proto::handshake::version_request_frame;
use proto::commands::PROTOCOL_VERSION;
// ONLY the TESTS of this module read JSON now (the production config reading
// moved to `transport_cfg`, sub-row 3.6b) — and the tests name `Map`
// QUALIFIED, so only `Value` is imported, TEST-ONLY:
#[cfg(test)]
use serde_json::Value;

// ---------------------------------------------------------------------------
// Sub-row 3.6b (the USER's split of plan item 3.6 into 3.6a/3.6b/3.6c, decision
// 38) moved the CONFIG/LOCK/ENDPOINT/NONCE half of this row — the address
// names, the `phin.toml` reading, the `state/core.lock` file and the nonce
// mint — into the sibling module `core/src/technical/transport_cfg.rs`, with
// its OWN tests there. Sub-row 3.6c (this module, now the split is finished)
// REWIRED the two callers that still named these types through `transport::`
// — `core/src/main.rs`'s daemon branch and `core/tests/transport_lifecycle.rs`
// — onto `core::technical::transport_cfg::` DIRECTLY and deleted the temporary
// re-export block: `transport_cfg` is the single source and this module merely
// CONSUMES it, by an ordinary `use` for the names its production code calls
// plus a TEST-ONLY `use` for the names only its own tests read.
// ---------------------------------------------------------------------------

use crate::technical::transport_cfg::{
    ConfigIssue,
    Endpoint,
    LockReading,
    Removal,
    endpoint_complaint,
    endpoint_from_config,
    parse_endpoint,
    endpoint_text,
    lock_path,
    mint_nonce,
    read_lock,
    remove_lock,
    write_lock,
    DEFAULT_PORT
};
#[cfg(test)]
use crate::technical::transport_cfg::{CONFIG_FILE_NAME, DEFAULT_ADDRESS, LOCK_FILE_NAME, STATE_DIR_NAME};

// [`StreamText`]'s unit tests call the TRAIT METHOD `read_text` DIRECTLY
// (`interpret::TextInput`, plan 3.4's seam), which THIS toolchain only
// resolves with the TRAIT itself in scope — production hands the stream
// to `interpret::run_session`, which reaches the seam from ITS side:
#[cfg(test)]
use crate::protocol::interpret::TextInput;

/// The idle timeout this build defaults to: 15 MINUTES without commands
/// (`core-protocol.md:16`), named because the value crosses a process boundary
/// and the config keys of `data-files.md:146-152` hold no key for it.
pub const DEFAULT_IDLE_SECONDS: i64 = 15 * 60;

/// The argv flag that selects daemon mode in `core/src/main.rs`. Invented HERE
/// (decision 35): the row of plan item 3.6 has no CLI, plan 6.1 owns the
/// user-facing one, and the daemon CHILD of this row has to be told daemon mode,
/// the endpoint, the data directory and the idle timeout SOMEHOW.
pub const DAEMON_FLAG: &str = "--serve";

/// The argv flags carrying the daemon child's parameters (see [`DAEMON_FLAG`]).
pub const ADDRESS_FLAG: &str = "--address=";
pub const PORT_FLAG: &str = "--port=";
pub const DATA_DIR_FLAG: &str = "--data-dir=";
pub const IDLE_FLAG: &str = "--idle-timeout=";

/// The largest pending, still unterminated text one connection may hold
/// ([PENDING_MAX_BYTES] in the module doc): a resource limit of THIS transport,
/// not a protocol rule.
pub const PENDING_MAX_BYTES: usize = 65_536;
// ---------------------------------------------------------------------------
// One connection: the pair the interpreter of plan 3.4 is fed
// ---------------------------------------------------------------------------

/// The read side of one connection: the abstract command stream
/// [`interpret::run_session`] reads, over a socket instead of over stdin.
#[derive(Debug)]
pub struct StreamText {
    /// The socket this side reads.
    pub reader: TcpStream,

    /// The text received after the last line end, kept exactly as `proto`'s
    /// framing keeps a partial line — "the stream never resynchronizes on a
    //  partial line" (`core-protocol.md:203-209`) — and bounded by
    /// [`PENDING_MAX_BYTES`] (see the module doc).
    pub pending: String,

    /// Set once a read failed or the peer closed its side, so nothing more can
    /// arrive. "Ended" and "failed" stay DIFFERENT, exactly as `StdinText` of
    /// `core/src/main.rs` keeps them apart, because plan 3.4's exit-code table
    /// reads that difference: [`interpret::TextRead.Done`] versus
    /// [`interpret::TextRead.Unreadable`].
    pub ended: bool,

    /// Why the stream broke, when it broke rather than ended.
    pub unreadable: String,

    /// Set when a read reached the timeout this connection was given: for a
    /// connection that is the doc's idle condition (`core-protocol.md:16`), NOT
    /// a failure, and the serve loop of the daemon reads this field.
    pub timed_out: bool,

    /// Set when the pending text was flushed as a half line at the end of the
    /// stream, so the next read says "no more text" ([`interpret::Stop
    /// .HalfLineAtEnd`] is then what the interpreter reports).
    pub flushed: bool
}

/// Hand out the next text of the connection.
///
/// A read blocks until a line end arrives, the peer closes, or the timeout of
/// [`serve_connection`] fires; a chunk that ends mid-line is handed out ONLY at
/// the end of the stream, where nothing can complete it any more, which is what
/// keeps the half line PENDING in the interpreter's framing rather than decoded.
impl interpret::TextInput for StreamText {
    fn read_text(&mut self) -> interpret::TextRead {
        if self.ended {
            if self.unreadable.len() > 0 {
                return interpret::TextRead::Unreadable(self.unreadable.to_string());
            }
            return interpret::TextRead::Done;
        }
        if self.flushed {
            // The half line was already handed out; the stream is over.
            self.ended = true;
            return interpret::TextRead::Done;
        }
        loop {
            let mut buffer = vec![0u8];
            match self.reader.read(&mut buffer) {
                Err(reason) => {
                    let kind = reason.kind();
                    if kind == std::io::ErrorKind::WouldBlock || kind == std::io::ErrorKind::TimedOut {
                        // The idle timeout of this connection: the session ends
                        // CLEANly, and the field says why.
                        self.timed_out = true;
                        return interpret::TextRead::Done;
                    }
                    let message = format!("{reason}");
                    self.ended = true;
                    self.unreadable = message.to_string();
                    return interpret::TextRead::Unreadable(message);
                }
                Ok(read) => {
                    if read == 0 {
                        // End of the peer's side: a half line is handed out, and
                        // only then does the stream end.
                        self.ended = true;
                        if self.pending.len() > 0 {
                            let half = self.pending.to_string();
                            self.pending = String::new();
                            self.flushed = true;
                            return interpret::TextRead::Text(half);
                        }
                        return interpret::TextRead::Done;
                    }
                    // The bytes of one read are decoded as UTF-8. A read that
                    // cut a multi-byte character in half is reported below as an
                    // unreadable stream, and this transport NEVER decodes such
                    // text into a message (`proto::codec::decode_line` of plan 3.1
                    // would reject the line in any case).
                    match std::string::String::from_utf8(buffer) {
                        Err(_) => {
                            self.ended = true;
                            self.unreadable = "the peer sent bytes that are not UTF-8".to_string();
                            return interpret::TextRead::Unreadable(self.unreadable.to_string());
                        }
                        Ok(decoded) => { self.pending += &decoded; }
                    }
                    if self.pending.len() > PENDING_MAX_BYTES {
                        // This transport's own resource limit: a peer that
                        // streams without a line end is DROPPED rather than
                        // allowed to grow the pending text forever.
                        let shown = self.pending.len();
                        self.ended = true;
                        self.unreadable = format!("pending text exceeded {PENDING_MAX_BYTES} bytes without a line end ({shown} bytes)");
                        return interpret::TextRead::Unreadable(self.unreadable.to_string());
                    }
                    if self.pending.contains("\n") {
                        let chunk = self.pending.to_string();
                        self.pending = String::new();
                        return interpret::TextRead::Text(chunk);
                    }
                }
            }
        }
    }
}

/// The write side of one connection: the event sink of plan 3.3 over a socket
/// instead of over stdout.
#[derive(Debug)]
pub struct LatchSink {
    /// The socket this side writes.
    pub writer: TcpStream,

    /// Set by the FIRST frame this sink failed to deliver, and kept, so that
    /// every later frame of the session is DROPPED: plan 3.3's writer has
    /// already counted that command as terminal and cannot observe the failure
    /// (`core/src/protocol/writer.rs` puts that obligation on the sink of plan
    /// 3.4, and states that the same obligation applies to the sink of plan 3.6).
    /// A failure of EITHER the write or the flush latches, exactly as
    /// `LatchLines` of `core/src/main.rs` does.
    pub broken: bool
}

impl EventSink for LatchSink {
    fn write_line(&mut self, line: String) {
        if self.broken {
            // Dropped on purpose: this is the line after the stream broke.
        } else {
            match self.writer.write_all(line.as_bytes()) {
                Err(_) => { self.broken = true; }
                Ok(_) => {
                    match self.writer.flush() {
                        Err(_) => { self.broken = true; }
                        Ok(_) => { }
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// The connection of one session: handshake first, then the pair
// ---------------------------------------------------------------------------

/// The pair plan 3.4's loop is fed, plus what identifies the session it belongs
/// to. [`interpret::run_session`] takes the read side and the write side
/// SEPARATELY, which is what the doc means by "the socket pair": one socket,
/// two handles (`TcpStream::try_clone` shares the file descriptor, as a socket
/// pair of a local daemon does).
#[derive(Debug)]
pub struct Connection {
    /// The abstract command stream of [`interpret::run_session`].
    pub text: StreamText,

    /// The event sink of plan 3.3, over the same socket.
    pub sink: LatchSink,

    /// The endpoint this connection was made to, as `core-protocol.md:26`
    /// writes it.
    pub address: String,

    /// The session id `proto::handshake::session_id_of` derives from the nonce
    /// this connection announced; `core-protocol.md:385` says a session that
    /// ends at the socket keeps that id, and the caller who starts a NEW
    /// connection must NOT reuse it (`commands.rs:87`).
    pub session: String
}

/// Why one attempt at an endpoint came down to what it came down to. One enum
/// serves BOTH the reuse question of `architecture.md:247-248` and the handshake
/// obligation of `core-protocol.md:15`, because they are the SAME procedure
/// seen from two sides: what cannot be commanded is what must not be asked.
#[derive(Debug, Eq, PartialEq)]
pub enum Attempt {
    /// The endpoint was reached, the version answer was read and checked, and
    /// the session pair was handed out: the texts are the version the peer
    /// reports and the session id.
    Handshook(String, String),
    /// Nothing listens at the endpoint: the connection itself failed. The texts
    /// are the endpoint and the OS's own message.
    NothingListening(String, String),
    /// Something listens, but what answers the version request is NOT a core
    /// daemon (`core-protocol.md:11` calls the daemon the "ONLY answerer of
    /// version requests at this address"). Clause (d): this transport refuses
    /// rather than commanding a foreign listener. The texts are the endpoint
    /// and `proto`'s complaint from `handshake.rs:36`.
    NotOurDaemon(String, String),
    /// The version the peer reports does not match the version this build
    /// speaks: `handshake.rs:36`'s `VersionMismatch` carrying BOTH numbers, plus
    /// the endpoint. `commands.rs:87` keeps this "fatal for the session" and the
    /// daemon is NOT sent a command line; a version TRANSITION is plan 3.13's
    /// ("the upgrade story"), which is why this transport neither kills the
    /// older daemon nor starts a second one at the same endpoint.
    VersionProblem(i64, i64, String),
    /// The frame naming the version request could not be written: the texts are
    /// the endpoint and the OS's own message. NOTHING was ASKED of this
    /// endpoint, which is why this is not a handshake outcome at all.
    FrameUnwritten(String, String),
    /// The stream broke before an answer line ended: endpoint, OS message.
    Broke(String, String),
    /// The answer line is not decodable by plan 3.1's codec: endpoint and
    /// `proto`'s own message.
    Undecodable(String, String),
    /// The handshake answer carries no `version` field at all, so the version
    /// THIS BUILD SPEAKS is not substituted for it (`core-protocol.md:391-393`:
    /// "`answer` is checked ... and `answer.version` is the number that is
    /// compared"). Endpoint and session id.
    NoVersionField(String, String),
    /// The peer streamed more than [`PENDING_MAX_BYTES`] bytes without a line
    /// end: the endpoint and the size seen.
    PendingFull(String, usize)
}

/// Why the endpoint cannot be handed a session at all.
#[derive(Debug, Eq, PartialEq)]
pub enum Why {
    /// The data directory holds no `state/core.lock` file, and nothing listens
    /// at the endpoint either: `architecture.md:247`.
    NoLockFile,
    /// A lock file is there, but this build cannot read it; the text is
    /// `handshake.rs:38`'s own complaint. The daemon that wrote it is NOT in
    /// charge here, and the daemon this flow starts OVERWRITES the file
    /// (`data-files.md:143`: the one lock file is the current daemon's).
    LockUnreadable(String),
    /// Nothing listens at the endpoint the lock (or the configuration) names,
    /// which is how a lock left behind by a daemon that has since EXITED reads:
    /// clause (c). The text is the endpoint.
    NothingAt(String),
    /// The peer at the endpoint reports a different protocol version, and the
    /// endpoint is already taken, so starting another core there is NOT this
    /// plan's job (a version transition is plan 3.13's). Both numbers are
    /// kept.
    VersionOf(i64, i64),
    /// Something listens, but it is not a core daemon: clause (d), and the
    /// text is `proto`'s complaint.
    Foreign(String),
    /// A handshake answer carried no `version` field, and the version this
    /// build speaks was NOT substituted.
    NoVersionAnswer,
    /// The version request frame could not be written to the endpoint.
    FrameUnwritten,
    /// The stream broke before an answer line ended.
    Broke,
    /// A frame of the answer could not be decoded by plan 3.1's codec.
    Undecodable,
    /// The peer streamed more than [`PENDING_MAX_BYTES`] bytes without a line
    /// end.
    PendingFull
}

/// Why a session was not handed out. Clause (d) depends on the FIRST text naming
/// the configured address, so the complaint is assembled here rather than
/// left for a caller to invent.
#[derive(Debug, Eq, PartialEq, Clone)]
pub enum SessionFailure {
    /// Something listens at the configured address but is NOT our daemon
    /// (`architecture.md:250`): the texts are the CONFIGURED ADDRESS and
    /// `proto`'s complaint. No non-handshake line was ever sent to it —
    /// see [`open_session`], which reads and checks ONE answer line before
    /// a pair exists at all.
    /// (Variant order kept: the FIRST variant stays the FIRST variant so
    /// `refuse`'s mapping and every caller's `match` are unaffected.)
    /// The `state/core.lock` file EXISTS but this build could not READ it,
    /// and `architecture.md:247`'s discovery rule is "IF A LOCK FILE EXISTS,
    /// the interface CONNECTS and HANDSHAKES" — with the contents unreadable
    /// NO handshake can even be ASKED, and NOTHING here may declare the
    /// daemon that wrote it GONE (process-id liveness is NEVER the test,
    /// `architecture.md:247`), so this flow REFUSES with NO SPAWN of a
    /// SECOND daemon. The texts are the CONFIGURED ADDRESS and `std::io`'s
    /// OWN complaint (`transport_cfg`'s typed [`LockReading.Unreadable`]).
    LockUnreadable(String, String),
    NotAtEndpoint(String, String),
    /// The endpoint could not be read from the configuration file; the list
    /// is [`endpoint_from_config`]'s typed complaints.
    ConfigRejected(Vec::<ConfigIssue>),
    /// The peer reports a protocol version this build does not speak; a
    /// version TRANSITION is plan 3.13's, so this transport starts no
    /// second daemon at the taken address. The texts are the version seen,
    /// the version this build speaks, and the configured address.
    VersionSpoken(i64, i64, String),
    /// The daemon child of this flow could not be started at all; the texts
    /// are the configured address and the reason (an executable path that
    /// does not exist, a data directory whose path is not text — the DATA
    /// the spawn seam needs, see [`Spawner`]).
    SpawnRefused(String, String),
    /// A daemon child WAS started and it never handed out a checked
    /// handshake at the configured address within
    /// [`DEFAULT_SPAWN_WAIT_SECONDS`] (or the caller-given wait); the texts
    /// are the configured address and why the last attempt failed.
    SpawnedUnreachable(String, String)
}

/// The seam that starts the daemon child, taking the executable path and the
/// argument list as DATA (`spawn_arguments` builds them from the endpoint,
/// the data directory and the idle timeout — the flags of
/// [`DAEMON_FLAG`] and friends). A SEAM, not a direct call, because clause
/// (a) of the row's tests — "no daemon -> the SPAWN path is taken" — must
/// be observable IN-PROCESS without a process: the tests hand in a recorder
/// that stores the argv it was given and starts nothing.
pub trait Spawner {
    /// Start `executable` with `args`, and say whether the START was made.
    fn start(&mut self, executable: String, args: Vec::<String>) -> bool;
}

/// The PRODUCTION spawner: `std::process::Command`, and a DROPPED `Child`
/// detaches it (toolchain fact of this row's cycle-1 log), so the daemon
/// child outlives the interface process that started it, as
/// `architecture.md:249` needs ("the interface starts a core daemon
/// process, which ... performs the handshake" — the DAEMON then lives on
/// for the next interface).
#[derive(Debug)]
pub struct SpawnByArgv { }

impl Spawner for SpawnByArgv {
    fn start(&mut self, executable: String, args: Vec::<String>) -> bool {
        match std::process::Command::new(&executable).args(&args).spawn() {
            Err(_) => { return false; }
            // The child is DROPPED on purpose: holding it would make this
            // transport the parent that must WAIT, and the daemon must
            // outlive the session that started it.
            Ok(_) => { return true; }
        }
    }
}

/// The arguments of the daemon child, as DATA: the daemon mode flag and the
/// four parameter flags ([`DAEMON_FLAG`], [`ADDRESS_FLAG`], [`PORT_FLAG`],
/// [`DATA_DIR_FLAG`], [`IDLE_FLAG`]) carrying the endpoint, the data directory
/// and the idle timeout, each flag and its value as ONE argv entry. ([`Spawner.start`]
/// takes the EXECUTABLE path separately, as DATA: `std::process::self_exe_path`
/// does NOT exist in this toolchain — E0425, a toolchain fact of this row —
/// so the CALLER's `argv[0]` is what `core/src/main.rs` hands, and the
/// integration tests hand `CARGO_BIN_EXE_core`.)
///
/// [`None`] is the ONE refusal this leaves the caller: a data directory whose
/// path cannot be handed over as text at all (`to_str`), which no argument
/// could carry faithfully; `connect_or_spawn` reports it as
/// [`SessionFailure.SpawnRefused`].
pub fn spawn_arguments(endpoint: &Endpoint, data_dir: &std::path::Path, idle_seconds: i64) -> Option::<Vec::<String>> {
    match data_dir.to_str() {
        None => { return None; }
        Some(held) => {
            let args = [
                DAEMON_FLAG.to_string(),
                format!("{flag}{value}", flag = ADDRESS_FLAG, value = endpoint.host),
                format!("{flag}{value}", flag = PORT_FLAG, value = endpoint.port.to_string()),
                format!("{flag}{value}", flag = DATA_DIR_FLAG, value = held.to_string()),
                format!("{flag}{value}", flag = IDLE_FLAG, value = idle_seconds.to_string())
            ].to_vec();
            return Some(args);
        }
    }
}

/// The wait for a freshly SPAWNED daemon to write its lock file and answer
/// the handshake at the configured address: 5 SECONDS, a RECORDED INVENTION
/// (`architecture.md:249` fixes THAT the interface starts the daemon and
/// handshakes the daemon it started, never how long the daemon may take to
/// appear); [`connect_or_spawn`] lets a caller pass its own value, which is
/// how the tests stay quick.
pub const DEFAULT_SPAWN_WAIT_SECONDS: i64 = 5;

/// The interval of the discovery wait's polls, milliseconds: small next to
/// the 5 SECONDS above, and the same NON-BLOCKING-poll style as the daemon
/// accept-loop (no poll primitive, no channel in this toolchain).
const DISCOVERY_POLL_MILLIS: i64 = 50;

/// Turn a refused [`Attempt`] into the typed interface failure of
/// `architecture.md:250` — every text naming the CONFIGURED address, and the
/// pair NEVER handed out. Clause (d)'s "no command sent" is guaranteed
/// UPSTREAM by [`open_session`], which wrote the version request frame and
/// read ONE answer line and NOTHING else before this mapping runs.
fn refuse(attempt: Attempt, address: String) -> Vec::<SessionFailure> {
    match attempt {
        Attempt::NotOurDaemon(at, complaint) => { return [SessionFailure::NotAtEndpoint(at.to_string(), complaint.to_string())].to_vec(); }
        Attempt::Undecodable(at, complaint) => { return [SessionFailure::NotAtEndpoint(at.to_string(), complaint.to_string())].to_vec(); }
        Attempt::Broke(at, complaint) => { return [SessionFailure::NotAtEndpoint(at.to_string(), complaint.to_string())].to_vec(); }
        Attempt::FrameUnwritten(at, complaint) => { return [SessionFailure::NotAtEndpoint(at.to_string(), complaint.to_string())].to_vec(); }
        Attempt::PendingFull(at, seen) => { return [SessionFailure::NotAtEndpoint(at.to_string(), format!("the peer streamed {seen} bytes without a line end", seen = seen))].to_vec(); }
        Attempt::NoVersionField(at, _) => { return [SessionFailure::NotAtEndpoint(at.to_string(), "the handshake answer carried no version field".to_string())].to_vec(); }
        Attempt::VersionProblem(seen, spoken, at) => { return [SessionFailure::VersionSpoken(seen, spoken, at.to_string())].to_vec(); }
        Attempt::NothingListening(at, complaint) => { return [SessionFailure::SpawnedUnreachable(at.to_string(), complaint.to_string())].to_vec(); }
        Attempt::Handshook(_, _) => { return [SessionFailure::NotAtEndpoint(address, "a handshake answered without a session pair".to_string())].to_vec(); }
    }
}

/// The interface-side discovery of `architecture.md:247-250`, in one call:
///
/// * the endpoint comes from the configuration ([`endpoint_from_config`]);
/// * a readable lock file means CONNECT to the CONFIGURED address and
///   HANDSHAKE ([`open_session`]) — success is REUSE, and the lock file is
///   NOT removed here (`architecture.md:248`: nothing on the interface side
///   of this module removes a lock; see [`remove_lock`]'s doc);
/// * a MISSING lock, a lock whose readable text gives NO usable nonce
///   (clause (c): the handshake is IMPOSSIBLE, so it FAILED by construction
///   and the fresh daemon OVERWRITES the file) or a STALE lock (NOTHING
///   listens at the endpoint) means SPAWN ([`Spawner`] with [`spawn_arguments`]
///   -built flags) and then connect + HANDSHAKE with the nonce the
///   SPAWNED daemon itself wrote; a lock that EXISTS but CANNOT BE READ
///   is the typed [`SessionFailure.LockUnreadable`] REFUSAL — the doc
///   forbids starting a SECOND daemon while a lock EXISTS
///   (`architecture.md:247`):
///   this call RE-READS the lock after the spawn and NEVER re-uses the
///   stale nonce (decision 33 — the DAEMON mints), which is clause (c)
///  's "lock overwritten" seen from the interface;
/// * a listener that is NOT our daemon is an error NAMING THE CONFIGURED
///   ADDRESS and NO spawn happens (the address is TAKEN,
///   `architecture.md:250`), with clause (d)'s "no command sent" pinned by
///   [`open_session`] writing the version request frame and reading ONE
///   answer line and NOTHING else.
///
/// A successful pair is handed to the caller ONLY after the checked answer
/// of [`open_session`], so [`interpret.run_session`] CANNOT be entered
/// before a pair exists; what the caller feeds it is clause (e)'s business.
pub fn connect_or_spawn(executable: String, data_dir: &std::path::Path, idle_seconds: i64, handshake_seconds: i64, spawn_wait_seconds: i64, spawner: &mut dyn Spawner) -> (Option::<Connection>, Vec::<SessionFailure>) {
    let endpoint_read = endpoint_from_config(data_dir);
    match endpoint_read {
        (None, issues) => {
            return (None, [SessionFailure::ConfigRejected(issues)].to_vec());
        }
        (Some(endpoint), config_issues) => {
            if config_issues.len() > 0 {
                return (None, [SessionFailure::ConfigRejected(config_issues)].to_vec());
            }
            let address = endpoint_text(endpoint.host.to_string(), endpoint.port);
            let lock_read = read_lock(data_dir);
            let mut stale_nonce = String::new();
            match lock_read {
                LockReading::Read(record) => { stale_nonce = record.nonce.to_string(); }
                LockReading::Unreadable(kind, complaint) => {
                    // THE LOCK FILE EXISTS but this build CANNOT READ IT.
                    // `architecture.md:247`: "IF A LOCK FILE EXISTS, THE
                    // INTERFACE CONNECTS and HANDSHAKES" — with the contents
                    // unreadable NO handshake can even be ASKED of the
                    // endpoint, and NOTHING HERE may declare the daemon that
                    // WROTE the lock GONE (process-id liveness is NEVER the
                    // test, `architecture.md:247`, and this module never
                    // inspects a pid), so starting a SECOND daemon is
                    // FORBIDDEN: the flow REFUSES with a typed failure NAMING
                    // THE CONFIGURED ADDRESS and NO SPAWN happens. (The
                    // OLD tuple return read this exact state as ABSENT —
                    // the bug the review of plan item 3.6b recorded.)
                    let message = format!("the lock file of [{dir}] exists but could not be read ({why}): {seen}", dir = lock_path(data_dir).display(), why = kind.to_string(), seen = complaint);
                    return (None, [SessionFailure::LockUnreadable(address, message)].to_vec());
                }
                LockReading::Malformed(partial, _) => {
                    // The TEXT was READ and `proto`'s parser REFUSED it
                    // (missing/malformed VALUE, unknown or repeated key). If
                    // it STILL carries a WELLFORMED NONCE the file says WHO
                    // the daemon is, so CONNECT and HANDSHAKE as usual — a
                    // PARSE complaint is NOT a failed handshake; otherwise
                    // NO handshake is possible, which IS a FAILED HANDSHAKE
                    // (`architecture.md:247`), so the SPAWN path below runs
                    // and the daemon this flow starts OVERWRITES the file —
                    // clause (c)'s "lock OVERWRITTEN" reading.
                    match partial {
                        Some(record) => {
                            if is_wellformed_nonce(&record.nonce) {
                                stale_nonce = record.nonce.to_string();
                            }
                        }
                        None => { }
                    }
                }
                LockReading::Absent => { }
            }
            if stale_nonce.len() > 0 {
                // The lock is READABLE and names a nonce: CONNECT and
                // HANDSHAKE (`architecture.md:247`). Success is REUSE
                // and the lock file stays (nothing here removes it).
                let attempt = open_session(&endpoint, stale_nonce.to_string(), handshake_seconds, idle_seconds);
                match attempt {
                    (Some(connection), _) => {
                        return (Some(connection), Vec::<SessionFailure>::new());
                    }
                    (None, why) => {
                        // Clause (c): ONLY the "nothing listens there" reading
                        // of the stale lock falls through to the SPAWN below
                        // ("the daemon this flow starts OVERWRITES the file");
                        // every other refusal is FINAL, the address is TAKEN
                        // (`architecture.md:250`), and NO SPAWN happens.
                        match why {
                            Attempt::NothingListening(_, _) => { }
                            _ => {
                                return (None, refuse(why, address.to_string()));
                            }
                        }
                    }
                }
            }
            // The SPAWN path: clauses (a) and (c).
            let built = spawn_arguments(&endpoint, data_dir, idle_seconds);
            match built {
                None => {
                    return (None, [SessionFailure::SpawnRefused(address.to_string(), "the data directory path could not be handed to the daemon child as text".to_string())].to_vec());
                }
                Some(args) => {
                    if spawner.start(executable.to_string(), args) != true {
                        return (None, [SessionFailure::SpawnRefused(address.to_string(), "the daemon child could not be started".to_string())].to_vec());
                    }
                    // Wait for the SPAWNED daemon: ITS OWN fresh lock file
                    // (the nonce of the daemon that wrote it, decision 33 —
                    // the STALE nonce above is NEVER handshaked a second
                    // time), and THEN the handshake with that nonce. Waiting
                    // on the lock CHANGING is what keeps "process-id
                    // liveness is NEVER the test" (`architecture.md:250`) true
                    // in code, not just in prose.
                    let total_millis = spawn_wait_seconds * 1000;
                    let mut waited: i64 = 0;
                    let mut last_complaint = "the spawned daemon had written no usable lock file yet".to_string();
                    loop {
                        let mut fresh_nonce = String::new();
                        match read_lock(data_dir) {
                            LockReading::Read(record) => { fresh_nonce = record.nonce.to_string(); }
                            LockReading::Malformed(partial, _) => {
                                // A partial-but-USABLE fresh lock (a
                                // repeated or unknown KEY the parser
                                // reported while the NONCE read FINE) counts:
                                // the nonce of the daemon that WROTE it is
                                // what the handshake asks for.
                                match partial {
                                    Some(record) => {
                                        if is_wellformed_nonce(&record.nonce) {
                                            fresh_nonce = record.nonce.to_string();
                                        }
                                    }
                                    None => { }
                                }
                            }
                            // ABSENT or UNREADABLE YET: the SPAWNED daemon
                            // has not written its lock READABLY (the
                            // BOUNDED budget below decides, never a pid
                            // check).
                            _ => { }
                        }
                        if fresh_nonce.len() > 0 && fresh_nonce != stale_nonce {
                            let attempt = open_session(&endpoint, fresh_nonce, handshake_seconds, idle_seconds);
                            match attempt {
                                (Some(connection), _) => {
                                    return (Some(connection), Vec::<SessionFailure>::new());
                                }
                                (None, why) => {
                                    match why {
                                        Attempt::NothingListening(_, complaint) => {
                                            last_complaint = complaint;
                                        }
                                        _ => {
                                            return (None, refuse(why, address.to_string()));
                                        }
                                    }
                                }
                            }
                        }
                        if waited >= total_millis {
                            return (None, [SessionFailure::SpawnedUnreachable(address.to_string(), format!("the spawned daemon never answered a checked handshake at [{at}] within {waited} ms: {why}", at = address.to_string(), waited = waited, why = last_complaint))].to_vec());
                        }
                        std::thread::sleep(millis_pause(DISCOVERY_POLL_MILLIS));
                        waited += DISCOVERY_POLL_MILLIS;
                    }
                }
            }
        }
    }
}

// =============================================================================
// One connection of EITHER side: the handshake that gates the pair
// =============================================================================

/// Ask an endpoint for its version, and hand out the session pair when the
/// answer says it is a core daemon of THIS build's protocol version.
///
/// The order is fixed, and the tests of this module pin it:
///
/// 1. validate the nonce with `proto::handshake::is_wellformed_nonce` — the
///    obligation `handshake.rs:267-272` puts on EVERY builder of a
///    handshake request (3.6a review finding 4; `transport_cfg` states the
///    same contract): a NONCE that is not a wellformed instance id is
///    refused BEFORE the socket is even opened, as a typed
///    [`Attempt.FrameUnwritten`], and NO frame is ever asked for;
/// 2. connect, with the read timeout of `core-protocol.md:16` set on the socket;
/// 3. write the frame naming the version request (`handshake.rs:26`);
/// 4. read ONE line;
/// 5. check it with `proto`'s checker of `handshake.rs:36`;
/// 6. only then hand the pair to the caller, so that NO command of any kind
///    reaches this transport before a checked handshake answer — clause (d)
///    and finding 1.2 turn on this, and `interpret::run_session` of plan 3.4
///    CANNOT be entered before a pair exists.
pub fn open_session(endpoint: &Endpoint, nonce: String, handshake_seconds: i64, idle_seconds: i64) -> (Option::<Connection>, Attempt) {
    let address = endpoint_text(endpoint.host.to_string(), endpoint.port);
    // STEP 0 of the gate — `transport_cfg`'s `parse_endpoint` WIRED INTO
    // IT (it had NO production caller, dead surface the 0-warning gate
    // cannot flag): a hand-built endpoint with an EMPTY HOST or a port
    // outside `0..65_535` names NOTHING that could be asked, so the
    // refusal is typed `Attempt.FrameUnwritten` (NOTHING was asked of
    // this endpoint — this is NOT an outcome of any peer) and NO SOCKET
    // IS OPENED AT ALL. `endpoint_from_config` refuses such an endpoint
    // upstream already; this is the CALLER-SIDE half of the same
    // contract as the nonce validation below (3.6a review finding 4).
    match parse_endpoint(endpoint.host.to_string(), endpoint.port) {
        (None, issues) => {
            let mut complaints = String::new();
            for held in issues {
                if complaints.len() > 0 {
                    complaints.push_str("; ");
                }
                complaints.push_str(&endpoint_complaint(&held));
            }
            return (None, Attempt::FrameUnwritten(address, complaints));
        }
        (Some(_), _) => { }
    }
    // 3.6a review finding 4: `version_request_frame` SWALLOWS a codec refusal
    // into `String::new` (`handshake.rs:267-272`), so the builder of a
    // handshake request validates the nonce FIRST — BEFORE ANY SOCKET IS
    // OPENED, so nothing can reach a peer on this build's behalf with a
    // malformed instance id. A lock file that names a bad nonce never
    // arrives this way through [`connect_or_spawn`] (`parse_lock` refuses
    // such a record, and the daemon of THIS build mints a wellformed one
    // BY CONSTRUCTION — see `transport_cfg`'s mint and its shape test);
    // this check is the CALLER-SIDE half of the contract, whatever a
    // hand-started caller hands in.
    if !is_wellformed_nonce(&nonce) {
        return (None, Attempt::FrameUnwritten(address, format!("the nonce [{held_nonce}] is not a wellformed instance id, so no version request frame was built", held_nonce = nonce)));
    }
    let socket_attempt = open_socket(endpoint);
    match socket_attempt {
        Err(reason) => {
            let message = format!("{reason}");
            return (None, Attempt::NothingListening(address, message));
        }
        Ok(socket) => {
            // The read timeout of the handshake read.
            set_read_deadline(&socket, handshake_seconds);
            match socket.try_clone() {
                Err(reason) => {
                    let message = format!("{reason}");
                    return (None, Attempt::Broke(address, message));
                }
                Ok(mut writer) => {
                    let frame = version_request_frame(&nonce);
                    if frame.len() == 0 {
                        // `handshake.rs:234` writes the empty string when its
                        // codec refuses; a nonce this module validates cannot
                        // produce that, so this is a refusal of THIS side to ASK,
                        // not an outcome of the peer: NOTHING was asked.
                        return (None, Attempt::FrameUnwritten(address, "the codec refused the version request frame".to_string()));
                    }
                    match writer.write_all(frame.as_bytes()) {
                        Err(reason) => {
                            let message = format!("{reason}");
                            return (None, Attempt::FrameUnwritten(address, message));
                        }
                        Ok(_) => {
                            let answer = read_answer_line(&writer, handshake_seconds);
                            match answer {
                                AnswerLine::Expired => {
                                    return (None, Attempt::Broke(address, format!("the answer line never ended within {handshake_seconds} seconds", handshake_seconds = handshake_seconds)));
                                }
                                AnswerLine::Broken(why) => {
                                    return (None, Attempt::Broke(address, why));
                                }
                                AnswerLine::TooLong(seen) => {
                                    return (None, Attempt::PendingFull(address, seen));
                                }
                                AnswerLine::Got(line) => {
                                    match check_handshake_answer(&nonce, &line) {
                                        HandshakeCheck::Echoed => {
                                            // The pair goes out ONLY here. The read
                                            // timeout is widened to the idle timeout
                                            // for the session itself (`core-protocol.md:16`:
                                            // the idle timeout IS the connection's
                                            // read timeout).
                                            set_read_deadline(&socket, idle_seconds);
                                            let session = session_id_of(&nonce);
                                            let text = StreamText {
                                                reader: socket,
                                                pending: String::new(),
                                                ended: false,
                                                unreadable: String::new(),
                                                timed_out: false,
                                                flushed: false
                                            };
                                            let sink = LatchSink { writer: writer, broken: false };
                                            return (Some(Connection { text: text, sink: sink, address: address.to_string(), session: session.to_string() }), Attempt::Handshook(PROTOCOL_VERSION.to_string(), session));
                                        }
                                        HandshakeCheck::NotEchoed(sent_back, announced) => {
                                            let complaint = format!("the listener answered the version request with the nonce [{asked_back}], not the nonce [{announced}] of this lock file: it is not our daemon", asked_back = sent_back, announced = announced);
                                            return (None, Attempt::NotOurDaemon(address, complaint));
                                        }
                                        HandshakeCheck::NotHello(name) => {
                                            let complaint = format!("the listener answered the version request with a [{name}] line, not a hello: it is not our daemon", name = name);
                                            return (None, Attempt::NotOurDaemon(address, complaint));
                                        }
                                        HandshakeCheck::UnreadableAnswer(reason) => {
                                            return (None, Attempt::Undecodable(address, error_text(&reason)));
                                        }
                                        HandshakeCheck::VersionMismatch(spoken, this_build) => {
                                            return (None, Attempt::VersionProblem(spoken, this_build, address));
                                        }
                                    }
                                }
                            }
                            // Every arm of [`AnswerLine`] returns, and every
                            // arm of the [`HandshakeCheck`] switch returns, so
                            // the scope DIVERGES without a tail: the
                            // "no version field" reading of
                            // [`Attempt.NoVersionField`] is NOT produced by
                            // `handshake.rs:36` (an absent `v` means THIS
                            // build's version there), and this module keeps
                            // that variant for a checker that GROWS such a
                            // reading rather than inventing an outcome now.
                        }
                    }
                }
            }
        }
    }
}

/// The text ONE line of the peer's stream, read for the handshake of
/// [`open_session`]: exactly one line ends the read, and the peer's timeout
/// and [`PENDING_MAX_BYTES`] bound it from both sides.
#[derive(Debug, Eq, PartialEq)]
enum AnswerLine {
    /// The one line, WITHOUT its `\\n`; anything the peer sent after the line
    //  end is NOT read here (the handshake reads ONE line and nothing else,
    //  and a peer that answered twice before being asked a second question
    //  is not this build's peer to begin with).
    Got(String),
    /// The read reached the timeout before a line end arrived.
    Expired,
    /// The stream broke (an OS failure, or the peer closed its side mid-line);
    //  the text is the complaint.
    Broken(String),
    /// The peer streamed more than [`PENDING_MAX_BYTES`] bytes without a line
    //  end: the size seen, the same bound [`StreamText`] applies to a session.
    TooLong(usize)
}

fn read_answer_line(mut reader: &TcpStream, timeout_seconds: i64) -> AnswerLine {
    set_read_deadline(&reader, timeout_seconds);
    let mut pending = String::new();
    loop {
        let mut buffer = vec![0u8];
        match reader.read(&mut buffer) {
            Err(reason) => {
                let kind = reason.kind();
                if kind == std::io::ErrorKind::WouldBlock || kind == std::io::ErrorKind::TimedOut {
                    return AnswerLine::Expired;
                }
                let message = format!("{reason}");
                return AnswerLine::Broken(message);
            }
            Ok(read) => {
                if read == 0 {
                    return AnswerLine::Broken("the peer closed its side before an answer line ended".to_string());
                }
                match String::from_utf8(buffer) {
                    Err(_) => { return AnswerLine::Broken("the peer sent bytes that are not UTF-8".to_string()); }
                    Ok(decoded) => {
                        pending += &decoded;
                        if pending.len() > PENDING_MAX_BYTES {
                            return AnswerLine::TooLong(pending.len());
                        }
                        match pending.find("\n") {
                            Some(index) => {
                                let line = &pending[..index];
                                return AnswerLine::Got(line.to_string());
                            }
                            None => { }
                        }
                    }
                }
            }
        }
    }
}

/// The `Duration` of a timeout given in SECONDS (every timeout of this module
/// is an [`i64`] of seconds, the type the flags and the config carry): the
/// NON-CASTING `try_into` conversion, and a NEGATIVE value — which no caller
/// of this module may pass, the flags validate their numbers — clamps to an
/// INSTANT timeout rather than silently switching the timeout OFF.
fn timeout_value(seconds: i64) -> Option::<Duration> {
    match seconds.try_into() {
        Err(_) => { return Some(Duration::from_secs(0)); }
        Ok(unsigned) => { return Some(Duration::from_secs(unsigned)); }
    }
}

/// The `Duration` of a pause given in MILLISECONDS (the poll intervals of
/// this module): same NON-CASTING route as [`timeout_value`].
fn millis_pause(value: i64) -> Duration {
    return match value.try_into() {
        Err(_) => { Duration::from_millis(0) }
        Ok(millis) => { Duration::from_millis(millis) }
    };
}

/// Set the read timeout of a socket, keeping the toolchain's own REPORT of
/// whether the socket accepted it: a REFUSED deadline would mean the read
/// blocks forever, which no caller of this module wants, but nothing this
/// build can DO about a refusal except let the read behave as the socket
/// chooses — the failure reason is not a protocol event, and the doc fixes
/// no error for it (`core-protocol.md:16` only fixes THAT a timeout is set).
fn set_read_deadline(socket: &TcpStream, seconds: i64) {
    match socket.set_read_timeout(timeout_value(seconds)) {
        Err(_) => { }
        Ok(_) => { }
    }
}

/// The length of the handshake read timeout this transport sets by default
/// ([`open_session`] takes the value; production callers pass this constant):
/// `core-protocol.md:16` fixes THAT a read timeout is set on the socket,
/// never ITS length, so 5 seconds is a RECORDED INVENTION, small enough for
/// a dead interface to fail fast and far too small to confuse the idle timeout
/// (15 MINUTES) of the same paragraph.
pub const HANDSHAKE_TIMEOUT_SECONDS: i64 = 5;

/// Open the socket of an endpoint: the port of an [`Endpoint`] is an [`i64`]
/// (TOML reads integers as i64), while the toolchain binds ports as `u16`, so
/// the NON-CASTING `try_into` conversion decides whether the configured port
/// is bindable AT ALL — a port out of `0..65535` never reaches a peer, which
/// `parse_endpoint` and `endpoint_from_config` refuse upstream as well.
fn open_socket(endpoint: &Endpoint) -> Result::<TcpStream, std::io::Error> {
    match bind_port(endpoint) {
        None => { return Err::<TcpStream, std::io::Error>(std::io::Error::new(std::io::ErrorKind::InvalidInput, "the configured port cannot be bound by this toolchain")); }
        Some(short) => { return TcpStream::connect((endpoint.host.to_string(), short)); }
    }
}

/// The port of an endpoint as the value the toolchain binds and connects
/// with: see [`open_socket`] and [`bind_listener`].
fn bind_port(endpoint: &Endpoint) -> Option::<u16> {
    return match endpoint.port.try_into() {
        Err(_) => { None }
        Ok(short) => { Some(short) }
    };
}

// =============================================================================
// The daemon side: start-up, serving, idle exit, lock removal
// =============================================================================

/// The text of plan 2.5's [`core::technical::fs_atomic.WriteError`]: it is a
/// typed error WITHOUT a `Display` (its data is the path, the
/// [`std::io::ErrorKind`] and the OS text of the failed step), so the transport
/// renders it itself rather than let the lock's failure reason be lost.
fn write_failure_text(reason: &fs_atomic::WriteError) -> String {
    match reason {
        fs_atomic::WriteError::DirectoryRejected(path, kind, text) => { return format!("the directory of [{at}] was refused ({why}): {text}", at = path.display(), why = kind.to_string(), text = text); }
        fs_atomic::WriteError::PreviousUnreadable(path, kind, text) => { return format!("the previous lock at [{at}] could not be read ({why}): {text}", at = path.display(), why = kind.to_string(), text = text); }
        fs_atomic::WriteError::TempWriteFailed(path, kind, text) => { return format!("the temporary lock file next to [{at}] could not be written ({why}): {text}", at = path.display(), why = kind.to_string(), text = text); }
        fs_atomic::WriteError::BackupWriteFailed(path, kind, text) => { return format!("the backup of the previous lock at [{at}] could not be written ({why}): {text}", at = path.display(), why = kind.to_string(), text = text); }
        fs_atomic::WriteError::RenameFailed(path, kind, text) => { return format!("the lock file could not be renamed into place at [{at}] ({why}): {text}", at = path.display(), why = kind.to_string(), text = text); }
    }
}

/// The daemon's own half of this transport: the listener it bound on the
/// configured address, the address as the lock file writes it, and the nonce
/// it minted (`architecture.md:249`: the DAEMON writes the lock, the interface
/// READS it — decision 33, so no interface ever knows a nonce before the
/// daemon that wrote it exists). The session id every connection of THIS
/// daemon answers the handshake with is `proto::handshake::session_id_of` of
/// that nonce (decision 28: this is the per-instance id plan 3.5's
/// `DEFAULT_SESSION` placeholder waits for).
#[derive(Debug)]
pub struct DaemonHandle {
    /// The listener bound on the configured address.
    pub listener: TcpListener,
    /// The `host:port` text, the `address=` value of the lock file.
    pub address: String,
    /// The minted nonce, the `nonce=` value of the lock file.
    pub nonce: String,
    /// `session_id_of(nonce)`, handed to `interpret::run_session` per connection.
    pub session: String
}

/// What daemon start-up came down to.
#[derive(Debug)]
pub enum DaemonStart {
    /// The listener is bound and the lock file is written: the handle carries
    /// both (the lock file is the daemon's from this moment on,
    /// `data-files.md:143`).
    Ready(DaemonHandle),
    /// The configured address could not be bound — a port held by an unrelated
    /// process, `architecture.md:250`. The texts are the CONFIGURED ADDRESS
    /// and the OS's own message, and NO lock file was written, so no
    /// interface reads a daemon that is not there.
    AddressTaken(String, String),
    /// The address is bound but the lock file could not be written; the
    /// daemon cannot let interfaces discover it, so it must not serve (the
    /// texts are the address and [`write_failure_text`] of `fs_atomic`'s
    /// typed complaint).
    LockUnwritten(String, String)
}

/// The bind of the daemon's listener on the configured address
/// (`core-protocol.md:12`: TCP on `127.0.0.1`).
fn bind_listener(endpoint: &Endpoint) -> Result::<TcpListener, std::io::Error> {
    match bind_port(endpoint) {
        None => { return Err::<TcpListener, std::io::Error>(std::io::Error::new(std::io::ErrorKind::InvalidInput, "the configured port cannot be bound by this toolchain")); }
        Some(short) => { return TcpListener::bind((endpoint.host.to_string(), short)); }
    }
}

/// The seed of the daemon's nonce: its process id (`std::process::id`, the
/// toolchain's pid call), the endpoint it was told to bind, and the path of
/// the lock file it writes — see [`mint_nonce`]'s doc for why the DAEMON
/// mints from its own identity (decision 33).
pub fn daemon_nonce_seed(address: String, data_dir: &std::path::Path) -> String {
    let seed = format!("{pid}:{at}:{lock}", pid = std::process::id().to_string(), at = address, lock = lock_path(data_dir).display());
    return seed;
}

/// Start the daemon: bind, then write `state/core.lock`
/// (`core-protocol.md:18-28`, `data-files.md:143`), and only then serve.
///
/// The order is fixed by the docs: the lock file appears WITH the daemon,
/// never before it, so a discovery that reads a lock always finds a listener
/// behind it — until the daemon EXITS, which is exactly the stale-lock case
/// of clause (c) ([`remove_lock`] is the other half, see [`serve_daemon`]).
pub fn daemon_start(endpoint: &Endpoint, data_dir: &std::path::Path) -> DaemonStart {
    let address = endpoint_text(endpoint.host.to_string(), endpoint.port);
    let bind_attempt = bind_listener(endpoint);
    match bind_attempt {
        Err(reason) => {
            let message = format!("{reason}");
            return DaemonStart::AddressTaken(address, message);
        }
        Ok(listener) => {
            let nonce = mint_nonce(&daemon_nonce_seed(address.to_string(), data_dir));
            let session = session_id_of(&nonce.to_string());
            let record = LockRecord { address: address.to_string(), nonce: nonce.to_string() };
            match write_lock(data_dir, &record) {
                Err(reason) => {
                    return DaemonStart::LockUnwritten(address, write_failure_text(&reason));
                }
                Ok(_) => {
                    // `idle_seconds` is THIS daemon's own idle timeout: it
                    // arrives through the daemon's argv from
                    // [`spawn_arguments`] (decision 34 — the documented config
                    // keys of `data-files.md:146-152` hold NO timeout key,
                    // and inventing one is forbidden), and it becomes the
                    // read timeout of every connection and the accept-loop's
                    // clock in [`serve_daemon`].
                    return DaemonStart::Ready(DaemonHandle { listener: listener, address: address, nonce: nonce.to_string(), session: session });
                }
            }
        }
    }
}

/// What one served connection left the daemon to do. The TWO outcomes that
/// end the daemon are exactly the doc's two (`core-protocol.md:192-201`: the
/// idle timeout, and the `shutdown` command whose answer IS the closed
/// stream); everything else is "this interface went away, another may come".
#[derive(Debug, Eq, PartialEq)]
pub enum SessionEnd {
    /// The peer closed its side (end of stream): the session is over and the
    /// daemon stays for the next connection — which is what the live test
    /// of clause (e) turns on.
    PeerClosed,
    /// The connection's read reached the idle timeout: `core-protocol.md:16`'s
    /// "15 minutes WITHOUT commands" seen from the socket — the daemon EXITS.
    IdleTimeout,
    /// The connection sent `shutdown`: the daemon EXITS and removes the lock
    /// (`core-protocol.md:199`).
    ShutDown,
    /// The connection's stream broke mid-session ([`interpret.Stop.StreamBroken`]
    /// of the connection); the doc's "Unknown input ... the daemon stays
    /// alive" answers an unreadable line by ending THIS session only.
    Unreadable(String),
    /// The peer closed with a half line pending: clean as [`PeerClosed`] —
    /// "the stream never resynchronizes on a partial line" and at the end of
    /// the input NOTHING can complete it; the text is that tail.
    HalfLine(String),
    /// The event sink latched: an event line could not be written (plan 3.3's
    /// obligation on the sink, see [`LatchSink`]); the session is over like
    /// a peer that vanished.
    SinkBroken
}

/// Serve ONE accepted connection: hand the socket pair to plan 3.4's
/// interpreter loop — [`interpret.run_session`] with the daemon's
/// lock-DERIVED session id — exactly as [`open_session`] hands a pair to
/// the same loop on the interface side. NO separate handshake is performed
/// here: the interface's FIRST line IS the `version` request of
/// [`open_session`], which THIS build's interpreter answers with the `hello`
/// line the interface's checker then verifies (decision 29/30 split: the
/// transport guarantees the ORDER, the interpreter answers the VERB).
pub fn serve_one_connection(stream: TcpStream, idle_seconds: i64, session: &str, work: &mut dyn interpret::Work) -> SessionEnd {
    // The listener of [`serve_daemon`] is NON-BLOCKING, and on this platform
    // the accepted socket INHERITS that, which would turn every read into an
    // instant `WouldBlock`: the accepted stream is put back to BLOCKING and
    // the read timeout of `core-protocol.md:16` becomes the idle timeout.
    match stream.set_nonblocking(false) {
        Err(_) => { }
        Ok(_) => { }
    }
    set_read_deadline(&stream, idle_seconds);
    match stream.try_clone() {
        Err(_) => { return SessionEnd::Unreadable("the socket pair of the connection could not be made".to_string()); }
        Ok(writer) => {
            let mut text = StreamText {
                reader: stream,
                pending: String::new(),
                ended: false,
                unreadable: String::new(),
                timed_out: false,
                flushed: false
            };
            let mut sink = LatchSink { writer: writer, broken: false };
            let stop = interpret::run_session(&mut text, &mut sink, work, session.to_string());
            // The idle condition is read from the TEXT side, not from `stop`:
            // a timed-out read hands the interpreter [`interpret.TextRead.Done`]
            // (see [`StreamText`]'s `read_text`), which ends the session as
            // `EndOfInput` — the FLAG says WHICH end of input it was.
            if text.timed_out {
                return SessionEnd::IdleTimeout;
            }
            match stop {
                interpret::Stop::ShutdownRequested => { return SessionEnd::ShutDown; }
                interpret::Stop::StreamBroken(why) => { return SessionEnd::Unreadable(why); }
                interpret::Stop::HalfLineAtEnd(tail) => { return SessionEnd::HalfLine(tail); }
                interpret::Stop::EndOfInput => {
                    if sink.broken {
                        return SessionEnd::SinkBroken;
                    }
                    return SessionEnd::PeerClosed;
                }
            }
        }
    }
}

/// What the daemon's whole life came down to; EVERY variant carries the
/// [`Removal`] of the lock file (`core-protocol.md:199`,
/// `data-files.md:143`: the lock file is removed on exit, on EVERY exit
/// path OF THE DAEMON — reuse on the interface side never removes it, see
/// [`connect_or_spawn`] and [`remove_lock`]'s docs).
#[derive(Debug, Eq, PartialEq)]
pub enum DaemonExit {
    /// The idle timeout passed with no connection in hand and no command
    //  seen: exit, as `core-protocol.md:16` orders.
    IdleExit(Removal),
    /// A connection sent `shutdown`: exit.
    ShutdownExit(Removal),
    /// The listener itself broke; the texts are the complaint and the removal.
    ListenerBroken(String, Removal)
}

/// The accept-loop's pause as a wait: 250 ms BETWEEN looks at the
/// NON-BLOCKING listener — small next to an idle timeout of MINUTES, so the
/// timeout's own LENGTH stays honoured while the wait needs no thread, no
/// channel and no second socket (this toolchain has no poll primitive and
/// no stdlib channel, the toolchain facts of this row's cycle-1 log).
const ACCEPT_POLL_MILLIS: i64 = 250;

/// Serve the daemon: accept connections ONE AT A TIME and serve each
/// through [`serve_one_connection`], exiting — and REMOVING the lock file —
/// on the idle timeout or on `shutdown`.
///
/// ONE CONNECTION AT A TIME is this build's reading of the sharing rule
/// (`core-protocol.md:14`: "the core serializes commands, so commands from
/// different interfaces interleave WHOLE-COMMAND-BY-WHOLE-COMMAND, never line
/// by line"): the daemon finishes one session's event blocks — EVERY line
/// of one command's block — before it reads the next connection, so no line
/// of one interface can EVER land inside a command's block on another's
/// socket. True CONCURRENT serving of several OPEN sessions is DEFERRED
/// (see the module doc); the tests pin the interleaving THIS build DELIVERS.
pub fn serve_daemon(listener: &TcpListener, data_dir: &std::path::Path, idle_seconds: i64, work: &mut dyn interpret::Work, session: String) -> DaemonExit {
    match listener.set_nonblocking(true) {
        Err(_) => { }
        Ok(_) => { }
    }
    // The idle clock counts MILLISECONDS WAITED with no connection in hand;
    // it RESETS on every accepted connection, which is what "WITHOUT
    // COMMANDS" means at the accept boundary. Within a session the SAME
    // timeout is the socket read deadline ([`serve_one_connection`]).
    let idle_millis = idle_seconds * 1000;
    let mut waited: i64 = 0;
    loop {
        match listener.accept() {
            Ok(pair) => {
                let stream = pair.0;
                match serve_one_connection(stream, idle_seconds, &session, work) {
                    SessionEnd::IdleTimeout => {
                        return DaemonExit::IdleExit(remove_lock(data_dir));
                    }
                    SessionEnd::ShutDown => {
                        return DaemonExit::ShutdownExit(remove_lock(data_dir));
                    }
                    SessionEnd::PeerClosed => { }
                    SessionEnd::HalfLine(_) => { }
                    SessionEnd::SinkBroken => { }
                    SessionEnd::Unreadable(_) => { }
                }
                waited = 0;
            }
            Err(reason) => {
                let kind = reason.kind();
                if kind != std::io::ErrorKind::WouldBlock && kind != std::io::ErrorKind::TimedOut {
                    return DaemonExit::ListenerBroken(format!("{reason}"), remove_lock(data_dir));
                }
                std::thread::sleep(millis_pause(ACCEPT_POLL_MILLIS));
                waited += ACCEPT_POLL_MILLIS;
                if waited >= idle_millis {
                    return DaemonExit::IdleExit(remove_lock(data_dir));
                }
            }
        }
    }
}

/// The argv of the daemon mode of `core/src/main.rs`, as DATA: the
/// [`Spawner`] seam takes the executable path and this argument list, and
/// `main.rs` parses the SAME flags back with [`daemon_settings`] — one flag
/// table for the child and its own process, no hidden channel.
pub struct DaemonSettings {
    /// The host to bind (the `--address=` flag, else the configuration).
    pub host: String,
    /// The port to bind (the `--port=` flag, else the configuration).
    pub port: i64,
    /// The data directory (the `--data-dir=` flag, else the CURRENT
    /// DIRECTORY — `data-files.md:152`: "data directory (default: current
    /// directory)").
    pub data_dir: std::path::PathBuf,
    /// The idle timeout in seconds (the `--idle-timeout=` flag, else
    /// [`DEFAULT_IDLE_SECONDS`], decision 34).
    pub idle_seconds: i64
}

/// Why the daemon flags could not be read.
#[derive(Debug, Eq, PartialEq, Clone)]
pub enum DaemonFlagIssue {
    /// A `--port=`/`--idle-timeout=` value that is not an integer; the texts
    /// are the flag and the value seen.
    NotANumber(String, String),
    /// A `--port=` outside `0..65535`.
    PortOutOfRange(i64),
    /// A `--idle-timeout=` that is ZERO or NEGATIVE: an instant idle is a
    /// daemon that exits before any interface can connect, which no documented
    /// rule asks for (RECORDED EDGE).
    IdleTooSmall(i64),
    /// The `--address=`/`--port=` flags left the address UNSET and the
    /// CONFIGURATION of `core-protocol.md:14` could not be read either; the
    /// list is [`endpoint_from_config`]'s typed complaints. (The daemon this
    /// flow SPAWNS always gets BOTH flags EXPLICITLY from [`spawn_arguments`],
    /// so this refusal only bites a hand-started daemon.)
    ConfigRejected(Vec::<ConfigIssue>)
}

/// Read the daemon flags of `argv` (see [`spawn_arguments`]: the SAME
/// constants build and read the flags). A flag GIVEN TWICE: the LAST value
/// wins, the same recorded edge as a repeated lock-file key.
pub fn daemon_settings(args: Vec::<String>) -> (Option::<DaemonSettings>, Vec::<DaemonFlagIssue>) {
    let mut host = String::new();
    let mut port = DEFAULT_PORT;
    let mut port_given = false;
    let mut idle_seconds = DEFAULT_IDLE_SECONDS;
    let mut data_dir = std::path::PathBuf::new();
    match std::env::current_dir() {
        Err(_) => { }
        Ok(held) => { data_dir = held; }
    }
    let mut issues = Vec::<DaemonFlagIssue>::new();
    for held in args {
        if held.starts_with(ADDRESS_FLAG) {
            match held.find("=") {
                Some(index) => { host = held[index + 1 ..].to_string(); }
                None => { }
            }
        }
        if held.starts_with(PORT_FLAG) {
            match held.find("=") {
                Some(index) => {
                    match held[index + 1 ..].to_string().parse::<i64>() {
                        Err(_) => { issues.push(DaemonFlagIssue::NotANumber(PORT_FLAG.to_string(), held[index + 1 ..].to_string())); }
                        Ok(value) => {
                            if value < 0 || value > 65_535 {
                                issues.push(DaemonFlagIssue::PortOutOfRange(value));
                            } else {
                                port = value;
                                port_given = true;
                            }
                        }
                    }
                }
                None => { }
            }
        }
        if held.starts_with(DATA_DIR_FLAG) {
            match held.find("=") {
                Some(index) => { data_dir = std::path::PathBuf::from(held[index + 1 ..].to_string()); }
                None => { }
            }
        }
        if held.starts_with(IDLE_FLAG) {
            match held.find("=") {
                Some(index) => {
                    match held[index + 1 ..].to_string().parse::<i64>() {
                        Err(_) => { issues.push(DaemonFlagIssue::NotANumber(IDLE_FLAG.to_string(), held[index + 1 ..].to_string())); }
                        Ok(value) => {
                            if value <= 0 {
                                issues.push(DaemonFlagIssue::IdleTooSmall(value));
                            } else {
                                idle_seconds = value;
                            }
                        }
                    }
                }
                None => { }
            }
        }
    }
    if issues.len() > 0 {
        return (None, issues);
    }
    // The ADDRESS precedence of the module doc: an explicit `--address=`/
    // `--port=` flag is EXPLICIT; what a flag leaves unset falls back to
    // the CONFIGURATION of `core-protocol.md:14` ("bind from
    // config/flags"), which falls back to the documented DEFAULT itself.
    let host_given = host.len() > 0;
    if !(host_given && port_given) {
        match endpoint_from_config(&data_dir) {
            (None, issues) => {
                return (None, [DaemonFlagIssue::ConfigRejected(issues)].to_vec());
            }
            (Some(config_endpoint), _) => {
                if !host_given { host = config_endpoint.host; }
                if !port_given { port = config_endpoint.port; }
            }
        }
    }
    return (Some(DaemonSettings { host: host, port: port, data_dir: data_dir, idle_seconds: idle_seconds }), Vec::<DaemonFlagIssue>::new());
}

// ============================================================================
// Tests
// ============================================================================

/// A CWD-relative scratch directory under `test-scratch-transport/<name>`, REMOVED
/// FIRST if left behind by a FAILED run, CREATED for this run WITH A BOUNDED
/// RETRY (the race documented by the review of plan item 3.6b: a SIBLING
/// test's cleanup removes the shared scratch PARENT while this test is INSIDE
/// `create_dir_all` — the creation is ATTEMPTED AT MOST FIVE TIMES so a
/// transient ENOENT under the concurrent best-effort parent removal cannot
/// fail a test), and REMOVED again at the end by [`cleanup_scratch`] — which
/// also drops the scratch parent of THIS MODULE ONLY (`test-scratch-transport`,
/// not the scratch parents of `fs_atomic`/`view`: the NEW modules of this
/// row own their OWN scratch parents, once the hygiene gate is
/// EMPTY after a test run `find . -name 'test-scratch*'` reads EMPTY). Every
/// test of this module uses a name of its OWN — `cargo test` runs the tests of
/// one binary on PARALLEL threads, so DISTINCT directories (and DISTINCT FREE
/// PORTS, see [`free_port`]) make the tests unable to collide.
#[cfg(test)]
fn scratch_dir(name: String) -> std::path::PathBuf {
    let dir = std::path::PathBuf::from("test-scratch-transport").join(name).to_path_buf();
    match std::fs::exists(&dir) {
        Ok(true) => {
            match std::fs::remove_dir_all(&dir) {
                Err(_) => { }
                Ok(_) => { }
            }
        }
        Ok(false) => { }
        Err(_) => { }
    }
    // BOUNDED RETRY of the creation: the race documented by the review of
    // plan item 3.6b — a SIBLING test's cleanup removes the shared scratch
    // PARENT while this test is INSIDE `create_dir_all`, so the parent vanishes
    // mid-call and the CHILD creation dies with ENOENT. `create_dir_all`
    // RECREATES the parent, so one more ATTEMPT SUCCEEDS; FIVE attempts
    // BOUND the loop so a genuine failure cannot spin.
    let mut tries: i64 = 0;
    while tries < 5 {
        match std::fs::create_dir_all(&dir) {
            Ok(_) => { break; }
            Err(_) => { }
        }
        tries += 1;
    }
    return dir;
}

#[cfg(test)]
fn cleanup_scratch(dir: &std::path::PathBuf) {
    match std::fs::remove_dir_all(&dir) {
        Err(_) => { }
        Ok(_) => { }
    }
    // The EMPTY parent goes too, best effort: a sibling test still RUNNING
    // keeps the directory non-empty and this removal is simply REFUSED.
    match std::fs::remove_dir(std::path::PathBuf::from("test-scratch-transport")) {
        Err(_) => { }
        Ok(_) => { }
    }
}

/// A port NO listener holds. This toolchain has NO accessor for the port an
/// `:0` bind chose (`accept()` carries a `SocketAddr.port()`, but only AFTER
/// a client connected — which is AFTER the port had to be NAMED in the
/// configuration), and the probe list `.port()`/`get_port()`/`local_port()`/
/// `get_addr()`/`getsockaddr()` on a listener all E0599/E0609. So a test
/// passes its OWN FREE BASE and gets the FIRST port from it a BIND SUCCEEDS
/// ON — a SUCCESSFUL bind is the PROOF of freeness — the probe listener
/// dying at the end of the scope releasing it. DEVIATION from an
/// "OS-chosen port" RECORDED for the reviewer; every test's base is DISTINCT
/// and NO range touches the documented default 47623 (planning decision 7).
#[cfg(test)]
fn free_port(base: i64) -> i64 {
    let mut candidate = base;
    while candidate < base + 50 {
        let short: u16 = candidate.try_into().unwrap_or(0);
        match TcpListener::bind(("127.0.0.1", short)) {
            Err(_) => { }
            Ok(_) => { return candidate; }
        }
        candidate += 1;
    }
    return base;
}

/// The `phin.toml` of a scratch data directory, carrying the DOTTED spelling
/// `data-files.md:149` and a FREE port: EVERY test of this module WRITES a
/// configuration, so "the endpoint comes from the configuration" is
/// EXERCISED rather than defaulted (planning decision 7 again).
#[cfg(test)]
fn write_config(dir: &std::path::PathBuf, port: i64) {
    let text = format!("core.address = \"127.0.0.1:{at}\"\n", at = port.to_string());
    match std::fs::write(&dir.join(CONFIG_FILE_NAME.to_string()), text) {
        Err(_) => { }
        Ok(_) => { }
    }
}

#[cfg(test)]
fn contains_string(held: &Vec::<String>, needle: String) -> bool {
    for item in held {
        if item.to_string() == needle {
            return true;
        }
    }
    return false;
}

/// The ONE line a socket's peer streamed, as text (used by the fake listeners
/// below to SEE what the interface wrote BEFORE anything else).
#[cfg(test)]
fn read_all_text(mut socket: &TcpStream) -> String {
    set_read_deadline(socket, 5);
    let mut collected = String::new();
    loop {
        let mut buffer = vec![0u8];
        match socket.read(&mut buffer) {
            Err(_) => { return collected; }
            Ok(read) => {
                if read == 0 {
                    return collected;
                }
                match String::from_utf8(buffer) {
                    Err(_) => { }
                    Ok(decoded) => { collected += &decoded; }
                }
            }
        }
    }
}

/// What a FAKE listener thread (the clause (b)/(c) fake daemon answering
/// `hello`, and the clause (d) FOREIGN listener answering NONCE garbage)
/// needs: bundled in a STRUCT because `std::thread::spawn` takes a ZERO-
/// ARGUMENT closure, and this toolchain's closures capture by COPY only
/// a struct argument (the `port: i64` and `answer_session: String` free-
/// variable forms E0373-borrow — probe fact).
#[cfg(test)]
struct ListenerSpec {
    pub host: String,
    pub port: i64,
    /// The `session` value the `hello` answer carries (HELLO's session is
    /// informational, so the fake answers with the session the REAL
    /// daemon of that nonce would carry).
    pub session: String,
    /// Where the FOREIGN listener writes EVERY byte it received, so the
    /// test can prove the interface sent NOTHING but the handshake line.
    pub log_path: String,
    /// `hello` answering fake vs FOREIGN garbage answering listener.
    pub answers_hello: bool,

    /// When NON-EMPTY the fake answers with THIS nonce instead of the one
    /// it was ASKED with: the peer that speaks a perfect `hello` for ANOTHER
    /// daemon (route 1 of the review findings — the NONCE ECHO of
    /// `handshake.rs:424`). EMPTY keeps the echo of the nonce received, which
    /// is what every fake of this module already does.
    pub reply_nonce: String,

    /// When POSITIVE the fake's hello CLAIMS THIS protocol version instead of
    /// [`PROTOCOL_VERSION`]: the peer that speaks a version this build does
    /// NOT (route 2 of the review findings). ZERO or NEGATIVE keeps this
    /// build's version.
    pub hello_version: i64
}

#[cfg(test)]
fn listener_work(spec: ListenerSpec) {
    let endpoint = Endpoint { host: spec.host.to_string(), port: spec.port };
    match bind_port(&endpoint) {
        None => { return; }
        Some(short) => {
            match TcpListener::bind((spec.host.to_string(), short)) {
                Err(_) => { return; }
                Ok(listener) => {
                    loop {
                    match listener.accept() {
                        Err(_) => { return; }
                        Ok(held) => {
                            let mut socket = held.0;
                            let mut received = String::new();
                            match read_answer_line(&socket, 5) {
                                AnswerLine::Got(line) => {
                                    received += &line.to_string();
                                    received += "\n";
                                    if spec.answers_hello {
                                        let mut seen = String::new();
                                        match serde_json::from_str::<Value>(&line) {
                                            Err(_) => { }
                                            Ok(held) => {
                                                match held.as_object() {
                                                    None => { }
                                                    Some(fields) => {
                                                        match fields.get("nonce") {
                                                            None => { }
                                                            Some(nonce_value) => {
                                                                match nonce_value.as_str() {
                                                                    None => { }
                                                                    Some(held_nonce) => { seen = held_nonce.to_string(); }
                                                                }
                                                            }
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                        let mut asked_back = seen;
                                        if spec.reply_nonce.len() > 0 {
                                            asked_back = spec.reply_nonce.to_string();
                                        }
                                        let mut claimed_version = PROTOCOL_VERSION;
                                        if spec.hello_version > 0 {
                                            claimed_version = spec.hello_version;
                                        }
                                        match proto::codec::encode_frame(&proto::codec::Message::Outbound(proto::event::Event::Hello(asked_back.to_string(), spec.session.to_string(), claimed_version))) {
                                            Err(_) => { }
                                            Ok(frame) => {
                                                match socket.write_all(frame.as_bytes()) {
                                                    Err(_) => { }
                                                    Ok(_) => { }
                                                }
                                            }
                                        }
                                    } else {
                                        // FOREIGN: an `event` line this build
                                        // can DECODE (a `result` with its
                                        // `cmd`) but which is NOT a
                                        // `hello` — the checker's `NotHello`
                                        // reading, clause (d)'s "not our
                                        // daemon". A line this build cannot
                                        // decode at all would land on the
                                        // transport's `Undecodable` ATTEMPT
                                        // instead; BOTH readings REFUSE, and
                                        // the one here pins the NotHello
                                        // path of `handshake.rs:355`.
                                        match socket.write_all("{\"v\":1,\"event\":\"result\",\"cmd\":\"list-pending\"}\n".to_string().as_bytes()) {
                                            Err(_) => { }
                                            Ok(_) => { }
                                        }
                                    }
                                }
                                _ => { return; }
                            }
                            if !spec.answers_hello {
                                // Drain the REST of the interface's stream
                                // (NOTHING is there when the transport
                                // behaved, and the CLOSED stream ends this
                                // loop), then write EVERY byte received to
                                // the log — the clause (d) PROOF.
                                loop {
                                    let mut buffer = vec![0u8];
                                    match socket.read(&mut buffer) {
                                        Err(_) => { break; }
                                        Ok(read) => {
                                            if read == 0 {
                                                break;
                                            }
                                            match String::from_utf8(buffer) {
                                                Err(_) => { }
                                                Ok(decoded) => { received += &decoded; }
                                            }
                                        }
                                    }
                                }
                                match std::fs::write(&std::path::PathBuf::from(spec.log_path.to_string()), received) {
                                    Err(_) => { }
                                    Ok(_) => { }
                                }
                            }
                        }
                    }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
fn start_fake_daemon(host: String, port: i64, session: String) {
    let spec = ListenerSpec { host: host.to_string(), port: port, session: session.to_string(), log_path: String::new(), answers_hello: true, reply_nonce: String::new(), hello_version: 0 };
    std::thread::spawn(|| -> () { listener_work(spec) });
}

#[cfg(test)]
fn start_foreign_listener(host: String, port: i64, log_path: String) {
    let spec = ListenerSpec { host: host.to_string(), port: port, session: String::new(), log_path: log_path.to_string(), answers_hello: false, reply_nonce: String::new(), hello_version: 0 };
    std::thread::spawn(|| -> () { listener_work(spec) });
}

/// The fake interface of the DAEMON-side tests: it shows up at the listener
/// ONLY AFTER `wait_before_millis` (the accept clock has counted THAT much
/// when it arrives, which is what route 3 pins). It asks the version request
/// ONLY when told to, it may send the documented `shutdown` command line, it
/// either CLOSES its write end (the session ENDS at end-of-input) or keeps it
/// OPEN SILENTLY for `hold_millis` (the connection's READ deadline is what
/// ends it), and with `closes_read` it shuts down its OWN read end so the
/// DAEMON's writes to it DIE — the shape [`LatchSink`] breaks on. Bundled in
/// a STRUCT because `std::thread::spawn` takes a ZERO-ARGUMENT closure (the
/// toolchain fact of this row, see [`ListenerSpec`]).
#[cfg(test)]
struct QuietPeerSpec {
    pub host: String,
    pub port: i64,
    /// The nonce the version request announces, when one is sent at all.
    pub nonce: String,
    /// How long the peer waits BEFORE connecting at all, milliseconds.
    pub wait_before_millis: i64,
    /// Whether the peer asks for the version of the daemon.
    pub sends_version_request: bool,
    /// Whether the peer also sends `shutdown`.
    pub sends_shutdown: bool,
    /// Whether the peer half-closes its write end after what it sends.
    pub closes_write: bool,
    /// Whether the peer shuts down its OWN read end, so the daemon's writes
    /// to it fail: the broken pipe [`LatchSink`] latches on.
    pub closes_read: bool,
    /// How long the peer then HOLDS its socket OPEN, milliseconds.
    pub hold_millis: i64
}

#[cfg(test)]
fn quiet_peer_work(spec: QuietPeerSpec) {
    std::thread::sleep(millis_pause(spec.wait_before_millis));
    let short: u16 = spec.port.try_into().unwrap_or(0);
    match TcpStream::connect((spec.host.to_string(), short)) {
        Err(_) => { return; }
        Ok(mut socket) => {
            if spec.closes_read {
                // Shut down FIRST: the probe fact of the sink test below is
                // that the FIRST write of the daemon is still ACCEPTED into
                // the socket buffer, its FLUSH reports OK, and the failure
                // shows up on a LATER write (`Broken pipe`).
                match socket.shutdown(std::net::Shutdown::Read) {
                    Err(_) => { }
                    Ok(_) => { }
                }
                let mut buffer = vec![0u8];
                loop {
                    match socket.read(&mut buffer) {
                        Err(_) => { break; }
                        Ok(held) => { if held == 0 { break; } }
                    }
                }
            }
            if spec.sends_version_request {
                let asked_nonce = spec.nonce.to_string();
                match socket.write_all(version_request_frame(&asked_nonce).as_bytes()) {
                    Err(_) => { }
                    Ok(_) => { }
                }
            }
            if spec.sends_shutdown {
                match proto::codec::encode_frame(&proto::codec::Message::Inbound(proto::command::Command::Shutdown)) {
                    Err(_) => { }
                    Ok(frame) => {
                        match socket.write_all(frame.as_bytes()) {
                            Err(_) => { }
                            Ok(_) => { }
                        }
                    }
                }
            }
            if spec.closes_write {
                match socket.shutdown(std::net::Shutdown::Write) {
                    Err(_) => { }
                    Ok(_) => { }
                }
            }
            // The socket STAYS OPEN for the hold: dropping it here would
            // end the DAEMON's session at end-of-input, not at its READ
            // deadline.
            std::thread::sleep(millis_pause(spec.hold_millis));
        }
    }
}

#[cfg(test)]
fn start_quiet_peer(spec: QuietPeerSpec) {
    std::thread::spawn(|| -> () { quiet_peer_work(spec) });
}

/// The recorder-spawner of the SPAWN-path tests: it records the EXECUTABLE
/// path and the argument list it was HANDED (the DATA of the [`Spawner`]
/// seam) and starts NOTHING — or, for clause (c), it plays the freshly
/// STARTED daemon: it writes the new lock file and starts the
/// [`fake_daemon_work`] thread.
#[cfg(test)]
struct FakeSpawner {
    pub starts: Vec::<Vec::<String>>,
    pub succeed_start: bool,
    pub plays_daemon: bool,
    pub host: String,
    pub port: i64,
    pub data_dir_text: String,
    pub new_nonce: String,
    pub new_session: String
}

#[cfg(test)]
impl Spawner for FakeSpawner {
    fn start(&mut self, executable: String, args: Vec::<String>) -> bool {
        let mut recorded = [executable.to_string()].to_vec();
        for held in args {
            recorded.push(held.to_string());
        }
        self.starts.push(recorded);
        if self.plays_daemon {
            let record = LockRecord { address: endpoint_text(self.host.to_string(), self.port), nonce: self.new_nonce.to_string() };
            let new_dir = std::path::PathBuf::from(self.data_dir_text.to_string());
            match write_lock(&new_dir, &record) {
                Err(_) => { }
                Ok(_) => { }
            }
            start_fake_daemon(self.host.to_string(), self.port, self.new_session.to_string());
        }
        return self.succeed_start;
    }
}

// ----------------------------------------------------------------------------
// The clauses of the row, IN-PROCESS: (a) spawn path, (b) reuse, (c) stale
// lock, (d) foreign listener — (e) the in-process HALF, and (f)/(g)'s
// process-level halves live in `core/tests/transport_lifecycle.rs`.
// ----------------------------------------------------------------------------

/// Wait until an open session at the endpoint can be handshaked, for the
/// FAKE listener THREADS whose bind happens on another thread: WITHOUT this
/// the test could connect-refuse BEFORE the fake listener is bound and read
/// the live daemon as clause (c). Only `NothingListening` RETRIES; every
/// other outcome — INCLUDING a REFUSAL — means the listener is there.
#[cfg(test)]
fn wait_for_handshake(endpoint: &Endpoint, nonce: String) -> Attempt {
    let mut last = Attempt::NothingListening(endpoint.host.to_string(), "wait exhausted".to_string());
    let mut tries: i64 = 0;
    while tries < 40 {
        let attempt = open_session(endpoint, nonce.to_string(), HANDSHAKE_TIMEOUT_SECONDS, DEFAULT_IDLE_SECONDS);
        match attempt {
            (Some(_), why) => { return why; }
            (None, why) => {
                match why {
                    Attempt::NothingListening(_, _) => { }
                    _ => { return why; }
                }
                last = why;
            }
        }
        std::thread::sleep(millis_pause(50));
        tries += 1;
    }
    return last;
}

#[test]
fn no_daemon_and_no_lock_take_the_spawn_path_and_hand_the_child_its_flags_as_data() {
    // Clause (a): NO daemon and NO lock -> the SPAWN path is taken, and
    // the SEAM is HANDED the executable path plus the documented flag
    // list as DATA ([`spawn_arguments`]: `--serve`,
    // `--address=`, `--port=`, `--data-dir=`, `--idle-timeout=`).
    // The recorder starts NOTHING and REFUSES the START, so the typed
    // [`SessionFailure.SpawnRefused`] naming the CONFIGURED ADDRESS closes
    // the test — the spawn HAVING BEEN ATTEMPTED is what clause (a) asserts.
    let dir = scratch_dir("no_daemon_and_no_lock_take_the_spawn_path_and_hand_the_child_its_flags_as_data".to_string());
    let port = free_port(40201);
    write_config(&dir, port);
    let mut spawner = FakeSpawner {
        starts: Vec::<Vec::<String>>::new(),
        succeed_start: false,
        plays_daemon: false,
        host: DEFAULT_ADDRESS.to_string(),
        port: port,
        data_dir_text: dir.to_string_lossy().to_string(),
        new_nonce: "aaaaaaaaaaaaaaaa".to_string(),
        new_session: String::new()
    };
    let held = connect_or_spawn("/marker/executable/handed/as/data".to_string(), &dir, DEFAULT_IDLE_SECONDS, HANDSHAKE_TIMEOUT_SECONDS, 1, &mut spawner);
    match held.0 {
        Some(_) => { assert!(false); }
        None => { }
    }
    assert!(held.1.len() == 1);
    let expected_at = endpoint_text(DEFAULT_ADDRESS.to_string(), port);
    match &held.1[0] {
        SessionFailure::SpawnRefused(at, _) => { assert!(at == &expected_at); }
        _ => { assert!(false); }
    }
    assert!(spawner.starts.len() == 1);
    let recorded = &spawner.starts[0];
    assert!(recorded[0] == "/marker/executable/handed/as/data");
    assert!(contains_string(recorded, DAEMON_FLAG.to_string()));
    assert!(contains_string(recorded, format!("{flag}{host}", flag = ADDRESS_FLAG, host = DEFAULT_ADDRESS.to_string())));
    assert!(contains_string(recorded, format!("{flag}{value}", flag = PORT_FLAG, value = port.to_string())));
    assert!(contains_string(recorded, format!("{flag}{value}", flag = DATA_DIR_FLAG, value = dir.to_string_lossy().to_string())));
    assert!(contains_string(recorded, format!("{flag}{value}", flag = IDLE_FLAG, value = DEFAULT_IDLE_SECONDS.to_string())));
    cleanup_scratch(&dir);
}

#[test]
fn a_spawned_daemon_that_never_answers_hands_back_a_typed_unreachable() {
    // The other half of clause (a)'s machinery: the START SUCCEEDED
    // (recorder) and NOTHING ever wrote a fresh lock nor answered at the
    // endpoint -> typed [`SessionFailure.SpawnedUnreachable`] naming the
    // CONFIGURED ADDRESS, and the STALE NONCE was never handshaked.
    let dir = scratch_dir("a_spawned_daemon_that_never_answers_hands_back_a_typed_unreachable".to_string());
    let port = free_port(40251);
    write_config(&dir, port);
    let mut spawner = FakeSpawner {
        starts: Vec::<Vec::<String>>::new(),
        succeed_start: true,
        plays_daemon: false,
        host: DEFAULT_ADDRESS.to_string(),
        port: port,
        data_dir_text: dir.to_string_lossy().to_string(),
        new_nonce: "aaaaaaaaaaaaaaaa".to_string(),
        new_session: String::new()
    };
    let held = connect_or_spawn("/marker/executable".to_string(), &dir, DEFAULT_IDLE_SECONDS, HANDSHAKE_TIMEOUT_SECONDS, 0, &mut spawner);
    match held.0 {
        Some(_) => { assert!(false); }
        None => { }
    }
    assert!(held.1.len() == 1);
    let expected_at = endpoint_text(DEFAULT_ADDRESS.to_string(), port);
    match &held.1[0] {
        SessionFailure::SpawnedUnreachable(at, _) => { assert!(at == &expected_at); }
        _ => { assert!(false); }
    }
    assert!(spawner.starts.len() == 1);
    cleanup_scratch(&dir);
}

#[test]
fn an_existing_daemon_is_reused_starts_no_second_core_and_keeps_the_lock_file() {
    // Clause (b): lock file present + a listener answering the handshake
    // -> REUSE. NO SECOND CORE: the recorder-spawner was never called.
    // The lock file is NOT removed on the reuse path
    // (`architecture.md:248`).
    let dir = scratch_dir("an_existing_daemon_is_reused_starts_no_second_core_and_keeps_the_lock_file".to_string());
    let port = free_port(40301);
    write_config(&dir, port);
    let nonce = "6f2c1a48e0b7d9c3".to_string();
    let record = LockRecord { address: endpoint_text(DEFAULT_ADDRESS.to_string(), port), nonce: nonce.to_string() };
    match write_lock(&dir, &record) {
        Err(_) => { }
        Ok(_) => { }
    }
    start_fake_daemon(DEFAULT_ADDRESS.to_string(), port, session_id_of(&nonce));
    let endpoint = Endpoint { host: DEFAULT_ADDRESS.to_string(), port: port };
    match wait_for_handshake(&endpoint, nonce.to_string()) {
        Attempt::Handshook(_, _) => { }
        _ => { assert!(false); }
    }
    let mut spawner = FakeSpawner {
        starts: Vec::<Vec::<String>>::new(),
        succeed_start: true,
        plays_daemon: false,
        host: DEFAULT_ADDRESS.to_string(),
        port: port,
        data_dir_text: dir.to_string_lossy().to_string(),
        new_nonce: "bbbbbbbbbbbbbbbb".to_string(),
        new_session: String::new()
    };
    let held = connect_or_spawn("/marker/executable".to_string(), &dir, DEFAULT_IDLE_SECONDS, HANDSHAKE_TIMEOUT_SECONDS, 0, &mut spawner);
    match held.0 {
        Some(connection) => {
            assert!(connection.session == session_id_of(&nonce));
            assert!(connection.address == endpoint_text(DEFAULT_ADDRESS.to_string(), port));
        }
        None => { assert!(false); }
    }
    assert!(held.1.len() == 0);
    assert!(spawner.starts.len() == 0);
    // The lock file is STILL there with the SAME nonce: reuse never removes
    // it.
    match read_lock(&dir) {
        LockReading::Read(fresh) => { assert!(fresh.nonce == nonce); }
        _ => { assert!(false); }
    }
    cleanup_scratch(&dir);
}

#[test]
fn a_stale_lock_gives_a_new_daemon_and_the_lock_file_is_overwritten() {
    // Clause (c): a readable lock whose endpoint holds NOTHING
    // (a daemon that has since EXITED left it) -> the flow SPAWNS,
    // the NEW daemon's OWN fresh nonce (written by the recorder playing
    // the daemon, as `daemon_start` does) is the one handshaked, and
    // the lock file now holds THAT fresh nonce: OVERWRITTEN.
    let dir = scratch_dir("a_stale_lock_gives_a_new_daemon_and_the_lock_file_is_overwritten".to_string());
    let port = free_port(40401);
    write_config(&dir, port);
    let stale = "6f2c1a48e0b7d9c3".to_string();
    let record = LockRecord { address: endpoint_text(DEFAULT_ADDRESS.to_string(), port), nonce: stale.to_string() };
    match write_lock(&dir, &record) {
        Err(_) => { }
        Ok(_) => { }
    }
    let fresh = "bbbbbbbbbbbbbbbb".to_string();
    let mut spawner = FakeSpawner {
        starts: Vec::<Vec::<String>>::new(),
        succeed_start: true,
        plays_daemon: true,
        host: DEFAULT_ADDRESS.to_string(),
        port: port,
        data_dir_text: dir.to_string_lossy().to_string(),
        new_nonce: fresh.to_string(),
        new_session: session_id_of(&fresh)
    };
    let held = connect_or_spawn("/marker/executable".to_string(), &dir, DEFAULT_IDLE_SECONDS, HANDSHAKE_TIMEOUT_SECONDS, 2, &mut spawner);
    assert!(spawner.starts.len() == 1);
    match held.0 {
        Some(connection) => { assert!(connection.session == session_id_of(&fresh)); }
        None => { assert!(false); }
    }
    assert!(held.1.len() == 0);
    match read_lock(&dir) {
        // OVERWRITTEN: the lock NOW holds the fresh nonce — clause (c) seen
        // from the FILE.
        LockReading::Read(now) => { assert!(now.nonce == fresh); }
        _ => { assert!(false); }
    }
    cleanup_scratch(&dir);
}

#[test]
fn an_unrelated_listener_is_refused_naming_the_configured_address_and_only_the_handshake_line_reached_it() {
    // Clause (d): a listener at the configured address answers the
    // version request with a line that is NOT a `hello`:
    // `architecture.md:250` — an error NAMING THE CONFIGURED ADDRESS,
    // NO spawn (the address is TAKEN), and the peer's log shows it
    // received NOTHING but the handshake line: no command of ANY kind
    // was ever sent, which is what "the interface never sends commands
    // to an unknown process" MEANS observable.
    let dir = scratch_dir("an_unrelated_listener_is_refused_naming_the_configured_address_and_only_the_handshake_line_reached_it".to_string());
    let port = free_port(40501);
    write_config(&dir, port);
    let nonce = "6f2c1a48e0b7d9c3".to_string();
    let record = LockRecord { address: endpoint_text(DEFAULT_ADDRESS.to_string(), port), nonce: nonce.to_string() };
    match write_lock(&dir, &record) {
        Err(_) => { }
        Ok(_) => { }
    }
    let log_path = dir.join("peer_saw.log".to_string()).to_path_buf();
    start_foreign_listener(DEFAULT_ADDRESS.to_string(), port, log_path.to_string_lossy().to_string());
    let endpoint = Endpoint { host: DEFAULT_ADDRESS.to_string(), port: port };
    // Make sure the FOREIGN listener is up BEFORE the DISCOVERY runs
    // (else the test would read the port EMPTY and take the spawn path
    // of clause (c) instead):
    match wait_for_handshake(&endpoint, nonce.to_string()) {
        Attempt::NotOurDaemon(_, _) => { }
        _ => { assert!(false); }
    }
    let mut spawner = FakeSpawner {
        starts: Vec::<Vec::<String>>::new(),
        succeed_start: true,
        plays_daemon: false,
        host: DEFAULT_ADDRESS.to_string(),
        port: port,
        data_dir_text: dir.to_string_lossy().to_string(),
        new_nonce: "bbbbbbbbbbbbbbbb".to_string(),
        new_session: String::new()
    };
    let held = connect_or_spawn("/marker/executable".to_string(), &dir, DEFAULT_IDLE_SECONDS, HANDSHAKE_TIMEOUT_SECONDS, 1, &mut spawner);
    match held.0 {
        Some(_) => { assert!(false); }
        None => { }
    }
    assert!(held.1.len() == 1);
    let expected_at = endpoint_text(DEFAULT_ADDRESS.to_string(), port);
    match &held.1[0] {
        SessionFailure::NotAtEndpoint(at, _) => { assert!(at == &expected_at); }
        _ => { assert!(false); }
    }
    // NO SPAWN: the address is TAKEN.
    assert!(spawner.starts.len() == 0);
    // The peer's log: exactly the ONE handshake frame, and NOTHING else.
    let mut seen_text = String::new();
    let mut tries: i64 = 0;
    while tries < 40 {
        match std::fs::read_to_string(&log_path) {
            Err(_) => { }
            Ok(held_text) => { seen_text = held_text.to_string(); }
        }
        if seen_text.len() > 0 {
            break;
        }
        std::thread::sleep(millis_pause(50));
        tries += 1;
    }
    assert!(seen_text == version_request_frame(&nonce));
    // THE SAME FACT SEEN TYPED, which is the amended row's exact wording
    // ("the peer demonstrably received NOTHING but the handshake line"):
    // the bytes DECODE via plan 3.1's codec to EXACTLY the `version`
    // COMMAND carrying THIS test's nonce — the handshake request of
    // `core-protocol.md:15` — and NO verb of plan 3.2's command table
    // except `version` occurs ANYWHERE in them, so no command of any kind
    // ("the interface never sends commands to an unknown process",
    // `architecture.md:250`) can have crossed this socket.
    let mut asked = false;
    match proto::codec::decode_line(&seen_text) {
        Err(_) => { assert!(false); }
        Ok(held) => {
            match held {
                proto::codec::Message::Inbound(proto::command::Command::Version(given)) => {
                    asked = true;
                    assert!(given == nonce);
                }
                _ => { assert!(false); }
            }
        }
    }
    assert!(asked);
    let rows = proto::commands::command_rows();
    for name in proto::commands::command_names(&rows) {
        if name != "version".to_string() {
            assert!(!seen_text.contains(&name));
        }
    }
    cleanup_scratch(&dir);
}

#[test]
fn an_unreadable_lock_file_refuses_the_session_naming_the_address_and_starts_no_second_daemon() {
    // THE FINDING OF THE REVIEW OF PLAN ITEM 3.6b, closed on the INTERFACE
    // side: an EXISTING-but-UNREADABLE lock file used to read as ABSENT, and
    // ABSENT means SPAWN — which `architecture.md:247` FORBIDS ("If a lock
    // file exists, the interface connects and sends the handshake"): the
    // daemon that WROTE the unreadable lock may STILL be listening, and NO
    // SECOND daemon may be started at a CONFIGURED address a lock EXISTS
    // for. Here `state/core.lock` is a DIRECTORY (the file EXISTS, `std::io`
    // refuses the READ with something that is NOT `NotFound`), so
    // [`connect_or_spawn`] REFUSES with the typed
    // [`SessionFailure.LockUnreadable`] NAMING THE CONFIGURED ADDRESS, and
    // the recorder-spawner shows NO SPAWN HAPPENED.
    let dir = scratch_dir("an_unreadable_lock_file_refuses_the_session_naming_the_address_and_starts_no_second_daemon".to_string());
    let port = free_port(40901);
    write_config(&dir, port);
    match std::fs::create_dir(&dir.join(STATE_DIR_NAME.to_string())) {
        Err(_) => { assert!(false); }
        Ok(_) => { }
    }
    match std::fs::create_dir(&dir.join(STATE_DIR_NAME.to_string()).join(LOCK_FILE_NAME.to_string())) {
        Err(_) => { assert!(false); }
        Ok(_) => { }
    }
    let mut spawner = FakeSpawner {
        starts: Vec::<Vec::<String>>::new(),
        succeed_start: true,
        plays_daemon: false,
        host: DEFAULT_ADDRESS.to_string(),
        port: port,
        data_dir_text: dir.to_string_lossy().to_string(),
        new_nonce: "bbbbbbbbbbbbbbbb".to_string(),
        new_session: String::new()
    };
    let held = connect_or_spawn("/marker/executable".to_string(), &dir, DEFAULT_IDLE_SECONDS, HANDSHAKE_TIMEOUT_SECONDS, 1, &mut spawner);
    match held.0 {
        Some(_) => { assert!(false); }
        None => { }
    }
    assert!(held.1.len() == 1);
    match &held.1[0] {
        SessionFailure::LockUnreadable(at, _) => { assert!(at == &endpoint_text(DEFAULT_ADDRESS.to_string(), port)); }
        _ => { assert!(false); }
    }
    // NO SECOND DAEMON: the seam was NEVER asked.
    assert!(spawner.starts.len() == 0);
    cleanup_scratch(&dir);
}

#[test]
fn the_connection_gate_refuses_a_hand_built_endpoint_the_configuration_would_refuse() {
    // `parse_endpoint` (transport_cfg) WIRED into the connection gate by
    // this sub-row — it had NO production caller, dead surface the 0-warning
    // gate cannot flag for a `pub mod` of a bin crate. A HAND-BUILT
    // [`Endpoint`] with a port OUTSIDE `0..65_535` or an EMPTY HOST names
    // NOTHING that could answer a version request, so [`open_session`]
    // REFUSES with the typed [`Attempt.FrameUnwritten`] NAMING THE ENDPOINT
    // TEXT THE CALLER GAVE and opens NO SOCKET: on the EMPTY-host case the
    // port is FREE, so the ABSENCE of the `NothingListening` reading IS the
    // PROOF the connect never happened (same proof style as the
    // illformed-nonce refusal below).
    let wide = Endpoint { host: DEFAULT_ADDRESS.to_string(), port: 70_000 };
    match open_session(&wide, "aaaaaaaaaaaaaaaa".to_string(), HANDSHAKE_TIMEOUT_SECONDS, DEFAULT_IDLE_SECONDS) {
        (Some(_), _) => { assert!(false); }
        (None, why) => {
            match why {
                Attempt::FrameUnwritten(at, complaint) => {
                    assert!(at == endpoint_text(DEFAULT_ADDRESS.to_string(), 70_000));
                    assert!(complaint.len() > 0);
                }
                _ => { assert!(false); }
            }
        }
    }
    let blank = Endpoint { host: String::new(), port: free_port(40951) };
    match open_session(&blank, "aaaaaaaaaaaaaaaa".to_string(), HANDSHAKE_TIMEOUT_SECONDS, DEFAULT_IDLE_SECONDS) {
        (Some(_), _) => { assert!(false); }
        (None, why) => {
            match why {
                Attempt::FrameUnwritten(at, complaint) => {
                    assert!(complaint.len() > 0);
                    assert!(at == endpoint_text(String::new(), blank.port));
                }
                _ => { assert!(false); }
            }
        }
    }
}

#[cfg(test)]
struct StreamerSpec {
    pub host: String,
    pub port: i64,
    /// How many bytes the peer streams WITHOUT EVER SENDING A line
    /// end, in 4 KiB WRITES: BELOW, AT THE EDGE of and OVER
    /// [`PENDING_MAX_BYTES`] across the two tests that use this.
    pub bytes: i64
}

#[cfg(test)]
fn streamer_work(spec: StreamerSpec) {
    // THE PEER THAT STREAMS WITH NO line end: CONNECT (the backlog
    // trick of [`socket_pair`]'s probe fact — the caller's listener
    // needs NO accept for this connect to COMPLETE) and push `bytes`
    // BYTES; its OWN send buffer BACK-pressures it EXACTLY as a REAL
    // hostile peer, and when the CALLER's end DIES the pending WRITE
    // ENDS WITH AN ERROR the loop ignores. `close_after` is a probe
    // fact about the KERNEL'S OWN buffer, pinned as an ASSERTION BY
    // THE CALLER (the over-limit test below): a peer whose every byte
    // ARRIVES is INDISTINGUISHABLE from one still writing — the limit
    // check fires on the READ side, ONE byte at a time.
    // THE PEER'S bytes ARRIVE at the socket the CALLER's LISTENER
    // ACCEPTS — the caller ACCEPTS this very connection below, which
    // is ALSO the REAL shape: the DAEMON reads sessions from an
    // ACCEPTED socket (`core-protocol.md:14`), never from the
    // interface's own handle.
    let short: u16 = spec.port.try_into().unwrap_or(0);
    match TcpStream::connect((spec.host.to_string(), short)) {
        Err(_) => { return; }
        Ok(mut socket) => {
            let mut chunk = String::new();
            while chunk.len() < 4096 {
                chunk += "aaaaaaaa";
            }
            let mut written = 0;
            while written < spec.bytes {
                match socket.write_all(chunk.as_bytes()) {
                    Err(_) => { return; }
                    Ok(_) => { }
                }
                match socket.flush() {
                    Err(_) => { return; }
                    Ok(_) => { }
                }
                written += 4096;
            }
        }
    }
}

#[test]
fn a_peer_just_under_the_pending_limit_with_no_line_end_ends_as_the_half_line_flush_never_a_complaint() {
    // THE LOW SIDE of the SAME BOUNDARY ([`PENDING_MAX_BYTES`], the
    // transport's OWN RECORDED INVENTION — NOT plan 3.4's 65_536-LINE
    // pending BUDGET, which `interpret` pins in ITS OWN unit tests):
    // 61440 bytes withOUT A line end, then the peer's socket DIES.
    // NEVER a limit complaint — the bytes are ALL handed out, the
    // framing rule holds to the end ("the stream never resynchronizes
    // on a partial line", `core-protocol.md:203-209`: at the END of
    // the input the HALF line is handed out and NOTHING completes it)
    // — read through the DAEMON's OWN [`StreamText`]. The reads run
    // BOUNDED against the deadline (the peer's thread AND its socket
    // end on their own), and the test ACCEPTS BOTH ROUTEs the kernel
    // takes (mid-stream WouldBlock → IDLE `Done`, or the peer finishing
    // first) — the ASSERTION is on the TEXT EVENTUALLY handed out:
    // EXACTLY the 61440 bytes ONCE, and the limit NEVER complains.
    let port = free_port(41501);
    match TcpListener::bind(("127.0.0.1".to_string(), port.try_into().unwrap_or(0))) {
        Err(_) => { assert!(false); }
        Ok(_listener) => {
            let spec = StreamerSpec { host: "127.0.0.1".to_string(), port: port, bytes: 61_440 };
            let _peer_thread = std::thread::spawn(|| -> () { streamer_work(spec); });
            // THE CALLER ACCEPTS THE PEER'S OWN CONNECTION — blocking
            // here is BOUNDED by the peer thread's connect, and the
            // accepted socket is EXACTLY what the DAEMON reads:
            match _listener.accept() {
                Err(_) => { assert!(false); }
                Ok(held) => {
                    let interface_end = held.0;
                    set_read_deadline(&interface_end, 1);
                    let mut stream = StreamText {
                        reader: interface_end,
                        pending: String::new(),
                        ended: false,
                        unreadable: String::new(),
                        timed_out: false,
                        flushed: false
                    };
                    let mut handed = String::new();
                    let mut tries = 0;
                    while tries < 20 {
                        match stream.read_text() {
                            interpret::TextRead::Text(half) => { handed = half; break; }
                            interpret::TextRead::Done => { }
                            interpret::TextRead::Unreadable(why) => {
                                println!("limit complained UNDER the limit: {why}", why = why);
                                assert!(false);
                            }
                        }
                        tries += 1;
                    }
                    if handed.len() != 61_440 { assert!(false); }
                }
            }
        }
    }
}

#[test]
fn the_pending_limit_refuses_a_peer_streaming_without_a_line_end_and_the_buffer_never_grows() {
    // THE HIGH SIDE: the peer streams 4096 OVER the limit. The check
    // fires ONE BYTE AT A TIME (`StreamText` reads a single byte and
    // checks AFTER EVERY read), so the FIRST complaint is CERTAIN to
    // carry EXACTLY `PENDING_MAX_BYTES + 1` seen bytes WHATEVER the
    // kernel's buffer sizes — and after the drop the SAME complaint
    // repeats VERBATIM: the pending buffer was DROPPED, it NEVER
    // grew. That is `architecture.md:244`'s "malformed input NEVER
    // kills the daemon" seen from the reader side: the SESSION ends,
    // the STREAM is dropped, NOTHING hangs — the read loop below is
    // BOUNDED by the deadline of EVERY read.
    let port = free_port(41601);
    match TcpListener::bind(("127.0.0.1".to_string(), port.try_into().unwrap_or(0))) {
        Err(_) => { assert!(false); }
        Ok(_listener) => {
            let spec = StreamerSpec { host: "127.0.0.1".to_string(), port: port, bytes: 73_728 };
            let _peer_thread = std::thread::spawn(|| -> () { streamer_work(spec); });
            // THE CALLER ACCEPTS THE PEER'S OWN CONNECTION — blocking
            // here is BOUNDED by the peer thread's connect, and the
            // accepted socket is EXACTLY what the DAEMON reads:
            match _listener.accept() {
                Err(_) => { assert!(false); }
                Ok(held) => {
                    let interface_end = held.0;
                    set_read_deadline(&interface_end, 1);
                    let mut stream = StreamText {
                        reader: interface_end,
                        pending: String::new(),
                        ended: false,
                        unreadable: String::new(),
                        timed_out: false,
                        flushed: false
                    };
                    let mut first = String::new();
                    let mut tries = 0;
                    while tries < 30 {
                        match stream.read_text() {
                            interpret::TextRead::Unreadable(why) => { first = why; break; }
                            interpret::TextRead::Done => { }
                            interpret::TextRead::Text(_half) => { }
                        }
                        tries += 1;
                    }
                    if first.len() == 0 { assert!(false); }
                    assert!(first.contains("65536"));
                    assert!(first.contains("65537"));
                    match stream.read_text() {
                        interpret::TextRead::Unreadable(again) => { assert!(again == first); }
                        _ => { assert!(false); }
                    }
                }
            }
        }
    }
}

#[test]
fn an_illformed_nonce_is_refused_before_the_peer_is_asked_for_a_version_request() {
    // 3.6a review finding 4 pinned HERE: [`open_session`] is this build's
    // ONLY builder of a handshake request, and `version_request_frame`
    // SWALLOWS a codec refusal into `String::new`
    // (`handshake.rs:267-272`), so the nonce is validated with
    // `is_wellformed_nonce` BEFORE a frame is asked for — and BEFORE the
    // socket is opened at all: the typed [`Attempt.FrameUnwritten`] NAMES
    // the CONFIGURED ADDRESS and NOTHING could have reached whatever
    // listens there. The endpoint port is a FREE port, so the ABSENCE of
    // the `NothingListening` reading is itself the PROOF that the refusal
    // happened upstream of the connect.
    let endpoint = Endpoint { host: DEFAULT_ADDRESS.to_string(), port: free_port(40801) };
    match open_session(&endpoint, "NOT-A-NONCE".to_string(), HANDSHAKE_TIMEOUT_SECONDS, DEFAULT_IDLE_SECONDS) {
        (Some(_), _) => { assert!(false); }
        (None, why) => {
            match why {
                Attempt::FrameUnwritten(at, why_text) => {
                    assert!(at == endpoint_text(DEFAULT_ADDRESS.to_string(), endpoint.port));
                    assert!(why_text.len() > 0);
                }
                _ => { assert!(false); }
            }
        }
    }
}

// ----------------------------------------------------------------------------
// Clause (e): two interfaces on ONE daemon — whole-command-by-whole-
// command, never line-by-line. The in-process half of the row; the
// process-level half lives in `core/tests/transport_lifecycle.rs`.
// ----------------------------------------------------------------------------

#[cfg(test)]
fn read_stream_lines(socket: &TcpStream) -> Vec::<String> {
    // EVERY line THIS reader received, in arrival order, empty segments
    // between NEWLINES skipped: the reader is driven to EOF (its write
    // end CLOSED with its session). ONE consumption of the stream, THEN
    // the two analyses below run on the LINES.
    let mut lines = Vec::<String>::new();
    for line in read_all_text(socket).split("\n") {
        if line.len() == 0 {
            continue;
        }
        lines.push(line.to_string());
    }
    return lines;
}

#[cfg(test)]
fn kinds_of(lines: &Vec::<String>) -> String {
    // The KIND name of every DECODED outbound line, `|`-joined: Vec-of-
    // String equality against a bracket literal does not TYPECHECK in
    // this toolchain (a bracket literal infers `Vec<&str>`), so the
    // comparison of the interleaving OBSERVATION goes through the join.
    let mut kinds = Vec::<String>::new();
    for line in lines {
        match proto::codec::decode_line(&line) {
            Err(_) => { }
            Ok(held) => {
                match held {
                    proto::codec::Message::Outbound(event) => { kinds.push(proto::event::event_kind(&event)); }
                    _ => { }
                }
            }
        }
    }
    return kinds.join("|");
}

#[cfg(test)]
fn hello_of(lines: &Vec::<String>) -> (String, String) {
    // The (nonce, session) of the HELLO line among the lines: clause
    // (e)'s "EACH client gets a hello" observable — its OWN nonce echoed,
    // the daemon's ONE session carried to BOTH readers.
    for line in lines {
        match proto::codec::decode_line(&line) {
            Err(_) => { }
            Ok(held) => {
                match held {
                    proto::codec::Message::Outbound(proto::event::Event::Hello(nonce_field, session_field, _)) => {
                        return (nonce_field.to_string(), session_field.to_string());
                    }
                    _ => { }
                }
            }
        }
    }
    return (String::new(), String::new());
}

#[cfg(test)]
#[cfg(test)]
struct CountingWork {
    pub log: Vec::<String>,
}

#[cfg(test)]
impl interpret::Work for CountingWork {
    // The recorder-work of the shared-daemon test: it RECORDS the command
    // NOUN it was run FOR, and emits as MANY `progress` lines as the
    // `writer` session was opened with — an `import` session (ONE
    // parameter of the command table) answers ONE progress line, every
    // OTHER command of the table TWO. THAT asymmetry is what makes a
    // LINE-BY-LINE interleaving of the two sessions (the forbidden
    // reading) OBSERVABLE: reader B's stream would CARRY part of a
    // two-progress block from reader A's command.
    fn do_command(&mut self, command: proto::command::Command, sink: &mut dyn EventSink) -> interpret::Answered {
        let name = proto::command::command_name(&command);
        self.log.push(name.to_string());
        let mut total: i64 = 2;
        match command {
            proto::command::Command::Import(_) => { total = 1; }
            _ => { }
        }
        let mut stream = crate::protocol::writer::start(name.to_string(), total);
        let mut written: i64 = 0;
        while written < total {
            match crate::protocol::writer::progress(&mut stream, sink, None) {
                Err(_) => { assert!(false); }
                Ok(_) => { }
            }
            written += 1;
        }
        match crate::protocol::writer::succeed(&mut stream, sink, &serde_json::Map::<String, Value>::new()) {
            Err(_) => { assert!(false); }
            Ok(_) => { }
        }
        return interpret::Answered::EndedWith;
    }
}

#[test]
fn one_daemon_hands_hellos_to_two_interfaces_which_interleave_whole_command_by_whole_command_never_line_by_line() {
    // Clause (e), IN-PROCESS:
    // * ONE daemon — the SAME [`serve_daemon`] call serves BOTH
    //   connections, and EACH reader's hello echoes ITS OWN nonce while
    //   carrying the SAME daemon session: identity is per-daemon,
    //   handshakes are per-session;
    // * the interleaving UNIT — reader A runs `resume` (ONE progress
    //   line) THEN `list-pending` (TWO), and its stream shows each
    //   command as a CONTIGUOUS whole block, never interleaved WITH
    //   reader B's `import` block: reader B sees exactly ONE progress
    //   line for ITS whole stream, so NOTHING from A's TWO-progress
    //   command CROSS-TALKED into B;
    // * the work recorder shows the WHOLE commands in ACCEPT order
    //   (`import, list-pending` from A then `import` from B) — never
    //   half-commands, never the line-granular mix a "both streams at
    //   once" schedule WOULD produce.
    // Sessions END at EOF (no shutdown here: THAT is clause (g), and
    // its PROCESS-level form); the daemon then exits on the idle timeout.
    let dir = scratch_dir("one_daemon_hands_hellos_to_two_interfaces_which_interleave_whole_command_by_whole_command_never_line_by_line".to_string());
    let port = free_port(40601);
    let short: u16 = port.try_into().unwrap_or(0);
    match TcpListener::bind(("127.0.0.1", short)) {
        Err(_) => { assert!(false); }
        Ok(listener) => {
            match TcpStream::connect(("127.0.0.1", short)) {
                Err(_) => { assert!(false); }
                Ok(reader_a) => {
                    match TcpStream::connect(("127.0.0.1", short)) {
                        Err(_) => { assert!(false); }
                        Ok(reader_b) => {
                            // Session A: import, THEN list-pending.
                            {
                                let mut writer_a = Option::<TcpStream>::None;
                                match reader_a.try_clone() {
                                    Err(_) => { }
                                    Ok(held_clone) => { writer_a = Some(held_clone); }
                                }
                                match writer_a {
                                    None => { }
                                    Some(mut writer_a) => {
                                for frame in [version_request_frame(&"aaaaaaaaaaaaaaaa".to_string())].to_vec() {
                                    match writer_a.write_all(frame.as_bytes()) {
                                        Err(_) => { }
                                        Ok(_) => { }
                                    }
                                }
                                match proto::codec::encode_frame(&proto::codec::Message::Inbound(proto::command::Command::Import("the-garage".to_string()))) {
                                    Err(_) => { }
                                    Ok(frame) => {
                                        match writer_a.write_all(frame.as_bytes()) {
                                            Err(_) => { assert!(false); }
                                            Ok(_) => { }
                                        }
                                    }
                                }
                                match proto::codec::encode_frame(&proto::codec::Message::Inbound(proto::command::Command::ListPending)) {
                                    Err(_) => { assert!(false); }
                                    Ok(frame) => {
                                        match writer_a.write_all(frame.as_bytes()) {
                                            Err(_) => { assert!(false); }
                                            Ok(_) => { }
                                        }
                                    }
                                }
                                }
                            }
                            }
                            // The CLONE is not enough for EOF: the live
                            // `reader_a` ALSO names the connection, so the
                            // reader's OWN WRITE END must be half-closed —
                            // this toolchain spells it `std::net::Shutdown::
                            // Write` (`SHUT_WR`/`SocketShutdown`/`close_write`
                            // are all ABSENT — probe facts).
                            match reader_a.shutdown(std::net::Shutdown::Write) {
                                Err(_) => { }
                                Ok(_) => { }
                            }
                            // Session B: import, NOTHING else.
                            {
                                let mut writer_b = Option::<TcpStream>::None;
                                match reader_b.try_clone() {
                                    Err(_) => { }
                                    Ok(held_clone) => { writer_b = Some(held_clone); }
                                }
                                match writer_b {
                                    None => { }
                                    Some(mut writer_b) => {
                                for frame in [version_request_frame(&"bbbbbbbbbbbbbbbb".to_string())].to_vec() {
                                    match writer_b.write_all(frame.as_bytes()) {
                                        Err(_) => { }
                                        Ok(_) => { }
                                    }
                                }
                                match proto::codec::encode_frame(&proto::codec::Message::Inbound(proto::command::Command::Import("the-garage".to_string()))) {
                                    Err(_) => { }
                                    Ok(frame) => {
                                        match writer_b.write_all(frame.as_bytes()) {
                                            Err(_) => { assert!(false); }
                                            Ok(_) => { }
                                        }
                                    }
                                }
                            }
                                }
                            }
                            match reader_b.shutdown(std::net::Shutdown::Write) {
                                Err(_) => { }
                                Ok(_) => { }
                            }
                            // The write ends are HALF-CLOSED (the scoped
                            // clones DIED and each reader's own write end
                            // got `Shutdown::Write`), so both sessions
                            // END AT EOF,
                            // idle timeout AFTER THE LAST session ends.
                            let mut work = CountingWork { log: Vec::<String>::new() };
                            let dbg_exit = serve_daemon(&listener, &dir, 1, &mut work, "core-shared".to_string());
                            match dbg_exit {
                                DaemonExit::IdleExit(Removal::WasAbsent) => { }
                                DaemonExit::IdleExit(Removal::Removed) => { }
                                DaemonExit::IdleExit(Removal::Failed(_, _)) => { assert!(false); }
                                _ => { assert!(false); }
                            }
                            // Reader A: hello, then `import` (ONE progress
                            // line), then `list-pending` (TWO), EACH command
                            // a CONTIGUOUS whole block:
                            let lines_a = read_stream_lines(&reader_a);
                            assert!(kinds_of(&lines_a) == "hello|progress|result|progress|progress|result");
                            let (nonce_a, session_a) = hello_of(&lines_a);
                            assert!(nonce_a == "aaaaaaaaaaaaaaaa");
                            assert!(session_a == "core-shared");
                            // Reader B: exactly its OWN block — ONE progress
                            // line ONLY, so NOTHING of A's TWO-progress
                            // command ever CROSS-TALKED into B, and its
                            // hello echoes ITS OWN nonce on the SAME
                            // daemon session:
                            let lines_b = read_stream_lines(&reader_b);
                            assert!(kinds_of(&lines_b) == "hello|progress|result");
                            let (nonce_b, session_b) = hello_of(&lines_b);
                            assert!(nonce_b == "bbbbbbbbbbbbbbbb");
                            assert!(session_b == session_a.to_string());
                        }
                    }
                    cleanup_scratch(&dir);
                }
            }
            cleanup_scratch(&dir);
        }
    }
}

#[test]
fn a_connection_sending_shutdown_ends_the_daemon_and_lock_removal_is_reported() {
    // The in-process HALF of clause (g)'s lock observation: a connection
    // that sends the documented `shutdown` line ([`Command::Shutdown`] of
    // `core-protocol.md:199` — "exit the daemon and remove `state/core.lock`")
    // makes [`serve_one_connection`] answer `ShutDown`, and [`serve_daemon`]
    // EXITS THEN REMOVING THE LOCK: the lock file is written up front, GONE
    // after, and the reported [`DaemonExit.ShutdownExit`] carries the
    // [`Removal`] result — which is exactly what plan 3.5's daemon exit path
    // will report. Clause (g) PROCESS-LEVEL form (a real daemon child, a
    // real shutdown through a real interface) lives in the integration file.
    let dir = scratch_dir("a_connection_sending_shutdown_ends_the_daemon_and_lock_removal_is_reported".to_string());
    let port = free_port(40701);
    let nonce = "6f2c1a48e0b7d9c3".to_string();
    let record = LockRecord { address: endpoint_text(DEFAULT_ADDRESS.to_string(), port), nonce: nonce.to_string() };
    match write_lock(&dir, &record) {
        Err(_) => { assert!(false); }
        Ok(_) => { }
    }
    match TcpListener::bind(("127.0.0.1", port.try_into().unwrap_or(0))) {
        Err(_) => { assert!(false); }
        Ok(listener) => {
            match TcpStream::connect(("127.0.0.1", port.try_into().unwrap_or(0))) {
                Err(_) => { assert!(false); }
                Ok(reader) => {
                    // Version request, THEN the shutdown line, THEN the
                    // write end half-closed: the shape of a real interface
                    // saying GOODBYE to the daemon.
                    match reader.try_clone() {
                        Err(_) => { assert!(false); }
                        Ok(mut writer) => {
                            match writer.write_all(version_request_frame(&nonce).as_bytes()) {
                                Err(_) => { }
                                Ok(_) => { }
                            }
                            match proto::codec::encode_frame(&proto::codec::Message::Inbound(proto::command::Command::Shutdown)) {
                                Err(_) => { }
                                Ok(frame) => {
                                    match writer.write_all(frame.as_bytes()) {
                                        Err(_) => { }
                                        Ok(_) => { }
                                    }
                                }
                            }
                        }
                    }
                    match reader.shutdown(std::net::Shutdown::Write) {
                        Err(_) => { }
                        Ok(_) => { }
                    }
                    let mut nowhere = interpret::wired_nowhere();
                    match serve_daemon(&listener, &dir, 1, &mut nowhere, "core-6f2c".to_string()) {
                        DaemonExit::ShutdownExit(removal) => {
                            match removal {
                                Removal::Removed => { }
                                _ => { assert!(false); }
                            }
                        }
                        _ => { assert!(false); }
                    }
                    // THE LOCK FILE IS GONE (the Removal above said
                    // `Removed`; this is the FILE itself, clause (g)'s
                    // "lock removed on daemon exit" observable).
                    match read_lock(&dir) {
                        LockReading::Absent => { }
                        _ => {
                            println!("lock still present after the daemon exited");
                            assert!(false);
                        }
                    }
                    cleanup_scratch(&dir);
                }
            }
            cleanup_scratch(&dir);
        }
    }
}

// ---------------------------------------------------------------------------
// THE SIX UNPINNED ROUTES of this row's own gate, measured by the review of
// sub-row 3.6c (cycle 2): each mutant SURVIVED the whole suite, so each
// route gets its OWN test HERE — where the row's check puts them ("the
// handshake is checked BEFORE any command", "the sink flushes per line and
// latches", "15 minutes WITHOUT commands", "lock removed on daemon exit").
// ---------------------------------------------------------------------------

#[cfg(test)]
fn start_hello_answering_fake(host: String, port: i64, session: String, reply_nonce: String, hello_version: i64) {
    // The fake daemon of routes 1 and 2: it answers the version request with
    // a `hello` of THIS build's framing, but with the nonce and/or the
    // protocol version of ANOTHER daemon — the two ways a perfect-looking
    // `hello` is NOT our daemon, which `handshake.rs:424` (the nonce echo)
    // and `handshake.rs:427` (the CLAIMED version) each catch.
    let spec = ListenerSpec { host: host.to_string(), port: port, session: session.to_string(), log_path: String::new(), answers_hello: true, reply_nonce: reply_nonce.to_string(), hello_version: hello_version };
    std::thread::spawn(|| -> () { listener_work(spec) });
}

#[test]
fn a_hello_answer_echoing_another_nonce_is_refused_as_not_our_daemon() {
    // ROUTE 1, THE NONCE ECHO. A peer that answers the version request with
    // a WELLFORMED `hello` carrying a DIFFERENT nonce is not our daemon
    // (`core-protocol.md:11`: the daemon is the ONLY answerer of version
    // requests at this address, and the lock file's nonce identifies WHICH
    // daemon), so [`open_session`] REFUSES with [`Attempt.NotOurDaemon`] and
    // hands out NO pair. Pinning it HERE is what makes the refusal of the
    // ECHO belong to this row's gate: the check itself lives in `proto`,
    // but the OUTCOME the interface reports is this module's.
    let port = free_port(57001);
    let asked = "6f2c1a48e0b7d9c3".to_string();
    let foreign = "11ab2233cc4455dd".to_string();
    start_hello_answering_fake("127.0.0.1".to_string(), port, "core-6f2c".to_string(), foreign.to_string(), PROTOCOL_VERSION);
    let endpoint = Endpoint { host: "127.0.0.1".to_string(), port: port };
    match wait_for_handshake(&endpoint, asked) {
        Attempt::NotOurDaemon(at, complaint) => {
            assert!(at == endpoint_text("127.0.0.1".to_string(), port));
            assert!(complaint.len() > 0);
        }
        _ => {
            println!("the wrong-nonce hello came down as something other than NotOurDaemon");
            assert!(false);
        }
    }
}

#[test]
fn a_hello_answer_at_another_protocol_version_is_reported_as_the_spoken_version_not_as_not_our_daemon() {
    // ROUTE 2, THE VERSION-MISMATCH OUTCOME (the carry-forward obligation of
    // the 3.6a/3.6b reviews): a peer answering the version request with a
    // hello that CLAIMS a version this build does not speak reaches
    // [`Attempt.VersionProblem`] with the version SEEN, the version this
    // build speaks and the configured address, and [`refuse`] turns THAT into
    // [`SessionFailure.VersionSpoken`] — a version TRANSITION is plan 3.13's,
    // so this transport starts NO second daemon there and the caller is NOT
    // told the address is occupied or that a foreign listener answered.
    // Asserting the reported FAILURE also pins the ATTEMPT, since `refuse`
    // maps EVERY other attempt to `NotAtEndpoint` (or, for nothing listening,
    // `SpawnedUnreachable`), so only `VersionProblem` can produce this text.
    let port = free_port(57101);
    let asked = "6f2c1a48e0b7d9c3".to_string();
    start_hello_answering_fake("127.0.0.1".to_string(), port, "core-6f2c".to_string(), String::new(), PROTOCOL_VERSION + 1);
    let endpoint = Endpoint { host: "127.0.0.1".to_string(), port: port };
    match wait_for_handshake(&endpoint, asked) {
        Attempt::VersionProblem(seen, spoken, at) => {
            assert!(seen == PROTOCOL_VERSION + 1);
            assert!(spoken == PROTOCOL_VERSION);
            assert!(at == endpoint_text("127.0.0.1".to_string(), port));
            let failures = refuse(Attempt::VersionProblem(seen, spoken, at), endpoint_text("127.0.0.1".to_string(), port));
            assert!(failures.len() == 1);
            for held in failures {
                match held {
                    SessionFailure::VersionSpoken(looked, spoke, at_text) => {
                        assert!(looked == PROTOCOL_VERSION + 1);
                        assert!(spoke == PROTOCOL_VERSION);
                        assert!(at_text == endpoint_text("127.0.0.1".to_string(), port));
                    }
                    _ => {
                        println!("the version mismatch was reportable as not-our-daemon/occupied address");
                        assert!(false);
                    }
                }
            }
        }
        _ => {
            println!("the version mismatch came down as something other than Attempt::VersionProblem");
            assert!(false);
        }
    }
}

#[test]
fn the_named_default_idle_seconds_is_the_15_minutes_without_commands_of_the_doc() {
    // ROUTE 4: [`DEFAULT_IDLE_SECONDS`] is the value that crosses the process
    // boundary of the SPAWNED daemon (`IDLE_FLAG`), so its LENGTH is part of
    // the interface even though every test of this row passes its OWN small
    // window: `core-protocol.md:16` fixes 15 MINUTES. THE LITERALS BELOW
    // NINE HUNDRED AND 15 TIMES 60, NOT the constant, so the named default
    // cannot quietly become another number.
    assert!(DEFAULT_IDLE_SECONDS == 900_i64);
    assert!(DEFAULT_IDLE_SECONDS == 15_i64 * 60_i64);
}


#[test]
fn the_sink_latches_at_a_write_that_fails_on_a_socket_whose_reader_has_gone_away() {
    // ROUTE 5a — the reachable arm of the sink's obligation ("the sink
    // FLUSHES per line and latches the FIRST failure on BOTH arms"): a
    // [`LatchSink`] writing to a connection whose reader has shut down its
    // read end LATCHES, so the line AFTER the break is DROPPED instead of
    // going out of order (plan 3.3's writer has ALREADY counted the failed
    // frame as terminal and cannot observe the sink — the obligation
    // `core/src/protocol/writer.rs` puts on this sink). The loop is a
    // TOOLCHAIN PROBE FACT, pinned as an assertion by the peer: with the
    // reader gone the FIRST `write_all` is still ACCEPTED into the socket
    // buffer and the FLUSH that follows reports OK (an unbuffered socket has
    // nothing to flush), so the failure arrives with the peer's RST on a
    // LATER write — hence "one of the writes of a latched sink HAS to fail",
    // given 20 tries and 50 ms each.
    let port = free_port(57701);
    match TcpListener::bind(("127.0.0.1", port.try_into().unwrap_or(0))) {
        Err(_) => { assert!(false); }
        Ok(listener) => {
            let spec = QuietPeerSpec { host: "127.0.0.1".to_string(), port: port, nonce: String::new(), wait_before_millis: 0, sends_version_request: false, sends_shutdown: false, closes_write: false, closes_read: true, hold_millis: 0 };
            start_quiet_peer(spec);
            match listener.accept() {
                Err(_) => { assert!(false); }
                Ok(pair) => {
                    let mut sink = LatchSink { writer: pair.0, broken: false };
                    let mut tries: i64 = 0;
                    while !sink.broken && tries < 20 {
                        sink.write_line("one\n".to_string());
                        tries += 1;
                        std::thread::sleep(millis_pause(50));
                    }
                    if !sink.broken {
                        println!("the sink never latched on a socket whose reader is gone");
                        assert!(false);
                    }
                }
            }
        }
    }
}

#[test]
fn the_sink_latches_at_a_flush_that_fails_on_an_already_written_line() {
    // ROUTE 5b — the FLUSH arm, named as `core/src/main.rs` names its own
    // sink test. THAT test can hand `LatchLines` a writer whose FLUSH fails,
    // because `LatchLines` is GENERIC over a `std::io::Write`; the writer of
    // [`LatchSink`] is a `TcpStream`, and on THIS toolchain `TcpStream::flush`
    // is the default NO-OP of `std::io::Write` — the probe above PINS
    // that, with the peer's read end CLOSED the failure shows up as `Broken
    // pipe` on the SECOND write while the FLUSH of the FIRST reports OK —
    // so NO socket can hand a FAILURE to the flush arm of THIS module. What
    // is pinned HERE is therefore the arm ITSELF, read back out of this
    // file: the line after the flush call has to be the `Err` arm that SETS
    // `broken`, exactly as the write arm does, so DROPPING the latch from
    // the flush arm FAILS this test and the two arms stay SYMMETRIC.
    // (The arm being UNREACHABLE for a socket is RECORDED for the reviewer:
    // closing it properly means the `std::io::Write` SEAM of `LatchLines`,
    // which is a production change this sub-row does not make.)
    let mut flush_call_seen = false;
    let mut flush_arm_latches = false;
    let mut previous_line_was_the_flush_call = false;
    match std::fs::read("src/technical/transport.rs".to_string()) {
        Err(_) => { }
        Ok(held) => {
            match String::from_utf8(held) {
                Err(_) => { }
                Ok(text) => {
                    for line in text.split("\n") {
                        if previous_line_was_the_flush_call {
                            if line.contains("Err(_)") && line.contains("self.broken = true;") {
                                flush_arm_latches = true;
                            }
                        }
                        previous_line_was_the_flush_call = line.contains("self.writer.flush()");
                        if line.contains("self.writer.flush()") {
                            flush_call_seen = true;
                        }
                    }
                }
            }
        }
    }
    if !flush_call_seen {
        println!("this file no longer flushes per line, so the flush arm is GONE");
        assert!(false);
    }
    if !flush_arm_latches {
        println!("the flush arm of the sink latch no longer latches the first failure");
        assert!(false);
    }
}

#[test]
fn a_connection_that_never_sends_a_line_ends_the_session_as_the_connections_idle_timeout() {
    // ROUTE 6a — the CONNECTION's OWN READ DEADLINE, the half of
    // `core-protocol.md:16`'s "15 minutes WITHOUT commands" that
    // [`serve_one_connection`] implements: the idle timeout IS the read
    // timeout of the open stream (the doc's clause, and the reason the
    // handshake's SHORT timeout is WIDENED in [`open_session`] before the
    // pair goes out). A timed-out read hands plan 3.4's loop
    // [`interpret.TextRead.Done`], so the END OF INPUT it ended on is told
    // apart ONLY by `StreamText`'s FLAG — hence the assertion HERE on
    // [`SessionEnd.IdleTimeout`] itself, which NOTHING in the suite checked
    // before (the live lifecycle test is satisfied by the accept clock alone,
    // see the two tests below). ONE SECOND stands in for the 15 MINUTES;
    // the peer's write end stays OPEN, so the session CANNOT end at end-of-
    // input, and the ONLY thing that can end it is the DEADLINE.
    let port = free_port(57501);
    match TcpListener::bind(("127.0.0.1", port.try_into().unwrap_or(0))) {
        Err(_) => { assert!(false); }
        Ok(listener) => {
            let spec = QuietPeerSpec { host: "127.0.0.1".to_string(), port: port, nonce: "6f2c1a48e0b7d9c3".to_string(), wait_before_millis: 0, sends_version_request: true, sends_shutdown: false, closes_write: false, closes_read: false, hold_millis: 4000 };
            start_quiet_peer(spec);
            match listener.accept() {
                Err(_) => { assert!(false); }
                Ok(pair) => {
                    let mut nowhere = interpret::wired_nowhere();
                    let session_text = "core-6f2c".to_string();
                    match serve_one_connection(pair.0, 1, &session_text, &mut nowhere) {
                        SessionEnd::IdleTimeout => { }
                        _ => {
                            println!("the quiet connection ended otherwise than SessionEnd::IdleTimeout");
                            assert!(false);
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn a_daemon_that_never_sees_a_connection_exits_idle_on_the_accept_loop_clock() {
    // ROUTE 6b — the ACCEPT-LOOP clock, the OTHER route to the same
    // documented exit: a listener NOBODY ever connects to, so the daemon
    // DIES on `waited >= idle_millis` with no connection in hand and NO
    // command seen, REMOVING the lock file (`data-files.md:143`). Distinct
    // from the test above because NOTHING is being SERVED there — that is
    // the reading of "WITHOUT commands" at the accept boundary.
    let dir = scratch_dir("a_daemon_that_never_sees_a_connection_exits_idle_on_the_accept_loop_clock".to_string());
    let port = free_port(57601);
    let nonce = "6f2c1a48e0b7d9c3".to_string();
    let record = LockRecord { address: endpoint_text("127.0.0.1".to_string(), port), nonce: nonce.to_string() };
    match write_lock(&dir, &record) {
        Err(_) => { assert!(false); }
        Ok(_) => { }
    }
    match TcpListener::bind(("127.0.0.1", port.try_into().unwrap_or(0))) {
        Err(_) => { assert!(false); }
        Ok(listener) => {
            let mut nowhere = interpret::wired_nowhere();
            match serve_daemon(&listener, &dir, 1, &mut nowhere, "core-6f2c".to_string()) {
                DaemonExit::IdleExit(removal) => {
                    match removal {
                        Removal::Removed => { }
                        _ => {
                            println!("the idle exit of the accept clock did not report the removal");
                            assert!(false);
                        }
                    }
                }
                _ => {
                    println!("a daemon with no connection at all exited otherwise than IdleExit");
                    assert!(false);
                }
            }
            match read_lock(&dir) {
                LockReading::Absent => { }
                _ => {
                    println!("the lock file survived the idle exit");
                    assert!(false);
                }
            }
            cleanup_scratch(&dir);
        }
    }
}

#[test]
fn a_quiet_connection_ends_the_daemon_through_the_idle_timeout_arm_of_the_accept_loop() {
    // ROUTE 6c — the ARM that CONNECTS the two routes:
    // `SessionEnd::IdleTimeout => return DaemonExit::IdleExit(remove_lock(
    // data_dir))`. A connection that answers the version request and then
    // goes SILENT hands [`serve_one_connection`] the IdleTimeout of the test
    // above, and a SECOND interface is QUEUED BEHIND it sending `shutdown`:
    // the pinned build RETURNS on that arm, NEVER accepting the queued
    // one, so the exit IS `IdleExit`; an arm replaced by a NO-OP WALKS ON
    // to the queued interface and reports `ShutdownExit` — the accept-loop
    // clock CANNOT produce that difference, which is why the finding says
    // the READ-DEADLINE route was pinned by NOTHING. The 500 ms delay of
    // the shutdown interface FIXES the accept ORDER (the quiet connection
    // is connected first and is the one ACCEPTED).
    let dir = scratch_dir("a_quiet_connection_ends_the_daemon_through_the_idle_timeout_arm_of_the_accept_loop".to_string());
    let port = free_port(57401);
    let nonce = "6f2c1a48e0b7d9c3".to_string();
    let record = LockRecord { address: endpoint_text("127.0.0.1".to_string(), port), nonce: nonce.to_string() };
    match write_lock(&dir, &record) {
        Err(_) => { assert!(false); }
        Ok(_) => { }
    }
    match TcpListener::bind(("127.0.0.1", port.try_into().unwrap_or(0))) {
        Err(_) => { assert!(false); }
        Ok(listener) => {
            let quiet = QuietPeerSpec { host: "127.0.0.1".to_string(), port: port, nonce: nonce.to_string(), wait_before_millis: 0, sends_version_request: true, sends_shutdown: false, closes_write: false, closes_read: false, hold_millis: 5000 };
            start_quiet_peer(quiet);
            let bye = QuietPeerSpec { host: "127.0.0.1".to_string(), port: port, nonce: nonce.to_string(), wait_before_millis: 500, sends_version_request: true, sends_shutdown: true, closes_write: true, closes_read: false, hold_millis: 0 };
            start_quiet_peer(bye);
            let mut nowhere = interpret::wired_nowhere();
            match serve_daemon(&listener, &dir, 2, &mut nowhere, "core-6f2c".to_string()) {
                DaemonExit::IdleExit(removal) => {
                    match removal {
                        Removal::Removed => { }
                        _ => {
                            println!("the read-deadline exit did not report the removal");
                            assert!(false);
                        }
                    }
                }
                DaemonExit::ShutdownExit(_) => {
                    println!("the accept loop served the QUEUED interface: the SessionEnd::IdleTimeout arm of serve_daemon is a NO-OP");
                    assert!(false);
                }
                _ => {
                    println!("the daemon came down otherwise than IdleExit");
                    assert!(false);
                }
            }
            match read_lock(&dir) {
                LockReading::Absent => { }
                _ => {
                    println!("the lock file survived the read-deadline exit");
                    assert!(false);
                }
            }
            cleanup_scratch(&dir);
        }
    }
}

#[test]
fn the_accept_loop_idle_clock_starts_over_after_a_connection_is_served() {
    // ROUTE 3 — `waited = 0;` of [`serve_daemon`]. The clock counts
    // MILLISECONDS WAITED with NO connection in hand, and "WITHOUT
    // COMMANDS" RESTARTS that count once a connection HAS been served: an
    // interface arriving INSIDE the RESET window but OUTSIDE ONE
    // uninterrupted window from the START of the daemon is the OBSERVABLE
    // difference. With the window of 4 SECONDS: the FIRST interface shows up
    // at 2.5 SECONDS (the clock has counted 2500 of the 4000 ms) and ENDS at
    // end-of-input, so the reset build is STILL ALIVE at 5.5 SECONDS when the
    // SECOND interface sends `shutdown`; a clock that never RESETS has ALREADY
    // EXITED AT 4 SECONDS and the exit it reports is `IdleExit`. THE MARGINS
    // on both sides are 1 SECONDS against a 250 ms poll, so the test STAYS
    // RELIABLE under `RUST_TEST_THREADS=8`.
    let dir = scratch_dir("the_accept_loop_idle_clock_starts_over_after_a_connection_is_served".to_string());
    let port = free_port(57301);
    let nonce = "6f2c1a48e0b7d9c3".to_string();
    let record = LockRecord { address: endpoint_text("127.0.0.1".to_string(), port), nonce: nonce.to_string() };
    match write_lock(&dir, &record) {
        Err(_) => { assert!(false); }
        Ok(_) => { }
    }
    match TcpListener::bind(("127.0.0.1", port.try_into().unwrap_or(0))) {
        Err(_) => { assert!(false); }
        Ok(listener) => {
            let first = QuietPeerSpec { host: "127.0.0.1".to_string(), port: port, nonce: nonce.to_string(), wait_before_millis: 2500, sends_version_request: true, sends_shutdown: false, closes_write: true, closes_read: false, hold_millis: 0 };
            start_quiet_peer(first);
            let second = QuietPeerSpec { host: "127.0.0.1".to_string(), port: port, nonce: nonce.to_string(), wait_before_millis: 5500, sends_version_request: true, sends_shutdown: true, closes_write: true, closes_read: false, hold_millis: 0 };
            start_quiet_peer(second);
            let mut nowhere = interpret::wired_nowhere();
            match serve_daemon(&listener, &dir, 4, &mut nowhere, "core-6f2c".to_string()) {
                DaemonExit::ShutdownExit(removal) => {
                    match removal {
                        Removal::Removed => { }
                        _ => {
                            println!("the shutdown exit did not report the removal");
                            assert!(false);
                        }
                    }
                }
                DaemonExit::IdleExit(_) => {
                    println!("the daemon exited idle WHILE an interface was waiting INSIDE the reset window: the accept-loop clock never restarted");
                    assert!(false);
                }
                _ => {
                    println!("the daemon came down otherwise than ShutdownExit");
                    assert!(false);
                }
            }
            match read_lock(&dir) {
                LockReading::Absent => { }
                _ => {
                    println!("the lock file survived the shutdown exit");
                    assert!(false);
                }
            }
            cleanup_scratch(&dir);
        }
    }
}
