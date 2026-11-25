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
//! * **The plugin side of plan item 4.4b lives HERE as [`ExecImporter`].** A
//!   plugin is an external executable per `doc/architecture/plugin-protocol.md`
//!   (JSON-Lines on its stdin/stdout, discovered from the `plugins` key of
//!   `phin.toml` per user decision G3, [`exec_importers`] wiring
//!   `transport_cfg::plugins_from_config` into the same [`import_file`] loop).
//!
//! The built-ins here are the two statement readers of plan items 4.2/4.3 and
//! the mailbox entry of plan item 4.5 ([`BuiltInMbox]). The statement readers
//! probe by CONTENT MARKER because no filename convention exists anywhere in
//! phin for statements (planning decision 13); each one accepts only its own
//! marker, so a bank B statement is `Unsupported` as far as bank A is
//! concerned. The mailbox entry is the one built-in decision 13 identifies by
//! EXTENSION (`.mbox`, case-insensitive): mailbox content is arbitrary text no
//! content marker could single out — and it never yields transactions (R002,
//! see [`BuiltInMbox]).

use std::cell::RefCell;
use std::io::BufRead;
use std::io::Read;
use std::io::Write;
use std::path::Path;
use std::process::Child;
use std::process::ChildStderr;
use std::process::ChildStdin;
use std::process::ChildStdout;
use std::process::ExitStatus;
use std::process::Stdio;
use std::sync::mpsc::channel;
use std::sync::mpsc::Receiver;
use std::sync::mpsc::RecvTimeoutError;
use std::sync::mpsc::Sender;
use std::thread::JoinHandle;
use std::time::Duration;

use serde_json::Map;
use serde_json::Value;

use crate::domain::transaction::from_stored;
use crate::domain::transaction::Transaction;
use crate::technical::id::source_basename;
use crate::technical::id::transaction_base_id;
use crate::technical::parse_bank_a::parse_bank_a;
use crate::technical::parse_bank_a::ParseReport;
use crate::technical::parse_bank_b::parse_bank_b;
use crate::technical::pdf_text::extract_text;
use crate::technical::pdf_text::PdfTextError;
use crate::technical::transport_cfg::plugins_from_config;
use crate::technical::transport_cfg::ConfigIssue;

/// The `kind` a `probe` request of `doc/architecture/plugin-protocol.md`
/// carries for statement importers (`bank-statement`); an importer of another
/// kind (a mailbox, plan item 4.5) advertises a different one.
pub const KIND_BANK_STATEMENT: &str = "bank-statement";

/// Dialect name [`ProbeKind::Dialect`] carries for the layout of fixture bank A.
pub const DIALECT_BANK_A: &str = "bank_a";
/// Dialect name [`ProbeKind::Dialect`] carries for the layout of fixture bank B.
pub const DIALECT_BANK_B: &str = "bank_b";
/// Dialect name the mailbox importer [`BuiltInMbox`] carries (plan item 4.5);
/// named here so the set of dialect names lives in one place.
pub const DIALECT_MBOX: &str = "mbox";

/// The plugin timeout default of `doc/architecture/plugin-protocol.md` ("the
/// core kills a plugin after a configurable timeout (default 60 s)"), planning
/// decision 11; [`exec_importers`] hands it to every plugin discovered from
/// `phin.toml`, while a test [`ExecImporter`] carries 1 second instead.
pub const DEFAULT_PLUGIN_TIMEOUT_SECS: u64 = 60;

/// How long [`PluginChild::reap`] is willing to wait for the two helper threads
/// of a child that is already dead — the stdout reader thread and the stderr
/// drain — before giving up on them (detaching both). A plugin that hands its
/// stdout/stderr to a GRANDCHILD keeps those pipes open, so neither the reader
/// nor the drain ever sees EOF; the grace window is what keeps the reaping,
/// and with it the whole import, bounded. It is a courtesy window for a
/// well-behaved plugin (EOF on a closed pipe is immediate), not a timeout for
/// the plugin itself, which is [`DEFAULT_PLUGIN_TIMEOUT_SECS`] (and the
/// `ExecImporter`'s own `timeout_secs`).
const REAPER_GRACE_SECS: u64 = 2;

/// Name of the built-in bank A importer, i.e. the string an `error` event of
/// plan item 4.8 uses to name a failing importer.
const NAME_BUILT_IN_BANK_A: &str = "built-in-bank-a";
/// Name of the built-in bank B importer.
const NAME_BUILT_IN_BANK_B: &str = "built-in-bank-b";
/// Name of the built-in mailbox importer of plan item 4.5.
const NAME_BUILT_IN_MBOX: &str = "built-in-mbox";

/// The pinned warning [`BuiltInMbox`] answers every extraction with: mail is
/// an ANNOTATION of a transaction, never a transaction of its own (R002,
/// `doc/architecture/architecture.md` "Mail as annotation"). The annotation
/// side (plan item 4.7) is reached through [`crate::technical::mbox`] by plan
/// item 4.8 directly, NOT through this trait's transactions channel — which
/// is exactly why [`Importer::extract_transactions`], which can only hand out
/// transactions, stays empty for a mailbox. One constant for the warning the
/// entry returns and the tests assert on, so the two cannot drift.
const MBOX_NO_TRANSACTIONS: &str = "mail importers never yield transactions (R002)";

/// The pinned refusal of an `annotation` event on the PLUGIN wire (planning
/// decision 17): Phase 4 has no annotation-plugin story (R002 keeps mail
/// annotation to the built-ins, Phase 7 owns the plugin side), so receiving
/// `{"event":"annotation"}` from a plugin importer is the typed failure the
/// task pins, NOT a translation into `Extracted`. One constant for the refusal
/// the host returns and the test asserts on, so the two cannot drift.
const ANNOTATION_REFUSAL: &str = "annotation events not supported for plugin importers yet (Phase 7)";

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
    /// failure, a timeout, or anything else a `log`/`error` event phrases as
    /// text. (Amendment A4 moved the missing-converter case out of this
    /// variant and onto [`ImportError::ConverterMissing`], which is typed.)
    Failed(String),
    /// The PDF text converter (`pdftotext`, `doc/architecture/architecture.md`)
    /// is not installed. Typed on purpose (amendment A4): plan item 4.8 maps
    /// this variant — NEVER a message lookup — onto the `converter-missing`
    /// error code of `doc/architecture/core-protocol.md`.
    ConverterMissing,
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

/// The built-in mailbox entry of plan item 4.5: it recognizes a mailbox by its
/// `.mbox` extension (case-insensitive) — the one built-in probing by
/// EXTENSION, because mailbox content is arbitrary text no content marker
/// could single out (planning decision 13).
///
/// It NEVER yields transactions: R002 (`doc/architecture/architecture.md`
/// "Mail as annotation") pins that mail becomes an annotation OF a
/// transaction and must never create one, so [`Importer::extract_transactions`]
/// answers the empty transactions channel with [`MBOX_NO_TRANSACTIONS`] — the
/// rule holds even THROUGH the host path, not only in the annotation code.
/// The message reader itself is [`crate::technical::mbox`]; plan item 4.7
/// turns its messages into annotations and plan item 4.8 wires that up, never
/// this trait's transactions channel.
pub struct BuiltInMbox;

impl Importer for BuiltInMbox {
    fn name(&self) -> String {
        return NAME_BUILT_IN_MBOX.to_string();
    }

    fn probe(&self, path: &Path) -> ProbeKind {
        // Extension-only, never reading the file: the probe must be cheap and
        // cannot fail, and no CONTENT of an mbox file is a reliable marker.
        return if extension_is(path, "mbox") {
            ProbeKind::Dialect(DIALECT_MBOX.to_string())
        } else {
            ProbeKind::Unsupported
        };
    }

    fn extract_transactions(&self, _path: &Path) -> Result<Extracted, ImportError> {
        // R002, pinned: a mailbox has no transactions to extract, and the one
        // warning is what plan item 4.8 can log about WHY nothing came.
        return Ok(Extracted { transactions: Vec::new(), warnings: vec![MBOX_NO_TRANSACTIONS.to_string()] });
    }
}

/// The host: the importers a run consults, in the order they are consulted.
///
/// A built-in host uses [`builtin_importers`] (the two statement readers and
/// the mailbox entry); plan item 4.4b appends the plugin importers it
/// discovers from `phin.toml`.
pub struct Host {
    /// The configured importers, consulted in this order (probe pass first,
    /// extract pass second — order preserved in both).
    pub importers: Vec<Box<dyn Importer>>,
}

/// The built-in importers, in a fixed, deterministic order: bank A first, then
/// bank B, then the mailbox entry of plan item 4.5 ([`BuiltInMbox`]). The
/// plugin importers of plan item 4.4b are appended by the host that reads the
/// configuration.
pub fn builtin_importers() -> Vec<Box<dyn Importer>> {
    let mut importers: Vec<Box<dyn Importer>> = Vec::new();
    importers.push(Box::new(BuiltInBankA));
    importers.push(Box::new(BuiltInBankB));
    importers.push(Box::new(BuiltInMbox));
    return importers;
}

// ---------------------------------------------------------------------------
// Plan item 4.4b: plugin importers as external processes
// ---------------------------------------------------------------------------

/// A plugin importer of plan item 4.4b: an external executable spoken to over
/// the JSON-Lines framing of `doc/architecture/plugin-protocol.md`, discovered
/// from the `plugins` key of `phin.toml` (user decision G3,
/// [`exec_importers`]).
///
/// ONE child process per file-run (planning decision 15): [`Importer::probe`]
/// spawns it, writes the `probe` request, and keeps its stdin open; the
/// supporter's [`Importer::extract_transactions`] writes the `extract`
/// request to the SAME child and reads its lines until the `{"event":"end"}`
/// terminator; every exit path kills and reaps it, and the child's own
/// `Drop` (`PluginChild`) is the backstop for a child no explicit path reaped.
/// A `probe:false` answer means the plugin never receives an `extract` request.
///
/// Every read from the child is a `recv_timeout` over the reader thread's
/// channel, bounded by [`ExecImporter::timeout_secs`] (planning decision 11,
/// default [`DEFAULT_PLUGIN_TIMEOUT_SECS`]); on expiry the child is killed,
/// reaped, and the run reports a `timed out` failure. There is no infinite
/// wait anywhere.
///
/// Decision 16 (`plugin-protocol.md:39-41`): the CORE computes ids — a
/// transaction line carrying an `id` is accepted and its `id` DISCARDED.
/// `plugin-protocol.md` fixes that `amount` on the wire is an INTEGER of
/// minor units, so the wire rows go through
/// [`crate::domain::transaction::from_stored`] (the store-side constructor
/// that takes the count as-is), deliberately NOT `from_parts`, which would
/// re-scale the minor-unit count (its own INVARIANT).
///
/// A transaction line's `date` must already be the canonical `YYYY-MM-DD`
/// form the wire example of `plugin-protocol.md` prints; a malformed or
/// unusable line is the typed `Failed("invalid line from <name>")` of
/// `plugin-protocol.md:20-23` ("answers with an invalid line"), and an
/// `annotation` event is the typed refusal of planning decision 17 (no
/// Phase-4 story for annotation plugins exists; Phase 7 owns that).
pub struct ExecImporter {
    /// The command line that STARTS the plugin: `argv[0]` is the executable
    /// and the rest are its arguments (the fake plugin tests run
    /// `python3 <script>` this way).
    pub argv: Vec<String>,
    /// The configured path string naming this plugin (`phin.toml`'s
    /// `plugins` entry, e.g. `"./importers/bank-x"`); the `name` a probe
    /// ANSWER carries is only logged, never the identity (task T5 pins it).
    pub name: String,
    /// The bounded-read window, in seconds, for EVERY read of the child's
    /// output; the configured value, [`DEFAULT_PLUGIN_TIMEOUT_SECS`] for
    /// plugins discovered from the configuration (planning decision 11).
    pub timeout_secs: u64,
    /// The child of the CURRENT file-run, empty between runs (planning
    /// decision 15). The `RefCell` only because the [`Importer`] contract
    /// takes `&self` — single-threaded like the rest of the host.
    child: RefCell<Option<PluginChild>>,
}

impl ExecImporter {
    /// Build one plugin importer from its three configured parts; the private
    /// child cell starts empty and lives only during a file-run.
    pub fn new(argv: Vec<String>, name: String, timeout_secs: u64) -> ExecImporter {
        return ExecImporter { argv, name, timeout_secs, child: RefCell::new(None) };
    }

    /// Start the plugin process: `argv` as configured with stdin, stdout and
    /// stderr piped by the core (`plugin-protocol.md:17` — the core writes
    /// requests to stdin and reads responses from stdout; stderr is captured
    /// so the plugin's own explanation can name the typed failure).
    fn spawn_child(&self) -> Result<PluginChild, String> {
        let program = match self.argv.first() {
            None => { return Err("ExecImporter was configured with an empty argv".to_string()); }
            Some(program) => { program }
        };
        let mut command = std::process::Command::new(program);
        for argument in self.argv.iter().skip(1) {
            command.arg(argument);
        }
        command.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
        let mut child = match command.spawn() {
            Err(reason) => { return Err(reason.to_string()); }
            Ok(child) => { child }
        };
        let stdin = match child.stdin.take() {
            None => { return Err("the plugin child has no stdin pipe".to_string()); }
            Some(stdin) => { stdin }
        };
        let stdout = match child.stdout.take() {
            None => { return Err("the plugin child has no stdout pipe".to_string()); }
            Some(stdout) => { stdout }
        };
        let stderr = child.stderr.take();
        let (lines, reader, reader_done) = spawn_line_reader(stdout);
        return Ok(PluginChild { child, stdin, lines, reader: Some(reader), reader_done, stderr, reported_kind: String::new() });
    }
}

impl Importer for ExecImporter {
    fn name(&self) -> String {
        return self.name.clone();
    }

    fn probe(&self, path: &Path) -> ProbeKind {
        // Decision 15: at most one child per file-run lives here. A leftover
        // from an earlier file (a supporter this instance was never asked to
        // extract from) is reaped BEFORE the next probe, so one probe never
        // holds two children and no file is spawned twice.
        match self.child.borrow_mut().take() {
            None => { }
            Some(stale) => { let _ = stale.reap(); }
        }
        let mut process = match self.spawn_child() {
            // The `Importer` contract forbids a failing probe: a plugin that
            // cannot even be started DECLINES, its spawn failure surfacing
            // nowhere (the trait's own rule; the run keeps its other
            // importers).
            Err(_reason) => { return ProbeKind::Unsupported; }
            Ok(process) => { process }
        };
        let request = plugin_request_line("probe", path, KIND_BANK_STATEMENT);
        match process.write_request(&request) {
            Err(_) => {
                let _ = process.reap();
                return ProbeKind::Unsupported;
            }
            Ok(_) => { }
        }
        match process.read_line(self.timeout_secs) {
            LineRead::Line(text) => {
                match decode_probe_answer(&text) {
                    ProbeAnswer::Supported { kind } => {
                        // The probe response's optional `kind` names the
                        // dialect; an unnamed one falls back to the
                        // configured identity. The CHILD is told that SAME
                        // resolved dialect: `reported_kind` must never stay
                        // empty, or the `extract` request would go out with
                        // `"kind":""` while the host logged a different one.
                        let dialect = if kind.len() > 0 { kind } else { self.name.clone() };
                        process.reported_kind = dialect.clone();
                        *self.child.borrow_mut() = Some(process);
                        return ProbeKind::Dialect(dialect);
                    }
                    // `supported:false`, an `error` line the plugin declines
                    // with, and an undecodable answer all read as
                    // `Unsupported`: the plugin must then NEVER see an
                    // `extract` request (the host only extracts from
                    // supporters, and this instance reaped its child here).
                    ProbeAnswer::NotSupported | ProbeAnswer::Declined | ProbeAnswer::Invalid => {
                        let _ = process.reap();
                        return ProbeKind::Unsupported;
                    }
                }
            }
            // A timed-out, closed, or unreadable probe answer declines — the
            // probe cannot fail — and the child dies on the spot, bounded.
            LineRead::Timeout | LineRead::Closed | LineRead::IoFailure(_) => {
                let _ = process.reap();
                return ProbeKind::Unsupported;
            }
        }
    }

    fn extract_transactions(&self, path: &Path) -> Result<Extracted, ImportError> {
        // Decision 15: the SAME child that probed this file answers the
        // extract; there is no second spawn for the same file-run, and an
        // extract without a just-probed child has no process to speak to.
        let mut process = match self.child.borrow_mut().take() {
            None => {
                return Err(ImportError::Failed(format!("plugin importer {} was asked to extract without a supporting probe for this file", self.name)));
            }
            Some(process) => { process }
        };
        // `plugin-protocol.md`: the `kind` of the extract request is the one
        // the plugin reported in its probe answer.
        let request = plugin_request_line("extract", path, &process.reported_kind);
        match process.write_request(&request) {
            Err(reason) => {
                let died = process.reap();
                return Err(plugin_child_failure(&self.name, &format!("its input was closed before the extract request ({reason})"), died));
            }
            Ok(_) => { }
        }
        let mut transactions: Vec<Transaction> = Vec::new();
        loop {
            let line = match process.read_line(self.timeout_secs) {
                LineRead::Line(line) => { line }
                LineRead::Timeout => {
                    // Planning decision 11: kill, reap, and name the window.
                    // `plugin-protocol.md:24-25`: "kills a plugin after a
                    // configurable timeout ... reports it as `plugin-failed`".
                    let _ = process.reap();
                    return Err(ImportError::Failed(format!("plugin {} timed out after {} s", self.name, self.timeout_secs.to_string())));
                }
                // `plugin-protocol.md:20-23`: a plugin that crashes or leaves
                // its stdout early is reported NAMING the plugin.
                LineRead::Closed => {
                    let died = process.reap();
                    return Err(plugin_child_failure(&self.name, "closed its output before the end terminator", died));
                }
                LineRead::IoFailure(reason) => {
                    let died = process.reap();
                    return Err(plugin_child_failure(&self.name, &format!("its output could not be read ({reason})"), died));
                }
            };
            let held = match parse_plugin_line(&line) {
                None => {
                    let _ = process.reap();
                    return Err(ImportError::Failed(format!("invalid line from {}", self.name)));
                }
                Some(held) => { held }
            };
            match event_text(&held) {
                Some("end") => { break; }
                Some("annotation") => {
                    // Planning decision 17: an annotation object on the
                    // plugin wire has no Phase-4 story (R002 keeps mail
                    // annotation to the built-ins); the refusal is typed.
                    let _ = process.reap();
                    return Err(ImportError::Failed(ANNOTATION_REFUSAL.to_string()));
                }
                Some("transactions") => {
                    let built = match transaction_of_plugin_line(&held) {
                        None => {
                            let _ = process.reap();
                            return Err(ImportError::Failed(format!("invalid line from {}", self.name)));
                        }
                        Some(built) => { built }
                    };
                    transactions.push(built);
                }
                Some("error") => {
                    // The versioning section's error line: the event names
                    // the plugin; the core re-names it with the CONFIGURED
                    // name so plan item 4.8 can point at the right plugin.
                    let reported = match held.get("text").and_then(Value::as_str) {
                        None => { format!("error line: [{line}]") }
                        Some(text) => { text.to_string() }
                    };
                    let _ = process.reap();
                    return Err(ImportError::Failed(format!("plugin {} reported: {reported}", self.name)));
                }
                _ => {
                    let _ = process.reap();
                    return Err(ImportError::Failed(format!("invalid line from {}", self.name)));
                }
            }
        }
        // The file-run ends at its terminator: decision 15 gives the NEXT
        // file a fresh child, so this one is killed and reaped HERE.
        let _ = process.reap();
        return Ok(Extracted { transactions, warnings: Vec::new() });
    }
}

/// The live child process of one [`ExecImporter`] file-run (planning decision
/// 15): spawned by [`Importer::probe`], spoken to again by
/// [`Importer::extract_transactions`], killed and reaped at the end of that
/// run — by the happy path, by every failure path, and by [`Drop`].
struct PluginChild {
    /// The OS child; [`PluginChild::reap`] kills it (a no-op when it already
    /// exited) and waits it, so no zombie and no plugin outlives the run.
    child: Child,
    /// The core→plugin request pipe, open from the probe until the child is
    /// gone (`plugin-protocol.md:17`).
    stdin: ChildStdin,
    /// The plugin's stdout as complete lines over a channel, pushed by the
    /// detached reader thread ([`spawn_line_reader`]); every read of it is
    /// bounded by the `timeout_secs` window.
    lines: Receiver<Result<String, std::io::Error>>,
    /// The reader thread, `take`n by `reap` to join AFTER the child died (the
    /// closing write side ends its read then) — and ONLY when the completion
    /// channel below answers, because that join is otherwise unbounded.
    reader: Option<JoinHandle<()>>,
    /// The reader thread's completion signal: it sends one value on EVERY exit
    /// path ([`spawn_line_reader`]), and `reap` waits for it no longer than
    /// [`REAPER_GRACE_SECS`]. No answer means the plugin kept the write side
    /// open through a grandchild: the reader is then detached, never joined.
    reader_done: Receiver<()>,
    /// The `kind` the CORE will send on the extract request: the `kind` this
    /// plugin reported in its `probe` answer, or — when it named none — the
    /// resolved dialect, never an empty string (`plugin-protocol.md`: "`kind`
    /// on the extract request is fixed here, taken from the `kind` the plugin
    /// reported in its `probe` response"; the host's own fallback is part of
    /// that decision, so the child must see it too).
    reported_kind: String,
    /// The plugin's stderr, drained AFTER the child is reaped so the plugin's
    /// own explanation can ride the typed failure — drained only once the
    /// write side is certainly closed, never while the child runs, and even
    /// then on a throw-away thread bounded by [`REAPER_GRACE_SECS`]
    /// ([`drain_stderr_bounded`]).
    stderr: Option<ChildStderr>,
}

impl PluginChild {
    /// Write one request line to the plugin's stdin and flush it so the line
    /// lands NOW (JSON-Lines: one object per line).
    fn write_request(&mut self, text: &str) -> Result<(), String> {
        // Edition 2024: the pipes are written THROUGH a borrow (`Write for
        // &mut ChildStdin`), not by value.
        let sink: &mut ChildStdin = &mut self.stdin;
        let wrote = match sink.write_all(text.as_bytes()) {
            Err(reason) => { Err(reason) }
            Ok(_) => {
                match sink.write_all(b"\n") {
                    Err(reason) => { Err(reason) }
                    Ok(_) => { sink.flush() }
                }
            }
        };
        match wrote {
            Err(reason) => { return Err(reason.to_string()); }
            Ok(_) => { return Ok(()); }
        }
    }

    /// Read exactly ONE plugin line, bounded by `timeout_secs` seconds: the
    /// reader thread decouples the blocking pipe read from this call and
    /// `Receiver::recv_timeout` is the only wait here (the review focus "no
    /// infinite waits anywhere").
    fn read_line(&self, timeout_secs: u64) -> LineRead {
        match self.lines.recv_timeout(Duration::from_secs(timeout_secs)) {
            Ok(Ok(line)) => { return LineRead::Line(line); }
            Ok(Err(reason)) => { return LineRead::IoFailure(reason.to_string()); }
            Err(RecvTimeoutError::Timeout) => { return LineRead::Timeout; }
            Err(RecvTimeoutError::Disconnected) => { return LineRead::Closed; }
        }
    }

    /// Kill the child (a no-op when it exited on its own), reap it, join the
    /// reader thread — bounded by [`REAPER_GRACE_SECS`] — and drain stderr on
    /// a bounded drain thread: the ONE exit path of every child, and the only
    /// place stderr is read, which is why it cannot deadlock on a running
    /// child.
    ///
    /// EVERY wait here is bounded, deliberately: a plugin that hands its
    /// stdout (or stderr) to a grandchild keeps the pipe's write side open
    /// even after its own death, so the reader thread would never see EOF and
    /// an unbounded `JoinHandle::join` would hang the import — and through it
    /// the whole run — forever. The reader therefore announces its own
    /// completion ([`spawn_line_reader`]) and `reap` waits for that signal at
    /// most [`REAPER_GRACE_SECS`]: on an answer the join is immediate, on a
    /// missing one the thread is DETACHED (the `JoinHandle` simply dropped)
    /// and the still-locked pipe is left to the process' lifetime. Same for
    /// stderr ([`drain_stderr_bounded`]). A lost tail of the plugin's own
    /// output is a worse outcome than a stalled import, and the typed failure
    /// already names the plugin and its exit status.
    fn reap(mut self) -> ReapedChild {
        let _ = self.child.kill();
        let status = match self.child.wait() {
            Err(_) => { None }
            Ok(status) => { Some(status) }
        };
        let reader_finished = match self.reader_done.recv_timeout(Duration::from_secs(REAPER_GRACE_SECS)) {
            // No completion signal: the reader is still blocked in a read the
            // grandchild's open pipe never ends. Detach it.
            Err(RecvTimeoutError::Timeout) => { false }
            // `Ok(())` = the reader ended; a DISCONNECTED channel means its
            // sender went out of scope with the thread (a panicking reader),
            // so joining it cannot block either.
            Ok(_) | Err(RecvTimeoutError::Disconnected) => { true }
        };
        if reader_finished {
            match self.reader.take() {
                None => { }
                Some(reader) => {
                    match reader.join() {
                        Ok(_) => { }
                        Err(_) => { }
                    }
                }
            }
        }
        let stderr_text = match self.stderr.take() {
            None => { String::new() }
            Some(pipe) => { drain_stderr_bounded(pipe) }
        };
        return ReapedChild { stderr_text: stderr_text.trim().to_string(), status };
    }
}

impl Drop for PluginChild {
    fn drop(&mut self) {
        // Decision 15's backstop: any path that ABANDONS the file-run without
        // an explicit `reap` still ends the child — kill, then wait so the
        // process is reaped, never a zombie and never a plugin outliving the
        // `ExecImporter`.
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Where one bounded [`PluginChild::read_line`] came out.
enum LineRead {
    /// One complete line (the trailing newline stripped).
    Line(String),
    /// The `timeout_secs` window expired before a line landed.
    Timeout,
    /// The plugin closed its stdout (or exited): the channel's sender is
    /// gone — for a plugin, "closed its output" (`plugin-protocol.md:20-23`).
    Closed,
    /// The pipe itself failed; the text is the OS's own message.
    IoFailure(String),
}

/// What reaping a child collected: the exit status when the OS had one, and
/// the plugin's own stderr text (empty when it said nothing).
struct ReapedChild {
    stderr_text: String,
    status: Option<ExitStatus>,
}

/// Turn the blocking stdout pipe into a channel of complete lines: the
/// bounded-read mechanism of the review focus — the core NEVER blocks on the
/// pipe, it asks this channel with a deadline. The thread ends with the
/// child: the closed write side makes the read return 0 and the thread
/// finish, which turns the core's next read into [`LineRead::Closed`].
///
/// The third value it hands back is the reader's COMPLETION channel: the
/// thread sends one value on every exit path, which is what lets
/// [`PluginChild::reap`] bound even the join of a child whose stdout a
/// grandchild keeps open (otherwise the join below would be the last
/// unbounded wait of the run).
fn spawn_line_reader(stdout: ChildStdout) -> (Receiver<Result<String, std::io::Error>>, JoinHandle<()>, Receiver<()>) {
    let (sender, receiver) = channel::<Result<String, std::io::Error>>();
    let (done_sender, done) = channel::<()>();
    let reader = std::thread::spawn(move || {
        read_plugin_lines(stdout, sender);
        // Reaching this line means the read ended for any reason (EOF, a pipe
        // error, or the core stopping listening), so `reap` may join without
        // waiting further.
        let _signal = done_sender.send(());
    });
    return (receiver, reader, done);
}

/// Pump one plugin stdout pipe into the line channel until it ends: the body
/// of the [`spawn_line_reader`] thread, split out so that the thread's every
/// exit path — including a returning pump — passes the completion signal.
fn read_plugin_lines(stdout: ChildStdout, sender: Sender<Result<String, std::io::Error>>) {
    // Edition 2024: `Read for &mut ChildStdout` — the BufReader reads the
    // stdout handle through a mutable borrow, the handle itself owned
    // inside this thread.
    let mut handle = stdout;
    let mut pipe = std::io::BufReader::new(&mut handle);
    let mut line = String::new();
    loop {
        line.clear();
        match pipe.read_line(&mut line) {
            Ok(0) => { return; }
            Ok(_) => {
                let cut = match line.strip_suffix('\n') {
                    None => { line.as_str() }
                    Some(cut) => { cut }
                };
                if sender.send(Ok(cut.to_string())).is_err() {
                    // The core stopped listening; end the run's plugin.
                    return;
                }
            }
            Err(reason) => {
                let _ = sender.send(Err(reason));
                return;
            }
        }
    }
}

/// Collect the plugin's stderr, bounded like everything else around a child:
/// the blocking `read_to_string` runs on a THROW-AWAY thread whose result this
/// waits for at most [`REAPER_GRACE_SECS`]. A plugin that handed its stderr to
/// a grandchild keeps that pipe open past its own exit, and reading it here
/// directly would be an unbounded wait of the import. On expiry the drain is
/// detached and the plugin simply says nothing on the typed failure — which
/// still names the plugin and its exit status.
fn drain_stderr_bounded(stderr: ChildStderr) -> String {
    let (sender, receiver) = channel::<String>();
    let drainer = std::thread::spawn(move || {
        let mut held = stderr;
        let mut text = String::new();
        match held.read_to_string(&mut text) {
            Ok(_) => { }
            Err(_) => { }
        }
        let _result = sender.send(text);
    });
    match receiver.recv_timeout(Duration::from_secs(REAPER_GRACE_SECS)) {
        Ok(text) => {
            // The drain is over, so collecting its thread costs nothing.
            match drainer.join() {
                Ok(_) => { }
                Err(_) => { }
            }
            return text;
        }
        // Timeout: drop the `JoinHandle`, which detaches the still-blocked
        // drain. Disconnected: the drain thread went away without answering.
        Err(_) => { return String::new(); }
    }
}

/// The one request object of the framing as one JSON line,
/// `{"v":1,"cmd":"probe"|"extract","file":...,"kind":...}`
/// (`plugin-protocol.md`, the `probe` and `extract` requests). `serde_json`
/// is used directly — the plugin framing is simpler JSON-Lines, not the
/// core protocol's codec (task T5 pins that).
fn plugin_request_line(cmd: &str, path: &Path, kind: &str) -> String {
    let mut fields = Map::new();
    fields.insert("v".to_string(), Value::from(1));
    fields.insert("cmd".to_string(), Value::from(cmd));
    fields.insert("file".to_string(), Value::from(path.to_string_lossy().to_string()));
    fields.insert("kind".to_string(), Value::from(kind));
    match serde_json::to_string(&Value::Object(fields)) {
        Err(_) => { return String::new(); }
        Ok(text) => { return text; }
    }

}

/// Decode one plugin stdout line to a JSON OBJECT, [`None`] when it is not
/// JSON, not an object, or does not carry the version this build speaks —
/// `plugin-protocol.md` ("versioning"): every line carries an integer `v`,
/// and an absent `v` means `1`. The caller turns a [`None`] into the
/// `Failed("invalid line from <name>")` of `plugin-protocol.md:20-23`.
fn parse_plugin_line(text: &str) -> Option<Value> {
    let held = match serde_json::from_str::<Value>(text) {
        Err(_) => { return None; }
        Ok(held) => { held }
    };
    if !held.is_object() {
        return None;
    }
    match held.get("v") {
        None => { }
        Some(version) => {
            if version.as_i64() != Some(1) {
                return None;
            }
        }
    }
    return Some(held);
}

/// The `event` text of a decoded plugin line, [`None`] when there is none.
fn event_text(held: &Value) -> Option<&str> {
    return match held.get("event") {
        None => { None }
        Some(value) => { value.as_str() }
    };
}

/// What a `probe` answer line of the plugin said.
enum ProbeAnswer {
    /// `supported: true`; the `kind` text names the dialect (empty when the
    /// plugin named none — the field is optional on the wire).
    Supported { kind: String },
    /// `supported: false`.
    NotSupported,
    /// An `error` line: the plugin declines the probe on purpose.
    Declined,
    /// Not a usable `probe` answer at all.
    Invalid,
}

fn decode_probe_answer(text: &str) -> ProbeAnswer {
    let held = match parse_plugin_line(text) {
        None => { return ProbeAnswer::Invalid; }
        Some(held) => { held }
    };
    match event_text(&held) {
        Some("error") => { return ProbeAnswer::Declined; }
        Some("probe") => { }
        _ => { return ProbeAnswer::Invalid; }
    }
    match held.get("supported") {
        None => { return ProbeAnswer::Invalid; }
        Some(answer) => {
            match answer.as_bool() {
                None => { return ProbeAnswer::Invalid; }
                Some(true) => { }
                Some(false) => { return ProbeAnswer::NotSupported; }
            }
        }
    }
    match held.get("kind") {
        None => { return ProbeAnswer::Supported { kind: String::new() }; }
        Some(value) => {
            match value.as_str() {
                None => { return ProbeAnswer::Invalid; }
                Some(kind) => { return ProbeAnswer::Supported { kind: kind.to_string() }; }
            }
        }
    }
}

/// Build one transaction from a decoded `transactions` line, the wire object
/// of `plugin-protocol.md` ("Transactions on the wire"): `date` the canonical
/// `YYYY-MM-DD` text, `amount` an INTEGER of minor units, the text fields
/// carried through, and `counterparty` optional (the domain's "empty when
/// unknown"). Deliberately [`from_stored`] and NOT
/// [`crate::domain::transaction::from_parts`]: the wire's integer is already
/// a minor-unit count, and `from_parts` would re-scale it (its own INVARIANT:
/// `money::parse("-3999")` yields `-399900`).
///
/// Decision 16 (`plugin-protocol.md:39-41`, "plugins need not send `id`: the
/// core computes the content-hash id"): any `id` on the line is DISCARDED and
/// the id set is [`transaction_base_id`] of the built record — the occurrence
/// suffix stays the store's job (plan item 4.8).
fn transaction_of_plugin_line(held: &Value) -> Option::<Transaction> {
    let date = match held.get("date") {
        None => { return None; }
        Some(value) => { match value.as_str() { None => { return None; } Some(text) => { text } } }
    };
    let amount = match held.get("amount") {
        // An INTEGER of minor units on the wire; a float or a string is an
        // invalid line, never a guess.
        None => { return None; }
        Some(value) => { match value.as_i64() { None => { return None; } Some(count) => { count } } }
    };
    let currency = match held.get("currency") {
        None => { return None; }
        Some(value) => { match value.as_str() { None => { return None; } Some(text) => { text } } }
    };
    let description = match held.get("description") {
        None => { return None; }
        Some(value) => { match value.as_str() { None => { return None; } Some(text) => { text } } }
    };
    let source = match held.get("source") {
        None => { return None; }
        Some(value) => { match value.as_str() { None => { return None; } Some(text) => { text } } }
    };
    let counterparty = match held.get("counterparty") {
        None => { "" }
        Some(value) => { match value.as_str() { None => { return None; } Some(text) => { text } } }
    };
    let mut built = match from_stored("", date, amount, currency, description, counterparty, source) {
        Err(_) => { return None; }
        Ok(built) => { built }
    };
    built.id = transaction_base_id(&built);
    return Some(built);
}

/// The typed `Failed` for a plugin child that ended badly: it NAMES the
/// configured plugin (`plugin-protocol.md:20-23`, an `error` event "naming
/// the plugin"), states what the core observed, and carries the exit status
/// and the plugin's own stderr when the reaping collected them.
fn plugin_child_failure(name: &str, what: &str, died: ReapedChild) -> ImportError {
    let mut text = format!("plugin {name} {what}");
    if died.stderr_text.len() > 0 {
        text.push_str("; the plugin wrote on stderr: ");
        text.push_str(&died.stderr_text);
    }
    match died.status {
        None => { }
        Some(status) => {
            text.push_str("; exit status: ");
            text.push_str(&status.to_string());
        }
    }
    return ImportError::Failed(text);
}

/// The plugin importers a run consults, discovered from the `plugins` key of
/// `<data dir>/phin.toml` (plan item 4.4b, user decision G3); append them to
/// [`builtin_importers`] to get the full configured host. The host of plan
/// item 4.8 assembles that; the two lists keep their order.
///
/// * `plugins = ["./importers/bank-x"]` — one entry per plugin executable
///   (`data-files.md`), RELATIVE paths resolved against the DATA DIRECTORY
///   (`plugin-protocol.md:11` discovers plugins "from the configuration";
///   the data directory is the anchor `data-files.md` fixes for every
///   data-set file), absolute paths kept as given.
/// * The importer NAME is the CONFIGURED path string (task T5 pins it; the
///   `name` a probe response carries is logging only).
/// * A malformed `plugins` key is the typed [`ConfigIssue::PluginsNotStrings`]
///   of [`plugins_from_config`], handed out unchanged with an empty importer
///   list — no plugin is started on a half-read list, and a missing key or
///   file is simply the plugin-less run of `plugin-protocol.md:11`.
/// * A configured entry whose executable does NOT exist is the typed
///   [`ConfigIssue::PluginNotFound`] naming the configured path, and no
///   importer is built for it. `plugin-protocol.md:13` requires a plugin to be
///   executable; reporting it at discovery is the only way it becomes visible
///   at all, since the [`Importer`] probe contract forbids a failing probe — an
///   unspawnable plugin would otherwise silently decline every file forever.
///   Entries that DO exist stay importers even when they cannot be started, so
///   one broken path never costs the run the other plugins.
/// * The timeout is [`DEFAULT_PLUGIN_TIMEOUT_SECS`] (planning decision 11).
pub fn exec_importers(data_dir: &Path) -> (Vec::<Box<dyn Importer>>, Vec::<ConfigIssue>) {
    let (paths, issues) = plugins_from_config(data_dir);
    if issues.len() > 0 {
        return (Vec::<Box<dyn Importer>>::new(), issues);
    }
    let mut importers: Vec::<Box<dyn Importer>> = Vec::new();
    let mut complaints: Vec::<ConfigIssue> = Vec::new();
    for configured in paths {
        // Existence of the resolved executable is checked HERE, at discovery
        // time: a typo in `plugins` must show up as a typed complaint, not as a
        // plugin that quietly supports nothing.
        let program = plugin_program_path(&configured, data_dir);
        match std::fs::metadata(&program) {
            Err(_reason) => {
                complaints.push(ConfigIssue::PluginNotFound(configured.clone()));
            }
            Ok(_seen) => {
                importers.push(Box::new(exec_importer_for(&configured, data_dir)));
            }
        }
    }
    return (importers, complaints);
}

/// The [`ExecImporter`] one `plugins` entry of `phin.toml` describes: argv and
/// timeout wired exactly as a run needs them (the split of the resolution and
/// the wiring from the discovery loop of [`exec_importers`] exists so a test
/// can assert THIS, not just the constant next to it).
///
/// * `argv` is ONE element, the resolved executable path
///   ([`plugin_program_path`]); a plugin taking arguments is configured as a
///   shell wrapper, per `data-files.md`.
/// * `name` is the CONFIGURED path string verbatim (task T5 pins it).
/// * `timeout_secs` is [`DEFAULT_PLUGIN_TIMEOUT_SECS`] (planning decision 11,
///   `plugin-protocol.md:24-25`).
fn exec_importer_for(configured: &str, data_dir: &Path) -> ExecImporter {
    return ExecImporter::new(
        vec![plugin_program_path(configured, data_dir)],
        configured.to_string(),
        DEFAULT_PLUGIN_TIMEOUT_SECS,
    );
}

/// The executable path a `plugins` entry names: a RELATIVE entry resolves
/// against the DATA DIRECTORY (`data-files.md`, the anchor of every data-set
/// file), an absolute one is kept as given. This is `argv[0]` of the plugin
/// child and the path [`exec_importers`] checks for existence.
fn plugin_program_path(configured: &str, data_dir: &Path) -> String {
    return if Path::new(configured).is_absolute() {
        configured.to_string()
    } else {
        data_dir.join(configured).to_string_lossy().to_string()
    };
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
/// A missing converter is [`ImportError::ConverterMissing`] — typed, not a
/// string (amendment A4), because plan item 4.8 must emit the
/// `converter-missing` error code of `doc/architecture/core-protocol.md` by
/// matching the variant. The probe pass swallowed that failure on purpose
/// (every importer answered `Unsupported` for a file it could not read), so
/// the extract pass is the one place it surfaces.
fn import_error_of(reason: PdfTextError) -> ImportError {
    return match reason {
        PdfTextError::ToolMissing => { ImportError::ConverterMissing }
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
    // The mailbox importer of plan item 4.5 sits in this list right after the
    // two built-in statement readers; the order is what makes runs
    // deterministic.
    assert!(names == [
        NAME_BUILT_IN_BANK_A.to_string(),
        NAME_BUILT_IN_BANK_B.to_string(),
        NAME_BUILT_IN_MBOX.to_string(),
    ], "names: {names:?}");
}

#[test]
fn mbox_file_is_accepted_by_the_mbox_builtin_only_and_yields_no_transactions() {
    // R002 through the HOST path: a .mbox file is claimed by exactly ONE
    // built-in, it yields ZERO transactions, and the single warning it reports
    // is the pinned R002 statement — the host can never append a transaction
    // from a mailbox.
    let mbox = scratch_file(
        "mbox_host",
        "thread.mbox",
        b"From anna@example.com Mon Apr 01 12:00:00 2024\nFrom: anna@example.com\nSubject: hi\n\nhello\n",
    );
    let host = Host { importers: builtin_importers() };
    let outcome = import_file(&host, &mbox);

    let accepted: Vec<(String, String)> = outcome.accepted_by.clone();
    assert!(accepted == [(NAME_BUILT_IN_MBOX.to_string(), DIALECT_MBOX.to_string())], "accepted_by: {accepted:?}");
    assert!(outcome.failed.is_empty(), "the mailbox entry must not fail here: {:?}", outcome.failed);
    assert!(outcome.transactions.is_empty(), "R002: a mailbox never yields transactions: {:?}", outcome.transactions);
    assert!(outcome.warnings == [MBOX_NO_TRANSACTIONS.to_string()], "the R002 warning: {:?}", outcome.warnings);

    // The extension probe is case-insensitive, and it is ONLY the mbox entry:
    // bank A and bank B decline a .mbox, the mbox entry claims a .MBOX, and it
    // declines a non-mbox name.
    let mbox_importer = BuiltInMbox;
    let bank_a = BuiltInBankA;
    let bank_b = BuiltInBankB;
    assert!(bank_a.probe(&mbox) == ProbeKind::Unsupported, "bank A must not claim a mailbox");
    assert!(bank_b.probe(&mbox) == ProbeKind::Unsupported, "bank B must not claim a mailbox");
    let upper = scratch_file("mbox_host", "THREAD.MBOX", b"From a@b\n\n\n");
    assert!(mbox_importer.probe(&upper) == ProbeKind::Dialect(DIALECT_MBOX.to_string()), "the extension probe is case-insensitive");
    assert!(mbox_importer.probe(&std::path::Path::new("statement.pdf")) == ProbeKind::Unsupported, "the mbox entry claims no PDF");

    remove_scratch("mbox_host");
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

// ---------------------------------------------------------------------------
// Tests of plan item 4.4b: the plugin importer as an external process
// ---------------------------------------------------------------------------

/// The directory of the TEST-ONLY fixture scripts (`core/tests/fixtures`) —
/// `python3`, planning decision 12; the fake plugin is NOT a cargo target and
/// is never relied on through its executable bit, the core always runs
/// `python3 <script> …`.
#[cfg(test)]
fn find_fixture_dir() -> std::path::PathBuf {
    let start = std::env::current_dir().unwrap_or(std::path::PathBuf::new());
    let mut dir = start;
    loop {
        let candidate = dir.join("tests").join("fixtures");
        if candidate.is_dir() { return candidate; }
        match dir.parent() {
            None => { panic!("importer_host tests cannot find tests/fixtures relative to the test's working directory"); }
            Some(parent) => { dir = parent.to_path_buf(); }
        }
    }
}

/// The argv of an [`ExecImporter`] running the fake plugin in one behavior
/// mode: `python3 core/tests/fixtures/fake_importer.py <mode> [extra…]`.
#[cfg(test)]
fn fake_plugin_argv(mode: &str, extra: &Vec::<String>) -> Vec::<String> {
    let mut argv: Vec::<String> = Vec::new();
    argv.push("python3".to_string());
    argv.push(find_fixture_dir().join("fake_importer.py").to_string_lossy().to_string());
    argv.push(mode.to_string());
    for one in extra {
        argv.push(one.to_string());
    }
    return argv;
}

#[test]
fn fake_plugin_probe_false_never_extracts() {
    // The brief's acceptance mechanism, a FILESYSTEM MARKER: the fake plugin
    // writes the marker file ONLY if it ever receives an `extract` request,
    // and this test asserts that file was NEVER created. The marker sits in
    // this test's own sub-directory of the module-private scratch parent.
    let dir = std::path::PathBuf::from(SCRATCH_PARENT).join("fake_plugin");
    match std::fs::create_dir_all(&dir) {
        Err(reason) => { panic!("cannot create the fake plugin scratch directory: {reason}"); }
        Ok(_) => { }
    }
    let marker = dir.join("extract_marker.txt");
    // No leftovers from a failed earlier run.
    match std::fs::remove_file(&marker) {
        Err(_) => { }
        Ok(_) => { }
    }

    let host = Host {
        importers: vec![
            Box::new(ExecImporter::new(
                fake_plugin_argv("probe-false", &vec![marker.to_string_lossy().to_string()]),
                "fake-probe-false-plugin".to_string(),
                30,
            )),
            Box::new(StubImporter { importer_name: "stub-importer", dialect: "stub", text: STUB_STATEMENT }),
        ],
    };
    let path = std::path::Path::new("nothing-is-read-by-the-fake-plugin.stub");
    let outcome = import_file(&host, &path);

    // The plugin WAS probed (its line was requested), answered `false`, and
    // the host — its contract, `import_file`'s own order — never asked it on.
    let accepted: Vec::<String> = outcome.accepted_by.iter().map(|(name, _)| name.clone()).collect();
    assert!(accepted == ["stub-importer".to_string()], "the probe-false plugin is no supporter: {accepted:?}");
    assert!(outcome.transactions.len() == 1, "the stub's row survives: {:?}", outcome.transactions);
    assert!(outcome.failed.is_empty(), "a plugin simply declining is not a failure: {:?}", outcome.failed);

    // THE marker assertion (the mechanism the brief names): the file appears
    // only if the plugin ever saw an `extract` request.
    match std::fs::exists(&marker) {
        Ok(true) => { assert!(false, "the probe-false plugin received an extract request (the marker was written)"); }
        _ => { }
    }
    remove_scratch("fake_plugin");
}

#[test]
fn fake_plugin_nonzero_exit_names_plugin_others_run() {
    // `plugin-protocol.md:20-23`: a plugin that crashes mid-run is reported
    // NAMING the plugin, and the run continues — the other importer (here the
    // in-memory stub) still delivers its rows. The fake plugin answers a
    // supported probe and then exits 1; the core's extract turn then meets a
    // dead child (closed input or closed output — either route FAILS, naming
    // the plugin and carrying its stderr/exit status).
    let host = Host {
        importers: vec![
            Box::new(ExecImporter::new(fake_plugin_argv("exit-after-probe", &Vec::new()), "fake-exiting-plugin".to_string(), 30)),
            Box::new(StubImporter { importer_name: "stub-importer", dialect: "stub", text: STUB_STATEMENT }),
        ],
    };
    let path = std::path::Path::new("nothing-is-read-by-the-fake-plugin.stub");
    let outcome = import_file(&host, &path);

    assert!(outcome.accepted_by.len() == 2, "the probe said true, so it WAS a supporter: {:?}", outcome.accepted_by);
    assert!(outcome.failed.len() == 1, "exactly one failure: {:?}", outcome.failed);
    assert!(outcome.failed[0].0 == *"fake-exiting-plugin", "the failure names the plugin: {:?}", outcome.failed[0]);
    match &outcome.failed[0].1 {
        ImportError::Failed(text) => {
            assert!(text.contains("fake-exiting-plugin"), "the failure text names the plugin: {text}");
        }
        other => { assert!(false, "the plugin exit must be a Failed, got {other:?}"); }
    }
    assert!(outcome.transactions.len() == 1, "the healthy stub's row survives: {:?}", outcome.transactions);
    assert!(outcome.transactions[0].amount == -3999);

    // A plugin that cannot even be SPAWNED is simply `Unsupported` — the
    // `Importer` contract forbids a failing probe — never a panic.
    let missing_host = Host {
        importers: vec![Box::new(ExecImporter::new(vec!["definitely-not-a-plugin-xyz".to_string()], "missing-plugin".to_string(), 5))],
    };
    let outcome = import_file(&missing_host, path);
    assert!(outcome.accepted_by.is_empty(), "an unspawnable plugin declines: {:?}", outcome.accepted_by);
    assert!(outcome.failed.is_empty(), "declining is not failing: {:?}", outcome.failed);
}

#[test]
fn fake_plugin_timeout_killed() {
    // The review focus: NO infinite waits. The fake plugin answers the probe
    // and then SLEEPS 30 s without answering the extract request; the host
    // bounds every read by `timeout_secs` (1 here), kills and reaps the
    // child, and refuses with a `timed out` text — the wall clock proving the
    // bound, not the fixture's sleep.
    let importer = ExecImporter::new(fake_plugin_argv("sleep-after-probe", &Vec::new()), "fake-sleeping-plugin".to_string(), 1);
    let path = std::path::Path::new("nothing-is-read-by-the-fake-plugin.stub");
    let started = std::time::Instant::now();
    let probe = importer.probe(path);
    assert!(probe == ProbeKind::Dialect("sleepy".to_string()), "the fake plugin supports and names its dialect: {probe:?}");
    let got = importer.extract_transactions(path);
    let elapsed = started.elapsed();

    match got {
        Err(ImportError::Failed(text)) => {
            assert!(text.contains("timed out"), "the refusal says timed out: {text}");
            assert!(text.contains("fake-sleeping-plugin"), "the refusal names the plugin: {text}");
            assert!(text.contains("1 s"), "the refusal names the window: {text}");
        }
        other => { assert!(false, "a plugin that never answers must fail, got {other:?}"); }
    }
    assert!(elapsed.as_secs() < 5, "the 1 s window bounded the run, but it took {seconds} s", seconds = elapsed.as_secs_f64().to_string());

    // Decision 15: the killed child is GONE; a second extract on the same
    // instance is refused WITHOUT spawning a second child for the file-run.
    match importer.extract_transactions(path) {
        Err(ImportError::Failed(text)) => { assert!(text.contains("fake-sleeping-plugin"), "the refusal names the importer: {text}"); }
        other => { assert!(false, "no second child for the same file-run, got {other:?}"); }
    }
}

#[test]
fn reaping_a_child_whose_stdout_a_grandchild_holds_stays_bounded() {
    // The one plugin scenario that made the old `PluginChild::reap` unbounded:
    // a plugin that exits AFTER handing its stdout to a grandchild leaves the
    // pipe's write side open, so the reader thread never sees EOF and joining
    // it waits forever — hanging the whole import, and the run behind it. The
    // reap now bounds that wait through the reader's completion channel
    // ([`REAPER_GRACE_SECS`]) and detaches the thread instead; the failure
    // still lands, still names the plugin, and the wall clock keeps the bound
    // pinned. The grandchild sleeps 5 s, comfortably past the grace window.
    let importer = ExecImporter::new(fake_plugin_argv("grandchild-holds-stdout", &Vec::new()), "leaky-stdout-plugin".to_string(), 1);
    let path = std::path::Path::new("statement.pdf");
    let probe = importer.probe(path);
    assert!(probe == ProbeKind::Dialect("leaky".to_string()), "the plugin supports the file before it disappears: {probe:?}");

    let started = std::time::Instant::now();
    let got = importer.extract_transactions(path);
    let elapsed = started.elapsed();

    match got {
        Err(ImportError::Failed(text)) => { assert!(text.contains("leaky-stdout-plugin"), "the failure still names the plugin: {text}"); }
        other => { assert!(false, "a child a grandchild outlives must fail, not hand back {other:?}"); }
    }
    assert!(elapsed.as_secs() < 5, "the reap of the stuck reader took {seconds} s, so the wait stayed bounded", seconds = elapsed.as_secs_f64().to_string());
}

#[test]
fn fake_plugin_transactions_decoded() {
    // The wire object of `plugin-protocol.md` ("Transactions on the wire"):
    // `amount` an INTEGER of MINOR UNITS (hence the store-side constructor
    // `from_stored`, never `from_parts`, which would re-scale), `date` the
    // canonical `YYYY-MM-DD` text, the text fields carried through.
    // Decision 16: the CORE computes the id and DISCARDS any wire id — the
    // fixture sends a bogus `"id":"deadbeef"` on the first line.
    let importer = ExecImporter::new(fake_plugin_argv("transactions", &Vec::new()), "fake-transaction-plugin".to_string(), 30);
    let path = std::path::Path::new("statement.pdf");
    assert!(importer.probe(path) == ProbeKind::Dialect("bank_x".to_string()));
    let got = match importer.extract_transactions(path) {
        Err(reason) => { assert!(false, "the fake plugin's rows must decode, got {reason:?}"); return; }
        Ok(got) => { got }
    };
    assert!(got.warnings.is_empty(), "the fake plugin says nothing else: {:?}", got.warnings);
    assert!(got.transactions.len() == 2, "two rows and the terminator: {:?}", got.transactions);

    let first = &got.transactions[0];
    assert!(first.date == "2024-01-03".to_string(), "the wire's canonical date carries through: {first:?}");
    assert!(first.amount == -3999, "the wire's integer is already minor units, no re-scaling: {first:?}");
    assert!(first.currency == "EUR".to_string(), "currency: {first:?}");
    assert!(first.description == "REWE MARKT 12".to_string(), "description: {first:?}");
    assert!(first.counterparty == "REWE MARKET".to_string(), "counterparty: {first:?}");
    assert!(first.source == "statement.pdf".to_string(), "source: {first:?}");

    // Decision 16, pinned: the id equals the CORE-computed base id even
    // though the plugin sent `"id":"deadbeef"`.
    let expected = crate::technical::id::base_id("2024-01-03", -3999, "EUR", "REWE MARKT 12", "REWE MARKET", "statement.pdf");
    assert!(first.id == expected, "the core-computed id wins over the wire id: [{}] != [{expected}]", first.id);

    let second = &got.transactions[1];
    assert!(second.date == "2024-01-09".to_string() && second.amount == -5990, "the second row decodes exactly: {second:?}");
    let expected_second = crate::technical::id::base_id("2024-01-09", -5990, "EUR", "DINNER ZUR LEITZ", "ZUR LEITZ", "statement.pdf");
    assert!(second.id == expected_second, "the id is computed even for a line without any id: [{}] != [{expected_second}]", second.id);
}

#[test]
fn a_missing_converter_is_the_typed_converter_missing_variant() {
    // Amendment A4, tested per the T5 notes' ruling: NO public API was widened
    // and no trait method added — the mapping is pinned INSIDE the module with
    // a REAL `PdfTextError::ToolMissing`, obtained through the already-public
    // `pdf_text::extract_text_with` seam (decision 6) by an unfindable program
    // name, and run through the existing `import_error_of` that the built-in
    // extract path already uses. No PATH hacks.
    let path = find_testdata_dir().join("bank_a.pdf");
    match crate::technical::pdf_text::extract_text_with("definitely-not-pdftotext-xyz", &path) {
        Err(reason) => {
            assert!(reason == PdfTextError::ToolMissing, "the injected program cannot exist, so the tool is missing: {reason:?}");
            assert!(import_error_of(reason) == ImportError::ConverterMissing, "the host maps the missing converter to the typed ConverterMissing variant, never to a Failed string");
        }
        Ok(text) => { assert!(false, "the injected program cannot exist, yet the extract handed back {chars} characters", chars = text.len().to_string()); }
    }
}

/// Create — cleaning first, so a failed earlier run cannot leave a stale
/// `plugins` key behind — the scratch DATA DIRECTORY of an `exec_importers`
/// test and return its ABSOLUTE path: the join `exec_importers` performs for a
/// relative `plugins` entry only means what it says when the anchor is
/// absolute. Never the shared `test-scratch` directory; the module-private
/// `SCRATCH_PARENT` of this module is used instead.
#[cfg(test)]
fn scratch_data_dir(sub: &str) -> std::path::PathBuf {
    let dir = std::path::PathBuf::from(SCRATCH_PARENT).join(sub);
    match std::fs::remove_dir_all(&dir) {
        Err(_) => { }
        Ok(_) => { }
    }
    match std::fs::create_dir_all(&dir) {
        Err(reason) => { panic!("cannot create the exec_importers scratch directory [{}]: {reason}", dir.display()); }
        Ok(_) => { }
    }
    let cwd = match std::env::current_dir() {
        Err(reason) => { panic!("the exec_importers test cannot find its working directory: {reason}"); }
        Ok(cwd) => { cwd }
    };
    return cwd.join(&dir);
}

/// Write one `plugins` key into the scratch data directory's `phin.toml`.
#[cfg(test)]
fn write_plugins_key(dir: &Path, text: &str) {
    match std::fs::write(&dir.join(crate::technical::transport_cfg::CONFIG_FILE_NAME.to_string()), text.to_string()) {
        Err(reason) => { panic!("cannot write the exec_importers scratch phin.toml: {reason}"); }
        Ok(_) => { }
    }
}

/// Create one EXISTING file below the scratch data directory, parent directories
/// included, and return its path — deliberately WITHOUT the executable bit, so
/// that "the configured plugin exists" and "the configured plugin can be
/// started" stay separate questions for these tests.
#[cfg(test)]
fn scratch_plugin_file(dir: &Path, relative: &str) -> std::path::PathBuf {
    let path = dir.join(relative);
    match path.parent() {
        None => { }
        Some(parent) => {
            match std::fs::create_dir_all(parent) {
                Err(reason) => { panic!("cannot create the scratch plugin directory [{}]: {reason}", parent.display()); }
                Ok(_) => { }
            }
        }
    }
    match std::fs::write(&path, "#!/bin/sh\nexit 0\n".to_string()) {
        Err(reason) => { panic!("cannot write the scratch plugin [{}]: {reason}", path.display()); }
        Ok(_) => { }
    }
    return path;
}

#[test]
fn exec_importers_resolves_relative_paths_against_the_data_dir() {
    // Plan item 4.4b, user decision G3 and task T5: the NAME is the configured
    // path string VERBATIM; a RELATIVE entry resolves against the data
    // directory and an absolute one is kept (`data-files.md`) — asserted here
    // through the argv the child would ACTUALLY be started with, not only
    // through the name that rides the error events.
    let dir = scratch_data_dir("exec_importers");
    let relative = "./importers/bank-x";
    // An absolute entry inside the SAME scratch tree: kept verbatim.
    let absolute_entry = dir.join("importers/bank-y").to_string_lossy().to_string();
    scratch_plugin_file(&dir, relative);
    scratch_plugin_file(&dir, "importers/bank-y");
    write_plugins_key(&dir, &format!("plugins = [\"{relative}\", \"{absolute_entry}\"]\n"));

    let (importers, issues) = exec_importers(&dir);
    assert!(issues.is_empty(), "a well-shaped plugins key of existing executables is clean: {issues:?}");
    assert!(importers.len() == 2, "one importer per configured path: {:?}", importers.len());
    assert!(importers[0].name() == relative.to_string(), "the name is the configured path string: {:?}", importers[0].name());
    assert!(importers[1].name() == absolute_entry.clone(), "absolute paths keep their name: {:?}", importers[1].name());

    // THE wiring, through the helper `exec_importers` boxes: `argv[0]` is the
    // RESOLVED executable path, not the configured text.
    let wired = exec_importer_for(relative, &dir);
    assert!(wired.argv.len() == 1, "a `plugins` entry is one executable, no arguments: {:?}", wired.argv);
    assert!(Path::new(&wired.argv[0]) == dir.join(relative).as_path(), "a RELATIVE entry resolves against the data directory {dir:?}, got [{}]", wired.argv[0]);
    assert!(wired.name == relative.to_string(), "the name is the configured text, not the resolved path: {:?}", wired.name);
    let wired_absolute = exec_importer_for(&absolute_entry, &dir);
    assert!(wired_absolute.argv.len() == 1, "a `plugins` entry is one executable, no arguments: {:?}", wired_absolute.argv);
    assert!(Path::new(&wired_absolute.argv[0]) == Path::new(&absolute_entry), "an ABSOLUTE entry is kept as given, got [{}]", wired_absolute.argv[0]);

    // Existing-but-unrunnable executables (`scratch_plugin_file` sets NO
    // executable bit) still answer `Unsupported`: the `Importer` contract
    // forbids a failing probe, so a spawn failure DECLINES — it never reaches
    // the run as a failure and never panics.
    let host = Host { importers };
    let outcome = import_file(&host, std::path::Path::new("nothing-is-not-spawned.stub"));
    assert!(outcome.accepted_by.is_empty(), "an unrunnable plugin supports nothing: {:?}", outcome.accepted_by);
    assert!(outcome.failed.is_empty(), "a declining plugin is not a failing one: {:?}", outcome.failed);
    remove_scratch("exec_importers");
}

#[test]
fn exec_importers_reports_a_configured_plugin_that_does_not_exist() {
    // A `plugins` entry whose executable is NOT there must not silently build an
    // `ExecImporter` that declines every probe — that is how a typo in the
    // configuration stayed invisible forever. Discovery reports it TYPED, with
    // the CONFIGURED path, while the entries that DO exist keep their importer:
    // one broken path may not cost the run the other plugins.
    let dir = scratch_data_dir("exec_importers_missing_plugin");
    scratch_plugin_file(&dir, "./present");
    write_plugins_key(&dir, "plugins = [\"./present\", \"./absent\", \"/opt/phin/importers/nowhere\"]\n");
    let (importers, issues) = exec_importers(&dir);

    assert!(importers.len() == 1, "only the existing executables become importers: got {}", importers.len().to_string());
    assert!(importers[0].name() == "./present".to_string(), "the healthy entry survives: {:?}", importers[0].name());
    assert!(issues.len() == 2, "one typed complaint per missing executable: {issues:?}");
    let named: Vec::<String> = issues.iter().map(|issue| {
        match issue {
            ConfigIssue::PluginNotFound(configured) => { configured.clone() }
            other => { format!("unexpected {other:?}") }
        }
    }).collect();
    assert!(named == ["./absent".to_string(), "/opt/phin/importers/nowhere".to_string()], "the complaint names the CONFIGURED path verbatim and is of the documented variant: {named:?}");

    // The `Importer` probe contract stays intact: what IS handed out only ever
    // declines (this scratch plugin is not executable), it never fails a run.
    let host = Host { importers };
    let outcome = import_file(&host, std::path::Path::new("nothing-is-not-spawned.stub"));
    assert!(outcome.accepted_by.is_empty() && outcome.failed.is_empty(), "a probe cannot fail, and an unrunnable plugin declines quietly: {outcome:?}");
    remove_scratch("exec_importers_missing_plugin");
}

#[test]
fn fake_plugin_probe_that_omits_kind_extracts_with_the_resolved_dialect() {
    // The probe answer's `kind` is OPTIONAL on the wire; when it is omitted the
    // host falls back to the configured identity — and the CHILD must be told
    // that SAME resolved value instead of an empty string (the review finding:
    // a host logging one dialect while sending `"kind":""`). The fake plugin
    // echoes the request's `kind` into a transaction's `description`, so what
    // this asserts is the byte the plugin actually received.
    let importer = ExecImporter::new(fake_plugin_argv("probe-no-kind", &Vec::new()), "fake-name-only-plugin".to_string(), 30);
    let path = std::path::Path::new("statement.pdf");
    let probe = importer.probe(path);
    assert!(probe == ProbeKind::Dialect("fake-name-only-plugin".to_string()), "a kind-less supported probe falls back to the configured identity: {probe:?}");
    let got = match importer.extract_transactions(path) {
        Err(reason) => { assert!(false, "the echo line must decode, got {reason:?}"); return; }
        Ok(got) => { got }
    };
    assert!(got.transactions.len() == 1, "one echoed row and the terminator: {:?}", got.transactions);
    assert!(got.transactions[0].description == "kind=[fake-name-only-plugin]".to_string(), "the extract request carried the RESOLVED dialect, neither an empty kind nor no kind at all: got [{desc}]", desc = got.transactions[0].description);
}

#[test]
fn fake_plugin_annotation_event_is_the_pinned_refusal() {
    // Planning decision 17, now EXERCISED end to end (so far the refusal existed
    // while no plugin ever emitted the event): the fake plugin answers a
    // supported probe and then sends the `annotation` event of
    // `plugin-protocol.md` ("mail importers use the same `extract` shape")
    // followed by the `end` terminator. Phase 4 has no annotation-plugin story,
    // so the host refuses with the pinned typed text instead of handing records
    // back.
    let importer = ExecImporter::new(fake_plugin_argv("annotation", &Vec::new()), "fake-annotation-plugin".to_string(), 30);
    let path = std::path::Path::new("mail_paypal.mbox");
    let probe = importer.probe(path);
    assert!(probe == ProbeKind::Dialect("annotating".to_string()), "the fake plugin supports the mailbox: {probe:?}");
    match importer.extract_transactions(path) {
        Err(reason) => {
            // The pinned literal of the task brief, word for word, held by the
            // SAME constant the host returns — the two cannot drift apart.
            assert!(ANNOTATION_REFUSAL == "annotation events not supported for plugin importers yet (Phase 7)", "the pinned refusal text, verbatim: [{ANNOTATION_REFUSAL}]");
            assert!(reason == ImportError::Failed(ANNOTATION_REFUSAL.to_string()), "the decision-17 refusal is what the run gets, got {reason:?}");
        }
        Ok(got) => { assert!(false, "annotation plugins are Phase 7, yet the extract handed back {} records", got.transactions.len().to_string()); }
    }
}

#[test]
fn exec_importers_hands_out_the_default_timeout() {
    // Planning decision 11: the timeout `exec_importers` configures is the
    // documented default of `plugin-protocol.md:24-25`, 60 seconds — pinned ON
    // THE WIRING of the plugin, not merely next to the constant. The bound's
    // EFFECT is `fake_plugin_timeout_killed`, which carries 1 s.
    let dir = scratch_data_dir("exec_importers_timeout");
    scratch_plugin_file(&dir, "./imp");
    write_plugins_key(&dir, "plugins = [\"./imp\"]\n");
    let (importers, issues) = exec_importers(&dir);
    assert!(issues.is_empty(), "clean key: {issues:?}");
    assert!(importers.len() == 1, "one configured plugin: {:?}", importers.len());
    assert!(importers[0].name() == "./imp".to_string(), "the name is the configured string: {:?}", importers[0].name());

    let wired = exec_importer_for("./imp", &dir);
    assert!(wired.timeout_secs == DEFAULT_PLUGIN_TIMEOUT_SECS, "the plugin the host builds carries the default window, got [{}]", wired.timeout_secs.to_string());
    assert!(wired.argv.len() == 1 && Path::new(&wired.argv[0]) == dir.join("./imp").as_path(), "and it is the resolved executable the child is started as, got [{}]", wired.argv[0]);
    assert!(DEFAULT_PLUGIN_TIMEOUT_SECS == 60, "the plugin timeout default is 60 s, got {DEFAULT_PLUGIN_TIMEOUT_SECS}");
    remove_scratch("exec_importers_timeout");
}

#[test]
fn a_config_plugin_list_that_is_malformed_refuses_importers_typed() {
    // The typed refusal, host-side: `plugins = 3` reaches [`exec_importers`] as
    // the typed [`ConfigIssue::PluginsNotStrings`] of `transport_cfg` with an
    // EMPTY importer list — no half-read list, no plugin started, no panic.
    let dir = std::path::PathBuf::from(SCRATCH_PARENT).join("exec_importers_malformed");
    match std::fs::create_dir_all(&dir) {
        Err(reason) => { panic!("cannot create the exec_importers scratch directory: {reason}"); }
        Ok(_) => { }
    }
    match std::fs::write(&dir.join(crate::technical::transport_cfg::CONFIG_FILE_NAME.to_string()), "plugins = 3\n".to_string()) {
        Err(reason) => { panic!("cannot write the scratch phin.toml: {reason}"); }
        Ok(_) => { }
    }
    let (importers, issues) = exec_importers(&dir);
    assert!(importers.is_empty(), "a malformed list hands out NO importers: {:?}", importers.len());
    assert!(issues.len() == 1, "exactly the typed complaint: {issues:?}");
    match &issues[0] {
        ConfigIssue::PluginsNotStrings(_) => { }
        other => { assert!(false, "the refusal must stay PluginsNotStrings, got {other:?}"); }
    }
    remove_scratch("exec_importers_malformed");
}
