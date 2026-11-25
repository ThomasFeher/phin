//! The mbox reader of plan item 4.5: Thunderbird-style mailbox files.
//!
//! The contract is TEXT IN, TEXT OUT — no file system access and no process
//! running anywhere in this module (plan ground rule "Test the domain, not the
//! process"), which is what makes the mailbox rules testable without a mailbox
//! file. Reading files, deciding what to do with the messages, and the
//! `state/markers.csv` marker that makes an import INCREMENTAL are all plan
//! item 4.8's job; this module only tells a caller where the messages of a text
//! begin and end, and what one message holds.
//!
//! Three rules, and only three, shape the split (plan item 4.5, "split on
//! `^From ` lines"):
//!
//! * A line that starts `From ` **at column 0** opens a message. Everything else
//!   — the same text inside a header value, inside a body line, or inside a MIME
//!   part — is ordinary content.
//! * A body line the writer escaped as `>From ` is the mailbox' way of storing a
//!   `From ` line WITHOUT opening a second message. It never splits anything,
//!   and the text this module exposes has it un-escaped back to `From `. The
//!   byte offsets stay exactly the bytes of the file either way, because the
//!   incremental resume of plan item 4.8 hands back the offset it stopped at.
//! * [`Message::end`] is the byte offset at which a message ENDS, i.e. the start
//!   of the next message (the end of the text for the last one): handing it
//!   to [`messages_from`] yields only what follows. That is the `offset` value a
//!   [`crate::technical::state::Marker`] records for a mailbox file (plan item
//!   2.7), and the reason this module does not read or write that marker itself.
//!
//! RFC 2045/2046 MIME parts are reached part-way: enough to decode a file
//! attachment ([`Attachment`]) and to read its name, and enough to recurse into
//! the `multipart/alternative` nesting PayPal-style mail sends. A malformed part
//! is skipped silently — the plan does not ask for reports about it, and a bad
//! attachment must not cost the caller the message.
//!
//! RFC 822 date text is parsed HERE, in [`parse_rfc822_date`], and deliberately
//! not in [`crate::domain::date::parse`]: that function stays strictly
//! `DD.MM.YYYY`/`YYYY-MM-DD` (Phase 1 decision 7), and a header date of a
//! mailbox is a different, mailbox-specific input (planning decision 10).

use std::collections::HashMap;

/// A decoded file attachment of one [`Message`]: the name the MIME headers name
/// it under, plus its decoded bytes.
#[derive(Debug, Eq, PartialEq)]
pub struct Attachment {
    /// The file name as the MIME headers name it (`filename=` of a
    /// `Content-Disposition`, or `name=` of a `Content-Type`); never a path, and
    /// empty only when no header named one — at which point there is no
    /// attachment at all (an attachment is a part naming a file).
    pub file_name: String,
    /// The part's content bytes: base64-decoded when the part says
    /// `Content-Transfer-Encoding: base64`, the body text bytes for any other
    /// transfer encoding (7bit/8bit/binary carry the content verbatim).
    pub bytes: Vec<u8>,
}

/// One message of an mbox file, with its byte position in the input.
#[derive(Debug, Eq, PartialEq)]
pub struct Message {
    /// The BYTE offset of the message's `From ` envelope line in the input text.
    pub start: usize,
    /// The BYTE offset at which this message ENDS: the start of the next
    /// message, or the end of the input for the last one. Handing it to
    /// [`messages_from`] resumes the file just after this message; that is the
    /// `offset` a `state/markers.csv` marker records (plan item 2.7).
    pub end: usize,
    /// The header fields between the envelope line and the body, in file order.
    /// A folded field is joined to its continuation line with a single space
    /// (RFC 2822 unfolding), and the field name keeps its file casing so a
    /// round-trip can reproduce the mailbox. Use [`header`] to read one.
    pub headers: Vec<(String, String)>,
    /// The body text of the message, `>From ` un-escaped where the file escaped
    /// it. For a multipart message this is the message's full MIME source —
    /// boundary lines and part headers included — with that un-escaping applied;
    /// the decoded parts live in [`Message::attachments`].
    pub body: String,
    /// The file attachments the message carries (empty for a single-part mail).
    pub attachments: Vec<Attachment>,
}

/// The mbox message separator: `From ` AT COLUMN 0 of a line opens a message.
/// Only this splits a mailbox; the same text anywhere else — a header value, a
/// body line, the inside of a MIME part — is ordinary content.
const SEPARATOR: &str = "From ";

/// The mbox quoting of a body line that would otherwise look like [`SEPARATOR`]:
/// the writer prefixes it with `>`, and the reader removes it again — without
/// ever treating such a line as a message separator.
const ESCAPED_SEPARATOR: &str = ">From ";

/// Month abbreviations of an RFC 822 date header, in the order of the calendar
/// (`Jan` = 1), lowercase because the header's month name is case-insensitive.
const MONTHS: [&str; 12] = ["jan", "feb", "mar", "apr", "may", "jun",
                            "jul", "aug", "sep", "oct", "nov", "dec"];

/// Split an mbox file into its messages.
///
/// The rules this call cannot break are documented on the module: split at a
/// column-0 `From ` line only, un-escape a `>From ` body line, and let
/// [`Message::end`] be the resume offset. Text BEFORE the first `From ` line is
/// no message and is dropped (the mbox format always starts one, and a file
/// that does not is not worth guessing about). Anything that does not look like
/// a message — no headers, no body, a body that starts right at a non-field
/// line — still yields a message with whatever it has, and never a panic.
pub fn split_messages(text: &str) -> Vec<Message> {
    return messages_in(text, 0, true);
}

/// The messages of an mbox file that start AT or after `from_offset` — the
/// incremental resume of plan item 4.5, reading the `offset` a
/// [`crate::technical::state::Marker`] recorded for the file (plan item 2.7).
///
/// Every message at or beyond `from_offset` is returned, so passing the
/// [`Message::end`] of the last message processed yields exactly the messages
/// that were appended since. An offset in the middle of a message (not every
/// message ends at the next message's start if the file was tampered with) is
/// treated as "this message is already known": the first message returned is
/// the next one that starts after it.
pub fn messages_from(text: &str, from_offset: usize) -> Vec<Message> {
    return messages_in(text, from_offset, false);
}

/// The value of the header field `name`, compared case-insensitively
/// (`Date`, `date`, `DATE` name the same field). The first such field wins, as
/// RFC 2822 requires; a field that occurs twice is a broken header, and guessing
/// which copy a reader meant is not the caller's problem.
pub fn header<'a>(message: &'a Message, name: &str) -> Option<&'a str> {
    return header_of(&message.headers, name);
}

/// The `X-Mozilla-Status` flag of a message, as Thunderbird stores it: four hex
/// digits (`0000` read, `0001` unread), and phin tolerates a `0x` prefix in case
/// a client writes one. [`None`] means NO flag: a header-less message and a
/// non-hex value are both "no status", never a guess of `0`, because plan
/// item 4.5 has to be able to tell an unread message from an unknown one.
pub fn x_mozilla_status(message: &Message) -> Option<u32> {
    let text = match header(message, "X-Mozilla-Status") {
        None => { return None; }
        Some(value) => { value.trim() }
    };
    let digits = match text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) {
        None => { text }
        Some(stripped) => { stripped.trim() }
    };
    return match u32::from_str_radix(digits, 16) {
        Err(_) => { None }
        Ok(flag) => { Some(flag) }
    };
}

/// The calendar date of an RFC 822 date header — the `Date:` header of a mailbox
/// — normalised to the `YYYY-MM-DD` of `doc/architecture/data-files.md`.
///
/// The pinned form is `DD MON YYYY HH:MM:SS (+|-ZZZZ)` with a two-digit day and
/// a three-letter month name in any casing (`02 Apr 2024 12:00:00 +0100`). The
/// date is the LOCAL calendar date the header prints, with its zone offset
/// ignored: a notification is identified with the transaction it describes, and
/// the transaction's date is local as well. Anything else — a missing field, a
/// one-digit day, an unknown month, an out-of-range time, a day the month does
/// not have (31 February, 29 February in a common year), trailing garbage — is
/// [`None`], never an approximation: this is the strictness of Phase 1 decision 7
/// and planning decision 10, which is why [`crate::domain::date::parse`] does not
/// see this form at all.
pub fn parse_rfc822_date(text: &str) -> Option<String> {
    let mut fields = text.split_whitespace();
    let (day_text, month_text, year_text, time_text, zone_text) = match (
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
    ) {
        (Some(day_text), Some(month_text), Some(year_text), Some(time_text), Some(zone_text)) => {
            (day_text, month_text, year_text, time_text, zone_text)
        }
        _ => { return None; }
    };
    // A form the pinned layout does not describe: one field too many.
    if fields.next().is_some() {
        return None;
    }
    let day = match two_ascii_digits(day_text) {
        None => { return None; }
        Some(day) => { day }
    };
    let month = match month_number(month_text) {
        None => { return None; }
        Some(month) => { month }
    };
    let year = match four_ascii_digits(year_text) {
        None => { return None; }
        Some(year) => { year }
    };
    // The day of the month the header names, not merely of any month: the
    // pinned form fixes day, month and year together, so an impossible calendar
    // date (31 February) is as little a date as a one-digit day is.
    if day < 1 || day > days_in_month(year, month) {
        return None;
    }
    if !valid_time(time_text) {
        return None;
    }
    if !valid_zone_offset(zone_text) {
        return None;
    }
    return Some(format!("{year:04}-{month:02}-{day:02}"));
}

// ---------------------------------------------------------------------------
// Splitting: byte-exact by design (the resume offset depends on it)
// ---------------------------------------------------------------------------

/// The messages of `text` whose start is at or after `from_offset`, sharing
/// [`split_messages`] and [`messages_from`] so their rules cannot drift apart.
///
/// `from_offset` is the resume position; `from_zero` is `true` for the whole-file
/// call, whose first separator sits at or after 0 anyway (so both calls find the
/// first one the same way).
fn messages_in(text: &str, from_offset: usize, from_zero: bool) -> Vec<Message> {
    let mut messages: Vec<Message> = Vec::new();
    let mut at = match next_separator(text, if from_zero { 0 } else { from_offset }) {
        None => { return messages; }
        Some(at) => { at }
    };
    while at < text.len() {
        // The end of this message is the start of the next one, or the end of the
        // text for the last message: that is the resume offset of
        // [`messages_from`], and the reason it can be handed back unchanged.
        let next = match next_separator(text, at + SEPARATOR.len()) {
            None => { text.len() }
            Some(next) => { next }
        };
        messages.push(message_of(text, at, next));
        at = next;
    }
    return messages;
}

/// The byte offset at which a line starting `From ` begins at or after `at`, or
/// [`None`] when no line does. The scan is over BYTES on purpose — the byte
/// offsets this returns are what a marker records — and it is correct for any
/// text: `'\n'` and `From ` are both ASCII, so a UTF-8 continuation byte (always
/// `>= 0x80`) can never be mistaken for either, and a split at these offsets
/// cannot cut a character.
fn next_separator(text: &str, at: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    if at >= bytes.len() {
        return None;
    }
    if starts_separator(bytes, at) {
        return Some(at);
    }
    let mut cursor = at;
    loop {
        let new_line = match find_byte(bytes, b'\n', cursor) {
            None => { return None; }
            Some(index) => { index }
        };
        let candidate = new_line + 1;
        if candidate >= bytes.len() {
            return None;
        }
        if starts_separator(bytes, candidate) {
            return Some(candidate);
        }
        cursor = candidate;
    }
}

/// True when the bytes at `at` open a message: `From ` there, and nothing before
/// it on its line. An offset at the very start of the text has no line before
/// it, so it sits at column 0 by definition.
fn starts_separator(bytes: &[u8], at: usize) -> bool {
    if bytes.len() - at < SEPARATOR.len() {
        return false;
    }
    if &bytes[at..at + SEPARATOR.len()] != SEPARATOR.as_bytes() {
        return false;
    }
    return at == 0 || bytes[at - 1] == b'\n';
}

/// The byte offset of the first `needle` at or after `at`, or [`None`] when the
/// bytes hold none. The byte-oriented search is safe here because `needle` is
/// ASCII [`u8`] — a UTF-8 continuation byte is never an ASCII byte, so it cannot
/// produce a false hit.
fn find_byte(bytes: &[u8], needle: u8, at: usize) -> Option<usize> {
    let mut index = at;
    while index < bytes.len() {
        if bytes[index] == needle {
            return Some(index);
        }
        index += 1;
    }
    return None;
}

// ---------------------------------------------------------------------------
// One message: envelope line, header block, body
// ---------------------------------------------------------------------------

/// Read one message out of `text[start..end]`, where the slice starts at the
/// message's `From ` envelope line and ends where the next message starts.
///
/// Nothing here can panic on a malformed message: a message with no header block
/// (its first line after the envelope is not a `Name: value` field) ends up with
/// no headers and a body that starts at that line, and a message with nothing
/// after the envelope line has an empty body and no attachments.
fn message_of(text: &str, start: usize, end: usize) -> Message {
    let region = match text.get(start..end) {
        None => { return Message { start, end, headers: Vec::new(), body: String::new(), attachments: Vec::new() }; }
        Some(region) => { region }
    };
    // The envelope line is the separator itself; the first header line, if any,
    // starts behind it.
    let envelope_end = match line_end(region, 0) {
        None => { return Message { start, end, headers: Vec::new(), body: String::new(), attachments: Vec::new() }; }
        Some(at) => { at + 1 }
    };
    let (headers, body_start) = parse_header_block(region, envelope_end);
    let attachments = attachments_of(header_of(&headers, "Content-Type"), region, body_start);
    return Message { start, end, headers, body: unescape_body(&region[body_start..]), attachments };
}

/// Parse the header block that begins at `block_start` of `text`: contiguous
/// `Name: value` lines up to the blank line that separates headers from a body
/// (RFC 2822). A continuation line starting with a space or a tab is folded onto
/// the PREVIOUS field with a single space (the pinned unfolding rule); a
/// continuation line with nothing to continue is dropped, since which field it
/// meant cannot be guessed.
///
/// Returns the fields and the byte offset in `text` where the BODY starts: just
/// behind the blank line, or at the first line that is not a field (a header-less
/// body), or at the end of the text.
fn parse_header_block(text: &str, block_start: usize) -> (Vec<(String, String)>, usize) {
    let mut headers: Vec<(String, String)> = Vec::new();
    let mut at = block_start;
    while at < text.len() {
        let line_end = match line_end(text, at) {
            None => { text.len() }
            Some(line_end) => { line_end }
        };
        let line = &text[at..line_end];
        if line.len() == 0 {
            // The blank line ends the header block; the body starts behind it.
            let body_start = if line_end < text.len() { line_end + 1 } else { text.len() };
            return (headers, body_start);
        }
        let first = match line.as_bytes().first() {
            None => { return (headers, line_end); }
            Some(first) => { *first }
        };
        if first == b' ' || first == b'\t' {
            match headers.last_mut() {
                None => { }
                Some(field) => {
                    field.1.push(' ');
                    field.1.push_str(line.trim());
                }
            }
            at = next_line_of(text, line_end);
            continue;
        }
        match field_of(line) {
            None => {
                // Not a field: it belongs to the body of a message that has none.
                return (headers, at);
            }
            Some(field) => { headers.push(field); }
        }
        at = next_line_of(text, line_end);
    }
    return (headers, text.len());
}

/// The `(name, value)` of a `Name: value` header line, the name kept as the file
/// writes it and the value trimmed; [`None`] when the line holds no colon (or its
/// colon sits at the start), so the line is not a field at all.
fn field_of(line: &str) -> Option<(String, String)> {
    let colon = match line.find(':') {
        None => { return None; }
        Some(colon) => { colon }
    };
    let name = line[..colon].trim();
    if name.len() == 0 {
        return None;
    }
    return Some((name.to_string(), line[colon + 1..].trim().to_string()));
}

/// The byte offset of the line terminator at or after `at`, or [`None`] when the
/// text holds no further newline. A `\r\n` line ending is stored as-is by the
/// fixtures and by Thunderbird files phin reads, so `\n` alone is the line
/// terminator here; a stray `\r` is content, trimmed away by the header and
/// value helpers.
fn line_end(text: &str, at: usize) -> Option<usize> {
    return find_byte(text.as_bytes(), b'\n', at);
}

/// The offset of the line AFTER the one ending at `line_end` (which is the
/// offset of its `'\n'`); the end of the text when there is no next line.
fn next_line_of(text: &str, line_end: usize) -> usize {
    return if line_end < text.len() { line_end + 1 } else { text.len() };
}

/// The body text of a message, with every line the mailbox writer escaped as
/// `>From ` un-escaped back to `From ` (the mbox quoting rule). Every other line
/// is kept verbatim, and the newline structure is kept as it is in the file —
/// this is the ONLY rewriting the body undergoes.
fn unescape_body(body: &str) -> String {
    if !body.contains(ESCAPED_SEPARATOR) {
        return body.to_string();
    }
    let mut unescaped = String::with_capacity(body.len());
    for line in body.split_inclusive('\n') {
        // split_inclusive keeps the terminator, so the line structure survives.
        let (content, terminator) = match line.strip_suffix('\n') {
            None => { (line, "") }
            Some(content) => { (content, "\n") }
        };
        match content.strip_prefix(ESCAPED_SEPARATOR) {
            None => { unescaped.push_str(content); }
            Some(rest) => { unescaped.push_str(SEPARATOR); unescaped.push_str(rest); }
        }
        unescaped.push_str(terminator);
    }
    return unescaped;
}

/// The value of `name` among `headers`, compared case-insensitively; the first
/// occurrence wins (RFC 2822 forbids repeated single-instance fields, so a second
/// one is a broken header and guessing is not the caller's business).
fn header_of<'a>(headers: &'a Vec<(String, String)>, name: &str) -> Option<&'a str> {
    for field in headers {
        if field.0.eq_ignore_ascii_case(name) {
            return Some(field.1.as_str());
        }
    }
    return None;
}

// ---------------------------------------------------------------------------
// RFC 822 date pieces
// ---------------------------------------------------------------------------

/// The month number of a three-letter month name (`jan` = 1, any casing),
/// [`None`] when it is not a month name.
fn month_number(text: &str) -> Option<u32> {
    let lowered = text.to_ascii_lowercase();
    if lowered.len() != 3 {
        return None;
    }
    return match MONTHS.iter().position(|month| **month == *lowered) {
        None => { None }
        Some(index) => {
            match u32::try_from(index + 1) {
                Err(_) => { None }
                Ok(number) => { Some(number) }
            }
        }
    };
}

/// The number of days the `month` (1 = January) of `year` has: 31 days for the
/// first, third, fifth, seventh, eighth, tenth and twelfth month, 30 for the
/// fourth, sixth, ninth and eleventh, and for February the 29 of a leap year or
/// the 28 of any other year. A `month` outside 1..=12 has NO days at all, so no
/// day of the month fits it — [`month_number`] never yields such a number, so
/// that arm only keeps this function total.
fn days_in_month(year: u32, month: u32) -> u32 {
    return match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => { 31 }
        4 | 6 | 9 | 11 => { 30 }
        2 => {
            if is_leap_year(year) { 29 } else { 28 }
        }
        _ => { 0 }
    };
}

/// True when `year` is a leap year: divisible by 4, except divisible by 100
/// unless it is also divisible by 400 (so 2024 has a 29 February, 2023 does not,
/// and 2100 will not either).
fn is_leap_year(year: u32) -> bool {
    return year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
}

/// True when `text` is a plausible `HH:MM:SS` clock time (two digits, colon, two
/// digits, colon, two digits, and the three numbers in range).
fn valid_time(text: &str) -> bool {
    let bytes = text.as_bytes();
    if bytes.len() != 8 || bytes[2] != b':' || bytes[5] != b':' {
        return false;
    }
    for at in [0usize, 1, 3, 4, 6, 7] {
        if !bytes[at].is_ascii_digit() {
            return false;
        }
    }
    return two_digits(bytes, 0) <= 23 && two_digits(bytes, 3) <= 59 && two_digits(bytes, 6) <= 59;
}

/// True when `text` is a `(+|-)ZZZZ` zone offset as RFC 822 prints it. Its VALUE
/// is never applied — the date phin stores is the local calendar date the header
/// states — but a broken offset means the header is not the pinned form either.
fn valid_zone_offset(text: &str) -> bool {
    let bytes = text.as_bytes();
    if bytes.len() != 5 || (bytes[0] != b'+' && bytes[0] != b'-') {
        return false;
    }
    for at in [1usize, 2, 3, 4] {
        if !bytes[at].is_ascii_digit() {
            return false;
        }
    }
    return true;
}

/// The value of `text` when it is EXACTLY two ASCII digits, else [`None`].
fn two_ascii_digits(text: &str) -> Option<u32> {
    let bytes = text.as_bytes();
    if bytes.len() != 2 || !bytes[0].is_ascii_digit() || !bytes[1].is_ascii_digit() {
        return None;
    }
    return Some(two_digits(bytes, 0));
}

/// The value of `text` when it is EXACTLY four ASCII digits, else [`None`].
fn four_ascii_digits(text: &str) -> Option<u32> {
    let bytes = text.as_bytes();
    if bytes.len() != 4 {
        return None;
    }
    for position in [0usize, 1, 2, 3] {
        if !bytes[position].is_ascii_digit() {
            return None;
        }
    }
    return Some(two_digits(bytes, 0) * 100 + two_digits(bytes, 2));
}

/// The value of the two ASCII digit BYTES at `at` of `bytes`; the caller has
/// already checked both are digits. Split out because [`two_ascii_digits`] and
/// [`valid_time`] both need it, and no `as` conversion is involved.
fn two_digits(bytes: &[u8], at: usize) -> u32 {
    return (u32::from(bytes[at]) - u32::from(b'0')) * 10 + (u32::from(bytes[at + 1]) - u32::from(b'0'));
}

// ---------------------------------------------------------------------------
// MIME parts and their attachments
// ---------------------------------------------------------------------------

/// One part of a multipart message: the fields between its delimiter lines and
/// the content body behind them (the blank line is gone).
struct Part {
    headers: Vec<(String, String)>,
    body: String,
}

/// The file attachments a message carries.
///
/// The message's `Content-Type` names the top-level boundary; without one there
/// is nothing to look for (the plan's "boundary-less message": its body is the
/// message's own body and it has no attachments). Multipart mail nests — a
/// `multipart/mixed` around a `multipart/alternative` around the parts is how
/// PayPal-style notifications and Thunderbird both send, and the PDF fixture one
/// level deeper — so the walk descends, bounded by [`MAX_MIME_NESTING`].
fn attachments_of(content_type: Option<&str>, region: &str, body_start: usize) -> Vec<Attachment> {
    let mut attachments: Vec<Attachment> = Vec::new();
    let boundary = match content_type {
        None => { return attachments; }
        Some(value) => {
            match boundary_of(value) {
                None => { return attachments; }
                Some(boundary) => { boundary }
            }
        }
    };
    collect_attachments(region, body_start, &boundary, MAX_MIME_NESTING, &mut attachments);
    return attachments;
}

/// Walk the MIME parts of `text[from..]` delimited by `boundary`, collecting
/// every part that is a file ([`attachment_of`]) and descending into nested
/// `multipart/*` parts until `depth` runs out (the bound that keeps a malformed
/// mailbox from walking forever).
fn collect_attachments(text: &str, from: usize, boundary: &str, depth: usize, out: &mut Vec<Attachment>) {
    for part in mime_parts(text, from, boundary) {
        let part_type = match header_of(&part.headers, "Content-Type") {
            None => { "" }
            Some(value) => { value }
        };
        if part_type.to_ascii_lowercase().starts_with("multipart/") {
            match boundary_of(part_type) {
                None => { }
                Some(inner) => {
                    if depth > 0 {
                        collect_attachments(&part.body, 0, &inner, depth - 1, out);
                    }
                }
            }
            continue;
        }
        match attachment_of(&part) {
            None => { }
            Some(attachment) => { out.push(attachment); }
        }
    }
}

/// The largest number of MIME nesting levels this module walks. Real mailboxes
/// nest a handful of levels; the bound exists so that a malformed
/// `multipart/*` chain cannot walk forever.
const MAX_MIME_NESTING: usize = 4;

/// The one part of a message that is a file attachment: it names a file (a
/// `filename=`/`name` parameter, the pinned source of the name) and it is not a
/// `text/*` part (the visible text of a notification is its body, not an
/// attachment). A part without a name — the `text/plain` and `text/html` parts
/// of a PayPal notification — is nobody's attachment, and a part whose base64 is
/// broken is skipped SILENTLY (the plan requires no report for that case).
fn attachment_of(part: &Part) -> Option<Attachment> {
    let part_type = match header_of(&part.headers, "Content-Type") {
        None => { "" }
        Some(value) => { value }
    };
    if part_type.to_ascii_lowercase().starts_with("text/") {
        return None;
    }
    let file_name = match part_file_name(part) {
        None => { return None; }
        Some(file_name) => { file_name }
    };
    let bytes = match part_content_bytes(part) {
        None => { return None; }
        Some(bytes) => { bytes }
    };
    return Some(Attachment { file_name, bytes });
}

/// The file name a part is stored under: `filename=` or `name=` of its
/// `Content-Disposition` first (that is the header that carries the STORE name),
/// then the same parameters of its `Content-Type` (`name=bank_b1.pdf`). The
/// fixture writes both, and every reader has to pick an order, so the
/// pinned order is the disposition, then the type.
fn part_file_name(part: &Part) -> Option<String> {
    let disposition = match header_of(&part.headers, "Content-Disposition") {
        None => { "" }
        Some(value) => { value }
    };
    let content_type = match header_of(&part.headers, "Content-Type") {
        None => { "" }
        Some(value) => { value }
    };
    for candidate in [disposition, content_type] {
        let parameters = parameters(candidate);
        for key in ["filename", "name"] {
            match parameters.get(key) {
                None => { }
                Some(file_name) => {
                    if file_name.len() > 0 {
                        return Some(file_name.clone());
                    }
                }
            }
        }
    }
    return None;
}

/// The content BYTES of a part: base64-decoded when the part says
/// `Content-Transfer-Encoding: base64` ([`decode_base64`] failing means the part
/// is skipped, so this returns [`None`]), and the body text bytes as written for
/// any other (or missing) transfer encoding — 7bit, 8bit and binary carry their
/// content verbatim, minus the newline the MIME framing adds before the next
/// delimiter.
fn part_content_bytes(part: &Part) -> Option<Vec<u8>> {
    match header_of(&part.headers, "Content-Transfer-Encoding") {
        None => { return Some(trimmed_body_bytes(&part.body)); }
        Some(encoding) => {
            if encoding.trim().eq_ignore_ascii_case("base64") {
                return match decode_base64(&part.body) {
                    None => { None }
                    Some(bytes) => { Some(bytes) }
                };
            }
            return Some(trimmed_body_bytes(&part.body));
        }
    }
}

/// The body text of a non-base64 part as bytes: the framing newline (or several)
/// the MIME delimiters add at its end is removed, everything else is kept.
fn trimmed_body_bytes(body: &str) -> Vec<u8> {
    return body.trim_end_matches('\n').as_bytes().to_vec();
}

/// The MIME parts of `text[from..]`, delimited by `--<boundary>` lines
/// (RFC 2046: the delimiter is `--` plus the boundary, and `--<boundary>--` ends
/// the MIME body). Text before the first delimiter (the preamble mail clients
/// leave for readers that do not understand multipart) and the epilogue behind
/// the closing delimiter are not parts, and a message whose body mentions the
/// boundary nowhere has NO parts — which is the plan's boundary-less case.
fn mime_parts(text: &str, from: usize, boundary: &str) -> Vec<Part> {
    let delimiter = format!("--{boundary}");
    let lines = lines_with_offsets(text, from);
    let mut parts: Vec<Part> = Vec::new();
    let mut index = 0usize;
    while index < lines.len() && !lines[index].1.starts_with(&delimiter) {
        index += 1;
    }
    while index < lines.len() {
        let line = lines[index].1;
        if line.ends_with("--") {
            // The closing delimiter: everything behind it is the MIME epilogue.
            return parts;
        }
        let content_first = index + 1;
        let mut stop = content_first;
        while stop < lines.len() && !lines[stop].1.starts_with(&delimiter) {
            stop += 1;
        }
        let part_end = match lines.get(stop) {
            None => { text.len() }
            Some((offset, _)) => { *offset }
        };
        let part_start = match lines.get(content_first) {
            None => { part_end }
            Some((offset, _)) => { *offset }
        };
        if part_start < part_end {
            let part_text = match text.get(part_start..part_end) {
                None => { "" }
                Some(part_text) => { part_text }
            };
            let (headers, content_start) = parse_header_block(part_text, 0);
            parts.push(Part { headers, body: part_text[content_start..].to_string() });
        }
        index = stop;
    }
    return parts;
}

/// The lines of `text` starting at `from`, each paired with its BYTE offset in
/// `text` — the offsets are what let a part be sliced out of the message text
/// without re-serialising it.
fn lines_with_offsets(text: &str, from: usize) -> Vec<(usize, &str)> {
    let mut lines: Vec<(usize, &str)> = Vec::new();
    let mut at = from;
    while at < text.len() {
        let end = match line_end(text, at) {
            None => { text.len() }
            Some(end) => { end }
        };
        lines.push((at, &text[at..end]));
        at = next_line_of(text, end);
    }
    return lines;
}

/// The `boundary=` parameter of a `Content-Type` value, i.e. the delimiter of a
/// multipart message ([`boundary_of`] reads it out of the header value); the
/// boundary value itself carries no dashes, and the delimiter line prepends the
/// two dashes RFC 2046 fixes there. [`None`] for a single-part message.
fn boundary_of(content_type: &str) -> Option<String> {
    return match parameters(content_type).get("boundary") {
        None => { None }
        Some(boundary) => {
            if boundary.len() == 0 {
                return None;
            }
            Some(boundary.clone())
        }
    };
}

/// The `parameter=value` pairs of a header value, keyed LOWERCASE (`boundary`,
/// `name`, `filename`, `charset` — RFC 2045 makes parameter names case-insensitive)
/// and with the quoting removed, so `attachment; filename="a, b.pdf"` and
/// `multipart/mixed; boundary=X` land in one map. The value itself (the media
/// type, the first piece) is not a pair and is skipped; a piece with no `=` is
/// skipped too, since guessing its meaning is not this module's job.
fn parameters(value: &str) -> HashMap<String, String> {
    let mut found: HashMap<String, String> = HashMap::new();
    let mut pieces: Vec<String> = Vec::new();
    let mut piece = String::new();
    let mut quoted = false;
    for character in value.chars() {
        if character == '"' {
            quoted = !quoted;
            continue;
        }
        if character == ';' && !quoted {
            pieces.push(piece.clone());
            piece.clear();
            continue;
        }
        piece.push(character);
    }
    pieces.push(piece);
    for piece in pieces.iter().skip(1) {
        let equal = match piece.find('=') {
            None => { continue; }
            Some(equal) => { equal }
        };
        let key = piece[..equal].trim().to_ascii_lowercase();
        if key.len() == 0 {
            continue;
        }
        found.insert(key, piece[equal + 1..].trim().to_string());
    }
    return found;
}

// ---------------------------------------------------------------------------
// base64
// ---------------------------------------------------------------------------

/// The bytes of a base64 part body, ignoring the line breaks the MIME framing
/// inserts (the fixture wraps at 76 characters per RFC 2045).
///
/// [`None`] says the body is NOT base64: an unexpected character, a number of
/// leftover 6-bit groups that cannot make a byte, or content behind the `=`
/// padding. The caller SKIPS such a part silently — the plan pins that (a broken
/// attachment is not worth a complaint, and a half-decoded attachment is not
/// worth bytes nobody asked for).
fn decode_base64(text: &str) -> Option<Vec<u8>> {
    let mut bytes: Vec<u8> = Vec::new();
    let mut buffer: u32 = 0;
    let mut groups: u32 = 0;
    let mut padded = false;
    for character in text.chars() {
        if character == '\n' || character == '\r' || character == ' ' || character == '\t' {
            continue;
        }
        if character == '=' {
            padded = true;
            continue;
        }
        if padded {
            // Data behind the padding: no base64 alphabet is meant that way.
            return None;
        }
        let value = match base_six_value(character) {
            None => { return None; }
            Some(value) => { value }
        };
        buffer = (buffer << 6) | value;
        groups += 1;
        if groups == 4 {
            bytes.push(low_byte(buffer >> 16));
            bytes.push(low_byte(buffer >> 8));
            bytes.push(low_byte(buffer));
            buffer = 0;
            groups = 0;
        }
    }
    if groups == 1 {
        // Two leftover bits of alphabet and no byte to build from them.
        return None;
    }
    if groups > 0 {
        // The tail of the message: 2 or 3 groups holding 1 or 2 final bytes,
        // with the base64 padding bits below those bytes shifted out.
        let whole_bytes = (groups * 6) / 8;
        let shift = (groups * 6) - (whole_bytes * 8);
        let mut index = whole_bytes;
        while index > 0 {
            index -= 1;
            bytes.push(low_byte(buffer >> (shift + (index * 8))));
        }
    }
    return Some(bytes);
}

/// The 6-bit value of a base64 alphabet character, [`None`] for anything else.
fn base_six_value(character: char) -> Option<u32> {
    return match character {
        'A'..='Z' => { Some(u32::from(character) - u32::from('A')) }
        'a'..='z' => { Some(u32::from(character) - u32::from('a') + 26) }
        '0'..='9' => { Some(u32::from(character) - u32::from('0') + 52) }
        '+' => { Some(62) }
        '/' => { Some(63) }
        _ => { None }
    };
}

/// The low eight bits of `value` as a byte. Masking first makes the conversion
/// total, so the `try_from` cannot fail and its `Err` arm is a fall-back value,
/// never a panic (the task pins `as` conversions out of code like this).
fn low_byte(value: u32) -> u8 {
    return match u8::try_from(value & 0xFF) {
        Ok(byte) => { byte }
        Err(_) => { 0 }
    };
}

// ---------------------------------------------------------------------------
// Tests. The module itself stays text-in/text-out; these tests only READ the
// testdata fixtures (plus the fixture PDF behind the attachment test), and
// write nothing anywhere — so they need no scratch directory at all, and a
// future one would have to use a module-private parent (e.g.
// SCRATCH_PARENT = "test-scratch-mbox"), never the shared `test-scratch/`.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::messages_from;
    use super::split_messages;
    use super::header;
    use super::x_mozilla_status;
    use super::Attachment;
    use super::Message;

    /// Locate `testdata/` relative to the test's working directory.
    ///
    /// Copied from the same pattern `crate::technical::id` and
    /// `crate::technical::importer_host` use: `cargo test` runs the test
    /// binary with the package root (`core/`) as its working directory, so the
    /// fixture directory sits one level up; the search walks up the parents
    /// trying only `testdata` per directory.
    fn find_testdata_dir() -> std::path::PathBuf {
        let start = std::env::current_dir().unwrap_or(std::path::PathBuf::new());
        let mut dir = start;
        loop {
            let candidate = dir.join("testdata");
            if candidate.is_dir() { return candidate; }
            match dir.parent() {
                None => { panic!("mbox tests cannot find testdata relative to the test's working directory"); }
                Some(parent) => { dir = parent.to_path_buf(); }
            }
        }
    }

    /// The fixture `name` under `testdata/` read to text. The fixtures are
    /// repository data this module never modifies (plan ground rule: fixtures
    /// are generated, not edited, and never by the reader anyway).
    fn fixture_text(name: &str) -> String {
        let path = find_testdata_dir().join(name);
        return match std::fs::read_to_string(&path) {
            Err(reason) => { panic!("cannot read fixture {}: {reason}", path.display()); }
            Ok(text) => { text }
        };
    }

    /// The fixture `name` under `testdata/` read as raw BYTES.
    fn fixture_bytes(name: &str) -> Vec<u8> {
        let path = find_testdata_dir().join(name);
        return match std::fs::read(&path) {
            Err(reason) => { panic!("cannot read fixture {}: {reason}", path.display()); }
            Ok(bytes) => { bytes }
        };
    }

    /// The lowercase hex SHA-256 of these BYTES: the same digest
    /// `crate::technical::id::sha256_hex` computes, but over bytes, because an
    /// attachment is arbitrary binary and not a `&str`.
    fn sha256_hex_of(bytes: &[u8]) -> String {
        use sha2::Digest;
        let digest = sha2::Sha256::digest(bytes);
        let mut hex = String::new();
        for byte in digest.iter() {
            hex.push_str(&format!("{:02x}", *byte));
        }
        return hex;
    }

    /// The byte offsets of a message list covering `text` from the very first
    /// message to the very end: each `end` the next `start`, the last `end` the
    /// end of the text — the exactness the resume offset depends on.
    fn assert_offsets_are_exact(messages: &Vec<Message>, text: &str) {
        assert!(messages.len() > 0, "expected at least one message");
        assert!(messages[0].start == 0, "the first message starts at byte 0, got {}", messages[0].start);
        for index in 0..messages.len() - 1 {
            assert!(messages[index].end == messages[index + 1].start, "message {index} must end where the next one starts");
        }
        assert!(messages[messages.len() - 1].end == text.len(), "the last end is the resume offset at the text's end");
    }

    #[test]
    fn fixture_paypal_yields_exactly_8_messages() {
        let text = fixture_text("mail_paypal.mbox");
        let messages = split_messages(&text);
        assert!(messages.len() == 8, "mail_paypal.mbox must yield 8 messages, got {}", messages.len());
        assert_offsets_are_exact(&messages, &text);
        // Every message carries a Date header of the pinned RFC 822 form.
        for message in &messages {
            let date = match header(message, "Date") {
                None => { panic!("a fixture message has no Date header: {message:?}"); }
                Some(date) => { date }
            };
            assert!(super::parse_rfc822_date(date).is_some(), "fixture Date header is not the pinned RFC 822 form: {date}");
        }
    }

    #[test]
    fn fixture_with_pdf_yields_exactly_12_messages() {
        let text = fixture_text("mail_with_pdf.mbox");
        let messages = split_messages(&text);
        assert!(messages.len() == 12, "mail_with_pdf.mbox must yield 12 messages, got {}", messages.len());
        assert_offsets_are_exact(&messages, &text);
    }

    #[test]
    fn attachment_decoded() {
        let text = fixture_text("mail_with_pdf.mbox");
        let messages = split_messages(&text);
        // Exactly ONE attachment across the whole thread: the application/pdf
        // part of the thread's fourth message; the text/plain and text/html
        // parts of a multipart are body, not attachment.
        let attachments: Vec<&Attachment> = messages.iter().flat_map(|message| message.attachments.iter()).collect();
        let names: Vec<String> = attachments.iter().map(|attachment| attachment.file_name.clone()).collect();
        assert!(attachments.len() == 1, "exactly one attachment expected across 12 messages, got names: {names:?}");
        let attachment = attachments[0];
        assert!(attachment.file_name == "bank_b1.pdf", "file_name: {}", attachment.file_name);
        assert!(attachment.bytes.starts_with(b"%PDF"), "decoded bytes must be a PDF; the first bytes are not %PDF");
        // Byte-identical to the generated fixture PDF: the README's claim,
        // proven here on the digest, not on length.
        let pdf_bytes = fixture_bytes("bank_b1.pdf");
        assert!(sha256_hex_of(&attachment.bytes) == sha256_hex_of(&pdf_bytes), "the decoded attachment is not the fixture bank_b1.pdf");
    }

    #[test]
    fn escaped_from_does_not_split() {
        let text = fixture_text("mail_with_pdf.mbox");
        // The raw file stores the quoted form at column 0…
        assert!(text.contains("\n>From the internet: hello\n"), "the escaped From line is missing from the regenerated fixture");
        let messages = split_messages(&text);
        // …and the message count is STILL 12: the line never splits anything.
        assert!(messages.len() == 12, "the escaped From line must not split the mailbox, got {} messages", messages.len());
        assert_offsets_are_exact(&messages, &text);
        // Exactly one message body carries the line UN-ESCAPED back to a
        // column-0 "From "; no exposed body keeps the escape anywhere.
        let mut unescaped_bodies = 0;
        for message in &messages {
            assert!(!message.body.contains(">From "), "an exposed body must not keep the mbox escape: {:?}", message.body);
            if message.body.lines().any(|line| line == "From the internet: hello") {
                unescaped_bodies += 1;
            }
        }
        assert!(unescaped_bodies == 1, "exactly one body must carry the un-escaped From line, got {unescaped_bodies}");
        // The un-escaping never moves a byte the resume works with: on the
        // extended fixture, resuming at the last end yields nothing new and
        // resuming mid-file yields exactly the FOLLOWING messages.
        assert!(messages_from(&text, messages[11].end).is_empty());
        assert!(messages_from(&text, messages[5].end).len() == 6, "resume at message 5's end yields the 6 following messages");
    }

    #[test]
    fn x_mozilla_status_read() {
        let text = fixture_text("mail_paypal.mbox");
        let messages = split_messages(&text);
        assert!(messages.len() == 8);
        // Message 3 of the fixture is stored UNREAD (0001 ⇒ flag 1), and every
        // other message is stored read (0000 ⇒ flag 0).
        let statuses: Vec<Option<u32>> = messages.iter().map(x_mozilla_status).collect();
        let expected: Vec<Option<u32>> = vec![Some(0), Some(0), Some(1), Some(0), Some(0), Some(0), Some(0), Some(0)];
        assert!(statuses == expected, "X-Mozilla-Status flags: {statuses:?}");
        // Inline: the header name is case-insensitive and a 0x prefix is
        // tolerated…
        let one = &split_messages("From a@b\nx-mozilla-status: 0x00ad\n\nbody\n")[0];
        assert!(x_mozilla_status(one) == Some(0xAD), "0x prefix must be tolerated: {one:?}");
        // …while NO header, and a non-hex value, are None — never a guess of 0,
        // so an unknown flag stays distinguishable from a read one.
        let none = &split_messages("From a@b\nSubject: s\n\nbody\n")[0];
        assert!(x_mozilla_status(none) == None, "no header means no flag: {none:?}");
        let broken = &split_messages("From a@b\nX-Mozilla-Status: not hex\n\nbody\n")[0];
        assert!(x_mozilla_status(broken) == None, "a non-hex value means no flag: {broken:?}");
    }

    #[test]
    fn resume_from_offset() {
        // A three-message mbox inline; the \n escapes put each "From " back at
        // column 0 of the text.
        let text = "From anna@example.com Mon Jan 01 10:00:00 2024\nSubject: one\n\nfirst body\nFrom tobias@example.com Mon Jan 01 10:00:01 2024\nSubject: two\n\nsecond body\nFrom anna@example.com Mon Jan 01 10:00:02 2024\nSubject: three\n\nthird body\n";
        let messages = messages_from(text, 0);
        assert!(messages.len() == 3, "three messages expected, got {}", messages.len());
        assert_offsets_are_exact(&messages, text);
        // Resume at the end of the last message processed: exactly what was
        // APPENDED since comes back, with the same byte offsets.
        let rest = messages_from(text, messages[1].end);
        assert!(rest.len() == 1 && rest[0].start == messages[2].start && rest[0].end == messages[2].end, "the resumed message is the third one, offsets and all: {rest:?}");
        // Resume past the very end: nothing new.
        assert!(messages_from(text, messages[2].end).is_empty());
        // An offset INSIDE a message treats that message as already known: the
        // first message returned is the next one that STARTS after it.
        let mid = messages_from(text, messages[1].start + 4);
        assert!(mid.len() == 1 && mid[0].start == messages[2].start, "an in-message offset resumes at the NEXT message");
        // The whole-file split and the resume from 0 agree message for message.
        assert!(split_messages(text) == messages);
    }

    #[test]
    fn parse_rfc822_date() {
        assert!(super::parse_rfc822_date("02 Apr 2024 12:00:00 +0100") == Some("2024-04-02".to_string()));
        assert!(super::parse_rfc822_date("25 Dec 2023 23:59:59 -0500") == Some("2023-12-25".to_string()));
        // The month name is case-insensitive; the zone offset is checked but
        // never applied (the date is the local calendar date the header states).
        assert!(super::parse_rfc822_date("02 apr 2024 12:00:00 -0700") == Some("2024-04-02".to_string()));
        // Anything outside the pinned form is None, never an approximation.
        assert!(super::parse_rfc822_date("garbage") == None);
        assert!(super::parse_rfc822_date("") == None);
        assert!(super::parse_rfc822_date("2 Apr 2024 12:00:00 +0100") == None, "the pinned form has a TWO-digit day");
        assert!(super::parse_rfc822_date("02 Xyz 2024 12:00:00 +0100") == None, "unknown month name");
        assert!(super::parse_rfc822_date("02 Apr 2024 12:00:00") == None, "no zone field");
        assert!(super::parse_rfc822_date("02 Apr 2024 12:00:00 GMT") == None, "an abbreviation is not a numeric offset");
        assert!(super::parse_rfc822_date("02 Apr 2024 24:00:00 +0100") == None, "out of range hour");
        assert!(super::parse_rfc822_date("02 Apr 2024 12:60:00 +0100") == None, "out of range minute");
        assert!(super::parse_rfc822_date("32 Apr 2024 12:00:00 +0100") == None, "out of range day");
        // The day is capped by the MONTH, not by 31: an impossible calendar date
        // is no date, and February follows the leap-year rule.
        assert!(super::parse_rfc822_date("31 Feb 2024 12:00:00 +0100") == None, "February has no 31st, in a leap year or not");
        assert!(super::parse_rfc822_date("30 Feb 2024 12:00:00 +0100") == None, "February has no 30th, in a leap year or not");
        assert!(super::parse_rfc822_date("29 Feb 2024 12:00:00 +0100") == Some("2024-02-29".to_string()), "2024 is a leap year, so it has a 29 February");
        assert!(super::parse_rfc822_date("29 Feb 2023 12:00:00 +0100") == None, "2023 is not a leap year, so it has no 29 February");
        // A 31st in a month that HAS 31 days stays a date.
        assert!(super::parse_rfc822_date("31 Mar 2024 18:45:20 +0100") == Some("2024-03-31".to_string()), "31 March is an ordinary date");
        assert!(super::parse_rfc822_date("02 Apr 2024 12:00:00 +0100 extra") == None, "trailing garbage");
        assert!(super::parse_rfc822_date("02 Apr 20241 12:00:00 +0100") == None, "the year must be four digits");
    }

    #[test]
    fn header_less_and_boundary_less_messages_never_panic() {
        // Empty text, and text with no separator line, yield no message.
        assert!(split_messages("").is_empty());
        assert!(split_messages("no envelope line here\nat all\n").is_empty());
        // A HEADER-LESS message: the first line that is not a field opens the
        // body instead, and nothing panics.
        let messages = split_messages("From a@b Mon Jan 01 00:00:00 2024\nplain text at once\n\nmore\n");
        assert!(messages.len() == 1, "one header-less message, got {}", messages.len());
        assert!(messages[0].headers.is_empty(), "no header was there to read: {:?}", messages[0].headers);
        assert!(messages[0].body.starts_with("plain text at once"), "the body starts at the non-field line: {:?}", messages[0].body);
        // A message that is only the envelope line: no headers, no body, no panic.
        let messages = split_messages("From solo");
        assert!(messages.len() == 1 && messages[0].headers.is_empty() && messages[0].body.is_empty());
        // BOUNDARY-LESS: the Content-Type names multipart but the body has no
        // delimiter line — the message has no attachments, and no panic.
        let messages = split_messages("From a@b\nContent-Type: multipart/mixed; boundary=ABSENT\n\nno delimiters here\n");
        assert!(messages.len() == 1 && messages[0].attachments.is_empty(), "boundary-less message: {:?}", messages);
        // A malformed base64 part is SKIPPED SILENTLY (the pinned rule).
        let messages = split_messages("From a@b\nContent-Type: multipart/mixed; boundary=B\n\n--B\nContent-Type: application/octet-stream; name=\"bad.bin\"\nContent-Transfer-Encoding: base64\n\n!!! not base64 !!!\n\n--B--\n");
        assert!(messages.len() == 1 && messages[0].attachments.is_empty(), "a broken base64 part must vanish without a trace: {:?}", messages[0].attachments);
        // Text BEFORE the first From line is no message and is dropped.
        let messages = split_messages("junk preamble\nFrom a@b\n\nbody\n");
        assert!(messages.len() == 1 && messages[0].start == 14, "preamble dropped, message starts at 14: {:?}", messages.iter().map(|m| m.start).collect::<Vec<usize>>());
        // From-like text NOT at column 0 never splits: inside a header value,
        // indented in a body, at the very start of a LINE inside a MIME body,
        // and the ">From " escape itself stays one message.
        let messages = split_messages("From a@b\nX-Note: From the boss\n From indented\n>From escaped\n\nbody From inside\n");
        assert!(messages.len() == 1, "only column-0 From splits: {}", messages.len());
        assert!(messages[0].body.starts_with("From escaped\n"), "the escaped line is un-escaped, the rest of the body survives: {:?}", messages[0].body);
        assert!(messages[0].body.contains("body From inside"));
    }

    #[test]
    fn header_folding_and_case_insensitive_reads() {
        // Continuation lines starting with a space or a tab are joined onto the
        // PREVIOUS field with a single space (the pinned unfolding rule), and
        // header() is case-insensitive.
        let messages = split_messages("From a@b\nSubject: broken\n\tand folded \nX-Mozilla-Status: 0000\n\nbody\n");
        let message = match messages.first() {
            None => { panic!("one message expected: {messages:?}"); }
            Some(message) => { message }
        };
        assert!(header(message, "subject") == Some("broken and folded"), "unfolded Subject: {:?}", header(message, "SUBJECT"));
        assert!(header(message, "SUBJECT") == Some("broken and folded"));
        assert!(x_mozilla_status(message) == Some(0), "the flag header survives the folding: {:?}", message.headers);
        // The envelope line itself is never a header field.
        assert!(header(message, "From").is_none(), "the envelope line is not a header: {:?}", message.headers);
    }

    #[test]
    fn nested_multipart_is_walked_but_bounded() {
        // A multipart/mixed around a multipart/alternative around the parts —
        // the nesting PayPal-style and Thunderbird mail send — is descended far
        // enough to find the file part, and a text/plain part inside the inner
        // multipart is still nobody's attachment.
        let text = "From a@b\nContent-Type: multipart/mixed; boundary=OUT\n\n--OUT\nContent-Type: text/plain\n\nseen\n--OUT\nContent-Type: multipart/alternative; boundary=IN\n\n--IN\nContent-Type: text/html; name=\"not-an-attachment.html\"\n\n<b>hi</b>\n--IN\nContent-Type: application/octet-stream; name=\"inner.bin\"\nContent-Transfer-Encoding: base64\n\naGVsbG8=\n--IN--\n--OUT--\n";
        let messages = split_messages(text);
        assert!(messages.len() == 1, "one message expected, got {}", messages.len());
        let attachments = &messages[0].attachments;
        let names: Vec<String> = attachments.iter().map(|attachment| attachment.file_name.clone()).collect();
        assert!(names == ["inner.bin".to_string()], "the nested file part is found, and named text parts are not attachments: {names:?}");
        assert!(attachments.len() == 1 && attachments[0].bytes == b"hello".to_vec(), "the nested base64 decodes: {:?}", attachments);
    }
}
