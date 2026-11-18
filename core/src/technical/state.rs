//! The import markers of `state/markers.csv` — plan item 2.7.
//!
//! One marker per source file (`path;mtime;size;offset`) lets the mail
//! importer resume incrementally: an unchanged source file is SKIPPED, a
//! changed one REPROCESSED from its recorded byte offset. The contract is
//! `doc/architecture/data-files.md`, section "`state/`": the file uses the
//! 2.1 codec of [`crate::technical::csv`] with columns
//! `path;mtime;size;offset`, documented in a `# columns:` comment.
//!
//! Style follows [`crate::technical::store_sidefiles`]: text in, text out,
//! NO I/O; the file is append-only and the LAST record for a path wins on
//! read. A corrupt marker file is the plan's "treated as empty with a
//! warning": [`read_markers`] returns an EMPTY marker set plus typed
//! [`MarkerWarning`]s, never a panic.

use super::csv::render_record;
use super::csv::CodecError;
use super::csv::Line;
use super::csv::read_document;
use super::csv::write_document;
use super::csv::records_of;

/// Columns of `state/markers.csv` (`path;mtime;size;offset`).
const MARKER_COLUMN_COUNT: usize = 4;

/// One import marker: `mtime`/`size` identify the processed version, `offset`
/// is the byte position the next pass resumes from.
#[derive(Debug, Eq, PartialEq)]
pub struct Marker {
    pub path: String,
    pub mtime: i64,
    pub size: i64,
    pub offset: i64,
}

/// Why the marker file could not be trusted: ANY warning empties the whole
/// marker set (plan 2.7: "treated as empty with a warning").
#[derive(Debug, Eq, PartialEq)]
pub enum MarkerWarning {
    /// [`super::csv::read_document`] rejected a line; the reason is the codec's own.
    MalformedLine(CodecError),
    /// A record decoded, but not into [`MARKER_COLUMN_COUNT`] columns.
    WrongColumnCount(usize),
    /// The `mtime`/`size`/`offset` column of a record was not a whole number.
    InvalidNumber(String),
}

/// What the importer does with one source file, given its marker.
#[derive(Debug, Eq, PartialEq)]
pub enum ImportDecision {
    /// The source file is unchanged (same mtime AND same size as recorded).
    Skip,
    /// The source file is newer (mtime changed) or a size difference was
    /// detected — or no marker names it at all.
    Reprocess,
}

/// Read the whole text of a `state/markers.csv` file into document lines,
/// markers and warnings.
///
/// Comments stay verbatim IN POSITION so [`write_markers`] round-trips the
/// file; records must hold [`MARKER_COLUMN_COUNT`] numeric fields. Corruption
/// never aborts or panics, but it empties the marker set — the plan's
/// "treated as empty with a warning" — while the lines still round-trip.
pub fn read_markers(text: &str) -> (Vec::<Line>, Vec::<Marker>, Vec::<MarkerWarning>) {
    match read_document(&text) {
        Err(reason) => {
            let mut warnings = Vec::<MarkerWarning>::new();
            warnings.push(MarkerWarning::MalformedLine(reason));
            return (Vec::<Line>::new(), Vec::<Marker>::new(), warnings);
        }
        Ok(lines) => {
            let mut markers = Vec::<Marker>::new();
            let mut warnings = Vec::<MarkerWarning>::new();
            for fields in records_of(&lines) {
                if fields.len() != MARKER_COLUMN_COUNT {
                    warnings.push(MarkerWarning::WrongColumnCount(fields.len()));
                    continue;
                }
                let mtime = fields[1].parse::<i64>();
                let size = fields[2].parse::<i64>();
                let offset = fields[3].parse::<i64>();
                match (mtime, size, offset) {
                    (Ok(mtime_value), Ok(size_value), Ok(offset_value)) => {
                        markers.push(Marker { path: fields[0].to_string(), mtime: mtime_value, size: size_value, offset: offset_value });
                    }
                    _ => { warnings.push(MarkerWarning::InvalidNumber(fields[0].to_string())); }
                }
            }
            if warnings.len() > 0 {
                return (lines, Vec::<Marker>::new(), warnings);
            }
            return (lines, markers, warnings);
        }
    }
}

/// Render document lines read by [`read_markers`] back into `state/markers.csv`
/// text: comments verbatim, records encoded by the codec. Delegates to
/// [`super::csv::write_document`]; a round trip over fixture-shape text is
/// byte-identical.
pub fn write_markers(lines: &Vec::<Line>) -> Result::<String, CodecError> {
    return write_document(lines);
}

/// The marker of `path`, or `None` when unmarked: the LAST record for a path
/// wins (append-only last-wins, as for `overrides.csv`).
pub fn marker_of(markers: &Vec::<Marker>, path: &str) -> Option::<Marker> {
    let mut found: Option::<Marker> = None;
    for marker in markers {
        if marker.path == path {
            found = Some(Marker { path: marker.path.to_string(), mtime: marker.mtime, size: marker.size, offset: marker.offset });
        }
    }
    return found;
}

/// The importer's decision for one source file: SKIP only when UNCHANGED
/// against its marker (same `mtime` AND same `size`); REPROCESS when the
/// mtime differs at all (newer, or older — the file was replaced), when the
/// size differs, or when no marker names the path.
pub fn decide(markers: &Vec::<Marker>, path: &str, mtime: i64, size: i64) -> ImportDecision {
    let marker = marker_of(markers, path);
    if marker == None {
        return ImportDecision::Reprocess;
    }
    let marker = marker.unwrap();
    if marker.mtime == mtime && marker.size == size {
        return ImportDecision::Skip;
    }
    return ImportDecision::Reprocess;
}

/// Whether an [`append_marker`] wrote a line.
#[derive(Debug, Eq, PartialEq)]
pub enum AppendOutcome {
    /// The record was not yet in the file: one line was appended.
    Appended,
    /// The EXACT marker is already known: the text is returned unchanged.
    Skipped,
}

/// Append one marker line to the text of a `state/markers.csv` file.
///
/// Text in, text out — no I/O; the existing text stays an untouched prefix.
/// SKIPPED only when the EXACT marker is already known; a NEWER marker for a
/// path on file is appended, so the last-wins read above sees it.
pub fn append_marker(text: &str, marker: &Marker) -> Result::<(String, AppendOutcome), CodecError> {
    let (_, markers, _) = read_markers(text);
    for known in markers {
        if known == *marker {
            return Ok((text.to_string(), AppendOutcome::Skipped));
        }
    }
    let mut fields = Vec::<String>::new();
    fields.push(marker.path.to_string());
    fields.push(marker.mtime.to_string());
    fields.push(marker.size.to_string());
    fields.push(marker.offset.to_string());
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

#[test]
fn unchanged_source_file_is_skipped_by_its_marker() {
    let mut markers = Vec::<Marker>::new();
    markers.push(Marker { path: "mail_rewe.mbox".to_string(), mtime: 1700000000, size: 4096, offset: 2048 });
    assert!(decide(&markers, "mail_rewe.mbox", 1700000000, 4096) == ImportDecision::Skip);
    // Same size but a newer mtime → reprocess.
    assert!(decide(&markers, "mail_rewe.mbox", 1700000001, 4096) == ImportDecision::Reprocess);
    // Same mtime but a different size → reprocess.
    assert!(decide(&markers, "mail_rewe.mbox", 1700000000, 4100) == ImportDecision::Reprocess);
    // An unmarked file is reprocessed.
    assert!(decide(&markers, "mail_paypal.mbox", 1, 1) == ImportDecision::Reprocess);
}

#[test]
fn corrupt_marker_file_is_treated_as_empty_with_a_typed_warning_and_never_panics() {
    // Unterminated quote: the codec itself rejects the document.
    let broken = "# columns: path;mtime;size;offset\nmail_rewe.mbox;\"never closed\n";
    let (_, markers, warnings) = read_markers(broken);
    assert!(markers.len() == 0);
    assert!(warnings.len() == 1);
    match warnings[0] {
        MarkerWarning::MalformedLine(CodecError::UnterminatedQuote) => { }
        _ => { assert!(false); }
    }
    // The decision logic then reprocesses everything, safely.
    assert!(decide(&markers, "mail_rewe.mbox", 1700000000, 4096) == ImportDecision::Reprocess);
    // Wrong column count and a non-numeric column also empty the set.
    let (_, markers2, warnings2) = read_markers("# columns: path;mtime;size;offset\nmail_x.mbox;1700000000;4096\nmail_y.mbox;high;4096;0\n");
    assert!(markers2.len() == 0);
    assert!(warnings2.len() == 2);
    match warnings2[0] {
        MarkerWarning::WrongColumnCount(3) => { }
        _ => { assert!(false); }
    }
    assert!(warnings2[1] == MarkerWarning::InvalidNumber("mail_y.mbox".to_string()));
}

#[test]
fn marker_file_round_trips_through_the_codec_with_comments_preserved() {
    let original = "# Phin import markers -- resume state per source file.\n# columns: path;mtime;size;offset\nmail_rewe.mbox;1700000000;4096;2048\n\"mail odd; name.mbox\";1700000001;512;0\n";
    let (lines, markers, warnings) = read_markers(&original);
    assert!(warnings.len() == 0);
    assert!(markers.len() == 2);
    assert!(markers[1].path == "mail odd; name.mbox".to_string());
    assert!(write_markers(&lines).unwrap() == original);
    // The last record for a path wins in an append-only file.
    let newer = Marker { path: "mail_rewe.mbox".to_string(), mtime: 1700000002, size: 4200, offset: 3000 };
    let (appended, outcome) = append_marker(&original, &newer).unwrap();
    assert!(outcome == AppendOutcome::Appended);
    assert!(appended.starts_with(original));
    let (_, markers2, _) = read_markers(&appended);
    let latest = marker_of(&markers2, "mail_rewe.mbox");
    assert!(latest != None);
    assert!(latest.unwrap().mtime == 1700000002);
    // Appending the EXACT same marker again is idempotent.
    let (same_text, same_outcome) = append_marker(&appended, &newer).unwrap();
    assert!(same_outcome == AppendOutcome::Skipped);
    assert!(same_text == appended);
}
