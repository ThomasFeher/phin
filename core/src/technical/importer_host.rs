//! The importer host of plan item 4.4a: one contract for every importer
//! (built-in or, from plan item 4.4b, executable plugin) plus the orchestration
//! loop that runs it.
//!
//! This is the in-code form of `doc/architecture/architecture.md` ("Import &
//! Extraction"): *probe every configured importer, then extract from the ones
//! that accepted it*. [`import_file`] implements exactly that order — probe
//! first, and only then ask the supporters for transactions — so a file a bank
//! does not understand is never handed to that bank's parser, and a broken
//! importer cannot stop the other ones.
//!
//! Two deliberate absences keep this layer small and testable:
//!
//! * **No protocol events.** The host returns plain data
//!   ([`FileOutcome`]); plan item 4.8 owns turning it into `progress`, `log`
//!   and `result`/`error` frames of `doc/architecture/core-protocol.md`. This
//!   is why the host can be unit-tested without a daemon (plan ground rule
//!   "Test the domain, not the process"), and it is planning decision 14.
//! * **No process spawning and no configuration.** The plugin side of plan
//!   item 4.4b (`doc/architecture/plugin-protocol.md`) plugs into the same loop
//!   by implementing [`Importer`] in a separate adapter; discovering and
//!   starting those processes is 4.4b's job, reading `phin.toml` is 4.4b's too.
//!
//! The built-ins here are the two statement readers of plan items 4.2/4.3; they
//! probe by CONTENT MARKER because no filename convention exists anywhere in
//! phin (planning decision 13). Each one accepts only its own marker, so a bank
//! B statement is `Unsupported` as far as bank A is concerned.

use std::path::Path;

use crate::domain::transaction::Transaction;
use crate::technical::id::source_basename;
use crate::technical::parse_bank_a::parse_bank_a;
use crate::technical::parse_bank_a::ParseReport;
use crate::technical::parse_bank_b::parse_bank_b;
use crate::technical::pdf_text::extract_text;
use crate::technical::pdf_text::PdfTextError;

/// The `kind` a `probe` request of `doc/architecture/plugin-protocol.md`
/// carries for statement importers (`bank-statement`); an importer of another
/// kind (a mailbox, plan item 4.5) advertises a different one.
pub const KIND_BANK_STATEMENT: &str = "bank-statement";

/// Dialect name [`ProbeKind::Dialect`] carries for the layout of fixture bank A.
pub const DIALECT_BANK_A: &str = "bank_a";
/// Dialect name [`ProbeKind::Dialect`] carries for the layout of fixture bank B.
pub const DIALECT_BANK_B: &str = "bank_b";
/// Dialect name a mailbox importer (plan item 4.5, registered by
/// [`builtin_importers`] later) carries; named here so the set of dialect names
/// lives in one place.
pub const DIALECT_MBOX: &str = "mbox";

/// Name of the built-in bank A importer, i.e. the string an `error` event of
/// plan item 4.8 uses to name a failing importer.
const NAME_BUILT_IN_BANK_A: &str = "built-in-bank-a";
/// Name of the built-in bank B importer.
const NAME_BUILT_IN_BANK_B: &str = "built-in-bank-b";

/// Content marker identifying a bank A statement (same marker the parser of
/// plan item 4.2 checks); probed on the FIRST line, where the statement header
/// of the fixture sits.
const MARKER_BANK_A: &str = "Bank A AG";
/// Content marker identifying a bank B statement (plan item 4.3's equivalent).
const MARKER_BANK_B: &str = "Bank B N";

/// What an [`Importer`]'s probe decided about one file.
///
/// `Dialect` names the concrete format the importer recognized (used for
/// logging and for choosing among several importers of the same kind, per
/// `doc/architecture/plugin-protocol.md`); `Unsupported` means "not mine, do
/// not ask me to extract anything".
#[derive(Debug, Eq, PartialEq)]
pub enum ProbeKind {
    /// The file belongs to this importer, and the string says which dialect.
    Dialect(String),
    /// This importer does not handle the file.
    Unsupported,
}

/// What one importer's extract run produced: the records it read and the
/// problems it wants the caller to report.
#[derive(Debug, Eq, PartialEq)]
pub struct Extracted {
    /// The transactions, in the order the source document prints them. No
    /// occurrence suffix is assigned here (that is the store's job, the
    /// `append` of `crate::technical::store_transactions` reached through plan
    /// item 4.8), and duplicates are kept.
    pub transactions: Vec<Transaction>,
    /// Human readable problems, phrased for a `log` event of plan item 4.8.
    pub warnings: Vec<String>,
}

/// Why an importer could not deliver transactions for a file it claimed to
/// support.
///
/// Typed, never a panic: plan item 4.8 maps this to an `error` event naming the
/// importer while the remaining importers keep running
/// (`doc/architecture/plugin-protocol.md`, "a failing importer must not break
/// the whole run").
#[derive(Debug, Eq, PartialEq)]
pub enum ImportError {
    /// The file (or the plumbing around it) could not be read at all; the
    /// string is the underlying reason. Maps to the `io` error code.
    Unreadable(String),
    /// The importer ran and failed: a converter complaint, a plugin reporting
    /// failure, or — for a built-in whose converter is absent — text naming the
    /// missing tool, which maps to the `converter-missing` error code.
    Failed(String),
}

/// The one contract every importer implements: built-ins (plan items 4.2/4.3),
/// the mailbox reader (plan item 4.5) and the executable plugins of plan item
/// 4.4b all speak it.
///
/// Object safe on purpose — the host holds `Box<dyn Importer>` — so the methods
/// take `&self` and a plain `&Path` and return owned data.
pub trait Importer {
    /// Stable importer name; it is what an `error` event of plan item 4.8
    /// names, so it must not change with the file it is reading.
    fn name(&self) -> String;

    /// Decide whether this importer handles `path`.
    ///
    /// Must be cheap, must never panic and must never fail: a file it cannot
    /// read (including a missing `pdftotext`) is simply [`ProbeKind::Unsupported`],
    /// because the failure becomes visible through [`Importer::extract_transactions`]
    /// of the importers that would have supported the file.
    fn probe(&self, path: &Path) -> ProbeKind;

    /// Extract the transactions of a file this importer announced as supported.
    /// Only importers whose [`Importer::probe`] said `Dialect` are ever asked.
    fn extract_transactions(&self, path: &Path) -> Result<Extracted, ImportError>;
}

/// The built-in importer of the bank A statement layout (plan item 4.2).
pub struct BuiltInBankA;

/// The built-in importer of the bank B statement layout (plan item 4.3).
pub struct BuiltInBankB;

impl Importer for BuiltInBankA {
    fn name(&self) -> String {
        return NAME_BUILT_IN_BANK_A.to_string();
    }

    fn probe(&self, path: &Path) -> ProbeKind {
        return probe_pdf_marker(path, MARKER_BANK_A, DIALECT_BANK_A);
    }

    fn extract_transactions(&self, path: &Path) -> Result<Extracted, ImportError> {
        let source = import_source(path);
        let report = match parse_bank_a(path, &source) {
            Err(reason) => { return Err(import_error_of(reason)); }
            Ok(report) => { report }
        };
        return Ok(extracted_of(report));
    }
}

impl Importer for BuiltInBankB {
    fn name(&self) -> String {
        return NAME_BUILT_IN_BANK_B.to_string();
    }

    fn probe(&self, path: &Path) -> ProbeKind {
        return probe_pdf_marker(path, MARKER_BANK_B, DIALECT_BANK_B);
    }

    fn extract_transactions(&self, path: &Path) -> Result<Extracted, ImportError> {
        let source = import_source(path);
        let report = match parse_bank_b(path, &source) {
            Err(reason) => { return Err(import_error_of(reason)); }
            Ok(report) => { report }
        };
        return Ok(extracted_of(report));
    }
}

/// The host: the importers a run consults, in the order they are consulted.
///
/// A built-in host uses [`builtin_importers`]; plan item 4.4b appends the
/// plugin importers it discovers from `phin.toml`, and plan item 4.5 appends
/// the mailbox reader.
pub struct Host {
    /// The configured importers, consulted in this order (probe pass first,
    /// extract pass second — order preserved in both).
    pub importers: Vec<Box<dyn Importer>>,
}

/// The built-in importers, in a fixed, deterministic order: bank A first, then
/// bank B. The mailbox importer of plan item 4.5 joins this list HERE — in the
/// task that creates `crate::technical::mbox`, this function gains a third
/// entry and nothing else changes — and the plugin importers of plan item 4.4b
/// are appended by the host that reads the configuration.
pub fn builtin_importers() -> Vec<Box<dyn Importer>> {
    let mut importers: Vec<Box<dyn Importer>> = Vec::new();
    importers.push(Box::new(BuiltInBankA));
    importers.push(Box::new(BuiltInBankB));
    return importers;
}

/// What one file produced across the whole host run.
///
/// Carries no protocol knowledge: plan item 4.8 turns it into events and into
/// the `result` payload of `doc/architecture/core-protocol.md`.
#[derive(Debug, Eq, PartialEq)]
pub struct FileOutcome {
    /// `(importer name, dialect)` of every importer that accepted the file, in
    /// the order the host probed them.
    pub accepted_by: Vec<(String, String)>,
    /// The transactions of all supporters merged in host order (a file claimed
    /// by two importers yields both extractions; duplicates are the store's
    /// problem, not the host's).
    pub transactions: Vec<Transaction>,
    /// Every warning of every supporter, in host order.
    pub warnings: Vec<String>,
    /// `(importer name, reason)` for every supporter whose extraction failed;
    /// the run continued regardless.
    pub failed: Vec<(String, ImportError)>,
}

/// Run the whole host over one file: probe every importer, then extract from
/// the supporters.
///
/// Probes everything in `host.importers` order and NEVER extracts from an
/// importer that said `Unsupported`; extracts from every supporter in the same
/// order, recording a failure in [`FileOutcome::failed`] instead of aborting the
/// run, so one broken importer cannot cost the other ones their transactions
/// (`doc/architecture/plugin-protocol.md`, "a failing importer must not break
/// the whole run"). Emits nothing: the caller decides what the wire looks like.
pub fn import_file(host: &Host, path: &Path) -> FileOutcome {
    // Pass one: probe. Every importer is asked, even when an earlier one
    // already claimed the file, because two importers may both handle it.
    let mut accepted_by: Vec<(String, String)> = Vec::new();
    let mut supporters: Vec<&dyn Importer> = Vec::new();
    for importer in &host.importers {
        match importer.probe(path) {
            ProbeKind::Unsupported => { }
            ProbeKind::Dialect(dialect) => {
                accepted_by.push((importer.name(), dialect));
                supporters.push(importer.as_ref());
            }
        }
    }

    // Pass two: extract from the supporters only, in probe order.
    let mut transactions: Vec<Transaction> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();
    let mut failed: Vec<(String, ImportError)> = Vec::new();
    for index in 0..supporters.len() {
        let importer = supporters[index];
        match importer.extract_transactions(path) {
            Err(reason) => { failed.push((importer.name(), reason)); }
            Ok(got) => {
                transactions.extend(got.transactions);
                warnings.extend(got.warnings);
            }
        }
    }

    return FileOutcome { accepted_by, transactions, warnings, failed };
}

/// The `source` provenance an import run stores for a file: the file NAME, never
/// the path it was read through ([`crate::technical::id::source_basename`]), so
/// re-importing a statement from another directory lands on the same id.
fn import_source(path: &Path) -> String {
    return source_basename(&path.to_string_lossy());
}

/// Map a PDF plumbing failure onto the host's own error type.
///
/// A missing converter is `Failed` with text naming it, which is how plan item
/// 4.8 recognizes the `converter-missing` case after the fact — the probe pass
/// swallowed that failure on purpose (every importer answered `Unsupported` for
/// a file it could not read), so the extract pass is the one place it surfaces.
fn import_error_of(reason: PdfTextError) -> ImportError {
    return match reason {
        PdfTextError::ToolMissing => {
            ImportError::Failed("converter-missing: the PDF text tool is not installed".to_string())
        }
        PdfTextError::Io(text) => { ImportError::Unreadable(text) }
        PdfTextError::Failed(text) => { ImportError::Failed(text) }
    };
}

/// Re-shape a parser report into [`Extracted`]: the warnings become the plain
/// text a `log` event carries, prefixed with the line they belong to.
fn extracted_of(report: ParseReport) -> Extracted {
    let mut warnings: Vec<String> = Vec::new();
    for warning in report.warnings {
        warnings.push(format!("line {}: {}", warning.line, warning.text));
    }
    return Extracted { transactions: report.transactions, warnings };
}

/// Cheap-first probe of a PDF-ish file: reject anything that is not a PDF
/// without reading it, then compare the first extracted line against
/// `marker`, answering `dialect` when it matches.
///
/// Any read failure — no `pdftotext`, an unreadable file, a file that is not a
/// PDF at all — is [`ProbeKind::Unsupported`], never a panic: a probe that
/// cannot tell must simply decline, and the failure surfaces through the
/// extract call of an importer that does claim the file.
fn probe_pdf_marker(path: &Path, marker: &str, dialect: &str) -> ProbeKind {
    if !extension_is(path, "pdf") {
        return ProbeKind::Unsupported;
    }
    let text = match extract_text(path) {
        Err(_) => { return ProbeKind::Unsupported; }
        Ok(text) => { text }
    };
    let first_line = match text.lines().next() {
        None => { return ProbeKind::Unsupported; }
        Some(line) => { line }
    };
    if first_line.contains(marker) {
        return ProbeKind::Dialect(dialect.to_string());
    }
    return ProbeKind::Unsupported;
}

/// True when `path` has the extension `expected`, compared case-insensitively
/// (`None.pdf`, `a.PDF`, `statement.pdf` all match `pdf`; `a.txt` does not).
fn extension_is(path: &Path, expected: &str) -> bool {
    return match path.extension() {
        None => { false }
        Some(extension) => { extension.to_string_lossy().to_ascii_lowercase() == expected }
    };
}

/// Locate `testdata/` relative to the test's working directory.
///
/// Copied from the same pattern `crate::technical::id`,
/// `crate::technical::pdf_text` and the two parsers use (and kept private to
/// this module): `cargo test` runs the test binary with the package root
/// (`core/`) as its working directory, so the fixture directory sits one level
/// up, and the search walks up the parents trying only `testdata` per directory.
#[cfg(test)]
fn find_testdata_dir() -> std::path::PathBuf {
    let start = std::env::current_dir().unwrap_or(std::path::PathBuf::new());
    let mut dir = start;
    loop {
        let candidate = dir.join("testdata");
        if candidate.is_dir() { return candidate; }
        match dir.parent() {
            None => { panic!("importer_host tests cannot find testdata relative to the test's working directory"); }
            Some(parent) => { dir = parent.to_path_buf(); }
        }
    }
}

/// The SCRATCH PARENT of THIS module: MODULE-PRIVATE, following the pattern of
/// `test-scratch-pdf-text` and friends, so no test's cleanup can delete another
/// module's scratch tree. Never the shared `test-scratch` parent.
#[cfg(test)]
const SCRATCH_PARENT: &str = "test-scratch-importer-host";

/// The pure statement parser the in-memory stub importers of these tests wrap:
/// the stubs hand out a fixed text so they never need a readable file.
#[cfg(test)]
use crate::technical::parse_bank_a::parse_bank_a_text;

/// A statement text small enough to inline, in the layout of fixture bank A.
#[cfg(test)]
const STUB_STATEMENT: &str = "Bank A AG -- Stub Statement\n\
                              Date     Description         Amount (EUR)\n\
                              03.01.2024 REWE MARKT 12     -39,99 EUR\n";

/// An importer that claims every file and then fails its extraction.
#[cfg(test)]
struct FailImporter;

#[cfg(test)]
impl Importer for FailImporter {
    fn name(&self) -> String {
        return "fail-importer".to_string();
    }
    fn probe(&self, _path: &Path) -> ProbeKind {
        return ProbeKind::Dialect("broken".to_string());
    }
    fn extract_transactions(&self, _path: &Path) -> Result<Extracted, ImportError> {
        return Err(ImportError::Failed("this importer always fails".to_string()));
    }
}

/// An in-memory importer with a fixed name, dialect and statement text: it
/// neither reads the file it is given nor touches the file system.
#[cfg(test)]
struct StubImporter {
    importer_name: &'static str,
    dialect: &'static str,
    text: &'static str,
}

#[cfg(test)]
impl Importer for StubImporter {
    fn name(&self) -> String {
        return self.importer_name.to_string();
    }
    fn probe(&self, _path: &Path) -> ProbeKind {
        return ProbeKind::Dialect(self.dialect.to_string());
    }
    fn extract_transactions(&self, path: &Path) -> Result<Extracted, ImportError> {
        let source = source_basename(&path.to_string_lossy());
        let report = parse_bank_a_text(self.text, &source);
        let mut warnings: Vec<String> = Vec::new();
        for warning in report.warnings {
            warnings.push(format!("line {}: {}", warning.line, warning.text));
        }
        return Ok(Extracted { transactions: report.transactions, warnings });
    }
}

/// An importer that declines every file but counts any extraction attempt, so
/// a test can assert that the host never asks a declining importer to extract.
#[cfg(test)]
struct DecliningImporter {
    extracts: std::rc::Rc<std::cell::Cell<usize>>,
}

#[cfg(test)]
impl Importer for DecliningImporter {
    fn name(&self) -> String {
        return "declining-importer".to_string();
    }
    fn probe(&self, _path: &Path) -> ProbeKind {
        return ProbeKind::Unsupported;
    }
    fn extract_transactions(&self, _path: &Path) -> Result<Extracted, ImportError> {
        self.extracts.set(self.extracts.get() + 1);
        return Ok(Extracted { transactions: Vec::new(), warnings: Vec::new() });
    }
}

/// Write one scratch file below a module-private scratch directory and return
/// its path.
#[cfg(test)]
fn scratch_file(sub: &str, name: &str, bytes: &[u8]) -> std::path::PathBuf {
    let dir = std::path::PathBuf::from(SCRATCH_PARENT).join(sub);
    match std::fs::create_dir_all(&dir) {
        Err(reason) => { panic!("cannot create the importer_host scratch directory: {reason}"); }
        Ok(_) => { }
    }
    let path = dir.join(name);
    match std::fs::write(&path, bytes) {
        Err(reason) => { panic!("cannot write the scratch file {}: {reason}", path.display()); }
        Ok(_) => { }
    }
    return path;
}

/// Remove one test's own scratch sub-directory, then try the (module-private)
/// scratch parent; the parent removal simply fails while another test of this
/// module still has a sub-directory in it.
#[cfg(test)]
fn remove_scratch(sub: &str) {
    let dir = std::path::PathBuf::from(SCRATCH_PARENT).join(sub);
    match std::fs::remove_dir_all(&dir) { Err(_) => { } Ok(_) => { } }
    match std::fs::remove_dir(std::path::PathBuf::from(SCRATCH_PARENT)) { Err(_) => { } Ok(_) => { } }
}

#[test]
fn bank_a_file_picked_by_bank_a_only() {
    let host = Host { importers: builtin_importers() };
    let path = find_testdata_dir().join("bank_a.pdf");
    let outcome = import_file(&host, &path);

    // Exactly the bank A built-in accepted it, with the bank A dialect.
    let accepted: Vec<(String, String)> = outcome.accepted_by.clone();
    assert!(accepted == [("built-in-bank-a".to_string(), "bank_a".to_string())], "accepted_by: {accepted:?}");
    for (name, _) in &outcome.accepted_by {
        assert!(name != NAME_BUILT_IN_BANK_B, "the bank B built-in must not claim a bank A file: {name}");
    }

    // The six rows of the fixture came out of the built-in, with provenance and
    // empty counterparty as the parsers of plan items 4.2/4.3 store them.
    assert!(outcome.failed.is_empty(), "no built-in may fail on the fixture: {:?}", outcome.failed);
    assert!(outcome.warnings.is_empty(), "the fixture is clean, warnings: {:?}", outcome.warnings);
    assert!(outcome.transactions.len() == 6, "expected six rows, got {:?}", outcome.transactions);
    let amounts: Vec<i64> = outcome.transactions.iter().map(|row| row.amount).collect();
    assert!(amounts == [-3999, -5990, 42000, -1847, -123456, -12990], "amounts: {amounts:?}");
    for row in &outcome.transactions {
        assert!(row.source == "bank_a.pdf", "source must be the file name: {:?}", row.source);
        assert!(row.counterparty == *"", "counterparty stays empty in Phase 4: {:?}", row);
    }
}

#[test]
fn bank_b_file_picked_by_bank_b_only() {
    // Each built-in accepts ONLY its own content marker: bank A's built-in
    // answers `Unsupported` for a bank B statement (decision 13).
    let host = Host { importers: builtin_importers() };
    let path = find_testdata_dir().join("bank_b1.pdf");
    let outcome = import_file(&host, &path);

    let accepted: Vec<(String, String)> = outcome.accepted_by.clone();
    assert!(accepted == [("built-in-bank-b".to_string(), "bank_b".to_string())], "accepted_by: {accepted:?}");
    assert!(outcome.failed.is_empty(), "no built-in may fail on the fixture: {:?}", outcome.failed);
    assert!(outcome.warnings.is_empty(), "the fixture is clean, warnings: {:?}", outcome.warnings);
    assert!(outcome.transactions.len() == 4, "expected four rows, got {:?}", outcome.transactions);
    let amounts: Vec<i64> = outcome.transactions.iter().map(|row| row.amount).collect();
    assert!(amounts == [-8990, -2500, 85000, -3456], "amounts: {amounts:?}");
    assert!(outcome.transactions[1].description == "TAXI AIRPORT; NIGHT");
}

#[test]
fn broken_importer_does_not_break_run() {
    // A supporter that always fails is recorded by name; the other supporter's
    // transactions survive regardless ("a failing importer must not break the
    // whole run", doc/architecture/plugin-protocol.md).
    let host = Host {
        importers: vec![
            Box::new(StubImporter { importer_name: "stub-importer", dialect: "stub", text: STUB_STATEMENT }),
            Box::new(FailImporter),
        ],
    };
    let path = std::path::Path::new("nothing-is-read-by-the-stubs.stub");
    let outcome = import_file(&host, &path);

    assert!(outcome.failed.len() == 1, "exactly one failure expected, got {:?}", outcome.failed);
    assert!(outcome.failed[0].0 == *"fail-importer", "the failure must name the importer: {:?}", outcome.failed[0]);
    assert!(outcome.failed[0].1 == ImportError::Failed("this importer always fails".to_string()));
    assert!(outcome.transactions.len() == 1, "the healthy importer's rows survive: {:?}", outcome.transactions);
    assert!(outcome.transactions[0].amount == -3999);
    let accepted: Vec<(String, String)> = outcome.accepted_by.clone();
    assert!(accepted == [
        ("stub-importer".to_string(), "stub".to_string()),
        ("fail-importer".to_string(), "broken".to_string()),
    ], "accepted_by: {accepted:?}");
}

#[test]
fn probe_false_never_extract() {
    // The probe pass decides who extracts; an importer that answers
    // `Unsupported` must not be asked, not even once.
    let extracts = std::rc::Rc::new(std::cell::Cell::<usize>::new(0));
    let host = Host {
        importers: vec![
            Box::new(DecliningImporter { extracts: extracts.clone() }),
            Box::new(StubImporter { importer_name: "stub-importer", dialect: "stub", text: STUB_STATEMENT }),
        ],
    };
    let path = std::path::Path::new("nothing-is-read-by-the-stubs.stub");
    let outcome = import_file(&host, &path);

    assert!(extracts.get() == 0, "a declining importer must never extract, attempts: {}", extracts.get());
    assert!(outcome.accepted_by.len() == 1, "only the stub accepts: {:?}", outcome.accepted_by);
    assert!(outcome.transactions.len() == 1);
}

#[test]
fn builtin_importers_order_is_stable() {
    let importers = builtin_importers();
    let names: Vec<String> = importers.iter().map(|importer| importer.name()).collect();
    // The mailbox importer of plan item 4.5 joins this list HERE, after the two
    // built-in statement readers; the order is what makes runs deterministic.
    assert!(names == [NAME_BUILT_IN_BANK_A.to_string(), NAME_BUILT_IN_BANK_B.to_string()], "names: {names:?}");
}

#[test]
fn builtin_probing_is_by_content_not_by_name() {
    // Decision 13: the marker decides, so a bank A statement renamed to a bank B
    // file name is still the bank A built-in's file.
    let bank_a = BuiltInBankA;
    let bank_b = BuiltInBankB;
    let path = find_testdata_dir().join("bank_a.pdf");
    assert!(bank_a.probe(&path) == ProbeKind::Dialect(DIALECT_BANK_A.to_string()));
    assert!(bank_b.probe(&path) == ProbeKind::Unsupported);

    let path = find_testdata_dir().join("bank_b2.pdf");
    assert!(bank_b.probe(&path) == ProbeKind::Dialect(DIALECT_BANK_B.to_string()));
    assert!(bank_a.probe(&path) == ProbeKind::Unsupported);
}

#[test]
fn a_non_pdf_file_is_accepted_by_nobody() {
    // A text file is `Unsupported` for every built-in without reading it, so the
    // outcome is empty: no transactions, no warnings, and above all no error.
    let path = scratch_file("non_pdf", "notes.txt", b"03.01.2024 REWE MARKT 12 -39,99 EUR\n");
    let host = Host { importers: builtin_importers() };
    let outcome = import_file(&host, &path);

    assert!(outcome.accepted_by.is_empty(), "a .txt file is nobody's: {:?}", outcome.accepted_by);
    assert!(outcome.transactions.is_empty());
    assert!(outcome.warnings.is_empty());
    assert!(outcome.failed.is_empty(), "no supporter means no failure: {:?}", outcome.failed);

    // The extension check is case-insensitive, so `.PDF` is the converter's
    // turn; these bytes are not a PDF, and that is a swallowed probe failure.
    let upper = scratch_file("non_pdf", "notes.PDF", b"Bank A AG -- not really a PDF\n");
    let outcome = import_file(&host, &upper);
    assert!(outcome.accepted_by.is_empty(), "junk bytes never support anything: {:?}", outcome.accepted_by);
    assert!(outcome.failed.is_empty());

    remove_scratch("non_pdf");
}

#[test]
fn probe_never_panics_on_a_junk_pdf() {
    // The review focus of the contract: probing junk must be a clean
    // `Unsupported`, even though it shells out to the converter.
    let bank_a = BuiltInBankA;
    let bank_b = BuiltInBankB;
    let path = scratch_file("junk_pdf", "junk.pdf", b"this is not a PDF at all\x00\x01\x02");
    assert!(bank_a.probe(&path) == ProbeKind::Unsupported);
    assert!(bank_b.probe(&path) == ProbeKind::Unsupported);

    // A file without any extension, and a file that does not exist at all, are
    // both answers of `Unsupported` rather than crashes.
    let missing = find_testdata_dir().join("absent-probe-target.pdf");
    assert!(bank_a.probe(&missing) == ProbeKind::Unsupported);
    let no_extension = scratch_file("junk_pdf", "noextension", b"whatever");
    assert!(bank_b.probe(&no_extension) == ProbeKind::Unsupported);

    remove_scratch("junk_pdf");
}

#[test]
fn two_supporters_extract_in_host_order() {
    // A file two importers claim gets extracted twice, in the order the host
    // lists them; merging duplicates is the store's problem (plan item 4.8 with
    // `technical::store_transactions`).
    let host = Host {
        importers: vec![
            Box::new(StubImporter { importer_name: "second-importer", dialect: "stub_b", text: STUB_STATEMENT }),
            Box::new(StubImporter { importer_name: "first-importer", dialect: "stub_a", text: STUB_STATEMENT }),
        ],
    };
    let path = std::path::Path::new("claimed-by-two.stub");
    let outcome = import_file(&host, &path);

    let accepted: Vec<String> = outcome.accepted_by.iter().map(|(name, _)| name.clone()).collect();
    assert!(accepted == ["second-importer", "first-importer"], "probe order is host order: {accepted:?}");
    assert!(outcome.transactions.len() == 2, "both extractions land: {:?}", outcome.transactions);
    let sources: Vec<String> = outcome.transactions.iter().map(|row| row.source.clone()).collect();
    assert!(sources == ["claimed-by-two.stub", "claimed-by-two.stub"], "sources: {sources:?}");
}

#[test]
fn warnings_of_the_extractors_are_merged_as_text() {
    // The parser's typed line warnings become the human-readable text a `log`
    // event of plan item 4.8 carries, in host order.
    let host = Host {
        importers: vec![
            Box::new(StubImporter {
                importer_name: "unrecognized-importer",
                dialect: "stub",
                text: "Some document without a marker\n03.01.2024 REWE MARKT 12 -39,99 EUR\n",
            }),
        ],
    };
    let outcome = import_file(&host, std::path::Path::new("unrecognized.stub"));

    assert!(outcome.transactions.len() == 1, "the row is still parsed: {:?}", outcome.transactions);
    assert!(outcome.warnings.len() == 1, "the missing marker is reported: {:?}", outcome.warnings);
    assert!(outcome.warnings[0].starts_with("line 0: "), "warning text: {:?}", outcome.warnings[0]);
}
