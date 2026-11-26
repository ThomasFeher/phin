//! Mail annotation extraction of plan item 4.6: a mailbox yields ANNOTATIONS,
//! never transactions (rule R002, `doc/architecture/architecture.md` "Mail as
//! annotation"). Nothing in this module's public return type can name a
//! [`crate::domain::transaction::Transaction`]; the bank rows of an attached
//! invoice are parsed only to be FLATTENED into annotation item strings
//! (decision 19 and the compile-level proof test
//! `never_produces_transactions`).
//!
//! Two extraction paths read the messages [`crate::technical::mbox`] delivers:
//!
//! * **PayPal receipts** (`Subject:` exactly `Your PayPal payment receipt`):
//!   the `Betrag:` line of the body carries the fact amount and the `Grund:`
//!   line the counterparty hint. The amount keeps the PRINTED sign
//!   (amendment A5, which supersedes planning decision 9): `Betrag: 25,99` is
//!   stored `+2599`, `Betrag: -19,99` is stored `-1999` — the fixture's own
//!   `transactions.csv` rows copy that sign verbatim, and forcing every value
//!   negative would turn a client payment into an expense.
//! * **Invoice attachments**: a message with a file attachment gets an
//!   annotation carrying the attachment's statement rows as items. The bytes
//!   are written to a TEMP file named by their content hash (decision 20: a
//!   unique, collision-free name under `std::env::temp_dir()`; `pdftotext`
//!   needs a file path), run through [`crate::technical::pdf_text`] and
//!   [`crate::technical::parse_bank_b`], and the temp file is deleted again on
//!   every path — including the failing ones. An attachment whose extraction
//!   fails still yields the annotation, just with EMPTY items.
//!
//! The annotation itself carries no date/amount (`data-files.md` fixes the
//! `annotations.csv` columns), so [`AnnotatedMail`] pairs it with the FACTS
//! plan item 4.7 matches on (decision 19). The final confidence of linking is
//! NOT decided here: every extraction stamps [`CONF_EXTRACT`]. The annotation
//! id is `sha256(source|text|items)`-trimmed (decision 8) so relinking — which
//! changes `ref` and `confidence` — keeps the same id. Like the transaction
//! source, the provenance is normalized to the file BASENAME
//! ([`crate::technical::id::source_basename`]), so the same mailbox read
//! through a different path yields the same id.
//!
//! Reading the file, recording the `state/markers.csv` resume offset, and
//! linking are plan item 4.8's job; [`annotate_mbox`] only takes the offset a
//! marker would have stored and hands it to
//! [`crate::technical::mbox::messages_from`]. This is a technical module: the
//! temp-file and mailbox reads here are the only I/O, and both are required by
//! the task.

use std::path::Path;

use crate::domain::annotation::Annotation;
use crate::domain::annotation::clamp_confidence;
use crate::domain::annotation::items_to_text;
use crate::domain::money;
use crate::technical::id;
use crate::technical::mbox;
use crate::technical::mbox::Attachment;
use crate::technical::mbox::Message;
use crate::technical::parse_bank_b::parse_bank_b_text;
use crate::technical::pdf_text::extract_text;

/// The extraction confidence every annotation carries until plan item 4.7
/// assigns the FINAL confidence (0.0 for an orphan, per decision 5).
pub const CONF_EXTRACT: f64 = 0.9;

/// The number of lowercase hex characters an annotation id keeps from the
/// SHA-256 digest — the same 10-hex convention as the transaction id
/// (`doc/architecture/data-files.md`, `core/src/technical/id.rs`).
const ANNOTATION_ID_CHARS: usize = 10;

/// The number of hex characters of the content hash naming the invoice temp
/// file (decision 20); enough of the digest to be effectively unique and short
/// enough to keep the file name sane.
const TEMP_HASH_CHARS: usize = 16;

/// The exact `Subject:` of a PayPal payment receipt (case-sensitive, as
/// PayPal prints it); anything else is not this path's business.
const PAYPAL_SUBJECT: &str = "Your PayPal payment receipt";

/// Body line naming the amount of a PayPal receipt (German for "amount").
const BETRAG_PREFIX: &str = "Betrag:";

/// Body line naming the payment purpose of a PayPal receipt (German for
/// "reason"); it doubles as the counterparty hint, verbatim, `;` included.
const GRUND_PREFIX: &str = "Grund:";

/// One annotation extracted from a mail, plus the FACTS the linking of plan
/// item 4.7 matches on: the [`crate::domain::annotation::Annotation`] has no
/// date or amount field (`data-files.md` fixes the columns), so they ride
/// alongside (decision 19).
#[derive(Debug)]
pub struct AnnotatedMail {
    /// The annotation as it goes to `annotations.csv`: id derived from
    /// `source|text|items` (decision 8), an EMPTY `ref` (linking is plan
    /// item 4.7), and [`CONF_EXTRACT`] as the extraction confidence.
    pub annotation: Annotation,
    /// The fact date in canonical `YYYY-MM-DD` form, from the message's
    /// `Date:` header; EMPTY when the message has no usable Date header, in
    /// which case plan item 4.7 cannot match it.
    pub date: String,
    /// The fact amount in minor units. A PayPal receipt keeps the PRINTED sign
    /// of its `Betrag:` line (amendment A5): positive is income, negative is
    /// expense. An invoice fact is the SUM of the attachment rows' amounts —
    /// the statement's own sums.
    pub amount: i64,
    /// The counterparty the linker may compare case-insensitively: the
    /// message's `Grund:` line verbatim (a `;` inside such as
    /// `SUPERMARKET; DELICATES` is part of the hint), or the display name of
    /// the `From:` header for an invoice attachment.
    pub counterparty_hint: String,
}

/// Annotate every message of the mbox file at `path` that starts at or after
/// `from_offset` — the incremental resume of plan item 4.8, passing the
/// `offset` a `state/markers.csv` marker recorded (see
/// [`crate::technical::mbox::messages_from`]).
///
/// `source` is the provenance stored in the annotation; it is normalized to
/// the file BASENAME exactly like the transaction source is, so the same
/// mailbox read through a different path yields the same annotation id. An
/// unreadable file yields an EMPTY result here — reporting it (the `io` error
/// code) is the caller's job, and no annotation is a partial annotation.
pub fn annotate_mbox(path: &Path, source: &str, from_offset: usize) -> Vec<AnnotatedMail> {
    let text = match std::fs::read_to_string(path) {
        Err(_) => { return Vec::new(); }
        Ok(text) => { text }
    };
    return annotate_from(&text, source, from_offset);
}

/// Annotate the messages of an mbox file given as TEXT — the pure entry point
/// tests (and a caller that already has the text) use. The whole file is
/// annotated: same as [`annotate_mbox`] with `from_offset` 0.
///
/// Pure in the sense that matters for the import: the same text and the same
/// `source` yield a byte-identical result (an attached invoice is re-extracted
/// deterministically via a content-hash-named temp file that is deleted
/// again).
pub fn annotate_mbox_text(text: &str, source: &str) -> Vec<AnnotatedMail> {
    return annotate_from(text, source, 0);
}

/// The items of one attached invoice PDF: its statement rows flattened to
/// item strings `"<date> <description> <display-form amount>"` (the DISPLAY
/// form of [`crate::domain::money::print`], never a minor-unit integer).
///
/// The bytes are written to `std::env::temp_dir()/phin-invoice-<hash>.pdf`
/// named by the SHA-256 of the bytes themselves (decision 20), extracted with
/// [`crate::technical::pdf_text`] and parsed with
/// [`crate::technical::parse_bank_b`]; the temp file is removed on EVERY path.
/// Any failure — not a PDF, not a bank B statement, an unreadable temp file —
/// yields EMPTY items rather than an error: the annotation is still worth its
/// text, and a broken attachment must not cost the caller the message (the
/// same policy [`crate::technical::mbox`] applies to a malformed part).
/// `source_note` is the provenance string the parse level records (the
/// attachment's file name in production); it never reaches the item strings.
pub fn items_from_invoice_bytes(bytes: &[u8], source_note: &str) -> Vec<String> {
    let rows = match invoice_rows(bytes, source_note) {
        None => { return Vec::new(); }
        Some(rows) => { rows }
    };
    let mut items: Vec<String> = Vec::new();
    for row in rows {
        items.push(item_text(&row));
    }
    return items;
}

// ---------------------------------------------------------------------------
// The two message paths
// ---------------------------------------------------------------------------

/// Split `text` at `from_offset` via [`crate::technical::mbox`] and annotate
/// the messages that come out — the shared tail of both public entry points.
fn annotate_from(text: &str, source: &str, from_offset: usize) -> Vec<AnnotatedMail> {
    let messages = mbox::messages_from(text, from_offset);
    // The provenance is the file NAME, normalized once for every annotation
    // and for every id payload (same convention as the transaction source).
    let source_name = id::source_basename(source);
    let mut annotated: Vec<AnnotatedMail> = Vec::new();
    for message in messages.iter() {
        // The two paths are disjoint by the message itself: a PayPal receipt
        // is a notification (its html part is no attachment either way), any
        // other message only counts through what it carries attached.
        let subject = match mbox::header(message, "Subject") {
            None => { "" }
            Some(value) => { value }
        };
        if subject == PAYPAL_SUBJECT {
            match paypal_fact(message, &source_name) {
                None => { }
                Some(fact) => { annotated.push(fact); }
            }
            continue;
        }
        for attachment in message.attachments.iter() {
            annotated.push(invoice_fact(message, attachment, &source_name));
        }
    }
    return annotated;
}

/// The PayPal-receipt fact of `message`, [`None`] when its `Betrag:` line is
/// missing or cannot be read as an amount — a receipt without an amount is an
/// unusable fact, and an annotation of it could only ever be a lie about what
/// was paid (an empty body produces nothing either, per the task).
///
/// The `Grund:` line becomes the counterparty hint VERBATIM (the fixture's
/// `SUPERMARKET; DELICATES` keeps its `;`) and the annotation text; a message
/// without a `Grund:` line still yields the fact, with an empty hint.
fn paypal_fact(message: &Message, source: &str) -> Option<AnnotatedMail> {
    let betrag = match body_value(message, BETRAG_PREFIX) {
        None => { return None; }
        Some(value) => { value }
    };
    // Amendment A5 (supersedes planning decision 9): the amount keeps the
    // PRINTED sign — money::parse reads "-19,99 EUR" as -1999 and "25,99" as
    // 2599; no negation, no absolute value anywhere.
    let amount = match money::parse(&betrag) {
        Err(_) => { return None; }
        Ok(minor_units) => { minor_units }
    };
    let grund = match body_value(message, GRUND_PREFIX) {
        None => { String::new() }
        Some(value) => { value }
    };
    let text = format!("PayPal payment to {grund}");
    return Some(AnnotatedMail {
        annotation: build_annotation(source, &text, Vec::new()),
        date: fact_date(message),
        amount: amount,
        counterparty_hint: grund,
    });
}

/// The invoice fact of `message` carrying attachment `attachment`: the rows
/// of its PDF as item strings, the FACT amount the SUM of those rows' amounts
/// (documented: invoice totals are the statement's own sums), the FACT date
/// the message's `Date:` header, and the `From:` display name as the
/// counterparty hint. An attachment whose bytes are not a readable statement
/// yields an EMPTY item list and an amount of 0, but still the annotation
/// (the task pins that).
fn invoice_fact(message: &Message, attachment: &Attachment, source: &str) -> AnnotatedMail {
    let rows = match invoice_rows(&attachment.bytes, &attachment.file_name) {
        None => { Vec::new() }
        Some(rows) => { rows }
    };
    let mut items: Vec<String> = Vec::new();
    // The sum saturates at the i64 bounds rather than panicking; real
    // statements of a few hundred rows can never reach them.
    let mut amount: i64 = 0;
    for row in rows.iter() {
        items.push(item_text(&row));
        amount = amount.saturating_add(row.amount);
    }
    let text = format!("Invoice items from attached {}", attachment.file_name);
    return AnnotatedMail {
        annotation: build_annotation(source, &text, items),
        date: fact_date(message),
        amount: amount,
        counterparty_hint: from_display_name(message),
    };
}

// ---------------------------------------------------------------------------
// Fact extraction
// ---------------------------------------------------------------------------

/// One statement row of an attached invoice, as far as the annotation needs
/// it: the bank rows are flattened into item strings and sums, and the
/// [`crate::domain::transaction::Transaction`]s of the parse are DISCARDED
/// (decision 19; R002 never leaves the call boundary).
struct InvoiceRow {
    date: String,
    description: String,
    amount: i64,
}

/// The item string of one invoice row: `"<date> <description> <display-form
/// amount>"` — the DISPLAY form from [`crate::domain::money::print`] (e.g.
/// `2024-02-24 REWE MARKT 12 -34,56 €`), never a minor-unit integer.
fn item_text(row: &InvoiceRow) -> String {
    return format!("{} {} {}", row.date, row.description, money::print(row.amount));
}

/// The statement rows behind `bytes`: write them to the content-hash temp
/// file, extract with `pdftotext -layout`, parse as a bank B statement, and
/// flatten the rows (the transactions themselves are DISCARDED). [`None`] is
/// any failure — the temp file could not be written, the converter failed or
/// the text is no statement (an empty Some would be a statement with no
/// rows) — and the temp file is removed before returning on EVERY path that
/// created it, including the one where [`std::fs::write`] fails AFTER the
/// create: it is `File::create` followed by `write_all`, so a failing
/// `write_all` leaves a file on disk that this function still removes.
fn invoice_rows(bytes: &[u8], source_note: &str) -> Option<Vec<InvoiceRow>> {
    let path = std::env::temp_dir().join(temp_invoice_name(bytes));
    match std::fs::write(&path, bytes) {
        Err(_) => {
            // The create may well have succeeded while the write_all failed,
            // in which case the file IS on disk: remove it before returning so
            // no temp file survives this path either.
            remove_temp(&path);
            return None;
        }
        Ok(()) => { }
    }
    let text = match extract_text(&path) {
        Err(_) => {
            remove_temp(&path);
            return None;
        }
        Ok(text) => { text }
    };
    // The converter has read the file by now; nothing else needs it on disk.
    remove_temp(&path);
    let report = parse_bank_b_text(&text, source_note);
    let mut rows: Vec<InvoiceRow> = Vec::new();
    for transaction in report.transactions.iter() {
        rows.push(InvoiceRow {
            date: transaction.date.clone(),
            description: transaction.description.clone(),
            amount: transaction.amount,
        });
    }
    return Some(rows);
}

/// The pinned temp file name of decision 20: `phin-invoice-<hash>[0..16].pdf`
/// with the hash over the CONTENT BYTES of the attachment (the same digest
/// `id::sha256_hex` computes, but over bytes, since an attachment is not a
/// `&str`), so concurrent or repeated runs of the same attachment name the
/// same file and a different attachment never collides with it.
fn temp_invoice_name(bytes: &[u8]) -> String {
    return format!("phin-invoice-{}.pdf", id::first_chars(&sha256_hex_of(bytes), TEMP_HASH_CHARS));
}

/// The lowercase hex SHA-256 of these BYTES — `id::sha256_hex` takes a `&str`
/// and an attachment is arbitrary binary. The same helper shape
/// `crate::technical::mbox` uses in its tests (see its `sha256_hex_of`).
fn sha256_hex_of(bytes: &[u8]) -> String {
    use sha2::Digest;
    let digest = sha2::Sha256::digest(bytes);
    let mut hex = String::new();
    for byte in digest.iter() {
        hex.push_str(&format!("{:02x}", *byte));
    }
    return hex;
}

/// Remove the invoice temp file, ignoring the one error that could mean it is
/// already gone: the name is content-hash-unique, so a concurrent reader of
/// the SAME bytes may have deleted it first; either way this is not a failure
/// worth a complaint.
fn remove_temp(path: &Path) {
    match std::fs::remove_file(path) {
        Ok(()) => { }
        Err(_) => { }
    }
}

/// The value of the body line `PREFIX value` of `message`, the FIRST such
/// line (a receipt repeats itself in its html part at most); the value is
/// trimmed but nothing else is touched — the fixture prints `Betrag: 25,99
/// EUR` and [`crate::domain::money::parse`] reads the trailing `EUR` itself.
/// The body is searched line-wise over the message's full body region, which
/// for a multipart mail includes every part's text: the PayPal fixture's
/// `text/plain` part carries these lines.
fn body_value(message: &Message, prefix: &str) -> Option<String> {
    for line in message.body.lines() {
        let line = line.trim();
        match line.strip_prefix(prefix) {
            None => { }
            Some(rest) => { return Some(rest.trim().to_string()); }
        }
    }
    return None;
}

/// The fact date of `message`: the canonical `YYYY-MM-DD` its `Date:` header
/// carries, EMPTY when the message has no Date header or the header is not
/// the pinned RFC 822 form (such a fact is still returned, only unmatchable
/// for plan item 4.7).
fn fact_date(message: &Message) -> String {
    return match mbox::header(message, "Date") {
        None => { String::new() }
        Some(value) => { mbox::parse_rfc822_date(value).unwrap_or_else(String::new) }
    };
}

/// The display name of the `From:` header: the text BEFORE the `<` of an
/// address (`Tobias <tobias@example.com>` -> `Tobias`), or the whole value
/// when it carries no `<` (a bare address then names the counterparty
/// itself). No `From:` header means no hint, not a guess.
fn from_display_name(message: &Message) -> String {
    return match mbox::header(message, "From") {
        None => { String::new() }
        Some(value) => {
            match value.find('<') {
                None => { value.trim().to_string() }
                Some(at) => { value[..at].trim().to_string() }
            }
        }
    };
}

// ---------------------------------------------------------------------------
// The annotation record itself
// ---------------------------------------------------------------------------

/// Build the annotation record: the id of decision 8 is
/// `sha256(source|text|items)[0..10]` — deliberately NOT hashing `ref` or
/// `confidence`, because relinking changes both and the id must survive that
/// (see `doc/architecture/data-files.md`). The `ref` stays EMPTY: linking is
/// plan item 4.7; the confidence is [`CONF_EXTRACT`] clamped through
/// [`crate::domain::annotation::clamp_confidence`].
fn build_annotation(source: &str, text: &str, items: Vec<String>) -> Annotation {
    let payload = format!("{source}|{text}|{}", items_to_text(items.clone()));
    let id = id::first_chars(&id::sha256_hex(&payload), ANNOTATION_ID_CHARS);
    return Annotation {
        id: id,
        r#ref: String::new(),
        text: text.to_string(),
        items: items,
        source: source.to_string(),
        confidence: clamp_confidence(CONF_EXTRACT),
    };
}

// ---------------------------------------------------------------------------
// Tests. The fixtures under testdata/ are READ-ONLY here (they are generated,
// not edited, and this task never touches them); the invoice temp file lives
// under the system temp dir and is checked to be GONE again. If this module
// ever needs a scratch directory of its own it must use a module-private
// parent (e.g. SCRATCH_PARENT = "test-scratch-mail-annotate"), never the
// shared test-scratch/ (the flake the plan-item 3.6b review documented).
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::annotate_mbox;
    use super::annotate_mbox_text;
    use super::items_from_invoice_bytes;
    use super::AnnotatedMail;
    use super::CONF_EXTRACT;
    use crate::domain::annotation::Annotation;
    use crate::domain::annotation::items_to_text;
    use crate::technical::id;
    use crate::technical::mbox::split_messages;

    /// Serializes every test that runs an invoice extraction through the
    /// content-hash temp file: decision 20 names that file by the ATTACHMENT
    /// CONTENT, so tests annotating the same fixture (or reading the same
    /// PDF) share one path while `cargo test` runs them on parallel threads —
    /// an unserialized cleanup assertion could watch another thread's file
    /// appear mid-test. No test may hold the lock while asserting absence, so
    /// each locks for its whole body.
    static TEMP_FILE_LOCK: std::sync::Mutex::<()> = std::sync::Mutex::<()>::new(());

    /// Lock the extraction sequence for this test; a poisoned lock only ever
    /// means an earlier test panicked mid-extraction, and this module's tests
    /// touch no shared state, so carrying on is correct.
    fn lock_invoice_extraction() -> std::sync::MutexGuard<'static, ()> {
        return match TEMP_FILE_LOCK.lock() {
            Ok(guard) => { guard }
            Err(poisoned) => { poisoned.into_inner() }
        };
    }

    /// Locate `testdata/` relative to the test's working directory.
    ///
    /// Copied from the same pattern `crate::technical::id`,
    /// `crate::technical::mbox` and `crate::technical::importer_host` use (and
    /// kept private to this module): `cargo test` runs the test binary with
    /// the package root (`core/`) as its working directory, so the fixture
    /// directory sits one level up; the search walks up the parents trying
    /// only `testdata` per directory.
    fn find_testdata_dir() -> std::path::PathBuf {
        let start = std::env::current_dir().unwrap_or(std::path::PathBuf::new());
        let mut dir = start;
        loop {
            let candidate = dir.join("testdata");
            if candidate.is_dir() { return candidate; }
            match dir.parent() {
                None => { panic!("mail_annotate tests cannot find testdata relative to the test's working directory"); }
                Some(parent) => { dir = parent.to_path_buf(); }
            }
        }
    }

    /// The fixture `name` under `testdata/` read to text (fixtures are read,
    /// never written).
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

    /// The lowercase hex SHA-256 of these BYTES, for locating the temp file
    /// name the production code would have chosen (same shape as the module's
    /// own helper, which a test must not call directly to prove the name).
    fn sha256_hex_of(bytes: &[u8]) -> String {
        use sha2::Digest;
        let digest = sha2::Sha256::digest(bytes);
        let mut hex = String::new();
        for byte in digest.iter() {
            hex.push_str(&format!("{:02x}", *byte));
        }
        return hex;
    }

    /// The temp file path the module derives for `bytes` (decision 20).
    fn temp_invoice_path(bytes: &[u8]) -> std::path::PathBuf {
        let hex = sha256_hex_of(bytes);
        let stem = &hex[0..16];
        return std::env::temp_dir().join(format!("phin-invoice-{stem}.pdf"));
    }

    /// A message carrying a `application/pdf` part whose decoded bytes are
    /// plain garbage (`foo not a pdf at all\n`, base64-wrapped): the
    /// attachment edge case — the annotation survives, the items are empty.
    const BROKEN_PDF_MBOX: &str = "From anna@example.com Mon Apr 01 10:00:00 2024
From: Anna <anna@example.com>
Subject: invoice attempt
Date: 01 Apr 2024 10:00:00 +0200
Content-Type: multipart/mixed; boundary=BrokenBoundary1

--BrokenBoundary1
Content-Type: application/pdf; name=broken.pdf
Content-Disposition: attachment; filename=broken.pdf
Content-Transfer-Encoding: base64

Zm9vIG5vdCBhIHBkZiBhdCBhbGwK
--BrokenBoundary1--
";

    /// The bytes the [`BROKEN_PDF_MBOX`] attachment decodes to.
    const BROKEN_ATTACHMENT_BYTES: &[u8] = b"foo not a pdf at all\n";

    /// A PayPal receipt WITHOUT a Date header: the fact date stays empty, the
    /// fact is still returned.
    const PAYPAL_WITHOUT_DATE: &str = "From payments@example.com Tue Apr 02 12:00:00 2024
From: PAYPAL *NameOfPayee <payments@example.com>
Subject: Your PayPal payment receipt
Content-Type: text/plain; charset=UTF-8

Betrag: 25,99 EUR
Grund: LEGO STORE DE
";

    /// A PayPal receipt whose `Betrag:` cannot be read as money: no usable
    /// fact exists, so nothing is annotated.
    const PAYPAL_BROKEN_AMOUNT: &str = "From payments@example.com Tue Apr 02 12:00:00 2024
From: PAYPAL *NameOfPayee <payments@example.com>
Subject: Your PayPal payment receipt
Date: 02 Apr 2024 12:00:00 +0100
Content-Type: text/plain; charset=UTF-8

Betrag: FOO,BAR EUR
Grund: BROKEN CORNER
";

    /// A plain thread message with neither the PayPal subject nor an
    /// attachment: nothing at all is extracted from it.
    const PLAIN_MESSAGE: &str = "From anna@example.com Mon Apr 01 10:00:00 2024
From: Anna <anna@example.com>
Subject: hello
Date: 01 Apr 2024 10:00:00 +0200
Content-Type: text/plain; charset=us-ascii

just a note, no money involved
";

    #[test]
    fn paypal_fixture_yields_annotations_with_items_and_confidence() {
        let facts = annotate_mbox_text(&fixture_text("mail_paypal.mbox"), "mail_paypal.mbox");
        assert!(facts.len() == 8, "mail_paypal.mbox must yield 8 annotations, got {}", facts.len());

        // The first fact, exactly as amendment A5 pins it: the PRINTED sign of
        // `Betrag: 25,99` is KEPT, so +2599 (the plan's -2599 is retired).
        let first = &facts[0];
        assert!(first.date == "2024-04-02", "date: {}", first.date);
        assert!(first.amount == 2599, "amount (A5: printed sign kept): {}", first.amount);
        assert!(first.counterparty_hint == "LEGO STORE DE", "hint: {}", first.counterparty_hint);
        assert!(first.annotation.confidence == CONF_EXTRACT, "confidence: {}", first.annotation.confidence);

        // All eight amounts in message order: every `Betrag:` as printed.
        let amounts: Vec<i64> = facts.iter().map(|fact| fact.amount).collect();
        assert!(amounts == [2599, -1999, 899, -4500, 12000, -799, 3450, -2999], "amounts: {amounts:?}");

        // The `;` of the SUPERMARKET hint survives verbatim (the task's edge
        // case), and the annotation text names the Grund.
        let semicolon = facts.iter().find(|fact| fact.counterparty_hint == "SUPERMARKET; DELICATES");
        match semicolon {
            None => { panic!("the Grund with a ; must survive verbatim as the hint: {:?}", facts.iter().map(|f| f.counterparty_hint.clone()).collect::<Vec<String>>()); }
            Some(fact) => { assert!(fact.annotation.text == "PayPal payment to SUPERMARKET; DELICATES", "text: {}", fact.annotation.text); }
        }

        // PayPal annotations carry NO items (the receipt names no line items),
        // an empty ref (linking is plan item 4.7), and the mbox basename
        // provenance.
        for fact in &facts {
            assert!(fact.annotation.items.is_empty(), "a receipt carries no items: {:?}", fact.annotation);
            assert!(fact.annotation.r#ref == *"", "ref: {:?}", fact.annotation);
            assert!(fact.annotation.source == "mail_paypal.mbox", "source: {}", fact.annotation.source);
            assert!(fact.annotation.id.chars().count() == 10, "id: {}", fact.annotation.id);
            assert!(fact.annotation.text.starts_with("PayPal payment to "), "text: {:?}", fact.annotation.text);
        }
    }

    #[test]
    fn invoice_attachment_yields_items() {
        let _locked = lock_invoice_extraction();
        let facts = annotate_mbox_text(&fixture_text("mail_with_pdf.mbox"), "mail_with_pdf.mbox");
        assert!(facts.len() == 1, "exactly one AnnotatedMail across the 12 messages, got {}", facts.len());
        let fact = &facts[0];

        // The four bank B rows of the attached bank_b1.pdf, flattened.
        assert!(fact.annotation.items.len() == 4, "items: {:?}", fact.annotation.items);
        assert!(fact.annotation.items.iter().any(|item| item.contains("REWE MARKT 12")), "an item must name REWE MARKT 12: {:?}", fact.annotation.items);
        // The DISPLAY form, not a minor-unit integer (notes item 5).
        assert!(fact.annotation.items[0] == "2024-02-03 ALDI 4711 -89,90 €", "items carry the money::print form: {:?}", fact.annotation.items);
        for item in &fact.annotation.items {
            assert!(!item.contains("-8990") && !item.contains("85000"), "no minor-unit integer may leak into an item: {item}");
        }

        // The fact side: date = message Date header, amount = SUM of the
        // attachment rows (the statement's own sums: -8990 - 2500 + 85000 - 3456),
        // hint = the From display name.
        assert!(fact.date == "2024-03-31", "date: {}", fact.date);
        assert!(fact.amount == 70054, "amount (sum of the b1 rows): {}", fact.amount);
        assert!(fact.counterparty_hint == "Tobias", "hint: {}", fact.counterparty_hint);
        assert!(fact.annotation.text == "Invoice items from attached bank_b1.pdf", "text: {}", fact.annotation.text);
        assert!(fact.annotation.source == "mail_with_pdf.mbox", "source: {}", fact.annotation.source);
    }

    #[test]
    fn annotation_carries_confidence() {
        let _locked = lock_invoice_extraction();
        let mut facts = annotate_mbox_text(&fixture_text("mail_paypal.mbox"), "mail_paypal.mbox");
        facts.extend(annotate_mbox_text(&fixture_text("mail_with_pdf.mbox"), "mail_with_pdf.mbox"));
        assert!(facts.len() == 9);
        for fact in &facts {
            assert!(fact.annotation.confidence == 0.9, "extraction confidence 0.9 until 4.7: {:?}", fact.annotation);
        }
    }

    #[test]
    fn never_produces_transactions() {
        let _locked = lock_invoice_extraction();
        // R002 compile-level proof: these coercions PIN the public signatures.
        // If any return type ever gained a Transaction, this line stops
        // compiling — the return types simply cannot name one.
        let _: fn(&str, &str) -> Vec<AnnotatedMail> = annotate_mbox_text;
        let _: fn(&std::path::Path, &str, usize) -> Vec<AnnotatedMail> = annotate_mbox;
        let _: fn(&[u8], &str) -> Vec<String> = items_from_invoice_bytes;

        // The brief's compile path: the facts collect into annotations.
        let facts = annotate_mbox_text(&fixture_text("mail_with_pdf.mbox"), "mail_with_pdf.mbox");
        let annotations: Vec<Annotation> = facts.into_iter().map(|a| a.annotation).collect();
        assert!(annotations.len() == 1);

        // And the invoice fact amount: sum(-8990, -2500, 85000, -3456).
        let facts = annotate_mbox_text(&fixture_text("mail_with_pdf.mbox"), "mail_with_pdf.mbox");
        assert!(facts[0].amount == 70054, "invoice totals are the statement's own sums: {}", facts[0].amount);
    }

    #[test]
    fn escaped_from_body_not_annotated() {
        let _locked = lock_invoice_extraction();
        // The 12-message fixture with the `>From ` escaped body line still
        // yields EXACTLY one annotation (the pdf attachment message): the
        // un-escaped `From the internet: hello` line in the body is neither a
        // Betrag/Grund receipt nor an attachment.
        let facts = annotate_mbox_text(&fixture_text("mail_with_pdf.mbox"), "mail_with_pdf.mbox");
        assert!(facts.len() == 1, "the escaped From line must not create or lose an annotation, got {}", facts.len());
    }

    #[test]
    fn non_pdf_attachment_still_annotates_with_empty_items() {
        let _locked = lock_invoice_extraction();
        // The attachment edge case: the bytes are not a PDF, the extraction
        // fails, and the annotation is produced anyway with EMPTY items and
        // the amount 0 the empty sum makes.
        let facts = annotate_mbox_text(BROKEN_PDF_MBOX, "broken.mbox");
        assert!(facts.len() == 1, "a broken attachment still yields the annotation, got {:?}", facts);
        let fact = &facts[0];
        assert!(fact.annotation.items.is_empty(), "items: {:?}", fact.annotation.items);
        assert!(fact.amount == 0, "an unextractable attachment sums to 0: {}", fact.amount);
        assert!(fact.date == "2024-04-01", "date: {}", fact.date);
        assert!(fact.counterparty_hint == "Anna", "hint: {}", fact.counterparty_hint);
        assert!(fact.annotation.text == "Invoice items from attached broken.pdf", "text: {}", fact.annotation.text);
        // The temp file of the FAILING path is cleaned up too (decision 20).
        assert!(!temp_invoice_path(BROKEN_ATTACHMENT_BYTES).exists(), "the invoice temp file must not survive a failed extraction");
    }

    #[test]
    fn temp_file_is_named_by_content_hash_and_removed() {
        let _locked = lock_invoice_extraction();
        // Decision 20 on the success path: the fixture's own bytes name the
        // temp file, and it is gone again afterwards.
        let pdf = fixture_bytes("bank_b1.pdf");
        let items = items_from_invoice_bytes(&pdf, "bank_b1.pdf");
        assert!(items.len() == 4, "the public entry parses the fixture rows: {items:?}");
        assert!(items[2] == "2024-02-17 CLIENT PAYMENT 850,00 €", "display form: {items:?}");
        let path = temp_invoice_path(&pdf);
        assert!(!path.exists(), "the content-hash temp file must be deleted after use: {}", path.display());
        // And the same bytes always name the same (removable, idempotent) path.
        assert!(temp_invoice_path(&pdf) == temp_invoice_path(&fixture_bytes("bank_b1.pdf")));
    }

    #[test]
    fn broken_paypal_amount_yields_nothing() {
        // A receipt whose `Betrag:` cannot be read has NO usable fact: the
        // amount the linker needs does not exist, so annotating it would
        // invent a payment of 0. Empty bodies and attachment-less messages
        // produce nothing as well.
        assert!(annotate_mbox_text(PAYPAL_BROKEN_AMOUNT, "broken_paypal.mbox").is_empty());
        assert!(annotate_mbox_text(PLAIN_MESSAGE, "plain.mbox").is_empty());
    }

    #[test]
    fn missing_date_header_yields_empty_fact_date_but_survives() {
        // The task's edge case: no Date header -> the date fact is the empty
        // string, and the fact (amount and hint included) is still returned.
        let facts = annotate_mbox_text(PAYPAL_WITHOUT_DATE, "no_date.mbox");
        assert!(facts.len() == 1, "the fact is still returned: {facts:?}");
        assert!(facts[0].date == *"", "date: {:?}", facts[0].date);
        assert!(facts[0].amount == 2599);
        assert!(facts[0].counterparty_hint == "LEGO STORE DE");
    }

    #[test]
    fn annotation_id_survives_relinking() {
        // Decision 8: the id payload is `source|text|items` ONLY — a changed
        // ref or confidence (4.7 relinking the annotation) keeps the id.
        let facts = annotate_mbox_text(&fixture_text("mail_paypal.mbox"), "mail_paypal.mbox");
        let annotation = &facts[0].annotation;
        let payload = format!("{}|{}|{}", annotation.source, annotation.text, items_to_text(annotation.items.clone()));
        assert!(annotation.id == id::first_chars(&id::sha256_hex(&payload), 10), "the id must be sha256(source|text|items)[0..10]: {}", annotation.id);

        // An identical annotation with a DIFFERENT ref and confidence hashes
        // to the same id: relinking cannot change it.
        let relinked = Annotation {
            id: String::new(),
            r#ref: "a86d11da9a".to_string(),
            text: annotation.text.clone(),
            items: annotation.items.clone(),
            source: annotation.source.clone(),
            confidence: 0.0,
        };
        let payload = format!("{}|{}|{}", relinked.source, relinked.text, items_to_text(relinked.items.clone()));
        assert!(relinked.r#ref != annotation.r#ref && relinked.confidence != annotation.confidence);
        assert!(id::first_chars(&id::sha256_hex(&payload), 10) == annotation.id, "the id is stable across ref/confidence changes");
    }

    #[test]
    fn path_like_source_yields_the_same_annotations_as_the_basename() {
        // `source` goes through `id::source_basename`, and that normalization
        // is not cosmetic: the id payload starts with the source, so a mailbox
        // read through a path-like provenance must still yield the VERY
        // annotations the bare basename run yields — same id, same fields.
        let text = fixture_text("mail_paypal.mbox");
        let bare = annotate_mbox_text(&text, "mail_paypal.mbox");
        let via_path = annotate_mbox_text(&text, "/data/mail/mail_paypal.mbox");
        assert!(bare.len() == 8 && via_path.len() == bare.len(), "same number of annotations: {} vs {}", via_path.len(), bare.len());
        // one = the basename run, other = the path-like run: every field of an
        // annotation and of the fact must be identical, the id included.
        for (one, other) in bare.iter().zip(via_path.iter()) {
            assert!(other.annotation.id == one.annotation.id, "the id must not depend on the path read through: {} vs {}", other.annotation.id, one.annotation.id);
            assert!(other.annotation.source == one.annotation.source && other.annotation.source == "mail_paypal.mbox", "source: {}", other.annotation.source);
            assert!(other.annotation.text == one.annotation.text, "text: {}", other.annotation.text);
            assert!(other.annotation.items == one.annotation.items, "items: {:?}", other.annotation.items);
            assert!(other.annotation.r#ref == one.annotation.r#ref, "ref: {:?}", other.annotation.r#ref);
            assert!(other.annotation.confidence == one.annotation.confidence, "confidence: {}", other.annotation.confidence);
            assert!(other.date == one.date, "date: {}", other.date);
            assert!(other.amount == one.amount, "amount: {}", other.amount);
            assert!(other.counterparty_hint == one.counterparty_hint, "hint: {}", other.counterparty_hint);
        }
        assert!(format!("{via_path:?}") == format!("{bare:?}"), "the whole result is identical to the basename run");
    }

    #[test]
    fn annotating_the_same_text_twice_is_identical() {
        // Purity: no id, no clock, no counter enters the annotation, so a
        // second run is identical, and the file entry point agrees with the
        // text entry point (its temp-file round trip included).
        let text = fixture_text("mail_paypal.mbox");
        let first = annotate_mbox_text(&text, "mail_paypal.mbox");
        let second = annotate_mbox_text(&text, "mail_paypal.mbox");
        assert!(format!("{first:?}") == format!("{second:?}"), "the pure entry point is deterministic");

        let from_file = annotate_mbox(&find_testdata_dir().join("mail_paypal.mbox"), "mail_paypal.mbox", 0);
        assert!(format!("{from_file:?}") == format!("{first:?}"), "the file entry point yields the same annotations");
        // The resume offset of plan item 4.8: resuming at the last message's
        // end yields nothing new, and the byte offsets are mbox's exact ones.
        let messages = split_messages(&text);
        let resume = annotate_mbox(&find_testdata_dir().join("mail_paypal.mbox"), "mail_paypal.mbox", messages[messages.len() - 1].end);
        assert!(resume.is_empty(), "resume at the end of the last message annotates nothing new");
    }
}
