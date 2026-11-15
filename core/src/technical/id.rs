//! The content-hash transaction id of `transactions.csv` (plan item 2.2).
//!
//! The id scheme is pinned by the fixtures: `testdata/generate.py:84-86`
//! hashes the payload `date|amount|currency|description|counterparty|source`
//! with SHA-256 and keeps the first 10 hex characters of the lowercase digest
//! (`hashlib.sha256(...).hexdigest()[:10]`). The generator hardcodes `EUR` in
//! that slot; here the slot is the `currency` field of the record, which is
//! `EUR` for every fixture row and for every transaction phin accepts under
//! the "Single currency" decision (`doc/architecture/data-files.md`, `currency`
//! column) — so the fixture ids reproduce byte for byte, while a USD record
//! cannot collide with its EUR twin. The `source` slot holds the **basename**
//! of the source file: the fixtures store bare file names, and source identity
//! is path-independent, so the same statement record reached through a
//! different path hashes to the same id.
//!
//! Repeated identical content — two source records that normalize to the same
//! payload — share that base id; the store appends an occurrence suffix `#2`,
//! `#3`, … to the emitted line (`testdata/generate.py:377-389`, visible in
//! `testdata/transactions.csv` at line 16/17 as `80c63211a8` /
//! `80c63211a8#2`). The suffix is NOT part of the hash: [`base_id`] of a
//! duplicated row recomputes the bare 10 hex chars, and [`occurrence_id`]
//! decides the suffix from the ids already known to the store. This is what
//! makes the idempotence rule of `doc/architecture/data-files.md` ("before
//! appending, the core skips candidates whose id is already known") work: the
//! store reader of plan item 2.3 recomputes [`base_id`] from a parsed record
//! and compares it against the stored ids with [`occurrence_id`] /
//! [`base_id_of_id`].
//!
//! Hashing lives on this side because it is a storage concern; the domain
//! [`core::domain::transaction::Transaction`] only carries the finished `id`
//! string.

use crate::domain::transaction::Transaction;
use sha2::Digest;
use sha2::Sha256;

/// Number of lowercase hex characters the id keeps from the SHA-256 digest.
/// Pinned by `testdata/generate.py:86` (`hexdigest()[:10]`) and by every id of
/// `testdata/transactions.csv`.
const ID_HEX_CHARS: usize = 10;

/// The `source` column identifies a statement by its file name alone, never by
/// the path it was read through: the fixtures store bare names such as
/// `bank_b2.pdf`, so `/srv/statements/bank_b2.pdf` and `bank_b2.pdf` hash
/// identically. Both `/` and `\` end a path component, so a Windows-style
/// provenance string hashes to the same basename as the POSIX one (a POSIX
/// file name that itself contains a backslash is outside that precondition —
/// the importers of plan item 4.x never produce one). Trailing separators are
/// dropped before cutting, so `dir/` yields `dir` as GNU `basename` does
/// rather than the empty string, which would collide with a record that has
/// no source at all.
pub fn source_basename(source: &str) -> String {
    let mut head: &str = source;
    while head.len() > 0 && (head.ends_with("/") || head.ends_with("\\")) {
        head = &head[..head.len() - 1];
    }
    let mut cut = 0;
    match head.rfind("/") {
        Some(index) => { cut = index + 1; }
        None => { }
    }
    match head.rfind("\\") {
        Some(index) => {
            if index + 1 > cut { cut = index + 1; }
        }
        None => { }
    }
    return head[cut..].to_string();
}

/// The base id of a transaction: the first [`ID_HEX_CHARS`] lowercase hex
/// characters of `sha256("date|amount|currency|description|counterparty|source-basename")`,
/// the payload of `testdata/generate.py:84-86` reproduced byte for byte —
/// `amount` the `i64` minor-unit count exactly as `transactions.csv` stores it
/// (no re-scaling), `date` the canonical `YYYY-MM-DD` storage form passed
/// through.
///
/// Takes the stored field values, not display-form text: hashing the
/// normalized values is what makes a re-import of the same statement record
/// land on the same id. The `#N` occurrence suffix is NOT part of this hash
/// (see [`occurrence_id`]).
pub fn base_id(date: &str, amount: i64, currency: &str, description: &str, counterparty: &str, source: &str) -> String {
    let amount_text = amount.to_string();
    let basename = source_basename(source);
    let mut payload = date.to_string();
    payload.push_str("|");
    payload.push_str(&amount_text);
    payload.push_str("|");
    payload.push_str(currency);
    payload.push_str("|");
    payload.push_str(description);
    payload.push_str("|");
    payload.push_str(counterparty);
    payload.push_str("|");
    payload.push_str(&basename);
    return first_chars(&sha256_hex(&payload), ID_HEX_CHARS);
}

/// The base id of an already-built [`Transaction`], straight from its stored
/// fields — the wrapper the store modules of plan item 2.3 use when appending.
/// Delegates to [`base_id`], passing the record's own `currency`.
pub fn transaction_base_id(tx: &Transaction) -> String {
    return base_id(&tx.date, tx.amount, &tx.currency, &tx.description, &tx.counterparty, &tx.source);
}

/// The id to emit for a base id against the ids already stored, in file order:
/// the base itself on first sight, then `base#2`, `base#3`, … for the second,
/// third, … record with identical content (`testdata/generate.py:380-389`).
///
/// The next suffix is one past the HIGHEST occurrence number already stored
/// for that base, not a count of matching ids: a store holding `base` and
/// `base#5` continues at `base#6`, and a hand-edited file that carries a stray
/// `base#1` never re-emits a suffix that is already on file. An id "matches"
/// the base when it equals it or carries an all-digit `#N` occurrence suffix
/// (see [`occurrence_number`]), so the highest occurrence number survives
/// reading the suffixed ids back out of `transactions.csv`.
pub fn occurrence_id(base: &str, known_ids: &Vec::<String>) -> String {
    let mut highest = 0;
    for index in 0..known_ids.len() {
        let number = occurrence_number(&known_ids[index], base);
        if number > highest {
            highest = number;
        }
    }
    if highest == 0 {
        return base.to_string();
    }
    let mut emitted = base.to_string();
    emitted.push_str("#");
    let next = (highest + 1).to_string();
    emitted.push_str(&next);
    return emitted;
}

/// The occurrence number `id` carries for the base `base`: the bare base is
/// occurrence 1, `base#N` is occurrence `N` (`N` a non-empty run of ASCII
/// digits, saturating at the `usize` limit). Returns 0 when `id` does not
/// belong to `base` at all, so callers test the result with `> 0`.
pub fn occurrence_number(id: &str, base: &str) -> usize {
    if id == base {
        return 1;
    }
    let marker = base.to_string() + "#";
    if !id.starts_with(&marker) {
        return 0;
    }
    let digits = &id[marker.len()..];
    if digits.len() == 0 {
        return 0;
    }
    let mut number: usize = 0;
    for digit in digits.chars() {
        if !digit.is_ascii_digit() {
            return 0;
        }
        number = number.saturating_mul(10);
        number = number.saturating_add((digit as usize) - ('0' as usize));
    }
    return number;
}

/// True when `id` is the base id itself or one of its occurrence-suffixed
/// forms `base#N` (`N` a non-empty run of digits). A base id that is merely a
/// textual prefix of a stored id does not match: `80c63211` does not own the
/// stored id `80c63211a8`.
pub fn matches_base(id: &str, base: &str) -> bool {
    return occurrence_number(id, base) > 0;
}

/// The base part of a possibly suffixed stored id (`80c63211a8#2` ->
/// `80c63211a8`); an id without an occurrence suffix is returned unchanged.
/// The store reader of plan item 2.3 needs this to compare a stored id with a
/// recomputed base id.
pub fn base_id_of_id(id: &str) -> String {
    match id.rfind("#") {
        Some(index) => {
            let base = &id[..index];
            if matches_base(id, base) {
                return base.to_string();
            }
        }
        None => { }
    }
    return id.to_string();
}

/// The full lowercase hex digest of `payload` under SHA-256 — the same digest
/// `hashlib.sha256` produces in the fixture generator, over the UTF-8 bytes.
fn sha256_hex(payload: &str) -> String {
    let digest = Sha256::digest(payload.as_bytes());
    let mut hex = String::new();
    for byte in digest.iter() {
        hex.push_str(&format!("{:02x}", *byte));
    }
    return hex;
}

/// The first `count` characters of `text`; `text` is pure ASCII hex here, so a
/// byte slice cannot split a character.
fn first_chars(text: &str, count: usize) -> String {
    return text[..count].to_string();
}

#[test]
fn fixture_ids_are_reproduced_from_row_content() {
    let path = find_testdata_dir().join("transactions.csv");
    let text = std::fs::read_to_string(&path).unwrap();
    let document = crate::technical::csv::read_document(&text).unwrap();
    let records = crate::technical::csv::records_of(&document);
    assert!(records.len() == 22);

    let mut known_ids = Vec::<String>::new();
    for record in &records {
        assert!(record.len() == 7);
        let stored_id: &str = &record[0];
        let amount = record[2].parse::<i64>().unwrap();
        assert!(record[3] == "EUR");
        let base = base_id(&record[1], amount, &record[3], &record[4], &record[5], &record[6]);
        let emitted = occurrence_id(&base, &known_ids);
        assert!(emitted == stored_id);
        known_ids.push(emitted);
    }

    // File lines 16 and 17 (records 11 and 12; the four preamble comments
    // occupy lines 1-4): the duplicate REWE row and its `#2` occurrence.
    assert!(records[11][0] == "80c63211a8");
    assert!(records[12][0] == "80c63211a8#2");
    assert!(base_id(&records[11][1], records[11][2].parse::<i64>().unwrap(), &records[11][3], &records[11][4], &records[11][5], &records[11][6]) == base_id(&records[12][1], records[12][2].parse::<i64>().unwrap(), &records[12][3], &records[12][4], &records[12][5], &records[12][6]));
}

/// Locate `testdata/` relative to the test's working directory.
///
/// `cargo test` runs the test binary with the package root (`core/`) as its
/// working directory, so the fixture directory sits one level up, next to the
/// package; the search walks up the parents of the current directory, trying
/// only `testdata` per directory, so it never scans anything else.
#[cfg(test)]
fn find_testdata_dir() -> std::path::PathBuf {
    let start = std::env::current_dir().unwrap_or(std::path::PathBuf::new());
    let mut dir = start;
    loop {
        let candidate = dir.join("testdata");
        if candidate.is_dir() { return candidate; }
        match dir.parent() {
            None => { panic!("id tests cannot find testdata relative to the test's working directory"); }
            Some(parent) => { dir = parent.to_path_buf(); }
        }
    }
}

#[test]
fn same_transaction_with_different_source_path_keeps_the_id() {
    let stored_source = base_id("2024-03-05", -1500, "EUR", "REWE MARKT 12", "REWE MARKET", "bank_b2.pdf");
    let posix_path = base_id("2024-03-05", -1500, "EUR", "REWE MARKT 12", "REWE MARKET", "/srv/statements/bank_b2.pdf");
    let windows_path = base_id("2024-03-05", -1500, "EUR", "REWE MARKT 12", "REWE MARKET", "C:\\statements\\bank_b2.pdf");
    assert!(stored_source == "80c63211a8");
    assert!(posix_path == stored_source);
    assert!(windows_path == stored_source);
    // A trailing separator is a path separator, not part of the file name.
    assert!(base_id("2024-03-05", -1500, "EUR", "REWE MARKT 12", "REWE MARKET", "statements/bank_b2.pdf/") == stored_source);

    // The same through a Transaction: the id depends on the stored fields,
    // not on how the source was addressed.
    let tx = crate::domain::transaction::from_stored("80c63211a8", "2024-03-05", -1500, "EUR", "REWE MARKT 12", "REWE MARKET", "statements/bank_b2.pdf").unwrap();
    assert!(transaction_base_id(&tx) == "80c63211a8");
}

#[test]
fn id_hash_length_is_stable_at_ten_hex_chars() {
    let first = base_id("2024-01-03", -3999, "EUR", "REWE MARKT 12", "REWE MARKET", "bank_a.pdf");
    assert!(first == "39945b2fed");
    assert!(first.len() == ID_HEX_CHARS);
    let second = transaction_base_id(&crate::domain::transaction::from_stored("", "2024-04-21", 3450, "EUR", "SUPERMARKET; DELICATES", "DELICATES", "mail_paypal.mbox").unwrap());
    assert!(second == "06113c369c");
    assert!(second.len() == ID_HEX_CHARS);
    // The suffix is appended after hashing, so stored ids may be longer, but
    // the hash part itself never changes length.
    let mut twice_known = Vec::<String>::new();
    twice_known.push(first.to_string());
    twice_known.push(first.to_string() + "#2");
    let suffixed = occurrence_id(&first, &twice_known);
    assert!(suffixed == "39945b2fed#3");
    assert!(base_id_of_id(&suffixed) == first);
    assert!(base_id_of_id(&first) == first);
}

#[test]
fn hand_edited_field_changes_the_id() {
    // The fixture row of file line 5; retyping any hashed field by hand must
    // land on a different id, which is what keeps a hand edit from silently
    // impersonating the original fact.
    let original = base_id("2024-01-03", -3999, "EUR", "REWE MARKT 12", "REWE MARKET", "bank_a.pdf");
    assert!(original == "39945b2fed");
    let edited_description = base_id("2024-01-03", -3999, "EUR", "REWE MARKT 13", "REWE MARKET", "bank_a.pdf");
    assert!(edited_description != original);
    assert!(edited_description.len() == ID_HEX_CHARS);
    let edited_amount = base_id("2024-01-03", -3998, "EUR", "REWE MARKT 12", "REWE MARKET", "bank_a.pdf");
    assert!(edited_amount != original);
    let edited_date = base_id("2024-01-04", -3999, "EUR", "REWE MARKT 12", "REWE MARKET", "bank_a.pdf");
    assert!(edited_date != original);
    let edited_party = base_id("2024-01-03", -3999, "EUR", "REWE MARKT 12", "REWE MARKETZ", "bank_a.pdf");
    assert!(edited_party != original);
    let edited_source = base_id("2024-01-03", -3999, "EUR", "REWE MARKT 12", "REWE MARKET", "bank_b.pdf");
    assert!(edited_source != original);
    // The currency is hashed too, so a hand-edited code cannot impersonate the
    // EUR twin it was retyped from.
    let edited_currency = base_id("2024-01-03", -3999, "USD", "REWE MARKT 12", "REWE MARKET", "bank_a.pdf");
    assert!(edited_currency != original);
    assert!(edited_currency != edited_description);
}

#[test]
fn repeated_content_gets_increasing_occurrence_suffixes() {
    let base = base_id("2024-03-05", -1500, "EUR", "REWE MARKT 12", "REWE MARKET", "bank_b2.pdf");
    assert!(base == "80c63211a8");
    let mut known_ids = Vec::<String>::new();
    let first = occurrence_id(&base, &known_ids);
    assert!(first == base);
    known_ids.push(first);
    let second = occurrence_id(&base, &known_ids);
    assert!(second == "80c63211a8#2");
    known_ids.push(second);
    let third = occurrence_id(&base, &known_ids);
    assert!(third == "80c63211a8#3");
    known_ids.push(third);
    let fourth = occurrence_id(&base, &known_ids);
    assert!(fourth == "80c63211a8#4");
    // A different transaction is unaffected by these ids.
    let other = base_id("2024-03-05", -1590, "EUR", "CAFETARIA", "CAFETARIA", "bank_b2.pdf");
    assert!(occurrence_id(&other, &known_ids) == other);
    // A base id that is a textual prefix of a stored id must not count it.
    assert!(occurrence_id("80c63211", &known_ids) == "80c63211");
    // A non-numeric suffix is not an occurrence marker.
    assert!(matches_base("80c63211a8#x", "80c63211a8") == false);
    assert!(matches_base("80c63211a8#10", "80c63211a8") == true);
    // The next suffix is one past the HIGHEST occurrence on file, not a count:
    // a store that already holds `base` and `base#5` continues at `base#6`.
    let mut sparse = Vec::<String>::new();
    sparse.push(base.to_string());
    sparse.push(base.to_string() + "#5");
    assert!(occurrence_id(&base, &sparse) == "80c63211a8#6");
    // A stray `base#1` (which the generator never emits) does not shift the
    // suffix past what is on file.
    let mut with_one = Vec::<String>::new();
    with_one.push(base.to_string());
    with_one.push(base.to_string() + "#1");
    assert!(occurrence_id(&base, &with_one) == "80c63211a8#2");
}

#[test]
fn digest_matches_python_hashlib_on_known_vectors() {
    // Vectors from `hashlib.sha256(...).hexdigest()` (empty string and the
    // canonical abc vector), i.e. the same digest the fixture generator ran.
    assert!(sha256_hex("") == "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
    assert!(sha256_hex("abc") == "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
    assert!(sha256_hex("2024-01-03|-3999|EUR|REWE MARKT 12|REWE MARKET|bank_a.pdf").starts_with("39945b2fed"));
    // Non-ASCII payload: the fixture payload is ASCII, but hashlib encodes
    // UTF-8, so the digest must be over the UTF-8 bytes, not the code points.
    assert!(sha256_hex("ä") == "33e6d73fee82904c8d7afb78de1154d1e8dc2a0edb08120e63df5b9385c2d9cc");
}
