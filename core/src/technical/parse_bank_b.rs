//! The built-in importer of plan item 4.3: a parser for the statement layout
//! of fixture bank B (`testdata/bank_b1.pdf`, `testdata/bank_b2.pdf`).
//!
//! Per `doc/architecture/architecture.md` ("Import & Extraction") phin has two
//! built-in statement readers; this is the one of them that reads layout B,
//! i.e. `YYYY-MM-DD` dates, a `Card-Number` column between date and
//! description, point-decimal amounts (`-89.90`) with trailing ` EUR`. It is
//! built on [`crate::technical::pdf_text`] (plan item 4.1) and nothing else:
//! date and amount normalization are delegated to [`crate::domain::date`] and
//! [`crate::domain::money`], the record is built with
//! [`crate::domain::transaction::from_parts`] (the importer constructor, never
//! a minor-unit count), and the provisional id comes from
//! [`crate::technical::id::transaction_base_id`]. Per
//! `doc/architecture/architecture.md` ("one importer per bank") this is a
//! SEPARATE parser from [`crate::technical::parse_bank_a`], not a parameterized
//! shared one; only the report and warning TYPES are shared, imported from
//! [`crate::technical::parse_bank_a`].
//!
//! Two things are deliberately NOT done here:
//!
//! * **No occurrence suffix.** The id returned is the bare base id; the `#N`
//!   suffix of duplicated content is assigned by the store when a line is
//!   appended, because it depends on the ids already on file
//!   (`doc/architecture/data-files.md`, `core/src/technical/id.rs::occurrence_id`).
//!   The byte-identical `REWE MARKT 12` pair of `bank_b2.pdf` therefore both
//!   survive this parser with the same provisional id.
//! * **No counterparty recognition.** `doc/architecture/data-files.md:71`
//!   documents the column as the counterparty name "as recognized by the
//!   importer", and `core/src/domain/transaction.rs:60` documents the slot as
//!   "empty when unknown". The Phase-4 importers recognize nothing, so
//!   `counterparty` is the empty string; the description is never copied into
//!   it (user decision I2, which supersedes planning decision 18). A later,
//!   specified heuristic would fill the slot in this file and nowhere else.
//!
//! The parser is permissive in one direction only: a statement whose rows do
//! not fit the layout produces a typed [`crate::technical::parse_bank_a::BankRowWarning`]
//! naming the line and keeps the rest of the file, and a text that is not a
//! bank B statement at all is reported as such but still parsed. It never
//! panics and never writes anything.

use std::path::Path;

use crate::domain::date::parse as date_parse;
use crate::domain::money::parse as money_parse;
use crate::domain::money::print as money_print;
use crate::domain::transaction::from_parts as transaction_from_parts;
use crate::domain::transaction::Transaction;
use crate::technical::id::transaction_base_id;
use crate::technical::parse_bank_a::BankRowWarning;
use crate::technical::parse_bank_a::ParseReport;
use crate::technical::pdf_text::extract_text;
use crate::technical::pdf_text::PdfTextError;

/// The currency layout B prints and the only currency the fixtures carry; like
/// layout A, layout B has no per-row currency field, only an `Amount (EUR)`
/// column header.
const CURRENCY: &str = "EUR";

/// Marker naming this statement producer; a text without it on its first line
/// is reported as "not a Bank B statement" (plan item 4.3's broken-variant
/// case, same policy as the bank A parser).
const STATEMENT_MARKER: &str = "Bank B N";

/// Read the PDF at `path` with [`crate::technical::pdf_text::extract_text`] and
/// parse it as a bank B statement.
///
/// `source` is the provenance string stored in the record (production passes the
/// file name, e.g. `bank_b1.pdf`); [`crate::technical::id::source_basename`]
/// normalizes a path later, so passing a full path still hashes to the same id.
/// A missing `pdftotext` or an unreadable file is the typed [`PdfTextError`] of
/// plan item 4.1, which plan item 4.4/4.8 maps to the `converter-missing` or
/// `io` error code.
pub fn parse_bank_b(path: &Path, source: &str) -> Result<ParseReport, PdfTextError> {
    let text = match extract_text(path) {
        Err(reason) => { return Err(reason); }
        Ok(text) => { text }
    };
    return Ok(parse_bank_b_text(&text, source));
}

/// Parse the extracted text of a bank B statement. Pure: the same text and the
/// same `source` yield a byte-identical [`ParseReport`].
///
/// A line is a candidate row when it starts with a `YYYY-MM-DD` date; every
/// other line (statement header, `Kredit:`/`Monat:` lines, table header,
/// footer, summary, `Total:` line, blank line, form feed) is skipped silently.
/// A candidate row whose date, card-number token, amount or record cannot be
/// built becomes a [`BankRowWarning`] and the parse continues. Duplicate rows
/// both survive in order (deduplication happens at append time, in the store).
pub fn parse_bank_b_text(text: &str, source: &str) -> ParseReport {
    let mut report = ParseReport {
        transactions: Vec::new(),
        warnings: Vec::new(),
    };

    // The statement-wide check comes first so its warning is the first one a
    // reader sees, and so the rows are still parsed (the broken-variant
    // requirement of plan item 4.3: a damaged header must not lose the rows).
    let recognized = match text.lines().next() {
        None => { false }
        Some(head) => { head.contains(STATEMENT_MARKER) }
    };
    if !recognized {
        report.warnings.push(BankRowWarning {
            line: 0,
            text: "not a Bank B statement".to_string(),
        });
    }

    for (index, raw_line) in text.lines().enumerate() {
        let line = raw_line.trim();
        if !starts_with_date(line) {
            continue;
        }
        let number = index + 1;
        match build_row(line, source) {
            Err(reason) => { report.warnings.push(BankRowWarning { line: number, text: reason }); }
            Ok(record) => { report.transactions.push(record); }
        }
    }

    return report;
}

/// True when `line` begins with a `YYYY-MM-DD` date token, the only marker of a
/// statement row in layout B. Headers, footers, summaries and the `Total:` line
/// cannot satisfy it, which is why they are skipped without a warning.
fn starts_with_date(line: &str) -> bool {
    let mut parts = line.split_whitespace();
    match parts.next() {
        None => { false }
        Some(token) => { looks_like_layout_b_date(token) }
    }
}

/// True when `token` is exactly `YYYY-MM-DD` with digits in every position
/// (calendar validity is checked by [`crate::domain::date`], not here).
fn looks_like_layout_b_date(token: &str) -> bool {
    let bytes = token.as_bytes();
    if bytes.len() != 10 {
        return false;
    }
    for (index, byte) in bytes.iter().enumerate() {
        match index {
            4 | 7 => {
                if *byte != b'-' {
                    return false;
                }
            }
            _ => {
                if *byte < b'0' || *byte > b'9' {
                    return false;
                }
            }
        }
    }
    return true;
}

/// True when `token` is a card-number token `^\d{4}(-\d{4}){2}$`, i.e. four
/// digits, dash, four digits, dash, four digits — the shape of the
/// `Card-Number` column `pdftotext -layout` prints between date and
/// description. The check is deliberately a shape check of exactly that
/// pattern (the fixture prints `4711-0000-4711`); anything else on the card
/// column of a candidate row is a warning, not a silently misparsed row.
fn looks_like_card_number(token: &str) -> bool {
    let bytes = token.as_bytes();
    if bytes.len() != 14 {
        return false;
    }
    for (index, byte) in bytes.iter().enumerate() {
        match index {
            4 | 9 => {
                if *byte != b'-' {
                    return false;
                }
            }
            _ => {
                if *byte < b'0' || *byte > b'9' {
                    return false;
                }
            }
        }
    }
    return true;
}

/// Turn one candidate row (already date-first) into a transaction, or a human
/// readable reason it is not one.
///
/// The layout is `DATE CARD-NUMBER DESCRIPTION AMOUNT EUR`: the leading date
/// token and the card-number token behind it are cut off (the card number is
/// DROPPED, it never belongs to the description), the trailing ` EUR` / `€`
/// marker is stripped, and the last remaining token is the amount; everything
/// between is the description — including a `;` inside the merchant text such
/// as `TAXI AIRPORT; NIGHT`, which is part of the description and never a
/// field separator. Currency symbols are stripped here, before
/// [`crate::domain::money::parse`] sees the token, so the parser itself never
/// knows a currency symbol.
fn build_row(line: &str, source: &str) -> Result<Transaction, String> {
    // Strip the printed currency marker of layout B first (case-sensitive
    // `EUR`, with `€` tolerated as the other spelling of the same field), then
    // split the row into its four parts.
    let without_marker = strip_currency_marker(line);
    let mut tokens = without_marker.split_whitespace();
    let date_token = match tokens.next() {
        None => { return Err(format!("row without a date: {line}")); }
        Some(token) => { token.to_string() }
    };
    // The card-number column sits directly behind the date; a candidate row
    // without it does not follow the layout and is a warning, not a row whose
    // card number leaks into the description.
    match tokens.next() {
        None => { return Err(format!("row without a card-number token: {line}")); }
        Some(token) => {
            if !looks_like_card_number(token) {
                return Err(format!("rejected card-number token {token} on row: {line}"));
            }
        }
    }
    // A row that is only date and card number has no amount token at all:
    // `next_back` yields None once the front tokens were already consumed, so
    // that case is the `None` arm below.
    let amount_token = match tokens.next_back() {
        None => { return Err(format!("row without an amount: {line}")); }
        Some(token) => { token.to_string() }
    };

    // `starts_with_date` accepted the shape of the date token, so this can only
    // fail on an impossible calendar date such as `2024-02-31`.
    let date = match date_parse(&date_token) {
        Err(reason) => { return Err(format!("rejected date on row: {line} ({reason:?})")); }
        Ok(normalized) => { normalized }
    };

    let amount = match money_parse(&amount_token) {
        Err(reason) => { return Err(format!("rejected amount {amount_token} on row: {line} ({reason:?})")); }
        Ok(minor_units) => { minor_units }
    };

    // What is left between the card number and the amount is the merchant text;
    // the column padding of `pdftotext -layout` is collapsed into single
    // spaces, but every other character — above all the `;` of
    // `TAXI AIRPORT; NIGHT` — stays inside the description.
    let mut description_parts: Vec<String> = Vec::new();
    for token in tokens {
        description_parts.push(token.to_string());
    }
    let description = description_parts.join(" ");

    // Canonical amount TEXT, never a minor-unit count: `from_parts` takes the
    // display form, so re-rendering through `money::print` is what keeps the
    // value from being re-scaled (see the INVARIANT of `domain::transaction`).
    let canonical_amount = money_print(amount);

    // The counterparty slot stays empty: `doc/architecture/data-files.md:71`
    // documents the column as the name "as recognized by the importer", and
    // `core/src/domain/transaction.rs:60` documents it as "empty when
    // unknown". This importer recognizes no counterparty, so nothing goes in;
    // the description is not copied here (user decision I2).
    let counterparty = String::new();
    let built = match transaction_from_parts("", &date, &canonical_amount, CURRENCY, &description, &counterparty, source) {
        Err(reason) => { return Err(format!("rejected row: {line} ({reason:?})")); }
        Ok(record) => { record }
    };

    // Decision 7: the provisional base id of the record as built, never an
    // occurrence-suffixed id (that suffix needs the ids already on file). The
    // byte-identical pair of `bank_b2.pdf` therefore gets the SAME provisional
    // id twice; the store of task 4.8 turns the second one into `base#2`.
    let base = transaction_base_id(&built);
    let mut record = built;
    record.id = base;
    return Ok(record);
}

/// Drop a trailing `EUR` or `€` marker (and the space in front of it) from a
/// row; anything else is left alone so that the row can still be diagnosed.
fn strip_currency_marker(line: &str) -> String {
    let trimmed = line.trim_end();
    if trimmed.ends_with("€") {
        return trimmed[..trimmed.len() - "€".len()].trim_end().to_string();
    }
    if trimmed.ends_with("EUR") {
        return trimmed[..trimmed.len() - "EUR".len()].trim_end().to_string();
    }
    return trimmed.to_string();
}

/// Locate `testdata/` relative to the test's working directory.
///
/// Copied from the same pattern `crate::technical::id`,
/// `crate::technical::pdf_text` and `crate::technical::parse_bank_a` use (and
/// kept private to this module): `cargo test` runs the test binary with the
/// package root (`core/`) as its working directory, so the fixture directory
/// sits one level up.
#[cfg(test)]
fn find_testdata_dir() -> std::path::PathBuf {
    let start = std::env::current_dir().unwrap_or(std::path::PathBuf::new());
    let mut dir = start;
    loop {
        let candidate = dir.join("testdata");
        if candidate.is_dir() { return candidate; }
        match dir.parent() {
            None => { panic!("parse_bank_b tests cannot find testdata relative to the test's working directory"); }
            Some(parent) => { dir = parent.to_path_buf(); }
        }
    }
}

/// The extracted text of `testdata/<name>.pdf`, extracted per test that needs
/// it (the fixtures are read, never written).
#[cfg(test)]
fn fixture_text(name: &str) -> String {
    let path = find_testdata_dir().join(format!("{name}.pdf"));
    match extract_text(&path) {
        Err(reason) => { panic!("pdftotext must extract the fixture {name}.pdf, got {reason:?}"); }
        Ok(text) => { text }
    }
}

#[test]
fn fixture_b1_exact() {
    // Every transaction of `testdata/bank_b1.pdf` with exact amounts, dates,
    // descriptions and provisional ids (the ids were verified by sha256 over
    // `date|amount-minor-units|EUR|description||bank_b1.pdf` with an EMPTY
    // counterparty slot, user decision I2).
    let text = fixture_text("bank_b1");
    let report = parse_bank_b_text(&text, "bank_b1.pdf");
    assert!(report.warnings.is_empty(), "the fixture is a clean bank B statement, warnings: {:?}", report.warnings);
    assert!(report.transactions.len() == 4, "expected four rows, got {:?}", report.transactions);

    // The amounts in the order the statement prints them.
    let amounts: Vec<i64> = report.transactions.iter().map(|row| row.amount).collect();
    assert!(amounts == [-8990, -2500, 85000, -3456], "amounts: {amounts:?}");

    // The dates, already canonical in layout B, stay canonical.
    let dates: Vec<&str> = report.transactions.iter().map(|row| row.date.as_str()).collect();
    assert!(dates == ["2024-02-03", "2024-02-11", "2024-02-17", "2024-02-24"], "dates: {dates:?}");

    // The descriptions exactly as the statement prints them — the card-number
    // column is DROPPED, the `;` of the second row is part of the description
    // and must not split the row further, and the `4711` of `ALDI 4711` is
    // merchant text, not a card number, because the card column is the token
    // directly behind the date.
    let descriptions: Vec<&str> = report.transactions.iter().map(|row| row.description.as_str()).collect();
    assert!(descriptions == ["ALDI 4711", "TAXI AIRPORT; NIGHT", "CLIENT PAYMENT", "REWE MARKT 12"], "descriptions: {descriptions:?}");

    // User decision I2 (supersedes planning decision 18): this importer
    // recognizes no counterparty, so the slot is empty on every row.
    for row in &report.transactions {
        assert!(row.counterparty == *"", "counterparty must stay empty: {:?}", row);
        assert!(row.currency == "EUR");
        assert!(row.source == "bank_b1.pdf");
    }

    // The provisional ids: the base id of the fields THIS parser produced
    // (verified by sha256 with an empty counterparty slot). They differ from
    // the hand-annotated ids of `testdata/transactions.csv`, which no Phase-4
    // parser can derive (the fixture pins a non-empty counterparty there); the
    // fixture bytes stay untouched. No occurrence suffix is ever invented here.
    let ids: Vec<&str> = report.transactions.iter().map(|row| row.id.as_str()).collect();
    assert!(ids == ["42b62ea5c6", "88a5a3a524", "9f6679bc82", "2b67dab066"], "ids: {ids:?}");
    for row in &report.transactions {
        assert!(row.id == transaction_base_id(row), "the id must be the provisional base id of the record: {:?}", row.id);
        assert!(!row.id.contains("#"), "no occurrence suffix here: {:?}", row.id);
    }
}

#[test]
fn fixture_b2_duplicate_pair_kept() {
    // The byte-identical adjacent `REWE MARKT 12 -15.00` pair of
    // `testdata/bank_b2.pdf`: BOTH rows survive the parser, with the same
    // provisional base id twice. The `#2` suffix of the pair on file
    // (`testdata/transactions.csv`) is assigned by the store at append time
    // (task 4.8), never by the parser.
    let text = fixture_text("bank_b2");
    let report = parse_bank_b_text(&text, "bank_b2.pdf");
    assert!(report.warnings.is_empty(), "the fixture is a clean bank B statement, warnings: {:?}", report.warnings);
    assert!(report.transactions.len() == 4, "expected four rows, got {:?}", report.transactions);

    let amounts: Vec<i64> = report.transactions.iter().map(|row| row.amount).collect();
    assert!(amounts == [-1590, -1500, -1500, -6789], "amounts: {amounts:?}");
    let dates: Vec<&str> = report.transactions.iter().map(|row| row.date.as_str()).collect();
    assert!(dates == ["2024-03-02", "2024-03-05", "2024-03-05", "2024-03-19"], "dates: {dates:?}");

    let ids: Vec<&str> = report.transactions.iter().map(|row| row.id.as_str()).collect();
    assert!(ids == ["14b233a536", "96c4015370", "96c4015370", "0701248af6"], "ids: {ids:?}");
    for row in &report.transactions {
        assert!(!row.id.contains("#"), "the parser never assigns an occurrence suffix: {:?}", row.id);
        assert!(row.id == transaction_base_id(row), "the id must be the provisional base id of the record: {:?}", row.id);
        assert!(row.counterparty == *"", "counterparty must stay empty: {:?}", row);
    }
}

#[test]
fn broken_variant_warns_and_keeps_rows() {
    // The fixture layout with one corrupted amount row: the row is reported and
    // dropped, the rest of the statement survives.
    let text = "Bank B N -- Broken Statement\
                 - Monthly Statement\n\
                 Date      Card-Number     Description        Amount (EUR)\n\
                 2024-02-03 4711-0000-4711 ALDI 4711                      -89.90 EUR\n\
                 2024-02-11 4711-0000-4711 TAXI AIRPORT; NIGHT                 FOO,BAR EUR\n\
                 2024-02-17 4711-0000-4711 CLIENT PAYMENT                     850.00 EUR\n\
                 2024-02-24 4711-0000-4711 REWE MARKT 12                     -34.56 EUR\n\
                 Total: 700.54 EUR\n";
    let report = parse_bank_b_text(text, "broken_b.pdf");

    assert!(report.transactions.len() == 3, "expected three rows, got {:?}", report.transactions);
    assert!(report.warnings.len() == 1, "expected exactly one warning, got {:?}", report.warnings);
    assert!(report.warnings[0].line == 4, "the corrupted row is the fourth line: {:?}", report.warnings[0]);
    assert!(report.warnings[0].text.contains("FOO,BAR"), "the warning must carry the row: {:?}", report.warnings[0]);

    let amounts: Vec<i64> = report.transactions.iter().map(|row| row.amount).collect();
    assert!(amounts == [-8990, 85000, -3456], "amounts: {amounts:?}");
    for row in &report.transactions {
        assert!(row.counterparty == *"", "counterparty must stay empty: {:?}", row);
    }
}

#[test]
fn twice_identical() {
    // The parser is a pure function: no id, no counter and no clock enters the
    // record, so a second run on the same text is byte-identical.
    let text = fixture_text("bank_b1");
    let first = parse_bank_b_text(&text, "bank_b1.pdf");
    let second = parse_bank_b_text(&text, "bank_b1.pdf");
    assert!(first.transactions == second.transactions);
    assert!(first.warnings == second.warnings);
}

#[test]
fn missing_statement_marker_warns_but_still_parses() {
    // The broken-variant requirement of plan item 4.3: an unrecognizable
    // statement is reported (line 0) and its rows are parsed anyway.
    let text = "Some other document\n\
                 2024-02-03 4711-0000-4711 ALDI 4711 -89.90 EUR\n";
    let report = parse_bank_b_text(text, "unknown.pdf");
    assert!(report.warnings == [BankRowWarning { line: 0, text: "not a Bank B statement".to_string() }]);
    assert!(report.transactions.len() == 1);
    assert!(report.transactions[0].amount == -8990);
    assert!(report.transactions[0].description == "ALDI 4711");
    assert!(report.transactions[0].counterparty == "");
}

#[test]
fn header_and_summary_lines_never_yield_transactions() {
    // The statement header, the `Kredit:`/`Monat:` lines, the table header,
    // the footer and above all the `Total:` line are never rows.
    let text = fixture_text("bank_b1");
    let report = parse_bank_b_text(&text, "bank_b1.pdf");
    assert!(report.warnings.is_empty(), "no line of the fixture may look like a broken row: {:?}", report.warnings);
    // The summary total of the statement (700.54) is not a transaction.
    for row in &report.transactions {
        assert!(row.amount != 70054, "the summary total must not become a transaction: {:?}", row);
        assert!(!row.description.contains("Total"), "no summary line may become a row: {:?}", row);
    }
}

#[test]
fn card_number_column_is_dropped_and_must_have_its_shape() {
    // A candidate row whose card column is NOT the `XXXX-XXXX-XXXX` shape is a
    // typed warning; the card number of a good row never leaks into the
    // description.
    let good = parse_bank_b_text("Bank B N -- Card Check\n2024-02-03 4711-0000-4711 ALDI 4711 -89.90 EUR\n", "card_b.pdf");
    assert!(good.warnings.is_empty(), "warnings: {:?}", good.warnings);
    assert!(good.transactions[0].description == "ALDI 4711", "the card number is dropped: {:?}", good.transactions[0]);

    let bad = parse_bank_b_text("Bank B N -- Card Check\n2024-02-03 ****-****-4711 ALDI 4711 -89.90 EUR\n", "card_b.pdf");
    assert!(bad.transactions.is_empty(), "a masked card column is not a row of this layout: {:?}", bad.transactions);
    assert!(bad.warnings.len() == 1 && bad.warnings[0].line == 2, "warnings: {:?}", bad.warnings);
    assert!(bad.warnings[0].text.contains("****-****-4711"), "the warning names the rejected token: {:?}", bad.warnings[0]);
}

#[test]
fn unparseable_date_on_a_candidate_row_warns() {
    // The shape check accepts `YYYY-MM-DD`, so only an impossible calendar date
    // reaches `domain::date` and is turned into a warning instead of a panic.
    let text = "Bank B N -- Impossible Dates\n\
                 2024-02-31 4711-0000-4711 ALDI 4711 -89.90 EUR\n\
                 2024-02-03 4711-0000-4711 ALDI 4711 -89.90 EUR\n";
    let report = parse_bank_b_text(text, "impossible_b.pdf");
    assert!(report.transactions.len() == 1, "the impossible date row is dropped: {:?}", report.transactions);
    assert!(report.warnings.len() == 1);
    assert!(report.warnings[0].line == 2, "{:?}", report.warnings[0]);
    assert!(report.transactions[0].amount == -8990);
    assert!(report.transactions[0].counterparty == "");
}

#[test]
fn euro_sign_spelling_is_tolerated() {
    // Layout B prints `EUR`; the same field is printed as `€` by other bank B
    // style statements, and the parser accepts both.
    let text = "Bank B N -- Euro Sign Statement\n\
                 2024-02-03 4711-0000-4711 ALDI 4711 -89.90 €\n";
    let report = parse_bank_b_text(text, "sign_b.pdf");
    assert!(report.warnings.is_empty(), "warnings: {:?}", report.warnings);
    assert!(report.transactions.len() == 1);
    assert!(report.transactions[0].amount == -8990);
    assert!(report.transactions[0].counterparty == *"");
}

#[test]
fn the_wrapper_reads_a_pdf_file() {
    // The production entry point: same report as the pure text parser, and a
    // missing file is the typed error of plan item 4.1 rather than a panic.
    let path = find_testdata_dir().join("bank_b1.pdf");
    let from_file = match parse_bank_b(&path, "bank_b1.pdf") {
        Err(reason) => { panic!("the fixture bank_b1.pdf must parse, got {reason:?}"); }
        Ok(report) => { report }
    };
    let from_text = parse_bank_b_text(&fixture_text("bank_b1"), "bank_b1.pdf");
    assert!(from_file == from_text);

    let missing = find_testdata_dir().join("does-not-exist-bank-b.pdf");
    match parse_bank_b(&missing, "does-not-exist-bank-b.pdf") {
        Err(PdfTextError::Failed(_)) => { }
        other => { panic!("a missing PDF file must be a typed Failed error, got {other:?}"); }
    }
}
