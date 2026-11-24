//! The `pdftotext -layout` call helper of plan item 4.1 — the ONLY way Phase 4
//! turns a PDF file into text.
//!
//! The recorded decision is in `doc/architecture/architecture.md` ("PDF text
//! extraction: `pdftotext -layout`; the alternative is a native binding such as
//! `pdfium-render`"): phin shells out to poppler's `pdftotext` and keeps the
//! `-layout` output, which preserves the column alignment of the table rows so
//! the bank parsers of plan items 4.2/4.3 can split a row on whitespace. The
//! alternative (`pdfium-render`, i.e. no external tool at all) is a drop-in
//! swap BECAUSE this module contains the whole decision: nothing outside this
//! file knows that `pdftotext` exists, and the only surface the importers of
//! plan item 4.4+ use is [`extract_text`]. Swapping the implementation would
//! touch neither the callers nor the error type's shape beyond the variant
//! naming a missing tool.
//!
//! The text is returned VERBATIM: the form feed characters `pdftotext` emits
//! between pages are kept, and nothing is trimmed, so a page boundary stays
//! visible to the parsers (they work line by line and never see a form feed as
//! a row).
//!
//! This module performs I/O (it starts a process) and therefore lives on the
//! technical side; the domain never sees anything but the resulting text.

use std::path::Path;
use std::process::Command;

/// The program name Phase 4 uses for PDF text extraction. Named once here so
/// the `pdfium-render` alternative of `doc/architecture/architecture.md` has a
/// single place to change, and so [`extract_text_with`] can run a different
/// program in a test.
const DEFAULT_PROGRAM: &str = "pdftotext";

/// Why extracting text from a PDF did not work.
///
/// Typed, never a panic: the caller of plan item 4.4 decides whether a missing
/// tool is a `converter-missing` error event or a startup-time complaint.
#[derive(Debug, Eq, PartialEq)]
pub enum PdfTextError {
    /// The converter program is not installed (the spawn failed with
    /// `NotFound`). Maps to the `converter-missing` error code of
    /// `doc/architecture/core-protocol.md`.
    ToolMissing,
    /// Any other failure of the spawn/IO plumbing (the message is the OS
    /// error's own text). Maps to the `io` error code.
    Io(String),
    /// The program ran but reported failure: non-zero exit status (the string
    /// is the captured stderr, i.e. poppler's own explanation — including the
    /// case of a file that is not a readable PDF).
    Failed(String),
}

/// Extract the layout-preserving text of the PDF at `path` with the system's
/// `pdftotext`.
///
/// The convenience wrapper every Phase-4 importer calls; see
/// [`extract_text_with`] for what actually happens.
pub fn extract_text(path: &Path) -> Result<String, PdfTextError> {
    return extract_text_with(DEFAULT_PROGRAM, path);
}

/// Extract the layout-preserving text of the PDF at `path` by running
/// `<program> -layout <path> -` and capturing its stdout.
///
/// `program` is injectable on purpose (decision 6 of the Phase-4 plan): the
/// missing-tool case must be testable without touching `PATH`, so a test passes
/// a program name that cannot exist. The arguments are exactly those of the
/// `pdftotext` decision: `-layout` keeps the column layout of a table, the file
/// path is passed as a POSIX path, and `-` means "write the text to stdout", so
/// no temporary output file is ever created.
///
/// The stdout is decoded lossily as UTF-8 (a statement may carry bytes that are
/// not valid UTF-8; those become the replacement character rather than failing
/// the import) and returned VERBATIM — no trimming, form feeds between pages included.
///
/// A non-zero exit status is [`PdfTextError::Failed`] carrying the captured
/// stderr, so a non-existent or unreadable PDF is a typed error, never a panic.
/// A spawn failure whose kind is `NotFound` means the converter is absent and
/// yields [`PdfTextError::ToolMissing`]; every other spawn/IO error yields
/// [`PdfTextError::Io`].
pub fn extract_text_with(program: &str, path: &Path) -> Result<String, PdfTextError> {
    let output = match Command::new(program).arg("-layout").arg(path).arg("-").output() {
        Err(reason) => {
            if reason.kind() == std::io::ErrorKind::NotFound {
                return Err(PdfTextError::ToolMissing);
            }
            return Err(PdfTextError::Io(reason.to_string()));
        }
        Ok(output) => { output }
    };

    if !output.status.success() {
        let complaint = String::from_utf8_lossy(&output.stderr).to_string();
        return Err(PdfTextError::Failed(complaint));
    }

    return Ok(String::from_utf8_lossy(&output.stdout).to_string());
}

/// Locate `testdata/` relative to the test's working directory.
///
/// Copied from the same pattern `crate::technical::id` uses (and kept private
/// to this module): `cargo test` runs the test binary with the package root
/// (`core/`) as its working directory, so the fixture directory sits one level
/// up, next to the package; the search walks up the parents of the current
/// directory, trying only `testdata` per directory, so it never scans anything
/// else.
#[cfg(test)]
fn find_testdata_dir() -> std::path::PathBuf {
    let start = std::env::current_dir().unwrap_or(std::path::PathBuf::new());
    let mut dir = start;
    loop {
        let candidate = dir.join("testdata");
        if candidate.is_dir() { return candidate; }
        match dir.parent() {
            None => { panic!("pdf_text tests cannot find testdata relative to the test's working directory"); }
            Some(parent) => { dir = parent.to_path_buf(); }
        }
    }
}

/// The SCRATCH PARENT of THIS module: MODULE-PRIVATE, following the pattern the
/// repo already uses for `test-scratch-view`, `test-scratch-fs-atomic` and
/// `test-scratch-transport-cfg`, so no test's cleanup can ever delete another
/// module's scratch tree (the flake documented by the review of plan item 3.6b
/// came from the SHARED `test-scratch` parent, which is GONE now).
#[cfg(test)]
const SCRATCH_PARENT: &str = "test-scratch-pdf-text";

/// A blank, syntactically valid one-page PDF with NO content stream: a catalog,
/// a page tree, one page with a `MediaBox`, and a correct `xref`/`trailer`.
/// `pdftotext` accepts it and prints nothing, which is what makes
/// [`empty_pdf_yields_empty_result`] test the "empty PDF" edge case without a
/// fixture generator. Regenerate by writing these exact bytes with
/// `python3 -c`-style byte concatenation if a tool ever rejects them; do not
/// edit them by hand.
#[cfg(test)]
const BLANK_PDF_BYTES: &[u8] = b"%PDF-1.4\n1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 >>\nendobj\n3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox[0 0 300 300] >>\nendobj\nxref\n0 4\n0000000000 65535 f \n0000000009 00000 n \n0000000058 00000 n \n0000000115 00000 n \ntrailer\n<< /Size 4 /Root 1 0 R >>\nstartxref\n185\n%%EOF\n";

#[test]
fn fixture_bank_a_text_in_reading_order() {
    // The fixture PDFs are read, never written; `testdata/` is read-only here.
    let path = find_testdata_dir().join("bank_a.pdf");
    let text = extract_text(&path).expect("pdftotext must extract the fixture bank_a.pdf");

    // Reading order survives: the header of the statement comes first.
    assert!(text.contains("Bank A AG"));
    assert!(text.find("Bank A AG").unwrap() < text.find("MEGA MART").unwrap());

    // `-layout` kept the columns: the amount sits on its OWN row, attached to
    // its description, so a parser can split the line into fields.
    let mut mega_line: Option<String> = None;
    for line in text.lines() {
        if line.contains("MEGA MART") {
            mega_line = Some(line.to_string());
        }
    }
    let row = mega_line.expect("the fixture bank_a.pdf must carry a MEGA MART row");
    assert!(row.contains("-1.234,56"), "the amount must stay attached to its row: {row}");
    assert!(row.contains("24.01.2024"), "the date of that row must be on the same line: {row}");
    // The form feed / summary structure of the fixture is untouched: the text is
    // returned verbatim, so the total line of the statement is still there.
    assert!(text.contains("Total: -1.062,82 EUR"));
}

#[test]
fn missing_tool_is_typed_error() {
    // The program name is injected, so this tests the spawn failure itself
    // without touching PATH (decision 6 of the Phase-4 plan).
    let path = find_testdata_dir().join("bank_a.pdf");
    let outcome = extract_text_with("definitely-not-pdftotext-xyz", &path);
    assert!(outcome == Err(PdfTextError::ToolMissing), "{outcome:?}");
}

#[test]
fn empty_pdf_yields_empty_result() {
    // A blank page is not an error: exit 0, empty stdout, so the caller sees an
    // empty text (a form feed would be allowed, hence trim, not equality).
    let dir = std::path::PathBuf::from(SCRATCH_PARENT).join("pdf_text");
    match std::fs::create_dir_all(&dir) {
        Err(reason) => { panic!("cannot create the pdf_text scratch directory: {reason}"); }
        Ok(_) => { }
    }
    let path = dir.join("blank.pdf");
    match std::fs::write(&path, BLANK_PDF_BYTES) {
        Err(reason) => { panic!("cannot write the blank PDF fixture: {reason}"); }
        Ok(_) => { }
    }

    let text = match extract_text(&path) {
        Err(reason) => { panic!("a blank PDF must extract to empty text, got {reason:?}"); }
        Ok(text) => { text }
    };
    assert!(text.trim().is_empty(), "expected empty text, got {text:?}");

    match std::fs::remove_file(&path) { Err(_) => { } Ok(_) => { } }
    match std::fs::remove_dir(&dir) { Err(_) => { } Ok(_) => { } }
    match std::fs::remove_dir(std::path::PathBuf::from(SCRATCH_PARENT)) { Err(_) => { } Ok(_) => { } }
}

#[test]
fn nonexistent_file_is_failed() {
    // pdftotext itself reports "I/O Error: Couldn't open file ..."; that is a
    // non-zero exit status, i.e. a typed Failed carrying poppler's wording,
    // NOT a panic and NOT ToolMissing.
    let path = find_testdata_dir().join("does-not-exist-at-all.pdf");
    let outcome = extract_text(&path);
    match &outcome {
        Err(PdfTextError::Failed(_)) => { }
        other => { panic!("a missing PDF file must be a typed Failed error, got {other:?}"); }
    }
}
