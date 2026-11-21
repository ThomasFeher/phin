//! The rules of the handshake and of `state/core.lock` — plan item 3.6, first
//! half; the sockets and the files live in `core/src/technical/transport.rs`.
//!
//! `doc/architecture/core-protocol.md` ("Lock file and discovery", "Handshake")
//! fixes the data this module holds, so the transport can perform connect-or-spawn
//! without inventing rules of its own:
//!
//! * the shape of the lock file — plain text, one `key=value` per line, `#`
//!   comments allowed, keys `address` and `nonce`, written at daemon start-up
//!   and removed on exit;
//! * the nonce that identifies a daemon instance, and the rule that an
//!   interface reads the lock file **for the nonce only**: the address the
//!   lock names is never a connection instruction, it is only comparable with
//!   the configured address;
//! * the handshake pair: the `version` command of the command table carrying
//!   that nonce (the first line on the connection, before any other command),
//!   and the `hello` answer whose `nonce` field must echo exactly what was
//!   sent AND whose version must be the version this build speaks — "the
//!   handshake compares versions before any command is sent";
//! * the typed reading of one answer: our daemon, a nonce that does not echo,
//!   a version this build cannot answer, or a peer that is not our daemon at
//!   all, which is what makes a foreign listener on the configured address an
//!   error naming that address instead of a second daemon.
//!
//! The module is pure like the rest of `proto`: no socket and no file. Reading
//! `state/core.lock`, parsing `phin.toml` and connecting live in the transport
//! in the core; what lives here is the encoding, the nonce rule, and the
//! comparison that decides *our daemon, or not*.
//!
//! Three things are RECORDED as not being in the doc, plus one hazard of the
//! format that this module RECORDS instead of papering over (see below):
//!
//! * the SHAPE of the nonce — the doc asks only for "a nonce" and its example
//!   carries 16 lowercase hex characters, so [`NONCE_HEX_CHARS`] fixes that
//!   length and [`is_wellformed_nonce`] fixes lowercase hex. The value itself is
//!   minted in the core, not here: the daemon mints it with `core`'s own
//!   `technical/id.rs` hash helpers (decision 33), so `proto` holds the
//!   VALIDATION only and no second hex encoder and no digest-to-nonce route
//!   (an earlier draft had one, and it implied an undocumented channel from an
//!   interface to the daemon it spawns, which `core-protocol.md:20`/
//!   `:49-51` and `architecture.md:249` do not have: the DAEMON writes the lock
//!   and the interface READS it);
//! * the session id of [`session_id_of`] — `core-<nonce>` carries the lock nonce
//!   where the doc's `hello` example (`core-protocol.md:74`) shows the
//!   informational value `core-1`. Decision 28 keeps the per-instance id, so a
//!   doc amendment is owed to the orchestrator; the doc is not edited here;
//! * the EMPTY VALUE on EITHER key — `core-protocol.md:18-28` fixes NEITHER
//!   side, it never writes an empty `address=` nor an empty `nonce=`, so this
//!   build goes ONE way for BOTH keys: an EMPTY value is a TYPED REFUSAL with
//!   NO record handed out next to the complaint — [`LockIssue.BadAddress`] for
//!   `address=`, [`LockIssue.BadNonce`] for `nonce=` — exactly as a lock file
//!   naming only ONE of the two keys identifies no daemon instance. The way
//!   had to be chosen rather than left asymmetric (reviewer finding 2 on
//!   plan item 3.6a cycle 2), it is pinned by the test of [`parse_lock`] below
//!   in both directions, and the doc amendment is owed to the orchestrator;
//!   the doc is not edited here.
//!
//! ONE HAZARD IS RECORDED AS A LIMIT OF THIS MODULE, NOT AS A RULE OF THE
//! PROTOCOL, and PRODUCTION IS UNCHANGED about it: [`render_lock`] writes the
//! two values VERBATIM — `core-protocol.md:18-28` fixes NO escaping, so inventing
//! one would be a protocol change this plan item does not make — which means a
//! value containing `\n` CANNOT survive a [`render_lock`] -> [`parse_lock`] round
//! trip, because `\n` is the ONLY line separator the format has and the newline
//! SPLITS the value into two lines before the parser ever sees it. WHICH WAY
//! THIS BUILD GOES is recorded here and pinned by ONE assertion in
//! `a_lock_record_round_trips_and_comments_and_blank_lines_are_allowed`: this
//! build REFUSES NOTHING at render time, so the round-trip promise holds ONLY
//! for the values it actually writes — a `host:port` address and a lowercase-hex
//! nonce, both newline-free — and the hazard itself (an `address` carrying a
//! `\n` becomes an accepted-but-unusable record, its value SPLIT across two
//! lines) is handed to plan items 3.6b/3.6c, which MUST keep a `\n` out of the
//! address they put into `state/core.lock`. An escaping scheme is NOT invented
//! silently: adding one would change the pinned non-survival assertion.

use crate::codec::CodecError;
use crate::codec::decode_fields;
use crate::codec::encode_frame;
use crate::codec::Message;
use crate::codec::read_version;
use crate::command::Command;
use crate::command::command_name;
use crate::commands::PROTOCOL_VERSION;
use crate::event::Event;
use crate::event::event_kind;
use serde_json::Value;

/// The nonce of a daemon instance: exactly [`NONCE_HEX_CHARS`] lowercase hex
/// characters, the shape of the doc's lock-file example (`nonce=6f2c1a48e0b7d9c3`).
/// The length is a RECORDED INVENTION, kept equal to the doc example's length.
const NONCE_HEX_CHARS: usize = 16;

/// The `address=` key of `state/core.lock`.
pub const LOCK_ADDRESS_KEY: &str = "address";

/// The `nonce=` key of `state/core.lock`, the one key an interface acts on.
pub const LOCK_NONCE_KEY: &str = "nonce";

/// The whole content of one lock file: the address its writer listens on, plus
/// the nonce identifying this daemon instance to the interfaces.
#[derive(Debug, Eq, PartialEq)]
pub struct LockRecord {
    /// The `address=` value, e.g. `127.0.0.1:47623`. Never used as a connection
    /// instruction ("the lock file is read for the nonce only").
    pub address: String,
    /// The `nonce=` value, see [`is_wellformed_nonce`].
    pub nonce: String,
}

/// Why a lock file's text could not be taken as one record. A lock file this
/// module cannot read is NOT fatal to the transport: it means "no lock file",
/// and the discovery flow starts a daemon that overwrites it.
#[derive(Debug, Eq, PartialEq, Clone)]
pub enum LockIssue {
    /// A line that is neither a comment, nor empty, nor `key=value`; the text
    /// is what was seen.
    MalformedLine(String),
    /// A `key=` line whose key is neither [`LOCK_ADDRESS_KEY`] nor
    /// [`LOCK_NONCE_KEY`]; the key is what was seen. Reported, not fatal: a
    /// newer core may write keys this build does not know, and they must not
    /// hide our daemon.
    UnknownKey(String),
    /// The same key written by more than one line; the key is what was seen.
    /// RECORDED EDGE (`core-protocol.md:18-28` says nothing about a repeated
    /// key): the LAST value wins, which is how `state/markers.csv` reads a
    /// repeated path (`core/src/technical/state.rs`).
    DuplicateKey(String),
    /// The text holds no `address=` line. Per `core-protocol.md:20-28` the file
    /// contains BOTH values, so a text holding one of them holds no record at
    /// all, whatever the other value is.
    NoAddress,
    /// An `address=` line carrying NO VALUE AT ALL — empty, or nothing but
    /// whitespace, which the trimming of [`parse_lock`] reduces to empty. The
    /// empty text is what was seen. RECORDED EDGE (`core-protocol.md:18-28`
    /// fixes neither an empty `address=` nor an empty `nonce=`): this build
    /// refuses it, SYMMETRIC with [`LockIssue.BadNonce`] — reviewer finding 2
    /// on plan item 3.6a asked for ONE way, and the way chosen is that an
    /// empty value on EITHER key is a typed issue and no record, never a
    /// record holding an empty field while the other empty value is refused;
    /// the refusal of both is stated in the doc of [`parse_lock`] below and
    /// pinned by its test.
    BadAddress(String),
    /// The text holds no `nonce=` line.
    NoNonce,
    /// A `nonce=` value that is not [`NONCE_HEX_CHARS`] lowercase hex characters
    /// ("no lock file" for the transport); the text is the value seen.
    BadNonce(String),
}

/// Parse the text of a `state/core.lock` file into the record it holds.
///
/// Whitespace around a key, around the `=` and around a value is TRIMMED
/// (RECORDED EDGE: the doc fixes neither). A repeated key reports
/// [`LockIssue.DuplicateKey`] and the last value wins. A record is returned ONLY
/// when the text carries BOTH keys with usable values, because the doc's
/// format section writes both lines into the one file.
///
/// A RECORDED EDGE of its own (the doc fixes neither side): an EMPTY VALUE is
/// refused the SAME WAY on BOTH keys — an empty `address=` is
/// [`LockIssue.BadAddress`] and an empty `nonce=` is
/// [`LockIssue.BadNonce`] — and in either case NO record is handed out next to
/// the complaint, because a lock file naming an empty address identifies no
/// daemon instance any more than a lock file naming a non-nonce does. This is
/// a CHOICE of this build, not a rule of the doc; `core-protocol.md:18-28`
/// fixes neither side, the doc is NOT edited here (the doc amendment is owed
/// to the orchestrator), and the test of this module pins the refusal in both
/// directions.
pub fn parse_lock(text: &str) -> (Option::<LockRecord>, Vec::<LockIssue>) {
    let mut address: Option::<String> = None;
    let mut nonce: Option::<String> = None;
    let mut issues = Vec::<LockIssue>::new();
    let mut nonce_seen = false;
    let mut address_seen = false;
    let mut bad_nonce = false;
    let mut bad_address = false;
    for line in text.split("\n") {
        let trimmed = line.trim().to_string();
        if trimmed.len() == 0 || trimmed.starts_with("#") {
            continue;
        }
        match trimmed.find("=") {
            None => {
                issues.push(LockIssue::MalformedLine(trimmed));
            }
            Some(index) => {
                let key = trimmed[..index].trim().to_string();
                let value = trimmed[index + 1 ..].trim().to_string();
                if key == LOCK_ADDRESS_KEY {
                    if address_seen {
                        issues.push(LockIssue::DuplicateKey(key));
                    }
                    address_seen = true;
                    // An `address=` line with no value is refused as a typed
                    // issue, exactly as an unusable `nonce=` is (RECORDED EDGE,
                    // the doc fixes neither side): an empty address names no
                    // listener, so it must not become a record with an empty
                    // field while an empty nonce is refused.
                    if value.len() == 0 {
                        issues.push(LockIssue::BadAddress(value.to_string()));
                        bad_address = true;
                    }
                    address = Some(value);
                    continue;
                }
                if key == LOCK_NONCE_KEY {
                    if nonce_seen {
                        issues.push(LockIssue::DuplicateKey(key));
                    }
                    nonce_seen = true;
                    if !is_wellformed_nonce(&value) {
                        issues.push(LockIssue::BadNonce(value.to_string()));
                        bad_nonce = true;
                    }
                    nonce = Some(value);
                    continue;
                }
                issues.push(LockIssue::UnknownKey(key));
            }
        }
    }
    if address == None {
        issues.push(LockIssue::NoAddress);
    }
    if nonce == None {
        issues.push(LockIssue::NoNonce);
    }
    if bad_nonce || bad_address {
        // A record whose nonce is not a nonce, or whose address is empty, is
        // no record at all: the transport starts a fresh daemon and overwrites
        // the file.
        return (None, issues);
    }
    match (address, nonce) {
        (Some(address_value), Some(nonce_value)) => { return (Some(LockRecord { address: address_value, nonce: nonce_value }), issues); }
        _ => { return (None, issues); }
    }
}

/// Render the record as the text of `state/core.lock`.
///
/// The two lines are the two keys of `core-protocol.md:18-28` in the order the
/// doc's own fenced example writes them, `address=` first, one `\n` after each,
/// and nothing else at all: the `#` comments that section permits are for a
/// reader, and this writer adds none (pinned line by line against the doc's
/// block by the tests below).
///
/// THE VALUES ARE WRITTEN VERBATIM, VALIDATED BY NOTHING HERE (RECORDED HAZARD,
/// PRODUCTION UNCHANGED): `\n` is the format's only line separator and no
/// escaping exists in `core-protocol.md:18-28`, so a value carrying a `\n` is
/// NOT a value this protocol can put in `state/core.lock` — it comes back from
/// [`parse_lock`] as a DIFFERENT record with an extra line, never as the record
/// written. The obligation is on the CALLER (plan items 3.6b/3.6c): the transport
/// writes a `host:port` endpoint and a nonce validated by
/// [`is_wellformed_nonce`], and MUST keep a `\n` out of the address; the round
/// trip is pinned for those values and the non-survival of a newline-bearing
/// address is pinned too, so neither promise can drift silently.
pub fn render_lock(record: &LockRecord) -> String {
    let mut text = String::new();
    text.push_str(&LOCK_ADDRESS_KEY);
    text.push_str("=");
    text.push_str(&record.address);
    text.push_str("\n");
    text.push_str(&LOCK_NONCE_KEY);
    text.push_str("=");
    text.push_str(&record.nonce);
    text.push_str("\n");
    return text;
}

/// Whether `nonce` is a wellformed instance id: [`NONCE_HEX_CHARS`] characters
/// of lowercase hex, the shape of the doc's example.
pub fn is_wellformed_nonce(nonce: &str) -> bool {
    if nonce.len() != NONCE_HEX_CHARS {
        return false;
    }
    for character in nonce.chars() {
        if character < '0' { return false; }
        if character > 'f' { return false; }
        // The gap between `9` and `a` holds no hex digit.
        if character > '9' && character < 'a' { return false; }
    }
    return true;
}

/// The session id the handshake of one daemon instance answers with, derived from
/// the nonce of its lock file: the value plan item 3.5 handed as
/// `interpret::DEFAULT_SESSION` because it had no lock file, which plan item 3.6
/// replaces with this per-instance id ("Handshake": the `session` field is
/// informational, so any value identifying THIS instance works). The deviation
/// from the doc example's `core-1` is recorded in this module's doc above.
pub fn session_id_of(nonce: &str) -> String {
    let mut id = String::new();
    id.push_str("core-");
    id.push_str(&nonce.to_string());
    return id;
}

/// The one line an interface sends **before any other command** on a connection:
/// the `version` command of the command table carrying the nonce read from
/// `state/core.lock`.
pub fn version_request(nonce: &str) -> Command {
    return Command::Version(nonce.to_string());
}

/// That handshake request as one frame; the empty string means the codec
/// refused, which for a nonce this module validates cannot happen.
///
/// OBLIGATION ON THE CALLER (plan items 3.6b/3.6c): this function SWALLOWS a
/// codec refusal into [`String::new`], so a caller MUST validate the nonce with
/// [`is_wellformed_nonce`] BEFORE asking for a frame, and must treat the empty
/// string as "no handshake request", never send it.
pub fn version_request_frame(nonce: &str) -> String {
    match encode_frame(&Message::Inbound(version_request(nonce))) {
        Err(_reason) => { return String::new(); }
        Ok(frame) => { return frame; }
    }
}

/// What one answer to the handshake request says about the listener.
#[derive(Debug, Eq, PartialEq)]
pub enum HandshakeCheck {
    /// The listener answered `hello` echoing the nonce sent at the version this
    /// build speaks: this is our daemon, and the connection may carry commands.
    Echoed,
    /// The line carried, or the `hello` payload claimed, a protocol version
    /// this build does not speak; both versions are kept (the one read, and
    /// [`PROTOCOL_VERSION`]) so the transport can name them. THIS is how "the
    /// handshake compares versions before any command is sent"
    /// (`core-protocol.md:86`) is observable: without it a genuine daemon of
    /// another protocol version, which `core/src/protocol/interpret.rs:470-475`
    /// answers with an `error` event, could only be reported as a foreign
    /// listener, i.e. as an occupied address.
    VersionMismatch(i64, i64),
    /// The answer carried a different nonce: "the listener is not our daemon";
    /// both nonces are kept so the transport can name them in its error.
    NotEchoed(String, String),
    /// The answer line decoded, but is not a `hello` event at all; the name is
    /// what was seen. An unrelated process on the configured address answers
    /// something else — the transport reports it and does NOT start a daemon,
    /// because the configured address is taken.
    NotHello(String),
    /// The answer line is not decodable by the codec of plan item 3.1: also
    /// "not our daemon".
    UnreadableAnswer(CodecError)
}

/// What one answer line of the handshake can be read as, BEFORE the nonce
/// comparison [`check_handshake_answer`] performs.
///
/// The three readings are `proto`'s OWN: [`read_version`] for the version of the
/// line and [`decode_fields`] for the message it carries — which is exactly
/// what [`decode_line`] does, except that `decode_line` throws the version away
/// after checking it, and the version of the ANSWER line is precisely what
/// `doc/architecture/core-protocol.md:86` ("the handshake compares versions
/// before any command is sent") needs compared, so this module reads the two
/// fields separately rather than losing one of them.
#[derive(Debug, Eq, PartialEq)]
enum AnswerLine {
    /// The version the line carries, and the message decoded from its rest.
    Read(i64, Message),
    /// The line carries a `v` above the version this build speaks, which
    /// [`read_version`] refuses with [`CodecError.UnsupportedVersion`] carrying
    /// that version: the version itself is the finding, so it is NOT reported
    /// as an unreadable line.
    RefusedVersion(i64),
    /// The line is not one JSON object, or holds no command/event discriminator
    /// this build can read; the codec's own reason is kept.
    Unreadable(CodecError),
}

/// Read one answer line as the version it carries and the message it holds.
fn read_answer_line(line: &str) -> AnswerLine {
    let parsed = serde_json::from_str::<Value>(line).map_err(|_| CodecError::UnparseableLine);
    match parsed {
        Err(_) => { return AnswerLine::Unreadable(CodecError::UnparseableLine); }
        Ok(mut held) => {
            match held.as_object_mut() {
                None => { return AnswerLine::Unreadable(CodecError::UnparseableLine); }
                Some(mut fields) => {
                    match read_version(&mut fields) {
                        Err(reason) => {
                            match reason {
                                CodecError::UnsupportedVersion(read) => { return AnswerLine::RefusedVersion(read); }
                                _ => { return AnswerLine::Unreadable(reason); }
                            }
                        }
                        Ok(spoken) => {
                            match decode_fields(&mut fields) {
                                Err(reason) => { return AnswerLine::Unreadable(reason); }
                                Ok(message) => { return AnswerLine::Read(spoken, message); }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Check the daemon's answer to [`version_request_frame`]: decode one line,
/// compare the echoed nonce with the nonce sent ("Handshake") and the version
/// with the version this build speaks ("Versioning").
///
/// The version is compared FIRST and EXACTLY: `core-protocol.md:86` says the
/// handshake compares versions before any command is sent, and
/// `core/src/protocol/interpret.rs:470-475` already answers a `version` line
/// whose version is not this build's with an `error` event rather than a `hello`,
/// so a peer of another protocol version reaches this build as a line carrying
/// either a `v` [`read_version`] refuses or a `hello` claiming another `version`
/// — both are a [`HandshakeCheck.VersionMismatch`], which is NEITHER "our
/// daemon" NOR "the address is taken by an unrelated process", and an interface
/// may send no commands in either case.
pub fn check_handshake_answer(nonce_sent: &str, line: &str) -> HandshakeCheck {
    match read_answer_line(line) {
        AnswerLine::RefusedVersion(read) => { return HandshakeCheck::VersionMismatch(read, PROTOCOL_VERSION); }
        AnswerLine::Unreadable(reason) => { return HandshakeCheck::UnreadableAnswer(reason); }
        AnswerLine::Read(line_version, message) => {
            if line_version != PROTOCOL_VERSION {
                return HandshakeCheck::VersionMismatch(line_version, PROTOCOL_VERSION);
            }
            match message {
                Message::Outbound(event) => {
                    match event {
                        Event::Hello(nonce, _session, claimed) => {
                            if nonce != nonce_sent.to_string() {
                                return HandshakeCheck::NotEchoed(nonce_sent.to_string(), nonce.to_string());
                            }
                            if claimed != PROTOCOL_VERSION {
                                return HandshakeCheck::VersionMismatch(claimed, PROTOCOL_VERSION);
                            }
                            return HandshakeCheck::Echoed;
                        }
                        _ => { return HandshakeCheck::NotHello(event_kind(&event).to_string()); }
                    }
                }
                // An inbound line is what the interface writes, never what a
                // daemon answers.
                Message::Inbound(command) => { return HandshakeCheck::NotHello(command_name(&command).to_string()); }
            }
        }
    }
}

// ============================================================================
// Tests
// ============================================================================

/// The fenced block of the doc's lock-format section (core-protocol.md, "Lock
/// file and discovery") as the lines it holds, read through `proto`'s ONE
/// doc-reading helper (`proto/src/codec.rs`, where the doc scans of the package
/// live — reviewer finding 5 on plan item 3.6 forbade a second copy of that
/// body). This block is the contract [`render_lock`] is measured against.
#[cfg(test)]
fn doc_lock_block() -> Vec::<String> {
    return crate::codec::fenced_block_after(&crate::codec::read_core_protocol_doc(), "Format (plain text");
}

#[test]
fn the_lock_text_is_rendered_exactly_as_the_doc_lock_block_line_by_line() {
    // Findings 6 and 13: the FORMAT of `state/core.lock`, the KEY ORDER of
    // `render_lock` and the rule that this writer adds no `#` header are pinned
    // to the doc's own fenced block LINE BY LINE. The record is the doc's
    // example record, read back through parse_lock first, so renaming a key in
    // the doc (`address=` -> `host=`), swapping the two lines, lengthening the
    // example or adding a comment header all fail HERE.
    let block = doc_lock_block();
    assert!(block.len() == 2);
    // FINDING 3 of the review of plan item 3.6a cycle 2 adds the cross-pin
    // below (the FORMAT of the block itself is pinned by the comment above):
    // the doc's LOCK block and its handshake examples describe the SAME daemon
    // instance, so the nonce the LOCK block writes must be the nonce the
    // handshake request carries. Without such an assertion, editing the doc's
    // lock nonce to ANOTHER VALID 16-hex value (`6f2c1a48e0b7d9c3` ->
    // `0011223344556677`) left every test of this module green: the shape,
    // key names and order were pinned, the AGREEMENT of the two sections
    // was not. Both values are read out of the doc AT TEST TIME through
    // `proto`'s ONE doc reader (`codec::read_core_protocol_doc`,
    // `codec::doc_examples`, `codec::decode_line`), so the two doc sections
    // cannot drift silently. The pin deliberately does NOT hard-code either
    // value, so it constrains the doc only as far as "one daemon instance, one
    // nonce in both sections"; if a future doc wants the two sections to carry
    // DIFFERENT example values, that is a doc decision for the orchestrator,
    // and this assertion is where it would have to be stated, not worked around.
    let (cross_ids, cross_examples) = crate::codec::doc_examples(&crate::codec::read_core_protocol_doc());
    assert!(cross_ids.len() == cross_examples.len());
    let mut cross_request = String::new();
    for index in 0 .. cross_ids.len() {
        if cross_ids[index].to_string() == "core-handshake-request".to_string() {
            cross_request = cross_examples[index].to_string();
        }
    }
    assert!(cross_request.len() > 0);
    // The nonce of that example, taken OUT of the JSON value with the codec of
    // plan item 3.1 rather than by matching a substring of the doc's line.
    let mut cross_nonce = String::new();
    match crate::codec::decode_line(&cross_request) {
        Err(_) => { assert!(false); }
        Ok(message) => {
            match message {
                crate::codec::Message::Inbound(command) => {
                    match command {
                        crate::command::Command::Version(nonce) => { cross_nonce = nonce.to_string(); }
                        _ => { assert!(false); }
                    }
                }
                crate::codec::Message::Outbound(_event) => { assert!(false); }
            }
        }
    }
    assert!(cross_nonce.len() == NONCE_HEX_CHARS);
    let mut doc_text = String::new();
    for line in &block {
        doc_text.push_str(&line.to_string());
        doc_text.push_str("\n");
    }
    let (from_doc, issues) = parse_lock(&doc_text);
    assert!(issues.len() == 0);
    match from_doc {
        None => { assert!(false); }
        Some(record) => {
            let rendered = render_lock(&record);
            let (lines, tail) = crate::codec::split_lines(&rendered);
            // One `\n` after each value, nothing pending, nothing added.
            assert!(tail.len() == 0);
            assert!(lines.len() == block.len());
            let mut mismatches = Vec::<String>::new();
            for index in 0 .. lines.len() {
                if lines[index] != block[index] {
                    let rendered_line = lines[index].to_string();
                    let doc_line = block[index].to_string();
                    mismatches.push(format!("rendered [{rendered_line}] doc block [{doc_line}]"));
                }
            }
            if mismatches.len() > 0 {
                for shown in &mismatches {
                    println!("lock format drift: {shown}");
                }
            }
            assert!(mismatches.len() == 0);
            // THE CROSS-PIN OF FINDING 3: the nonce `parse_lock` reads back
            // out of the doc's OWN LOCK block is the nonce the doc's
            // handshake request carries. A LOCK block naming another nonce
            // than the handshake examples is an inconsistent doc, and this
            // assertion is what makes that visible at test time.
            assert!(record.nonce == cross_nonce);
        }
    }
}

#[test]
fn the_docs_lock_example_nonce_is_pinned_by_exact_value_not_substring() {
    // The other half of finding 6: the nonce LENGTH this module validates is
    // the length of the doc's example value, compared as a value taken OUT of
    // the doc block, not with `text.contains("6f2c1a48e0b7d9c3")` (which a
    // 24-character doc nonce would still satisfy).
    let block = doc_lock_block();
    assert!(block.len() == 2);
    let nonce_line = block[1].to_string();
    match nonce_line.find("=") {
        None => { assert!(false); }
        Some(index) => {
            let value = nonce_line[index + 1 ..].to_string();
            assert!(value.len() == NONCE_HEX_CHARS);
            assert!(is_wellformed_nonce(&value));
            // One character longer or shorter than the doc example is refused.
            let shorter = value[..NONCE_HEX_CHARS - 1].to_string();
            assert!(shorter.len() == NONCE_HEX_CHARS - 1);
            assert!(!is_wellformed_nonce(&shorter));
            let longer = value.to_string() + "0";
            assert!(longer.len() == NONCE_HEX_CHARS + 1);
            assert!(!is_wellformed_nonce(&longer));
        }
    }
    // The address line of the same block names the key parse_lock reads.
    let address_line = block[0].to_string();
    assert!(address_line.starts_with(LOCK_ADDRESS_KEY));
}

#[test]
fn the_nonce_shape_this_module_validates_is_a_recorded_invention() {
    // core-protocol.md:20 asks only for "a nonce"; the SHAPE — lowercase hex
    // of NONCE_HEX_CHARS — is RECORDED here, and the value is minted in the
    // core (decision 33), not derived from a digest in proto. Anything else is
    // not a nonce this protocol writes.
    let example = "6f2c1a48e0b7d9c3".to_string();
    assert!(is_wellformed_nonce(&example));
    assert!(example.len() == NONCE_HEX_CHARS);
    assert!(!is_wellformed_nonce("6F2C1A48E0B7D9C3"));
    assert!(!is_wellformed_nonce("6f2c1a48e0b7d9c"));
    assert!(!is_wellformed_nonce("6f2c1a48e0b7d9c30"));
    assert!(!is_wellformed_nonce("6f2c1a48e0b7d9c-"));
    assert!(!is_wellformed_nonce(""));
    // The gap between `9` and `a` holds no hex digit.
    assert!(!is_wellformed_nonce("6f2c1a48e0b7d9g3"));
}

#[test]
fn a_lock_record_round_trips_and_comments_and_blank_lines_are_allowed() {
    let record = LockRecord { address: "127.0.0.1:47623".to_string(), nonce: "6f2c1a48e0b7d9c3".to_string() };
    let rendered = render_lock(&record);
    let (back, issues) = parse_lock(&rendered);
    match back {
        Some(held) => { assert!(held == record); }
        None => { assert!(false); }
    }
    assert!(issues.len() == 0);
    // Comments and blank lines, as the format section allows, before and after.
    let parts = ["# written at start-up\n\n".to_string(), rendered, "# removed on exit\n".to_string()].to_vec();
    let commented = parts.join("");
    let (back2, issues2) = parse_lock(&commented);
    match back2 {
        Some(held) => { assert!(held == record); }
        None => { assert!(false); }
    }
    assert!(issues2.len() == 0);
    // FINDING 3 of the review of plan item 3.6a cycle 3: `render_lock`
    // (handshake.rs:217-230) writes BOTH fields VERBATIM and validates NOTHING,
    // so the round trip above holds ONLY for values this build actually puts in
    // `state/core.lock`. THE ONE ASSERTION below pins the OTHER half of that
    // claim: a record whose `address` contains a `\n` CANNOT survive a
    // `render_lock` -> `parse_lock` round trip, because `\n` is the format's ONLY
    // line separator and the newline SPLITS the value into two lines before
    // `parse_lock` sees it — here the written text holds TWO `address=` lines,
    // so what comes back is the last-wins record `127.0.0.1:2` WITH
    // `LockIssue.DuplicateKey("address")`, NOT the record written. THE WAY THIS
    // BUILD GOES (stated in this module's doc and in `render_lock`'s own doc
    // comment, PRODUCTION UNTOUCHED) is to REFUSE NOTHING at render time and
    // to HAND THE HAZARD TO plan items 3.6b/3.6c, which must keep a `\n` out of
    // the address they write (`transport_cfg.rs` supplies a `host:port` endpoint
    // and the nonce is validated lowercase hex, so neither can carry a newline
    // there). No escaping is invented here — `core-protocol.md:18-28` fixes
    // none — and if a future item DID add one, this pinned non-survival is where
    // that protocol change would have to be stated, not worked around.
    let written = LockRecord { address: "127.0.0.1:47623\naddress=127.0.0.1:2".to_string(), nonce: "6f2c1a48e0b7d9c3".to_string() };
    let (read_back, read_issues) = parse_lock(&render_lock(&written));
    match read_back {
        Some(held) => {
            // THE ONE ASSERTION OF THE FINDING: the round trip does NOT return
            // the record that was written.
            assert!(held != written);
            // Non-vacuity of that assertion: the value was SPLIT AT the newline,
            // so the text names the address twice and the last-wins rule of the
            // test above leaves `127.0.0.1:2` behind. A renderer that ESCAPED
            // or REFUSED the newline would change what is read back, and this
            // is exactly where that change would have to be decided.
            assert!(held.address == "127.0.0.1:2".to_string());
            assert!(held.nonce == "6f2c1a48e0b7d9c3".to_string());
        }
        None => { assert!(false); }
    }
    assert!(read_issues.contains(&LockIssue::DuplicateKey("address".to_string())));
}

#[test]
fn a_lock_file_the_protocol_cannot_read_is_reported_not_guessed() {
    // A nonce that is not a nonce makes the whole file unreadable: the
    // transport treats that as "no lock file" and starts a daemon that
    // overwrites it.
    let (broken, issues_a) = parse_lock("address=127.0.0.1:1\nnonce=NOT_A_NONCE\n");
    assert!(broken == None);
    assert!(issues_a.contains(&LockIssue::BadNonce("NOT_A_NONCE".to_string())));
    // An unknown key is reported but does not hide an otherwise complete record:
    // a newer core's extra key must not make our daemon invisible.
    let (newer, issues_b) = parse_lock("address=127.0.0.1:1\nnonce=6f2c1a48e0b7d9c3\npid=4242\n");
    let same = LockRecord { address: "127.0.0.1:1".to_string(), nonce: "6f2c1a48e0b7d9c3".to_string() };
    match newer {
        Some(held) => { assert!(held == same); }
        None => { assert!(false); }
    }
    assert!(issues_b.contains(&LockIssue::UnknownKey("pid".to_string())));
    // A line that is neither a comment nor `key=value` is malformed, not guessed
    // at.
    let (junk, issues_c) = parse_lock("nonsense line\n");
    assert!(junk == None);
    assert!(issues_c.contains(&LockIssue::MalformedLine("nonsense line".to_string())));
    // The empty file is "no lock file", not a parse failure.
    let (empty, issues_d) = parse_lock("");
    assert!(empty == None);
    assert!(issues_d.contains(&LockIssue::NoAddress));
    assert!(issues_d.contains(&LockIssue::NoNonce));
}

#[test]
fn a_lock_file_holding_one_of_the_two_values_holds_no_record() {
    // core-protocol.md:20-28 writes BOTH lines into the one file, so a text
    // carrying only one of them names no daemon instance. Pinned in both
    // directions, and the issue says which key is missing — a record must NOT
    // be returned together with the complaint about the other key.
    let (nonce_only, issues_a) = parse_lock("nonce=6f2c1a48e0b7d9c3\n");
    assert!(nonce_only == None);
    assert!(issues_a.contains(&LockIssue::NoAddress));
    assert!(!issues_a.contains(&LockIssue::NoNonce));
    let (address_only, issues_b) = parse_lock("address=127.0.0.1:47623\n");
    assert!(address_only == None);
    assert!(issues_b.contains(&LockIssue::NoNonce));
    assert!(!issues_b.contains(&LockIssue::NoAddress));
}

#[test]
fn an_empty_value_of_either_lock_key_is_a_typed_issue_and_no_record() {
    // Reviewer finding 2 on plan item 3.6a cycle 2: the EMPTY-VALUE handling
    // was ASYMMETRIC — `parse_lock("address=\nnonce=6f2c1a48e0b7d9c3\n")` yielded
    // `Some(LockRecord { address: "", … })` with NO issue at all, while an
    // empty `nonce=` was refused as `LockIssue.BadNonce("")`. The doc fixes
    // NEITHER side (`core-protocol.md:18-28` never writes an empty value), so
    // ONE way had to be chosen, and the way this module goes is the SYMMETRIC
    // REFUSAL: an empty value on EITHER key is a typed `LockIssue` —
    // [`LockIssue.BadAddress`] for `address=`, [`LockIssue.BadNonce`] for
    // `nonce=` — and NO record is handed out next to the complaint, exactly
    // as the one-of-two-values rule above refuses a record naming half a
    // daemon instance. A lock file this module cannot read is "no lock file"
    // for the transport, which starts a daemon that overwrites it; a record
    // with an empty `address` would instead be a lock file that CAN be read
    // and that names an address no daemon ever listens on.
    let (address_empty, issues_a) = parse_lock("address=\nnonce=6f2c1a48e0b7d9c3\n");
    assert!(address_empty == None);
    assert!(issues_a.contains(&LockIssue::BadAddress("".to_string())));
    // The key IS present, so the complaint is the EMPTY VALUE, not a missing
    // key (that would be `LockIssue.NoAddress`, which this text does not raise).
    assert!(!issues_a.contains(&LockIssue::NoAddress));
    // The mirror image on the OTHER key, which was already refused this way
    // before this cycle and is pinned here so the two sides cannot drift apart
    // again.
    let (nonce_empty, issues_b) = parse_lock("address=127.0.0.1:47623\nnonce=\n");
    assert!(nonce_empty == None);
    assert!(issues_b.contains(&LockIssue::BadNonce("".to_string())));
    assert!(!issues_b.contains(&LockIssue::NoNonce));
    // Whitespace around a value is trimmed (pinned by the test below), so a
    // value of nothing but spaces IS an empty value and is refused the same
    // way on both keys: the issue text is the trimmed value, empty.
    let (spaced, issues_c) = parse_lock("address=   \nnonce=   \n");
    assert!(spaced == None);
    assert!(issues_c.contains(&LockIssue::BadAddress("".to_string())));
    assert!(issues_c.contains(&LockIssue::BadNonce("".to_string())));
    // And a value that merely starts with whitespace but holds text is still
    // the record it reads as, so this refusal did not become a general ban on
    // whitespace: only an EMPTY value is refused.
    let (still, issues_d) = parse_lock("address=  127.0.0.1:47623  \nnonce=  6f2c1a48e0b7d9c3  \n");
    assert!(issues_d.len() == 0);
    match still {
        Some(record) => { assert!(record.address == "127.0.0.1:47623".to_string()); }
        None => { assert!(false); }
    }
}

#[test]
fn whitespace_around_a_key_and_a_repeated_key_are_pinned_lock_edges() {
    // The doc fixes neither; they are PINNED rather than left free (precedent:
    // 3.3's amendment row). Whitespace around the key, around `=` and around
    // the value is trimmed, so an indented lock file still reads.
    let (spaced, issues_a) = parse_lock("  address = 127.0.0.1:47623  \n\tnonce=  6f2c1a48e0b7d9c3  \n");
    assert!(issues_a.len() == 0);
    let wanted = LockRecord { address: "127.0.0.1:47623".to_string(), nonce: "6f2c1a48e0b7d9c3".to_string() };
    match spaced {
        Some(held) => { assert!(held == wanted); }
        None => { assert!(false); }
    }
    // A key written twice reports DuplicateKey, and the LAST value wins — which
    // is how `state/markers.csv` reads a repeated path (core/src/technical/state
    // .rs), so the two files of `state/` agree.
    let (twice, issues_b) = parse_lock("address=127.0.0.1:1\nnonce=6f2c1a48e0b7d9c3\naddress=127.0.0.1:2\n");
    assert!(issues_b.contains(&LockIssue::DuplicateKey("address".to_string())));
    match twice {
        Some(held) => { assert!(held.address == "127.0.0.1:2".to_string()); }
        None => { assert!(false); }
    }
    // FINDING 1 of the review of plan item 3.6a cycle 3 (THE BLOCKER): the
    // SAME edge on the OTHER key was UNPINNED — the reviewer's mutants
    // `m_dup_nonce_unreported` (suppressing the `DuplicateKey` REPORT) and
    // `m_dup_first_wins_nonce` (FIRST `nonce=` value winning) both SURVIVED all
    // 33 tests, while the two identical mutants on `address=` are CAUGHT by the
    // four lines above. The two assertions below MIRROR those for `address=`
    // against the code site that already treats the two keys ALIKE
    // (handshake.rs:172-182, production UNTOUCHED by this cycle): a repeated
    // `nonce=` REPORTS `LockIssue.DuplicateKey("nonce")` AND the LAST value
    // wins, the same last-wins rule `state/markers.csv` reads a repeated path
    // with, and the complaint does NOT swallow the record the good lines wrote.
    let (twice_nonce, issues_e) = parse_lock("address=127.0.0.1:47623\nnonce=6f2c1a48e0b7d9c3\nnonce=6f2c1a48e0b7d9c4\n");
    assert!(issues_e.contains(&LockIssue::DuplicateKey("nonce".to_string())));
    match twice_nonce {
        Some(held) => {
            assert!(held.nonce == "6f2c1a48e0b7d9c4".to_string());
            // The record SURVIVES the complaint, and the other key is untouched
            // by it, so the outcome is exactly "last value wins, reported".
            assert!(held.address == "127.0.0.1:47623".to_string());
        }
        None => { assert!(false); }
    }
    // A malformed key (no `=`) is reported as such and keeps the record the
    // two good lines wrote.
    let (stray, issues_c) = parse_lock("address=127.0.0.1:47623\nnonce=6f2c1a48e0b7d9c3\noops\n");
    assert!(issues_c.contains(&LockIssue::MalformedLine("oops".to_string())));
    match stray {
        Some(held) => { assert!(held == wanted); }
        None => { assert!(false); }
    }
}

#[test]
fn the_handshake_pair_of_this_module_is_the_docs_test_example_as_a_value() {
    // The two `test-example`s of core-protocol.md "Handshake", taken OUT of the
    // doc through `proto`'s ONE doc-reading helper and compared as VALUES
    // (serde_json sorts keys, so frames are never compared byte-wise — plan
    // 3.1c2's obligation): the request this module writes decodes to the doc's
    // request, and the doc's answer passes this module's check with the nonce
    // of the doc's request.
    let (ids, examples) = crate::codec::doc_examples(&crate::codec::read_core_protocol_doc());
    let mut request = String::new();
    let mut answer = String::new();
    for index in 0 .. ids.len() {
        let id = ids[index].to_string();
        if id == "core-handshake-request".to_string() { request = examples[index].to_string(); }
        if id == "core-handshake-response".to_string() { answer = examples[index].to_string(); }
    }
    assert!(request.len() > 0);
    assert!(answer.len() > 0);
    // The nonce the doc's request carries, read with proto's decoder.
    let mut sent = String::new();
    match crate::codec::decode_line(&request) {
        Err(_) => { assert!(false); }
        Ok(message) => {
            match message {
                crate::codec::Message::Inbound(command) => {
                    match command {
                        crate::command::Command::Version(nonce) => { sent = nonce.to_string(); }
                        _ => { assert!(false); }
                    }
                }
                crate::codec::Message::Outbound(_event) => { assert!(false); }
            }
        }
    }
    assert!(is_wellformed_nonce(&sent));
    // This module's frame decodes to the same message as the doc's line.
    match crate::codec::decode_line(&version_request_frame(&sent)) {
        Err(_) => { assert!(false); }
        Ok(written) => {
            match crate::codec::decode_line(&request) {
                Err(_) => { assert!(false); }
                Ok(documented) => { assert!(written == documented); }
            }
        }
    }
    // The doc's answer, checked against the nonce the doc's request carries.
    assert!(check_handshake_answer(&sent, &answer) == HandshakeCheck::Echoed);
}

#[test]
fn a_hello_answer_at_another_protocol_version_is_a_version_mismatch() {
    // Findings 4 and 8: "the handshake compares versions before any command is
    // sent" (core-protocol.md:86) needs an outcome of its own. A daemon of
    // another protocol version is NOT a foreign process, and it is NOT our
    // daemon: an interface may send no commands to either, but only a
    // version-shaped answer may be reported as a version at all.
    let sent = "6f2c1a48e0b7d9c3".to_string();
    let session = session_id_of(&sent);
    // A hello answering at the version this build speaks, through the sanctioned
    // 3.5 route (`crate::codec::encode_frame` of the hello event — this module
    // writes no hello line of its own).
    let own_answer = crate::codec::encode_frame(&crate::codec::Message::Outbound(crate::event::Event::Hello(sent.to_string(), session.to_string(), crate::commands::PROTOCOL_VERSION)));
    match own_answer {
        Err(_) => { assert!(false); }
        Ok(own_frame) => {
            assert!(check_handshake_answer(&sent, &own_frame) == HandshakeCheck::Echoed);
            // The same event with the payload version of another build.
            let others = crate::codec::encode_frame(&crate::codec::Message::Outbound(crate::event::Event::Hello(sent.to_string(), session.to_string(), crate::commands::PROTOCOL_VERSION + 1)));
            match others {
                Err(_) => { assert!(false); }
                Ok(other_frame) => {
                    let wanted = HandshakeCheck::VersionMismatch(crate::commands::PROTOCOL_VERSION + 1, crate::commands::PROTOCOL_VERSION);
                    assert!(check_handshake_answer(&sent, &other_frame) == wanted);
                }
            }
        }
    }
    // The `v` of the ANSWER line is read with `codec::read_version` and KEPT,
    // not discarded by decoding: the doc's answer example, rebuilt field by field
    // with a `v` this build does not speak, is refused by the codec, and that
    // refusal is reported as the version that was read — NOT as
    // [`HandshakeCheck.UnreadableAnswer`], which is for a line that is not one
    // JSON object at all.
    let mut fields = serde_json::Map::<String, Value>::new();
    crate::codec::put_number(&mut fields, crate::commands::PROTOCOL_FIELD, crate::commands::PROTOCOL_VERSION + 1);
    crate::codec::put_text(&mut fields, crate::commands::EVENT_FIELD, &crate::event::event_kind(&crate::event::Event::Hello(sent.to_string(), session.to_string(), crate::commands::PROTOCOL_VERSION)));
    crate::codec::put_text(&mut fields, crate::commands::HELLO_NONCE_FIELD, &sent);
    crate::codec::put_text(&mut fields, crate::commands::HELLO_SESSION_FIELD, &session);
    crate::codec::put_number(&mut fields, crate::commands::HELLO_VERSION_FIELD, crate::commands::PROTOCOL_VERSION);
    let rebuilt = serde_json::Value::Object(fields);
    let line = rebuilt.to_string();
    let wanted_refused = HandshakeCheck::VersionMismatch(crate::commands::PROTOCOL_VERSION + 1, crate::commands::PROTOCOL_VERSION);
    assert!(check_handshake_answer(&sent, &line) == wanted_refused);
    // THE ANSWER-LINE GATE ITSELF, NOT THE CODEC IN FRONT OF IT: `codec::read_version`
    // (codec.rs:241-248) accepts ANY `v <= PROTOCOL_VERSION`, so an answer
    // line carrying a LOWER `v` than this build speaks is decoded successfully
    // and reaches the comparison `check_handshake_answer` performs on the
    // version of the line. The case above cannot see that comparison: a `v`
    // ABOVE this build fails earlier, in the codec
    // (`AnswerLine::RefusedVersion`), and the payload case fails later
    // (`HELLO_VERSION_FIELD`), so deleting the line-version comparison left
    // ALL 32 tests green (reviewer finding 1 on plan item 3.6a cycle 2,
    // mutant `iii_drop_line_version_check`). A `hello` answered at `v:0` with
    // a WELL-FORMED payload at the version this build speaks is a version
    // mismatch naming BOTH versions, exactly the reviewer's
    // characterization (`{"event":"hello","nonce":"6f2c1a48e0b7d9c3","session":"core-6f2c1a48e0b7d9c3","v":0,"version":1}`
    // -> `VersionMismatch(0, 1)`), and it is NEITHER `Echoed` NOR
    // `UnreadableAnswer`.
    let mut low_fields = serde_json::Map::<String, Value>::new();
    crate::codec::put_number(&mut low_fields, crate::commands::PROTOCOL_FIELD, 0);
    crate::codec::put_text(&mut low_fields, crate::commands::EVENT_FIELD, &crate::event::event_kind(&crate::event::Event::Hello(sent.to_string(), session.to_string(), crate::commands::PROTOCOL_VERSION)));
    crate::codec::put_text(&mut low_fields, crate::commands::HELLO_NONCE_FIELD, &sent);
    crate::codec::put_text(&mut low_fields, crate::commands::HELLO_SESSION_FIELD, &session);
    crate::codec::put_number(&mut low_fields, crate::commands::HELLO_VERSION_FIELD, crate::commands::PROTOCOL_VERSION);
    let low_line = serde_json::Value::Object(low_fields).to_string();
    // The line IS decodable: the codec reads `v:0` and hands the `hello` through,
    // so the outcome below can only come from this module's own comparison.
    let decodable = crate::codec::decode_line(&low_line);
    match decodable {
        Err(_) => { assert!(false); }
        Ok(_) => { }
    }
    let wanted_low = HandshakeCheck::VersionMismatch(0, crate::commands::PROTOCOL_VERSION);
    assert!(check_handshake_answer(&sent, &low_line) == wanted_low);
    // A line that is not one JSON object at all stays the OTHER outcome.
    let foreign = check_handshake_answer(&sent, "I really am not a daemon\n");
    let wanted_unreadable = HandshakeCheck::UnreadableAnswer(CodecError::UnparseableLine);
    assert!(foreign == wanted_unreadable);
    // A peer that answers a command line, or a different event, is "not our
    // daemon" and is named by what it answered.
    let answered_command = check_handshake_answer(&sent, &version_request_frame(&sent));
    assert!(answered_command == HandshakeCheck::NotHello(crate::command::command_name(&crate::command::Command::Version(sent.to_string())).to_string()));
    let answered_event = check_handshake_answer(&sent, "{\"v\":1,\"event\":\"log\",\"level\":\"info\",\"text\":\"hi\"}\n");
    assert!(answered_event == HandshakeCheck::NotHello(crate::event::event_kind(&crate::event::Event::Log("info".to_string(), "hi".to_string())).to_string()));
    // A hello echoing another nonce says so with both nonces.
    let other_nonce = crate::codec::encode_frame(&crate::codec::Message::Outbound(crate::event::Event::Hello("0000000000000000".to_string(), session.to_string(), crate::commands::PROTOCOL_VERSION)));
    match other_nonce {
        Err(_) => { assert!(false); }
        Ok(echoed_other) => {
            let wanted_echo = HandshakeCheck::NotEchoed(sent.to_string(), "0000000000000000".to_string());
            assert!(check_handshake_answer(&sent, &echoed_other) == wanted_echo);
        }
    }
}

#[test]
fn the_session_id_of_a_daemon_carries_its_lock_nonce() {
    // Decision 28: plan item 3.6 hands `interpret::run_session` a per-instance
    // id derived from the nonce of `state/core.lock` instead of DEFAULT_SESSION,
    // so several daemons of one data directory are told apart by the `session`
    // field of their handshake. RECORDED DEVIATION (`core-protocol.md:74` shows
    // the informational value `core-1`): the doc's example is NOT what this
    // build writes, and the check ignores the field, so the doc's own answer
    // example still echoes; a doc amendment is owed to the orchestrator.
    let first = session_id_of("6f2c1a48e0b7d9c3");
    let second = session_id_of("6f2c1a48e0b7d9c4");
    assert!(first != second);
    assert!(first == "core-6f2c1a48e0b7d9c3".to_string());
    assert!(first != "core-1".to_string());
    // And the check does not care, which is what makes the deviation harmless
    // to the doc's example pair.
    let answer = crate::codec::encode_frame(&crate::codec::Message::Outbound(crate::event::Event::Hello("6f2c1a48e0b7d9c3".to_string(), "core-1".to_string(), crate::commands::PROTOCOL_VERSION)));
    match answer {
        Err(_) => { assert!(false); }
        Ok(frame) => { assert!(check_handshake_answer("6f2c1a48e0b7d9c3", &frame) == HandshakeCheck::Echoed); }
    }
}
