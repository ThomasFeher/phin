//! The built-in importer of plan item 4.2: a parser for the statement layout of
//! fixture bank A (`testdata/bank_a.pdf`).
//!
//! Per `doc/architecture/architecture.md` ("Import & Extraction") phin has two
//! built-in statement readers; this is the one of them that reads layout A, i.e.
//! `DD.MM.YYYY` dates, German amount spelling (`1.234,56`), trailing ` EUR`, and
//! the whole description in one column. It is built on
//! [`crate::technical::pdf_text`] (plan item 4.1) and nothing else: date and
//! amount normalization are delegated to [`crate::domain::date`] and
//! [`crate::domain::money`], the record is built with
//! [`crate::domain::transaction::from_parts`] (the importer constructor, never a
//! minor-unit count), and the provisional id comes from
//! [`crate::technical::id::transaction_base_id`].
//!
//! Two things are deliberately NOT done here:
//!
//! * **No occurrence suffix.** The id returned is the bare base id; the `#N`
//!   suffix of duplicated content is assigned by the store when a line is
//!   appended, because it depends on the ids already on file
//!   (`doc/architecture/data-files.md`, `core/src/technical/id.rs::occurrence_id`).
//! * **No counterparty recognition.** `doc/architecture/data-files.md:71`
//!   documents the column as the counterparty name "as recognized by the
//!   importer", and `core/src/domain/transaction.rs:60` documents the slot as
//!   "empty when unknown". The Phase-4 importers recognize nothing, so
//!   `counterparty` is the empty string; the description is never copied into
//!   it (user decision I2, which supersedes planning decision 18). A later,
//!   specified heuristic would fill the slot in this file and nowhere else.
//!
//! The parser is permissive in one direction only: a statement whose rows do not
//! fit the layout produces a typed [`BankRowWarning`] naming the line and keeps
//! the rest of the file, and a text that is not a bank A statement at all is
//! reported as such but still parsed. It never panics and never writes anything.

use std::path::Path;

use crate::domain::date::parse as date_parse;
use crate::domain::money::parse as money_parse;
use crate::domain::money::print as money_print;
use crate::domain::transaction::from_parts as transaction_from_parts;
use crate::domain::transaction::Transaction;
use crate::technical::id::transaction_base_id;
use crate::technical::pdf_text::extract_text;
use crate::technical::pdf_text::PdfTextError;

/// The currency layout A prints and the only currency the fixture carries;
/// layout A has no per-row currency field, only an `Amount (EUR)` column header.
const CURRENCY: &str = "EUR";

/// Marker naming this statement producer; a text without it on its first line is
/// reported as "not a Bank A statement" (plan item 4.2's broken-variant case).
const STATEMENT_MARKER: &str = "Bank A AG";

/// A problem on one specific line of the statement text.
///
/// `line` is 1-based, except for the whole-file problems (e.g. "this is not a
/// bank A statement"), which use line 0. `text` is the offending line as read,
/// so a reviewer can find it in the source document.
#[derive(Debug, Eq, PartialEq)]
pub struct BankRowWarning {
    /// 1-based line number, or 0 for a statement-wide problem.
    pub line: usize,
    /// What was wrong, phrased for a human (or a `log` event of plan item 4.8).
    pub text: String,
}

/// What parsing a statement text produced: the records that could be built and
/// the lines that could not.
#[derive(Debug, Eq, PartialEq)]
pub struct ParseReport {
    /// The transactions that were read, in the order the statement prints them.
    pub transactions: Vec<Transaction>,
    /// Every line that was skipped or rejected, in line order.
    pub warnings: Vec<BankRowWarning>,
}

/// Read the PDF at `path` with [`crate::technical::pdf_text::extract_text`] and
/// parse it as a bank A statement.
///
/// `source` is the provenance string stored in the record (production passes the
/// file name, e.g. `bank_a.pdf`); [`crate::technical::id::source_basename`]
/// normalizes a path later, so passing a full path still hashes to the same id.
/// A missing `pdftotext` or an unreadable file is the typed [`PdfTextError`] of
/// plan item 4.1, which plan item 4.4/4.8 maps to the `converter-missing` or
/// `io` error code.
pub fn parse_bank_a(path: &Path, source: &str) -> Result<ParseReport, PdfTextError> {
    let text = match extract_text(path) {
        Err(reason) => { return Err(reason); }
        Ok(text) => { text }
    };
    return Ok(parse_bank_a_text(&text, source));
}

/// Parse the extracted text of a bank A statement. Pure: the same text and the
/// same `source` yield a byte-identical [`ParseReport`].
///
/// A line is a candidate row when it starts with a `DD.MM.YYYY` date; every
/// other line (statement header, table header, summary, `Total:` line, blank
/// line, form feed) is skipped silently. A candidate row whose date, amount or
/// record cannot be built becomes a [`BankRowWarning`] and the parse continues.
pub fn parse_bank_a_text(text: &str, source: &str) -> ParseReport {
    let mut report = ParseReport {
        transactions: Vec::new(),
        warnings: Vec::new(),
    };

    // The statement-wide check comes first so its warning is the first one a
    // reader sees, and so the rows are still parsed (the broken-variant
    // requirement of plan item 4.2: a damaged header must not lose the rows).
    let recognized = match text.lines().next() {
        None => { false }
        Some(head) => { head.contains(STATEMENT_MARKER) }
    };
    if !recognized {
        report.warnings.push(BankRowWarning {
            line: 0,
            text: "not a Bank A statement".to_string(),
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

/// True when `line` begins with a `DD.MM.YYYY` date token, the only marker of a
/// statement row in layout A. Headers, summaries and the `Total:` line cannot
/// satisfy it, which is why they are skipped without a warning.
fn starts_with_date(line: &str) -> bool {
    let mut parts = line.split_whitespace();
    match parts.next() {
        None => { false }
        Some(token) => { looks_like_layout_a_date(token) }
    }
}

/// True when `token` is exactly `DD.MM.YYYY` with digits in every position
/// (calendar validity is checked by [`crate::domain::date`], not here).
fn looks_like_layout_a_date(token: &str) -> bool {
    let bytes = token.as_bytes();
    if bytes.len() != 10 {
        return false;
    }
    for (index, byte) in bytes.iter().enumerate() {
        match index {
            2 | 5 => {
                if *byte != b'.' {
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
/// The leading date token is cut off, the trailing ` EUR` / `€` marker is
/// stripped, and the last remaining token is the amount; everything between is
/// the description. Currency symbols are stripped here, before
/// [`crate::domain::money::parse`] sees the token, so the parser itself never
/// knows a currency symbol.
fn build_row(line: &str, source: &str) -> Result<Transaction, String> {
    // Strip the printed currency marker of layout A first (case-sensitive
    // `EUR`, with `€` tolerated as the other spelling of the same field), then
    // split the row into its three parts.
    let without_marker = strip_currency_marker(line);
    let mut tokens = without_marker.split_whitespace();
    let date_token = match tokens.next() {
        None => { return Err(format!("row without a date: {line}")); }
        Some(token) => { token.to_string() }
    };
    // A row that is only its own date has no amount token at all: `next_back`
    // yields None once the front token was already consumed, so that case is
    // the `None` arm below.
    let amount_token = match tokens.next_back() {
        None => { return Err(format!("row without an amount: {line}")); }
        Some(token) => { token.to_string() }
    };

    // `starts_with_date` accepted the shape of the date token, so this can only
    // fail on an impossible calendar date such as `31.02.2024`.
    let date = match date_parse(&date_token) {
        Err(reason) => { return Err(format!("rejected date on row: {line} ({reason:?})")); }
        Ok(normalized) => { normalized }
    };

    let amount = match money_parse(&amount_token) {
        Err(reason) => { return Err(format!("rejected amount {amount_token} on row: {line} ({reason:?})")); }
        Ok(minor_units) => { minor_units }
    };

    // What is left between the date and the amount is the merchant text; the
    // column padding of `pdftotext -layout` is collapsed into single spaces.
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
    // occurrence-suffixed id (that suffix needs the ids already on file).
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
/// Copied from the same pattern `crate::technical::id` and
/// `crate::technical::pdf_text` use (and kept private to this module):
/// `cargo test` runs the test binary with the package root (`core/`) as its
/// working directory, so the fixture directory sits one level up.
#[cfg(test)]
fn find_testdata_dir() -> std::path::PathBuf {
    let start = std::env::current_dir().unwrap_or(std::path::PathBuf::new());
    let mut dir = start;
    loop {
        let candidate = dir.join("testdata");
        if candidate.is_dir() { return candidate; }
        match dir.parent() {
            None => { panic!("parse_bank_a tests cannot find testdata relative to the test's working directory"); }
            Some(parent) => { dir = parent.to_path_buf(); }
        }
    }
}

/// The extracted text of `testdata/bank_a.pdf`, extracted once per test that
/// needs it (the fixtures are read, never written).
#[cfg(test)]
fn fixture_bank_a_text() -> String {
    let path = find_testdata_dir().join("bank_a.pdf");
    match extract_text(&path) {
        Err(reason) => { panic!("pdftotext must extract the fixture bank_a.pdf, got {reason:?}"); }
        Ok(text) => { text }
    }
}

#[test]
fn fixture_a_yields_exact_transactions() {
    let text = fixture_bank_a_text();
    let report = parse_bank_a_text(&text, "bank_a.pdf");
    assert!(report.warnings.is_empty(), "the fixture is a clean bank A statement, warnings: {:?}", report.warnings);
    assert!(report.transactions.len() == 6, "expected six rows, got {:?}", report.transactions);

    // The amounts in the order the statement prints them.
    let amounts: Vec<i64> = report.transactions.iter().map(|row| row.amount).collect();
    assert!(amounts == [-3999, -5990, 42000, -1847, -123456, -12990], "amounts: {amounts:?}");

    // The dates, normalized to the canonical storage form.
    let dates: Vec<&str> = report.transactions.iter().map(|row| row.date.as_str()).collect();
    assert!(dates == ["2024-01-03", "2024-01-09", "2024-01-15", "2024-01-21", "2024-01-24", "2024-01-28"], "dates: {dates:?}");

    // The descriptions exactly as the fixture prints them, padding collapsed.
    let descriptions: Vec<&str> = report.transactions.iter().map(|row| row.description.as_str()).collect();
    assert!(descriptions == ["REWE MARKT 12", "DINNER ZUR LEITZ", "PAYMENT TO TOBIAS", "BOOKS ONLINE", "MEGA MART 04", "LIDL 0815"], "descriptions: {descriptions:?}");

    // User decision I2 (supersedes planning decision 18): this importer
    // recognizes no counterparty, so the slot is empty, and the description is
    // not copied into it.
    for row in &report.transactions {
        assert!(row.counterparty == *"", "counterparty must stay empty: {:?}", row);
        assert!(row.currency == "EUR");
        assert!(row.source == "bank_a.pdf");
    }

    // The provisional ids: the base id of the fields THIS parser produced
    // (sha256 of `date|amount-minor-units|EUR|description||bank_a.pdf`, first
    // 10 hex, counterparty slot empty). They differ from the hand-annotated ids
    // of `testdata/transactions.csv`, which no Phase-4 parser can derive (see
    // the findings log); the fixture bytes stay untouched. No occurrence
    // suffix is ever invented here.
    let ids: Vec<&str> = report.transactions.iter().map(|row| row.id.as_str()).collect();
    assert!(ids == ["f694f9c9cf", "b9e2c15681", "beb2aa2192", "a65e62e0a5", "04dfddfd6f", "967d1a6610"], "ids: {ids:?}");
    for row in &report.transactions {
        assert!(row.id == transaction_base_id(row), "the id must be the provisional base id of the record: {:?}", row.id);
        assert!(!row.id.contains("#"), "no occurrence suffix here: {:?}", row.id);
    }
}

#[test]
fn broken_variant_warns_and_keeps_rows() {
    // The fixture layout with one corrupted amount row: the row is reported and
    // dropped, the rest of the statement survives.
    let text = "Bank A AG -- Broken Statement\n\
                 Date     Description         Amount (EUR)\n\
                 03.01.2024 REWE MARKT 12                -39,99 EUR\n\
                 09.01.2024 DINNER ZUR LEITZ              FOO,BAR EUR\n\
                 15.01.2024 PAYMENT TO TOBIAS               420,00 EUR\n\
                 21.01.2024 BOOKS ONLINE                 -18,47 EUR\n\
                 24.01.2024 MEGA MART 04               -1.234,56 EUR\n\
                 28.01.2024 LIDL 0815              -129,90 EUR\n\
                 Total: -1.062,82 EUR\n";
    let report = parse_bank_a_text(text, "broken_a.pdf");

    assert!(report.transactions.len() == 5, "expected five rows, got {:?}", report.transactions);
    assert!(report.warnings.len() == 1, "expected exactly one warning, got {:?}", report.warnings);
    assert!(report.warnings[0].line == 4, "the corrupted row is the fourth line: {:?}", report.warnings[0]);
    assert!(report.warnings[0].text.len() > 0, "the warning must name the offending row");
    assert!(report.warnings[0].text.contains("FOO,BAR"), "the warning must carry the row: {:?}", report.warnings[0]);

    let amounts: Vec<i64> = report.transactions.iter().map(|row| row.amount).collect();
    assert!(amounts == [-3999, 42000, -1847, -123456, -12990], "amounts: {amounts:?}");
    for row in &report.transactions {
        assert!(row.counterparty == *"", "counterparty must stay empty: {:?}", row);
    }
}

#[test]
fn twice_identical() {
    // The parser is a pure function: no id, no counter and no clock enters the
    // record, so a second run on the same text is byte-identical.
    let text = fixture_bank_a_text();
    let first = parse_bank_a_text(&text, "bank_a.pdf");
    let second = parse_bank_a_text(&text, "bank_a.pdf");
    assert!(first.transactions == second.transactions);
    assert!(first.warnings == second.warnings);
}

#[test]
fn two_column_number_stays_on_row() {
    // The `-layout` output pads the columns with wide runs of spaces; the amount
    // is the last token before the currency marker no matter how wide the gap.
    let text = "Bank A AG -- Padded Statement\n\
                 03.01.2024 REWE MARKT 12                -39,99 EUR\n";
    let report = parse_bank_a_text(text, "padded_a.pdf");
    assert!(report.warnings.is_empty(), "a padded row is a clean row, warnings: {:?}", report.warnings);
    assert!(report.transactions.len() == 1, "expected one row, got {:?}", report.transactions);
    let row = &report.transactions[0];
    assert!(row.amount == -3999);
    assert!(row.description == "REWE MARKT 12");
    assert!(row.counterparty == *"", "counterparty must stay empty: {:?}", row);
    assert!(row.date == "2024-01-03");
}

#[test]
fn header_and_summary_lines_never_yield_transactions() {
    // The statement header, the table header, the period lines, the summary page
    // and above all the `Total:` line are never rows.
    let text = fixture_bank_a_text();
    let report = parse_bank_a_text(&text, "bank_a.pdf");
    assert!(report.warnings.is_empty(), "no line of the fixture may look like a broken row: {:?}", report.warnings);
    // The summary total of the statement (-1.062,82) is not a transaction.
    for row in &report.transactions {
        assert!(row.amount != -106282, "the summary total must not become a transaction: {:?}", row);
        assert!(!row.description.contains("Total"), "no summary line may become a row: {:?}", row);
    }
}

#[test]
fn missing_statement_marker_warns_but_still_parses() {
    // The broken-variant requirement of plan item 4.2: an unrecognizable
    // statement is reported (line 0) and its rows are parsed anyway.
    let text = "Some other document\n\
                 03.01.2024 REWE MARKT 12 -39,99 EUR\n";
    let report = parse_bank_a_text(text, "unknown.pdf");
    assert!(report.warnings == [BankRowWarning { line: 0, text: "not a Bank A statement".to_string() }]);
    assert!(report.transactions.len() == 1);
    assert!(report.transactions[0].amount == -3999);
    assert!(report.transactions[0].counterparty == *"");
}

#[test]
fn unparseable_date_on_a_candidate_row_warns() {
    // The shape check accepts `DD.MM.YYYY`, so only an impossible calendar date
    // reaches `domain::date` and is turned into a warning instead of a panic.
    let text = "Bank A AG -- Impossible Dates\n\
                 31.02.2024 REWE MARKT 12 -39,99 EUR\n\
                 03.01.2024 REWE MARKT 12 -39,99 EUR\n";
    let report = parse_bank_a_text(text, "impossible_a.pdf");
    assert!(report.transactions.len() == 1, "the impossible date row is dropped: {:?}", report.transactions);
    assert!(report.warnings.len() == 1);
    assert!(report.warnings[0].line == 2, "{:?}", report.warnings[0]);
    assert!(report.transactions[0].amount == -3999);
    assert!(report.transactions[0].counterparty == "");
}

#[test]
fn euro_sign_spelling_is_tolerated() {
    // Layout A prints `EUR`; the same field is printed as `€` by other bank A
    // style statements, and the parser accepts both.
    let text = "Bank A AG -- Euro Sign Statement\n\
                 03.01.2024 REWE MARKT 12 -39,99 €\n";
    let report = parse_bank_a_text(text, "sign_a.pdf");
    assert!(report.warnings.is_empty(), "warnings: {:?}", report.warnings);
    assert!(report.transactions.len() == 1);
    assert!(report.transactions[0].amount == -3999);
    assert!(report.transactions[0].counterparty == *"");
}

#[test]
fn the_wrapper_reads_a_pdf_file() {
    // The production entry point: same report as the pure text parser, and a
    // missing file is the typed error of plan item 4.1 rather than a panic.
    let path = find_testdata_dir().join("bank_a.pdf");
    let from_file = match parse_bank_a(&path, "bank_a.pdf") {
        Err(reason) => { panic!("the fixture bank_a.pdf must parse, got {reason:?}"); }
        Ok(report) => { report }
    };
    let from_text = parse_bank_a_text(&fixture_bank_a_text(), "bank_a.pdf");
    assert!(from_file == from_text);

    let missing = find_testdata_dir().join("does-not-exist-bank-a.pdf");
    match parse_bank_a(&missing, "does-not-exist-bank-a.pdf") {
        Err(PdfTextError::Failed(_)) => { }
        other => { panic!("a missing PDF file must be a typed Failed error, got {other:?}"); }
    }
}
