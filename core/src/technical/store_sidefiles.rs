//! The store reader/writer of `annotations.csv`, `overrides.csv` and
//! `categories.csv` — plan item 2.4. Read/append for each, text in / text out.
//!
//! The contract is `doc/architecture/data-files.md`: `annotations.csv`
//! (line 75) columns `id;ref;text;items;source;confidence` with `items`
//! `|`-separated, and `overrides.csv` (line 95) columns `id;category;reason`.
//! Rows are built through the domain constructors —
//! [`crate::domain::annotation::from_parts`] (which splits/trims `items` and
//! clamps `confidence`) and [`crate::domain::classify::Override`], the type
//! [`crate::domain::classify::classify`] consumes.
//!
//! Style and rules follow the sibling [`crate::technical::store_transactions`]:
//! text in, text out, NO I/O; `#` and blank lines are skipped; a corrupt line
//! never aborts the read but becomes a typed [`ReadError`] carrying its 1-BASED
//! line number; append leaves the existing bytes untouched and skips a record
//! already known. Orphan reporting is a typed return value (decision 10):
//! [`read_overrides`] takes the known transaction ids and returns the
//! overrides whose `id` matches none of them — `doc/architecture/data-files.md:99`
//! ("An override for an unknown id is stored and reported as an orphan on read").

use super::csv::parse_record;
use super::csv::render_record;
use super::csv::CodecError;
use super::csv::Line;
use super::csv::write_document;

use super::id::base_id_of_id;
use super::id::matches_base;
use crate::domain::annotation::from_parts;
use crate::domain::annotation::items_to_text;
use crate::domain::annotation::Annotation;
use crate::domain::classify::Override;

/// Columns of `annotations.csv` (`id;ref;text;items;source;confidence`).
const ANNOTATION_COLUMN_COUNT: usize = 6;

/// Columns of `overrides.csv` (`id;category;reason`).
const OVERRIDE_COLUMN_COUNT: usize = 3;

/// Columns of `categories.csv` (`parent;name`).
const CATEGORY_COLUMN_COUNT: usize = 2;

/// Why one line of `annotations.csv` or `overrides.csv` could not become a row.
/// Every variant carries the 1-BASED line number of the offending line.
#[derive(Debug, Eq, PartialEq)]
pub enum ReadError {
    /// [`super::csv::parse_record`] rejected the line; the reason is the codec's own.
    MalformedLine(usize, CodecError),
    /// The record decoded, but not into the expected column count; the second
    /// number is what the line actually held.
    WrongColumnCount(usize, usize),
    /// The `confidence` column of `annotations.csv` was not a decimal `f64`
    /// (the contract says `0…1`; out-of-range values are NOT an error —
    /// [`crate::domain::annotation::clamp_confidence`] clamps them).
    InvalidConfidenceNumber(usize),
}

/// Whether an [`append_annotation`] / [`append_override`] wrote a line.
#[derive(Debug, Eq, PartialEq)]
pub enum AppendOutcome {
    /// The record was not yet in the file: one line was appended.
    Appended,
    /// The record is already known: the text is returned unchanged, byte for byte.
    Skipped,
}

/// The line without one trailing `\r`.
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

/// Read the whole text of an `annotations.csv` file into rows and typed errors.
///
/// `#`/blank lines are skipped, every other line is decoded by
/// [`super::csv::parse_record`] and must hold [`ANNOTATION_COLUMN_COUNT`]
/// fields, handed to [`crate::domain::annotation::from_parts`] in contract
/// column order `id;ref;text;items;source;confidence`. The `confidence` field
/// is parsed to `f64` first (a non-decimal becomes
/// [`ReadError.InvalidConfidenceNumber`]); range clamping is the domain's
/// [`crate::domain::annotation::clamp_confidence`]. Reading never aborts on
/// corruption: rows that parsed and one [`ReadError`] per bad line are both
/// returned.
pub fn read_annotations(text: &str) -> (Vec::<Annotation>, Vec::<ReadError>) {
    let mut rows = Vec::<Annotation>::new();
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
                if fields.len() != ANNOTATION_COLUMN_COUNT {
                    errors.push(ReadError::WrongColumnCount(number, fields.len()));
                    continue;
                }
                match fields[5].parse::<f64>() {
                    Err(_) => { errors.push(ReadError::InvalidConfidenceNumber(number)); }
                    Ok(confidence) => {
                        rows.push(from_parts(fields[0].to_string(), fields[1].to_string(), fields[2].to_string(), fields[3].to_string(), fields[4].to_string(), confidence));
                    }
                }
            }
        }
    }
    return (rows, errors);
}

/// Read the whole text of an `overrides.csv` file into rows, orphans and typed
/// errors.
///
/// `#`/blank lines are skipped, every other line must decode into
/// [`OVERRIDE_COLUMN_COUNT`] fields `id;category;reason`, built into
/// [`crate::domain::classify::Override`]. A corrupt line becomes a typed
/// [`ReadError`] with its 1-based line number and never aborts the read.
///
/// ORPHANS (decision 10, typed return value): an override whose `id` equals
/// none of `known_ids` (the ids of the transactions on file, as read by
/// [`crate::technical::store_transactions`]) is ALSO returned in the second
/// component — `doc/architecture/data-files.md:99` "An override for an unknown
/// id is stored and reported as an orphan on read". The match is EXACT id
/// equality, mirroring the `.rev()` scan in [`crate::domain::classify::classify`]
/// (`entry.id == transaction.id`), so an override naming a base id does not
/// match a suffixed stored id `base#N` there either. Orphans are still
/// returned as rows: they are stored, and a later transaction may land on them.
pub fn read_overrides(text: &str, known_ids: &Vec::<String>) -> (Vec::<Override>, Vec::<Override>, Vec::<ReadError>) {
    let mut rows = Vec::<Override>::new();
    let mut orphans = Vec::<Override>::new();
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
                if fields.len() != OVERRIDE_COLUMN_COUNT {
                    errors.push(ReadError::WrongColumnCount(number, fields.len()));
                    continue;
                }
                let entry = Override { id: fields[0].to_string(), category: fields[1].to_string(), reason: fields[2].to_string() };
                let known = known_ids.iter()
                    .any(|known_id| *known_id == entry.id);
                if !known {
                    orphans.push(Override { id: entry.id.to_string(), category: entry.category.to_string(), reason: entry.reason.to_string() });
                }
                rows.push(entry);
            }
        }
    }
    return (rows, orphans, errors);
}

/// Append one annotation line to the text of an `annotations.csv` file.
///
/// Text in, text out — no I/O. The existing text is never touched: the line is
/// rendered by [`super::csv::render_record`] over the contract column order
/// `id;ref;text;items;source;confidence` (items joined back with `|` by
/// [`crate::domain::annotation::items_to_text`], confidence rendered from its
/// clamped `f64`) and appended after the existing text, with one LF added
/// first if the file did not end with a line ending.
///
/// The candidate is SKIPPED when its id is already known: the candidate's
/// base ([`super::id::base_id_of_id`]) matches any stored annotation id
/// ([`super::id::matches_base`] — the bare base or one of its `#N` occurrence
/// forms), which is the annotation analogue of the idempotence rule of
/// `doc/architecture/data-files.md`. A field the codec cannot render — a line
/// break inside `text` — is the codec's own [`super::csv::CodecError.NewlineInField`].
pub fn append_annotation(text: &str, annotation: &Annotation) -> Result::<(String, AppendOutcome), CodecError> {
    let base = base_id_of_id(&annotation.id);
    let (rows, _) = read_annotations(text);
    for row in rows {
        if matches_base(&row.id, &base) {
            return Ok((text.to_string(), AppendOutcome::Skipped));
        }
    }
    let mut fields = Vec::<String>::new();
    fields.push(annotation.id.to_string());
    fields.push(annotation.r#ref.to_string());
    fields.push(annotation.text.to_string());
    fields.push(items_to_text(annotation.items.clone()));
    fields.push(annotation.source.to_string());
    fields.push(annotation.confidence.to_string());
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

/// Append one override line to the text of an `overrides.csv` file.
///
/// Text in, text out — no I/O; the existing bytes stay an untouched prefix,
/// rendered as above over `id;category;reason`.
///
/// The candidate is SKIPPED only when the EXACT record is already known
/// (an identical [`crate::domain::classify::Override`], compared with `==`):
/// `overrides.csv` is append-only and a superseded entry stays in the file,
/// so a NEWER override for an id already on file is NOT skipped — both lines
/// survive and the LAST one wins on read, matching the `.rev()` scan in
/// [`crate::domain::classify::classify`] (line 86).
pub fn append_override(text: &str, entry: &Override) -> Result::<(String, AppendOutcome), CodecError> {
    let (rows, _, _) = read_overrides(text, &Vec::<String>::new());
    for row in rows {
        if row == *entry {
            return Ok((text.to_string(), AppendOutcome::Skipped));
        }
    }
    let mut fields = Vec::<String>::new();
    fields.push(entry.id.to_string());
    fields.push(entry.category.to_string());
    fields.push(entry.reason.to_string());
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

// ============================================================================
// `categories.csv` (columns `parent;name`, doc/architecture/data-files.md:105)
// ============================================================================

/// One row of `categories.csv`: node `name` under `parent`; an empty `parent`
/// marks a root node (`doc/architecture/data-files.md:105`). The tree builder
/// in [`crate::domain::category`] requires node names to be unique across the
/// tree, so the exact `(parent, name)` pair identifies a row.
#[derive(Debug, Eq, PartialEq)]
pub struct Category {
    parent: String,
    name: String,
}

/// Read the whole text of a `categories.csv` file into document lines, rows
/// and typed errors.
///
/// Unlike the annotation/override readers this one ALSO returns the
/// [`super::csv::Line`] document: `categories.csv` is edited by hand and
/// "order in the file is the display order and is preserved"
/// (`doc/architecture/data-files.md:105`), so the reader keeps comment lines
/// verbatim IN POSITION and every row in file order — [`super::csv::write_document`]
/// over the returned lines reproduces the file byte for byte (see
/// [`write_categories`]). Blank lines are the one shape NOT kept: the codec's
/// document has no blank-line variant, matching the guarantee documented on
/// [`super::csv::write_document`] for the `testdata/` fixture shape.
///
/// A corrupt line never aborts the read: it becomes a typed [`ReadError`]
/// carrying its 1-BASED line number ([`ReadError.WrongColumnCount`] when the
/// line did not hold exactly [`CATEGORY_COLUMN_COUNT`] fields) and the lines
/// before and after it survive in both returned lists.
pub fn read_categories(text: &str) -> (Vec::<Line>, Vec::<Category>, Vec::<ReadError>) {
    let mut lines = Vec::<Line>::new();
    let mut rows = Vec::<Category>::new();
    let mut errors = Vec::<ReadError>::new();
    let mut number = 0;
    for raw_line in text.split("\n") {
        number = number + 1;
        let line = drop_carriage_return(&raw_line);
        if is_blank(&line) {
            continue;
        }
        if line.starts_with("#") {
            lines.push(Line::Comment(line.to_string()));
            continue;
        }
        match parse_record(&line) {
            Err(reason) => { errors.push(ReadError::MalformedLine(number, reason)); }
            Ok(fields) => {
                if fields.len() != CATEGORY_COLUMN_COUNT {
                    errors.push(ReadError::WrongColumnCount(number, fields.len()));
                    continue;
                }
                let entry = Category { parent: fields[0].to_string(), name: fields[1].to_string() };
                lines.push(Line::Record(fields));
                rows.push(entry);
            }
        }
    }
    return (lines, rows, errors);
}

/// Render document lines read by [`read_categories`] back into `categories.csv`
/// text: comments verbatim, records encoded by [`super::csv::render_record`],
/// every line LF-terminated. Delegates to [`super::csv::write_document`]; a
/// round trip over text in the fixture shape is byte-identical.
pub fn write_categories(lines: &Vec::<Line>) -> Result::<String, CodecError> {
    return write_document(lines);
}

/// Append one category line to the text of a `categories.csv` file.
///
/// Text in, text out — no I/O; the existing text stays an untouched prefix,
/// rendered as above over `parent;name`.
///
/// The candidate is SKIPPED only when the EXACT row is already known (an
/// identical [`Category`], compared with `==`): the file is append-only and
/// hand-edited, so the reader does not fold rows and a duplicate would only
/// be reported by [`crate::domain::category`]'s [`BuildError.DuplicateName`].
pub fn append_category(text: &str, entry: &Category) -> Result::<(String, AppendOutcome), CodecError> {
    let (_, rows, _) = read_categories(text);
    for row in rows {
        if row == *entry {
            return Ok((text.to_string(), AppendOutcome::Skipped));
        }
    }
    let mut fields = Vec::<String>::new();
    fields.push(entry.parent.to_string());
    fields.push(entry.name.to_string());
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

#[cfg(test)]
const ANNOTATION_FIXTURE: &str = "# annotations -- information from other sources (mail).\n# columns: id;ref;text;items;source;confidence\nann00000001;39945b2fed;REWE delivery note;1x groceries|1x wine;mail_rewe.mbox;0.9\nann00000002;;unlinked PayPal note;;mail_paypal.mbox;0.4\nann00000003;140ef279a4;\"taxi, night; quoted\";1x ride;mail_paypal.mbox;1.0\n";

#[cfg(test)]
fn make_annotation(id: &str, ref_id: &str, text: &str, confidence: f64) -> Annotation {
    return from_parts(id.to_string(), ref_id.to_string(), text.to_string(), "1x book|1x ebook".to_string(), "mail_x.mbox".to_string(), confidence);
}

#[cfg(test)]
fn make_override(id: &str, category: &str, reason: &str) -> Override {
    return Override { id: id.to_string(), category: category.to_string(), reason: reason.to_string() };
}

#[test]
fn annotation_fixture_parses_without_errors() {
    let (rows, errors) = read_annotations(ANNOTATION_FIXTURE);
    assert!(rows.len() == 3);
    assert!(errors.len() == 0);
    // items split on `|`; an empty items column yields an EMPTY list.
    assert!(rows[0].items.len() == 2);
    assert!(rows[0].items[0] == "1x groceries".to_string());
    assert!(rows[1].items.len() == 0);
    assert!(rows[1].r#ref == "".to_string());
    // The quoted field with an embedded `;` decodes.
    assert!(rows[2].text == "taxi, night; quoted".to_string());
    assert!(rows[2].confidence == 1.0);
    assert!(rows[0].confidence == 0.9);
}

#[test]
fn corrupt_annotation_line_yields_a_typed_error_with_its_line_number_and_the_rest_survives() {
    // Unterminated quote on line 4, non-decimal confidence on line 6; the
    // rows before and after both lines still parse.
    let broken = ANNOTATION_FIXTURE.replace(
        "ann00000002;;unlinked PayPal note;;mail_paypal.mbox;0.4\n",
        "ann00000002;\"never closed\nann00000004;39945b2fed;bad confidence;1x book;mail_x.mbox;high\n",
    );
    let (rows, errors) = read_annotations(&broken);
    assert!(rows.len() == 2);
    assert!(errors.len() == 2);
    match errors[0] {
        ReadError::MalformedLine(4, CodecError::UnterminatedQuote) => { }
        _ => { assert!(false); }
    }
    match errors[1] {
        ReadError::InvalidConfidenceNumber(5) => { }
        _ => { assert!(false); }
    }
    // The row after both bad lines survives.
    assert!(rows[1].id == "ann00000003".to_string());
    assert!(rows[1].confidence == 1.0);
}

#[test]
fn override_for_an_unknown_id_is_stored_and_reported_as_an_orphan_on_read() {
    let base = "# columns: id;category;reason\n39945b2fed;food:supermarket;user chose this\n";
    let orphan = make_override("ffffffffff", "leisure:games", "guessed");
    // Stored: the append succeeds and the old bytes stay an untouched prefix.
    let (text, outcome) = append_override(base, &orphan).unwrap();
    assert!(outcome == AppendOutcome::Appended);
    assert!(text.starts_with(base));
    // Reported on read: known ids are the two fixture transactions;
    // `ffffffffff` matches neither and comes back as an orphan.
    let mut known_ids = Vec::<String>::new();
    known_ids.push("39945b2fed".to_string());
    known_ids.push("140ef279a4".to_string());
    let (rows, orphans, errors) = read_overrides(&text, &known_ids);
    assert!(errors.len() == 0);
    assert!(rows.len() == 2);
    assert!(orphans.len() == 1);
    assert!(orphans[0] == orphan);
    // The orphan is stored, not dropped: it is also a row.
    assert!(rows[1] == orphan);
    // An override naming a known id is NOT an orphan.
    assert!(rows[0].id == "39945b2fed".to_string());
}

#[test]
fn duplicate_override_for_the_same_id_keeps_both_lines_and_the_last_wins_on_read() {
    let base = "# columns: id;category;reason\n39945b2fed;food:supermarket;first thought\n";
    let second = make_override("39945b2fed", "leisure:games", "corrected later");
    // NOT skipped: the record differs, so both lines survive in the file.
    let (text, outcome) = append_override(base, &second).unwrap();
    assert!(outcome == AppendOutcome::Appended);
    assert!(text.starts_with(base));
    let (rows, _orphans, errors) = read_overrides(&text, &Vec::<String>::new());
    assert!(errors.len() == 0);
    assert!(rows.len() == 2);
    assert!(rows[0].category == "food:supermarket".to_string());
    assert!(rows[1].category == "leisure:games".to_string());
    // The LAST line wins, exactly as the `.rev()` scan in
    // [`crate::domain::classify::classify`] (line 86) sees it.
    let row = crate::domain::transaction::from_stored("39945b2fed", "2024-01-03", -3999, "EUR", "REWE MARKT 12", "REWE MARKET", "bank_a.pdf").unwrap();
    let no_rules = Vec::<crate::domain::rule::Rule>::new();
    let classification = crate::domain::classify::classify(&row, &no_rules, &rows);
    assert!(classification == crate::domain::classify::Classification::Category("leisure:games".to_string()));
    // Appending the EXACT same record again is idempotent: text unchanged.
    let (same_text, same_outcome) = append_override(&text, &second).unwrap();
    assert!(same_outcome == AppendOutcome::Skipped);
    assert!(same_text == text);
}

#[test]
fn corrupt_override_line_yields_a_typed_error_and_the_rest_still_parses() {
    let broken = "# columns: id;category;reason\n39945b2fed;food:supermarket;ok\nbad;line;with;too;many;columns\n140ef279a4;finance:income;\"unterminated\n";
    let (rows, orphans, errors) = read_overrides(broken, &Vec::<String>::new());
    assert!(rows.len() == 1);
    assert!(orphans.len() == 1);
    assert!(errors.len() == 2);
    match errors[0] {
        ReadError::WrongColumnCount(3, 6) => { }
        _ => { assert!(false); }
    }
    match errors[1] {
        ReadError::MalformedLine(4, CodecError::UnterminatedQuote) => { }
        _ => { assert!(false); }
    }
    assert!(rows[0].id == "39945b2fed".to_string());
}

#[test]
fn append_skips_an_annotation_whose_id_is_already_on_file() {
    // Same base id, suffixed form on file also counts as known.
    let candidate = make_annotation("ann00000001", "deadbeef00", "fresh text", 0.5);
    let (text, outcome) = append_annotation(ANNOTATION_FIXTURE, &candidate).unwrap();
    assert!(outcome == AppendOutcome::Skipped);
    assert!(text == ANNOTATION_FIXTURE);
    let suffixed = make_annotation("ann00000003#2", "39945b2fed", "other", 0.5);
    let (same_text, same_outcome) = append_annotation(ANNOTATION_FIXTURE, &suffixed).unwrap();
    assert!(same_outcome == AppendOutcome::Skipped);
    assert!(same_text == ANNOTATION_FIXTURE);
    // A fresh id appends exactly one line and leaves the old text a byte prefix.
    let fresh = make_annotation("ann00000009", "39945b2fed", "NEW note", 0.75);
    let (appended, append_outcome) = append_annotation(ANNOTATION_FIXTURE, &fresh).unwrap();
    assert!(append_outcome == AppendOutcome::Appended);
    assert!(appended.starts_with(ANNOTATION_FIXTURE));
    let (rows, errors) = read_annotations(&appended);
    assert!(errors.len() == 0);
    assert!(rows.len() == 4);
    assert!(rows[3].id == "ann00000009".to_string());
    assert!(rows[3].confidence == 0.75);
    assert!(rows[3].items.len() == 2);
    assert!(rows[3].items[0] == "1x book".to_string());
    // Appending twice is idempotent.
    let (twice, second_outcome) = append_annotation(&appended, &fresh).unwrap();
    assert!(second_outcome == AppendOutcome::Skipped);
    assert!(twice == appended);
}

#[cfg(test)]
const CATEGORY_FIXTURE: &str = "# Phin categories -- tree of parent:child names, order is preserved.\n# columns: parent;name\n;food\n;finance\n;leisure\nfood;supermarket\nfood;restaurant\nfinance;income\nfinance;expense\nexpense;transport\nexpense;utilities\nleisure;books\nleisure;games\n";

#[cfg(test)]
fn make_category(parent: &str, name: &str) -> Category {
    return Category { parent: parent.to_string(), name: name.to_string() };
}

#[test]
fn categories_fixture_round_trips_byte_identical_with_comments_and_order() {
    // The verbatim `testdata/categories.csv`: 11 rows and 2 comment lines.
    let (lines, rows, errors) = read_categories(CATEGORY_FIXTURE);
    assert!(errors.len() == 0);
    assert!(rows.len() == 11);
    assert!(super::csv::comments_of(&lines).len() == 2);
    // File order is display order and is preserved: first root `food`,
    // `finance;expense` (row index 6) after `finance` itself (index 1).
    assert!(rows[0].parent == "".to_string() && rows[0].name == "food".to_string());
    assert!(rows[1].parent == "".to_string() && rows[1].name == "finance".to_string());
    assert!(rows[6].parent == "finance".to_string() && rows[6].name == "expense".to_string());
    // Round trip byte-identical, comments included.
    match write_categories(&lines) {
        Err(_) => { assert!(false, "fixture must render"); }
        Ok(text) => { assert!(text == CATEGORY_FIXTURE); }
    }
}

#[test]
fn append_category_keeps_the_old_bytes_untouched_and_skips_an_existing_row() {
    let fresh = make_category("expense", "cycling");
    let (text, outcome) = append_category(CATEGORY_FIXTURE, &fresh).unwrap();
    assert!(outcome == AppendOutcome::Appended);
    // The old file survives as an untouched byte prefix, comment included.
    assert!(text.starts_with(CATEGORY_FIXTURE));
    let (_lines, rows, errors) = read_categories(&text);
    assert!(errors.len() == 0);
    assert!(rows.len() == 12);
    assert!(rows[11] == fresh);
    // The new line renders in contract column order `parent;name`.
    assert!(text.ends_with("expense;cycling\n"));
    // The exact row already on file is SKIPPED: text unchanged byte for byte.
    let (same_text, same_outcome) = append_category(&text, &fresh).unwrap();
    assert!(same_outcome == AppendOutcome::Skipped);
    assert!(same_text == text);
    let existing = make_category("food", "restaurant");
    let (untouched_text, untouched_outcome) = append_category(CATEGORY_FIXTURE, &existing).unwrap();
    assert!(untouched_outcome == AppendOutcome::Skipped);
    assert!(untouched_text == CATEGORY_FIXTURE);
    // A root row (empty parent) appends as the leading empty column.
    let root = make_category("", "travel");
    let (with_root, root_outcome) = append_category(CATEGORY_FIXTURE, &root).unwrap();
    assert!(root_outcome == AppendOutcome::Appended);
    assert!(with_root.ends_with(";travel\n"));
}

#[test]
fn corrupt_category_line_yields_a_typed_error_with_its_line_number_and_the_rest_survives() {
    // Too many columns on line 3, unterminated quote on line 4; the rows
    // before and after both lines still parse, and the comment survives.
    let broken = "# columns: parent;name\n;food\nfood;supermarket;extra;columns\nfinance;\"unterminated\n;leisure\n";
    let (lines, rows, errors) = read_categories(broken);
    assert!(rows.len() == 2);
    assert!(errors.len() == 2);
    match errors[0] {
        ReadError::WrongColumnCount(3, 4) => { }
        _ => { assert!(false); }
    }
    match errors[1] {
        ReadError::MalformedLine(4, CodecError::UnterminatedQuote) => { }
        _ => { assert!(false); }
    }
    assert!(rows[0].parent == "".to_string() && rows[0].name == "food".to_string());
    assert!(rows[1].parent == "".to_string() && rows[1].name == "leisure".to_string());
    // The comment line survives in the document, so a round trip keeps it.
    assert!(super::csv::comments_of(&lines).len() == 1);
}

#[test]
fn append_rejects_a_field_with_a_line_break_via_the_codec_error() {
    let broken = make_annotation("ann00000010", "", "TWO\nLINES", 0.5);
    match append_annotation(ANNOTATION_FIXTURE, &broken) {
        Err(reason) => { assert!(reason == CodecError::NewlineInField); }
        Ok(_) => { assert!(false); }
    }
    let broken_override = make_override("39945b2fed", "food:super\nmarket", "reason");
    match append_override("# columns: id;category;reason\n", &broken_override) {
        Err(reason) => { assert!(reason == CodecError::NewlineInField); }
        Ok(_) => { assert!(false); }
    }
}
