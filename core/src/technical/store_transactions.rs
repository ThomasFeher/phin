//! The store reader of `transactions.csv` — plan item 2.3, reader half.
//!
//! This module turns the TEXT of a `transactions.csv` file into
//! [`crate::domain::transaction::Transaction`] values. Opening the file
//! (and the atomic rename of plan item 2.5) belong to other parts
//! of the store; the reader itself performs no I/O, exactly like the codec
//! in [`crate::technical::csv`] that it delegates every line to. The writer
//! half of plan item 2.5 lives here as [`append`]: text in, text out,
//! still no I/O — it appends one rendered line to the file text and never
//! touches the bytes before it.
//!
//! The contract applied here is `doc/architecture/data-files.md`, section
//! "`transactions.csv` — facts, never edited in place" (line 60): the fixed
//! column set `id;date;amount;currency;description;counterparty;source`,
//! `amount` as an `i64` count of minor units, `date` already canonical
//! `YYYY-MM-DD`. Comment lines (`#`, e.g. the `# columns:` preamble) and
//! empty/whitespace-only lines are skipped, as the codec contract of
//! `doc/architecture/data-files.md:14-27` requires.
//!
//! A corrupt line must NOT abort the read: [`read`] returns the rows that
//! parsed successfully AND one typed [`ReadError`] per line that did not,
//! each carrying the 1-based line number of the offending line (decision 10:
//! typed return values, no events/commands).
//!
//! Rows are built ONLY through
//! [`crate::domain::transaction::from_stored`]: `transactions.csv` stores
//! the amount already as a minor-unit count, so the display-form
//! constructor `from_parts` would scale it a second time (`money::parse("-3999")`
//! yields `-399900`) and is forbidden here — see the invariant in
//! `core/src/domain/transaction.rs`.

use super::csv::parse_record;
use super::csv::render_record;
use super::csv::CodecError;
use super::id::transaction_base_id;
use super::id::matches_base;
use crate::domain::transaction::from_stored;
use crate::domain::transaction::BuildError;
use crate::domain::transaction::Transaction;

/// Number of columns of `transactions.csv`
/// (`id;date;amount;currency;description;counterparty;source`,
/// `doc/architecture/data-files.md:60`).
const COLUMN_COUNT: usize = 7;

/// Why one line of `transactions.csv` could not become a row.
///
/// Every variant carries the 1-BASED line number of the offending line, so
/// the caller can report corruption to the user with the position they would
/// see in an editor. The underlying codec or constructor reason is carried
/// through unchanged.
#[derive(Debug)]
pub enum ReadError {
    /// [`super::csv::parse_record`] rejected the line; the reason is the
    /// codec's own ([`super::csv::CodecError`]: unterminated quote, stray
    /// quote, …).
    MalformedLine(usize, CodecError),
    /// The line decoded into a record, but not into the contract's
    /// [`COLUMN_COUNT`] columns; the second number is what the line actually
    /// held, so the report can say "line 17 had 5 columns, expected 7".
    WrongColumnCount(usize, usize),
    /// The `amount` column was not a plain `i64` minor-unit count — e.g. an
    /// empty field, or the display form `-39,99 €` that only importers may
    /// write, which must never reach `transactions.csv`.
    InvalidAmountNumber(usize),
    /// [`crate::domain::transaction::from_stored`] rejected the row
    /// (non-canonical or unparsable date); the reason is the constructor's
    /// own [`crate::domain::transaction::BuildError`].
    BuildRejected(usize, BuildError),
}

/// Read the whole text of a `transactions.csv` file into rows and typed errors.
///
/// Lines are counted 1-based, as an editor would number them. A line that is
/// empty or whitespace-only is ignored, a line starting with `#` is a comment
/// and ignored (the `# columns:` preamble and hand-written notes survive in
/// the file, they simply produce no row). Every other line is decoded by
/// [`super::csv::parse_record`], and a line the codec rejects becomes the
/// typed error [`ReadError.MalformedLine`] carrying the codec's reason —
/// reading never aborts on corruption.
///
/// Lines are cut on `\n`; one trailing `\r` is dropped per line, tolerating a
/// CRLF file while the codec writes LF (`doc/architecture/data-files.md`,
/// "Line endings"). A `\r` anywhere else stays part of the line, so
/// [`super::csv::parse_record`] rejects it as
/// [`super::csv::CodecError.NewlineInField`] — corruption, not a line ending.
///
/// Each record must hold exactly [`COLUMN_COUNT`] fields; the fields are
/// handed to [`crate::domain::transaction::from_stored`] in contract column
/// order, with the `amount` field converted to the stored `i64` minor-unit
/// count as-is. A rejected line never aborts the read: the successfully
/// parsed rows are returned together with one [`ReadError`] per bad line.
pub fn read(text: &str) -> (Vec::<Transaction>, Vec::<ReadError>) {
    let mut rows = Vec::<Transaction>::new();
    let mut errors = Vec::<ReadError>::new();
    let mut number = 0;
    for raw_line in text.split("\n") {
        number = number + 1;
        let line = drop_carriage_return(&raw_line);
        if is_blank(&line) {
            continue;
        }
        if line.starts_with("#") {
            continue;
        }
        match parse_record(&line) {
            Err(reason) => { errors.push(ReadError::MalformedLine(number, reason)); }
            Ok(fields) => {
                if fields.len() != COLUMN_COUNT {
                    errors.push(ReadError::WrongColumnCount(number, fields.len()));
                    continue;
                }
                match fields[2].parse::<i64>() {
                    Err(_) => { errors.push(ReadError::InvalidAmountNumber(number)); }
                    Ok(minor_units) => {
                        match from_stored(&fields[0], &fields[1], minor_units, &fields[3], &fields[4], &fields[5], &fields[6]) {
                            Err(reason) => { errors.push(ReadError::BuildRejected(number, reason)); }
                            Ok(transaction) => { rows.push(transaction); }
                        }
                    }
                }
            }
        }
    }
    return (rows, errors);
}

/// The line without one trailing `\r`; a line without one is returned as-is.
fn drop_carriage_return(line: &str) -> String {
    if line.ends_with("\r") {
        return line[..line.len() - "\r".len()].to_string();
    }
    return line.to_string();
}

/// True when the line is empty or holds only whitespace.
fn is_blank(line: &str) -> bool {
    for character in line.chars() {
        if character != ' ' && character != '\t' {
            return false;
        }
    }
    return true;
}

/// Whether an [`append`] wrote a line or skipped the candidate.
#[derive(Debug, Eq, PartialEq)]
pub enum AppendOutcome {
    /// The candidate's computed id was not yet in the file: one line was
    /// appended after the existing text.
    Appended,
    /// The candidate's computed id is already in the file — the idempotence
    /// rule of `doc/architecture/data-files.md` ("before appending, the core
    /// skips candidates whose id is already known"): the text is returned
    /// unchanged, byte for byte.
    Skipped,
}

/// Append one transaction line to the text of a `transactions.csv` file.
///
/// Text in, text out — like the reader above, this performs no I/O: the
/// part of plan item 2.5 that opens the file and does the atomic
/// write-temporary-then-rename stays with the caller.
///
/// The id of the appended line is NOT taken from `tx.id` but recomputed
/// from the stored fields with [`super::id::transaction_base_id`], so a
/// candidate an importer built fresh lands on the same id as the row
/// already on file if it describes the same fact. The candidate is SKIPPED
/// when that base id is already known: any stored id that matches the base
/// ([`super::id::matches_base`] — the bare base or one of its occurrence-
/// suffixed forms `base#N`) counts as known, which is what keeps a re-
/// import from duplicating rows. The known ids are read back out of the
/// file text with [`read`]; rows that failed to parse carry no usable id
/// and cannot block a candidate.
///
/// The existing text is never touched: the line is rendered by
/// [`super::csv::render_record`] over the contract column order
/// `id;date;amount;currency;description;counterparty;source` and appended
/// after the existing text — with one LF added first if the file did not
/// end with a line ending, and one LF after the new line — so the new line
/// is the only change and every old line stays byte-identical.
///
/// A field the codec cannot render — a line break inside `description` or
/// `counterparty` — is the codec's own
/// [`super::csv::CodecError.NewlineInField`], returned unchanged.
pub fn append(text: &str, tx: &Transaction) -> Result::<(String, AppendOutcome), CodecError> {
    let base = transaction_base_id(tx);
    let (rows, _) = read(text);
    for row in rows {
        if matches_base(&row.id, &base) {
            return Ok((text.to_string(), AppendOutcome::Skipped));
        }
    }
    let mut fields = Vec::<String>::new();
    fields.push(base.to_string());
    fields.push(tx.date.to_string());
    fields.push(tx.amount.to_string());
    fields.push(tx.currency.to_string());
    fields.push(tx.description.to_string());
    fields.push(tx.counterparty.to_string());
    fields.push(tx.source.to_string());
    match render_record(&fields) {
        Err(reason) => { return Err(reason); }
        Ok(line) => {
            let mut new_text = text.to_string();
            if new_text.len() > 0 && !new_text.ends_with("\n") {
                new_text.push_str("\n");
            }
            new_text.push_str(&line);
            new_text.push_str("\n");
            return Ok((new_text, AppendOutcome::Appended));
        }
    }
}

/// The verbatim text of `testdata/transactions.csv`: the four `#` preamble
/// comment lines, then the 22 real rows (lines 5-26), each line ending with
/// LF. Embedded verbatim so the fixture test does not depend on the file
/// being present at the test's working directory.
#[cfg(test)]
const FIXTURE_TEXT: &str = r#"# Phin transactions -- append-only facts, never edited in place.
# columns: id;date;amount;currency;description;counterparty;source
# amount is an integer in minor units (cents); negative = expense.
# dates are normalised to YYYY-MM-DD; id = short content hash.
39945b2fed;2024-01-03;-3999;EUR;REWE MARKT 12;REWE MARKET;bank_a.pdf
f596357483;2024-01-09;-5990;EUR;DINNER ZUR LEITZ;ZUR LEITZ;bank_a.pdf
dc0db110c4;2024-01-15;42000;EUR;PAYMENT TO TOBIAS;TAXI BERLIN;bank_a.pdf
3d7478d7cc;2024-01-21;-1847;EUR;BOOKS ONLINE;THONIX;bank_a.pdf
2b5a9d2799;2024-01-24;-123456;EUR;MEGA MART 04;MEGA MART;bank_a.pdf
7a8051207a;2024-01-28;-12990;EUR;LIDL 0815;LIDL;bank_a.pdf
a86d11da9a;2024-02-03;-8990;EUR;ALDI 4711;ALDI;bank_b1.pdf
140ef279a4;2024-02-11;-2500;EUR;"TAXI AIRPORT; NIGHT";TAXI BERLIN;bank_b1.pdf
e86f3fc214;2024-02-17;85000;EUR;CLIENT PAYMENT;ALICE;bank_b1.pdf
fbd45cccf1;2024-02-24;-3456;EUR;REWE MARKT 12;REWE MARKET;bank_b1.pdf
58ed99aa4c;2024-03-02;-1590;EUR;CAFETARIA;CAFETARIA;bank_b2.pdf
80c63211a8;2024-03-05;-1500;EUR;REWE MARKT 12;REWE MARKET;bank_b2.pdf
80c63211a8#2;2024-03-05;-1500;EUR;REWE MARKT 12;REWE MARKET;bank_b2.pdf
5b35bef1fd;2024-03-19;-6789;EUR;AMAZON MARKETPLACE;AMZN;bank_b2.pdf
79caf386d4;2024-04-02;2599;EUR;LEGO STORE DE;LEGO STORE;mail_paypal.mbox
4fd4783f1e;2024-04-05;-1999;EUR;NETFLIX.COM;NETFLIX;mail_paypal.mbox
30fc834ea9;2024-04-09;899;EUR;BAKERY BERLIN;BAKERY BERLIN;mail_paypal.mbox
bad5886754;2024-04-12;-4500;EUR;STEAM PURCHASE;VALVE;mail_paypal.mbox
0cfe0127e0;2024-04-15;12000;EUR;CLIENT PAYMENT;ALICE;mail_paypal.mbox
3d3a5287f1;2024-04-18;-799;EUR;BOOKS ONLINE;THONIX;mail_paypal.mbox
06113c369c;2024-04-21;3450;EUR;"SUPERMARKET; DELICATES";DELICATES;mail_paypal.mbox
f9aeec83dc;2024-04-26;-2999;EUR;"TAXI AIRPORT; NIGHT";TAXI BERLIN;mail_paypal.mbox
"#;

#[test]
fn fixture_of_twenty_two_real_rows_parses_without_errors() {
    let (rows, errors) = read(FIXTURE_TEXT);
    assert!(rows.len() == 22);
    assert!(errors.len() == 0);
    // Spot-check both constructors-critical values: the quoted description
    // with an embedded `;` decodes, and the stored amount stays a minor-unit
    // count (line 12, `140ef279a4`, must be -2500, NOT -250000).
    let taxi = from_stored("140ef279a4", "2024-02-11", -2500, "EUR", "TAXI AIRPORT; NIGHT", "TAXI BERLIN", "bank_b1.pdf");
    assert!(rows[7] == taxi.unwrap());
    assert!(rows[7].amount == -2500);
    assert!(rows[7].description == "TAXI AIRPORT; NIGHT");
    // The duplicate id with `#N` suffix survives as part of the id.
    assert!(rows[11].id == "80c63211a8");
    assert!(rows[12].id == "80c63211a8#2");
    assert!(rows[11] != rows[12]);
    // Sign convention: income and expense keep their stored sign.
    assert!(rows[2].is_income());
    assert!(!rows[0].is_income());
}

#[test]
fn corrupt_line_in_the_middle_yields_a_typed_error_with_its_line_number() {
    // Deliberately corrupt the file after line 15 (CAFETARIA): an
    // unterminated quote on the NEW line 16. The 22 real rows sit on lines
    // 5-26, so the rows before and after the corrupt line must still parse.
    let broken = FIXTURE_TEXT.replace(
        "58ed99aa4c;2024-03-02;-1590;EUR;CAFETARIA;CAFETARIA;bank_b2.pdf\n",
        "58ed99aa4c;2024-03-02;-1590;EUR;CAFETARIA;CAFETARIA;bank_b2.pdf\n80c63211a8;\"never closed\n",
    );
    let (rows, errors) = read(&broken);
    assert!(rows.len() == 22);
    assert!(errors.len() == 1);
    match errors[0] {
        ReadError::MalformedLine(16, CodecError::UnterminatedQuote) => { }
        _ => { assert!(false); }
    }
    // The rows after the corrupt line are not lost: line 18 is still there.
    assert!(rows[13].id == "5b35bef1fd");
    assert!(rows[13].amount == -6789);
}

#[test]
fn comment_and_blank_lines_are_skipped() {
    let text = "# Phin transactions -- append-only facts, never edited in place.\n# columns: id;date;amount;currency;description;counterparty;source\n\n   \t  \n39945b2fed;2024-01-03;-3999;EUR;REWE MARKT 12;REWE MARKET;bank_a.pdf\n\n# a note written by hand between the rows\n   \n80c63211a8#2;2024-03-05;-1500;EUR;REWE MARKT 12;REWE MARKET;bank_b2.pdf\n";
    let (rows, errors) = read(text);
    assert!(rows.len() == 2);
    assert!(errors.len() == 0);
    assert!(rows[0].id == "39945b2fed");
    assert!(rows[1].id == "80c63211a8#2");
    // A comment line never reaches the codec as a row, and a blank line never
    // becomes a WrongColumnCount error.
    let only_noise = "# columns: id;date;amount;currency;description;counterparty;source\n\n \n\t\n";
    let (no_rows, no_errors) = read(only_noise);
    assert!(no_rows.len() == 0);
    assert!(no_errors.len() == 0);
}

#[test]
fn append_skips_candidates_whose_id_is_already_on_file() {
    // Same content as fixture line 5 -> same base id -> skipped, text
    // unchanged byte for byte. The candidate's own `id` field is irrelevant:
    // the id is recomputed from the stored fields.
    let duplicate = crate::domain::transaction::from_stored("whatever-the-importer-set", "2024-01-03", -3999, "EUR", "REWE MARKT 12", "REWE MARKET", "bank_a.pdf").unwrap();
    assert!(transaction_base_id(&duplicate) == "39945b2fed");
    let (text, outcome) = append(FIXTURE_TEXT, &duplicate).unwrap();
    assert!(outcome == AppendOutcome::Skipped);
    assert!(text == FIXTURE_TEXT);
    // A suffixed occurrence on file also counts as known: the fixture holds
    // `80c63211a8` AND `80c63211a8#2`, so a fresh candidate with that base
    // is skipped, not appended and not suffixed `#3`.
    let rewe_twin = crate::domain::transaction::from_stored("", "2024-03-05", -1500, "EUR", "REWE MARKT 12", "REWE MARKET", "bank_b2.pdf").unwrap();
    assert!(transaction_base_id(&rewe_twin) == "80c63211a8");
    let (same_text, same_outcome) = append(FIXTURE_TEXT, &rewe_twin).unwrap();
    assert!(same_outcome == AppendOutcome::Skipped);
    assert!(same_text == FIXTURE_TEXT);
    // Path-independent source identity skips too: the same statement record
    // reached through another path hashes to the same base id.
    let via_path = crate::domain::transaction::from_stored("", "2024-01-03", -3999, "EUR", "REWE MARKT 12", "REWE MARKET", "/srv/statements/bank_a.pdf").unwrap();
    let (still_same, path_outcome) = append(FIXTURE_TEXT, &via_path).unwrap();
    assert!(path_outcome == AppendOutcome::Skipped);
    assert!(still_same == FIXTURE_TEXT);
    // Appending twice is idempotent: the second append sees the first line.
    let fresh = crate::domain::transaction::from_stored("", "2024-05-01", -199, "EUR", "NEW SHOP", "SHOP", "new.pdf").unwrap();
    let (once, first_outcome) = append(FIXTURE_TEXT, &fresh).unwrap();
    assert!(first_outcome == AppendOutcome::Appended);
    let (twice, second_outcome) = append(&once, &fresh).unwrap();
    assert!(second_outcome == AppendOutcome::Skipped);
    assert!(twice == once);
}

#[test]
fn append_adds_exactly_one_line_and_leaves_the_old_text_untouched() {
    // A candidate with fresh content, description carrying the codec's
    // quoting case (an embedded `;`).
    let fresh = crate::domain::transaction::from_stored("", "2024-05-01", -199, "EUR", "NEW SHOP; DELICATES", "SHOP", "new.pdf").unwrap();
    let base = transaction_base_id(&fresh);
    assert!(base.len() == 10);
    let (new_text, outcome) = append(FIXTURE_TEXT, &fresh).unwrap();
    assert!(outcome == AppendOutcome::Appended);
    // The old text is an exact byte prefix: every old line, including the
    // four preamble comments, stays byte-identical.
    assert!(new_text.starts_with(FIXTURE_TEXT));
    // The only addition is the rendered line plus its LF.
    let mut fields = Vec::<String>::new();
    fields.push(base.to_string());
    fields.push("2024-05-01".to_string());
    fields.push("-199".to_string());
    fields.push("EUR".to_string());
    fields.push("NEW SHOP; DELICATES".to_string());
    fields.push("SHOP".to_string());
    fields.push("new.pdf".to_string());
    let rendered = render_record(&fields).unwrap();
    let mut expected = FIXTURE_TEXT.to_string();
    expected.push_str(&rendered);
    expected.push_str("\n");
    assert!(new_text == expected);
    // The appended line reads back as exactly the candidate's fact: the
    // amount stays a minor-unit count, and the id is the recomputed base,
    // not the candidate's own `id` field (which was empty here).
    let (rows, errors) = read(&new_text);
    assert!(errors.len() == 0);
    assert!(rows.len() == 23);
    assert!(rows[22].id == base);
    assert!(rows[22].amount == -199);
    assert!(rows[22].description == "NEW SHOP; DELICATES");
    let reread = from_stored(&base, "2024-05-01", -199, "EUR", "NEW SHOP; DELICATES", "SHOP", "new.pdf").unwrap();
    assert!(rows[22] == reread);
    // The 22 old rows are unchanged, in the same order.
    let (old_rows, old_errors) = read(FIXTURE_TEXT);
    assert!(old_errors.len() == 0);
    assert!(old_rows.len() == 22);
    assert!(rows[0] == old_rows[0]);
    assert!(rows[21] == old_rows[21]);
}

#[test]
fn append_to_empty_text_and_to_text_without_a_trailing_line_ending() {
    let fresh = crate::domain::transaction::from_stored("", "2024-05-01", -199, "EUR", "NEW SHOP", "SHOP", "new.pdf").unwrap();
    // An empty file gets exactly one line, LF-terminated, nothing before it.
    let (from_empty, outcome) = append("", &fresh).unwrap();
    assert!(outcome == AppendOutcome::Appended);
    assert!(from_empty.ends_with("\n"));
    let (rows, errors) = read(&from_empty);
    assert!(errors.len() == 0);
    assert!(rows.len() == 1);
    assert!(rows[0].id == transaction_base_id(&fresh));
    // A file whose last line is not terminated: the old bytes all survive,
    // the unterminated last line gets its missing LF, and the new line
    // follows — the existing text stays a byte-for-byte prefix.
    let unterminated = "# columns: id;date;amount;currency;description;counterparty;source\n39945b2fed;2024-01-03;-3999;EUR;REWE MARKT 12;REWE MARKET;bank_a.pdf";
    let (appended, append_outcome) = append(unterminated, &fresh).unwrap();
    assert!(append_outcome == AppendOutcome::Appended);
    assert!(appended.starts_with(unterminated));
    let (rows, errors) = read(&appended);
    assert!(errors.len() == 0);
    assert!(rows.len() == 2);
    assert!(rows[0].id == "39945b2fed");
    // The id check works on the unterminated file too: the existing row is
    // known, so its candidate is skipped and the text is returned as-is,
    // WITHOUT the missing LF being added.
    let known = crate::domain::transaction::from_stored("", "2024-01-03", -3999, "EUR", "REWE MARKT 12", "REWE MARKET", "bank_a.pdf").unwrap();
    let (skipped, skip_outcome) = append(unterminated, &known).unwrap();
    assert!(skip_outcome == AppendOutcome::Skipped);
    assert!(skipped == unterminated);
}

#[test]
fn append_rejects_a_field_with_a_line_break_via_the_codec_error() {
    // A line break inside a field would split the record over two lines,
    // so the codec's own typed error comes back unchanged and no text is
    // produced — the candidate is never written half-way.
    let broken = crate::domain::transaction::from_stored("", "2024-05-01", -1, "EUR", "TWO\nLINES", "SHOP", "new.pdf").unwrap();
    match append(FIXTURE_TEXT, &broken) {
        Err(reason) => { assert!(reason == CodecError::NewlineInField); }
        Ok(_) => { assert!(false); }
    }
    let carriage = crate::domain::transaction::from_stored("", "2024-05-01", -1, "EUR", "COUNTER\rPARTY", "SHOP", "new.pdf").unwrap();
    match append(FIXTURE_TEXT, &carriage) {
        Err(reason) => { assert!(reason == CodecError::NewlineInField); }
        Ok(_) => { assert!(false); }
    }
}
