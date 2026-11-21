//! The CONFIG half of the daemon transport of plan item 3.6 — the module
//! the row of the plan puts under `core/src/technical/`: the names
//! `127.0.0.1:47623` (the DEFAULT of `core-protocol.md:14`), the
//! `phin.toml` READING that names the endpoint (`basic_toml` over the
//! DOTTED spelling of `data-files.md:149`), the `state/core.lock`
//! file (path, read, atomic write, removal), and the nonce the DAEMON
//! MINTS for its lock file.

//! Sub-row 3.6b of the USER's split of plan item 3.6 into 3.6a/3.6b/3.6c
//! (decision 38): this module holds the CONFIG/LOCK/ENDPOINT/NONCE half and
//! its own tests; the connection gate, the daemon side, the spawn seam and
//! `connect_or_spawn` live in the sibling `transport.rs` (sub-row 3.6c
//! owns rewiring the callers that name these types through `transport::`,
//! which is why `transport.rs` re-exports this module's public names —
//! see the block there, NOT this one).

//! # What the interface docs fix (the halves this module implements)

//! * **Address** (`core-protocol.md:14`): `core.address` and `core.port`
//!   in the TOML configuration, default `127.0.0.1:47623`; only the address
//!   is configurable, and THE CONFIG NAMES THE ENDPOINT — `endpoint_from_config`
//!   APPLIES the port parsed out of `core.address` (this was the production
//!   bug of cycle 3: the endpoint always fell back to 47623); a test pins
//!   the applied port.
//! * **Lock file** (`core-protocol.md:18-28`, `data-files.md:143`):
//!   `state/core.lock` inside the data directory, holding the address the
//!   daemon listens on plus its nonce, WRITTEN at daemon start-up and
//!   REMOVED on exit. Reading it answers with the typed [`LockReading`]:
//!   ABSENT (only `NotFound` reads as absent), UNREADABLE (the file EXISTS
//!   but `std::io` refused it — NEVER folded into ABSENT: `architecture.md:247`
//!   forbids the SECOND daemon an absent reading would start), MALFORMED
//!   (`proto` refused the text; the partial record survives) and READ.
//!   The TEXT itself is encoded and parsed by
//!   `proto/src/handshake.rs` (`render_lock`/`parse_lock`), which is
//!   `proto`'s half of this row; every WRITE of the file goes through
//!   plan 2.5's `core::technical::fs_atomic::write_atomic`.
//! * **The nonce** (`architecture.md:249`): the DAEMON writes the lock,
//!   the interface READS it — so the daemon MINTS its own nonce from its
//!   own identity with this package's `technical/id.rs` helpers
//!   (decision 33, which dropped `proto`'s digest-to-nonce route for
//!   implying an undocumented interface -> child channel; decision 37
//!   moved the minting onto `id::sha256_hex`). The 16-lowercase-hex-char
//!   SHAPE rule itself stays in `proto::handshake::is_wellformed_nonce`
//!   (a recorded invention matching the doc example), and the test
//!   `the_minted_nonce_has_the_shape_proto_validates` pins the two
//!   halves together.

//! # Recorded decisions of this half

//! * **The address precedence.** `core.address` may be written as the
//!   `host:port` text the lock file uses (`core-protocol.md:26`);
//!   `core.port` then OVERRIDES the port inside it, because two keys
//!   cannot both be "the address" of `core-protocol.md:14`.
//! * **A configuration is READ, never guessed.** An ABSENT `phin.toml`
//!   is not a complaint — the endpoint is the documented default;
//!   `basic_toml`'s parse failure, a file that EXISTS but CANNOT BE READ
//!   (`ConfigIssue.NotReadable`, "the OS's own message") and a key that
//!   is PRESENT with the wrong type are typed `ConfigIssue`s and NEVER
//!   fall back to a default endpoint.
//! * **The handshake-request contract (3.6a review finding 4;**
//!   `proto/src/handshake.rs:267-272`**):** `version_request_frame`
//!   SWALLOWS a codec refusal into `String::new`, so a builder of a
//!   handshake request MUST validate the nonce with
//!   `proto::handshake::is_wellformed_nonce` BEFORE asking for the frame
//!   and must treat the empty string as "no handshake request". THIS
//!   module builds NO frame — the only builder in this build is the
//!   sibling's `open_session`, which RE-CHECKS whatever a lock file
//!   handed it before writing ANYTHING; what THIS module guarantees is
//!   that the nonce IT MINTS passes that check BY CONSTRUCTION, which
//!   `the_minted_nonce_has_the_shape_proto_validates` pins.

//! # Deferred, and who owes it

//! * Everything that CONSUMES the endpoint — the connection gate, the
//!   daemon side, the spawn seam — is sub-row 3.6c in `transport.rs`.

use crate::technical::fs_atomic;
use crate::technical::id;
use proto::handshake::LockRecord;
use proto::handshake::LockIssue;
use proto::handshake::parse_lock;
use proto::handshake::render_lock;
use serde_json::Map;
use serde_json::Value;

/// The default host of the core address (`core-protocol.md:14`).
pub const DEFAULT_ADDRESS: &str = "127.0.0.1";

/// The default port of the core address (`core-protocol.md:14`). NEVER bound by
/// a test — planning decision 7 — so the tests carry a free port through the
/// configuration instead.
pub const DEFAULT_PORT: i64 = 47623;

/// The name of the lock file inside `<data dir>/state/` (`data-files.md:143`).
pub const LOCK_FILE_NAME: &str = "core.lock";

/// The state directory the lock file lives in (`data-files.md:139-143`).
pub const STATE_DIR_NAME: &str = "state";

/// The configuration file the endpoint is read from (`data-files.md:151`, a
/// DRAFT path, honoured by user decision 31=C).
pub const CONFIG_FILE_NAME: &str = "phin.toml";

/// The `core.address` key of the configuration (`data-files.md:146-152`).
pub const ADDRESS_KEY: &str = "core.address";

/// The `core.port` key of the configuration (`data-files.md:146-152`).
pub const PORT_KEY: &str = "core.port";

/// The `address` key inside the `core` table the parser builds for the dotted
/// key above.
pub const ADDRESS_TABLE_KEY: &str = "address";

/// The `port` key inside the `core` table.
pub const PORT_TABLE_KEY: &str = "port";

/// The length of the nonce of a daemon instance, in lowercase hex characters.
/// The SHAPE rule itself lives in `proto::handshake::is_wellformed_nonce` (it
/// is pinned there to the doc's example); this constant is the SAME LENGTH
/// used when the daemon MINTS one, and the test
/// `the_minted_nonce_has_the_shape_proto_validates` pins the two together.
pub const NONCE_HEX_CHARS: usize = 16;

/// The `state/core.lock` text of `core-protocol.md:18-28` as one address string
/// (`host:port`), the value the lock file names and the value an error about the
/// configured address names back.
pub fn endpoint_text(host: String, port: i64) -> String {
    return format!("{host}:{port}");
}

/// An endpoint: the host and port this build binds or connects to.
#[derive(Debug, Eq, PartialEq)]
pub struct Endpoint {
    pub host: String,
    pub port: i64
}

/// Why a host/port pair is not a usable endpoint. The port is read as an
/// [`i64`] from TOML and validated rather than narrowed by a cast, which the
/// style of this repository forbids entirely.
#[derive(Debug, Eq, PartialEq)]
pub enum EndpointIssue {
    /// The host text is empty.
    EmptyHost,
    /// The port text holds no digits, or nothing at all.
    PortNotANumber(String),
    /// The port is outside the TCP range a listener can bind.
    PortOutOfRange(i64)
}

/// Check a host/port pair.
pub fn parse_endpoint(host: String, port: i64) -> (Option::<Endpoint>, Vec::<EndpointIssue>) {
    let mut issues = Vec::<EndpointIssue>::new();
    if host.len() == 0 {
        issues.push(EndpointIssue::EmptyHost);
    }
    if port < 0 || port > 65_535 {
        issues.push(EndpointIssue::PortOutOfRange(port));
    }
    if issues.len() > 0 {
        return (None, issues);
    }
    return (Some(Endpoint { host: host.to_string(), port: port }), issues);
}

/// Read the `host:port` text of a configured `core.address`.
///
/// `core-protocol.md:14` names ONE address while `data-files.md:149` names TWO
/// keys, and the lock file of `core-protocol.md:26` writes the address as
/// `host:port`; so the address text may carry the port, and an explicit
/// `core.port` overrides it. RECORDED precedence decision.
pub fn read_address(text: &str) -> (String, Option::<i64>, Vec::<EndpointIssue>) {
    let mut issues = Vec::<EndpointIssue>::new();
    match text.find(":") {
        None => {
            let host = text.trim().to_string();
            if host.len() == 0 {
                issues.push(EndpointIssue::EmptyHost);
                return (host, None, issues);
            }
            return (host, None, issues);
        }
        Some(index) => {
            let host = text[..index].trim().to_string();
            let port_text = text[index + 1 ..].trim().to_string();
            if host.len() == 0 {
                issues.push(EndpointIssue::EmptyHost);
            }
            match port_text.parse::<i64>() {
                Err(_) => {
                    issues.push(EndpointIssue::PortNotANumber(port_text));
                    return (host, None, issues);
                }
                Ok(port) => {
                    if port < 0 || port > 65_535 {
                        issues.push(EndpointIssue::PortOutOfRange(port));
                        return (host, None, issues);
                    }
                    if host.len() == 0 {
                        return (host, None, issues);
                    }
                    return (host, Some(port), issues);
                }
            }
        }
    }
}


// ----------------------------------------------------------------------------
// The configuration: where the endpoint comes from
// ----------------------------------------------------------------------------

/// Why the endpoint could not be taken from the configuration.
#[derive(Debug, Eq, PartialEq, Clone)]
pub enum ConfigIssue {
    /// The configuration file exists but could not be read; the text is the
    //  OS's own message.
    NotReadable(String),
    /// The configuration file is not TOML; the text is `basic_toml`'s own
    //  message plus its `line_col()` pair, destructured rather than assumed.
    NotTOML(String),
    /// The `core` table is not a table, `core.address` is not text, or
    //  `core.port` is not an integer: a key that is PRESENT with the wrong type
    //  must not read as absent and fall back to the documented default.
    WrongType(String),
    /// `core.port` is outside the range a listener can bind.
    PortOutOfRange(i64)
}

/// The text a configuration key holds, [`None`] when the key is absent; a key
/// present with the wrong JSON type pushes [`ConfigIssue.WrongType`] naming
/// `complaint` onto the caller's list.
fn text_of(fields: &Map::<String, Value>, key: &str, complaint: &str, issues: &mut Vec::<ConfigIssue>) -> Option::<String> {
    match fields.get(key) {
        None => { return None; }
        Some(held) => {
            match held.as_str() {
                None => {
                    issues.push(ConfigIssue::WrongType(complaint.to_string()));
                    return None;
                }
                Some(value) => { return Some(value.to_string()); }
            }
        }
    }
}

/// The integer a configuration key holds, [`None`] when the key is absent; a
/// present-but-wrong type pushes [`ConfigIssue.WrongType`] and an unbindable
/// port pushes [`ConfigIssue.PortOutOfRange`] onto the caller's list.
fn int_of(fields: &Map::<String, Value>, key: &str, complaint: &str, issues: &mut Vec::<ConfigIssue>) -> Option::<i64> {
    match fields.get(key) {
        None => { return None; }
        Some(held) => {
            match held.as_i64() {
                None => {
                    issues.push(ConfigIssue::WrongType(complaint.to_string()));
                    return None;
                }
                Some(value) => {
                    if value < 0 || value > 65_535 {
                        issues.push(ConfigIssue::PortOutOfRange(value));
                        return None;
                    }
                    return Some(value);
                }
            }
        }
    }
}

/// Read `core.address`/`core.port` out of `<data dir>/phin.toml`.
///
/// * A file that is ABSENT is not a complaint: the endpoint is the documented
///   default of `core-protocol.md:14`. Every test of this module WRITES a
///   configuration naming a free port, so "the endpoint comes from the
///   configuration" is exercised rather than defaulted (planning decision 7:
///   the documented default port is never bound by a test).
/// * `basic_toml::from_str<serde_json::Value>` reads the DOTTED spelling of
///   `data-files.md:149` (`core.address`) into a `core` TABLE, so ONE lookup
///   path serves both spellings, and `serde_json::Value` is already a dependency
///   of this package — which keeps the values in the same typed JSON the codec
///   of plan 3.1 reads. THIS is the production use of the `basic-toml`
///   dependency, which cycle 1's reviewer found unused and warned about.
/// * A file that EXISTS but CANNOT BE READ is [`ConfigIssue.NotReadable`]
///   ("the configuration file exists but could not be read; the text is the
///   OS's own message" — that variant's own doc, WHICH this reading now
///   produces); only `NotFound` defaults. An unreadable configuration is
///   NEVER guessed back into a default endpoint — the caller gets the typed
///   complaint list and no endpoint.
pub fn endpoint_from_config(data_dir: &std::path::Path) -> (Option::<Endpoint>, Vec::<ConfigIssue>) {
    let path = data_dir.join(CONFIG_FILE_NAME.to_string());
    match std::fs::read_to_string(path) {
        Err(reason) => {
            // ONLY "there is NO such file" defaults
            // (`core-protocol.md:14`); anything else the OS refused is a
            // typed [`ConfigIssue.NotReadable`] naming ITS OWN message,
            // never a guess.
            if reason.kind() == std::io::ErrorKind::NotFound {
                return (Some(Endpoint { host: DEFAULT_ADDRESS.to_string(), port: DEFAULT_PORT }), Vec::<ConfigIssue>::new());
            }
            let message = format!("{reason}");
            return (None, [ConfigIssue::NotReadable(message)].to_vec());
        }
        Ok(text) => {
            match basic_toml::from_str::<Value>(&text) {
                Err(reason) => {
                    let mut where_text = String::new();
                    match reason.line_col() {
                        None => { }
                        Some((line, column)) => { where_text = format!(" at line {line} column {column}"); }
                    }
                    let message = format!("{reason}");
                    let mut not_toml = Vec::<ConfigIssue>::new();
                    not_toml.push(ConfigIssue::NotTOML(format!("{message}{where_text}")));
                    return (None, not_toml);
                }
                Ok(held) => {
                    let mut issues = Vec::<ConfigIssue>::new();
                    match held.as_object() {
                        None => {
                            issues.push(ConfigIssue::WrongType(CONFIG_FILE_NAME.to_string()));
                            return (None, issues);
                        }
                        Some(root) => {
                            match root.get("core") {
                                None => { return (Some(Endpoint { host: DEFAULT_ADDRESS.to_string(), port: DEFAULT_PORT }), Vec::<ConfigIssue>::new()); }
                                Some(held_section) => {
                                    match held_section.as_object() {
                                        None => {
                                            issues.push(ConfigIssue::WrongType("core".to_string()));
                                            return (None, issues);
                                        }
                                        Some(section) => {
                                            let mut host = DEFAULT_ADDRESS.to_string();
                                            let mut port = DEFAULT_PORT;
                                            // `core.address` may carry the port
                                            // (`core-protocol.md:26` writes the address
                                            // as `host:port`); an explicit `core.port`
                                            // then OVERRIDES it, because `core-protocol.md:14`
                                            // names ONE address while `data-files.md:149`
                                            // names TWO keys.
                                            match text_of(section, ADDRESS_TABLE_KEY, ADDRESS_KEY, &mut issues) {
                                                None => { }
                                                Some(address_text) => {
                                                    // `core.address` may carry the port
                                                    // (`core-protocol.md:26` writes the address
                                                    // as `host:port`); an explicit `core.port`
                                                    // below OVERRIDES it.
                                                    let parsed_address = read_address(&address_text);
                                                    host = parsed_address.0;
                                                    match parsed_address.1 {
                                                        Some(held_port) => { port = held_port; }
                                                        None => { }
                                                    }
                                                    for issue in &parsed_address.2 {
                                                        match issue {
                                                            // The purpose-built `ConfigIssue::PortOutOfRange`
                                                            // carries the PORT itself and is what
                                                            // `core/src/main.rs` already renders
                                                            // (finding of the review of plan item 3.6b:
                                                            // mapping it to `WrongType` bypassed
                                                            // that rendering); the OTHER endpoint
                                                            // complaints keep the typed `WrongType`
            // naming the KEY, which is the honest reading of a TEXT key
            // holding a bad port.
                                                            EndpointIssue::PortOutOfRange(value) => { issues.push(ConfigIssue::PortOutOfRange(*value)); }
                                                            _ => { issues.push(ConfigIssue::WrongType(endpoint_complaint(issue))); }
                                                        }
                                                    }
                                                }
                                            }
                                            match int_of(section, PORT_TABLE_KEY, PORT_KEY, &mut issues) {
                                                None => { }
                                                Some(value) => { port = value; }
                                            }
                                            if issues.len() > 0 {
                                                return (None, issues);
                                            }
                                            return (Some(Endpoint { host: host, port: port }), Vec::<ConfigIssue>::new());
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
}

/// The text of an endpoint complaint, for a complaint about the configuration
/// key that produced it, OR — since sub-row 3.6c WIRES `parse_endpoint` INTO
/// THE CONNECTION GATE — about a hand-built `Endpoint` the gate refuses BEFORE
/// ANY SOCKET IS OPENED (why this is `pub`).
pub fn endpoint_complaint(issue: &EndpointIssue) -> String {
    match issue {
        EndpointIssue::EmptyHost => { return "core.address has an empty host".to_string(); }
        EndpointIssue::PortNotANumber(text) => { return format!("core.address has a port that is not a number: [{text}]"); }
        EndpointIssue::PortOutOfRange(port) => { return format!("core.address carries a port that cannot be bound: {port}"); }
    }
}

// ---------------------------------------------------------------------------
// The lock file
// ---------------------------------------------------------------------------

/// The path of `state/core.lock` inside the data directory (`data-files.md:143`).
pub fn lock_path(data_dir: &std::path::Path) -> std::path::PathBuf {
    return data_dir.join(STATE_DIR_NAME).join(LOCK_FILE_NAME).to_path_buf();
}

/// What reading the lock file came down to — THREE DISTINCT readings the
/// OLD tuple return merged into one `(None, [])`, which contradicted this
/// function's own doc and made an EXISTING-but-UNREADABLE lock read as
/// ABSENT, i.e. "start a SECOND daemon", which `architecture.md:247`
/// forbids ("If a lock file exists, the interface connects and sends the
/// handshake"; the review of plan item 3.6b recorded exactly this). The
/// CALLER (`transport.rs`'s `connect_or_spawn`) decides from the difference:
/// ABSENT may spawn, UNREADABLE must NOT (the refusal is typed there and
/// names the CONFIGURED address), and MALFORMED falls to the SPAWN path of
/// a STALE lock (clause (c): a lock whose text gives no usable nonce hand-
/// shaked nothing, the handshake FAILED, and the daemon this flow starts
/// OVERWRITES the file).
#[derive(Debug, Eq, PartialEq)]
pub enum LockReading {
    /// The file was read and `proto`'s parser accepted it WHOLE: the record
    //  (address + wellformed nonce) is exactly what `core-protocol.md:26`
    //  describes.
    Read(LockRecord),
    /// There is NO `state/core.lock` file (`std::io`'s `NotFound`) — the
    //  only reading `architecture.md:247` answers with "start a daemon".
    Absent,
    /// The file EXISTS but this build could not READ it (permissions, the
    //  path going through a NON-DIRECTORY, the file being a DIRECTORY);
    //  the texts are the `std::io` kind and the OS's OWN message. The
    //  daemon that wrote it is NOT known to be gone — process-id liveness
    //  is never the test (`architecture.md:247`) and this module never
    //  inspects a pid — so NOTHING here may start a SECOND daemon.
    Unreadable(std::io::ErrorKind, String),
    /// The text was READ but `proto`'s parser refused it (a missing or
    //  malformed VALUE, an unknown key, a repeated key): the issues are
    //  `handshake.rs:38`'s complaints, and the optional partial record is
    //  what the GOOD lines carried — `transport.rs` handshakes through it
    //  when it holds a wellformed nonce, and otherwise takes the SPAWN
    //  path of a stale lock.
    Malformed(Option::<LockRecord>, Vec::<LockIssue>)
}

pub fn read_lock(data_dir: &std::path::Path) -> LockReading {
    match std::fs::read_to_string(&lock_path(data_dir)) {
        Err(reason) => {
            if reason.kind() == std::io::ErrorKind::NotFound {
                return LockReading::Absent;
            }
            let message = format!("{reason}", reason = reason);
            return LockReading::Unreadable(reason.kind(), message);
        }
        Ok(text) => {
            match parse_lock(&text) {
                (None, issues) => { return LockReading::Malformed(None, issues); }
                (Some(record), issues) => {
                    if issues.len() == 0 {
                        return LockReading::Read(record);
                    }
                    return LockReading::Malformed(Some(record), issues);
                }
            }
        }
    }
}

/// Write the lock file, atomically through plan 2.5's helper of `fs_atomic.rs`
/// (temporary file plus rename), with the text of `proto`'s lock writer.
pub fn write_lock(data_dir: &std::path::Path, record: &LockRecord) -> Result::<fs_atomic::BackupOutcome, fs_atomic::WriteError> {
    return fs_atomic::write_atomic(&lock_path(data_dir), &render_lock(record));
}

/// What removing the lock file came down to.
///
/// The DAEMON takes this path on exit (`core-protocol.md:199`,
/// `data-files.md:143`); an interface that REUSED a running daemon never does
/// (`architecture.md:248`), which is why nothing on the interface side of this
/// module removes a lock. `std::fs::remove_file` is used directly because
/// plan 2.5's `fs_atomic.rs` has no removal helper.
#[derive(Debug, Eq, PartialEq)]
pub enum Removal {
    Removed,
    WasAbsent,
    Failed(std::io::ErrorKind, String)
}

pub fn remove_lock(data_dir: &std::path::Path) -> Removal {
    match std::fs::remove_file(&lock_path(data_dir)) {
        Ok(_) => { return Removal::Removed; }
        Err(reason) => {
            if reason.kind() == std::io::ErrorKind::NotFound {
                return Removal::WasAbsent;
            }
            let message = format!("{reason}");
            return Removal::Failed(reason.kind(), message);
        }
    }
}

/// The nonce of a fresh daemon instance, minted by the DAEMON from its own
/// identity — its process id, the endpoint it was told to bind, and the path of
/// the lock file it writes (`architecture.md:249` puts the writing side in the
/// daemon, so the interface never has to know a nonce before the daemon exists:
/// decision 33, which dropped `proto`'s digest-to-nonce route for implying an
/// undocumented interface -> child channel).
///
/// The value is the first [`NONCE_HEX_CHARS`] lowercase hex characters of this
/// package's SHA-256 reading of the seed, i.e. the same reading `technical/id.rs`
/// uses for transaction ids; the 16-character SHAPE rule itself stays in
/// `proto::handshake::is_wellformed_nonce`, where the doc example pins it.
///
/// THE CONTRACT THIS MINT SERVES (3.6a review finding 4, stated in this
/// module's doc): whoever builds a handshake request with
/// `proto::handshake::version_request_frame` MUST validate the nonce with
/// `is_wellformed_nonce` FIRST — and a nonce THIS module minted passes that
/// check BY CONSTRUCTION, which `the_minted_nonce_has_the_shape_proto_validates`
/// pins; the sibling module's `open_session` re-checks whatever a lock file
/// handed IT before writing any frame.
pub fn mint_nonce(seed: &str) -> String {
    return id::first_chars(&id::sha256_hex(&seed), NONCE_HEX_CHARS);
}


// ============================================================================
// Tests
// ============================================================================

/// A CWD-relative scratch directory under `test-scratch-transport-cfg/<name>`,
/// REMOVED FIRST if left behind by a FAILED run, CREATED for this run WITH A
/// BOUNDED RETRY (the race documented by the review of plan item 3.6b: a
/// SIBLING test's cleanup removes the shared scratch PARENT while this test is
/// INSIDE `create_dir_all`; AT MOST FIVE creation attempts bound the loop), and
/// REMOVED again at the end by [`cleanup_scratch`] — which also drops the
/// scratch parent of THIS MODULE ONLY (`test-scratch-transport-cfg`, NEVER the
/// the `test-scratch-fs-atomic`/`test-scratch-view` parents: this module owns its
/// OWN scratch parent), so after a test run `find . -name 'test-scratch*'`
/// reads EMPTY. Every test of this module uses a name of its OWN — `cargo test`
/// runs the tests of one binary on PARALLEL threads, so DISTINCT directories
/// (and DISTINCT FREE PORTS, see [`free_port`]) make the tests unable to collide.
#[cfg(test)]
fn scratch_dir(name: String) -> std::path::PathBuf {
    let dir = std::path::PathBuf::from("test-scratch-transport-cfg").join(name).to_path_buf();
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
    match std::fs::remove_dir_all(dir) {
        Err(_) => { }
        Ok(_) => { }
    }
    // The EMPTY parent goes too, best effort: a sibling test still RUNNING
    // keeps the directory non-empty and this removal is simply REFUSED.
    match std::fs::remove_dir(std::path::PathBuf::from("test-scratch-transport-cfg")) {
        Err(_) => { }
        Ok(_) => { }
    }
}

/// A port NO listener holds — the SAME probe route as `free_port` of
/// `transport.rs` (see its module for the toolchain facts: no port
/// accessor on a bound listener). The BASES of THIS module (401xx) never
/// touch the documented default 47623 (planning decision 7) NOR the bases
/// of `transport.rs` (402xx and up), so the two modules' parallel test
/// threads cannot collide on ONE port.
#[cfg(test)]
fn free_port(base: i64) -> i64 {
    let mut candidate = base;
    while candidate < base + 50 {
        let short: u16 = candidate.try_into().unwrap_or(0);
        match std::net::TcpListener::bind(("127.0.0.1", short)) {
            Err(_) => { }
            Ok(_) => { return candidate; }
        }
        candidate += 1;
    }
    return base;
}

#[test]
fn the_minted_nonce_has_the_shape_proto_validates() {
    // The TWO halves of the nonce decision (33/37): core MINTS via its own
    // `id::sha256_hex`, proto VALIDATES the 16-lowercase-hex-char SHAPE —
    // this pins the two together, AND pins the mint to the SAME reading
    // `technical/id.rs` uses for transaction ids: the first
    // [`NONCE_HEX_CHARS`] characters of `id::sha256_hex(seed)`, which is
    // what makes the handshake-request contract of the module doc
    // (`is_wellformed_nonce` BEFORE `version_request_frame`) satisfiable
    // BY CONSTRUCTION for every nonce this module minted.
    let seed = "the seed of this test".to_string();
    let minted = mint_nonce(&seed);
    assert!(minted.len() == NONCE_HEX_CHARS);
    assert!(proto::handshake::is_wellformed_nonce(&minted));
    let hex = id::sha256_hex(&seed);
    assert!(minted == id::first_chars(&hex, NONCE_HEX_CHARS));
    // LOWERCASE HEX: the same digit range `is_wellformed_nonce` fixes (the
    // gap between `9` and `a` holds no hex digit).
    for character in minted.chars() {
        assert!(character >= '0' && character <= '9' || character >= 'a' && character <= 'f');
    }
}

#[test]
fn the_minted_nonce_is_stable_for_a_seed_and_different_across_seeds() {
    // The mint is a function of the seed ONLY: the daemon identity
    // `transport.rs`'s `daemon_nonce_seed` builds (pid, endpoint, lock path)
    // mints ONE nonce — stable — and DISTINCT identities mint DISTINCT
    // ones, which is what the nonce of `core-protocol.md:27` ("the
    // interface compares the nonce ... a DIFFERENT nonce is a different
    // daemon instance") needs across two daemon generations. BOTH keep
    // `proto`'s shape.
    let a = mint_nonce(&"1234:127.0.0.1:40101:/tmp/x/state/core.lock".to_string());
    let b = mint_nonce(&"1234:127.0.0.1:40101:/tmp/x/state/core.lock".to_string());
    let c = mint_nonce(&"1235:127.0.0.1:40101:/tmp/x/state/core.lock".to_string());
    assert!(a == b);
    assert!(a != c);
    assert!(proto::handshake::is_wellformed_nonce(&a));
    assert!(proto::handshake::is_wellformed_nonce(&c));
}

#[test]
fn the_configuration_names_the_endpoint_and_a_core_port_key_overrides_it() {
    // `core-protocol.md:14` names ONE address while `data-files.md:149`
    // names TWO keys: `core.address` carries `host:port` (the spelling of
    // the lock file, `core-protocol.md:26`), and `core.port` then OVERRIDES
    // the port inside it — the RECORDED precedence decision of this module.
    // The FIRST assertion is the cycle 3 production bug pinned: the port
    // PARSED OUT of `core.address` is APPLIED, not dropped on the floor
    // while the endpoint falls back to the documented 47623.
    let dir = scratch_dir("the_configuration_names_the_endpoint_and_a_core_port_key_overrides_it".to_string());
    let port = free_port(40101);
    match std::fs::write(&dir.join(CONFIG_FILE_NAME.to_string()), format!("core.address = \"127.0.0.1:{at}\"\n", at = port.to_string())) {
        Err(_) => { assert!(false); }
        Ok(_) => { }
    }
    match endpoint_from_config(&dir) {
        (Some(endpoint), issues) => {
            assert!(issues.len() == 0);
            assert!(endpoint.port == port);
            assert!(endpoint.host == DEFAULT_ADDRESS);
        }
        (None, issues) => {
            for held in issues {
                match held {
                    ConfigIssue::NotTOML(text) => { assert!(text.len() > 0); }
                    _ => { }
                }
            }
            assert!(false);
        }
    }
    match std::fs::write(&dir.join(CONFIG_FILE_NAME.to_string()), format!("core.address = \"127.0.0.1:{at}\"\ncore.port = {other}\n", at = port.to_string(), other = (port + 1).to_string())) {
        Err(_) => { assert!(false); }
        Ok(_) => { }
    }
    match endpoint_from_config(&dir) {
        (Some(endpoint), issues) => {
            assert!(issues.len() == 0);
            assert!(endpoint.port == port + 1);
        }
        (None, _) => { assert!(false); }
    }
    cleanup_scratch(&dir);
}

#[test]
fn a_configuration_that_is_not_toml_refuses_the_endpoint_rather_than_defaulting() {
    // A file that IS THERE and CANNOT BE PARSED must not read as absent and
    // fall back to the documented default (`ConfigIssue`'s own doc): the
    // endpoint comes back [`None`] with `basic_toml`'s typed complaint.
    let dir = scratch_dir("a_configuration_that_is_not_toml_refuses_the_endpoint_rather_than_defaulting".to_string());
    match std::fs::write(&dir.join(CONFIG_FILE_NAME.to_string()), "core.address = [\n".to_string()) {
        Err(_) => { assert!(false); }
        Ok(_) => { }
    }
    let held = endpoint_from_config(&dir);
    assert!(held.0 == None);
    assert!(held.1.len() > 0);
    match &held.1[0] {
        ConfigIssue::NotTOML(text) => { assert!(text.len() > 0); }
        _ => { assert!(false); }
    }
    cleanup_scratch(&dir);
}

#[test]
fn only_an_absent_configuration_defaults_and_an_unreadable_one_is_a_typed_complaint() {
    // The two READINGS of a missing/unreadable file: ABSENT -> the
    // documented default of `core-protocol.md:14` with NO complaint;
    // PRESENT but NOT READABLE -> [`ConfigIssue.NotReadable`] carrying the
    // OS's OWN message (that variant's doc, which this build now PRODUCES)
    // and NEVER a guessed endpoint. The unreadable form used here is a
    // `phin.toml` that is a DIRECTORY: `read_to_string` then fails with an
    // OS error that is not `NotFound`.
    let dir = scratch_dir("only_an_absent_configuration_defaults_and_an_unreadable_one_is_a_typed_complaint".to_string());
    match endpoint_from_config(&dir) {
        (Some(endpoint), issues) => {
            assert!(issues.len() == 0);
            assert!(endpoint.host == DEFAULT_ADDRESS);
            assert!(endpoint.port == DEFAULT_PORT);
        }
        (None, _) => { assert!(false); }
    }
    match std::fs::create_dir(&dir.join(CONFIG_FILE_NAME.to_string())) {
        Err(_) => { assert!(false); }
        Ok(_) => { }
    }
    let held = endpoint_from_config(&dir);
    assert!(held.0 == None);
    assert!(held.1.len() == 1);
    match &held.1[0] {
        ConfigIssue::NotReadable(text) => { assert!(text.len() > 0); }
        _ => { assert!(false); }
    }
    cleanup_scratch(&dir);
}

#[test]
fn a_configuration_key_holding_the_wrong_type_is_a_typed_issue_never_a_default() {
    // `ConfigIssue.WrongType`'s rule: a key PRESENT with the wrong type
    // must not read as absent. `core.address = 42` (an INTEGER where TEXT
    // is named) and `core.port = "47623"` (TEXT where an INTEGER is named)
    // both refuse the endpoint with the KEY NAMED, never a default.
    let dir = scratch_dir("a_configuration_key_holding_the_wrong_type_is_a_typed_issue_never_a_default".to_string());
    match std::fs::write(&dir.join(CONFIG_FILE_NAME.to_string()), "core.address = 42\n".to_string()) {
        Err(_) => { assert!(false); }
        Ok(_) => { }
    }
    let held = endpoint_from_config(&dir);
    assert!(held.0 == None);
    assert!(held.1.len() == 1);
    match &held.1[0] {
        ConfigIssue::WrongType(text) => { assert!(text == &ADDRESS_KEY.to_string()); }
        _ => { assert!(false); }
    }
    match std::fs::write(&dir.join(CONFIG_FILE_NAME.to_string()), "core.port = \"47623\"\n".to_string()) {
        Err(_) => { assert!(false); }
        Ok(_) => { }
    }
    let held2 = endpoint_from_config(&dir);
    assert!(held2.0 == None);
    assert!(held2.1.len() == 1);
    match &held2.1[0] {
        ConfigIssue::WrongType(text) => { assert!(text == &PORT_KEY.to_string()); }
        _ => { assert!(false); }
    }
    cleanup_scratch(&dir);
}

#[test]
fn the_lock_file_lands_at_state_core_lock_in_the_data_dir_and_round_trips_through_the_atomic_writer() {
    // `data-files.md:143`: `state/core.lock` INSIDE the data directory —
    // [`lock_path`]'s composition IS the assertion — and EVERY write goes
    // through plan 2.5's [`fs_atomic.write_atomic`] (temporary file plus
    // rename): a SECOND write's text round-trips through [`read_lock`],
    // the outcome says [`fs_atomic.BackupOutcome.PreviousKept`], and the
    // PREVIOUS text sits at the `<lock>.bak` sibling `fs_atomic` promises
    // — which is what PROVES the route was the atomic writer, not
    // `std::fs::write`.
    let dir = scratch_dir("the_lock_file_lands_at_state_core_lock_in_the_data_dir_and_round_trips_through_the_atomic_writer".to_string());
    let port = free_port(40151);
    let record = LockRecord { address: endpoint_text(DEFAULT_ADDRESS.to_string(), port), nonce: "6f2c1a48e0b7d9c3".to_string() };
    match write_lock(&dir, &record) {
        Err(_) => { assert!(false); }
        Ok(outcome) => { assert!(outcome == fs_atomic::BackupOutcome::NothingToKeep); }
    }
    let expected = dir.join(STATE_DIR_NAME.to_string()).join(LOCK_FILE_NAME.to_string()).to_path_buf();
    assert!(lock_path(&dir) == expected);
    match std::fs::exists(&expected) {
        Ok(true) => { }
        _ => { assert!(false); }
    }
    // The RAW TEXT of the file is `proto`'s rendering of the record:
    match std::fs::read_to_string(&expected) {
        Err(_) => { assert!(false); }
        Ok(text) => { assert!(text == render_lock(&record)); }
    }
    // And [`read_lock`] hands the SAME record back:
    match read_lock(&dir) {
        LockReading::Read(held) => {
            assert!(held.address == record.address);
            assert!(held.nonce == record.nonce);
        }
        _ => { assert!(false); }
    }
    // The SECOND write of a fresh nonce keeps the PREVIOUS text as the
    // `.bak` sibling and the file NOW holds the NEW text:
    let fresh = LockRecord { address: record.address.to_string(), nonce: "bbbbbbbbbbbbbbbb".to_string() };
    match write_lock(&dir, &fresh) {
        Err(_) => { assert!(false); }
        Ok(outcome) => { assert!(outcome == fs_atomic::BackupOutcome::PreviousKept); }
    }
    match read_lock(&dir) {
        LockReading::Read(now) => { assert!(now.nonce == fresh.nonce); }
        _ => { assert!(false); }
    }
    let bak = std::path::PathBuf::from(lock_path(&dir).display().to_string() + ".bak");
    match std::fs::exists(&bak) {
        Ok(true) => { }
        _ => { assert!(false); }
    }
    match std::fs::read_to_string(&bak) {
        Err(_) => { assert!(false); }
        Ok(text) => { assert!(text == render_lock(&record)); }
    }
    cleanup_scratch(&dir);
}

#[test]
fn lock_removal_reports_absent_then_present_then_failed() {
    // [`Removal`]'s three readings IN ONE test. ABSENT -> [`WasAbsent`]
    // (`NotFound` IS how absent reads, and an interface REUSING a daemon
    // never removes anything — see the enum's doc). PRESENT ->
    // [`Removed`] and the file is GONE. FAILED -> the OS's kind AND text:
    // here `<data dir>/state` is a FILE, so `state/core.lock` is a path
    // through a NON-DIRECTORY and the OS refuses with
    // `NotADirectory` — NOT `NotFound`, which is exactly why the reading
    // is `Failed` and never a false "was absent".
    let dir = scratch_dir("lock_removal_reports_absent_then_present_then_failed".to_string());
    match remove_lock(&dir) {
        Removal::WasAbsent => { }
        _ => { assert!(false); }
    }
    let record = LockRecord { address: endpoint_text(DEFAULT_ADDRESS.to_string(), 40161), nonce: "6f2c1a48e0b7d9c3".to_string() };
    match write_lock(&dir, &record) {
        Err(_) => { assert!(false); }
        Ok(_) => { }
    }
    match remove_lock(&dir) {
        Removal::Removed => { }
        _ => { assert!(false); }
    }
    assert!(remove_lock(&dir) == Removal::WasAbsent);
    // The `state` DIRECTORY the writer created is still THERE (empty):
    // replacing it with a FILE makes `state/core.lock` a path THROUGH a
    // NON-DIRECTORY, which the OS refuses with `NotADirectory`:
    match std::fs::remove_dir(&dir.join(STATE_DIR_NAME.to_string())) {
        Err(_) => { assert!(false); }
        Ok(_) => { }
    }
    match std::fs::write(&dir.join(STATE_DIR_NAME.to_string()), "not a directory".to_string()) {
        Err(_) => { assert!(false); }
        Ok(_) => { }
    }
    match remove_lock(&dir) {
        Removal::Failed(kind, text) => {
            assert!(kind == std::io::ErrorKind::NotADirectory);
            assert!(text.len() > 0);
        }
        _ => { assert!(false); }
    }
    cleanup_scratch(&dir);
}


#[test]
fn a_core_table_configuration_names_the_same_endpoint_as_the_dotted_key_and_a_non_default_host_is_applied() {
    // TWO FINDINGS OF THE REVIEW OF PLAN ITEM 3.6b PINNED AT ONCE:
    // * the `[core]` TABLE spelling of the configuration had NO test at
    //   all (`grep -rn '"\[core\]' core/` was EMPTY then) although the
    //   sibling gate must accept whatever spelling the USER's config file
    //   uses — `basic_toml` folds BOTH the DOTTED `core.address` of
    //   `data-files.md:149` AND the `[core]` TABLE with `address`/`port`
    //   INTO the SAME nested table, and this pins ONE lookup path serves
    //   BOTH spellings, TABLE + `port` override included;
    // * the configured HOST was UNPINNED: every config test of 3.6b wrote
    //   `127.0.0.1`, EQUAL to the DEFAULT, so the mutant "discard the
    //   parsed host, ALWAYS DEFAULT_ADDRESS" SURVIVED all 166 tests. The
    //   HOST here is `localhost` (which resolves to the loopback address
    //   the FRAMING of `core-protocol.md:12` demands WHEN this endpoint is
    //   later bound, but NOTHING binds it in this test — the endpoint's
    //   host text IS the assertion), and the DOTTED spelling asserts the
    //   host too, so a dropped host now fails HERE.
    let dir = scratch_dir("a_core_table_configuration_names_the_same_endpoint_as_the_dotted_key_and_a_non_default_host_is_APPLIED".to_string());
    let port = free_port(40121);
    match std::fs::write(&dir.join(CONFIG_FILE_NAME.to_string()), format!("[core]\naddress = \"localhost:{at}\"\n", at = port.to_string())) {
        Err(_) => { assert!(false); }
        Ok(_) => { }
    }
    match endpoint_from_config(&dir) {
        (Some(endpoint), issues) => {
            assert!(issues.len() == 0);
            assert!(endpoint.host == "localhost".to_string());
            assert!(endpoint.port == port);
        }
        (None, issues) => {
            for held in issues {
                match held {
                    ConfigIssue::NotTOML(text) => { assert!(text.len() > 0); }
                    _ => { }
                }
            }
            assert!(false);
        }
    }
    // THE TABLE + explicit `port` OVERRIDE (the recorded precedence):
    match std::fs::write(&dir.join(CONFIG_FILE_NAME.to_string()), format!("[core]\naddress = \"localhost:{at}\"\nport = {other}\n", at = port.to_string(), other = (port + 1).to_string())) {
        Err(_) => { assert!(false); }
        Ok(_) => { }
    }
    match endpoint_from_config(&dir) {
        (Some(endpoint), issues) => {
            assert!(issues.len() == 0);
            assert!(endpoint.port == port + 1);
            assert!(endpoint.host == "localhost".to_string());
        }
        (None, _) => { assert!(false); }
    }
    // THE DOTTED spelling, NON-DEFAULT HOST too — SAME nested table,
    // SAME result, which is what "ONE lookup path serves both spellings"
    // means observable:
    match std::fs::write(&dir.join(CONFIG_FILE_NAME.to_string()), format!("core.address = \"localhost:{at}\"\n", at = port.to_string())) {
        Err(_) => { assert!(false); }
        Ok(_) => { }
    }
    match endpoint_from_config(&dir) {
        (Some(endpoint), issues) => {
            assert!(issues.len() == 0);
            assert!(endpoint.host == "localhost".to_string());
            assert!(endpoint.port == port);
        }
        (None, _) => { assert!(false); }
    }
    cleanup_scratch(&dir);
}

#[test]
fn an_unbindable_configuration_port_is_the_typed_port_issue_carrying_the_port_not_a_wrong_type() {
    // THE FINDING OF THE REVIEW OF PLAN ITEM 3.6b: port-RANGE refusals were
    // mapped to `ConfigIssue::WrongType`, BYPASSING the purpose-built
    // `ConfigIssue::PortOutOfRange(i64)` variant that `core/src/main.rs`
    // ALREADY renders ("the configured port {value} cannot be bound").
    // BOTH spellings of an unbindable port — the port INSIDE `core.address`
    // and the KEY `core.port` — come back as `PortOutOfRange` carrying
    // THE PORT ITSELF; the wrong-TYPE reading keeps naming the KEY
    // (asserted below on the OTHER edge, a non-numeric port text).
    let dir = scratch_dir("an_unbindable_configuration_port_is_the_typed_port_issue_carrying_the_port_not_a_wrong_type".to_string());
    match std::fs::write(&dir.join(CONFIG_FILE_NAME.to_string()), "core.address = \"127.0.0.1:70000\"\n".to_string()) {
        Err(_) => { assert!(false); }
        Ok(_) => { }
    }
    match endpoint_from_config(&dir) {
        (Some(_), _) => { assert!(false); }
        (None, issues) => {
            assert!(issues.len() == 1);
            match &issues[0] {
                ConfigIssue::PortOutOfRange(value) => { assert!(*value == 70_000); }
                _ => { assert!(false); }
            }
        }
    }
    match std::fs::write(&dir.join(CONFIG_FILE_NAME.to_string()), "core.port = 99999\n".to_string()) {
        Err(_) => { assert!(false); }
        Ok(_) => { }
    }
    match endpoint_from_config(&dir) {
        (Some(_), _) => { assert!(false); }
        (None, issues) => {
            assert!(issues.len() == 1);
            match &issues[0] {
                ConfigIssue::PortOutOfRange(value) => { assert!(*value == 99_999); }
                _ => { assert!(false); }
            }
        }
    }
    cleanup_scratch(&dir);
}

#[test]
fn the_lock_reading_is_typed_absent_unreadable_or_malformed_and_never_a_bare_none() {
    // THE FINDING OF THE REVIEW OF PLAN ITEM 3.6b: `read_lock` SWALLOWED
    // EVERY read failure into `(None, [])`, so an EXISTING-but-UNREADABLE
    // lock read as ABSENT — and per `architecture.md:247` ABSENT means
    // "start a daemon", which the doc FORBIDS while a lock EXISTS ("If a
    // lock file exists, the interface connects and sends the handshake").
    // The THREE readings are DISTINCT here: a FRESH dir is [`LockReading.Absent`]
    // (ONLY `NotFound` reads that way); a `core.lock` that is a DIRECTORY
    // (`std::io`'s `IsADirectory`, the file EXISTS) is
    // [`LockReading.Unreadable`] carrying the kind AND the OS's OWN message
    // — NEVER Absent; readable-but-REFUSED text is
    // [`LockReading.Malformed`], and a refusal that still carried the
    // NONCE keeps the PARTIAL record (the sibling gate handshakes through
    // it), while a missing value holds NO record at all.
    let dir = scratch_dir("the_lock_reading_is_typed_absent_unreadable_or_malformed_and_never_a_bare_none".to_string());
    match read_lock(&dir) {
        LockReading::Absent => { }
        _ => { assert!(false); }
    }
    // The file EXISTS and CANNOT BE READ: `state/core.lock` as a DIRECTORY.
    match std::fs::create_dir(&dir.join(STATE_DIR_NAME.to_string())) {
        Err(_) => { assert!(false); }
        Ok(_) => { }
    }
    match std::fs::create_dir(&dir.join(STATE_DIR_NAME.to_string()).join(LOCK_FILE_NAME.to_string())) {
        Err(_) => { assert!(false); }
        Ok(_) => { }
    }
    match read_lock(&dir) {
        LockReading::Unreadable(kind, text) => {
            assert!(kind == std::io::ErrorKind::IsADirectory);
            assert!(text.len() > 0);
        }
        LockReading::Absent => {
            // THE EXACT BUG the finding recorded: the UNREADABLE lock read
            // as ABSENT. It must not happen NOW.
            assert!(false);
        }
        _ => { assert!(false); }
    }
    // GONE again for the MALFORMED rounds:
    match std::fs::remove_dir(&dir.join(STATE_DIR_NAME.to_string()).join(LOCK_FILE_NAME.to_string())) {
        Err(_) => { assert!(false); }
        Ok(_) => { }
    }
    // A readable lock with NO nonce VALUE: refused, NO record.
    match std::fs::write(&dir.join(STATE_DIR_NAME.to_string()).join(LOCK_FILE_NAME.to_string()), "address=127.0.0.1:47623\n".to_string()) {
        Err(_) => { assert!(false); }
        Ok(_) => { }
    }
    match read_lock(&dir) {
        LockReading::Malformed(partial, issues) => {
            assert!(issues.len() > 0);
            match partial {
                Some(_) => { assert!(false); }
                None => { }
            }
        }
        LockReading::Read(_) => { assert!(false); }
        _ => { assert!(false); }
    }
    // A readable lock whose NONCE is fine but which carries an UNKNOWN KEY:
    // REFUSED WITH A PARTIAL RECORD — the sibling gate HANDSHAKES through
    // that nonce (`architecture.md:247`: a lock EXISTS, so CONNECT).
    match std::fs::write(&dir.join(STATE_DIR_NAME.to_string()).join(LOCK_FILE_NAME.to_string()), "address=127.0.0.1:47623\nnonce=6f2c1a48e0b7d9c3\nghost=1\n".to_string()) {
        Err(_) => { assert!(false); }
        Ok(_) => { }
    }
    match read_lock(&dir) {
        LockReading::Malformed(partial, issues) => {
            assert!(issues.len() > 0);
            match partial {
                Some(held) => { assert!(held.nonce == "6f2c1a48e0b7d9c3".to_string()); }
                None => { assert!(false); }
            }
        }
        LockReading::Read(_) => { assert!(false); }
        _ => { assert!(false); }
    }
    cleanup_scratch(&dir);
}