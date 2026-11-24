# Plan
Date: 2026-11-23
Goal: Implement Phase 4 (Import & Extraction, plan items 4.1–4.8) of Phin: PDF text helper, two bank-statement parsers, importer host (built-in + plugin executables), mbox reader, mail annotation, automatic linking, and the `import` command wired into the interpreter.
Plan format: detailed-v1

## Context (applies to every task)

- Goal / success shape: every plan item 4.1–4.8 of `doc/implementation/plan.md:193-235` is implemented, tested, and its checkbox ticked; `cargo test --workspace` green and `cargo build --workspace --all-targets` at 0 warnings; `import` demonstrable on the `testdata/` fixtures through the daemon protocol.
- Non-goals: report generation (Phase 5), UI verbs `set-category`/`confirm-annotation`/`list-pending` wiring (Phase 6), example plugin crates and plugin doc-reading tests (Phase 7, `plan.md:323-330` names items 3.1 and 7.5 for the doc tests), Thunderbird sqlite optimization (`architecture.md:53`), no plugin-protocol doc-reading test (belongs to 7.5).
- Architecture / conventions (facts from scout, all verified unless marked):
  - Source of truth: `doc/architecture/architecture.md`, `core-protocol.md`, `plugin-protocol.md`, `data-files.md` (per `AGENTS.md`). `doc/implementation/plan.md` lists tasks; ground rules at `plan.md:5-32`: one task at a time; tests mandatory in the same task; never touch real user data — fixtures under `testdata/` only (rule at `plan.md:20-21`); `///` doc comments on public items; docs updated in the task that changes a protocol/format (`plan.md:22-25`); commit per task, message `"<id>: <title>"`, and the commit ticks the checkbox in `doc/implementation/plan.md` (`plan.md:26`); stop and report if an architecture decision must change (`plan.md:27-28`).
  - `pdftotext -layout` decision: `architecture.md:42, 373, 430` (native `pdfium-render` is the recorded alternative; detect missing tool and report a clear error).
  - Import runtime view: `architecture.md:236-243` (probe every importer → extract from supporters → Data Store computes content-hash id, appends only unknown ids → progress events; classification not stored).
  - Mail as annotation (R002): `architecture.md:268-273` — mbox importer resumes from stored marker, yields annotations; automatic link on date/amount/counterparty with confidence 0…1 (`architecture.md:450`); low-confidence links stay pending; mail never creates a transaction.
  - Plugin protocol: `plugin-protocol.md:9-25` (plugins from configuration list of executables, one process per plugin per run, stdin/stdout JSON-Lines, probe-then-extract, failing plugin → `error` event naming the plugin, kill after configurable timeout default 60 s → `plugin-failed`), wire shapes `plugin-protocol.md:27-41, 43-64, 66-90, 92-103, 127-137`.
  - Data files: `data-files.md:10-27` CSV codec; `:26-41` id format (10 hex, `#N` occurrence suffix); `:60-75` `transactions.csv` columns `id;date;amount;currency;description;counterparty;source`; `:77-91` `annotations.csv` columns `id;ref;text;items;source;confidence`, empty `ref` = orphan/pending; `:139-144` `state/markers.csv` columns `path;mtime;size;offset`; `:146-153` config `phin.toml` in the data directory, literal keys `core.address/port/idle_timeout` plus conceptual "which plugins / which mail folders to scan" without literal names.
  - `import` command: `core-protocol.md:92-117` (`| import | path | Import one file … |`, example `{"v":1,"cmd":"import","path":"/data/statements/2024/bank_a.pdf"}`); events `core-protocol.md:119-170` (exactly one `result`/`error`/`cancelled` per command; `progress` example `{"v":1,"event":"progress","done":12,"total":30,"file":"bank_a.pdf","cmd":"import"}`; `log` example; error codes `unknown-command`, `plugin-failed`, `converter-missing`, `io`, `busy` at `:169-170`); cancellation `core-protocol.md:172-190` (finish current item, discard partial result, answer `cancelled`).
  - Handshake gating: handshake before any command (`core-protocol.md:57-61`); transport already gates it (Phase 3, `core/src/technical/transport.rs` serve path); carry-forward from Phase-3 impl log: "from 4.8 a peer that never handshakes could drive data-changing verbs" (`.agents/plans/2026-11-18_phase-3-core-protocol/02_impl.md:121`).
- Existing code to build on (all verified present; do not redo):
  - `core/src/technical/pdf_text.rs`, `parse_bank_a.rs`, `parse_bank_b.rs`, `importer_host.rs`, `mbox.rs`, `mail_annotate.rs`, `link.rs` do NOT exist. `core/src/technical/mod.rs:7-15` module list must gain a `pub mod` line in each task that adds a module.
  - Domain (Phase 1 done): `domain::transaction::from_parts(&str,&str,&str,&str,&str,&str,&str) -> Result<Transaction,BuildError>` at `core/src/domain/transaction.rs:89` — doc at `:67-68` says "this is the constructor the importers of plan item 4.x must use"; INVARIANT at `:83-88`: never pass a stored minor-unit count (`money::parse("-3999")` ⇒ `-399900`); `from_stored` at `:119` is for the store reader only. `domain::date::parse` accepts ONLY `DD.MM.YYYY` and `YYYY-MM-DD` (`core/src/domain/date.rs:40,30-38`). `domain::money::parse(text)->Result<i64,…>` at `core/src/domain/money.rs:33` (handles `1.234,56 €`, `-1234.56`, `1.000`; Phase-1 decision 13 pins amount disambiguation — read the module). `domain::annotation::Annotation{id, r#ref, text, items: Vec<String>, source, confidence: f64}` at `core/src/domain/annotation.rs:26-49`; `clamp_confidence` :56 (NaN→0.0, <0→0.0, >1→1.0); `from_parts` :73 (`|`-split items); `items_to_text` :95.
  - Store/technical (Phase 2 done): `technical::id`: `source_basename` `core/src/technical/id.rs:63` (source column is file NAME only), `base_id(date,amount,currency,description,counterparty,source)` :81 (payload `date|amount|currency|description|counterparty|source-basename`, SHA-256 first 10 hex), `transaction_base_id` :97, `occurrence_id(base,known_ids)` :126 (next suffix = one past highest occurrence on file), `sha256_hex` :190, `first_chars` :200. `technical::store_transactions::read(text)->(Vec<Transaction>,Vec<ReadError>)` :93 and `append(text,&Transaction)->Result<(String,AppendOutcome),CodecError>` :186. `technical::store_sidefiles`: `read_annotations` :96, `append_annotation` :194, `read_categories` :290. `technical::fs_atomic::write_atomic(path,content)->Result<BackupOutcome,WriteError>` :143. `technical::view::read_view(dir)->View` :138 (never writes). `technical::state`: `Marker{path,mtime,size,offset}` :29, `read_markers(text)` :65, `write_markers` :92, `marker_of` :107, `decide(markers,path,mtime,size)->ImportDecision{Skip,Reprocess}` :120 (Skip only if same mtime AND size), `append_marker(text,&Marker)->Result<(String,AppendOutcome),CodecError>` :143 — module is text-in/text-out, no I/O.
  - Protocol (Phase 3 done): `core/src/protocol::writer`: `EventSink{write_line}` :88, `start(cmd,total)->CommandStream` :234, `request_cancel` :247, `cancel_requested(&CommandStream)` :252, `progress(stream,sink,file:Option<String>)->Result<i64,WriteError>` :268, `log(stream,sink,level,text)` :291, `succeed(stream,sink,payload:&serde_json::Map<String,Value>)` :305 (payload = extra top-level fields), `fail(stream,sink,reason,text)` :317, `cancel(stream,sink)` :327; `writer::run`/`CommandWork` are TEST-ONLY and cannot express per-command payload — note at `core/src/protocol/interpret.rs:192-201` says widening belongs to 4.8/6.2. `core/src/protocol::interpret`: `Work{fn do_command(&mut self, Command, &mut dyn EventSink)->Answered}` :202-205; `Answered::{EndedWith, AnswerWith(ErrorReason,String)}` :178-187; `run` :281, `run_session` :303; `WiredNowhere` :654/`wired_nowhere()` :659; `import` arm refuses with `plugin-failed` and text naming "plan item 4.8" at :666-668; pinned test `every_command_of_the_table_is_refused_with_a_documented_code_naming_its_phase` at :1094-1133 (expects `codes_of(...) == ["plugin-failed","plugin-failed","plugin-failed","io","io","io","io"]` :1116 and message index 0 containing `4.8` :1121-1127); test helpers `scripted`, `collecting`, `kinds_of`, `codes_of`, `blamed_of`, `texts_of` at :720-943. `proto::command::Command::Import(String)` at `proto/src/command.rs:50-71`.
  - Transport/config: `core/src/technical::transport_cfg`: `CONFIG_FILE_NAME="phin.toml"` :102, `endpoint_from_config(&Path)->(Option<Endpoint>,Vec<ConfigIssue>)` :391, `idle_timeout_or_default` :263, `ConfigIssue` :226; only `core.address/port/idle_timeout` parsed today (`:105-125`). `core/src/main.rs:312-316` is the only production call of `interpret::run`, injecting `interpret::wired_nowhere()`. Integration tests use `CARGO_BIN_EXE_core` (`core/tests/transport_lifecycle.rs`), free ports, never 47623, scratch dirs `test-scratch/<name>`, child waits bounded.
- Fixtures (from `testdata/generate.py` / `testdata/README.md`; "Do not edit them by hand; regenerate instead"):
  - `pdftotext -layout testdata/bank_a.pdf -` gives: header `Bank A AG -- Account Statement`, table header `Date     Description         Amount (EUR)`, 6 rows `03.01.2024 REWE MARKT 12 -39,99 EUR`, `09.01.2024 DINNER ZUR LEITZ -59,90 EUR`, `15.01.2024 PAYMENT TO TOBIAS 420,00 EUR`, `21.01.2024 BOOKS ONLINE -18,47 EUR`, `24.01.2024 MEGA MART 04 -1.234,56 EUR`, `28.01.2024 LIDL 0815 -129,90 EUR`, then summary line `Total: -1.062,82 EUR`.
  - `bank_b1.pdf`: `Bank B N -- Monthly Statement`, rows `2024-02-03 4711-0000-4711 ALDI 4711 -89.90 EUR`, `2024-02-11 4711-0000-4711 TAXI AIRPORT; NIGHT -25.00 EUR`, `2024-02-17 4711-0000-4711 CLIENT PAYMENT 850.00 EUR`, `2024-02-24 4711-0000-4711 REWE MARKT 12 -34.56 EUR`. `bank_b2.pdf`: rows `2024-03-02 … CAFETARIA -15.90`, `2024-03-05 … REWE MARKT 12 -15.00` TWICE (adjacent, byte-identical), `2024-03-19 … AMAZON MARKETPLACE -67.89`.
  - `testdata/transactions.csv` pins expected ids: bank_a.pdf → `39945b2fed, f596357483, dc0db110c4, 3d7478d7cc, 2b5a9d2799, 7a8051207a`; bank_b1.pdf → `a86d11da9a, 140ef279a4, e86f3fc214, fbd45cccf1`; bank_b2.pdf → `58ed99aa4c, 80c63211a8, 80c63211a8#2, 5b35bef1fd`; mail_paypal.mbox rows exist but mail rows in `transactions.csv` are fixture-side only (R002: code must never add them — see T8/T9).
  - `mail_paypal.mbox`: 8 messages, `Subject: Your PayPal payment receipt`, Date headers `02/05/… Apr 2024 12:00:00 +0100`, multipart text/html with body lines `Betrag: <amount>, EUR` / `Grund: <what>`: `25,99 LEGO STORE DE`, `-19,99 NETFLIX.COM`, `8,99 BAKERY BERLIN`, `-45,00 STEAM PURCHASE`, `120,00 CLIENT PAYMENT`, `-7,99 BOOKS ONLINE`, `34,50 SUPERMARKET; DELICATES`, `-29,99 TAXI AIRPORT; NIGHT`. No `X-Mozilla-Status` header today (only named in `plan.md:217`).
  - `mail_with_pdf.mbox`: 12 messages; message 4 carries `Content-Type: application/pdf; name=bank_b1.pdf`, base64 attachment. Neither mbox contains a `>From ` escaped body line today.
  - Changing mbox/PDF bytes does not change any transaction id (source is only the basename); no test hard-codes PDF/mbox bytes; `id.rs:205-211` is the only disk-reading test (`testdata/transactions.csv`, 22 records).
- Build / test / lint commands:
  - `cargo build --workspace --all-targets` must be at 0 warnings; `cargo test --workspace` must be green (currently core lib 199, proto 33, all green at HEAD `fc6539e`).
  - Fixtures: `python3 testdata/generate.py [target-dir]` (deterministic, stdlib-only).
  - No rustfmt/clippy gate (tree is not rustfmt-formatted). No CI, no Makefile.
- Environment facts: `pdftotext` installed (`/usr/bin/pdftotext`, poppler 26.11.0); `python3` available (generate.py). Toolchain `cargo 1.100.0`. This machine's repo root is the CWD.
- Global decisions (bind all tasks):
  - G1 (user): the ~150-LOC task cap of `plan.md:12-13` no longer binds (user, 2026-11-23); tasks are sized by content, and Phase 4 is split at 4.4a/4.4b for content reasons.
  - G2 (user): `import` `path` may be a FILE or a DIRECTORY (Q1=B). Directory ⇒ core walks it recursively (sorted by path) and feeds every file to probe/extract. `core-protocol.md:99` and its example must be amended in T9 with a `<!-- test-example -->`.
  - G3 (user): plugin discovery configures from a new literal `phin.toml` key `plugins` (array of executable path strings), documented in `data-files.md` in T5.
  - G4 (user): mbox fixtures are extended in `testdata/generate.py` (X-Mozilla-Status headers + escaped `>From ` body line) and regenerated (T6).
  - G5 (user): link numbers — unique match ⇒ confidence 0.9 and linked; ambiguous ⇒ 0.4 per candidate, kept pending (empty `ref`, both candidates returned typed); orphan ⇒ 0.0; date window ±3 days; amount must match exactly; counterparty = case-insensitive substring containment.
  - G6: conventions carried from Phases 1–3: no `std::fs`/`std::net` text under `core/src/domain` (purity test `core/src/technical/mod.rs:56,85`) — all Phase 4 I/O lives in `core/src/technical/`; typed errors, no panics/`as` casts; `///` doc comments; inline `#[test]` tests, scratch dirs `test-scratch/<name>`; decode event frames with `proto::codec::decode_line`, never compare bytes; error codes fixed to the five documented ones.
  - G7: each task ticks its checkbox in `doc/implementation/plan.md` in the same commit (`plan.md:26`); commit message `"<id>: <title>"` where id is `4.1`…`4.8` (4.4a/4.4b for T5).
  - G8: never read real user mail data; only `testdata/` fixtures and `test-scratch/` (`plan.md:20-21`).
- Definitions: task = one deliverable with one success check; brief = global context + one task section; plan item N M of `doc/implementation/plan.md`; "host" = the Import & Extraction block of `architecture.md:206-208`.

## Task index

| id | title | depends on | files | success check | size |
|----|-------|------------|-------|---------------|------|
| T1 | 4.1 `pdftotext -layout` call helper | — | `core/src/technical/pdf_text.rs` (new), `core/src/technical/mod.rs` | `cargo test -p core pdf_text` green; `cargo build --workspace --all-targets` 0 warnings | 1 file (~90 LOC) + mod line |
| T2 | 4.2 Bank statement parser (bank A) | T1 | `core/src/technical/parse_bank_a.rs` (new), `core/src/technical/mod.rs` | `cargo test -p core parse_bank_a` green: 6 fixture txs with exact ids/dates/amounts; broken variant warns yet yields rows; twice identical | 1 file (~130 LOC) + mod line |
| T3 | 4.3 Bank statement parser (bank B) | T2 | `core/src/technical/parse_bank_b.rs` (new), `core/src/technical/mod.rs` | `cargo test -p core parse_bank_b` green incl. `#2` pair from `bank_b2.pdf` | 1 file (~130 LOC) + mod line |
| T4 | 4.4a Importer contract + built-in importers + host orchestration | T2, T3 | `core/src/technical/importer_host.rs` (new), `core/src/technical/mod.rs` | `cargo test -p core importer_host` green: probe-then-extract orchestration with in-memory importers | 1 file (~150 LOC) + mod line |
| T5 | 4.4b Plugin-executable host + `plugins` config key | T4 | `core/src/technical/importer_host.rs` (extend), `core/src/technical/transport_cfg.rs` (extend), `core/src/technical/mod.rs` (if new module), `doc/architecture/data-files.md` | `cargo test -p core` green incl. fake-plugin tests (probe-false never extracted, non-zero exit named, timeout kill) and config test | 2 files + docs + fixture scripts |
| T6 | 4.5 mbox reader (+ fixture extension) | — | `core/src/technical/mbox.rs` (new), `core/src/technical/mod.rs`, `testdata/generate.py`, regenerated `testdata/mail_paypal.mbox`, `testdata/mail_with_pdf.mbox`, `testdata/README.md` | `cargo test -p core mbox` green: 8/12 messages, `>From` no-split, attachment bytes, offset resume; `python3 testdata/generate.py` deterministic | 1 file (~140 LOC) + generator |
| T7 | 4.6 Mail annotation extraction | T1, T6 | `core/src/technical/mail_annotate.rs` (new), `core/src/technical/mod.rs` | `cargo test -p core mail_annotate` green: PayPal annotation with items; invoice attachment items; confidence carried; return type carries no `Transaction` | 1 file (~130 LOC) + mod line |
| T8 | 4.7 Automatic annotation → transaction linking | T7 | `core/src/technical/link.rs` (new), `core/src/technical/mod.rs` | `cargo test -p core link` green: unique ⇒ 0.9; two candidates ⇒ 0.4 pending both recorded; none ⇒ orphan 0.0; window boundaries | 1 file (~120 LOC) + mod line |
| T9 | 4.8 `import` command wired into interpreter | T4, T5, T6, T8 | `core/src/technical/importer_host.rs` (run loop) or `core/src/protocol/import_work.rs` (new), `core/src/protocol/mod.rs`, `core/src/main.rs`, `core/src/protocol/interpret.rs` (only if seam widened), `doc/architecture/core-protocol.md` | integration test green: directory import appends exactly the expected transactions/annotations, second import appends nothing, cancel leaves files valid | 2-4 files (~150-250 LOC) |

## Tasks

### T1 — 4.1 `pdftotext -layout` call helper

**Depends on:** — (plan 0.3 fixtures exist)
**Size:** one new file, ~90 LOC; well under any budget.
**Why:** plan item 4.1 (`plan.md:195-202`): the only way Phase 4 reads PDF text; the recorded `pdftotext` decision (`architecture.md:42,373`) must be contained in one module so the `pdfium` alternative could swap it.
**Current state (verified):** no `pdf_text.rs` exists (`core/src/technical/mod.rs:7-15`). `/usr/bin/pdftotext` (poppler 26.11.0) present. Fixture PDFs `testdata/bank_a.pdf` etc. exist.
**Already done / do not redo:** nothing; do not touch fixtures or other modules besides adding the `pub mod pdf_text;` line to `core/src/technical/mod.rs`.
**Files:**
- Create: `core/src/technical/pdf_text.rs`
- Modify: `core/src/technical/mod.rs` (add `pub mod pdf_text;`)
- Read first: `core/src/technical/id.rs` (style), `core/src/technical/state.rs` (module-doc style)

**Interfaces:**
- Consumes: nothing besides `std::process::Command`.
- Produces (exact):
  - `pub enum PdfTextError { ToolMissing, Io(String), Failed(String) }` (derive `Debug, Eq, PartialEq`; `Failed` carries captured stderr).
  - `pub fn extract_text(path: &Path) -> Result<String, PdfTextError>` — convenience calling the inner fn with program `"pdftotext"`.
  - `pub fn extract_text_with(program: &str, path: &Path) -> Result<String, PdfTextError>` — runs `<program> -layout <path-as-posix-path> -`, captures stdout as UTF-8 (lossy); non-zero exit ⇒ `Failed(stderr text)`; spawn error whose kind is `NotFound` ⇒ `ToolMissing`; other spawn/IO errors ⇒ `Io`.
  - Text is returned VERBATIM (form feeds between pages kept) so column layout is preserved for the parsers.

**Implementation brief:**
1. In `pdf_text.rs`, implement `extract_text_with` with `std::process::Command::new(program).arg("-layout").arg(path).arg("-")` and `output()`; never call `unwrap`; map errors to the typed enum.
2. `extract_text` calls `extract_text_with("pdftotext", path)` (document it).
3. `///` doc comments: record the `pdftotext -layout` decision reference (`architecture.md:42`) and that the alternative is `pdfium-render`.

**Pinned decisions:** D-loc (global G1); D-inject (program name injectable for tests — decision 6 below).
**Edge cases / failure modes:** missing tool → `ToolMissing`, not panic; empty (blank) PDF → exit 0, empty stdout → `Ok("")`; non-existent file → `Failed` (pdftotext exits non-zero), not a panic; two-column/label-value alignment must survive because `-layout` preserves columns.
**Tests / acceptance criteria:** inline `#[test]` (purity: this file may use std::process — it is technical):
  - `fixture_bank_a_text_in_reading_order`: `extract_text` on `testdata/bank_a.pdf` (locate via the `find_testdata_dir()` pattern of `id.rs:205-211`, copy it private) → text contains `"Bank A AG"`, and the line containing `MEGA MART` contains `"-1.234,56"` (number attached to its row).
  - `missing_tool_is_typed_error`: `extract_text_with("definitely-not-pdftotext-xyz", …)` → `Err(PdfTextError::ToolMissing)`.
  - `empty_pdf_yields_empty_result`: write the hand-crafted blank one-page PDF bytes below to `test-scratch/pdf_text/blank.pdf`; assert `extract_text` returns `Ok(t)` where `t.trim().is_empty()` (form feed allowed). Required byte shape (write as a byte-string constant and regenerate — must be a syntactically valid PDF without a content stream):
    `%PDF-1.4` + objects: Catalog→Pages, Pages(Kids[page],Count 1), Page with `/MediaBox[0 0 300 300]`, minimal `xref`/`trailer`/`%%EOF` (pdftotext 26 accepts this shape — scout verified nothing here: if pdftotext rejects it, the test must build the blank PDF by invoking `python3 -c` stdlib writer? NO — keep the constant; if it fails, report, do not change fixture policy).
  - `nonexistent_file_is_failed`: `extract_text` on missing path → `Err(PdfTextError::Failed(_))`.
**Success check:** `cargo test -p core pdf_text` green + `cargo build --workspace --all-targets` 0 warnings.
**Review focus:** error typing (no panics), program injection point, verbatim text (no trimming inside the library fn), doc comments.
**Out of scope:** no caching, no "detect once" run-state (the host of 4.4 logs once; see T4), no pdfium.
**References:** `plan.md:195-202`, `architecture.md:42,373,430`.

### T2 — 4.2 Bank statement parser (bank A)

**Depends on:** T1 (consumes `pdf_text::extract_text`)
**Size:** one new file ~130 LOC.
**Why:** plan item 4.2 (`plan.md:203-207`): built-in importer for fixture bank A; the second of the two built-in importers per `architecture.md:206-208`.
**Current state (verified):** `pdf_text::extract_text` from T1. `date::parse` accepts `DD.MM.YYYY` (`domain/date.rs:40,30-38`). `money::parse` handles German `1.234,56` (`domain/money.rs:33`); read the whole money module for the Phase-1 disambiguation rule (decision 13, `.agents/plans/2026-11-14_phase-1-domain-model/02_impl.md`). `transaction::from_parts(id,date,amount_str,currency,description,counterparty,source)` (`domain/transaction.rs:89`) — never pass a minor-unit count as text (INVARIANT at `:83-88`). `id::base_id` (`technical/id.rs:81`) hashes `date|amount|currency|description|counterparty|source-basename`. Fixture text (see Context) with 6 rows; expected ids pinned in `testdata/transactions.csv`.
**Already done / do not redo:** do not re-implement money/date/id; do not add currency symbols inside the parser (strip before `money::parse`).
**Files:**
- Create: `core/src/technical/parse_bank_a.rs`
- Modify: `core/src/technical/mod.rs` (`pub mod parse_bank_a;`)
- Read first: `core/src/domain/money.rs`, `core/src/domain/transaction.rs`, `core/src/technical/id.rs`

**Interfaces:**
- Consumes: `pdf_text::extract_text`, `date::parse`, `money::parse`, `transaction::from_parts`, `id::base_id`, `id::transaction_base_id`.
- Produces (exact):
  - `pub struct BankRowWarning { pub line: usize, pub text: String }` (derive Debug, Eq, PartialEq).
  - `pub struct ParseReport { pub transactions: Vec<Transaction>, pub warnings: Vec<BankRowWarning> }`.
  - `pub fn parse_bank_a_text(text: &str, source: &str) -> ParseReport` — pure, text-in.
  - `pub fn parse_bank_a(path: &Path, source: &str) -> Result<ParseReport, PdfTextError>` — wrapper calling `pdf_text::extract_text`.
  - `source` is the provenance string (file name); `id::source_basename` normalizes later, but pass the basename in production.
  - Each returned `Transaction` carries the provisional base id from `id::transaction_base_id` (occurrence suffix `#N` is assigned at append time in 4.8, not here).

**Implementation brief:**
1. Split text into lines; a line is a candidate row iff it starts with `\d{2}\.\d{2}\.\d{4}`; the statement header/table/summary lines (anything else) are skipped silently. Recognize the statement by first line containing `Bank A AG` — if absent, add warning `BankRowWarning{line:0,text:"not a Bank A statement"}` and still parse candidate rows (the broken-variant requirement).
2. On a candidate row: split off the leading date; strip trailing `" EUR"` (case-sensitive `EUR`; also tolerate `€`); the remaining amount token is the last whitespace-separated token before `EUR` — pass it to `money::parse`; middle text = description.
3. Counterparty: for this fixture set, description is the merchant text; set `counterparty` = description (uppercase form as printed; keep both identical — no smarter heuristic; pin).
4. Normalize date via `date::parse` → `YYYY-MM-DD`; amount string via `money::parse` → re-render with `money::print` before `from_parts`? NO — `from_parts` takes a TEXT amount: pass the ORIGINAL amount token to `money`-compatible text only if it round-trips; else warn. Simpler pinned rule: pass `money::print(money::parse(token))` output into `from_parts` (canonical text ⇒ canonical minor units).
5. Unparsable amount/date on a candidate row ⇒ push `BankRowWarning { line (1-based), text }`, keep going; never panic.
6. Build `Transaction::from_parts(base_id, iso_date, canonical_amount_text, "EUR", description, counterparty, source)`; `BuildError` ⇒ warning, not abort.

**Pinned decisions:** provisional base id without `#N` (decision 7); counterparty == description (decision 18).
**Edge cases / failure modes:** broken variant text where one row has `FOO,BAR EUR` → 5 transactions + 1 warning; header/summary lines never produce transactions (`Total: -1.062,82 EUR` line starts with `Total`, not a date, so it is skipped); running the parser twice on the same text returns byte-identical results (pure function).
**Tests / acceptance criteria (inline `#[test]`):**
  - `fixture_a_yields_exact_transactions`: parse `pdf_text::extract_text("testdata/bank_a.pdf")` → `transactions.len()==6`; amounts in order `[-3999,-5990,42000,-1847,-123456,-12990]`; dates `["2024-01-03","2024-01-09","2024-01-15","2024-01-21","2024-01-24","2024-01-28"]`; ids == `["39945b2fed","f596357483","dc0db110c4","3d7478d7cc","2b5a9d2799","7a8051207a"]` (same order as in `testdata/transactions.csv` — verify order at implementation time against the CSV and assert membership if order differs); descriptions per fixture rows.
  - `broken_variant_warns_and_keeps_rows`: inline text = valid A rows with one corrupted amount row → `transactions.len()==5`, `warnings.len()==1`, warning text non-empty.
  - `twice_identical`: parse the same text twice → equal `transactions` (assert_eq on Vec<Transaction>).
  - `two_column_number_stays_on_row`: an inline row with wide spacing like `03.01.2024 REWE MARKT 12                -39,99 EUR` ⇒ amount -3999.
**Success check:** `cargo test -p core parse_bank_a` green; workspace build 0 warnings.
**Review focus:** never invent ids/occurrence suffixes; no money re-scaling (`from_parts` text only); warnings typed, no panic path; fixture ids asserted exactly.
**Out of scope:** no store writes, no host, no `transactions.csv` edits.
**References:** `plan.md:203-207`; fixture text quoted in Context.

### T3 — 4.3 Bank statement parser (bank B)

**Depends on:** T2 (shares shape and decisions; separate file per plan)
**Size:** one new file ~130 LOC.
**Why:** plan item 4.3 (`plan.md:208-209`): second fixture layout; proves one parser per bank (Q3 scenario `architecture.md:421`).
**Current state (verified):** fixture rows (Context): dates `YYYY-MM-DD` (already canonical), amounts `-89.90 EUR` (point decimals), card-number column `4711-0000-4711` between date and description; `bank_b2.pdf` has two adjacent byte-identical `REWE MARKT 12 -15.00` rows. Expected ids: b1 `a86d11da9a,140ef279a4,e86f3fc214,fbd45cccf1`; b2 `58ed99aa4c,80c63211a8,80c63211a8#2,5b35bef1fd` (id column of `testdata/transactions.csv`). `money::parse` handles `-1234.56`-style text (read module; verify `-89.90` case in test).
**Already done / do not redo:** T2's shape; do not parameralize T2 into a shared "generic parser" (each bank is its own importer — `architecture.md:435`: one importer per bank).
**Files:**
- Create: `core/src/technical/parse_bank_b.rs`
- Modify: `core/src/technical/mod.rs` (`pub mod parse_bank_b;`)

**Interfaces:**
- Consumes: as T2.
- Produces (exact): same shapes as T2 but `pub fn parse_bank_b_text(text: &str, source: &str) -> ParseReport` and `pub fn parse_bank_b(path: &Path, source: &str) -> Result<ParseReport, PdfTextError>`; reuses `parse_bank_a::BankRowWarning`? NO — define its own `pub struct BankRowWarning` in this module? NO: reuse `parse_bank_a::{BankRowWarning, ParseReport}` via `use crate::technical::parse_bank_a::{...}` — pin: single warning/report types live in `parse_bank_a` and `parse_bank_b` imports them.

**Implementation brief:**
1. Candidate row: starts with `\d{4}-\d{2}-\d{2}`; skip header/summary; statement marker line must contain `Bank B N` else warning at line 0 (same policy as T2).
2. Layout per fixture: `DATE 4711-0000-4711 DESCRIPTION AMOUNT EUR` — card-number token (matches `^\d{4}(-\d{4}){2}$`) is consumed and dropped (not part of description); description = text between card number and amount; amount = token before `EUR`; canonicalization as in T2 (money::print of money::parse).
3. Counterparty = description (same pinned rule as T2).
4. Duplicate rows must BOTH survive parsing (dedup happens at append; parser keeps occurrences in order).
5. Warning/BuildError policy identical to T2.

**Pinned decisions:** decision 18 (counterparty=description); card token dropped from description; parser keeps duplicates (dedup is 4.8/store).
**Edge cases / failure modes:** `TAXI AIRPORT; NIGHT` description contains `;` (must not split further — it is inside the description text); byte-identical adjacent rows yield two equal transactions (same provisional base id, no suffix assigned by the parser).
**Tests / acceptance criteria:**
  - `fixture_b1_exact`: `parse_bank_b_text(extract_text(bank_b1).unwrap(), "bank_b1.pdf")` → 4 txs, amounts `[-8990,-2500,85000,-3456]`, dates `2024-02-*`, provisional ids `["a86d11da9a","140ef279a4","e86f3fc214","fbd45cccf1"]`, description of row 2 == `"TAXI AIRPORT; NIGHT"`.
  - `fixture_b2_duplicate_pair_kept`: b2 text → 4 txs; provisional ids `["58ed99aa4c","80c63211a8","80c63211a8","5b35bef1fd"]` (NO `#2` from the parser — assert no `#` in any id).
  - `broken_variant_warns_and_keeps_rows` and `twice_identical`: as T2.
**Success check:** `cargo test -p core parse_bank_b` green.
**Review focus:** no dedup in parser; no shared generic parser with bank A; exact id assertions.
**Out of scope:** store, host, config.
**References:** `plan.md:208-209`.

### T4 — 4.4a Importer contract + built-in importers + host orchestration

**Depends on:** T2, T3
**Size:** one new file ~150 LOC; in-process only — subprocess work deliberately lives in T5.
**Why:** plan item 4.4 first half (`plan.md:210-215`): the host framework of `architecture.md:206-208` ("probe every configured importer, then extract from the ones that accepted it"); wraps the built-ins behind one contract so plugin importers (T5) plug into the same loop.
**Current state (verified):** `parse_bank_a`, `parse_bank_b` (T2/T3). Built-in probing must be content-based — no filename convention exists anywhere (scout: grep found none). Split rationale per user (Q5/G1): contract+orchestration vs executable-process host are separately reviewable.
**Already done / do not redo:** do not spawn processes here; mbox built-in registers in T6/T9 (module does not exist yet) — pin the trait so `mbox` can implement it later.
**Files:**
- Create: `core/src/technical/importer_host.rs`
- Modify: `core/src/technical/mod.rs`

**Interfaces:**
- Consumes: `parse_bank_a`, `parse_bank_b`, `pdf_text`.
- Produces (exact; consumed by T5, T6, T9):
  - `pub enum ProbeAdvice {}`? NO — `pub const KIND_BANK_STATEMENT: &str = "bank-statement";` (`plugin-protocol.md:49` request kind) and dialect names `"bank_a"`, `"bank_b"`, `"mbox"`.
  - `pub enum ProbeKind { Dialect(String), Unsupported }` (derive Debug, Eq, PartialEq).
  - `pub trait Importer { fn name(&self) -> String; fn probe(&self, path: &Path) -> ProbeKind; fn extract_transactions(&self, path: &Path) -> Result<Extracted, ImportError>; }` where `pub struct Extracted { pub transactions: Vec<Transaction>, pub warnings: Vec<String> }` and `pub enum ImportError { Unreadable(String), Failed(String) }` (derive Debug, Eq, PartialEq). Object-safety required (`Box<dyn Importer>`).
  - `pub struct BuiltInBankA;` / `pub struct BuiltInBankB;` `impl Importer for …`: name `"built-in-bank-a"` / `"built-in-bank-b"`; probe: `pdf_text::extract_text` then first line contains `Bank A AG` ⇒ `ProbeKind::Dialect("bank_a")` (likewise `Bank B N` ⇒ `"bank_b"`; each built-in only accepts ITS marker — a `bank_b` file gets `Unsupported` from bank A). extract: call its parse fn with `id::source_basename(path)`, map warnings to `Extracted.warnings`.
  - `pub struct Host { pub importers: Vec<Box<dyn Importer>> }`
  - `pub fn builtin_importers() -> Vec<Box<dyn Importer>>` (bank A, bank B; mbox added by T6 — pin: `builtin_importers` must live in `importer_host` and T6 appends the mbox entry there).
  - `pub struct FileOutcome { pub accepted_by: Vec<(String, String /*dialect*/)>, pub transactions: Vec<Transaction>, pub warnings: Vec<String>, pub failed: Vec<(String, ImportError)> }`
  - `pub fn import_file(host: &Host, path: &Path) -> FileOutcome` — probe EVERY importer, collect supporters; for each supporter `extract_transactions`; merge transactions; failures recorded in `failed` (other importers keep running).

**Implementation brief:**
1. Define the contract exactly as pinned (object-safe).
2. Built-ins as pinned; probe must be cheap first (extension check `.pdf` case-insensitive AND content marker; if not `.pdf` ⇒ `Unsupported` without reading).
3. `import_file` returns no events; event emission is 4.8's job — the host is testable without a daemon (plan ground rule "Test the domain, not the process").
4. Order preserved: probe in `host.importers` order; a file supported by two importers extracts from both (duplicates possible; store dedups later).
5. `builtin_importers()` order: `built-in-bank-a`, `built-in-bank-b`, then (T6) mbox.

**Pinned decisions:** decision 13 (content-marker probing), decision 14 (host emits no events; 4.8 owns events).
**Edge cases / failure modes:** probe itself failing (missing pdftotext) ⇒ `ProbeKind::Unsupported` and a swallowed error? NO: probe returns `Unsupported` on ANY read error (missing tool must be detectable once — 4.8 logs `ToolMissing` explicitly on extract); non-PDF file ⇒ both built-ins `Unsupported` ⇒ empty `FileOutcome` (no error).
**Tests / acceptance criteria (inline, using in-memory importers):**
  - `bank_a_file_picked_by_bank_a_only`: `import_file(&Host{builtin_importers()}, testdata/bank_a.pdf)` → `accepted_by == [("built-in-bank-a","bank_a")]`, 6 txs, bank_b built-in absent from accepted_by.
  - `broken_importer_does_not_break_run`: custom `FailImporter{probe⇒Dialect("x"), extract⇒Err(Failed)}` + a `StubImporter` (static text import via `parse_bank_a_text` on embedded text) both supporting: `failed.len()==1`, stub's txs still present.
  - `probe_false_never_extract`: a stub whose `probe` ⇒ `Unsupported` and whose `extract` increments a counter — counter stays 0 (use `Rc<Cell<usize>>` or return via interior mutability).
  - `builtin_importers_order_is_stable`: first two names exact.
**Success check:** `cargo test -p core importer_host` green.
**Review focus:** object safety, probe never panics on junk files, no event/interpreter knowledge, order determinism.
**Out of scope:** no processes (T5), no config (T5), no store/interpreter (T9).
**References:** `plan.md:210-215`, `architecture.md:206-208,175`, `plugin-protocol.md:18-19`.

### T5 — 4.4b Plugin-executable host + `plugins` config key

**Depends on:** T4
**Size:** extends `importer_host.rs` (~150 LOC), `transport_cfg.rs` (~40 LOC), one fake-plugin script + docs; content split from T4 per user Q5/G1.
**Why:** plan item 4.4 second half: plugin importers as external processes per `plugin-protocol.md:9-25`; user decision G3: literal `plugins` key in `phin.toml`.
**Current state (verified):** `transport_cfg` parses only `core.address/core.port/core.idle_timeout` (`:105-125`); `basic_toml` dep exists in `core/Cargo.toml`; `plugin-protocol.md:43-64,66-90` wire shapes; `:92-103` annotation variant exists but built-in mbox (T6/T7) uses it internally — plugin-side annotation wire is exercised only when a plugin emits it: pin that T5 decodes BOTH `transactions` and `annotation` events from plugins (annotation variant needed for completeness, `plugin-protocol.md:92-103`); no doc test for `plugin-protocol.md` exists (that is 7.5 — out of scope). `python3` available (precedent `testdata/generate.py`).
**Already done / do not redo:** `proto::codec` is for the CORE protocol frames; plugin lines are simpler JSON-Lines — use `serde_json` directly (precedent: `basic_toml`+`serde_json` in transport_cfg).
**Files:**
- Modify: `core/src/technical/importer_host.rs` (add `ExecImporter` + plugin framing)
- Modify: `core/src/technical/transport_cfg.rs` (`plugins_from_config`)
- Create: `core/tests/fixtures/fake_importer.py` (test-only executable script; NOT registered as a cargo target)
- Modify: `doc/architecture/data-files.md` (configuration section: add literal key `plugins` — array of executable path strings; relative paths resolved against the data directory; mark as "Fixed here (plan item 4.4b)" like decision 8/40 precedent)
- Read first: `core/src/technical/transport_cfg.rs` (ConfigIssue style, `endpoint_from_config` pattern :391), `doc/architecture/plugin-protocol.md`

**Interfaces:**
- Consumes: T4 `Importer` trait, `ProbeKind`, `ImportError`, `Extracted`.
- Produces (exact):
  - `pub struct ExecImporter { pub argv: Vec<String>, pub name: String, pub timeout_secs: u64 }` `impl Importer`: probe: spawn once per probe-call? NO — "one process per plugin per run" (`plugin-protocol.md:12-15`): pin "run" = one `ExecImporter` instance handles one file; spawn at `probe`, keep stdin alive, reuse in `extract_transactions` for the same file, child killed at drop. Write `{"v":1,"cmd":"probe","file":"<path>","kind":"bank-statement"}` line; read one line; decode `{"v":1,"event":"probe","supported":bool,"kind":?,"name":?}` ⇒ `ProbeKind::Dialect(kind)` or `Unsupported`; then `{"v":1,"cmd":"extract","file":"<path>","kind":"<reported kind>"}`; read lines until `{"v":1,"event":"end"}`; each `{"v":1,"event":"transactions",...}` maps onto the wire object of `plugin-protocol.md:27-41` and is built with `transaction::from_parts` (ignore any `id` field on the line — `plugin-protocol.md:39-41` — but compute `id::base_id` for the built-in-less plugin anyway? pin: the CORE computes ids: set the transaction's id to `id::transaction_base_id` after construction, discarding any sent id).
  - Timeout: `timeout_secs` (host passes default 60, `plugin-protocol.md:24-25`); implement bounded read: `wait_timeout` thread pattern or `StdCommand::spawn` + poll of `try_wait` with deadline (no external crate); on expiry kill child and return `ImportError::Failed("timed out after N s")`.
  - `pub enum ConfigIssue` extension in `transport_cfg`: `pub fn plugins_from_config(data_dir: &Path) -> (Vec<String>, Vec<ConfigIssue>)` — reads `phin.toml` key `plugins` (`plugins = ["./importers/bank-x"]`), array of strings; missing key/file ⇒ `(vec![], vec![])`; non-array or non-string entries ⇒ push existing-style `WrongType`? add new variant `PluginsNotStrings(String)` to `ConfigIssue` (enum is public; adding a variant is additive).
  - `pub fn exec_importers(data_dir: &Path) -> (Vec<Box<dyn Importer>>, Vec<ConfigIssue>)` in `importer_host` — calls `plugins_from_config`, wraps each as `ExecImporter` (name = `id::source_basename` of the configured path? NO — pin: name = configured path string; probe response `name` is only logged).

**Implementation brief:**
1. `ExecImporter`: child stdin/stdout pipes (`Stdio::piped`); write request lines with `serde_json::Map` (insert `v`, `cmd`, `file`, `kind` — key order lexicographic is fine, decode on assert).
2. Read plugin stdout with `BufReader.lines()`; a malformed/invalid line ⇒ `ImportError::Failed("invalid line from <name>")`; non-zero exit mid-stream ⇒ `ImportError::Failed` naming the plugin (`plugin-protocol.md:20-23`); `{"v":1,"event":"error",...}` from plugin ⇒ `ImportError::Failed` too.
3. Kill-on-timeout: bounded reads — pin implementation: reader thread sends lines over `std::sync::mpsc::Receiver::recv_timeout(Duration::from_secs(timeout_secs))`; on timeout `child.kill()`+`wait()` and fail.
4. Unknown `cmd` handling on plugin side is the plugin's duty (`plugin-protocol.md:129-137`); the core just reads its lines.
5. `plugins_from_config` in `transport_cfg` following the `basic_toml::from_str::<serde_json::Value>` pattern of `endpoint_from_config` :391; document the new key in `data-files.md` config section AND note resolution of relative paths against the data directory.
6. In `exec_importers`, join relative paths with the data dir; keep absolute paths.

**Pinned decisions:** G3 (`plugins` key), D-python-fake (decision 12: fake plugin is a `python3` script), one-process-per-file-run (decision 15), timeout default 60 s (decision 11), core computes ids discarding wire id (decision 16).
**Edge cases / failure modes:** plugin answers `probe:false` ⇒ never written an `extract` (test); plugin exits non-zero ⇒ `failed` entry names it, other importers still run (host already runs them — T4 `import_file` behavior); timeout kills the child and does not hang the test (bounded); config `plugins` non-string ⇒ typed `ConfigIssue`, no panic; child that closes stdout early ⇒ `Failed`.
**Tests / acceptance criteria:**
  - `fake_plugin_probe_false_never_extracts`: python3 script fixture prints `{"v":1,"event":"probe","supported":false}` then reads forever; assert its stderr/marker file (script writes a file when extract received) never created.
  - `fake_plugin_nonzero_exit_names_plugin_others_run`: script exits 1 after probe true; `FileOutcome.failed[0]` text contains plugin name; stub built-in still extracted.
  - `fake_plugin_timeout_killed`: script sleeps 30 s; `ExecImporter{timeout_secs:1,...}` → `ImportError::Failed` containing `"timed out"`, wall time < 5 s.
  - `fake_plugin_transactions_decoded`: script emits two `transactions` lines + `{"v":1,"event":"end"}`; decoded tx amounts/dates exact; decoded transaction id == core-computed `id::base_id(...)` regardless of wire `id` (script sends a wrong `"id":"deadbeef"`? — plugin lines have no id field; instead script omits id and core fills it).
  - `plugins_from_config_*`: `test-scratch/…/phin.toml` with `plugins = ["./imp"]` → `["./imp"]`; missing file → empty; `plugins = 3` → one `ConfigIssue`.
**Success check:** `cargo test -p core` green (incl. these) + workspace build 0 warnings + `doc/architecture/data-files.md` contains the `plugins` key text.
**Review focus:** no infinite waits (all child interactions bounded); probe false really never extracts; id overwrite (never trust plugin ids); doc updated in same commit.
**Out of scope:** plugin doc-reading test (7.5), example plugin crates (Phase 7), `mail_folders` key (not needed: G2 directory import drives the walk), annotation variant over the plugin wire may return `Unsupported`-for-annotation lines? pin: an `annotation` event from an importer plugin maps into `Extracted`? — NO: T5 keeps `extract_transactions` transaction-only; receiving `event":"annotation"` ⇒ `ImportError::Failed("annotation events not supported for plugin importers yet (Phase 7)")` — write that as a TODO-free pinned error (decision 17).
**References:** `plan.md:210-215`, `plugin-protocol.md:9-25,127-137`, `data-files.md:146-153`.

### T6 — 4.5 mbox reader (+ fixture extension)

**Depends on:** — (0.3 fixtures; independent of T1–T4; run in parallel with T2/T3/T4)
**Size:** one new module ~140 LOC + `generate.py` changes.
**Why:** plan item 4.5 (`plan.md:216-220`): built-in Thunderbird reader; incremental resume uses 2.7 markers.
**Current state (verified):** fixtures: `mail_paypal.mbox` 8 messages (structure quoted in Context), `mail_with_pdf.mbox` 12 messages, message 4 base64 attachment `bank_b1.pdf`; NEITHER mbox has `X-Mozilla-Status` or a `>From ` body line (grep; `X-Mozilla-Status` only at `plan.md:217`). Changing mbox bytes changes no id (source basename only; `id.rs:205-211` is the only disk test, reads `transactions.csv`). `generate.py` `build_mbox` writes headers at its own lines ~270-286; README `Do not edit by hand; regenerate instead` (`testdata/README.md:3-4`) and sha256 manifest info at `:9-11`.
**Already done / do not redo:** `state::Marker/decide/append_marker` exist (text-level, no I/O) — do not re-implement; mbox.rs stays text-in/text-out so it is unit-testable (plan ground rule 4).
**Files:**
- Create: `core/src/technical/mbox.rs`
- Modify: `core/src/technical/mod.rs` (`pub mod mbox;`)
- Modify: `testdata/generate.py` — add `X-Mozilla-Status: 0000`/`0001` header lines to `mail_paypal.mbox` messages (e.g. status `0001` on message 3, `0000` elsewhere) and a body line starting `>From the internet: hello` (escaped `From ` — the generator writes `>From ...` so the raw file contains no split-triggering `From ` column) inside one plain message body of `mail_with_pdf.mbox`.
- Regenerate: `python3 testdata/generate.py` (writes `testdata/mail_paypal.mbox`, `mail_with_pdf.mbox`; verify `transactions.csv` byte-unchanged — the mbox changes must not touch it; assert with `git diff testdata/transactions.csv` empty).
- Modify if the README lists hashes: `testdata/README.md` (update any manifest/notes mentioning mbox content; check what `:9-11` says and update as needed).
- Modify: `core/src/technical/importer_host.rs` — extend `builtin_importers()` with `BuiltInMbox` (probe: extension `.mbox` (case-insensitive) ⇒ `ProbeKind::Dialect("mbox")`; `extract_transactions` ⇒ `Ok(Extracted{transactions: vec![], warnings: vec!["mail importers never yield transactions (R002)"]})` — pinned so the R002 rule holds even through the host path; annotation extraction is T7, consumed by 4.8 directly, not through `extract_transactions`).

**Interfaces:**
- Consumes: none (pure text).
- Produces (exact; consumed by T7 and T9):
  - `pub struct Attachment { pub file_name: String, pub bytes: Vec<u8> }`
  - `pub struct Message { pub start: usize, pub end: usize, pub headers: Vec<(String, String)>, pub body: String, pub attachments: Vec<Attachment> }` (start/end are BYTE offsets into the input; `end` = end of that message = resume offset).
  - `pub fn header<'a>(m: &'a Message, name: &str) -> Option<&'a str>` (case-insensitive).
  - `pub fn split_messages(text: &str) -> Vec<Message>` — split ONLY at lines equal to `From `+rest at column 0; a body line starting `>From ` un-escapes to `From ` in the stored body/headers region and never splits.
  - `pub fn messages_from(text: &str, from_offset: usize) -> Vec<Message>` — parses only messages whose start ≥ from_offset (resume, uses 2.7 marker offset semantics).
  - `pub fn x_mozilla_status(m: &Message) -> Option<u32>` — header `X-Mozilla-Status`, hex parsed (`0x` prefix tolerated).
  - `pub fn parse_rfc822_date(text: &str) -> Option<String>` — `DD MONTH_NAME YYYY HH:MM:SS (+|-ZZZZ)` (and 2-digit day) ⇒ `Some("YYYY-MM-DD")`; anything else `None`. (Kept here, not in `domain::date::parse`, per Phase-1 decision 7 strictness.)
  - MIME: multipart via `Content-Type: multipart/mixed; boundary=X` parts; base64 (`Content-Transfer-Encoding: base64`) attachment with `filename=`/`name=` from `Content-Type`/`Content-Disposition`; malformed base64 tolerated ⇒ that part skipped (skip-with-flag? pin: skipped silently; the plan does not require reporting).

**Implementation brief:**
1. Implement split scanning bytes; record start of each message at `From ` column-0 lines; `end` = start of next message (or len for last).
2. Unescape: leading `>From ` → `From ` in message text when exposing body/headers (so T7 reads the un-escaped line) BUT `end` offsets stay byte-exact for resume.
3. Headers: contiguous `Header: value` lines until blank line; unfold continuation lines starting with space/tab (pin: append with single space).
4. Parse multipart boundaries per message; attachments = non-text/plain parts with filename; base64 decode ignoring newlines.
5. Fixture extension first (steps in Files order): edit `generate.py`, run `python3 testdata/generate.py`, verify `mail_paypal.mbox` gained `X-Mozilla-Status:` lines and `mail_with_pdf.mbox` gained the `>From ` line, `transactions.csv` unchanged.

**Pinned decisions:** G4 (fixture extension), decision 10 (rfc822 date here).
**Edge cases / failure modes:** `From ` inside a `Content-Type` header? not in fixtures — but rule: only `From `+space at line start splits; escaped `>From ` in body must NOT split (count messages after extension: 12 stays 12 for `mail_with_pdf.mbox`); resume offset in the MIDDLE (resume offset equal to a message `end`) → returns following messages only; header-less / boundary-less messages must not panic.
**Tests / acceptance criteria:**
  - `fixture_paypal_yields_exactly_8_messages`: `split_messages(text of testdata/mail_paypal.mbox).len() == 8` (disk read via copied find-testdata helper).
  - `fixture_with_pdf_yields_exactly_12_messages` and `attachment_decoded`: the one `application/pdf` attachment has `file_name=="bank_b1.pdf"` and `bytes` start with `%PDF` AND sha256_hex(bytes) equals sha256 of `testdata/bank_b1.pdf` (read the file).
  - `escaped_from_does_not_split`: after regeneration, `mail_with_pdf.mbox` body contains a line whose unescaped form starts with `From `; message count still 12.
  - `x_mozilla_status_read`: paypal fixture message 3 ⇒ `Some(1)` (0001), others `Some(0)`.
  - `resume_from_offset`: `messages_from(text, last_end)` on a 3-message inline mbox: first call returns 3, then pass the last `end` ⇒ returns only new ones; byte offsets deterministic.
  - `parse_rfc822_date`: `"02 Apr 2024 12:00:00 +0100"` ⇒ `"2024-04-02"`; `"garbage"` ⇒ None.
**Success check:** `cargo test -p core mbox` green + `python3 testdata/generate.py` twice → `git status testdata/` shows no second-run change (determinism) + `git diff` shows `transactions.csv` untouched.
**Review focus:** byte offsets exactness (resume correctness), split rule (column-0 only), fixture regeneration committed with the task.
**Out of scope:** no annotations/mbox importer logic (T7), no marker file I/O (T9 writes markers via fs_atomic).
**References:** `plan.md:216-220`, `data-files.md:139-144`, `plan.md:151` (2.7 done).

### T7 — 4.6 Mail annotation extraction

**Depends on:** T1, T6
**Size:** one new module ~130 LOC.
**Why:** plan item 4.6 (`plan.md:221-226`): mail yields annotations (R002), never transactions.
**Current state (verified):** `Annotation` has NO date/amount field (`domain/annotation.rs:26-49`) — matching in 4.7 needs them, so T7 produces facts alongside (decision 19, deviation from plan's literal `Vec<Annotation>` because `link` must match date+amount and the Annotation record cannot carry them; plan intent "no transaction produced" preserved). PayPal fixture facts in Context (body `Betrag:`/`Grund:`); `mail_with_pdf.mbox` message 4 has the base64 `bank_b1.pdf` attachment; `parse_rfc822_date` from T6; `pdf_text::extract_text` from T1; `parse_bank_b` from T3.
**Already done / do not redo:** T6 parsing, T3 parsing — reuse; do not add date/amount columns to `annotations.csv` (`data-files.md:79-91` fixed).
**Files:**
- Create: `core/src/technical/mail_annotate.rs`
- Modify: `core/src/technical/mod.rs`
- Read first: `domain/annotation.rs`, `mbox.rs` (T6), `parse_bank_b.rs`

**Interfaces:**
- Consumes: `mbox::Message`, `mbox::header`, `mbox::parse_rfc822_date`, `pdf_text::extract_text`, `parse_bank_b::parse_bank_b_text`, `annotation::{Annotation, clamp_confidence}`, `id::{sha256_hex, first_chars}`.
- Produces (exact; consumed by T8 and T9):
  - `pub const CONF_EXTRACT: f64 = 0.9;` (extraction confidence carried until 4.7 assigns the FINAL confidence).
  - `pub struct AnnotatedMail { pub annotation: Annotation, pub date: String /*YYYY-MM-DD*/, pub amount: i64 /*minor units*/, pub counterparty_hint: String }` (the "facts" for linking; `amount` negative = money left the account).
  - `pub fn annotate_mbox_text(text: &str, source: &str) -> Vec<AnnotatedMail>` (pure entry for inline tests).
  - `pub fn annotate_mbox(path: &Path, source: &str, from_offset: usize) -> Vec<AnnotatedMail>` — resumes via `mbox::messages_from`.
  - `pub fn annotate_attachment_pdf(bytes: &[Vec<u8>], ...)`? pin: `pub fn items_from_invoice_bytes(bytes: &[u8], source_note: &str) -> Vec<String>` — writes bytes to `std::env::temp_dir()/phin-invoice-<sha256_hex(bytes)[0..16]>.pdf` (unique by content hash), runs `pdf_text::extract_text`, parses with `parse_bank_b_text`, each transaction ⇒ item string `format!("{} {} {}", date, description, money-print(amount))`; deletes the temp file after; on any failure ⇒ empty items.
  - Annotation id: `id::first_chars(id::sha256_hex(&format!("{}|{}|{}", source, text, annotation::items_to_text(items))), 10)` (decision 8: id excludes ref/confidence so relinked annotations keep their id).

**Implementation brief:**
1. PayPal path: for each message whose `Subject:` is `"Your PayPal payment receipt"`: text = `format!("PayPal payment to {}", grund)`; `Betrag:` token parsed via `money::parse` (German comma) — NEGATIVE convention: fixture `Betrag: 25,99` means money OUT ⇒ store `-2599`; `Grund:` = counterparty_hint; items = empty; date = `parse_rfc822_date(Date header)`; annotation confidence = `CONF_EXTRACT`; source = mbox basename.
2. Invoice path: for each message with a pdf attachment: bytes → `items_from_invoice_bytes`; annotation text = `format!("Attachment {} from {}", file_name, sender-of-From-header)`? pin: text = `format!("Invoice items from attached {}", file_name)`; amount/date: use the mbox Date header for the fact date; `amount` = sum of attachment transactions' amounts; counterparty_hint = message `From:` display name.
3. Never construct a `Transaction` at the call boundary — the return type contains no `Transaction` (compile-time proof via test below).
4. Empty body/attachment-less messages produce no annotation.

**Pinned decisions:** decision 19 (AnnotatedMail), decision 8 (annotation id scheme), decision 9 (amount sign convention: fixture `Betrag:` value ⇒ negative fact amount), decision 20 (temp file naming by content hash).
**Edge cases / failure modes:** `SUPERMARKET; DELICATES` Grund contains `;` — must survive as counterparty_hint verbatim; mbox Date header missing ⇒ `date` = empty string fact (still returned, but T8 cannot match it); attachment that fails extraction ⇒ empty items, annotation still produced; running annotate twice on same text ⇒ identical output (pure).
**Tests / acceptance criteria:**
  - `paypal_fixture_yields_annotations_with_items_and_confidence`: `annotate_mbox_text(paypal fixture, "mail_paypal.mbox")` len == 8; first fact: date `"2024-04-02"`, amount `-2599`, counterparty_hint `"LEGO STORE DE"`, confidence `0.9`.
  - `invoice_attachment_yields_items`: annotate `mail_with_pdf.mbox` → exactly 1 AnnotatedMail whose `annotation.items.len()==4` (bank_b1 rows) and item strings contain `"REWE MARKT 12"`.
  - `annotation_carries_confidence`: every `annotation.confidence == 0.9`.
  - `never_produces_transactions`: pin by signature: `fn assert_no_tx(annotated: Vec<AnnotatedMail>) { let _: Vec<Annotation> = annotated.into_iter().map(|a| a.annotation).collect(); }` compile-path test + `mail_with_pdf` facts `amount` == sum(-8990-2500+85000-3456)?? pin exact value at implementation: `70054`? — NO: sign pin: sum of b1 amounts = `-8990-2500+85000-3456 = 70054`; assert `facts[0].amount == 70054` (document: invoice totals are the statement's own sums).
  - `escaped_from_body_not_annotated`: 12-message fixture still yields exactly 1 mail annotation.
**Success check:** `cargo test -p core mail_annotate` green.
**Review focus:** R002 hard line (no `Transaction` constructed here except via `items_from_invoice_bytes` which must DISCARD them — assert no leak of parse results), sign convention, id stability.
**Out of scope:** linking (T8), persistence (T9), plugin wire annotations (T5/7.5).
**References:** `plan.md:221-226`, `architecture.md:268-273`, `plugin-protocol.md:92-103`.

### T8 — 4.7 Automatic annotation → transaction linking

**Depends on:** T7
**Size:** one new module ~120 LOC.
**Why:** plan item 4.7 (`plan.md:227-231`): core performs the automatic link and assigns final confidence (`plugin-protocol.md:101-103`).
**Current state (verified):** `Annotation` fields incl. `r#ref` (empty = orphan/pending, `domain/annotation.rs:26-49`, `data-files.md:84`); confidence 0…1 f64 clamped (`annotation.rs:56`); no numeric thresholds anywhere (scout verified); `view::read_view` gives `Vec<Entry>`+`View` (transactions available); G5 numbers pinned by user.
**Already done / do not redo:** annotation domain clamp; do not write files (T9 appends).
**Files:**
- Create: `core/src/technical/link.rs`
- Modify: `core/src/technical/mod.rs`

**Interfaces:**
- Consumes: `T7::{AnnotatedMail, CONF_EXTRACT}`? pin: consumes `domain::annotation::Annotation` + `T7::AnnotatedMail`.
- Produces (exact; consumed by T9):
  - `pub const DATE_WINDOW_DAYS: i64 = 3;`
  - `pub const CONF_LINKED: f64 = 0.9;` `pub const CONF_PENDING: f64 = 0.4;` `pub const CONF_ORPHAN: f64 = 0.0;`
  - `pub enum LinkStatus { Linked, Pending(Vec<String> /*transaction ids, ascending*/), Orphan }` (derive Debug, Eq, PartialEq)
  - `pub struct LinkedMail { pub annotation: Annotation /*ref+confidence assigned*/, pub status: LinkStatus }`
  - `pub fn auto_link(mail: &[AnnotatedMail], transactions: &[Transaction]) -> Vec<LinkedMail>` (pure; `domain::date::parse` not needed — both sides already `YYYY-MM-DD`; compare via `chrono`? no date crate — pin: convert both to `days-since-epoch` integers by a small proleptic formula (civil-days algorithm, stdlib `SystemTime`? NO — pure civil-date math fn `fn days_from_civil(y,m,d)->i64` private) — decision 21).

**Implementation brief:**
1. Candidate = transaction where |days(tx.date) − days(fact.date)| ≤ 3 AND tx.amount == fact.amount AND (counterparty_hint case-insensitive substring in `annotation.text` OR in any item) — OR counterparty_hint empty (then counterparty check passes vacuously).
2. Exactly 1 candidate ⇒ `Linked`: `annotation.r#ref = tx.id`, `confidence = CONF_LINKED`.
3. >1 candidates ⇒ `Pending(candidates sorted by tx id)`: `r#ref` stays empty, `confidence = CONF_PENDING`; "both recorded" = the candidate ids are the Pending payload (T9 returns them to the caller/list-pending later).
4. 0 candidates ⇒ `Orphan`: ref empty, confidence `CONF_ORPHAN`.
5. Deterministic: inputs in, outputs same order.

**Pinned decisions:** G5 (0.9/0.4/0.0, ±3 days, exact amount, substring counterparty — decision 5), decision 21 (civil-days integer date math).
**Edge cases / failure modes:** window boundary at exactly ±3 days INCLUDED, ±4 excluded (test both); amount differs by 1 minor unit ⇒ no candidate; transaction date non-canonical (from_stored guarantees canonical); duplicate-id transactions (b2 `#2`): both candidates with different ids ⇒ Pending branch (they are two candidates even with equal fields, because ids differ).
**Tests / acceptance criteria (inline, using inline `AnnotatedMail`s + `Transaction::from_stored` built literals):**
  - `unique_match_high_confidence`: paypal-shaped fact (date 2024-01-03, amount -3999, hint "REWE MARKET") + exactly one matching tx ⇒ `Linked`, confidence 0.9, ref == tx id.
  - `two_candidates_lower_confidence_both_recorded`: txs `80c63211a8` and `80c63211a8#2` (same fields, b2 pair) ⇒ `Pending(["80c63211a8","80c63211a8#2"])`, confidence 0.4, ref empty.
  - `no_candidate_orphan`: hint "NOTHING MATCHES" ⇒ `Orphan`, confidence 0.0.
  - `window_boundaries`: tx dates 2024-03-02 fact date 2024-02-28 ⇒ 3 days ⇒ candidate; 2024-02-27 ⇒ 4 days ⇒ orphan.
  - `unique_match_on_2024_02_29_vs_2024_02_27`: leap-year window math (fact 2024-02-26, tx 2024-02-29 ⇒ 3 days ⇒ match).
**Success check:** `cargo test -p core link` green.
**Review focus:** deterministic ordering, exact constants, no I/O, substring rule case-insensitivity.
**Out of scope:** persistence, pending listing command (6.3), user confirmation (confirm-annotation).
**References:** `plan.md:227-231`, `architecture.md:268-273,450`.

### T9 — 4.8 `import` command wired into interpreter

**Depends on:** T4, T5, T6, T8
**Size:** new `ImportWork` (~150 LOC) + widen `Work` seam (~30 LOC) + main.rs (~5 LOC) + integration tests + core-protocol.md text; deliberately one task: one deliverable, one protocol gate.
**Why:** plan item 4.8 (`plan.md:232-236`): the end-to-end import demonstrable through the protocol (Requirements 001/002 shape).
**Current state (verified):** `main.rs:312-316` injects `interpret::wired_nowhere()`; `import` refused today (`interpret.rs:666-668`) with pinned test `every_command_of_the_table_is_refused_with_a_documented_code_naming_its_phase` (:1094-1133 — expects `codes_of` row [0] = `plugin-failed` with `4.8` in text). Seam note (`interpret.rs:192-201`): `writer::run` can't express per-command payload; a wired phase drives `writer::start/progress/log/succeed` itself and must see the cancel flag: `Work::do_command(&mut self, Command, &mut dyn EventSink)` (:202-205) gives NO access to `CommandStream`/cancel — widening belongs here (4.8). Markers: `state::*` text-level; store: `store_transactions::append`, `id::occurrence_id`, `store_sidefiles::append_annotation`; progress example `core-protocol.md:132`; doc update obligation `plan.md:22-25`; G2: directory paths allowed.
**Already done / do not redo:** keep `WiredNowhere` (incl. its `import` arm and its pinned test) UNCHANGED — it stays the fallback work; production work is a NEW struct. Do not change `writer::run`/`CommandWork` (test-only convenience).
**Files:**
- Create: `core/src/protocol/import_work.rs` (module doc, `pub struct ImportWork`): `pub mod import_work;` in `core/src/protocol/mod.rs:14-15`
- Modify: `core/src/main.rs` (replace `wired_nowhere()` with `ImportWork::new(data_dir, …)` that delegates non-`import` commands to `WiredNowhere`)
- Modify: `core/src/protocol/interpret.rs` — widen `Work::do_command` to `fn do_command(&mut self, command: Command, stream: &mut writer::CommandStream, sink: &mut dyn EventSink) -> Answered` (decision 22: wired phases need the cancel flag + stream; update ALL implementors and call sites, incl. the test impls and the pinned test which must gain `start`ed streams — keep its assertions' codes/text identical, only plumbing changes)
- Modify: `doc/architecture/core-protocol.md` — command table row for `import`: "Import one file **or a directory of files** (bank statement PDF, mbox) via the importers."; add a result test-example after the import example: `<!-- test-example id=core-command-import-result -->` `{"v":1,"event":"result","cmd":"import","added":2,"annotations":1,"linked":1}` plus one sentence naming the three result fields as counters of the run; mark as "Fixed here (plan item 4.8)". (Decision G2 + decision 23.)
- Create (tests): `core/tests/import_lifecycle.rs` (uses `CARGO_BIN_EXE_core`, free port, `--data-dir=test-scratch/import_lifecycle`, small `--idle-timeout`)
- Read first: `interpret.rs` (whole), `writer.rs` (whole), `transport_cfg` usage in `main.rs`, `state.rs`, `id.rs::occurrence_id`.

**Interfaces:**
- Consumes: `interpret::Work` (widened), `writer::{start,progress,log,succeed,fail,cancel,cancel_requested}`, `proto::command::Command::Import`, `T4::{Host,builtin_importers,import_file,exec_importers,FileOutcome}`, `T6::{split_messages,messages_from}`, `T7::annotate_mbox`, `T8::{auto_link,LinkedMail,LinkStatus}`, `store_transactions::{read,append}`, `store_sidefiles::append_annotation`, `fs_atomic::write_atomic`, `state::{read_markers,decide,marker_of,append_marker,write_markers}`, `id::occurrence_id`.
- Produces: `pub struct ImportWork { … }` implementing `Work`; result payload keys exactly `added` (transactions newly appended), `annotations` (annotations newly appended), `linked` (annotations ending `Linked`).

**Implementation brief (ordered):**
1. Widen the seam (interpret.rs + call sites + implementors); keep `Answered` semantics; `WiredNowhere` ignores stream. Run `cargo build --workspace --all-targets` → all previous tests compile.
2. `ImportWork::new(data_dir: PathBuf)`; `do_command` on `Command::Import(path)`:
   a. Build host: `builtin_importers()` + `exec_importers(data_dir).0`; on config issues emit `writer::log(level:"warn", text:…)`.
   b. Enumerate items: if path is a FILE → `[path]`; if DIR → recursive walk sorted by path string; missing path ⇒ `writer::fail(…, ErrorReason::Io, "no such path: …")` — wait: `ErrorReason` codes: use `io` (documented set). Else not-found.
   c. `let mut stream = writer::start("import", total)` — BUT the interpreter already started the stream? Pin: the interpreter does NOT start streams today; the wired phase calls `writer::start` itself (precedent: interpret test work drives primitives itself, `interpret.rs:192-201` doc).
   d. For each file: `if writer::cancel_requested(&stream) { writer::cancel(...); return EndedWith }` BEFORE the file (finish current item, discard partial — writes happen only after a whole file so per-file granularity satisfies "finish the current item"); `writer::progress(&mut stream, sink, Some(file_name))`.
   e. mbox files: marker = `state::marker_of(read_markers(read markers.csv), path, mtime, size)` — read the marker text via `std::fs` here (the ONLY new fs reads); `decide` Skip/Reprocess; on Reprocess: `mbox::split_messages` for count/total? no progress sub-count needed — then `messages_from(text, marker offset)` → `mail_annotate::annotate_mbox` → `link::auto_link(mail_facts, view transactions from data dir)` → append annotations (empty ref ⇒ orphan/pending rows per `LinkStatus`) via `append_annotation` + `write_atomic`; update marker text via `append_marker` (offset = last processed message `end`, else keep) + `fs_atomic::write_atomic(state/markers.csv)`. `decide==Skip` ⇒ log line `"marker skip: <file>"`.
   f. non-mbox: `import_file(&host, path)`; per supporter `log(level:"info","probe: <name> supported")` style (matches `core-protocol.md:139` example); append transactions: for each tx, `occurrence_id(base, known ids)` then `store_transactions::append` (which skips exact duplicates? — read its AppendOutcome; pin: dedup happens via known-id set built from the file BEFORE the run; exact duplicate content ⇒ new occurrence id `#N` via occurrence_id (b2 pair!), "already seen in this same run" skip only when the identical (id,fields) pair was appended twice in one run — i.e. idempotence across runs comes from `decide`-skip of the whole file? NO — `decide` only skips mbox; for PDFs the store-level idempotence (`plan.md`/`architecture.md:241` appends only ids not yet known"): pin final rule: transaction skipped iff its occurrence-id base set already contains ALL occurrences of base present in file equal in number to what the run yields — implement as: compute `occurrence_id` against the union of (file-on-disk ids + ids appended so far this run); skip the tx iff that exact full id already exists. Re-importing a file then adds NOTHING because base ids all exist and occurrence numbers do NOT increase — pin this exact rule here to pass test `second import appends nothing`.)
   g. After all items: `writer::succeed(&mut stream, sink, payload)` with keys `added/annotations/linked` (i64 counters) → `Answered::EndedWith`.
3. `main.rs`: `ImportWork::new(data_dir.clone())`; keep everything else.
4. `core/tests/import_lifecycle.rs`: spawn daemon via `CARGO_BIN_EXE_core --serve --address=127.0.0.1:0?` — reuse existing transport test scaffolding (`transport_lifecycle.rs` pattern with free port + `--data-dir`); copy fixture files into the scratch data dir? NO — import path points at the (read-only) `testdata/` dir (G2 directory import) while data dir is the scratch copy. Assertions per test names below.
5. Doc update of `core-protocol.md` per Files (same commit).

**Pinned decisions:** G2 (dir), decision 23 (result payload keys), decision 22 (seam widening), decision 24 (dedup rule: occurrence-id existence check), decision 25 (marker update only after a fully processed file; mtime/size from `std::fs::metadata`).
**Edge cases / failure modes:** interrupted import (cancel after first progress): current file's partial results discarded — files already written stay valid (atomic writes); `transactions.csv` parseable after kill (test below); empty directory import ⇒ counters 0, result event; directory with no importer accepting anything ⇒ counters 0, no error; plugin failing mid-run ⇒ `log` error named plugin + run continues (host guarantee from T4); second import of mbox ⇒ `decide` Skip (marker same) ⇒ no new annotations; second import of PDF ⇒ idempotence rule (f) ⇒ no new transactions; handshake already gated by transport (`02_impl.md:121` carry-forward) — add an integration test `un_handshaked_peer_cannot_import`: connect raw socket, send `{"v":1,"cmd":"import","path":…}` WITHOUT `version` first ⇒ daemon must not execute the import (no transaction appended; connection closed or `error` — pin current transport behavior: read `transport.rs` gate and assert against it).
**Tests / acceptance criteria (integration, `core/tests/import_lifecycle.rs`):**
  - `import_directory_appends_exactly_expected`: daemon on free port, handshake, `import` on `testdata/` directory → result counters `added == 14` (bank_a 6 + b1 4 + b2 4; mail never adds — R002), `annotations >= 9` (8 paypal + 1 invoice; assert `>=9` — document why not exact: linking is not annotation-counting), `linked >= 0`; then `transactions.csv` in data dir contains exactly the 14 expected ids incl. `80c63211a8` AND `80c63211a8#2` (b2 pair) and NONE of the 8 `mail_paypal.mbox` ids from the fixture CSV.
  - `second_import_appends_nothing`: run import twice on same dir; second result `added==0 && annotations_added==0` (mbox decide-skip + pdf idempotence); file byte counts stable? assert: `store_transactions::read` count stays 14 and marker file has entries for both mbox fixtures.
  - `cancel_leaves_files_valid`: send `import` then `cancel` after first `progress` line; expect `cancelled` event; then `store_transactions::read(transactions.csv)` returns `(Vec, vec![])` with zero `ReadError`s (file valid); markers file still `read_markers` clean.
  - `un_handshaked_peer_cannot_import`: raw connect, send import command with no handshake, assert no import happened (data dir untouched) and connection state per transport policy.
  - interpreter-level unit test in `import_work.rs` tests: `import_work_refuses_missing_path` via `scripted` + collecting helpers pattern: result `error` code `io` naming path.
**Success check:** `cargo test --workspace` green (incl. `core/tests/import_lifecycle.rs`) + `cargo build --workspace --all-targets` 0 warnings + `doc/architecture/core-protocol.md` shows the amended `import` row and new `core-command-import-result` example + the four `plan.md:193-235` checkboxes for 4.1–4.8 all ticked across the phase's commits.
**Review focus:** cancel semantics (finish current item, nothing half-written), exact dedup rule (no accidental `#N` inflation on re-import), handshake gate test, doc amendment matches the emitted frames (`proto::codec::decode_line` asserts keys), seam widening did not change refusals.
**Out of scope:** `report`/`set-category`/`confirm-annotation`/`list-pending` wiring (Phases 5/6), plugin doc tests (7.5), Thunderbird sqlite.
**References:** `plan.md:232-236`, `core-protocol.md:92-117,119-170,172-190`, `interpret.rs:192-201,666-668,1094-1133`, `architecture.md:236-243,274-276`.

## Findings log (planning)
- [scout] Phase 4 items verbatim at `plan.md:193-235`; all Phase 0–3 items `[x]`; legend `plan.md:32`; one-task-at-a-time `plan.md:9-11`; ~150-LOC cap `plan.md:12-13` (user G1: no longer binding); tests-in-task `plan.md:14-16`; domain purity `plan.md:17-19`; no-real-data `plan.md:20-21`; docs-updated-in-task `plan.md:22-25`; commit-per-task `plan.md:26`; stop-on-decision-change `plan.md:27-28`; "Phase 4 needs 1.x+2.x+3.4" `plan.md:317-320`.
- [scout] `pdftotext -layout` decision + pdfium alternative: `architecture.md:42,373`; risk "detect at start-up, clear error" `:430`; importer block `:206-208`; import runtime `:236-243`; mail runtime `:268-273`; cancel `:274-276`; confidence glossary 0…1 `:450`; Q1 "crash during import leaves files valid" `:419`; one-plugin-per-bank `:435`.
- [scout] `import | path` single-file `core-protocol.md:99`; example `:112-117`; events/progress/log/error `:119-170`; cancel `:172-190`; error codes `:169-170`; handshake `:57-79`.
- [scout] plugin lifecycle `plugin-protocol.md:9-25` (config list, one process per plugin per run, timeout 60 s default → `plugin-failed`); wire shapes `:27-41,43-64,66-90,92-103`; core dedups ids `:89-90`; core assigns final confidence `:101-103`; versioning/error `:127-137`.
- [scout] `data-files.md`: codec `:10-27`; id `:26-41`; transactions `:58-75`; annotations `:77-91`; markers `:139-144`; config `:146-153` (no literal plugin key).
- [scout] existing signatures: `transaction::from_parts` `core/src/domain/transaction.rs:89` (+importer doc `:67-68`, invariant `:83-88`); `date::parse` strict `domain/date.rs:30-40`; `money::parse domain/money.rs:33`; `annotation domain/annotation.rs:26-95`; `id.rs:63-200`; `state.rs:29-143`; `view.rs:53-138`; `store_transactions.rs:93,186`; `store_sidefiles.rs:96,194`; `fs_atomic.rs:143`; `writer.rs:88-381`; `interpret.rs:178-303,638-668,1094-1133`; `transport_cfg.rs:102-125,391,226`; `main.rs:312-316`; seam note `interpret.rs:192-201`; carry-forwards `.agents/plans/2026-11-18_phase-3-core-protocol/02_impl.md:79,96,121`.
- [scout] fixtures: pdftotext outputs quoted in Context (bank A 6 rows German amounts; b1/b2 ISO dates + point decimals; b2 byte-identical pair → `80c63211a8`/`#2`); `transactions.csv` 22 rows with per-source ids (Context); `mail_paypal.mbox` 8 msgs (`Betrag:/Grund:` bodies, no X-Mozilla-Status), `mail_with_pdf.mbox` 12 msgs (msg 4 base64 `bank_b1.pdf`), no `>From` body line; only disk test reads `transactions.csv` (`id.rs:205-211`); no byte-hash test deps.
- [scout] confidence scale: only 0…1 f64 (`annotation.rs:56`, `data-files.md:88`, `architecture.md:450`); no thresholds anywhere.
- [scout] commands: `cargo build --workspace --all-targets` (0 warnings gate), `cargo test --workspace` (green at HEAD: core lib 199, proto 33), `python3 testdata/generate.py`; no rustfmt/clippy gate (tree unformatted); `pdftotext 26.11.0` at `/usr/bin/pdftotext`; cargo 1.100.0.
- [scout] conventions: `test-scratch/<name>` scratch dirs; decode with `proto::codec::decode_line` (serde_json lexicographic keys); purity test `core/src/technical/mod.rs:56,85`; `WiredNowhere` import refusal text names `4.8` (test must stay green); commit convention `<id>: <title>` ticks the checkbox (`.agents/plans/2026-11-15_phase-2-data-store/02_impl.md:17-18`).

## Decision log (planning)
| # | decision | rationale | decider | status |
|---|----------|-----------|---------|--------|
| 1 | One task per plan item; 4.4 split into 4.4a (T4) / 4.4b (T5) on content grounds (contract/orchestration vs executable-process host) | user: LOC cap `plan.md:12-13` no longer binds; split sensibly by content; Phase-3 precedent (3.6a–d) | user | resolved |
| 2 | `import path` = file OR directory; core walks directories; `core-protocol.md:99` amended with test-example | user Q1=B; plan 4.8's "fixture directory" test needs it; doc amendment per `plan.md:22-25` | user | resolved |
| 3 | Literal `plugins` key in `phin.toml` (array of executable paths), documented in `data-files.md` in T5 | user Q2; `plugin-protocol.md:11` requires config discovery; precedent decisions 8/40 | user | resolved |
| 4 | mbox fixtures extended in `generate.py` (`X-Mozilla-Status`, `>From` body line) and regenerated | user Q3=A; plan 4.5 names both; fixtures "regenerated, not edited" (`testdata/README.md:3-4`) | user | resolved |
| 5 | Link numbers: unique 0.9, ambiguous 0.4 pending (both candidates recorded, empty ref), orphan 0.0; window ±3 days; exact amount; substring counterparty | user Q4 (proposed accepted) | user | resolved |
| 6 | `extract_text_with(program, path)` public for testing | missing-tool test needs injection; no env PATH hacks | agent | JUSTIFIED |
| 7 | Parsers set provisional `id::transaction_base_id` (no `#N`); occurrence suffix assigned at append (T9) | `id::occurrence_id` needs on-file ids (`id.rs:126`); `plugin-protocol.md:39-41` core computes ids | agent | JUSTIFIED |
| 8 | Annotation id = sha256(`source|text|items`)[0..10] | ref+confidence change across lifecycle ⇒ id must exclude them (`data-files.md:26-41` orphan consequence); 10-hex convention (`id.rs:40`) | agent | JUSTIFIED |
| 9 | Fixture `Betrag:` values stored NEGATIVE as fact amounts (money left the account) | transactions.csv rows for the same events and sign rule `amount<0 expense` (`data-files.md:68`) | agent | JUSTIFIED |
| 10 | RFC-822 mbox date parsing lives in `mbox.rs`, not `domain::date::parse` | Phase-1 decision 7 keeps `date::parse` strict (2 layouts only) | agent | JUSTIFIED |
| 11 | Plugin timeout default 60 s, settable per `ExecImporter` | `plugin-protocol.md:24-25` default; tests need a small value | agent | JUSTIFIED |
| 12 | Fake plugin = `python3` script under `core/tests/fixtures/` | python3 precedent (`testdata/generate.py`); no extra Rust bin targets; JSON-Lines trivial | agent | JUSTIFIED |
| 13 | Built-in probing = content markers (`Bank A AG` / `Bank B N` / `.mbox` extension) | no filename convention anywhere; probe must distinguish banks | agent | JUSTIFIED |
| 14 | Host emits no protocol events; 4.8 owns events via `writer` | interpret-test precedent (`interpret.rs:192-201` doc); keeps host unit-testable (ground rule 4) | agent | JUSTIFIED |
| 15 | `ExecImporter` = one child process per file-run (spawn at probe, reuse for extract, kill on drop) | `plugin-protocol.md:12-15` "one process per plugin per run" | agent | JUSTIFIED |
| 16 | Core computes ids for plugin transactions, discarding any wire id | `plugin-protocol.md:39-41` | agent | JUSTIFIED |
| 17 | Plugin `annotation` events → typed error "not supported for plugin importers yet (Phase 7)" | no annotation-plugin story in Phase 4; R002 built-ins only; avoids inventing behavior | agent | JUSTIFIED |
| 18 | Bank parsers set `counterparty = description` | no counterparty-recognition spec exists; fixtures print merchant text only; importer "as recognized" (`data-files.md:71`) | agent | JUSTIFIED |
| 19 | `mail_annotate` returns `Vec<AnnotatedMail>` (annotation + date/amount/hint facts) not bare `Vec<Annotation>` | 4.7 must match date+amount but `Annotation` has neither field (`annotation.rs:26-49`); plan intent (no transactions produced) preserved | agent (deviates from plan text `plan.md:222`) | JUSTIFIED |
| 20 | Invoice temp file name = content hash (`std::env::temp_dir()`) | unique, collision-free, no new dep; pdftotext needs a file path | agent | JUSTIFIED |
| 21 | Date-window math = civil-days integer algorithm (private fn in `link.rs`) | no date crate in `core/Cargo.toml`; bounded window comparison only needs day counts | agent | JUSTIFIED |
| 22 | Widen `Work::do_command` to hand wired phases the `&mut CommandStream` | carry-forward `.agents/plans/2026-11-18_phase-3-core-protocol/02_impl.md:79,96` names 4.8 as the owner; cancel flag needed | agent | JUSTIFIED |
| 23 | Import result payload keys `added`, `annotations`, `linked`; `core-protocol.md` amended with `core-command-import-result` example | docs are source of truth and must be updated in the changing task (`plan.md:22-25`); no result shape existed for `import` | agent | JUSTIFIED |
| 24 | PDF re-import idempotence: skip iff the computed occurrence-id already exists in (on-file ∪ this-run) ids | `architecture.md:241` appends only unknown ids; `id::occurrence_id` semantics (`id.rs:126`); keeps b2 `#2` pair while second import adds nothing | agent | JUSTIFIED |
| 25 | Marker updated only after a fully processed mbox file (offset = last message `end`); mtime/size via `std::fs::metadata` | cancel/interrupt leaves files valid (Q1 scenario `architecture.md:419`); `state::decide` compares mtime+size | agent | JUSTIFIED |
| 26 | `WiredNowhere` and its pinned test stay unchanged; production gets `ImportWork` delegating non-import verbs to it | smallest change; refusal test `interpret.rs:1094-1133` stays green as fallback contract | agent | JUSTIFIED |

## Coverage check
| requirement / user goal | task(s) | how verified |
|-------------------------|---------|--------------|
| 4.1 pdftotext helper (plan.md:195-202) | T1 | `cargo test -p core pdf_text`: fixture reading order, two-column, missing tool typed, empty PDF |
| 4.2 bank A parser (plan.md:203-207) | T2 | exact 6 tx ids/amounts/dates; broken variant warns keeps rows; twice identical |
| 4.3 bank B parser (plan.md:208-209) | T3 | fixture b1/b2 incl. duplicate pair, `;` description |
| 4.4 importer host + plugin contract (plan.md:210-215) | T4 + T5 | probe-then-extract orchestration; probe-false never extracted; non-zero exit named; timeout kill; `plugins` config tests |
| 4.5 mbox reader (plan.md:216-220) | T6 | 8/12 message counts, `X-Mozilla-Status`, `>From` non-split, attachment bytes == bank_b1.pdf sha, offset resume |
| 4.6 mail annotations (plan.md:221-226) | T7 | paypal items/confidence; invoice attachment items; return type has no Transaction |
| 4.7 auto linking (plan.md:227-231) | T8 | unique 0.9, two candidates 0.4 pending, orphan 0.0, ±3-day boundaries incl. leap year |
| 4.8 `import` wired (plan.md:232-236) | T9 | `core/tests/import_lifecycle.rs`: directory import exact 14 transactions, mail never adds; second import nothing; cancel leaves valid files; un-handshaked peer cannot import |
| R002 mail never adds transactions | T7 (type), T9 (integration assert) | annotations vs transactions counters + absence of mail ids in `transactions.csv` |
| Doc truth maintained (ground rule 6) | T5 (`data-files.md` plugins), T9 (`core-protocol.md` import row + result example) | doc text asserted/inspected in same commits |
| Idempotence & crash-safe (Q1) | T9 | second-import test + cancel-validity test |
| Commit per task + checkbox ticks (ground rule 7) | all | G7 per task success checks |
