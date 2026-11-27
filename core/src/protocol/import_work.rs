//! The `import` command wired into the interpreter — plan item 4.8.
//!
//! [`interpret::WiredNowhere`] refused `import` with the `plugin-failed` code
//! naming this plan item; this module is what it deferred: [`ImportWork`] is
//! the [`interpret::Work`] the production daemon is handed (see
//! `core/src/main.rs`), and its `import` arm drives the whole command itself
//! through plan 3.3's [`writer`] primitives — exactly the precedent the test
//! work of `interpret.rs` pins for a WIRED phase (`writer::start`,
//! [`writer::progress`], [`writer::log`], [`writer::succeed`] and the cancel
//! flag, returning [`interpret::Answered::EndedWith`] so the interpreter
//! writes nothing more). Every OTHER verb delegates to
//! [`interpret::wired_nowhere()`] unchanged (decision 26): the refusal table
//! and its pinned test stay the fallback contract.
//!
//! The command contract is `doc/architecture/core-protocol.md` ("Commands",
//! "Events", "Cancellation") plus the amendments this task fixes:
//!
//! * the `path` parameter may be ONE FILE or a DIRECTORY (user decision G2);
//!   a directory is walked recursively, every file of it handed to the
//!   importer host in path-string order;
//! * the command ends with ONE `result` event whose payload keys are exactly
//!   `added` (transactions newly appended), `annotations` (annotations newly
//!   appended) and `linked` (of those appended annotations, the ones ending
//!   [`link::LinkStatus::Linked`) — decision 23, whose
//!   doc example `core-command-import-result` is amended in the same commit;
//! * a missing path answers the ONE `error` event with the documented `io`
//!   code naming the path, and the command still ends through the stream
//!   (`EndedWith`), so the interpreter adds nothing;
//! * the file loop checks [`writer::cancel_requested`] BEFORE each file
//!   (amendment A6 — the seam is NOT widened; a wired phase queries its own
//!   stream, because the daemon reads one line, runs the whole command, then
//!   reads the next, so a `cancel` line can never land mid-command anyway; the
//!   per-file branch is pinned by an inline test that pre-cancels the stream).
//!   Nothing is half written because each file's results are written only
//!   after that WHOLE file, through [`technical::fs_atomic::write_atomic`]
//!   (decision 25 for the mbox marker);
//! * an importer failure — including the typed
//!   [`importer_host::ImportError::ConverterMissing`] (amendment A4, mapped by
//!   VARIANT, never by message text) — becomes a `log` event naming the
//!   importer and the run continues ("a failing importer must not break the
//!   whole run"). Which choice this build makes when the converter is missing
//!   FOR THE WHOLE RUN is stated where the counters are inspected
//!   ([`converter_missing_terminates_the_run`]): the `converter-missing` code
//!   ends ONLY a run that appended NOTHING at all — no transaction and no
//!   annotation — because a `result` is the one terminal event that reports
//!   counters, and an `error` would throw away work the store already holds. A complaint
//!   about the `plugins` key of `phin.toml` is likewise a `warn` `log` event,
//!   never a hard failure (hand-off 4): the built-ins still import.
//!
//! The store rules are the ones `doc/architecture/data-files.md` fixes:
//! transactions are appended only when their computed occurrence id is not
//! already known (decision 24: the id is assigned by
//! [`technical::id::occurrence_id`] against the ids THIS RUN appended, and the
//! candidate is skipped when that exact full id already sits in the ids on
//! file or appended this run — a re-import then adds nothing while the
//! bank-B2 duplicate pair lands on `base` plus `base#2` in one run); mail
//! files go through the `state/markers.csv` markers, the annotation
//! extraction of [`technical::mail_annotate`] and the automatic linking of
//! [`technical::link`], NEVER through the transactions channel (R002: mail
//! data must not create a transaction, which is also why
//! [`importer_host::BuiltInMbox`] yields no transactions at all).
//!
//! Hand-offs honoured here: the per-file import is SEQUENTIAL (one shared
//! content-hash temp path per byte-identical attachment in
//! [`technical::mail_annotate`] — parallel runs would collide); the mbox text
//! is read HERE through `std::fs` so a real read failure is reportable rather
//! than swallowed into an empty annotation set; and the `items` column joins
//! with `|` (hand-off 3): a description containing `|` would split on
//! read-back, the fixtures contain `;` but no `|`, and NO escaping rule is
//! invented silently here — see the note in [`append_transaction_rows`] and
//! `store_sidefiles::append_annotation`.

use std::path::Path;
use std::path::PathBuf;

use proto::command::Command;
use proto::command::command_name;
use proto::event::ErrorReason;
use serde_json::Map;
use serde_json::Value;

use crate::domain::transaction::Transaction;
use crate::technical::fs_atomic;
use crate::technical::id;
use crate::technical::importer_host;
use crate::technical::link;
use crate::technical::mail_annotate;
use crate::technical::mbox;
use crate::technical::state;
use crate::technical::store_sidefiles;
use crate::technical::store_transactions;
use crate::technical::transport_cfg;
use super::interpret;
use super::writer;
use super::writer::EventSink;

/// The name of the store file of `doc/architecture/data-files.md` holding the
/// transactions (the join key of the mail linking and the target of the
/// idempotent append).
const TRANSACTIONS_FILE: &str = "transactions.csv";

/// The name of the store file holding the annotations.
const ANNOTATIONS_FILE: &str = "annotations.csv";

/// The subdirectory of the data directory holding `markers.csv`
/// (`doc/architecture/data-files.md`, section "`state/`").
const STATE_DIR: &str = "state";

/// The marker file name inside that directory.
const MARKERS_FILE: &str = "markers.csv";

/// The `level` of the `log` events carrying configuration complaints, plugin
/// warnings and importer failures: too important for `info`, too soft for a
/// hard failure.
const LEVEL_WARN: &str = "warn";

/// The `level` of the `log` events carrying routine progress information the
/// interface should see (probe acceptances, marker skips, candidate ids).
const LEVEL_INFO: &str = "info";

/// The work behind the `import` verb of the command table, plus the refusal
/// fallback for every other verb of it (decision 26).
///
/// The data directory it is handed is the anchor every store file and marker
/// file is opened through — the SAME `--data-dir=` the daemon already threads
/// through `core/src/main.rs` and `technical::transport`.
pub struct ImportWork {
    /// The data directory of this daemon instance (`data-files.md`: "data
    /// directory (default: current directory)", resolved by the caller).
    data_dir: PathBuf,
    /// The work every NON-import verb of the table is delegated to unchanged
    /// ([`interpret::wired_nowhere`], decision 26: its refusal table and its
    /// pinned test stay the fallback contract of this build).
    fallback: interpret::WiredNowhere,
}

impl ImportWork {
    /// The import work appending to (and reading from) the data directory
    /// `data_dir`; the other verbs delegate to the untouched
    /// [`interpret::wired_nowhere`] fallback.
    pub fn new(data_dir: PathBuf) -> ImportWork {
        return ImportWork { data_dir: data_dir, fallback: interpret::wired_nowhere() };
    }
}

/// The whole answer of `interpret::Work` for this type: `import` is driven
/// through plan 3.3's stream by [`ImportWork::run_import`] (the wired-phase
/// precedent of `interpret.rs`), and EVERY other verb falls through to
/// [`interpret::WiredNowhere`] verbatim, so the codes, the `cmd` fields and the
/// plan-item-naming texts of those refusals are exactly what the pinned test
/// of that type already asserts.
impl interpret::Work for ImportWork {
    fn do_command(&mut self, command: Command, sink: &mut dyn EventSink) -> interpret::Answered {
        // The name is taken from the table row BEFORE the match consumes the
        // command (the same rule `interpret::serve` states for the `cmd` field
        // of every event), so the command stream of this verb names `import`
        // without spelling a wire name here.
        let verb = command_name(&command);
        match command {
            Command::Import(path) => { return self.run_import(&verb, &path, sink); }
            other => { return self.fallback.do_command(other, sink); }
        }
    }
}

// ---------------------------------------------------------------------------
// The counters of one run (the `result` payload of decision 23)
// ---------------------------------------------------------------------------

/// The three counters the `result` event carries (decision 23: payload keys
/// `added`, `annotations`, `linked`) — plus the failure bookkeeping behind the
/// A4 mapping of [`importer_host::ImportError::ConverterMissing`].
struct RunCounters {
    /// Transactions APPENDED to `transactions.csv` this run.
    added: i64,
    /// Annotations APPENDED to `annotations.csv` this run.
    annotations: i64,
    /// Annotations whose link outcome was [`link::LinkStatus::Linked`].
    linked: i64,
    /// Failing (importer, file) PAIRS of the run, NOT files: one per importer
    /// that failed on one file, so a file TWO supporting importers failed on
    /// is counted TWICE (each of those failures is one `log` event and the run
    /// continues).
    failed_files: i64,
    /// Of those (importer, file) pairs, the ones whose failure carried the
    /// typed [`importer_host::ImportError::ConverterMissing`] variant — the
    /// A4 signal, mapped by VARIANT, never by message text. Comparing it with
    /// [`RunCounters::failed_files`] asks "was EVERY failure of this run a
    /// missing converter?", which counts pairs on both sides.
    converter_missing_files: i64,
}

impl RunCounters {
    /// A run nothing has written to yet.
    fn new() -> RunCounters {
        return RunCounters { added: 0, annotations: 0, linked: 0, failed_files: 0, converter_missing_files: 0 };
    }
}

/// Whether this run ends with the documented `converter-missing` `error` code
/// instead of its `result` (amendment A4's run-level half):
///
/// * `true` only when EVERY failing (importer, file) pair of the run carried
///   the typed [`importer_host::ImportError::ConverterMissing`] variant (a run
///   with a failure of another kind is a run with something to report, not a
///   converter complaint), AND the run appended NOTHING at all — neither a
///   transaction nor an annotation;
/// * `false` in every other case. THE pinned case: a run that appended mail
///   annotations while every statement importer failed with the missing
///   converter ends with its `result`, NOT with an `error` — those annotations
///   already landed in the store, and the `result` is the ONE terminal event
///   that reports them, so an `error` here would lose counters the interface
///   is entitled to see. The per-file `warn` `log` events named every failing
///   importer either way, and the run never dies on one importer failure
///   (`plugin-protocol.md`, "a failing importer must not break the whole
///   run").
fn converter_missing_terminates_the_run(counters: &RunCounters) -> bool {
    return counters.converter_missing_files > 0 && counters.failed_files == counters.converter_missing_files && counters.added == 0 && counters.annotations == 0;
}

// ---------------------------------------------------------------------------
// The store texts in flight, written per file (nothing half written)
// ---------------------------------------------------------------------------

/// The three store files of the run held as TEXTS (the `read`/`append`/
/// `append_annotation`/`append_marker` helpers of the store modules are text
/// in, text out), loaded once when the run starts and written back through
/// [`fs_atomic::write_atomic`] after each FILE that changed them — the
/// "writes happen per file, atomically" of `core-protocol.md` ("Cancellation")
/// that a cancelled or interrupted run leans on.
struct StoreTexts {
    /// The current text of `transactions.csv`.
    transactions: String,
    /// The current text of `annotations.csv`.
    annotations: String,
    /// The current text of `state/markers.csv`.
    markers: String,
    /// Whether each text changed since its last write.
    transactions_dirty: bool,
    annotations_dirty: bool,
    markers_dirty: bool,
    /// The FIRST failure of a read or an atomic write, kept rather than
    /// swallowed: it ends the command with the `io` `error` event, because a
    /// store that could not be touched is a store failure NO log can undo.
    complaint: Option<String>,
}

impl StoreTexts {
    /// Read the three store texts of `data_dir`: an ABSENT file is the empty
    /// text (an unwritten store is an empty store, not an error — the rule
    /// `technical::view` applies too), a file that is NOT there-but-unreadable
    /// (a directory in its place, …) records the [`StoreTexts::complaint`].
    fn load(data_dir: &Path) -> StoreTexts {
        let mut texts = StoreTexts { transactions: String::new(), annotations: String::new(), markers: String::new(), transactions_dirty: false, annotations_dirty: false, markers_dirty: false, complaint: None };
        texts.transactions = read_store_file(&data_dir.join(TRANSACTIONS_FILE), &mut texts);
        texts.annotations = read_store_file(&data_dir.join(ANNOTATIONS_FILE), &mut texts);
        texts.markers = read_store_file(&data_dir.join(STATE_DIR).join(MARKERS_FILE), &mut texts);
        return texts;
    }

    /// The first complaint wins: the terminal `error` event names ONE problem,
    /// and the log events already told the interface about every file.
    fn set_complaint(&mut self, why: String) {
        if self.complaint.is_none() {
            self.complaint = Some(why.to_string());
        }
    }
}

/// The text of `path`, empty when it is absent, and a complaint (through
/// `store`) when it is present but unreadable.
fn read_store_file(path: &Path, store: &mut StoreTexts) -> String {
    match std::fs::read_to_string(path) {
        Err(reason) => {
            if reason.kind() == std::io::ErrorKind::NotFound {
                return String::new();
            }
            store.set_complaint(format!("[{}] could not be read: {reason}", path.to_string_lossy()));
            return String::new();
        }
        Ok(text) => { return text; }
    }
}

/// Write back every store text that changed (per file, atomically — see
/// [`StoreTexts`]); a failure is remembered as the store complaint and a
/// `log` event of the running command, the run continuing with the files it
/// can still touch. Each text is COPIED out of the store before its write, so
/// no write ever borrows the store while the store's complaint is recorded.
fn flush_changed(data_dir: &Path, store: &mut StoreTexts, stream: &mut writer::CommandStream, sink: &mut dyn EventSink) {
    if store.transactions_dirty {
        let text = store.transactions.to_string();
        let written = write_store_text(data_dir, TRANSACTIONS_FILE, &text, stream, sink);
        store.transactions_dirty = false;
        match written {
            Err(complaint) => { store.set_complaint(complaint); }
            Ok(_) => { }
        }
    }
    if store.annotations_dirty {
        let text = store.annotations.to_string();
        let written = write_store_text(data_dir, ANNOTATIONS_FILE, &text, stream, sink);
        store.annotations_dirty = false;
        match written {
            Err(complaint) => { store.set_complaint(complaint); }
            Ok(_) => { }
        }
    }
    if store.markers_dirty {
        let text = store.markers.to_string();
        let written = write_store_text(&data_dir.join(STATE_DIR), MARKERS_FILE, &text, stream, sink);
        store.markers_dirty = false;
        match written {
            Err(complaint) => { store.set_complaint(complaint); }
            Ok(_) => { }
        }
    }
}

/// One [`fs_atomic::write_atomic`] of one changed store text. The target path
/// is rebuilt from `file` (and the `state/` directory for the marker file):
/// the SAME path the text was loaded through. A failure is reported twice, the
/// way every failure of this command is: once as the `log` event the interface
/// sees right away, and once as the [`Err`] text the caller keeps as the
/// store's complaint.
fn write_store_text(dir: &Path, file: &str, text: &str, stream: &mut writer::CommandStream, sink: &mut dyn EventSink) -> Result::<(), String> {
    let path = dir.join(file);
    match fs_atomic::write_atomic(&path, text) {
        Err(reason) => {
            let complaint = format!("[{}] could not be written: {}", path.to_string_lossy(), write_failure_text(&reason));
            log_line(stream, sink, LEVEL_WARN.to_string(), complaint.to_string());
            return Err(complaint);
        }
        Ok(_) => { return Ok(()); }
    }
}

/// The text of one [`fs_atomic::WriteError`] — it carries its three parts as
/// TYPED data and no `Display`, like every other typed error of this core, so
/// rendering it is the caller's job (the same treatment `main.rs` gives a
/// [`transport_cfg::ConfigIssue`]).
fn write_failure_text(reason: &fs_atomic::WriteError) -> String {
    match reason {
        fs_atomic::WriteError::DirectoryRejected(dir, kind, message) => { return format!("the directory [{}] was refused ({kind}): {message}", dir.to_string_lossy()); }
        fs_atomic::WriteError::PreviousUnreadable(target, kind, message) => { return format!("the previous version of [{}] could not be read ({kind}): {message}", target.to_string_lossy()); }
        fs_atomic::WriteError::TempWriteFailed(target, kind, message) => { return format!("the temporary file [{}] could not be written ({kind}): {message}", target.to_string_lossy()); }
        fs_atomic::WriteError::BackupWriteFailed(target, kind, message) => { return format!("the backup [{}] could not be written ({kind}): {message}", target.to_string_lossy()); }
        fs_atomic::WriteError::RenameFailed(target, kind, message) => { return format!("[{}] could not be renamed into place ({kind}): {message}", target.to_string_lossy()); }
    }
}

// ---------------------------------------------------------------------------
// The import run itself
// ---------------------------------------------------------------------------

impl ImportWork {
    /// Drive the WHOLE `import` command through its own command stream — the
    /// wired-phase claim of [`interpret::Answered::EndedWith`]: this method
    /// writes the terminal event whatever it is (`result`, `error` or
    /// `cancelled`) and the interpreter adds nothing.
    fn run_import(&self, verb: &str, given: &str, sink: &mut dyn EventSink) -> interpret::Answered {
        // The host: the built-in importers first (bank A, bank B, the mbox
        // entry), then the plugins discovered from `phin.toml` of THIS data
        // directory (plan item 4.4b). A plugin-config complaint is a `warn`
        // log event, never a hard failure (hand-off 4): the built-ins still
        // import, a missing plugin simply imports nothing of its own.
        let mut importers = importer_host::builtin_importers();
        let (plugins, issues) = importer_host::exec_importers(&self.data_dir);
        for plugin in plugins {
            importers.push(plugin);
        }
        let host = importer_host::Host { importers };
        // Enumerate the items BEFORE the stream exists: the total the stream
        // promises at `start` is the item count, which is what
        // `writer::progress` can never exceed (its `TotalPassed` rule).
        let items = match enumerate_items(Path::new(given)) {
            Ok(held) => held,
            Err(()) => {
                let mut stream = writer::start(verb.to_string(), 0);
                for held in issues {
                    log_line(&mut stream, sink, LEVEL_WARN.to_string(), config_issue_text(&held));
                }
                match writer::fail(&mut stream, sink, ErrorReason::Io, format!("no such path: {given}")) {
                    Err(_) => { }
                    Ok(_) => { }
                }
                return interpret::Answered::EndedWith;
            }
        };
        let total = match i64::try_from(items.len()) {
            Err(_) => {
                let mut stream = writer::start(verb.to_string(), 0);
                for held in issues {
                    log_line(&mut stream, sink, LEVEL_WARN.to_string(), config_issue_text(&held));
                }
                match writer::fail(&mut stream, sink, ErrorReason::Io, format!("[{given}] holds more files than this command can count")) {
                    Err(_) => { }
                    Ok(_) => { }
                }
                return interpret::Answered::EndedWith;
            }
            Ok(count) => count,
        };
        let mut stream = writer::start(verb.to_string(), total);
        for held in issues {
            log_line(&mut stream, sink, LEVEL_WARN.to_string(), config_issue_text(&held));
        }
        let mut store = StoreTexts::load(&self.data_dir);
        let mut counters = RunCounters::new();
        // The file loop ITSELF lives in a private helper taking the stream
        // (amendment A6): a unit test can pre-cancel the stream and pin the
        // branch; here it is the production loop unchanged.
        if !process_files(&self.data_dir, &host, &items, &mut stream, sink, &mut store, &mut counters) {
            // The stream ended with the `cancelled` event (the cancel check
            // before a file) or with an `error` a stream refusal pinned; the
            // command's terminal event was written HERE either way.
            return interpret::Answered::EndedWith;
        }
        // A store file this run could not touch is the one failure that DOESN'T
        // just log: the counters would claim work the store does not hold.
        match &store.complaint {
            Some(complaint) => {
                match writer::fail(&mut stream, sink, ErrorReason::Io, complaint.to_string()) {
                    Err(_) => { }
                    Ok(_) => { }
                }
                return interpret::Answered::EndedWith;
            }
            None => { }
        }
        // Amendment A4, the run-level half, decided by the counters of this
        // run (the rule, including why landed annotations rule the `error`
        // OUT, is [`converter_missing_terminates_the_run`]). The mapping keeps
        // it self-evident: the code is chosen by VARIANT-driven counters, never
        // by a message lookup
        if converter_missing_terminates_the_run(&counters) {
            match writer::fail(&mut stream, sink, ErrorReason::ConverterMissing, format!("the PDF text converter is missing for this whole run: {count} file(s) could not be converted", count = counters.converter_missing_files)) {
                Err(_) => { }
                Ok(_) => { }
            }
            return interpret::Answered::EndedWith;
        }
        let mut payload = Map::<String, Value>::new();
        payload.insert("added".to_string(), number_of(counters.added));
        payload.insert("annotations".to_string(), number_of(counters.annotations));
        payload.insert("linked".to_string(), number_of(counters.linked));
        match writer::succeed(&mut stream, sink, &payload) {
            Err(_) => { }
            Ok(_) => { }
        }
        return interpret::Answered::EndedWith;
    }
}

/// Enumerate the files of the `import` command: one FILE is itself; a
/// DIRECTORY walks down recursively and sorts the WHOLE list by the path
/// string (user decision G2 — sorted so the run is deterministic, and so the
/// mail files of a fixture directory land after the statements their
/// annotations may link to). A missing path is the typed `Err(())` the caller
/// answers with the `io` code; unreadable subdirectories of a walked tree are
/// skipped (the walk hands every file to the host, which probes files it
/// cannot read as `Unsupported` anyway).
fn enumerate_items(given: &Path) -> Result::<Vec::<PathBuf>, ()> {
    match std::fs::metadata(given) {
        Err(_) => { return Err(()); }
        Ok(_) => { }
    }
    if !given.is_dir() {
        return Ok(vec![given.to_path_buf()]);
    }
    let mut items = Vec::<PathBuf>::new();
    let mut left = vec![given.to_path_buf()];
    while left.len() > 0 {
        let dir = left.remove(0);
        match std::fs::read_dir(&dir) {
            Err(_) => { continue; }
            Ok(entries) => {
                for entry in entries {
                    match entry {
                        Err(_) => { continue; }
                        Ok(held) => {
                            let path = held.path();
                            match path.is_dir() {
                                true => { left.push(path.to_path_buf()); }
                                false => {
                                    match path.is_file() {
                                        true => items.push(path.to_path_buf()),
                                        false => { }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    items.sort_by(|first, second| first.to_string_lossy().partial_cmp(&second.to_string_lossy()).unwrap_or(core::cmp::Ordering::Equal));
    return Ok(items);
}

/// The file loop of the import run, kept PRIVATE and SEPARATE from
/// [`ImportWork::run_import`] so amendment A6's unit test can hand it a stream
/// a test has already cancelled with [`writer::request_cancel`] and pin that
/// NOTHING of the loop runs then (no progress line, no store change, one
/// `cancelled`).
///
/// Returns `true` when the loop ran out of items (the caller may then end the
/// command with its `result`), `false` when the stream ended by itself: the
/// one `cancelled` event of "Cancellation" after a cancel was seen BEFORE a
/// file — the partial result of the never-started file is discarded, the files
/// already written stay valid because every write happens after a whole file
/// (decision 25, `fs_atomic::write_atomic`). The import of one run is
/// SEQUENTIAL on purpose (hand-off 1: [`mail_annotate`] shares one content-hash
/// temp path for byte-identical attachments).
fn process_files(data_dir: &Path, host: &importer_host::Host, items: &Vec::<PathBuf>, stream: &mut writer::CommandStream, sink: &mut dyn EventSink, store: &mut StoreTexts, counters: &mut RunCounters) -> bool {
    for index in 0..items.len() {
        if writer::cancel_requested(stream) {
            match writer::cancel(stream, sink) {
                Err(_) => { }
                Ok(_) => { }
            }
            return false;
        }
        let path = &items[index];
        let shown = file_name_of(path);
        match writer::progress(stream, sink, Some(shown.to_string())) {
            Err(reason) => {
                // The stream itself refused the event (already ended, or a
                // `done` number past `total` — neither reachable from here by
                // construction, handled typed rather than assumed).
                match writer::fail(stream, sink, ErrorReason::Io, write_refusal_text(reason)) {
                    Err(_) => { }
                    Ok(_) => { }
                }
                return false;
            }
            Ok(_) => { }
        }
        process_one_file(host, path, &shown, stream, sink, store, counters);
        flush_changed(data_dir, store, stream, sink);
    }
    return true;
}

/// Process ONE file of the run: probe (and log) the host acceptances, then
/// either append the extracted transactions idempotently (any non-mail file)
/// or run the marker/annotation/link path (a file the mbox importer claimed).
/// Every failure of the importer host is a `log` event and the run continues;
/// the store texts change only through the helpers, so one whole file's
/// results either all landed or none (decision 25's guarantee for the marker
/// and its `progress`/`log` companions).
fn process_one_file(host: &importer_host::Host, path: &Path, shown: &String, stream: &mut writer::CommandStream, sink: &mut dyn EventSink, store: &mut StoreTexts, counters: &mut RunCounters) {
    let outcome = importer_host::import_file(host, path);
    for held in outcome.accepted_by.iter() {
        log_line(stream, sink, LEVEL_INFO.to_string(), format!("probe: {name} accepted {shown} as {dialect}", name = held.0, dialect = held.1));
    }
    for warning in outcome.warnings.iter() {
        log_line(stream, sink, LEVEL_WARN.to_string(), format!("{shown}: {warning}"));
    }
    for failed in outcome.failed.iter() {
        counters.failed_files = counters.failed_files + 1;
        let reason_text = match &failed.1 {
            importer_host::ImportError::Unreadable(text) => { format!("unreadable: {text}") }
            importer_host::ImportError::Failed(text) => { text.to_string() }
            importer_host::ImportError::ConverterMissing => {
                // Amendment A4: the VARIANT names the converter, and the
                // run-level inspection of the counters maps it to the
                // documented `converter-missing` code — never a text lookup.
                counters.converter_missing_files = counters.converter_missing_files + 1;
                "converter-missing: the PDF text converter could not run".to_string()
            }
        };
        log_line(stream, sink, LEVEL_WARN.to_string(), format!("importer {name} failed for {shown}: {why}", name = failed.0, why = reason_text));
    }
    let is_mbox = outcome.accepted_by.iter().any(|held| held.1 == importer_host::DIALECT_MBOX);
    match is_mbox {
        true => { process_mbox_file(path, shown, stream, sink, store, counters); }
        false => {
            match append_transaction_rows(&store.transactions, &outcome.transactions) {
                Err(reason) => {
                    log_line(stream, sink, LEVEL_WARN.to_string(), format!("{shown}: a transaction row could not be rendered: {reason:?}"));
                }
                Ok((new_text, appended)) => {
                    if appended > 0 {
                        store.transactions = new_text;
                        store.transactions_dirty = true;
                        counters.added = counters.added + appended;
                    }
                }
            }
        }
    }
}

/// The annotation path of one mailbox: the `state/markers.csv` marker decides
/// skip-or-reprocess, the reprocess reads the mbox TEXT HERE (hand-off 2:
/// [`mail_annotate::annotate_mbox`] swallows a read failure into an empty set,
/// which would report an unreadable file as a clean no-op), annotates the
/// messages from the marker's recorded `offset` ON (plan 4.8's step (e): the
/// RESUME of `mbox::messages_from`, never a re-annotation of the whole file
/// saved by the append dedup), links the facts against the transactions of the
/// run's store text, and appends the annotations with the `ref`/`confidence`
/// the link outcome carries (linked -> the candidate's id; pending and orphan
/// -> an EMPTY `ref`, `data-files.md`). Pending candidate ids become a `log`
/// event so the interface can see them.
///
/// The marker records what THIS pass read of the file, and only after the pass
/// is done (decision 25): `offset` is the `end` of the LAST message processed,
/// and the marker, the annotations and the store are written per file,
/// atomically — a cancel or a crash between two files never leaves a
/// half-processed marker behind. A file whose `mtime`/`size` stamp could not be
/// read gets NO marker at all: a `0`/`0` stamp would make [`state::decide`]
/// never skip the path again, and a marker stamped 0 would be a lie about what
/// was seen.
fn process_mbox_file(path: &Path, shown: &String, stream: &mut writer::CommandStream, sink: &mut dyn EventSink, store: &mut StoreTexts, counters: &mut RunCounters) {
    let path_text = path.to_string_lossy().to_string();
    // The stamp of the version this run is reading, kept WHOLE rather than
    // defaulted: the `None` case must not write a marker at all (see the
    // method doc), because `state::decide` compares mtime AND size, and a
    // marker of 0 and 0 would be a version no file can ever match.
    let stamp = file_stamp(path);
    let (mtime, size) = match stamp {
        None => {
            log_line(stream, sink, LEVEL_WARN.to_string(), format!("{shown}: its file stamp could not be read, so no marker is written for it"));
            // The only numbers `state::decide` can be handed without a stamp:
            // no marker on file can hold them (they are never written), so the
            // decision comes out `Reprocess`, which is what an unknown version
            // deserves.
            (0, 0)
        }
        Some(held) => held,
    };
    let (_, markers, warnings) = state::read_markers(&store.markers);
    for warning in warnings.iter() {
        log_line(stream, sink, LEVEL_WARN.to_string(), format!("{}: marker problem: {warning:?}", MARKERS_FILE));
    }
    // The marker of THIS path, read directly (`state::marker_of`) as well as
    // through `state::decide`, so the run can tell the interface WHAT it is
    // reprocessing: a marker whose offset no more than covers the file means
    // messages were appended since the last import.
    let recorded = state::marker_of(&markers, &path_text);
    match state::decide(&markers, &path_text, mtime, size) {
        state::ImportDecision::Skip => {
            log_line(stream, sink, LEVEL_INFO.to_string(), format!("marker skip: {shown}"));
            return;
        }
        state::ImportDecision::Reprocess => {
            match &recorded {
                None => {
                    log_line(stream, sink, LEVEL_INFO.to_string(), format!("{shown}: no marker of this mailbox is on file, so every message of it is annotated"));
                }
                Some(held) => {
                    match stamp {
                        None => {
                            log_line(stream, sink, LEVEL_INFO.to_string(), format!("{shown}: its marker says mtime {}, size {}, offset {}; the file's own stamp could not be read, so its messages from that offset on are annotated", held.mtime, held.size, held.offset));
                        }
                        Some((mtime_now, size_now)) => {
                            log_line(stream, sink, LEVEL_INFO.to_string(), format!("{shown}: its marker says mtime {}, size {}, offset {}; the file is now mtime {}, size {}, so its messages from that offset on are annotated", held.mtime, held.size, held.offset, mtime_now, size_now));
                        }
                    }
                }
            }
        }
    }
    let text = match std::fs::read_to_string(path) {
        Err(reason) => {
            log_line(stream, sink, LEVEL_WARN.to_string(), format!("{shown}: the mailbox could not be read: {reason}"));
            return;
        }
        Ok(held) => held
    };
    // THE RESUME of plan item 4.8's step (e): only the messages that start AT
    // OR AFTER the `offset` the marker recorded are annotated. That is
    // [`mbox::messages_from`]'s contract, which [`mail_annotate::annotate_mbox`]
    // implements through the byte offset a marker stores (its own module doc
    // names plan item 4.8 as the caller handing it that offset): an offset in
    // the middle of a message means "this message is already known". An
    // unmarked file resumes at 0, i.e. from its first message; an offset past
    // the text of a SHORTENED file can resume nowhere, so it is clamped.
    // The dedup of [`store_sidefiles::append_annotation`] by annotation id
    // stays the safety net under the resume, never the mechanism of it.
    let recorded_offset = match &recorded {
        None => 0,
        Some(held) => usize::try_from(held.offset).unwrap_or(0),
    };
    let resume = recorded_offset.min(text.len());
    // [`mail_annotate::annotate_mbox`] is the committed entry point that takes
    // the marker offset and hands it to `mbox::messages_from` (its module doc
    // names plan item 4.8 as that caller); it reads the file text ITSELF again,
    // which the read above already proved readable — the text read here stays
    // the authority for the offset below, and the annotation append's idempotence
    // covers a message that arrived in between the two reads. (The text-reading
    // variant [`mail_annotate::annotate_mbox_text`] cannot resume: it always
    // starts at the first message.)
    let annotated = mail_annotate::annotate_mbox(path, &path_text, resume);
    // Link against the transactions of the store as this run holds it now —
    // statements imported EARLIER in the same run are candidates too (the
    // sorted walk puts the fixture statements before its mail files).
    let (rows, row_errors) = store_transactions::read(&store.transactions);
    for error in row_errors.iter() {
        log_line(stream, sink, LEVEL_WARN.to_string(), format!("{}: a stored transaction row could not be read for linking: {error:?}", TRANSACTIONS_FILE));
    }
    let linked = link::auto_link(&annotated, &rows);
    let mut annotation_text = store.annotations.to_string();
    let mut appended_here = 0;
    for entry in linked.iter() {
        match store_sidefiles::append_annotation(&annotation_text, &entry.annotation) {
            Err(reason) => {
                log_line(stream, sink, LEVEL_WARN.to_string(), format!("an annotation could not be rendered: {reason:?}"));
            }
            Ok((new_text, outcome)) => {
                match outcome {
                    store_sidefiles::AppendOutcome::Appended => {
                        annotation_text = new_text;
                        appended_here = appended_here + 1;
                        counters.annotations = counters.annotations + 1;
                        // Decision 23's THIRD counter: annotations ending
                        // `Linked`. It is raised only for a row this run
                        // actually appended, so a re-import of an already
                        // annotated mailbox counts nothing a second time.
                        match &entry.status {
                            link::LinkStatus::Linked => { counters.linked = counters.linked + 1; }
                            link::LinkStatus::Pending(candidates) => {
                                // `data-files.md`: a pending annotation keeps
                                // an EMPTY `ref`; the candidate ids are what
                                // plan 6.3's `list-pending` will show, so the
                                // interface is told them here.
                                log_line(stream, sink, LEVEL_INFO.to_string(), format!("pending link for annotation {}: candidates: {}", entry.annotation.id, candidates.join(", ")));
                            }
                            link::LinkStatus::Orphan => { }
                        }
                    }
                    store_sidefiles::AppendOutcome::Skipped => { }
                }
            }
        }
    }
    if appended_here > 0 {
        store.annotations = annotation_text;
        store.annotations_dirty = true;
    }
    // The marker covers what this pass READ of the file: the `end` of the LAST
    // message the resume handed to the annotation path — for the whole file
    // that is its end, and for a resumed mailbox the end of the newest message
    // read. A pass that read no new message keeps the offset already covered.
    // Written only now, after the file is done (decision 25).
    let mut covered = resume;
    for message in mbox::messages_from(&text, resume) {
        covered = message.end;
    }
    let offset_number = i64::try_from(covered).unwrap_or(0);
    match stamp {
        None => {
            // The `warn` event above said WHY: no stamp, no version to record.
            // The path keeps its LAST KNOWN marker (or stays unmarked) and is
            // reprocessed from there on the next run, which is the honest
            // answer; a stamped 0/0 marker would be a version no file has and
            // would make `state::decide` refuse to skip it forever.
        }
        Some((mtime_now, size_now)) => {
            let marker = state::Marker { path: path_text.to_string(), mtime: mtime_now, size: size_now, offset: offset_number };
            match state::append_marker(&store.markers, &marker) {
                Err(reason) => {
                    log_line(stream, sink, LEVEL_WARN.to_string(), format!("the marker of {shown} could not be rendered: {reason:?}"));
                }
                Ok((new_text, outcome)) => {
                    match outcome {
                        state::AppendOutcome::Appended => {
                            store.markers = new_text;
                            store.markers_dirty = true;
                        }
                        state::AppendOutcome::Skipped => { }
                    }
                }
            }
        }
    }
}

/// The `mtime` (whole seconds before the Unix epoch) and `size` of a file,
/// the two numbers the marker contract of `data-files.md` compares; `None`
/// when the stamp is unreadable or the second clock does not make sense.
fn file_stamp(path: &Path) -> Option::<(i64, i64)> {
    let metadata = match std::fs::metadata(path) {
        Err(_) => { return None; }
        Ok(held) => held
    };
    let since_epoch = match metadata.modified() {
        Err(_) => { return None; }
        Ok(held) => held
    };
    let seconds = match since_epoch.duration_since(std::time::SystemTime::UNIX_EPOCH) {
        Err(_) => { return None; }
        Ok(held) => held
    };
    let mtime = match i64::try_from(seconds.as_secs()) {
        Err(_) => { return None; }
        Ok(held) => held
    };
    let size = match i64::try_from(metadata.len()) {
        Err(_) => { return None; }
        Ok(held) => held
    };
    return Some((mtime, size));
}

/// The `file` field of the `progress` event, and the name every `log` event
/// shows: the file's own name, falling back to the whole path when it has
/// none.
fn file_name_of(path: &Path) -> String {
    match path.file_name() {
        None => { return path.to_string_lossy().to_string(); }
        Some(held) => { return held.to_string_lossy().to_string(); }
    }
}

/// The message of a [`writer::WriteError`] refusal, kept rather than swallowed
/// (the same treatment `interpret::refusal_text` gives the type). The naming
/// note in `writer.rs` is about THIS pair: the event-writer's `WriteError` and
/// `fs_atomic`'s carry the same name, and this module touches both.
fn write_refusal_text(reason: writer::WriteError) -> String {
    match reason {
        writer::WriteError::CancelRequested => { return "the command stream was already asked to cancel".to_string(); }
        writer::WriteError::AlreadyFinished(ended) => { return format!("the command stream already ended with {ended:?}"); }
        writer::WriteError::Codec(codex) => { return proto::codec::error_text(&codex); }
        writer::WriteError::TotalPassed(promised) => { return format!("the stream promised {promised} items"); }
    }
}

/// The text one [`transport_cfg::ConfigIssue`] of the plugin discovery shows
/// the interface (the same rendering `main.rs` gives a config complaint on
/// stderr; the event stream is where an import run tells about `plugins`, so
/// the two renderings agree in shape).
fn config_issue_text(issue: &transport_cfg::ConfigIssue) -> String {
    match issue {
        transport_cfg::ConfigIssue::NotReadable(text) => { return format!("the configuration file could not be read: {text}"); }
        transport_cfg::ConfigIssue::NotTOML(text) => { return format!("the configuration file is not TOML: {text}"); }
        transport_cfg::ConfigIssue::WrongType(text) => { return format!("a configuration key holds the wrong type: {text}"); }
        transport_cfg::ConfigIssue::PortOutOfRange(value) => { return format!("the configured port {value} cannot be bound"); }
        transport_cfg::ConfigIssue::PluginsNotStrings(text) => { return format!("the configured plugin list is malformed: {text}"); }
        transport_cfg::ConfigIssue::PluginNotFound(text) => { return format!("the configured plugin [{text}] cannot be found in the data directory"); }
    }
}

/// One `log` event of the running command, its refusal (if the stream has
/// ended meanwhile) NOT swallowed silently by a store bug but immaterial to
/// the interface: the writer already answers it typed, and NO further event
/// of a finished command is allowed anyway.
fn log_line(stream: &mut writer::CommandStream, sink: &mut dyn EventSink, level: String, text: String) {
    match writer::log(stream, sink, level, text) {
        Err(_) => { }
        Ok(_) => { }
    }
}

/// The `result` payload number: `serde_json`'s i64 without a cast.
fn number_of(value: i64) -> Value {
    return Value::Number(serde_json::Number::from(value));
}

// ---------------------------------------------------------------------------
// The idempotence rule of decision 24 — the store's occurrence suffix
// ---------------------------------------------------------------------------

/// Append every candidate transaction whose EXACT occurrence id is not known
/// yet, returning the new file text and how many rows were appended.
///
/// The rule (decision 24, `architecture.md` "appends only unknown ids" and
/// `data-files.md` "idempotence"): the full id of a candidate is
/// [`id::occurrence_id`] of its base against the ids THIS CALL appended so far
/// (the bare base first, then `base#2`, `base#3`, …), and the candidate is
/// skipped when that exact full id already exists in the ids on file OR in
/// this run's appended ids. That is what makes the two pinned expectations
/// hold together:
///
/// * one run appends a byte-identical PAIR (`bank_b2` prints the same row
///   twice) as `base` and `base#2`, because the ids on file do not hold
///   either of them yet; and
/// * a SECOND run of the same files adds NOTHING, because every candidate's
///   first full id (the bare base) is already on file — a skipped candidate
///   is NOT recorded as appended, so its identical successor recomputes the
///   SAME full id and skips exactly as hard.
///
/// Why not [`store_transactions::append`]: that committed helper renders the
/// BARE base id (it "is NOT taken from tx.id" by plan 2.5's design) and skips
/// any candidate whose base already matches an id on file — so it can neither
/// write the `base#2` occurrence row the `transactions.csv` contract and the
/// `generate.py` fixtures pin, nor tell the second occurrence of one run
/// apart from a re-import. This helper therefore renders the SAME contract
/// columns through the SAME codec ([`crate::technical::csv::render_record`],
/// `id;date;amount;currency;description;counterparty;source` with the amount
/// in stored minor units) and appends after the existing text the way
/// `store_transactions::append` does; the id column is just the occurrence id
/// this rule computed.
///
/// KNOWN LIMITATION handed on from plan item 4.7 (hand-off 3, NOT silently
/// invented away here): nothing in this render path escapes a `|` INSIDE a
/// field, and the store's annotation `items` column joins with `|` (the
/// transaction columns are `;`-separated by the codec, so a description
/// containing `|` is safe as a transaction field, but an annotation ITEM
/// containing `|` would split on read-back). The fixture descriptions
/// contain `;` but no `|`; an escaping rule is NOT invented here — it needs
/// a doc change first.
fn append_transaction_rows(text: &str, candidates: &[Transaction]) -> Result::<(String, i64), crate::technical::csv::CodecError> {
    let (rows, _) = store_transactions::read(text);
    let mut on_file = Vec::<String>::new();
    for row in rows.iter() {
        on_file.push(row.id.to_string());
    }
    let mut appended_ids = Vec::<String>::new();
    let mut new_text = text.to_string();
    let mut appended = 0;
    for candidate in candidates.iter() {
        let base = id::transaction_base_id(candidate);
        let full = id::occurrence_id(&base, &appended_ids);
        if holds_id(&on_file, &full) || holds_id(&appended_ids, &full) {
            continue;
        }
        let mut fields = Vec::<String>::new();
        fields.push(full.to_string());
        fields.push(candidate.date.to_string());
        fields.push(candidate.amount.to_string());
        fields.push(candidate.currency.to_string());
        fields.push(candidate.description.to_string());
        fields.push(candidate.counterparty.to_string());
        fields.push(candidate.source.to_string());
        match crate::technical::csv::render_record(&fields) {
            Err(reason) => { return Err(reason); }
            Ok(line) => {
                if new_text.len() > 0 && !new_text.ends_with("\n") {
                    new_text.push_str("\n");
                }
                new_text.push_str(&line);
                new_text.push_str("\n");
                appended_ids.push(full.to_string());
                appended = appended + 1;
            }
        }
    }
    return Ok((new_text, appended));
}

/// Whether the id list holds this exact id (the exact-full-id rule of
/// decision 24 — the bare base and every `base#N` are separate ids).
fn holds_id(ids: &Vec::<String>, wanted: &String) -> bool {
    for held in ids.iter() {
        if held == wanted {
            return true;
        }
    }
    return false;
}

// ============================================================================
// Tests
// ============================================================================

/// The trait the tests call [`ImportWork::do_command`] through (the seam is an
/// implementation of [`interpret::Work`], so calling its method needs the
/// trait in scope).
#[cfg(test)]
use super::interpret::Work;

/// The SCRATCH PARENT of this module: MODULE-PRIVATE, following the pattern
/// of the other store and transport modules, so no test's cleanup can delete
/// another module's scratch tree. Never the shared `test-scratch` parent.
#[cfg(test)]
const SCRATCH_PARENT: &str = "test-scratch-import-work";

/// A scratch directory of this module's own, removed first if a failed run
/// left it behind and created anew (the pattern of the transport row).
#[cfg(test)]
fn scratch(name: String) -> std::path::PathBuf {
    let dir = std::path::PathBuf::from(SCRATCH_PARENT).join(name).to_path_buf();
    match std::fs::remove_dir_all(&dir) {
        Err(_) => { }
        Ok(_) => { }
    }
    match std::fs::create_dir_all(&dir) {
        Err(_) => { }
        Ok(_) => { }
    }
    return dir;
}

/// Remove a scratch directory and, best effort, the empty parent.
#[cfg(test)]
fn scratch_release(dir: &std::path::PathBuf) {
    match std::fs::remove_dir_all(dir) {
        Err(_) => { }
        Ok(_) => { }
    }
    match std::fs::remove_dir(std::path::PathBuf::from(SCRATCH_PARENT)) {
        Err(_) => { }
        Ok(_) => { }
    }
}

/// The frames one session wrote, collected instead of written to a stream —
/// read back with [`proto::codec::decode_line`] so no test compares bytes.
#[cfg(test)]
struct Frames {
    pub frames: Vec::<String>
}

#[cfg(test)]
fn collecting() -> Frames {
    return Frames { frames: Vec::<String>::new() };
}

#[cfg(test)]
impl EventSink for Frames {
    fn write_line(&mut self, line: String) {
        self.frames.push(line.to_string());
    }
}

/// The typed events of the collected frames, every frame required to read
/// back as exactly one event of the protocol.
#[cfg(test)]
fn events_of(frames: &Vec::<String>) -> Vec::<proto::event::Event> {
    let mut events = Vec::<proto::event::Event>::new();
    for index in 0..frames.len() {
        match proto::codec::decode_line(&frames[index]) {
            Ok(proto::codec::Message::Outbound(event)) => { events.push(event); }
            _ => { panic!("the frame [{frame}] of the import work does not read back as one event of the protocol", frame = frames[index].to_string()); }
        }
    }
    assert!(events.len() == frames.len());
    return events;
}

#[cfg(test)]
fn kinds_of(events: &Vec::<proto::event::Event>) -> Vec::<String> {
    let mut names = Vec::<String>::new();
    for index in 0..events.len() {
        names.push(proto::event::event_kind(&events[index]).to_string());
    }
    return names;
}

/// The `code` of every `error` event, as the text the interface reads.
#[cfg(test)]
fn codes_of(events: &Vec::<proto::event::Event>) -> Vec::<String> {
    let mut codes = Vec::<String>::new();
    for event in events.iter() {
        match event {
            proto::event::Event::Error(_, reason, _) => { codes.push(proto::event::reason_name(reason)); }
            _ => { }
        }
    }
    return codes;
}

/// The `text` of every `error` and every `log` event, in order.
#[cfg(test)]
fn texts_of(events: &Vec::<proto::event::Event>) -> Vec::<String> {
    let mut texts = Vec::<String>::new();
    for event in events.iter() {
        match event {
            proto::event::Event::Error(_, _, text) => { texts.push(text.to_string()); }
            proto::event::Event::Log(_, text) => { texts.push(text.to_string()); }
            _ => { }
        }
    }
    return texts;
}

/// The `cmd` name and the payload counter of a `result` event, looked up by
/// key (never compared byte for byte).
#[cfg(test)]
fn counter_of(events: &Vec::<proto::event::Event>, key: &str) -> i64 {
    for event in events.iter() {
        match event {
            proto::event::Event::Result(_, payload) => {
                match payload.get(key) {
                    Some(held) => {
                        match held.as_i64() {
                            Some(held_number) => { return held_number; }
                            None => { }
                        }
                    }
                    None => { }
                }
            }
            _ => { }
        }
    }
    panic!("no result event carried the payload key [{key}]");
}

/// One candidate transaction of the append helper: fresh parts through the
/// domain constructor the importers must use (never a stored minor count).
#[cfg(test)]
fn transaction_of(date: &str, amount: &str, description: &str, source: &str) -> Transaction {
    match crate::domain::transaction::from_parts("", date, amount, "EUR", description, "", source) {
        Err(reason) => { panic!("the test's transaction is not buildable: {reason:?}"); }
        Ok(held) => held
    }
}

#[test]
fn a_missing_path_answers_one_error_event_of_the_io_code_naming_the_path() {
    // The plan's edge case, through the seam: the whole terminal event comes
    // from this work (`EndedWith`), an `error` of the documented `io` code
    // naming the path, and NOTHING else on the stream.
    let dir = scratch("a_missing_path_answers_one_error_event_of_the_io_code_naming_the_path".to_string());
    let mut work = ImportWork::new(dir.to_path_buf());
    let mut out = collecting();
    let missing = format!("{}/nowhere-to-be-found.pdf", dir.to_string_lossy());
    let answered = work.do_command(Command::Import(missing.to_string()), &mut out);
    scratch_release(&dir);
    assert!(answered == interpret::Answered::EndedWith);
    let events = events_of(&out.frames);
    assert!(kinds_of(&events) == ["error".to_string()].to_vec());
    assert!(codes_of(&events) == ["io".to_string()].to_vec());
    assert!(texts_of(&events)[0].contains(&missing));
}

#[test]
fn a_pre_cancelled_stream_stops_the_file_loop_before_any_progress() {
    // Amendment A6's unit-level pin of the cancel branch: the loop helper,
    // handed a stream the test has ALREADY cancelled, never starts a file —
    // no `progress`, no `result`, exactly one `cancelled` naming the command,
    // every counter at zero, and NO store file appears in the data directory.
    let source_dir = scratch("a_pre_cancelled_stream_stops_the_file_loop_before_any_progress_source".to_string());
    let data_dir = scratch("a_pre_cancelled_stream_stops_the_file_loop_before_any_progress_data".to_string());
    let file = source_dir.join("statement.pdf");
    match std::fs::write(&file, "Bank A AG -- nothing to import\n") {
        Err(_) => { }
        Ok(_) => { }
    }
    let host = importer_host::Host { importers: importer_host::builtin_importers() };
    let items = vec![file.to_path_buf()];
    let mut out = collecting();
    let mut stream = writer::start("import".to_string(), 1);
    writer::request_cancel(&mut stream);
    let mut store = StoreTexts::load(&data_dir);
    let mut counters = RunCounters::new();
    let completed = process_files(&data_dir, &host, &items, &mut stream, &mut out, &mut store, &mut counters);
    scratch_release(&source_dir);
    assert!(!completed);
    assert!(counters.added == 0);
    assert!(counters.annotations == 0);
    assert!(counters.linked == 0);
    let events = events_of(&out.frames);
    assert!(kinds_of(&events) == ["cancelled".to_string()].to_vec());
    match &events[0] {
        proto::event::Event::Cancelled(Some(name)) => { assert!(name == "import"); }
        _ => { assert!(false); }
    }
    // Nothing was written to the data directory at all: the store texts stayed
    // as loaded (empty) and no atomic write happened.
    match std::fs::metadata(&data_dir.join(TRANSACTIONS_FILE)) {
        Err(_) => { }
        Ok(_) => { assert!(false, "the cancelled run wrote a store file"); }
    }
    scratch_release(&data_dir);
}

#[test]
fn a_second_pass_of_the_append_helper_appends_nothing_and_keeps_the_pair() {
    // Decision 24 at the helper level: ONE pass over a byte-identical pair
    // yields the bare base AND the `base#2` occurrence row (the bank-B2 pin);
    // the SECOND pass over the RESULT text adds nothing, byte-for-byte.
    let first = transaction_of("2024-03-05", "-15,00", "REWE MARKT 12", "bank_b2.pdf");
    let second = transaction_of("2024-03-05", "-15,00", "REWE MARKT 12", "bank_b2.pdf");
    let other = transaction_of("2024-03-19", "-67,89", "AMAZON MARKETPLACE", "bank_b2.pdf");
    let pair = vec![first, second, other];
    let pass_one = match append_transaction_rows("", &pair) {
        Err(reason) => { panic!("the append of the pair failed: {reason:?}"); }
        Ok(held) => held
    };
    assert!(pass_one.1 == 3);
    let (rows, errors) = store_transactions::read(&pass_one.0);
    assert!(errors.len() == 0);
    assert!(rows.len() == 3);
    let base = id::transaction_base_id(&pair[0]);
    let mut saw_base = 0;
    let mut saw_second = 0;
    for row in rows.iter() {
        if row.id == base.to_string() { saw_base = saw_base + 1; }
        if row.id == format!("{base}#2") { saw_second = saw_second + 1; }
    }
    assert!(saw_base == 1);
    assert!(saw_second == 1);
    // The second import run: same candidates, this time against the text the
    // first pass produced — NOTHING is appended and the text is unchanged.
    let pass_two = match append_transaction_rows(&pass_one.0, &pair) {
        Err(reason) => { panic!("the re-append of the pair failed: {reason:?}"); }
        Ok(held) => held
    };
    assert!(pass_two.1 == 0);
    assert!(pass_two.0 == pass_one.0);
}

#[test]
fn an_empty_directory_import_ends_with_the_three_zero_counters_and_no_error() {
    // The plan's edge case: an EMPTY directory is a command that promises no
    // items, writes no `progress` line and answers the one `result` of its
    // own with the three counters (decision 23) all at zero.
    let source_dir = scratch("an_empty_directory_import_ends_with_the_three_zero_counters_and_no_error".to_string());
    let data_dir = scratch("an_empty_directory_import_ends_with_the_three_zero_counters_data".to_string());
    let source = source_dir.to_string_lossy().to_string();
    let mut work = ImportWork::new(data_dir.to_path_buf());
    let mut out = collecting();
    let answered = work.do_command(Command::Import(source), &mut out);
    scratch_release(&source_dir);
    assert!(answered == interpret::Answered::EndedWith);
    let events = events_of(&out.frames);
    assert!(kinds_of(&events) == ["result".to_string()].to_vec());
    assert!(counter_of(&events, "added") == 0);
    assert!(counter_of(&events, "annotations") == 0);
    assert!(counter_of(&events, "linked") == 0);
    scratch_release(&data_dir);
}

/// The `text` of every `log` event of one `level`, in order.
#[cfg(test)]
fn logs_of_level(events: &Vec::<proto::event::Event>, level: &str) -> Vec::<String> {
    let mut texts = Vec::<String>::new();
    for event in events.iter() {
        match event {
            proto::event::Event::Log(seen, text) => {
                if seen.as_str() == level {
                    texts.push(text.to_string());
                }
            }
            _ => { }
        }
    }
    return texts;
}

/// Whether any of the given texts holds `wanted` (a `log` event of the run is
/// one of several, and which one carries WHAT is the point of these pins).
#[cfg(test)]
fn holds_text(texts: &Vec::<String>, wanted: &str) -> bool {
    for text in texts.iter() {
        if text.contains(wanted) {
            return true;
        }
    }
    return false;
}

/// The mbox TEXT of a mailbox carrying one PayPal-receipt message per given
/// `Grund:` value: the fixture's shape (the exact `Subject:` of a receipt, the
/// `Betrag:`/`Grund:` lines of the body, an RFC 822 `Date:` header) so
/// [`crate::technical::mail_annotate`] has a fact per message to extract. One
/// `Grund:` per message keeps the annotation ids apart, which is what makes a
/// resume pin tell the messages apart.
#[cfg(test)]
fn mbox_of(shops: &Vec::<String>) -> String {
    let mut text = String::new();
    for shop in shops.iter() {
        text.push_str("From mail@example.com Tue Apr 02 12:00:00 2024\n");
        text.push_str("From: sender@example.com\n");
        text.push_str("To: someone@example.com\n");
        text.push_str("Subject: Your PayPal payment receipt\n");
        text.push_str("Date: 02 Apr 2024 12:00:00 +0100\n");
        text.push_str("Content-Type: text/plain; charset=UTF-8\n");
        text.push_str("\n");
        text.push_str(&format!("Betrag: -19,99 EUR\nGrund: {shop}\n"));
    }
    return text;
}

/// Give `path` an `mtime` from BEFORE the Unix epoch, the stamp [`file_stamp`]
/// refuses to turn into a whole number of seconds (a marker of 0/0 is the
/// failure this module must not write, and no test may contrive it by hand).
/// `false` when the filesystem refuses such an mtime, which the caller reports
/// instead of pretending the case was pinned.
#[cfg(test)]
fn set_pre_epoch_mtime(path: &std::path::Path) -> bool {
    let file = match std::fs::OpenOptions::new().write(true).open(path) {
        Err(_) => { return false; }
        Ok(held) => held
    };
    let one_century = match std::time::SystemTime::UNIX_EPOCH.checked_sub(std::time::Duration::from_secs(2_208_988_800)) {
        None => { return false; }
        Some(held) => held
    };
    let times = std::fs::FileTimes::new().set_modified(one_century);
    match file.set_times(times) {
        Err(_) => { return false; }
        Ok(_) => { return true; }
    }
}

#[test]
fn the_converter_missing_code_ends_only_a_run_that_appended_nothing() {
    // Amendment A4's run-level rule as the counters state it: the code ends a
    // run of which NOTHING landed, and never a run whose counters the store
    // already holds. (The per-file `warn` events name every failing importer
    // in both cases; the difference is the ONE terminal event.)
    let mut counters = RunCounters { added: 0, annotations: 0, linked: 0, failed_files: 1, converter_missing_files: 1 };
    assert!(converter_missing_terminates_the_run(&counters), "a run of which nothing was appended and every failure named the missing converter");
    counters.annotations = 3;
    assert!(!converter_missing_terminates_the_run(&counters), "mail annotations LANDED while every statement importer failed with ConverterMissing: their counters belong in the ONE terminal event, which is the result");
    counters.annotations = 0;
    counters.added = 1;
    assert!(!converter_missing_terminates_the_run(&counters), "a transaction landed: the run has a counter to report, not a converter complaint");
    counters.added = 0;
    counters.failed_files = 2;
    assert!(!converter_missing_terminates_the_run(&counters), "a failure of another kind is mixed in: not a pure converter complaint");
    counters.failed_files = 0;
    counters.converter_missing_files = 0;
    assert!(!converter_missing_terminates_the_run(&counters), "no importer failed at all");
}

#[test]
fn a_mailbox_of_an_unreadable_stamp_annotates_and_writes_no_marker_row() {
    // The finding at the caller of [`file_stamp`']: an unreadable stamp used
    // to be answered with (0,0), which appended a `0`/`0` marker — a version
    // no file matches, so `state::decide` would never skip the path again and
    // the mailbox would be re-read on every run. The annotations still land
    // (the idempotence of the annotation append is the mbox path's safety
    // net), the `warn` event says why, and NO marker row does.
    let dir = scratch("a_mailbox_of_an_unreadable_stamp_annotates_and_writes_no_marker_row".to_string());
    let file = dir.join("mail_unstamped.mbox");
    match std::fs::write(&file, &mbox_of(&vec!["SHOP ONLY".to_string()])) {
        Err(_) => { }
        Ok(_) => { }
    }
    if !set_pre_epoch_mtime(&file) {
        println!("this filesystem refuses an mtime before the Unix epoch, so no unreadable stamp could be built here; the case is not pinned on this machine");
        scratch_release(&dir);
        return;
    }
    // The scenario the finding names: the stamp reads, the clock in it is not
    // a number of seconds since the epoch.
    assert!(file_stamp(&file) == None);
    let host = importer_host::Host { importers: importer_host::builtin_importers() };
    let mut store = StoreTexts::load(&dir);
    let mut counters = RunCounters::new();
    let mut out = collecting();
    let mut stream = writer::start("import".to_string(), 1);
    process_one_file(&host, &file, &file_name_of(&file), &mut stream, &mut out, &mut store, &mut counters);
    scratch_release(&dir);
    assert!(counters.annotations == 1, "the annotation of the message still lands");
    let (rows, errors) = store_sidefiles::read_annotations(&store.annotations);
    assert!(errors.len() == 0);
    assert!(rows.len() == 1);
    assert!(rows[0].text.contains("SHOP ONLY"));
    // NOT A MARKER: no text at all, nothing marked dirty, so nothing flushed.
    assert!(store.markers.len() == 0, "an unreadable stamp must not write a marker, let alone one stamped mtime 0 and size 0: {:?}", store.markers);
    assert!(!store.markers_dirty);
    let events = events_of(&out.frames);
    let warnings = logs_of_level(&events, "warn");
    assert!(holds_text(&warnings, "its file stamp could not be read"), "the run tells the interface why: {warnings:?}");
    assert!(holds_text(&warnings, "no marker is written for it"), "and says what it does instead: {warnings:?}");
}

#[test]
fn a_grown_mailbox_annotates_only_the_messages_after_its_marker_offset() {
    // Plan item 4.8's step (e), the RESUME: the mbox path annotates the
    // messages that start AT OR AFTER the `offset` the marker recorded, through
    // `mbox::messages_from` — which [`mail_annotate::annotate_mbox`] runs at
    // that offset — it does NOT re-annotate the whole file and lean on the
    // append dedup. THE pin: `annotations.csv` starts EMPTY, so the message
    // BEHIND the marker's offset would leave a row of its own for a whole-file
    // re-read; dedup could not tell the two behaviours apart.
    let dir = scratch("a_grown_mailbox_annotates_only_the_messages_after_its_marker_offset".to_string());
    let file = dir.join("mail_grown.mbox");
    let text = mbox_of(&vec!["SHOP FIRST".to_string(), "SHOP SECOND".to_string()]);
    match std::fs::write(&file, &text) {
        Err(_) => { }
        Ok(_) => { }
    }
    let messages = mbox::split_messages(&text);
    assert!(messages.len() == 2);
    let first_end = messages[0].end;
    let path_text = file.to_string_lossy().to_string();
    let (mtime_now, size_now) = match file_stamp(&file) {
        None => { panic!("the test's mailbox must carry a readable stamp"); }
        Some(held) => held
    };
    let mut store = StoreTexts::load(&dir);
    // The marker of an EARLIER pass that read the FIRST message only: its stamp
    // (an older mtime, the size of back then) says CHANGED, so `state::decide`
    // reprocesses the path, and its `offset` says where this pass resumes.
    store.markers = format!("# columns: path;mtime;size;offset\n{path_text};{};{};{}\n", mtime_now - 1, first_end, first_end);
    let host = importer_host::Host { importers: importer_host::builtin_importers() };
    let mut counters = RunCounters::new();
    let mut out = collecting();
    let mut stream = writer::start("import".to_string(), 1);
    process_one_file(&host, &file, &file_name_of(&file), &mut stream, &mut out, &mut store, &mut counters);
    scratch_release(&dir);
    let (rows, errors) = store_sidefiles::read_annotations(&store.annotations);
    assert!(errors.len() == 0);
    assert!(rows.len() == 1, "only the message behind the offset is annotated, not the whole file: {rows:?}");
    assert!(rows[0].text.contains("SHOP SECOND"));
    assert!(counters.annotations == 1);
    // Decision 25 keeps its shape: the new marker covers what THIS pass read —
    // the `end` of the last message resume reached, i.e. the end of the file —
    // stamped with the version it read, and the older marker row stays
    // (append-only), last one winning on read.
    let (_, markers, warnings) = state::read_markers(&store.markers);
    assert!(warnings.len() == 0, "the marker text the run carries reads clean: {warnings:?}");
    assert!(markers.len() == 2);
    match state::marker_of(&markers, &path_text) {
        None => { assert!(false, "the reprocessed mailbox left no marker of its path"); }
        Some(held) => {
            assert!(held.mtime == mtime_now && held.size == size_now, "the new marker stamps the version this pass read: {held:?}");
            assert!(held.offset == size_now, "decision 25: offset is the end of the last message read, which covers the whole file: {held:?}");
        }
    }
}

#[test]
fn files_no_importer_accepts_leave_the_counters_at_zero_and_write_no_error() {
    // The second edge case: a file no importer claims is not a failure — one
    // `progress` line for it, no acceptance log, no `error`, and the same
    // three zero counters end the command.
    let source_dir = scratch("files_no_importer_accepts_leave_the_counters_at_zero_and_write_no_error".to_string());
    let data_dir = scratch("files_no_importer_accepts_leave_the_counters_at_zero_and_write_no_error_data".to_string());
    let note = source_dir.join("notes.txt");
    match std::fs::write(&note, "no importer reads plain text\n") {
        Err(_) => { }
        Ok(_) => { }
    }
    let source = source_dir.to_string_lossy().to_string();
    let mut work = ImportWork::new(data_dir.to_path_buf());
    let mut out = collecting();
    let answered = work.do_command(Command::Import(source), &mut out);
    scratch_release(&source_dir);
    assert!(answered == interpret::Answered::EndedWith);
    let events = events_of(&out.frames);
    assert!(kinds_of(&events) == ["progress".to_string(), "result".to_string()].to_vec());
    assert!(codes_of(&events).len() == 0);
    assert!(counter_of(&events, "added") == 0);
    assert!(counter_of(&events, "annotations") == 0);
    assert!(counter_of(&events, "linked") == 0);
    scratch_release(&data_dir);
}
