//! The record codec every `.csv` file of the data directory shares.
//!
//! This is the contract of `doc/architecture/data-files.md`, section "CSV
//! codec (all `.csv` files)" (lines 14-27), implemented verbatim:
//!
//! | Rule | Implementation |
//! |------|----------------|
//! | Separator | `;` between the fields of one record. |
//! | Comments | A line whose first character is `#` is a comment and is kept verbatim; empty and whitespace-only lines are ignored on read. |
//! | Quoting | A field is wrapped in `"` when it contains `;` or `"`, or starts/ends with a space; a `"` inside a quoted field is doubled (`""`). |
//! | Newlines | A field must never contain a line break; writing one would split the record over two lines, which is the hard error [`CodecError.NewlineInField`]. |
//! | Line endings | CRLF is tolerated on read (one trailing `\r` is dropped per line), LF is written. |
//! | Empty columns | Optional columns may be empty, i.e. nothing between two separators. |
//! | Header | There is no header line; the column order is fixed by the spec and documented in a `# columns:` comment at the top of the file. |
//!
//! There is no file system access here: the codec turns text in, text out.
//! Opening the files, appending and the atomic rename of plan item 2.5 belong
//! to the store modules (`store_transactions.rs`, `store_sidefiles.rs`, …),
//! which call [`read_document`] / [`write_document`] or, when they only touch
//! one line, [`parse_record`] / [`render_record`].
//!
//! The column COUNT is not checked here either — the codec is generic and the
//! per-file column sets of `doc/architecture/data-files.md:45-110` are applied
//! by the store modules that build the domain structs.

/// One line of a `.csv` file, as read or as written: either a comment line
/// kept exactly as it was read (the `# columns:` preamble and any free-form
/// note), or a decoded record. Keeping comments in the list, in position, is
/// what makes a read → write round trip leave a hand-edited file unchanged:
/// `categories.csv` and the rules files are edited by hand and must not be
/// reordered behind the user's back.
#[derive(Debug, Eq, PartialEq)]
pub enum Line {
    /// A comment line, stored verbatim without its line ending.
    Comment(String),
    /// A record, stored as its decoded fields.
    Record(Vec::<String>),
}

/// What can go wrong while reading or writing one record or one document.
#[derive(Debug, Eq, PartialEq)]
pub enum CodecError {
    /// A field handed to the writer contained `\n` or `\r`. A record must fit
    /// on one line, so a line break inside a field is a hard error and not
    /// something the codec could escape (`doc/architecture/data-files.md`,
    /// "Newlines").
    NewlineInField,
    /// A quoted field was opened but never closed before the end of the line.
    UnterminatedQuote,
    /// A `"` appeared where the codec allows none: inside an unquoted field,
    /// or directly after a closing quote and before the next separator.
    StrayQuote,
    /// The line read was not a record: it was empty or started with `#`.
    /// [`read_document`] never offers such a line to [`parse_record`]; a caller
    /// that does has mistaken a comment (or a blank line) for data.
    NotARecord,
    /// The record handed to the writer had no field at all. A record always has
    /// at least one column, so an empty field list would render as a blank
    /// line, which read ignores.
    EmptyRecord,
}

/// Decode one record line into its fields.
///
/// Fields are split on every unquoted `;`; a field wrapped in `"` may contain
/// `;`, a doubled `""` stands for one `"`, and leading/trailing spaces inside
/// the quotes are kept. Empty fields between separators are kept as empty
/// strings, which is how the optional columns of `annotations.csv` (the `ref`
/// of an orphan) or `categories.csv` (the `parent` of a root node) are read.
///
/// Surrounding whitespace is NOT trimmed: the codec is lossless, so `" x "` and
/// `x` are different fields. Normalizing field text is the caller's job, done
/// through the `domain` parsers.
///
/// A line break inside the line (a record that was split over two lines) and a
/// line that is a comment or empty are typed errors, never a panic.
pub fn parse_record(line: &str) -> Result::<Vec::<String>, CodecError> {
    if line.len() == 0 {
        return Err(CodecError::NotARecord);
    }
    if line.starts_with("#") {
        return Err(CodecError::NotARecord);
    }
    if line.contains("\n") || line.contains("\r") {
        return Err(CodecError::NewlineInField);
    }

    let characters: Vec::<char> = line.chars().collect();
    let mut fields = Vec::<String>::new();
    let mut current = String::new();
    let mut in_quotes = false;
    let mut quote_closed = false;
    let mut index = 0;
    while index < characters.len() {
        let character = characters[index];
        if character == ';' && !in_quotes {
            fields.push(current);
            current = String::new();
            in_quotes = false;
            quote_closed = false;
            index = index + 1;
            continue;
        }
        if character == '"' && !in_quotes && current.len() == 0 && !quote_closed {
            in_quotes = true;
            index = index + 1;
            continue;
        }
        if character == '"' && in_quotes {
            let doubled = index + 1 < characters.len() && characters[index + 1] == '"';
            if doubled {
                current.push('"');
                index = index + 2;
                continue;
            }
            in_quotes = false;
            quote_closed = true;
            index = index + 1;
            continue;
        }
        if character == '"' {
            return Err(CodecError::StrayQuote);
        }
        if quote_closed {
            return Err(CodecError::StrayQuote);
        }
        current.push(character);
        index = index + 1;
    }
    if in_quotes {
        return Err(CodecError::UnterminatedQuote);
    }
    fields.push(current);
    return Ok(fields);
}

/// Encode fields into one record line, terminated by nothing — the caller adds
/// the line ending.
///
/// A field is quoted only when it needs to be: when it contains `;` or `"`, or
/// starts or ends with a space. A `"` inside a quoted field is doubled. An
/// empty field stays empty, so an optional column that carries no value is
/// written as nothing between two separators.
///
/// One extra quoting case is forced by the round-trip requirement: a `#` at the
/// start of the FIRST field would make the written line a comment line, which
/// read would drop, so that field is quoted (a quoted field starts with `"`,
/// hence is not a comment).
///
/// A field containing a line break is the typed error [`CodecError.NewlineInField`],
/// never a silently split record.
pub fn render_record(fields: &Vec::<String>) -> Result::<String, CodecError> {
    if fields.len() == 0 {
        return Err(CodecError::EmptyRecord);
    }
    let mut parts = Vec::<String>::new();
    for index in 0..fields.len() {
        let field = &fields[index];
        if field.contains("\n") || field.contains("\r") {
            return Err(CodecError::NewlineInField);
        }
        let first_field = index == 0;
        let needs_quotes = field.contains(";") || field.contains("\"") || field.starts_with(" ") || field.ends_with(" ") || (first_field && field.starts_with("#"));
        if needs_quotes {
            let mut quoted = "\"".to_string();
            for character in field.chars() {
                if character == '"' {
                    quoted.push('"');
                }
                quoted.push(character);
            }
            quoted.push('"');
            parts.push(quoted);
        } else {
            parts.push(field.to_string());
        }
    }
    return Ok(join(parts, ";".to_string()));
}

/// Join the rendered fields with `separator`.
fn join(parts: Vec::<String>, separator: String) -> String {
    let mut joined = String::new();
    for index in 0..parts.len() {
        if index > 0 {
            joined.push_str(&separator);
        }
        joined.push_str(&parts[index]);
    }
    return joined;
}

/// Split a whole file's text into lines, tolerating CRLF.
///
/// Lines are cut on `\n`; one trailing `\r` is dropped from each line, which is
/// what makes a CRLF file readable while the codec always writes LF
/// (`doc/architecture/data-files.md`, "Line endings"). A `\r` anywhere else stays
/// part of the line, so [`parse_record`] rejects it as
/// [`CodecError.NewlineInField`] — a stray carriage return in the middle of a
/// field is corruption, not a line ending.
///
/// A trailing line ending does not produce an extra empty line.
fn split_lines(text: &str) -> Vec::<String> {
    let mut lines = Vec::<String>::new();
    let mut current = String::new();
    for character in text.chars() {
        if character == '\n' {
            lines.push(drop_carriage_return(&current));
            current = String::new();
            continue;
        }
        current.push(character);
    }
    if current.len() > 0 {
        lines.push(drop_carriage_return(&current));
    }
    return lines;
}

/// The line without one trailing `\r`; a line without one is returned as-is.
fn drop_carriage_return(line: &String) -> String {
    if line.ends_with("\r") {
        return line[..line.len() - "\r".len()].to_string();
    }
    return line.to_string();
}

/// Read a whole `.csv` file into its comment lines and records, in file order.
///
/// A line starting with `#` becomes [`Line::Comment`] and keeps its text
/// verbatim, so the `# columns:` preamble and any note a user wrote survive a
/// read → write round trip in the position where they wrote it. Empty and
/// whitespace-only lines are ignored, as the codec contract says, and therefore
/// do not survive the round trip. Every other line is decoded by
/// [`parse_record`]; a corrupt line is the typed error that call returns, so
/// reading aborts — the store readers of plan item 2.3 catch that error per
/// line if they want to keep reading past it.
///
/// The field count is not checked here; the per-file column set is applied by
/// the store module that builds the domain struct from the fields.
pub fn read_document(text: &str) -> Result::<Vec::<Line>, CodecError> {
    let lines = split_lines(text);
    let mut document = Vec::<Line>::new();
    for line in &lines {
        if is_blank(line) {
            continue;
        }
        if line.starts_with("#") {
            document.push(Line::Comment(line.to_string()));
            continue;
        }
        match parse_record(line) {
            Err(reason) => { return Err(reason); }
            Ok(fields) => { document.push(Line::Record(fields)); }
        }
    }
    return Ok(document);
}

/// Render a document back into file text: comments verbatim, records encoded
/// by [`render_record`], every line ended with LF, and a trailing LF after the
/// last line.
///
/// `write_document(read_document(t))` equals `t` for every file text `t` that
/// uses LF line endings, has no blank lines and no redundant quoting — the
/// shape `testdata/` fixtures are written in — so a store module that only
/// appends can rely on the untouched lines staying byte-identical.
pub fn write_document(lines: &Vec::<Line>) -> Result::<String, CodecError> {
    let mut parts = Vec::<String>::new();
    for line in lines {
        match line {
            Line::Comment(text) => { parts.push(text.to_string()); }
            Line::Record(fields) => {
                match render_record(fields) {
                    Err(reason) => { return Err(reason); }
                    Ok(rendered) => { parts.push(rendered); }
                }
            }
        }
    }
    if parts.len() == 0 {
        return Ok("".to_string());
    }
    let mut text = join(parts, "\n".to_string());
    text.push('\n');
    return Ok(text);
}

/// The records of a document, in file order, comments dropped.
///
/// This is what the store readers hand to the domain constructors; the field
/// count is whatever the file held, so the caller checks it against the column
/// set of its file.
pub fn records_of(lines: &Vec::<Line>) -> Vec::<Vec::<String>> {
    let mut records = Vec::<Vec::<String>>::new();
    for line in lines {
        match line {
            Line::Comment(_) => { }
            Line::Record(fields) => { records.push(fields.clone()); }
        }
    }
    return records;
}

/// The comment lines of a document, in file order.
///
/// Useful for the `# columns:` preamble a store module checks before reading,
/// and for reporting a file's comments to the user.
pub fn comments_of(lines: &Vec::<Line>) -> Vec::<String> {
    let mut comments = Vec::<String>::new();
    for line in lines {
        match line {
            Line::Comment(text) => { comments.push(text.to_string()); }
            Line::Record(_) => { }
        }
    }
    return comments;
}

/// True when the line is empty or holds only whitespace.
fn is_blank(line: &String) -> bool {
    for character in line.chars() {
        if character != ' ' && character != '\t' {
            return false;
        }
    }
    return true;
}

#[test]
fn round_trip_identity_for_twenty_generated_records() {
    let vocabulary = ["plain", "with;semicolon", "with\"quote", " leading space", "trailing space ", "", "#hash", "a;b\"c", "1.234,56 €", "REWE MARKT 12"];
    let mut generated = 0;
    for index in 0..20 {
        let mut record = Vec::<String>::new();
        record.push(vocabulary[index % vocabulary.len()].to_string());
        record.push(vocabulary[(index * 3 + 1) % vocabulary.len()].to_string());
        record.push(vocabulary[(index * 7 + 5) % vocabulary.len()].to_string());
        match render_record(&record) {
            Err(_) => { assert!(false); }
            Ok(line) => {
                generated = generated + 1;
                assert!(parse_record(&line).unwrap() == record);
            }
        }
    }
    assert!(generated == 20);

    let mut document = Vec::<Line>::new();
    document.push(Line::Comment("# columns: a;b;c".to_string()));
    for index in 0..20 {
        let mut record = Vec::<String>::new();
        record.push(vocabulary[index % vocabulary.len()].to_string());
        record.push(vocabulary[(index * 3 + 1) % vocabulary.len()].to_string());
        record.push(vocabulary[(index * 7 + 5) % vocabulary.len()].to_string());
        document.push(Line::Record(record));
    }
    let written = write_document(&document).unwrap();
    let reread = read_document(&written).unwrap();
    assert!(reread == document);
    assert!(records_of(&reread).len() == 20);
    assert!(comments_of(&reread) == comments_of(&document));
}

#[test]
fn field_with_semicolon_and_quote_round_trips() {
    let mut tricky = Vec::<String>::new();
    tricky.push("140ef279a4".to_string());
    tricky.push("TAXI \"AIRPORT; NIGHT\"; \"NIGHT\"".to_string());
    tricky.push("".to_string());
    let rendered = render_record(&tricky).unwrap();
    assert!(rendered.contains("\"\""));
    assert!(parse_record(&rendered).unwrap() == tricky);

    let mut one_quote = Vec::<String>::new();
    one_quote.push("\"".to_string());
    assert!(render_record(&one_quote).unwrap() == "\"\"\"\"");
    assert!(parse_record("\"\"\"\"").unwrap() == one_quote);

    let mut quoted_and_spaced = Vec::<String>::new();
    quoted_and_spaced.push(" SUPERMARKET; DELICATES ".to_string());
    let shown = render_record(&quoted_and_spaced).unwrap();
    assert!(shown.starts_with("\" ") && shown.ends_with(" \""));
    assert!(parse_record(&shown).unwrap() == quoted_and_spaced);
}

#[test]
fn newline_in_field_is_a_typed_error() {
    let mut split_record = Vec::<String>::new();
    split_record.push("39945b2fed".to_string());
    split_record.push("REWE\nMARKT".to_string());
    assert!(render_record(&split_record).unwrap_err() == CodecError::NewlineInField);

    let mut carriage_record = Vec::<String>::new();
    carriage_record.push("REWE\rMARKT".to_string());
    assert!(render_record(&carriage_record).unwrap_err() == CodecError::NewlineInField);

    assert!(parse_record("39945b2fed;REWE\nMARKT").unwrap_err() == CodecError::NewlineInField);
    assert!(parse_record("a;b\rc").unwrap_err() == CodecError::NewlineInField);
}

#[test]
fn crlf_input_is_read_and_lf_is_written() {
    let crlf = "# columns: id;date;amount\r\n39945b2fed;2024-01-03;-3999\r\n80c63211a8#2;2024-03-05;-1500\r\n";
    let lf = "# columns: id;date;amount\n39945b2fed;2024-01-03;-3999\n80c63211a8#2;2024-03-05;-1500\n";
    let from_crlf = read_document(&crlf).unwrap();
    let from_lf = read_document(&lf).unwrap();
    assert!(from_crlf == from_lf);
    assert!(records_of(&from_crlf).len() == 2);
    assert!(records_of(&from_crlf)[1][0] == "80c63211a8#2");
    let written = write_document(&from_crlf).unwrap();
    assert!(written == lf);
    assert!(!written.contains("\r"));
    assert!(read_document(&written).unwrap() == from_crlf);
}

#[test]
fn comment_lines_are_preserved_on_write() {
    let original = "# Phin categories -- tree of parent:child names, order is preserved.\n# columns: parent;name\n;food\nfood;supermarket\n# a note written by hand below the data\nleisure;games\n";
    let document = read_document(&original).unwrap();
    assert!(write_document(&document).unwrap() == original);
    let comments = comments_of(&document);
    assert!(comments.len() == 3);
    assert!(comments[0] == "# Phin categories -- tree of parent:child names, order is preserved.");
    assert!(comments[1] == "# columns: parent;name");
    assert!(comments[2] == "# a note written by hand below the data");
    assert!(records_of(&document).len() == 3);
    assert!(records_of(&document)[0][0] == "");
    assert!(records_of(&document)[0][1] == "food");

    let mut broken = Vec::<Line>::new();
    broken.push(Line::Comment("# note".to_string()));
    broken.push(Line::Record(Vec::<String>::new()));
    assert!(write_document(&broken).unwrap_err() == CodecError::EmptyRecord);
}

#[test]
fn empty_optional_columns_round_trip() {
    let line = "39945b2fed;2024-01-03;-3999;EUR;REWE MARKT 12;;";
    let fields = parse_record(line).unwrap();
    assert!(fields.len() == 7);
    assert!(fields[5] == "");
    assert!(fields[6] == "");
    assert!(render_record(&fields).unwrap() == line);

    let mut root_category = Vec::<String>::new();
    root_category.push("".to_string());
    root_category.push("food".to_string());
    assert!(render_record(&root_category).unwrap() == ";food");
    assert!(parse_record(";food").unwrap() == root_category);

    let mut pending_annotation = Vec::<String>::new();
    pending_annotation.push("a1b2c3d4e5".to_string());
    pending_annotation.push("".to_string());
    pending_annotation.push("bought milk".to_string());
    pending_annotation.push("milk".to_string());
    pending_annotation.push("mail_paypal.mbox".to_string());
    pending_annotation.push("0.4".to_string());
    let shown = render_record(&pending_annotation).unwrap();
    assert!(shown == "a1b2c3d4e5;;bought milk;milk;mail_paypal.mbox;0.4");
    assert!(parse_record(&shown).unwrap() == pending_annotation);
}

#[test]
fn quoting_is_applied_only_when_needed() {
    let fixture_line = "06113c369c;2024-04-21;3450;EUR;\"SUPERMARKET; DELICATES\";DELICATES;mail_paypal.mbox";
    let fields = parse_record(fixture_line).unwrap();
    assert!(fields[4] == "SUPERMARKET; DELICATES");
    assert!(render_record(&fields).unwrap() == fixture_line);

    let mut plain = Vec::<String>::new();
    plain.push("REWE MARKT 12".to_string());
    plain.push("#2".to_string());
    plain.push("a#b".to_string());
    assert!(render_record(&plain).unwrap() == "REWE MARKT 12;#2;a#b");

    let mut first_field_needs_quotes = Vec::<String>::new();
    first_field_needs_quotes.push("# columns: id".to_string());
    first_field_needs_quotes.push("parent;name".to_string());
    let shown = render_record(&first_field_needs_quotes).unwrap();
    assert!(shown.starts_with("\"#"));
    assert!(parse_record(&shown).unwrap() == first_field_needs_quotes);
}

#[test]
fn corrupt_lines_are_typed_errors_not_panics() {
    assert!(parse_record("a;\"never closed").unwrap_err() == CodecError::UnterminatedQuote);
    assert!(parse_record("a;b\"c;d").unwrap_err() == CodecError::StrayQuote);
    assert!(parse_record("a;\"x\"y;d").unwrap_err() == CodecError::StrayQuote);
    assert!(parse_record("# columns: id;date").unwrap_err() == CodecError::NotARecord);
    assert!(parse_record("").unwrap_err() == CodecError::NotARecord);

    let no_fields = Vec::<String>::new();
    assert!(render_record(&no_fields).unwrap_err() == CodecError::EmptyRecord);

    let broken = "# columns: id;date\n39945b2fed;2024-01-03\n80c63211a8;\"broken\n";
    assert!(read_document(&broken).unwrap_err() == CodecError::UnterminatedQuote);
}

#[test]
fn money_parse_error_compares_with_equality() {
    use crate::domain::money::ParseError as MoneyParseError;
    assert!(MoneyParseError::Malformed == MoneyParseError::Malformed);
    assert!(MoneyParseError::Overflow != MoneyParseError::Malformed);
    match crate::domain::money::parse("12,34 €x") {
        Err(reason) => { assert!(reason == MoneyParseError::Malformed); }
        Ok(_) => { assert!(false); }
    }
}
