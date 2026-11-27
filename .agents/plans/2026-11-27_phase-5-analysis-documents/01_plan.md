# Plan
Date: 2026-11-27
Goal: Implement Phase 5 of the Phin plan (5.1–5.8): analyses over the merge-on-read store, a typed table type, Markdown/HTML rendering, delegated PDF/ODS export, the analysis-plugin host, and the `analyze`/`report` commands — folding in the upstream issues that Phase 5 would otherwise inherit (#2, #3, #4, #12).
Plan format: detailed-v1

## Context (applies to every task)

- **Goal / success shape:** Phin can answer "what did I spend on food in 2024?" through the protocol: an
  `analyze` command returns typed tables and a `report` command writes a Markdown/HTML (and, via external
  converters, PDF/ODS) document into the data directory. Requirement 005
  (`doc/architecture/architecture.md:15`: "Analyses are rendered as Markdown or HTML; PDF and
  LibreOffice/ODS output is delegated to external converters."), Q1 correctness (exact integer amounts,
  idempotence) and Q3 flexibility ("adding a new analysis must mean adding one analysis plugin",
  `architecture.md:421`). Definition of Done (`doc/implementation/plan.md:331-335`): `cargo test` green at
  the workspace root, requirements demonstrable through the CLI on fixtures, docs match code.
- **Non-goals:**
  - Interface processes (CLI/TUI/GUI, plan 6.1/6.4/6.5), `refine`/`list-pending` wiring (6.2/6.3),
    example plugin projects (7.1–7.3), the end-to-end test (7.4) and the final documentation pass (7.5)
    stay in their own phases.
  - Upstream issue **#1** (the daemon does not enforce the handshake, `transport.rs:1346`; the gate is
    interface-side only, `transport.rs:931`) is **not** fixed here — it changes transport behaviour, not
    analysis behaviour, and blocks nothing in this phase. See decision 24.
  - Upstream issue **#11** (flaky Phase-3 timing tests) is **not** fixed here: the honest fix is a soak
    test, which a prior reviewer called a USER decision. See decision 25.
  - No analysis **cache**: classification stays derived on read (`architecture.md` "Classification is
    Derived, never Stored", R003 at `architecture.md:13`). See decision 15.
- **Architecture / conventions** (all from scout; `[scout]` marks the source in the Findings log):
  - Amounts are `i64` minor units, never float (`architecture.md:336-338`). `core/src/domain/money.rs:33`
    `pub fn parse(text: &str) -> Result::<i64, ParseError>`, `:88 pub fn print(amount: i64) -> String`
    (canonical `-1.234,56 €`). **Rendering money is `money::print` — nothing else.**
  - Domain errors are `money::ParseError` / `date::ParseError`; `MoneyParseError`/`DateParseError` exist
    only as local `use … as` aliases (`transaction.rs:18,20`). There is no cross-module alias to reuse.
  - `Transaction` (`core/src/domain/transaction.rs:42-65`): `id, date (YYYY-MM-DD String), amount (i64),
    currency, description, counterparty, source`, `#[derive(Debug, Eq, PartialEq)]`; `is_income()` at `:143`
    is `amount > 0`; **category is not a field**.
  - The merge-on-read view (`core/src/technical/view.rs`): `:53 pub struct Entry { pub transaction:
      Transaction, pub category: Classification }`, `:69 pub struct View { pub entries: Vec::<Entry>,
      pub orphan_overrides: Vec::<Override>, pub dangling_refs: Vec::<DanglingRef>,
      pub rule_errors: Vec::<RuleError>, pub corrupt_lines: usize }`,
      `:138 pub fn read_view(dir: &std::path::Path) -> View`. `read_view` reads only
      `transactions.csv`, `annotations.csv`, `overrides.csv`, `rules/` — **it never reads
      `categories.csv`** (view.rs:139-144) and never validates rule categories.
  - `classify::Classification` (`core/src/domain/classify.rs:67`): `Category(String) | Unknown`.
  - `category::Tree` (`core/src/domain/category.rs:64`): `from_csv` :150 (forward parent references are
    allowed — parents resolve after the whole text is read, :146-148), `roots` :177, `children` :184
    (file order), `validate` :199 / `is_valid` :220. The fixture `categories.csv` makes
    `finance:expense:transport` a valid path (verified: test `three_level_path_is_valid` passes).
  - Event writing (`core/src/protocol/writer.rs`): `start(cmd, total)` :234, `progress(stream, sink,
    file: Option<String>)` :268, `log(stream, sink, level: String, text: String)` :291,
    `succeed(stream, sink, payload: &Map<String, Value>)` :305, `fail(stream, sink, reason: ErrorReason,
    text: String)` :317, `cancel(stream, sink)` :327, `request_cancel` :247, `cancel_requested(&stream)` :252.
    `writer::run` **cannot** express a command whose `result` payload summarises its items — the pinned note
    (`interpret.rs:192-201`) says a wired phase drives `start/progress/log/succeed` itself and checks the
    cancel flag between items. `import_work.rs` is the working precedent (`impl interpret::Work for
    ImportWork` at :146-161, `Command::Import(path) => …`, `other => self.fallback.do_command(other, sink)`;
    fallback = `interpret::wired_nowhere()`, decision 26).
  - Only the five documented error codes may be emitted: `unknown-command`, `plugin-failed`,
    `converter-missing`, `io`, `busy` (`core-protocol.md:180-181`; `interpret.rs:60-62`;
    `proto/src/event.rs:68 ErrorReason`).
  - Atomic writes (`core/src/technical/fs_atomic.rs`): `write_temp` :98 (`<path>.phiptmp`, same dir),
    `back_up` :114 (`<path>.bak`), `commit` :130, `write_atomic(path, content: &str) ->
    Result::<BackupOutcome, WriteError>` :143 (creates the directory). `.gitignore` lists `*.bak`
    but **not** `*.phiptmp` (Phase-2 finding, still open).
  - External-program precedent (`core/src/technical/pdf_text.rs:79 extract_text_with(program, path)`):
    spawn attempt, `ErrorKind::NotFound` → typed `PdfTextError::ToolMissing` → `ImportError::ConverterMissing`
    (`importer_host.rs:1127 import_error_of`) → `converter-missing`. Missing converter is a warning, not an
    error (`architecture.md:300`).
  - Plugin subprocess precedent (`core/src/technical/importer_host.rs`): `ExecImporter { argv, name,
    timeout_secs }` :347, `spawn_child` :377 (piped stdin/stdout/stderr), `read_line(timeout_secs)` :627
    (`recv_timeout`), `reap` :654 (kill → wait → bounded join → stderr drain),
    `drain_stderr_bounded` :782 (**stderr read only after reaping** = issue #3),
    `DEFAULT_PLUGIN_TIMEOUT_SECS: u64 = 60` :85, `REAPER_GRACE_SECS: u64 = 2` :96,
    `exec_importers(data_dir)` :995 (existence checked with `fs::metadata`, no exec bit = issue #2),
    `plugins` config key (`transport_cfg.rs:132 PLUGINS_KEY`, `plugins_from_config` :547,
    `ConfigIssue { NotReadable, NotTOML, WrongType, PortOutOfRange, PluginsNotStrings, PluginNotFound }` :233).
    Config file = `phin.toml` in the data dir (`CONFIG_FILE_NAME` :102), parsed with `basic-toml`.
  - Test conventions: `#[test]`s live in `#[cfg(test)] mod tests` inside the module; each module owns a
    **module-private scratch parent** (`const SCRATCH_PARENT: &str = "test-scratch-<module>"`) — the shared
    parent was removed because it caused flakes (`view.rs:178-189`, Phase-3 review 3.6b/3.6c). Doc-reading
    tests are the source-of-truth gate: `proto/src/codec.rs:358/374/399/438`
    `read_core_protocol_doc` / `marker_id` / `doc_examples` / `fenced_block_after` are `#[cfg(test)] pub`
    **and therefore NOT callable from `core`'s tests** (`#[cfg(test)]` items are not compiled into a
    dependency build; decision 36 keeps one owner in proto). `find_testdata_dir()` is duplicated per module
    (`id.rs:238`, `pdf_text.rs:108`, `parse_bank_a.rs:260`, `importer_host.rs:1188`, …).
  - Gates: `cargo test --workspace` green per task; `cargo build --workspace --all-targets` with 0 warnings;
    reviewers use `RUSTFLAGS=-Dwarnings cargo build --workspace --all-targets`. `cargo fmt --check` is
    **not** a gate ("core 286 diffs", Phase-3 log:44) and there is no clippy gate ("repo has no clippy
    gate", Phase-4 log:29). No `#[ignore]`/feature-gated tests exist. Never run two `cargo test`
    invocations at once (issue #11); probe timing-sensitive suites with
    `timeout 120 cargo test -p core --test <name>`.
- **Build / test / lint commands:** `cargo test --workspace` (all suites: proto 33, core lib 290, core bin 4,
  `core/tests/import_lifecycle.rs` 4, `core/tests/transport_lifecycle.rs` 4, cli/tui/gui 1 each),
  `cargo test -p core <filter>`, `cargo test -p proto`,
  `cargo build --workspace --all-targets`, `RUSTFLAGS=-Dwarnings cargo build --workspace --all-targets`.
  Toolchain: `cargo 1.100.0`/`rustc 1.100.0`. `pdftotext 26.11.0` present, `libreoffice 26.2.7.3` present,
  `pandoc` **NOT installed**, `python3 3.14.10` present. Network + crates.io index are reachable
  (`https://index.crates.io/config.json` → 200); `~/.cargo/registry/cache` holds 29 `.crate` files; no
  `vendor/`, no `.cargo/config*`.
- **Environment facts:** workspace members `proto, core, cli, tui, gui`, edition 2024, version 0.0.0.
  `core/Cargo.toml` deps: `proto` (path), `regex 1.13.1`, `sha2 0.11.1`, `serde_json 1.0`,
  `basic-toml 0.1.10`; `proto/Cargo.toml`: `serde_json 1.0`. Neither has a `[dev-dependencies]` section.
  `Cargo.lock` has 31 packages; no markdown/HTML crate in it. Style: turbofish
  (`Result::<T, E>`, `Vec::<T>`), explicit `return`, typed errors (never panic), domain layer stays free of
  `std::fs`/`std::net` (enforced by `technical/mod.rs`'s `domain_sources_are_pure` test, `FORBIDDEN` array).
- **Global decisions** (binding for every task; rationale in the Decision log):
  - **D-A** `analyze` is a new documented command (`name`, optional `year`); `report` stays as documented.
  - **D-B** Reports land in `<data_dir>/report/<year>.<ext>` (`report/all.<ext>` when no year), overwritten
    atomically, documented as **generated output**.
  - **D-C** Analysis plugins are configured by the new key `analysis_plugins`; canonical built-in slugs are
    `categories`, `cashflow`, `monthly-stats`, `biggest-spendings`.
  - **D-D** The plugin table event gains optional `"column_types"`; absent → column-name inference.
  - **D-E** PDF: `pandoc` when present, else LibreOffice from the rendered HTML. ODS: LibreOffice only.
    No `converters` config key in this phase.
  - **D-F** 5.6's tests parse their output with `dev-dependencies` `pulldown-cmark` + `quick-xml`.
  - **D-G** Issues #2/#3/#4 are fixed before the analysis host; one commit per issue where possible, and
    every commit naming the fixes mentions its issue number (supersedes ground rule 7's single commit for
    those tasks only).
  - **D-H** Unknown analysis name / unknown report format reuse `unknown-command` (no new error code).
  - **D-I** Fixture-derived expected values are hand-computed (values pinned in the task sections); a test
    may never read its expectation back from the code under test.
- **Definitions:** *task* = one deliverable + one success check + one review pass; *brief* = global context +
  one task section; *budget* = the size signals in `doc/implementation/plan.md:11-13`. "Store" = a data
  directory holding `transactions.csv`, `annotations.csv`, `overrides.csv`, `categories.csv`, `rules/`,
  `state/`. "Table" = the Phase 5.1 value type.

## Task index

| id | title | depends on | files | success check | size |
|----|-------|------------|-------|---------------|------|
| T1 | 5.1 Analysis result data type (`domain::result`) | — | `core/src/domain/result.rs` (new), `core/src/domain/mod.rs` | `cargo test -p core result` green (money column sums exactly with `checked_add`; overflow is typed; header-only empty table observable) | 2 files, ~150 LOC + 12 tests |
| T2 | 5.2 Income/expense per category per year (`analyse_categories`) | T1 | `core/src/technical/analyse_categories.rs` (new), `core/src/technical/mod.rs` | `cargo test -p core analyse_categories` green (fixture rows assert the pinned totals below) | 2 files, ~200 LOC + 8 tests |
| T3 | 5.3 Cash flow per month (`analyse_cashflow`) | T1, T2 | `core/src/technical/analyse_cashflow.rs` (new), `core/src/technical/mod.rs` | `cargo test -p core analyse_cashflow` green (pinned monthly/balance values; negative balance) | 2 files, ~120 LOC + 6 tests |
| T4 | 5.4 Monthly statistics + biggest spendings (`analyse_stats`) | T1, T2 | `core/src/technical/analyse_stats.rs` (new), `core/src/technical/mod.rs` | `cargo test -p core analyse_stats` green (top-N with ties; empty month; single-transaction month) | 2 files, ~170 LOC + 9 tests |
| T5 | 5.6 Markdown + HTML renderer (`render`) + dev-dependencies | T1 | `core/src/technical/render.rs` (new), `core/src/technical/mod.rs`, `core/Cargo.toml` | `cargo test -p core render` green (`\|` and `<` escaping, header-only empty table, `money::print` numbers, output parsed by `pulldown-cmark`/`quick-xml`) | 3 files, ~230 LOC + 10 tests, 2 dev-deps |
| T6 | 5.7 Export delegation (pandoc / LibreOffice) + atomic byte output | T5 | `core/src/technical/export.rs` (new), `core/src/technical/mod.rs`, `core/src/technical/fs_atomic.rs`, `doc/architecture/architecture.md` | `cargo test -p core export` green (missing converter → typed `ConverterMissing` + no partial file; real LibreOffice PDF/ODS bytes; byte round-trip + `.bak`) | 4 files, ~230 LOC + 8 tests |
| T7 | Plugin-process plumbing extraction (prepares #3/#4) | — | `core/src/technical/plugin_proc.rs` (new), `core/src/technical/importer_host.rs` (delegate), `core/src/technical/mod.rs` | `cargo test -p core importer_host` green with the **existing** fake-plugin test names and assertions unchanged | 3 files, ~180 LOC moved/extracted, no new behaviour |
| T8 | Continuous stderr pump + total timeout budget (#3, #4) | T7 | `core/src/technical/plugin_proc.rs`, `core/src/technical/importer_host.rs` (tests) | `cargo test -p core plugin_proc` + `cargo test -p core importer_host` green (chatty-stderr plugin succeeds; slow-chatter plugin hits the total budget) | 2 files, ~120 LOC + 4 tests, 2 commits |
| T9 | Plugin executable-bit check (#2) | T7 | `core/src/technical/transport_cfg.rs`, `core/src/technical/importer_host.rs` | `cargo test -p core transport_cfg` + `cargo test -p core exec_importers` green (`PluginNotExecutable` named; missing file still `PluginNotFound`) | 2 files, ~70 LOC + 4 tests |
| T10 | 5.5 Analysis plugin host + `analysis_plugins` key + plugin-protocol amendment | T1, T2, T8, T9 | `core/src/technical/analysis_host.rs` (new), `core/src/technical/mod.rs`, `core/src/technical/transport_cfg.rs`, `doc/architecture/plugin-protocol.md`, `doc/architecture/data-files.md`, `core/tests/fixtures/fake_analysis.py` (new) | `cargo test -p core analysis_host` green (built-in and plugin tables compare equal; plugin error → warning, built-ins still run) | 6 files, ~260 LOC + 9 tests |
| T11 | 5.8a `analyze` vocabulary: command table + typed view + core-protocol row/examples | — | `doc/architecture/core-protocol.md`, `proto/src/commands.rs`, `proto/src/command.rs`, `core/src/protocol/interpret.rs` | `cargo test -p proto` green (new `analyze` row + both new doc examples are swept by the existing doc tests; documented-command count 8 → 9) | 4 files, ~90 LOC + 2 test updates |
| T12 | 5.8b `analyze`/`report` wired into the interpreter (`AnalysisWork`) + report output files | T3, T4, T5, T6, T10, T11 | `core/src/protocol/analysis_work.rs` (new), `core/src/protocol/mod.rs`, `core/src/main.rs`, `doc/architecture/data-files.md`, `doc/architecture/architecture.md` | `cargo test -p core analysis_work` + `cargo test -p core --test import_lifecycle` green (scripted `analyze biggest-spendings` and `report markdown` yield the pinned payloads and the pinned file; unknown name/format → `unknown-command`) | 5 files, ~250 LOC + 10 tests |
| T13 | Hygiene sweep: review nits #12, stale Phase-3/4 comments, `.gitignore` | T12 | `core/src/technical/import_work.rs`/`core/src/protocol/import_work.rs`, `core/src/protocol/interpret.rs`, `proto/src/handshake.rs` (test comments), `.gitignore` | `cargo test --workspace` green + `RUSTFLAGS=-Dwarnings cargo build --workspace --all-targets` 0 warnings | 4-5 files, ~40 LOC, 1 commit naming #12 |

## Tasks

### T1 — 5.1 Analysis result data type (`core/src/domain/result.rs`)

**Depends on:** —
**Size:** 2 files, ~150 production LOC + ~12 tests; one pure module, one success check. Well inside budget.
**Why:** Every Phase-5 analysis, the renderer, the plugin host and the `analyze` payload need one shared,
typed table value. Plan item 5.1: "Table/series type with typed cells (money, count, date)"
(`doc/implementation/plan.md:245-246`).
**Current state (verified):**
- `core/src/domain/result.rs` does not exist; nothing Phase-5 exists anywhere except the refusal stub
  `WiredNowhere::Report` (`core/src/protocol/interpret.rs:663-701`), which says "plan items 5.1-5.8".
- `core/src/domain/mod.rs:13-20` lists the eight domain modules, alphabetical, no re-exports.
- Domain purity is enforced by `core/src/technical/mod.rs`'s `domain_sources_are_pure` test with
  `FORBIDDEN: [&str; 2] = ["std::fs", "std::net"]` — so `result.rs` may not import `std::fs`/`std::net`,
  but `serde_json` is allowed (it is a pure value type and `core` already depends on it). Do not put any
  JSON in this module anyway (T10 owns the JSON form).
- `money::print` (`core/src/domain/money.rs:88`) is the only formatter; the domain stores amounts as `i64`.
**Already done / do not redo:** nothing to redo. Do not add a `Table::render` (T5 owns rendering) and do
not add JSON conversion (T10 owns it).
**Files:**
- Create: `core/src/domain/result.rs`
- Modify: `core/src/domain/mod.rs` (add `pub mod result;` in the alphabetical list, after `pub mod money;`)
- Read first: `core/src/domain/money.rs`, `core/src/domain/transaction.rs`, `core/src/domain/classify.rs`,
  `core/src/technical/mod.rs` (the purity test).

**Interfaces:** (consumed by T2, T3, T4, T5, T6, T10, T12 — names are exact)
- Produces:
  - `pub enum Kind { Money, Count, Date, Text }` (`Copy, Clone, Debug, Eq, PartialEq`)
  - `pub fn kind_name(kind: Kind) -> &'static str` — `Money → "money"`, `Count → "count"`, `Date → "date"`, `Text → "text"`
  - `pub fn kind_of_name(name: &str) -> Kind` — `"money"|"count"|"date"|"text"` map to those; anything else → `Kind::Text`
  - `pub enum Cell { Money(i64), Count(i64), Date(String), Text(String) }` (`Debug, Eq, PartialEq`)
  - `pub struct Column { pub name: String, pub kind: Kind }` (`Debug, Eq, PartialEq, Clone`)
  - `pub struct Table { pub name: String, pub columns: Vec::<Column>, pub rows: Vec::<Vec::<Cell>> }` (`Debug, Eq, PartialEq`)
  - `pub fn table(name: &str, columns: &[(&str, Kind)]) -> Table` — rows empty
  - `pub enum AddError { ColumnCount(usize, usize), KindMismatch(usize, Kind, Kind) }` (`Debug, Eq, PartialEq`)
  - `pub fn add_row(table: &mut Table, row: Vec::<Cell>) -> Result::<(), AddError>`
  - `pub fn header(table: &Table) -> Vec::<String>` — the column display names, in order
  - `pub fn is_empty(table: &Table) -> bool` — true when `rows` is empty
  - `pub fn column_of(table: &Table, name: &str) -> Option::<usize>`
  - `pub fn money_sum(table: &Table, column: usize) -> Result::<i64, SumError>`
  - `pub enum SumError { NoSuchColumn(usize), NotMoney(usize, Kind), Overflow(usize) }` (`Debug, Eq, PartialEq`)

**Implementation brief:**
1. Define the types above in source order (`Kind`, `Kind` helpers, `Cell`, `Column`, `Table`, `AddError`,
   `SumError`) with `///` docs on every public item (ground rule 6). Module doc: why a typed cell exists
   (Q1 exactness: a renderer must never guess which column carries money) and that no formatting happens here.
2. `add_row`: first check `row.len() == table.columns.len()` → `AddError::ColumnCount(got_len, expected)`
   (pin the payload order: got first, expected second); then scan left to right for a kind mismatch
   (`Cell::Money` required for `Kind::Money`, etc.) → `AddError::KindMismatch(index, expected, got)`;
   only push the row when both checks pass — an error never leaves a half-added row.
3. `money_sum`: `column >= columns.len()` → `NoSuchColumn(column)`; the column's `kind != Kind::Money` →
   `NotMoney(column, kind)`; otherwise fold with `i64::checked_add` over the `Cell::Money(_)` values in
   `rows`; `None` → `Overflow(column)` (Q1: silent wraparound is forbidden). Empty rows → `Ok(0)`.
4. `header` returns `columns.iter().map(|c| c.name.clone()).collect()`; `is_empty` looks at `rows`.
5. Add `pub mod result;` to `core/src/domain/mod.rs`.
**Pinned decisions:** D-I (values in this task are hand-written test data, not derived from the code);
decision 11 (cells store `i64` minor units; formatting happens only in the renderer).
**Edge cases / failure modes:**
- A `Kind::Money` column holding `Cell::Count` must be rejected at `add_row`, never rendered later.
- `money_sum` on a column whose rows contain a non-`Money` cell: `add_row` already guarantees
  type-consistency, so a defensive `match … unreachable!()` is forbidden — use a `None`-safe fold
  that ignores nothing (rows can only be built through `add_row`).
- Overflow: a sum built past `i64::MAX` must be `SumError::Overflow`, not a panic.
- `kind_of_name("MONEY")` → `Kind::Text` (lower-case only; the wire form is lower-case).
**Tests / acceptance criteria** (in-module `#[cfg(test)] mod tests`, no I/O — a pure domain type):
1. `money_column_sums_exactly_no_float` — a `Money` column `[123456, -123456, 100050, -3999]` sums to `100051`.
2. `money_sum_overflows_as_a_typed_error` — `sum` of `[i64::MAX, 1]` → `SumError::Overflow(0)`.
3. `money_sum_refuses_a_count_column` — `Kind::Count` column → `SumError::NotMoney(1, Kind::Count)`;
   out-of-range column → `SumError::NoSuchColumn(7)`.
4. `add_row_rejects_wrong_column_count` — 3 cells into a 4-column table → `AddError::ColumnCount(3, 4)`
   and `table.rows.len()` unchanged.
5. `add_row_rejects_wrong_cell_kind_and_keeps_the_row_out` — `Cell::Count(2)` into a `Money` column →
   `AddError::KindMismatch(2, Kind::Money, Kind::Count)`; `rows.is_empty()`.
6. `empty_table_is_empty_and_its_header_survives` — `is_empty(&table("t", &[("a", Kind::Money)]))` is true and
   `header(...)` == `["a"]` (5.1's "empty table renders as header only" is completed in T5's renderer test).
7. `kind_names_round_trip` — `kind_of_name(kind_name(k)) == k` for all four; `kind_of_name("dmark") == Kind::Text`.
8. `table_is_comparable_and_stores_minor_units` — two equal tables compare `Eq`; `Cell::Money(-123456)`
   is stored verbatim (no scaling anywhere).
**Success check:** `cargo test -p core result` green, then `cargo test --workspace` green.
**Review focus:** no formatting/JSON in the domain module; no `panic!`/`unwrap` in production code;
`checked_add` really used; `add_row` leaves no partial row; module doc states why cells are typed.
**Out of scope:** rendering, JSON encoding, sorting/filtering helpers (analyses do that in T2–T4), any change
to `money.rs`/`date.rs`/`transaction.rs`, any interpreter change.

### T2 — 5.2 Income/expense per category per year (`core/src/technical/analyse_categories.rs`)

**Depends on:** T1
**Size:** 2 files, ~200 LOC + ~8 tests; one analysis + one shared helper the later tasks need. Within budget.
**Why:** Plan item 5.2 (built-in analysis of requirement 005 / `architecture.md:220`).
**Current state (verified):**
- Analyses consume the view: `view::read_view(&dir)` → `Vec::<view::Entry>` (T1 context).
- The category tree must be loaded separately: `store_sidefiles::read_categories(text)`
  (`store_sidefiles.rs:290`) returns `(Vec::<Line>, Vec::<Category>, Vec::<ReadError>)`, but `Category`'s
  fields are **private** (`:263-271`) so it cannot feed `category::from_csv`. Therefore the tree comes from
  `crate::domain::category::from_csv(&text)` (private names are irrelevant here). `read_view` does not read
  `categories.csv` (`view.rs:139-144`).
- Classification comes from `classify::classify` through the view; `Entry::category` is
  `Classification::Category(path)` or `Unknown` (`classify.rs:67`).
- `view.rs` test consts `FIXTURE_TRANSACTIONS_TEXT` (`view.rs:393`) / `FIXTURE_CATEGORIES_TEXT` (`:375`) /
  `FIXTURE_SAMPLE_RULES_TEXT` (`:356`) are `#[cfg(test)]`-private to `view.rs` and cannot be reused; each
  test module copies the text it needs (the existing convention).
- Verified fixture semantics (cross-checked against the passing view test
  `fixture_rules_and_categories_drive_classification`, `view.rs:422-466`): classification matches
  `transaction.description` only, ASCII case-insensitive substring, **last rule wins**, no overrides.
**Already done / do not redo:** the view, `classify`, `category::Tree`. Do not touch `view.rs`
(its consts and scratch parent stay as they are).
**Files:**
- Create: `core/src/technical/analyse_categories.rs`
- Modify: `core/src/technical/mod.rs` (add `pub mod analyse_categories;` to the `pub mod` list at the top,
  keeping the module's own ordering; the list currently reads `csv, fs_atomic, id, importer_host, link,
  mail_annotate, mbox, parse_bank_a, parse_bank_b, pdf_text, state, store_transactions, store_sidefiles,
  transport, transport_cfg, view`)
- Read first: `core/src/technical/view.rs`, `core/src/domain/category.rs`, `core/src/domain/classify.rs`,
  `testdata/categories.csv`, `testdata/transactions.csv`, `testdata/rules/sample.rules`.

**Interfaces:**
- Consumes: `view::Entry`, `category::Tree`, `classify::Classification`, `result::{Table, Cell, Kind, table}`.
- Produces (exact names, consumed by T3, T4, T10, T12):
  - `pub const NAME: &str = "categories";`
  - `pub const UNKNOWN_LABEL: &str = "unknown";`
  - `pub fn category_tree_at(data_dir: &std::path::Path) -> (crate::domain::category::Tree, Vec::<String>)`
  - `pub fn belongs_to_year(date: &str, year: Option::<i64>) -> bool`
  - `pub fn income_expense_by_category(entries: &[view::Entry], tree: &crate::domain::category::Tree, year: Option::<i64>) -> result::Table`
  - `pub fn unknown_category_paths(entries: &[view::Entry], tree: &crate::domain::category::Tree) -> Vec::<String>`

**Implementation brief:**
1. `category_tree_at`: read `<data_dir>/categories.csv` with `std::fs::read_to_string`; missing file or
  unreadable → `(Tree::default-equivalent, complaints)` — build the tree from the empty string
  (`category::from_csv("")` yields an empty tree, no error) and push one complaint string per failure,
  typed by hand: `"categories.csv could not be read: {reason}"`. A `category::BuildError` → push
  `format!("categories.csv is not a valid category tree: {reason:?}")` and use the empty tree. Never
  panic, never abort.
2. `belongs_to_year(date, year)`: `None` → true; `Some(y)` → the first four code points of `date` equal
  `y` rendered as four digits and the fifth character is `-` (dates are canonical `YYYY-MM-DD`).
  `Option::<i64>` because the protocol parameter is `ValueKind::Integer` (`proto/src/commands.rs:143`).
3. `income_expense_by_category`:
   a. Table name: `format!("income and expenses by category{}", suffix)` with `suffix = " {year}"` when a
     year is given, `""` otherwise (decision 16).
   b. Columns: `[("category", Kind::Text), ("income", Kind::Money), ("expense", Kind::Money), ("net", Kind::Money)]`.
   c. Bucket each entry whose date passes `belongs_to_year`:
      `Classification::Category(path)` that `category::is_valid(tree, path)` → that path; anything else
      (`Unknown`, or a path the tree does not know) → the pseudo-category `UNKNOWN_LABEL`.
   d. For each real category, the row's values cover that category **and every descendant** (roll-up):
      walk `category::children` recursively. Income = sum of `amount > 0`; expense = sum of `amount < 0`
      (kept negative); net = income + expense. Use `i64::checked_add`; an overflow pushes a warning into
      the caller's hands — pin: overflow is reported by skipping that category's own arithmetic never:
      instead `add_row` a row whose values are the saturating-safe… — **no**: pin exactly: on
      `checked_add` overflow the analysis returns a row of three `Cell::Text("overflow")` values for that
      category (documented in the module doc) and T12 logs a warning.
   e. Row order: depth-first pre-order over the tree in file order (`category::roots` then
     `category::children`), i.e. `food, food:supermarket, food:restaurant, finance, finance:income,
     finance:expense, finance:expense:transport, leisure, leisure:books, leisure:games`. Omit a category
     (and a whole empty subtree) whose roll-up count is 0. The `unknown` row, when its count > 0, is last.
   f. Category cell text = the full path (`food:supermarket`); the unknown row's text is `UNKNOWN_LABEL`.
4. `unknown_category_paths`: the distinct `Classification::Category` paths that fail
   `category::is_valid`, sorted ascending, deduplicated (T12 logs them as a warning; a rule that points at a
   category the user never defined must be visible).
**Pinned decisions:** D-I (pinned fixture numbers below), decision 12 (roll-up + omission + unknown-last),
decision 10 (year filter semantics), D-H (no new error code anywhere in this task).
**Edge cases / failure modes:**
- `categories.csv` missing → empty tree → every classified path is invalid → **all rows collapse into the
  `unknown` row** (assert this, it is the "unknown category grouped separately" gate).
- A rule category absent from the tree (e.g. `travel:taxi` with no `travel` node) counts as `unknown` AND
  appears in `unknown_category_paths`.
- `finance:expense:utilities` (a real tree node with no transactions) is omitted.
- `year: Some(2023)` → zero rows (`result::is_empty` true), header still present.
**Tests / acceptance criteria** (scratch parent `const SCRATCH_PARENT: &str = "test-scratch-analyse-categories"`,
module-private; the store is written with `std::fs::write` from a private `fn make_store(name)` /
`fn write_file(store, rel, text)` / `fn remove_scratch(store)` triple copied in shape from `view.rs:190-244`):
1. `fixture_store_yields_the_hand_computed_category_totals` — store built from `testdata/transactions.csv`
   (22 rows copied verbatim), `rules/sample.rules` written as `rules/10-built-in.rules`, `categories.csv`
   copied verbatim; `income_expense_by_category(&view.entries, &tree, None)`. The hand-computed expected
   rows, in this exact order, each as `(category, income, expense, net)` — `net = income + expense`:
   | # | category | income | expense | net |
   |---|----------|--------|---------|-----|
   | 1 | `food` | `0` | `-40015` | `-40015` |
   | 2 | `food:supermarket` | `0` | `-32435` | `-32435` |
   | 3 | `food:restaurant` | `0` | `-7580` | `-7580` |
   | 4 | `finance` | `139000` | `-5499` | `133501` |
   | 5 | `finance:income` | `139000` | `0` | `139000` |
   | 6 | `finance:expense` | `0` | `-5499` | `-5499` |
   | 7 | `finance:expense:transport` | `0` | `-5499` | `-5499` |
   | 8 | `leisure` | `0` | `-9435` | `-9435` |
   | 9 | `leisure:books` | `0` | `-2646` | `-2646` |
   | 10 | `leisure:games` | `0` | `-6789` | `-6789` |
   | 11 | `unknown` | `6948` | `-129955` | `-123007` |
   Assert `rows.len() == 11`, that no `finance:expense:utilities` row exists, and that the `net` values of
   rows 1, 4, 8 and 11 sum to `-38956` (the hand-computed total of all 22 fixture rows).
   Derivation for the reviewer: 6 rows land in `food:supermarket`
   (`-3999, -12990, -8990, -3456, -1500, -1500`), 2 in `food:restaurant` (`-5990, -1590`), 3 in
   `finance:income` (`42000, 85000, 12000`), 2 in `finance:expense:transport` (`-2500, -2999`), 2 in
   `leisure:books` (`-1847, -799`), 1 in `leisure:games` (`-6789`), 6 are `unknown`
   (`-123456, 2599, -1999, 899, -4500, 3450`); the view test `fixture_rules_and_categories_drive_classification`
   already pins six unknowns for this store.
2. `unknown_categories_are_grouped_and_reported` — same store, `categories.csv` deleted → exactly one row
   named `unknown` with `(145948, -184904, -38956)`; `unknown_category_paths` returns
   `["finance:expense", "finance:expense:transport", "finance:income", "food:restaurant", "food:supermarket",
   "leisure:books", "leisure:games"]` sorted (the 7 distinct paths the sample rules produce).
3. `year_filter_selects_rows_by_the_date_prefix` — `Some(2024)` == `None` for this store
   (all dates are 2024); `Some(2023)` → `result::is_empty(&table)` true and `header(&table).len() == 4`.
4. `subcategory_rollup_is_a_parent_plus_descendants_only_once` — a two-row store
   (`food;supermarket` + a `food`-classified transaction with description `GARDEN CENTRE` and rule
   `GARDEN CENTRE -> food`) shows `food` carrying both amounts and `food` itself receiving its own row's
   amount once.
5. `empty_store_is_an_empty_table_with_a_header` — an empty `transactions.csv` → `is_empty` true,
   `header` == `["category", "income", "expense", "net"]`.
6. `missing_categories_csv_is_reported_as_a_complaint` — `category_tree_at` on a store without
   `categories.csv` returns complaints.len() == 1 and an empty tree (roots empty).
7. `rule_category_outside_the_tree_is_reported` — store with rule `MEGA MART -> travel:taxi` → the transaction
   lands in `unknown` and `unknown_category_paths == ["travel:taxi"]`.
8. `fixture_store_total_matches_the_hand_checked_minus_38_956` — arithmetic guard: sum over all rows of
   `Money` `net` of the three roots + unknown == `-38956` (Q1 exactness; the number is hand-computed, never
   read from the code).
**Success check:** `cargo test -p core analyse_categories` green, then `cargo test --workspace` green.
**Review focus:** roll-up must not double count a transaction in both parent and child; the unknown bucket
lumps `Unknown` and tree-invalid paths (a reviewer checks both are covered); the pinned numbers are exactly
as written above; no `std::fs` outside `category_tree_at`.
**Out of scope:** cash flow, statistics, rendering, plugin hosts, interpreter wiring, `view.rs` changes.

### T3 — 5.3 Cash flow: cumulative balance per month (`core/src/technical/analyse_cashflow.rs`)

**Depends on:** T1, T2 (for `belongs_to_year`)
**Size:** 2 files, ~120 LOC + 6 tests. Small, one deliverable.
**Why:** Plan item 5.3 (`architecture.md:220` names "cash flow" among the built-in analyses).
**Current state (verified):** as T2; no cash-flow code exists.
**Already done / do not redo:** `T2::belongs_to_year` is the year filter — do not write a second one.
**Files:** Create `core/src/technical/analyse_cashflow.rs`; Modify `core/src/technical/mod.rs`
(`pub mod analyse_cashflow;`); Read first: `core/src/technical/analyse_categories.rs` (T2's result),
`core/src/technical/view.rs`.
**Interfaces:**
- Consumes: `view::Entry`, `result::{Table, Cell, Kind, table}`, `analyse_categories::belongs_to_year`.
- Produces: `pub const NAME: &str = "cashflow";` and
  `pub fn cash_flow(entries: &[view::Entry], year: Option::<i64>) -> result::Table`
**Implementation brief:**
1. Table name: `format!("cash flow{}", suffix)` with the same year-suffix rule as T2 (decision 16).
2. Columns: `[("month", Kind::Text), ("income", Kind::Money), ("expense", Kind::Money),
   ("net", Kind::Money), ("balance", Kind::Money)]`.
3. Group entries passing `belongs_to_year` by `month = &date[..7]` (`"2024-01"`); months ascending
   (lexicographic on the ISO key is chronological). Income/expense per month as in T2 (positive/negative
   `amount`, negatives kept negative).
4. `balance` = running total over **all months of the filtered set in ascending order**, i.e.
   `balance[n] = balance[n-1] + net[n]`, started at 0. Negative balances are written as-is, never clamped
   (plan 5.3: "negative balance represented (no clamp)").
5. `i64::checked_add` for both the monthly folds and the running balance; on overflow write the row's money
   cells as `Cell::Text("overflow")` from that column on (same convention as T2, so a renderer never sees a
   wrong number). Months before the first month that contains a transaction are **not** synthesised
   (a store with only February shows one row).
**Pinned decisions:** decision 10, D-I, decision 11.
**Edge cases / failure modes:** a store whose only transactions are income shows `expense = 0`; an all-year
store spans 2023-12 and 2024-01 (balance must cross the year boundary); `Some(2024)` on a store with 2023
rows ignores 2023 entirely (balance starts at the first 2024 month).
**Tests / acceptance criteria** (scratch parent `"test-scratch-analyse-cashflow"`):
1. `fixture_store_cash_flow_months_and_balances` — the T2 fixture store (same 22 rows, verbatim),
   `None`: rows `2024-01 (42000, -148282, -106282, -106282)`,
   `2024-02 (85000, -14946, 70054, -36228)`, `2024-03 (0, -11379, -11379, -47607)`,
   `2024-04 (18948, -10297, 8651, -38956)`; the last `balance` equals the hand-computed total `-38956`
   (plan 5.3's "sum of income minus expenses equals final balance").
2. `income_minuses_expenses_equals_final_balance` — sum of the `income` column minus sum of the `expense`
   column (via `result::money_sum`) equals the last row's `balance`.
3. `negative_balance_is_not_clamped` — one store with a single `-5000` transaction in `2024-01`:
   `balance == -5000` and no row is dropped.
4. `months_are_ascending_across_a_year_boundary` — store with `2023-12-20 -1000` and `2024-01-05 +2500`:
   two rows, `2023-12` first, balances `-1000` then `1500`.
5. `year_filter_starts_the_balance_at_the_first_month_of_that_year` — the two-row store with
   `Some(2024)` → one row `2024-01 (2500, 0, 2500, 2500)`.
6. `empty_month_is_not_synthesised` — store with only `2024-03` transactions → exactly one row.
7. `cash_flow_name_and_columns_are_pinned` — `NAME == "cashflow"`, table name `"cash flow 2024"` for
   `Some(2024)`, `header` == `["month", "income", "expense", "net", "balance"]`.
**Success check:** `cargo test -p core analyse_cashflow` green, then `cargo test --workspace` green.
**Review focus:** the running balance is over the **filtered** set only; months are not padded; the fixture
numbers are asserted verbatim (reviewer re-adds the four monthly `net` values: they must total −38 956).
**Out of scope:** category tree (cash flow is category-free on purpose), rendering, plugin host, wiring.

### T4 — 5.4 Monthly statistics + biggest spendings (`core/src/technical/analyse_stats.rs`)

**Depends on:** T1, T2 (year filter)
**Size:** 2 files, ~170 LOC + ~9 tests; two tables in one module (they share the month/aggregation helper
and both belong to one plan item). Within budget.
**Why:** Plan item 5.4; `architecture.md:220` names "monthly statistics, biggest spendings" as built-ins.
**Current state (verified):** no stats code exists. `view::Entry.transaction.id` is available for
deterministic tie-breaking (`transaction.rs:42-65`).
**Already done / do not redo:** `analyse_categories::belongs_to_year` (T2). Do not read the category tree —
neither table classifies anything.
**Files:** Create `core/src/technical/analyse_stats.rs`; Modify `core/src/technical/mod.rs`
(`pub mod analyse_stats;`); Read first: `analyse_categories.rs` (T2 result), `analyse_cashflow.rs` (T3),
`core/src/domain/result.rs` (T1).

**Interfaces:**
- Consumes: `view::Entry`, `result::{Table, Cell, Kind, table, add_row}`, `analyse_categories::belongs_to_year`.
- Produces:
  - `pub const NAME_MONTHLY: &str = "monthly-stats";`
  - `pub const NAME_BIGGEST: &str = "biggest-spendings";`
  - `pub const TOP_DEFAULT: usize = 10;`
  - `pub fn monthly_stats(entries: &[view::Entry], year: Option::<i64>) -> result::Table`
  - `pub fn biggest_spendings(entries: &[view::Entry], year: Option::<i64>, top: usize) -> result::Table`

**Implementation brief:**
1. `monthly_stats` table name `format!("monthly statistics{}", year-suffix)`; columns
   `[("month", Kind::Text), ("transactions", Kind::Count), ("income", Kind::Money),
     ("expense", Kind::Money), ("average expense (truncated)", Kind::Money)]`.
   Per month (`&date[..7]`, ascending): `transactions` = count of all transactions in the month
   (`Cell::Count`); income/expense as in T2/T3; average expense = **sum of the negative amounts divided by
   the number of expense transactions in that month**, `i64` division truncating toward zero
   (`expense / count` in Rust already truncates toward zero), and `0` when the month has no expense
   transaction (no divide-by-zero). The truncation is stated in the column header itself — decision 13 —
   because Q1 forbids floats and no exact form exists for an average.
2. `biggest_spendings` table name `format!("biggest spendings{}", year-suffix)` (with a year this equals the
   doc example `plugin-protocol.md:126`: `"biggest spendings 2024"`); columns
   `[("date", Kind::Date), ("description", Kind::Text), ("amount", Kind::Money)]`.
   Rows: **expenses only** (`amount < 0`; income rows are never "spendings"), sorted by
   `amount.abs()` descending; a tie is broken by `date` ascending, then `description` ascending, then
   `id` ascending — a total order, so the output is deterministic for identical input (Q1).
   `top` caps the row count (`top = 0` → zero rows); when fewer expenses exist than `top`, all are shown.
3. Both use `i64::checked_add` for the folds and the same overflow convention as T2/T3
   (`Cell::Text("overflow")` in the money cells).
**Pinned decisions:** decision 13 (truncated average + header wording), decision 14 (expenses only,
sort + tie-break order), decision 16 (display names + year suffix), D-I.
**Edge cases / failure modes:**
- Month with income only → `expense 0`, `average expense (truncated) 0` (never a panic).
- `top: usize` larger than the expense count; `top == 0` (zero rows, header present); `year: Some(2023)`
  on an all-2024 store (zero rows both tables).
- `-i64::MIN`.abs() overflows: pin `.abs()` never used — sort by `amount` ascending is NOT the same;
  use `amount.abs()` only through `checked_abs()` and order `None` (i.e. `i64::MIN`) first, documented.
- The duplicate fixture pair (`80c63211a8`, `80c63211a8#2`, both `-1500`, same date and description)
  exercises the tie-break: the bare id sorts before `#2` (`#` is `0x23`, digits `0x30`… `0x39`, so
  `80c63211a8` < `80c63211a8#2` lexicographically).
**Tests / acceptance criteria** (scratch parent `"test-scratch-analyse-stats"`; the fixture store is the same
22 rows as T2/T3, written into the scratch store):
1. `fixture_monthly_stats_are_hand_computed` — rows exactly:
   `2024-01 (6, 42000, -148282, -29656)`, `2024-02 (4, 85000, -14946, -4982)`,
   `2024-03 (4, 0, -11379, -2844)`, `2024-04 (8, 18948, -10297, -2574)`; assertion of the average values is
   per row (`-148282/5 = -29656` trunc, `-14946/3 = -4982`, `-11379/4 = -2844` trunc, `-10297/4 = -2574` trunc).
2. `top_n_with_ties_is_deterministic` — the fixture store,
   `biggest_spendings(&entries, None, 3)` → `[(2024-01-24, "MEGA MART 04", -123456),
   (2024-01-28, "LIDL 0815", -12990), (2024-02-03, "ALDI 4711", -8990)]`;
   `biggest_spendings(&entries, None, TOP_DEFAULT)` → the first 10 amounts
   `[-123456, -12990, -8990, -6789, -5990, -4500, -3999, -3456, -2999, -2500]`.
3. `the_duplicate_pair_orders_by_id` — `top = 16` (the fixture has exactly 16 expense rows); the two `-1500`
   rows are adjacent and the ids are `80c63211a8` then `80c63211a8#2`, and `rows.len() == 16`.
4. `month_without_expenses_has_zero_average_and_no_panic` — a store with one `+5000` transaction in
   `2024-05`: row `(2024-05, 1, 5000, 0, 0)`.
5. `single_transaction_month` — one `-4000` transaction: `(2024-06, 1, 0, -4000, -4000)`.
6. `empty_month_and_empty_store_yield_header_only` — an empty store → `result::is_empty` true; headers are
   `["month", "transactions", "income", "expense", "average expense (truncated)"]` and
   `["date", "description", "amount"]`.
7. `income_rows_never_appear_in_biggest_spendings` — the fixture store has 6 income rows; assert no row of
   `biggest_spendings(&entries, None, 100)` has `amount > 0` and `rows.len() == 16`.
8. `names_and_slugs_are_pinned` — `NAME_MONTHLY == "monthly-stats"`, `NAME_BIGGEST == "biggest-spendings"`,
   `biggest_spendings(&[], Some(2024), TOP_DEFAULT).name == "biggest spendings 2024"` (the doc example's
   display text), `monthly_stats(&[], Some(2024), ).name == "monthly statistics 2024"`.
9. `biggest_spendings_top_zero_is_an_empty_table` — `top = 0` → `is_empty` true, `header().len() == 3`.
**Success check:** `cargo test -p core analyse_stats` green, then `cargo test --workspace` green.
**Review focus:** the tie-break total order (a reviewer must confirm no input can produce a different
sequence); the average divides by the **expense count**, not the transaction count; no `unwrap`/`panic`;
`Cell::Date` really used for the date column (T5 renders and T10 round-trips the type).
**Out of scope:** rendering, export, plugin host, interpreter, category tree.

### T5 — 5.6 Markdown + HTML renderer (`core/src/technical/render.rs`)

**Depends on:** T1
**Size:** 3 files, ~230 LOC + ~10 tests + 2 dev-dependencies. Slightly above the LOC warning, but the two
output formats are one deliverable (one table→text contract, one reviewer gate) and cannot be split without
splitting the escaping rules. Written reason, per ground rule 2.
**Why:** Plan item 5.6; `architecture.md:223-224`: "Renders analysis results as Markdown and HTML natively;
delegates PDF and ODS to the external converters."
**Current state (verified):**
- No renderer exists. `money::print(i64)` (`money.rs:88`) yields the canonical `-1.234,56 €` and is the
  pinned money format ("long numbers formatted per the currency format of 1.1", `plan.md:266-268`).
- `Cargo.lock` has no markdown/HTML parser; the repo has never used one. `core/Cargo.toml` has no
  `[dev-dependencies]` section.
- Adding a dependency is a supply-chain event in this repo's history: planning decision 3 (USER chose
  `serde_json`), decision 31/32 (`basic-toml` chosen over `toml` by **measured** transitive delta:
  "basic-toml adds exactly ONE new package … while toml … adds FIVE"), and the boundary rule
  "the TOML dep pair MUST land WITH 3.6b's consuming file, NEVER before it" (a `basic-toml`
  `warning: unused dependency` fired otherwise). Phase-3 log:44 records that a stray dependency is a
  build warning, and `RUSTFLAGS=-Dwarnings` is a reviewer gate.
- **UNVERIFIED (network-dependent):** the resolved versions of `pulldown-cmark` and `quick-xml` and whether
  pulldown-cmark's table support needs `Options::ENABLE_TABLES`. Verification step is pinned below (step 0)
  and the outcome must be recorded in the commit message. Network is available (index probe → HTTP/2 200).
**Already done / do not redo:** do not add a markdown/HTML dependency to `[dependencies]` (it is
test-only); do not re-implement money formatting.
**Files:**
- Create: `core/src/technical/render.rs`
- Modify: `core/src/technical/mod.rs` (`pub mod render;`), `core/Cargo.toml` (new `[dev-dependencies]` section)
- Read first: `core/src/domain/result.rs` (T1), `core/src/domain/money.rs`, `testdata/README.md`.

**Interfaces:**
- Consumes: `result::{Table, Cell, Kind, header, is_empty}` (T1), `domain::money::print`.
- Produces:
  - `pub fn markdown(table: &result::Table) -> String`
  - `pub fn html(table: &result::Table) -> String`
  - `pub fn markdown_document(title: &str, tables: &[&result::Table]) -> String`
  - `pub fn html_document(title: &str, tables: &[&result::Table]) -> String`

**Implementation brief:**
0. **First step:** `cargo init --name phin-dep-probe /tmp/phin-dep-probe`,
  `cargo add --dev pulldown-cmark` + `cargo add --dev quick-xml` there, `cargo build --tests`, and note
  (a) the resolved versions, (b) the transitive package count added to its `Cargo.lock`, (c) the
  `rust-version` keys of both crates. Keep the one with the smaller supply chain in case a rival exists;
  record all of it in the commit message. Only then add to `core/Cargo.toml`:
  `[dev-dependencies]` `pulldown-cmark = "<resolved>"` and `quick-xml = "<resolved>"` — never `default-
  features` that pull a CLI helper if a `default-features = false` build still gives the table/html
  support; state the choice in the same commit message.
1. Cell→text rules (shared private helper `fn cell_text(cell: &result::Cell) -> String`):
   `Money(x)` → `crate::domain::money::print(*x)`; `Count(x)` → `x.to_string()`; `Date(s)` → `s.clone()`;
   `Text(s)` → `s.clone()`.
2. `markdown(table)`: a pipe table, one line per row, no alignment padding:
   `| h1 | h2 |\n|---|---|\n| c1 | c2 |\n`. Header comes from `result::header`. A **table with no rows**
   emits the header line and the `|---|` line and nothing else (5.1's "header only").
   Markdown escaping (`fn escape_markdown(text: &str) -> String`) runs on cell text and header names:
   `\r`/`\n` → a single space, then `\` → `\\`, then `|` → `\|` (in that order — escaping `|` first would
   double-escape).
3. `html(table)`: `<table>`, `<thead><tr><th>…</th></tr></thead>`, then — only when `rows` is non-empty —
   `<tbody>` with one `<tr>` per row of `<td>` cells, then `</table>`. HTML escaping
   (`fn escape_html(text: &str) -> String`) escapes `&` → `&amp;`, `<` → `&lt;`, `>` → `&gt;` (never `|`,
   which is not special in HTML). A header-only table omits `<tbody>` entirely.
4. Documents: `markdown_document(title, tables)` = `format!("# {title}\n\n")` then per table
   `format!("## {table.name}\n\n{}\n\n", markdown(table))` (the table's own `name` is the heading, the
   argument `title` is the document title). `html_document(title, tables)` = a complete document:
   `<!DOCTYPE html>\n<html>\n<head><meta charset=\"utf-8\"><title>{escaped title}</title></head>\n<body>\n`
   `h1` for the title, one `h2` per table name, the tables in order, then `</body>\n</html>`. A complete
   HTML document is required because T6 hands it to LibreOffice.
5. Nothing parses: the module is a pure `Table → String` mapping; no file access, no clock.
**Pinned decisions:** D-F (dev-dependency parsing), decision 11 (money formatting only via `money::print`),
decision 23 (dependency lands with the consuming code).
**Edge cases / failure modes:**
- A cell text containing `A|B` must not create a phantom markdown column (assert the parsed cell count).
- A cell text containing `<script>` or `&` must not break the HTML or leak markup.
- A cell containing a newline must not split a markdown row.
- A table whose `columns` is empty (a degenerate value built by hand in the test) must not panic.
- A zero-row table renders header only, in both formats.
**Tests / acceptance criteria** (in-module tests; no scratch dirs needed — pure functions):
1. `markdown_escapes_pipe_in_cell_and_header` — a table with header `["a|b"]` and cell `A|B` renders
   `| a\\|b |`/`A\\|B` and, when parsed with `pulldown-cmark` (with table support enabled — see step 0),
   yields exactly one column and the cell text `A|B`.
2. `html_escapes_angle_and_ampersand` — cell `<b>&x</b>` renders `&lt;b&gt;&amp;x&lt;/b&gt;` and quick-xml
   parses the fragment with the text content decoding back to `<b>&x</b>`.
3. `empty_table_renders_header_only_markdown` — output equals
   `"| category | income | expense | net |\n|---|---|---|---|\n"` for T1's header-only shape.
4. `empty_table_omits_the_body_in_html` — the fragment contains `<thead>` and no `<tbody>`.
5. `money_cells_use_the_domain_format` — `Cell::Money(-123456)` renders `-1.234,56 €` in both formats
   (plan 5.6: "long numbers formatted per the currency format of 1.1").
6. `html_document_is_well_formed_under_quick_xml` — `html_document("Réport <x>", &[t1, t2])`: feed it to
   `quick_xml::reader::Reader` over `Event`s and assert the start/end tags balance (a stack check), and
   that every `<td>` text round-trips.
7. `markdown_document_parses_back_to_the_same_cells` — parse the document with `pulldown-cmark`:
   the heading texts match the table names, and for each table the parsed header/cells equal the input
   (`Cell` values compared after `money::print` for money cells).
8. `documents_have_no_missing_table_for_empty_list` — `markdown_document("t", &[])` == `"# t\n\n"` and
   `html_document("t", &[])` still parses as a document.
9. `count_and_date_cells_render_verbatim` — `Cell::Count(6)` → `"6"`, `Cell::Date("2024-01-24")` →
   `"2024-01-24"`.
10. `carriage_returns_become_spaces_in_markdown` — cell `"a\r\nb"` renders `"a b"` on one line.
**Success check:** `cargo test -p core render` green; `cargo test --workspace` green;
`RUSTFLAGS=-Dwarnings cargo build --workspace --all-targets` has no `unused dependency` warning.
**Review focus:** escaping order in markdown (`\\` before `\|`), HTML escaping never escapes `|`, dev-deps
appear only in `[dev-dependencies]` and only in the same commit as the tests, no dependency in
`[dependencies]`, no new `[features]`, and the supply-chain delta is recorded in the commit message.
**Out of scope:** any file writing, PDF/ODS, command wiring, changing the table content (T2–T4 own it).

### T6 — 5.7 Export delegation to pandoc / LibreOffice (`core/src/technical/export.rs`)

**Depends on:** T5
**Size:** 4 files, ~230 LOC + ~8 tests. One deliverable (delegated conversion) plus the two-line byte-level
atomic writer it requires, which belongs here because 5.7 is the only consumer. Within budget.
**Why:** Plan item 5.7; `architecture.md:387-391` (decision "Delegated PDF and ODS export", status
accepted): "Markdown and HTML are generated natively; PDF and LibreOffice/ODS are produced by calling
`pandoc` / LibreOffice."; `architecture.md:300`: "Missing converters mean missing export formats, which is
reported as a warning, not as an error."; `architecture.md:284`: "converter processes are started for export
only".
**Current state (verified):**
- `pdftotext 26.11.0` and `libreoffice 26.2.7.3` are installed; **`pandoc` is NOT installed** on this
  machine. `libreoffice --convert-to` and `soffice --version` both work.
- `fs_atomic::write_atomic(path, content: &str)` takes text only (`fs_atomic.rs:143`); PDF/ODS are binary,
  so no byte writer exists. `fs_atomic` owns `<path>.phiptmp` (`:52-56`) and `<path>.bak` (`:59-62`) and
  `WriteError { DirectoryRejected, PreviousUnreadable, TempWriteFailed, BackupWriteFailed, RenameFailed }`
  (`:35-50`).
- The external-program precedent is `pdf_text::extract_text_with` (`pdf_text.rs:79`): one `Command` +
  `.output()`, `ErrorKind::NotFound` → `PdfTextError::ToolMissing`. There is no `which` helper.
- **UNVERIFIED (must be run and pinned before committing):** LibreOffice's HTML→ODS import filter string
  (`--infilter=\"HTML (StarCalc)\"`) and its PDF-from-HTML output. Verification step 1 below runs the real
  command line once by hand and the exact working arguments go into the module doc and the commit message.
**Already done / do not redo:** `fs_atomic` (extend, don't fork), `render::*` (T5), the `pdf_text`
detection pattern (copy the pattern, do not import `PdfTextError`).
**Files:**
- Create: `core/src/technical/export.rs`
- Modify: `core/src/technical/mod.rs` (`pub mod export;`), `core/src/technical/fs_atomic.rs`
  (add `write_atomic_bytes` only), `doc/architecture/architecture.md` (amend the wording of the accepted
  "Delegated PDF and ODS export" decision, near `:387-391`)
- Read first: `core/src/technical/fs_atomic.rs`, `core/src/technical/pdf_text.rs`, `render.rs` (T5).

**Interfaces:**
- Consumes: `render::{markdown_document, html_document}` (T5), `fs_atomic::WriteError`.
- Produces:
  - `pub enum Target { Pdf, Ods }` (`Debug, Eq, PartialEq, Copy, Clone`)
  - `pub enum ConvertError { ConverterMissing(String), Failed(String), Io(String) }` (`Debug, Eq, PartialEq`)`
  - `pub const PANDOC_PROGRAM: &str = "pandoc";`
  - `pub const OFFICE_PROGRAM: &str = "libreoffice";`
  - `pub fn convert(target: Target, markdown: &str, html: &str, workspace: &std::path::Path)
     -> Result::<Vec::<u8>, ConvertError>`
  - `pub fn converter_hint(target: Target) -> String` (the human text naming the programs T12 logs)
  - `fs_atomic::pub fn write_atomic_bytes(path: &std::path::Path, content: &[u8])
     -> Result::<BackupOutcome, WriteError>`
**Implementation brief:**
1. `write_atomic_bytes` in `fs_atomic.rs`: same steps as `write_atomic` (`ensure_directory` → read previous
  bytes → `write_temp`-equivalent in bytes → `back_up` → `commit`), sharing the existing helpers; the only
  new part is a byte temp writer. Doc `///` line: same contract as `write_atomic`, for binary content.
2. `convert` writes its intermediates into `workspace` (a directory the caller created, e.g.
  `<parent-of-target>/.phin-export-<process-id>`): `report.md` = `markdown`, `report.html` = `html`.
3. `Target::Ods`: run `libreoffice` with `--headless`,
  `-env:UserInstallation=file://{workspace}/lo-profile`, `--norestore`,
  `--infilter="HTML (StarCalc)"` (UNVERIFIED string — pin the verified one), `--convert-to ods`,
  `--outdir {workspace}`, `{workspace}/report.html`; expect `{workspace}/report.ods`.
4. `Target::Pdf`: try `pandoc {workspace}/report.md -o {workspace}/report.pdf` first; if the spawn yields
  `ErrorKind::NotFound` (pandoc absent — the normal case on this machine), fall back to
  `libreoffice --headless -env:UserInstallation=… --convert-to pdf --outdir {workspace}
  {workspace}/report.html`; expect `{workspace}/report.pdf`. If **both** programs are missing →
  `ConvertError::ConverterMissing(converter_hint(target))`.
5. Detection is a spawn attempt mapped through `ErrorKind::NotFound` → `ConverterMissing` — never a
  `which`/`PATH` search, mirroring `pdf_text`. Any non-zero exit status → `ConvertError::Failed(stderr
  text)`. An I/O error writing intermediates → `Io`. `Command::output()` is used directly (the recorded
  precedent, `pdf_text.rs:79`): converter processes are started for export only (`architecture.md:284`) and
  are bounded by the process itself; a converter timeout is out of scope here (decision 26).
6. Read the produced file's bytes and return them; **never** write the final target here — T12 writes
  bytes through `fs_atomic::write_atomic_bytes`, which is what makes "output written atomically" true.
  Remove the intermediates best-effort (`let _ =`), but only the ones this call created.
7. `architecture.md` amendment (one sentence in the accepted decision, status stays `accepted`): PDF comes
  from `pandoc` when present and from LibreOffice reading the rendered HTML otherwise; ODS comes from
  LibreOffice only; a missing converter is a warning. Quote the amended sentence in the commit message.
**Pinned decisions:** D-E (converter precedence, no config key), decision 19 (temp workspace layout and
`-env:UserInstallation`), decision 18 (byte atomic writer), decision 20 (spawn-attempt detection),
decision 26.
**Edge cases / failure modes:**
- `pandoc` missing **and** LibreOffice present → PDF still succeeds (this is the machine's normal state;
  the test asserts it).
- Both missing → `ConverterMissing`, and **no partial output file** exists at the target path (T12 checks
  this before writing anything).
- LibreOffice refusing because another instance holds a profile → the unique per-workspace profile dir
  avoids the shared `~/.config` lock; if the conversion still fails, the failure text is `Failed`, not a
  hang: `.output()` reaps the child.
- A LibreOffice run can take 5–20 s on first use: tests must not use tight timing assumptions.
**Tests / acceptance criteria** (scratch parent `"test-scratch-export"`; each test creates its own
`<scratch>/<name>/export/` workspace via a private `make_workspace`):
1. `missing_converter_is_typed_and_names_the_program` — inject a bogus program through a
  `convert_with(programs…, )` private-but-test-visible path (document the seam the way `pdf_text`
  documents `extract_text_with`; do not hack `PATH`), asserting
  `ConverterMissing(hint)` for `Target::Ods` and a bogus office program.
2. `ods_conversion_of_a_fixture_table_when_libreoffice_is_present` — run `convert(Target::Ods, …)` on a
  two-table `html_document`; if the real `libreoffice` is unavailable the test asserts
  `ConverterMissing` **instead of skipping** (no `#[ignore]`, no silent return — the absent branch is a
  first-class assertion, and this machine has LibreOffice so the real path is exercised).
  Real-path assertions: the returned bytes are non-empty, start with the ODS/ZIP magic `PK\x03\x04`, and
  `write_atomic_bytes` produces `<target>.bak` on the second write with the first bytes preserved.
3. `pdf_conversion_uses_pandoc_when_present_else_libreoffice` — real run: if `pandoc` exists, assert the
  pandoc branch; otherwise assert the LibreOffice fallback succeeded (this machine's state). PDF bytes
  start with `%PDF-`.
4. `no_partial_file_when_the_converter_is_missing` — a `ConverterMissing` result leaves the would-be target
  path absent (`!Path::new(...).exists()`), because `convert` never writes to the target.
5. `write_atomic_bytes_round_trips_binary_and_keeps_bak` — write `[0u8, 0xFF, 0x00, 0x41]`, then a second
  write of `[1, 2, 3]`: the target bytes are `[1, 2, 3]`, `<path>.bak` bytes are the first content, and the
  `<path>.phiptmp` sibling is gone.
6. `write_atomic_bytes_creates_the_directory` — target below a missing directory is created (same
  behaviour as `write_atomic`).
7. `intermediates_are_named_report_md_and_report_html` — after a successful `convert(Target::Ods, …)` the
  workspace no longer contains `report.html`/`report.ods` (cleanup ran) or, if the conversion failed, they
  may remain — assert only that the failure is typed and the target untouched (document the choice).
8. `converter_hint_names_the_programs` — `converter_hint(Target::Pdf)` contains `pandoc` and `libreoffice`;
  `converter_hint(Target::Ods)` contains `libreoffice`.
**Success check:** `cargo test -p core export` green (the real LibreOffice conversion included), then
`cargo test --workspace` green.
**Review focus:** no write to the final target from `export.rs` (atomicity belongs to the caller's
`write_atomic_bytes`), the exact verified command lines match the module doc, no `which`/`PATH` probing,
no `#[ignore]`/silent skip, no change to `write_atomic`'s existing behaviour.
**Out of scope:** file naming/location (T12), interpreter wiring, `plugins`/`analysis_plugins` config keys,
`pdf_text`, converter config keys, plugin timeouts.

### T7 — Plugin-process plumbing extraction (prepares #3/#4; no behaviour change)

**Depends on:** — (Phase 4 is complete; this task exists because the analysis host (T10) would otherwise
duplicate a plugin-process implementation that has two known defects, #3 and #4.)
**Size:** 3 files, ~180 LOC **moved** (not new logic) + ~10 new lines of module wiring. A move-only task is
worth its own reviewer gate: the question is solely "did the extraction preserve behaviour?".
**Why:** `importer_host.rs` owns the only child-process machinery (`spawn_child` :377, `read_line` :627,
`reap` :654, `spawn_line_reader` :730, `read_plugin_lines` :746, `drain_stderr_bounded` :782). T10 needs the
same pump for `analyse`, and Phase 4's issue table records #3 and #4 as unfixed hand-offs
("none of the three was taken up by 4.8, all three stay open", Phase-4 log:38).
**Current state (verified):** quote-level facts in the Context; the fake-plugin harness runs
`python3 core/tests/fixtures/fake_importer.py <mode> [extra…]` (`fake_plugin_argv` :1573, `find_fixture_dir`
:1557), tests include `fake_plugin_probe_false_never_extracts` :1585,
`fake_plugin_nonzero_exit_names_plugin_others_run` :1632, `fake_plugin_timeout_killed` :1671,
`fake_plugin_transactions_decoded` :1730, `fake_plugin_probe_that_omits_kind_extracts_with_the_resolved_dialect`
:1914, `fake_plugin_annotation_event_is_the_pinned_refusal` :1934,
`reaping_a_child_whose_stdout_a_grandchild_holds_stays_bounded` :1704.
**Already done / do not redo:** the whole bounded-read/kill/reap machinery — **move it, do not rewrite it**.
`ImportError` (incl. `ConverterMissing`, amendment A4), `plugin_child_failure` (:954),
`DEFAULT_PLUGIN_TIMEOUT_SECS` (:85) and the `Importer` trait stay in `importer_host.rs`.
**Files:**
- Create: `core/src/technical/plugin_proc.rs`
- Modify: `core/src/technical/importer_host.rs` (the `ExecImporter`/`PluginChild` region: :320-710 replaced
  with calls into `plugin_proc`; keep `probe`/`extract_transactions`/`plugin_child_failure` in
  `importer_host.rs`), `core/src/technical/mod.rs` (`pub mod plugin_proc;`)
- Read first: `core/src/technical/importer_host.rs` :320-810, `core/tests/fixtures/fake_importer.py`,
  `doc/architecture/plugin-protocol.md`.

**Interfaces:**
- Consumes: nothing new.
- Produces (consumed by T8 and T10):
  - `pub struct ProcessHandle { … }` (private fields: child, stdin, line channel + reader join handle,
    reader-done signal, stderr handle, `reported` string for the importer's `kind` bookkeeping)
  - `pub enum StartError { EmptyArgv, Spawn(String) }` (`Debug, Eq, PartialEq`)
  - `pub fn start(argv: &[String], name: &str, timeout_secs: u64) -> Result::<ProcessHandle, StartError>`
  - `pub fn send(&mut self, line: &str) -> Result::<(), RunError>`
  - `pub enum RunError { Io(String), Closed }` (`Debug, Eq, PartialEq`)
  - `pub enum LineRead { Text(String), Closed, TimedOut, Io(String) }` (`Debug, Eq, PartialEq`)`
  - `pub fn read_line(&mut self, timeout_secs: u64) -> LineRead`
  - `pub fn finish(&mut self) -> Summary`
  - `pub struct Summary { pub stderr_text: String, pub status: Option::<std::process::ExitStatus> }`
  - `pub const REAPER_GRACE_SECS: u64 = 2;`
  - `pub fn take_reported(&mut self) -> String` (the `reported_kind` bookkeeping the importer needs)
**Implementation brief:**
1. Move `spawn_line_reader`, `read_plugin_lines`, `drain_stderr_bounded`, `reap`, `read_line`, the
  `PluginChild` fields and the `Drop` impl into `plugin_proc.rs` as `ProcessHandle`, with the same
  kill→wait→bounded-join→stderr-drain order and the same `REAPER_GRACE_SECS` bounds. Doc each item with
  `///` and a module doc naming the two known defects as fixed-in-the-next-task ("stderr is drained only
  after the child is reaped, and every read gets its own timeout"; issues #3 and #4).
2. `ExecImporter` keeps its `child: RefCell<Option::<ProcessHandle>>` and its `Importer` trait methods
  (`probe`, `extract_transactions`) exactly as they are, now calling `plugin_proc::start`/`send`/`read_line`/
  `finish`. Spawn failure still means `ProbeKind::Unsupported` (the contract forbids a failing probe) and
  the mapping `Summary`→`ImportError` stays in `importer_host.rs::plugin_child_failure`.
3. Delete the moved code from `importer_host.rs`; keep its `LineRead`-shaped matching in
  `extract_transactions` (the same `Timeout`→`timed out after {secs} s`, `Closed`, `IoFailure` arms), so the
  error texts are byte-identical.
4. Any test in `importer_host.rs` that called a moved private helper keeps its **name and assertions** and
  its call site; nothing else may change.
**Pinned decisions:** D-G (issue-fold order), decision 31 (the move-only extraction that the analysis host
consumes, and the reason #3/#4 get fixed before a second host exists).
**Edge cases / failure modes:**
- `argv[0]` missing → `StartError::EmptyArgv` (the importer turns that into `ProbeKind::Unsupported` today).
- A child whose stdout is held open by a grandchild must stay bounded (the existing test
  `reaping_a_child_whose_stdout_a_grandchild_holds_stays_bounded` is the gate).
- `Drop` must still kill+wait (no zombie), and `finish` must be idempotent-safe (`&mut self`, the importer
  never calls it twice — document that).
**Tests / acceptance criteria:**
1. All pre-existing `importer_host` tests pass **unchanged** — the reviewer diff-checks that no assertion in
  `fake_plugin_*`, `bank_a_file_picked_by_bank_a_only`, `exec_importers_reports_a_configured_plugin_that_does_not_exist`,
  `a_config_plugin_list_that_is_malformed_refuses_importers_typed` was edited.
2. `start_with_an_empty_argv_is_typed` — `plugin_proc::start(&[], …)` → `StartError::EmptyArgv`.
3. `start_of_a_missing_program_is_a_typed_spawn_failure` — `start(&["definitely-not-a-plugin-xyz"], …)` →
  `StartError::Spawn(_)` and no panic.
4. `reading_a_closed_child_reports_closed` — run `python3 -c "pass"` (a program that exits immediately):
  `read_line` → `Closed`, then `finish()` returns a `status` with `success() == true` and empty
  `stderr_text`.
**Success check:** `cargo test -p core importer_host` green with unchanged test names/assertions, then
`cargo test --workspace` green.
**Review focus:** this is a move: a reviewer must confirm no logic changed (same bounds, same kill order,
same texts), that `importer_host` still owns the `ImportError` mapping, and that the module doc names the
two defects the next task fixes.
**Out of scope:** any behaviour change, the exec-bit check (T9), the config keys, the analysis host.

### T8 — Fix #3 (pump stderr and stdin concurrently) and #4 (total timeout budget)

**Depends on:** T7
**Size:** 1-2 files, ~120 LOC + 4 tests, two commits (one per issue, per D-G). One deliverable: a plugin
process whose pipes are pumped continuously and whose budget is total, not per read.
**Why:** Phase-4 issue #3 ("stderr drained only after reaping: > 64 KB of stderr mid-run fills the pipe and
the plugin is reported as `timed out`") and #4 ("every read is bounded by `timeout_secs` on its own, so a
plugin emitting one line per just-under-`timeout_secs` never ends"). Phase 5 **blocks** on both: the
`analyse` request carries the whole dataset (T10), so a large stdin write to a plugin that is busy writing
a large table to stdout deadlocks in both directions, and an analysis plugin that answers a large table
plus diagnostics on stderr would be misreported as a timeout.
**Current state (verified):** after T7, `plugin_proc::ProcessHandle` holds the reader thread
(`spawn_line_reader`/`read_plugin_lines`), the stderr handle drained only inside `finish()`
(`drain_stderr_bounded`), and `read_line(timeout_secs)` per-read windows.
`DEFAULT_PLUGIN_TIMEOUT_SECS: u64 = 60` (`importer_host.rs:85`).
**Already done / do not redo:** T7's extraction; the timing conversions of Phase-3 decision 39
(`i64::try_from(d.as_millis())`, no `as` casts; `Duration::milliseconds` does not exist in this toolchain).
**Files:** Modify `core/src/technical/plugin_proc.rs` (the pump and the deadline), `core/src/technical/importer_host.rs`
(caller of `read_line`, plus new tests).
**Interfaces:**
- Consumes: T7's `ProcessHandle`.
- Produces (T10 consumes these):
  - `pub fn read_line(&mut self) -> LineRead` (signature change: the window now comes from the handle's
    deadline; `LineRead::TimedOut` still names nothing — the caller formats the text)
  - `pub fn remaining(&self) -> std::time::Duration` (zero when expired)
  - `pub fn stderr_text(&self) -> String` (available **before** `finish`, so a caller can log a live plugin)
  - `pub const STDERR_CAP_BYTES: usize = 8 * 1024;`
  - unchanged: `send`, `finish`, `Summary`, `REAPER_GRACE_SECS`, `StartError`, `RunError`, `LineRead`
**Implementation brief:**
1. **#3 (commit message: `plugin: pump stderr and stdin continuously (fixes #3)`)**
   a. Start a stderr reader thread in `start` that reads lines into a bounded buffer: keep at most
     `STDERR_CAP_BYTES` bytes, appending and dropping from the front when the cap is reached, and record
     `format!("\u{2026} {} bytes of stderr dropped", n)` when bytes were dropped. Never block the child.
   b. `send` writes through a **writer-side discipline that cannot deadlock**: write the line, and on a
     partial/`BrokenPipe` write, drain stderr and the already-read stdout lines while retrying
     (`WouldBlock` is not available for a piped stdin here, so pin the mechanism: `stdin` is written from a
     dedicated writer thread fed by a `std::sync::mpsc::sync_channel(1)`; `send` hands the line to that
     thread and reports `RunError::Io` when the thread dies or the pipe closes). Document why: the
     analysis request of plan 5.5 is a single line larger than the pipe buffer.
   c. `finish` joins the stderr thread with `REAPER_GRACE_SECS` and takes the buffered text (no
     `read_to_string` after reaping any more); delete `drain_stderr_bounded`.
   d. `stderr_text()` returns the current buffer (used by T10's log line).
2. **#4 (commit message: `plugin: one total timeout budget (fixes #4)`)**
   a. `start` records `deadline = Instant::now() + Duration::from_secs(timeout_secs)`.
   b. `read_line()` computes the remaining window and does `recv_timeout(remaining)`; a remaining window of
     zero → `LineRead::TimedOut` immediately. Never create a zero-length deadline.
   c. `importer_host::extract_transactions` and `probe` keep their texts; the timeout text stays
     `"plugin {name} timed out after {secs} s"` (the pinned fake-plugin test asserts it — keep the number
     the configured `timeout_secs`, not the remaining time).
   d. Convert durations the way Phase-3 decision 39 pinned (an `Instant` difference converted through
     `i64::try_from(d.as_millis())`, no `as` casts); `Duration::milliseconds` does not exist in this
     toolchain.
3. Update the module doc: "the whole plugin conversation is bounded by one window, and neither pipe can
   fill up while the other is busy".
**Pinned decisions:** D-G (issue-numbered commits), Phase-3 decision 39 (the external precedent for
duration conversions without `as` casts), decision 31 (this fix-prep split of the plugin plumbing).
**Edge cases / failure modes:**
- A plugin writing 200 KB on stderr then answering normally must **not** be reported as timed out.
- A plugin that emits one line every ~0.9 s with a 2 s budget must end with `TimedOut` — not run forever.
- A plugin that closes stderr but keeps stdout open: `stderr_text()` stays empty, `finish` still bounded.
- A child writing to stdin after exiting: `send` → `RunError::Io("closed")`, never a panic.
**Tests / acceptance criteria** (extend the fake-plugin harness with new modes in
`core/tests/fixtures/fake_importer.py`; the harness runs `python3`, so no exec bit is needed):
1. `chatty_stderr_does_not_time_out` (in `importer_host`) — fake mode `chatty-stderr` writes ~200 KB of
   stderr then a valid `extract` answer: `extract_transactions` returns `Ok` and its transactions, and the
   failure text is not the timeout text.
2. `dropped_stderr_is_named` — mode `huge-stderr` writes 400 KB of stderr then exits non-zero: the
   `ImportError::Failed` text contains the marker `bytes of stderr dropped` and the plugin name.
3. `total_budget_ends_a_slow_chatter` — mode `slow-lines` prints one valid line every 0.9 s and a `end`
   terminator only after 10 s; `timeout_secs = 2` → `Err(ImportError::Failed(_))` containing
   `timed out after 2 s`, and the elapsed time is bounded (assert the total wall time is under 6 s with a
   generous margin, no wall-clock assertion tighter than that — the Phase-3d reviewer's lesson).
4. `stdin_write_of_a_large_request_does_not_deadlock` — `send` of a ~1 MB line to a fake plugin that never
   reads stdin: `send` returns `RunError::Io`/`Closed` (not a hang), and `finish` reaps the child.
5. `existing_importer_host_tests_stay_green` — the fake-plugin suite from T7 unchanged.
**Success check:** `cargo test -p core plugin_proc` and `cargo test -p core importer_host` green, then
`cargo test --workspace` green.
**Review focus:** no unbounded `join`/`read_to_string` remains; the stderr buffer cap is enforced; the
timeout is total (a reviewer must check the per-read `recv_timeout` call sites are gone); both commits name
their issue numbers.
**Out of scope:** analysis plugins, config keys, exec-bit checks (T9), plugin-protocol changes, per-plugin
budget configuration.

### T9 — Fix #2: the configured plugin executable bit is checked

**Depends on:** T7 (same files, avoids a conflict)
**Size:** 2 files, ~70 LOC + 4 tests. Small and independent of the pump work — hence a separate task and a
separate commit naming #2.
**Why:** Phase-4 issue #2: "`exec_importers checks existence only; a non-executable `plugins` entry is
invisible because a probe cannot fail by contract" (decision I4 deferred it and "it never landed"). T10
introduces `analysis_plugins` with the same discovery code, so the check must exist once, for both kinds.
**Current state (verified):** `exec_importers` (`importer_host.rs:995`) calls `plugin_program_path` (:1042,
relative paths resolve against the data dir — `data-files.md:155-161`) and `fs::metadata` for existence →
`ConfigIssue::PluginNotFound(configured)`; `scratch_plugin_file` (:1822) deliberately writes
`"#!/bin/sh\nexit 0\n"` **without** the exec bit ("`Existing-but-unrunnable executables (`scratch_plugin_file`
sets NO" …). `std::os::unix::fs::PermissionsExt` appears nowhere in the repo. `ConfigIssue` lives at
`transport_cfg.rs:233`.
**Already done / do not redo:** `PluginNotFound`, `plugins_from_config`'s typed complaints, the path rule.
**Files:** Modify `core/src/technical/transport_cfg.rs` (new `ConfigIssue` variant + one shared check),
`core/src/technical/importer_host.rs` (`exec_importers` calls it).
**Interfaces:**
- Produces: `ConfigIssue::PluginNotExecutable(String)` (new variant, payload = the configured string as
  given), `pub fn plugin_program_issue(configured: &str, data_dir: &std::path::Path) -> Option::<ConfigIssue>`
  in `transport_cfg` (`None` = fine to start), and `pub fn plugin_program_path(configured: &str,
  data_dir: &std::path::Path) -> String` moved from `importer_host.rs:1042` (so both kinds share one path rule).
  Consumed by T9 itself and T10.
**Implementation brief:**
1. Move `plugin_program_path` to `transport_cfg.rs` (behaviour identical: absolute kept, relative joined).
2. `plugin_program_issue`: metadata error → `Some(PluginNotFound(configured))`; on `Ok`, on `#[cfg(unix)]`
  read `metadata.permissions().mode() & 0o111 == 0` → `Some(PluginNotExecutable(configured))`; on
  `#[cfg(not(unix))]` skip the bit check (existence only, documented). Then `None`.
3. `exec_importers` calls it instead of the inline `fs::metadata` check; the complaint type for a missing
  file is unchanged (`PluginNotFound`) so existing tests keep their assertions.
4. Add `PluginNotExecutable(String)` to `ConfigIssue` and to its `Debug`-visible complaints path; the
  invariant "a key present with the wrong shape is a typed complaint, never a silent default" holds.
**Pinned decisions:** D-G (its own commit: `plugin: check the executable bit of a configured plugin (fixes #2)`),
decision 29 (unix-only bit check, non-unix path documented).
**Edge cases / failure modes:**
- A directory named in `plugins` → the metadata succeeds but the bit test passes on directories; pin the
  extra condition: `!metadata.is_file()` → `PluginNotExecutable` too? No: pin exactly — a non-file is
  reported as `PluginNotFound(configured)` (a directory is not an executable program). State it in code doc.
- A file with mode `0o644` → `PluginNotExecutable`; a file with `0o755` → `None`.
- `ConfigIssue` variants are compared with `==` in tests, so the derive (`Debug, Eq, PartialEq`) must be
  kept intact.
**Tests / acceptance criteria** (scratch parent stays `test-scratch-transport-cfg` / the importer's own;
`#[cfg(unix)]` on the bit tests):
1. `configured_plugin_without_exec_bit_is_named` (importer_host) — `plugins = ["./imp"]` with a
  `0o644` file → `exec_importers` returns `importers.is_empty()` and
  `issues == [ConfigIssue::PluginNotExecutable(\"./imp\".to_string())]`.
2. `missing_configured_plugin_still_plugin_not_found` — a path that does not exist → `PluginNotFound`
  (the existing test `exec_importers_reports_a_configured_plugin_that_does_not_exist` must stay green).
3. `executable_plugin_passes_the_check` — a `0o755` file → `issues.is_empty()` and
  `importers.len() == 1`.
4. `plugin_program_issue_reports_a_directory_as_missing` — `plugins = ["./subdir"]` where `subdir` is a
  directory → `PluginNotFound`.
5. `plugin_program_path_rule_is_shared` — `plugin_program_issue("./x", data_dir)` resolves the same as the
  old `importer_host::plugin_program_path` (relative → data dir; absolute → as given).
**Success check:** `cargo test -p core transport_cfg` and `cargo test -p core exec_importers` green, then
`cargo test --workspace` green.
**Review focus:** no silent drop of a non-executable entry (the whole point of #2); `PluginNotFound`
behaviour unchanged; the unix-only gate is `#[cfg(unix)]` and the non-unix path is documented, not skipped
silently.
**Out of scope:** plugin timeouts/pipe fixes (T8), analysis plugins (T10), running the configured program
in any new way.

### T10 — 5.5 Analysis plugin host + `analysis_plugins` key (`core/src/technical/analysis_host.rs`)

**Depends on:** T1, T2, T8, T9
**Size:** 6 files, ~260 LOC + ~9 tests + 1 fixture script. Above the LOC warning, with a written reason:
the host, its config key, its wire codec and the protocol amendment are one contract with one gate
(the parity test of plan 5.5 is the success check); splitting the codec out would create a task whose only
review question is "does this JSON match?". Per ground rule 2 the split signals (two independent success
checks, >5 files) are answered by keeping the config-key read (12 lines) with the host and the doc
amendment with the contract.
**Why:** Plan item 5.5; `architecture.md:175`: "Analysis: `analyse {transactions, categories} -> table`";
`architecture.md:421`: "adding a new analysis must mean adding one analysis plugin".
**Current state (verified):**
- Contract (`plugin-protocol.md:112-132`): the core sends the full dataset in one line,
  `{"v":1,"cmd":"analyse","name":"biggest-spendings","transactions":[{…,"category":"food:supermarket"}],
  "categories":[{"parent":"","name":"food"},{"parent":"food","name":"supermarket"}],"year":2024}`
  (example id `plugin-analyse-request`), and the plugin answers
  `{"v":1,"event":"table","name":"biggest spendings 2024","columns":["date","description","amount"],"rows":[["2024-01-03","REWE MARKT 12",-3999]]}`
  + `{"v":1,"event":"end"}` (example id `plugin-analyse-response`). **There is no `probe` step for analyses**
  (`plugin-protocol.md:18` names `probe` for importers only), and one process per plugin per run
  (`:15`). A plugin error names the plugin (`:134-144`, example `plugin-error`).
- `columns` are plain strings and `rows` untyped arrays, so cell types are unrepresentable today; plan 5.1
  requires typed cells. D-D amends the doc.
- `data-files.md:155-161` fixes `plugins` as "one entry per plugin importer of `plugin-protocol.md`" and
  the relative-path rule; nothing names analysis plugins. D-C adds `analysis_plugins`.
- `Transport` of T7/T8: `plugin_proc::start/send/read_line/finish/stderr_text/remaining` (total budget,
  continuous pumps) — the analysis request is one line larger than a pipe buffer, which is why T8 lands first.
**Already done / do not redo:** `plugin_proc` (T7/T8), `transport_cfg::plugin_program_issue` /
`plugin_program_path` (T9), `analyse_stats::biggest_spendings` (T4) for the parity test, `result` (T1).
**Files:**
- Create: `core/src/technical/analysis_host.rs`, `core/tests/fixtures/fake_analysis.py`
- Modify: `core/src/technical/mod.rs` (`pub mod analysis_host;`), `core/src/technical/transport_cfg.rs`
  (new key), `doc/architecture/plugin-protocol.md`, `doc/architecture/data-files.md`
- Read first: `core/src/technical/importer_host.rs` (the host patterns and `plugin_child_failure` text style),
  `doc/architecture/plugin-protocol.md`, `core/tests/fixtures/fake_importer.py`.

**Interfaces:**
- Consumes: `plugin_proc::{start, ProcessHandle, LineRead, RunError, REAPER_GRACE_SECS}`,
  `transport_cfg::{plugin_program_issue, plugin_program_path, ConfigIssue}`,
  `view::Entry`, `category::Tree`, `analyse_categories::{NAME, belongs_to_year}`,
  `analyse_stats::{NAME_MONTHLY, NAME_BIGGEST}`, `result::{Table, Cell, Column, Kind, table, add_row}`.
- Produces:
  - `pub const DEFAULT_ANALYSIS_TIMEOUT_SECS: u64 = 60;`
  - `pub struct AnalysisPlugin { pub argv: Vec::<String>, pub name: String, pub timeout_secs: u64 }`
  - `pub fn analysis_plugins_from_config(data_dir: &std::path::Path)
     -> (Vec::<AnalysisPlugin>, Vec::<ConfigIssue>)` (wraps
     `transport_cfg::analysis_plugins_from_config` (below) and turns each path into
     `AnalysisPlugin { argv: vec![plugin_program_path(path, data_dir)], name: path.clone(),
     timeout_secs: DEFAULT_ANALYSIS_TIMEOUT_SECS }`)
  - `pub fn json_of_table(table: &result::Table) -> serde_json::Value`
  - `pub fn table_of_json(value: &serde_json::Value) -> Result::<result::Table, String>`
  - `pub fn analyse_request_line(name: &str, entries: &[view::Entry], tree: &category::Tree,
     year: Option::<i64>) -> String`
  - `pub fn run_plugin_analyses(plugins: &[AnalysisPlugin], names: &[String], entries: &[view::Entry],
     tree: &category::Tree, year: Option::<i64>) -> (Vec::<result::Table>, Vec::<String>)`
  - `transport_cfg::pub const ANALYSIS_PLUGINS_KEY: &str = "analysis_plugins";`
  - `transport_cfg::pub fn analysis_plugins_from_config(data_dir: &std::path::Path)
     -> (Vec::<String>, Vec::<ConfigIssue>)` (reads the key, mirroring `plugins_from_config` exactly:
     missing file → no key and no complaints; non-array → `PluginsNotStrings` with the key name in the text;
     non-string entry → `PluginsNotStrings("analysis_plugins entry {index} is not a string")`)
**Implementation brief:**
1. `analyse_request_line`: build with `serde_json` in this key order: `v`, `cmd`, `name`, `transactions`,
   `categories`, `year`. Transaction objects carry `date, amount, currency, description, counterparty,
   source, category` (the doc example's field set; `category` is the derived classification, and a
   `Classification::Unknown` transaction carries `category` as the string `unknown`). `categories` is
   `[{"parent": "", "name": "food"}, …]` in `categories.csv` file order — build it from the raw text with
   `store_sidefiles::read_categories` (its `Category` rows have private fields, so re-read the columns
   directly with `csv::records_of` if needed; do not add a public accessor to `Category`). `year` is
   `null` when absent (the doc example shows the number form; absent means the whole dataset).
   Amounts are **integer minor units** (`plugin-protocol.md:39`), so `from_stored`-style values go out
   untouched — never re-scale.
2. `run_plugin_analyses`: for each plugin, start one process (one process per plugin per run) and, for each
   requested name, `send(analyse_request_line(...))` then read `table` then expect `end`:
   - a `table` event → `table_of_json` → push the table;
   - an `error` event → push a warning `format!("analysis plugin {name} reported: {text}")` and stop
     reading that plugin (reap, continue with the next plugin);
   - `LineRead::TimedOut` → kill/reap and warning
     `format!("plugin {name} timed out after {secs} s")`;
   - `Closed`/`Io`/a non-zero exit → `plugin_child_failure`-style warning naming the plugin and the stderr
     text (`plugin_proc::stderr_text`);
   - an unparseable line → `format!("invalid line from {name}")`;
   - a plugin that answers `end` with no table → no table, no warning (a plugin may decline silently).
   **A plugin failure is never fatal: the built-ins and the other plugins still run** (plan 5.5).
3. `table_of_json`: `name` must be a string; `columns` an array of strings; `column_types` optional
   (array of strings, same length as `columns`, `kind_of_name` per entry); when `column_types` is absent,
   infer by exact column name: `amount|income|expense|net|balance` → `Kind::Money`,
   `count|transactions` → `Kind::Count`, `date` → `Kind::Date`, anything else → `Kind::Text` (D-D).
   `rows` is an array of arrays: a `Money`/`Count` cell takes a JSON number (integer; a non-integer number
   is the typed error `format!("column {name} row {n} is not an integer")`), a `Date`/`Text` cell takes a
   JSON string; a wrong JSON type is a typed error naming the column. A wrong length is
   `format!("row {n} has {k} cells, {m} expected")`.
   `json_of_table` is the exact inverse (money/count → numbers, date/text → strings) and also emits
   `column_types` always, so the round trip is loss-free.
4. `doc/architecture/plugin-protocol.md` edits (the same task, ground rule 6):
   - in the `plugin-analyse-response` example add `"column_types":["date","text","money"]` after `columns`;
   - replace the trailing "Draft, not yet accepted" sentence about the table shape with: the shape is fixed,
     `column_types` is optional and one entry per column, its values are `money`, `count`, `date`, `text`,
     and when it is absent the core infers the type from the column name (the exact inference table above);
   - one sentence in the discovery bullet: importers come from the `plugins` key, analyses from the
     `analysis_plugins` key (see `data-files.md`).
5. `doc/architecture/data-files.md` Configuration section: extend the Keys sentence with
   `analysis_plugins` (an array of executable path strings, one entry per analysis plugin; relative paths
   resolve against the data directory, absolute paths kept as given — the same rule as `plugins`).
**Pinned decisions:** D-C, D-D, D-G (this task follows the issue fixes), decision 22 (no probe step:
every name is sent to every plugin), decision 30 (built-in and plugin tables are both included; the host
never deduplicates).
**Edge cases / failure modes:**
- A plugin that answers `probe` (an importer verb) → treat it as an invalid response for this contract:
  warning `format!("{name} is not an analysis plugin: it answered probe")` — pin the refusal the way
  Phase 4 pinned `ANNOTATION_REFUSAL`.
- `column_types` length mismatch vs `columns` → typed error (a plugin that lies about its table is
  reported, never guessed around).
- An empty `analysis_plugins` list or a missing key → `(empty, no complaints)` and no process started.
- A key that is not an array → complaints, and **no plugins handed out from a malformed list** (matches
  the pinned `plugins_from_config` behaviour).
**Tests / acceptance criteria** (scratch parent `"test-scratch-analysis-host"`; the fake plugin runs
`python3 core/tests/fixtures/fake_analysis.py <mode> [extra…]`, mirroring `fake_plugin_argv`):
1. `plugin_table_equals_the_builtin_table` (plan 5.5's parity gate) — the T4 fixture store;
   built-in: `analyse_stats::biggest_spendings(&entries, Some(2024), 3)`; plugin: fake mode
   `biggest-spendings` that re-emits the same three rows it was given in the request; assert
   `plugin_tables[0] == built_in_table` (full `Table` equality, so `column_types`/`Cell` kinds match too).
2. `plugin_error_is_a_warning_and_builtins_still_run` — fake mode `error-after-request`: the returned
   warnings name the plugin and the text it reported, `tables.is_empty()` for that plugin, and a second
   plugin (mode `biggest-spendings`) still returns its table.
3. `crashing_plugin_is_named_and_its_stderr_shown` — fake mode `exit-after-request` (non-zero exit with
   stderr): the warning contains the plugin name and `the plugin wrote on stderr:` (the Phase-4 text style).
4. `slow_plugin_hits_the_total_budget` — fake mode `no-answer` with `timeout_secs = 1`: warning contains
   `timed out after 1 s` and the whole `run_plugin_analyses` call is bounded.
5. `request_line_carries_the_documented_shape` — read `doc/architecture/plugin-protocol.md` in the test
   (two path candidates: `doc/…` from the workspace root and `doc/…` with `../` from `core`, the same
   fallback `proto`'s reader uses), take the block after `<!-- test-example id=plugin-analyse-request -->`,
   and assert `analyse_request_line("biggest-spendings", entries, tree, Some(2024))` parsed as JSON has
   the same set of top-level keys and the same transaction keys as the doc example — this is what keeps
   the amended doc true. The marker reader is a small private `#[cfg(test)]` helper in this module (the
   first core-side doc reader; the Phase-3 consolidation rule concerns the core-protocol reader inside
   proto, and this one reads a different file).
6. `column_types_absent_is_inferred_by_name` — a fake plugin answering without `column_types` and columns
   `["date", "description", "amount"]` yields kinds `Date, Text, Money`.
7. `column_types_wrong_length_is_typed` — `table_of_json` on a 3-column table with 2 `column_types`
   → `Err(_)` naming the mismatch; `rows` with a string under a `money` column → `Err(_)`.
8. `analysis_plugins_key_is_read_and_checked_like_plugins` — `phin.toml` with
   `analysis_plugins = ["./an", 3]` → `PluginsNotStrings` and no plugin; with a `0o644` file →
   `PluginNotExecutable`; with a `0o755` file → `argv.len() == 1` and `name` equals the configured string.
9. `json_of_table_and_table_of_json_round_trip` — build a table with all four `Kind`s, encode, decode, and
   assert equality (money stays `i64`, no float appears in the JSON text: the encoded text contains no `.`
   in a money position).
**Success check:** `cargo test -p core analysis_host` green (the parity test is the centrepiece), then
`cargo test --workspace` green.
**Review focus:** amounts cross the wire as integer minor units in both directions (a `f64` or a re-scaled
value is a blocker); a plugin failure never aborts the run; the amended `plugin-protocol.md` matches the
host's real JSON (test 5 pins it); no `probe` step invented for analyses; the config key's name and
relative-path rule matches `data-files.md`.
**Out of scope:** the `analyze`/`report` commands (T11/T12), rendering, export, example plugin projects
(plan 7.3), changing `importer_host`'s importer contract.

### T11 — 5.8a `analyze` vocabulary: doc row, command table, typed view (`analyze` command)

**Depends on:** — (runs any time; land it before T12)
**Size:** 4 files, ~90 LOC + 2 test updates. Small, one deliverable: the vocabulary of the new verb.
**Why:** Plan item 5.8 asks for an `analyze` **and** a `report` command, but only `report` is documented
(`core-protocol.md:96-105`) and `Command::Report(format, Option<i64>)` exists. D-A closes the gap: an
`analyze` verb is what makes "adding a new analysis means one plugin" (Q3) and plan 5.8's "unknown analysis
name → error event" testable through the protocol.
**Current state (verified):**
- `proto/src/commands.rs:182 command_rows()` holds the rows in the doc's order (module doc at `:173`),
  with helpers `row(name, parameters, either)` (`:189`) and `parameter(name, kind, required)` (`:193`);
  the `report` row is
  `row("report", [parameter("format", ValueKind::Text, true), parameter("year", ValueKind::Integer, false)].to_vec(), None)`.
- The typed view relates to a row by POSITION: `command_parameters` (private, `command.rs:146-162`) and
  `read_typed_command` (private, `:196-215`) both match on `position_of(..)` 0..7 (Version 0, Import 1,
  Report 2, SetCategory 3, ConfirmAnnotation 4, ListPending 5, Cancel 6, Shutdown 7).
- Pinned doc-sweep tests: `codec::every_test_example_of_the_core_protocol_doc_decodes_and_round_trips`,
  `commands::every_documented_command_example_carries_the_parameters_of_the_row_it_names`, and
  `appending_a_row_to_the_command_table_validates_the_new_command_with_no_other_change` (`:639`, which
  asserts `documented_rows.len() == 8`).
- `WiredNowhere` refuses `report` with `format!("no analysis or renderer is wired for {format} yet: plan
  items 5.1-5.8 connect them to the interpreter")` (`interpret.rs:663-701`), and
  `every_command_of_the_table_is_refused_with_a_documented_code_naming_its_phase` (`interpret.rs:1095`)
  asserts the code list `["plugin-failed", "plugin-failed", "plugin-failed", "io", "io", "io", "io"]`.
**Already done / do not redo:** the `report` row and `Command::Report` (do not rename, do not re-type);
`ValueKind::Integer` and `optional_number`/`optional_int_value` are already there for the optional year.
**Files:** Modify `doc/architecture/core-protocol.md` (command table + two examples + one sentence),
`proto/src/commands.rs`, `proto/src/command.rs`, `core/src/protocol/interpret.rs` (`WiredNowhere` + its
pinned test).
**Interfaces:**
- Produces: `proto::command::Command::Analyze(String, Option::<i64>)` (parameters: `name` required Text,
  `year` optional Integer) at table position 3, and the doc row. Consumed by T12.
**Implementation brief:**
1. In `core-protocol.md` insert the row **immediately after the `report` row** (analysis and documents are
   grouped in the doc, and the row order is the table order the code mirrors):
   `| \`analyze\` | \`name\`, optional \`year\` | Run one analysis by name and return its table(s). |`
2. After the "Result of a report" example, add a sentence and two examples:
   An unknown analysis name is answered with `code":"unknown-command"` and a text naming the name and the
   known analyses.
   `<!-- test-example id=core-command-analyze -->`
   ```json
   {"v":1,"cmd":"analyze","name":"biggest-spendings","year":2024}
   ```
   `<!-- test-example id=core-event-result-analyze -->`
   ```json
   {"v":1,"event":"result","cmd":"analyze","name":"biggest-spendings","year":2024,"tables":[{"name":"biggest spendings 2024","columns":["date","description","amount"],"column_types":["date","text","money"],"rows":[["2024-01-24","MEGA MART 04",-123456]]}]}
   ```
3. In `commands.rs` add the row after `report`:
   `row("analyze", [parameter("name", ValueKind::Text, true), parameter("year", ValueKind::Integer, false)].to_vec(), None),`
4. In `command.rs`: add the enum variant `Analyze(String, Option::<i64>)` after `Report`, and renumber:
   `command_parameters` and `read_typed_command` gain `Analyze(name, year) => (3, [text, optional_number])`
   and `3 => Analyze(text_value(…, 0)?, optional_int_value(…, 1)?)`, shifting `SetCategory → 4`,
   `ConfirmAnnotation → 5`, `ListPending → 6`, `Cancel → 7`, `Shutdown → 8` in **both** functions.
   Update the module doc sentence that explains the position relation.
5. In `interpret.rs`: add the `Command::Analyze(name, year)` arm to `WiredNowhere` with
   `Answered::AnswerWith(ErrorReason::PluginFailed, format!("no analysis is wired for {name} yet: plan items 5.1-5.8 connect them to the interpreter"))`
   (T13 rewords both refusal texts when the phase is wired).
6. Update `every_command_of_the_table_is_refused_with_a_documented_code_naming_its_phase` to expect four
   `plugin-failed` codes and the new verb in the blame list; update the count assertions from 8 to 9
   (the doc sweep tests need no change: they iterate the doc — that is the point of D-A landing the doc row).
**Pinned decisions:** D-A, D-H (reuse `unknown-command`, no new code), decision 9 (insert after `report`,
renumber positions), decision 28 (payload key set `name`, `year`, `tables` — pinned by the doc example).
**Edge cases / failure modes:**
- A `core-command-analyze` example whose parameters do not match the row must fail the sweep test
   (that is the gate; don't special-case it).
- Position renumbering must not break `command_name`/`command_fields` (they derive from
   `command_parameters`, so they follow automatically).
- `{`/`}` in a name or a non-integer `year` stay the existing `CodecError` shapes
   (`WrongType("year")`), no new error type.
**Tests / acceptance criteria:**
1. `every_documented_command_example_carries_the_parameters_of_the_row_it_names` — green with the new row
   and example (it sweeps `core-command-*` ids automatically).
2. `every_test_example_of_the_core_protocol_doc_decodes_and_round_trips` — green; the new
   `core-event-result-analyze` payload round-trips through `Event::Result`'s free map.
3. `analyze_round_trips_through_the_typed_view` (new) —
   `decode_line("{\"v\":1,\"cmd\":\"analyze\",\"name\":\"cashflow\",\"year\":2024}")` →
   `Command::Analyze("cashflow", Some(2024))`; `encode_frame` gives the same field set back; the form
   without `year` → `Command::Analyze("cashflow", None)` and re-encodes without the key.
4. `the_documented_command_rows_are_nine_now` — the count assertion updated: `command_rows().len() == 9`,
   `command_names()` contains `analyze` at index 3 and `set-category` at 4.
5. `analyze_is_refused_until_its_phase_is_wired` — the updated `WiredNowhere` test asserts the
   `plugin-failed` code and the text for `analyze`.
**Success check:** `cargo test -p proto` green and `cargo test -p core interpret` green, then
`cargo test --workspace` green.
**Review focus:** the doc row and the code row are the same row (name, order, parameter kinds); positions
renumbered in **both** `command_parameters` and `read_typed_command`; the doc examples use the pinned
payload keys; no new error code was invented (the draft five stay).
**Out of scope:** any implementation of the analysis itself, `report` changes, renderer/host changes, the
`analysis_plugins`/`column_types` doc text (T10).

### T12 — 5.8b `analyze`/`report` wired into the interpreter (`AnalysisWork`) + report files

**Depends on:** T3, T4, T5, T6, T10, T11
**Size:** 5 files, ~250 LOC + ~10 tests. One deliverable (the two commands wired end-to-end through the
document writer); the two doc lines it forces are part of the same gate. Within budget.
**Why:** Plan item 5.8; the Definition of Done needs requirement 005 demonstrable through the protocol on
fixtures; `interpret.rs` currently refuses `report` with "plan items 5.1-5.8" and has no `Analyze` arm
beyond T11's stub.
**Current state (verified):**
- `interpret::Work` seam: `pub trait Work { fn do_command(&mut self, command: Command,
  sink: &mut dyn EventSink) -> Answered; }` (`interpret.rs:202`);
  `pub enum Answered { EndedWith(…), AnswerWith(ErrorReason, String) }` (`:178`).
- The working composition pattern (`import_work.rs:123-161`): a struct holding `data_dir` + a fallback of
  `WiredNowhere`, one arm per owned verb, everything else delegated; amendment A6 pinned "do NOT widen
  `Work::do_command`" and "ImportWork creates its own `writer::start("import", total)`, drives
  `progress`/`log`, checks `writer::cancel_requested(&stream)` between files, and ends with
  `writer::succeed` (payload keys `added`/`annotations`/`linked`) or `writer::cancel`".
- `core/src/main.rs`: `:38 use core::protocol::import_work::ImportWork;`, daemon branch `:279-280`
  (`ImportWork::new(settings.data_dir.clone())` handed to `transport::serve_daemon(..., &mut import_work,
  handle.session)`), pipe branch `:355-356` (`interpret::run(&mut input, &mut sink, &mut import_work)`
  with `pipe_data_dir(&args)` at `:318`).
- `WiredNowhere` is reached only when nothing is wired; its texts and pinned tests stay (T13 rewords them).
- Report path/naming: nothing is documented anywhere except `core-protocol.md:170`
  (`"path":"report/2024.html"`); `architecture.md:296` lists the data directory contents without a
  `report/` entry. D-B fixes both.
- `doc/implementation/plan.md:274-276` (5.8) expects a scripted `analyze` + `report markdown` sequence and
  "unknown analysis name → error event".
**Already done / do not redo:** `ImportWork` (do not touch it — `AnalysisWork` wraps it), the writer
primitives, the `analyze` vocabulary (T11), the analyses (T2-T4), the renderer (T5), `export::convert` and
`fs_atomic::write_atomic_bytes` (T6), `run_plugin_analyses` (T10).
**Files:**
- Create: `core/src/protocol/analysis_work.rs`
- Modify: `core/src/protocol/mod.rs` (`pub mod analysis_work;`), `core/src/main.rs` (work chain),
  `doc/architecture/data-files.md` (new "Generated documents" entry), `doc/architecture/architecture.md`
  (the data-directory list near `:296` gains `report/`)
- Read first: `core/src/protocol/import_work.rs` (the composition and event style to copy),
  `core/src/protocol/writer.rs`, `core/src/protocol/interpret.rs`, `core/tests/import_lifecycle.rs`.

**Interfaces:**
- Consumes: `proto::command::{Command, command_name}`, `proto::event::ErrorReason`,
  `protocol::{interpret, writer}`, `technical::view::read_view`,
  `technical::analyse_categories::{NAME, category_tree_at, income_expense_by_category, unknown_category_paths}`,
  `technical::analyse_cashflow::{NAME as CASHFLOW_NAME, cash_flow}`,
  `technical::analyse_stats::{NAME_MONTHLY, NAME_BIGGEST, TOP_DEFAULT, monthly_stats, biggest_spendings}`,
  `technical::render::{markdown, html, markdown_document, html_document}`,
  `technical::export::{Target, ConvertError, convert}`,
  `technical::analysis_host::{analysis_plugins_from_config, run_plugin_analyses, json_of_table}`,
  `technical::fs_atomic::{write_atomic, write_atomic_bytes}`, `protocol::import_work::ImportWork`.
- Produces:
  - `pub struct AnalysisWork { … }` with `pub fn new(data_dir: std::path::PathBuf) -> AnalysisWork`
    (fields private: `data_dir`, `fallback: ImportWork`, `program_overrides: Option<(String, String)>` —
    the documented test seam for `export`'s programs, `None` in production)
  - `impl interpret::Work for AnalysisWork` with arms for `Command::Analyze` and `Command::Report`,
    `other => self.fallback.do_command(other, sink)`
  - `pub const FORMAT_MARKDOWN: &str = "markdown";` / `FORMAT_HTML: &str = "html"` /
    `FORMAT_PDF: &str = "pdf"` / `FORMAT_ODS: &str = "ods";`
  - `pub const BUILTIN_ANALYSES: [&str; 4]` = `analyse_categories::NAME`, cashflow, monthly-stats,
    biggest-spendings (order = report order)
  - `pub fn report_path(format: &str, year: Option::<i64>) -> Option::<(String, String)>` —
    `(relative path, extension)`: `report/{year}.{ext}` or `report/all.{ext}`; `None` for an unknown format
  - `pub fn report_title(year: Option::<i64>) -> String` = `format!("Phin report {}", label)` where
    `label` is the year or `"all years"`.
**Implementation brief:**
1. `Command::Analyze(name, year)`:
   a. `let view = read_view(&self.data_dir); let (tree, tree_complaints) = category_tree_at(&data_dir);`
   b. `let mut names_wanted = vec![name]` for built-ins; if the name is not one of `BUILTIN_ANALYSES`,
     the built-in table is `None` and the plugin tables are still asked (so a plugin-only analysis works).
     If no built-in matches **and** no plugin returns a table →
     `AnswerWith(ErrorReason::UnknownCommand, format!("unknown analysis: {name} (built-ins: categories, cashflow, monthly-stats, biggest-spendings)"))`.
   c. `unknown_category_paths` non-empty → `writer::log(…, "warning", format!("rules name categories the tree does not: {paths:?}"))`;
     `tree_complaints` → one `log` warning each.
   d. Build the tables (built-in first, then the plugin tables from
     `run_plugin_analyses(plugins, &[name], …)` — warnings from the host go out as `log` warnings).
   e. `let mut stream = writer::start(verb, tables.len() as i64)`; one `writer::progress` per table
     (`file: None`); then `writer::succeed(&stream, sink, payload)` with keys `name`, `year`
     (omitted when absent) and `tables` (an array of `json_of_table` values).
2. `Command::Report(format, year)`:
   a. `report_path(format, year)` → `None` →
     `AnswerWith(UnknownCommand, format!("unknown report format: {format} (markdown, html, pdf, ods)"))`.
   b. Build the document: title = `report_title(year)`; tables = the four built-ins in
     `BUILTIN_ANALYSES` order, then the plugin tables of
     `run_plugin_analyses(plugins, &BUILTIN_ANALYSES-names-as-strings, …)` in config order
     (decisions 16 and 30: plugin tables are appended, never merged, never deduplicated).
   c. `markdown_document(&title, &tables)` and `html_document(&title, &tables)` are built once;
     `markdown`/`html` for `Target`-free formats are the **document** form (a report is a document, not a
     bare table fragment).
   d. `format`/`year` payload keys: `year` is put only when given.
   e. For `markdown`/`html`: bytes = the document text; for `pdf`/`ods`: create the export workspace
     `create_dir_all({absolute target parent}/.phin-export-{std::process::id()})` and
     `export::convert(Target::Pdf | Ods, markdown_doc, html_doc, workspace)`;
     on `Err(ConvertError::ConverterMissing(hint))` → `writer::log(…, "warning", hint)` then
     `AnswerWith(ErrorReason::ConverterMissing, format!("no {format} output: {hint}"))` and **no file
     written**; on `Failed`/`Io` → `AnswerWith(ErrorReason::Other(…))`? No: pin
     `ErrorReason::PluginFailed` for a converter that ran and failed (the draft codes are five; a converter
     is an external process, the same class as a plugin), text = the converter's message.
   f. Write once: text formats via `fs_atomic::write_atomic`, byte formats via
     `fs_atomic::write_atomic_bytes`; a `WriteError` → `AnswerWith(ErrorReason::Io, …)`.
   g. Events: `writer::start(verb, 1 + tables.len() as i64)`, one `progress` per rendered table and one
     after the write (with `file: Some(relative path)`), `writer::cancel` if `writer::cancel_requested`
     is set **before** the write (nothing half-written reaches the data dir — plan Cancellation section,
     `core-protocol.md:183-201`), then `writer::succeed` with `format`, `year` (when given) and `path`.
3. `main.rs`: build `let mut analysis_work = AnalysisWork::new(settings.data_dir.clone());` in the daemon
   branch and `AnalysisWork::new(pipe_data_dir(&args))` in the pipe branch (a one-line change each), so
   `AnalysisWork` → `ImportWork` → `WiredNowhere` is the chain. Do not change `ImportWork`.
4. `data-files.md`: add a `### Generated documents` entry after the `state/` section:
   `report/` holds rendered analysis documents (`report/<year>.<ext>` for one year, `report/all.<ext>` for
   the whole dataset; extensions `md`, `html`, `pdf`, `ods`). They are **output, not facts**: they are
   rewritten in place (atomic temporary-file + rename, previous version kept as `<path>.bak`), they are
   never read back by the core, and deleting them loses nothing that cannot be regenerated.
5. `architecture.md`: add `report/` (generated documents) to the data-directory list near `:296`.
**Pinned decisions:** D-B (path, overwrite, documented as generated output), D-H (unknown verb/format
codes), decision 16 (report table set and order), decision 17 (`year` key omitted when absent; `all` in
the name), decision 18 (byte writer for PDF/ODS), decision 21 (`AnalysisWork` wraps `ImportWork`, the
`Work` seam is not widened, decision A6/26 holds), decision 26 (converter failure → `plugin-failed`).
**Edge cases / failure modes:**
- `report pdf` on a machine without `pandoc` (this machine) must still produce a PDF through LibreOffice
  (T6 pins the fallback); without LibreOffice at all it must log a warning and answer
  `converter-missing` with **no file** created.
- A cancelled report leaves no `report/` entry at all; a report whose write fails leaves the previous file
  intact (atomic writer, T6).
- `year: Some(2023)` on an all-2024 store → a document with four header-only tables (a report of an empty
  year is valid, not an error).
- A rules-file category outside the tree, or a missing `categories.csv`: warnings, and the analyses still
  run (all rows collapse to `unknown`, T2).
- Two reports in a row: the second overwrites and keeps `report/2024.md.bak` (`.gitignore` already covers
  `*.bak`).
**Tests / acceptance criteria** (scratch parent `"test-scratch-analysis-work"`; scripted sessions through
`interpret::run(&mut StringInput, &mut sink, &mut work)` with `interpret::scripted(lines)` and a frame
collector (the `import_work` tests' `collecting()`/`events_of`/`kinds_of` shape); the store is the T2/T3/T4
fixture store):
1. `analyze_biggest_spendings_returns_the_pinned_table` — command
   `{"v":1,"cmd":"analyze","name":"biggest-spendings","year":2024}`: exactly one `progress`
   (`done: 1, total: 1`) and one `result` whose payload has `name == "biggest-spendings"`,
   `year == 2024`, `tables.len() == 1`, and whose first table's `columns == ["date","description","amount"]`,
   `column_types == ["date","text","money"]`, and whose first `rows` entry is
   `["2024-01-24", "MEGA MART 04", -123456]` (the largest fixture expense).
2. `analyze_unknown_name_is_an_error_naming_the_builtins` — `analyze no-such` with no plugins configured →
   one `error` event, `code == "unknown-command"`, `text == "unknown analysis: no-such (built-ins: categories, cashflow, monthly-stats, biggest-spendings)"`,
   and the loop continues (a following `ping`-like/`version` or the session end is clean).
3. `analyze_cashflow_and_the_other_builtins_answer_tables` — `analyze categories`, `analyze cashflow`,
   `analyze monthly-stats` each give `tables.len() == 1` and the pinned table names
   `income and expenses by category 2024`, `cash flow 2024`, `monthly statistics 2024`.
4. `report_markdown_writes_the_expected_file_and_payload` — `report markdown` with `year: 2024`: one
   `result` whose payload has `format == "markdown"`, `year == 2024`, `path == "report/2024.md"`;
   the file exists at `<store>/report/2024.md`; its text starts with `# Phin report 2024` and contains the
   four headings `## income and expenses by category 2024`, `## cash flow 2024`,
   `## monthly statistics 2024`, `## biggest spendings 2024`; one progress event carries
   `file: "report/2024.md"`.
5. `report_without_a_year_is_the_all_document` — `report html` without `year`: `path == "report/all.html"`,
   the payload has **no** `year` key, and the file starts with `<!DOCTYPE html>`.
6. `report_twice_overwrites_and_keeps_the_previous_version` — two `report markdown` runs:
   `report/2024.md.bak` exists and is non-empty, and `report/2024.md` still parses as a markdown document
   (contains `# Phin report 2024`).
7. `report_unknown_format_is_an_error_and_writes_nothing` — `report xyz` → `code == "unknown-command"`,
   `text == "unknown report format: xyz (markdown, html, pdf, ods)"`, and
   `!Path::new("<store>/report/xyz…").exists()` — pin that the `report/` directory was not created at all.
8. `report_pdf_uses_the_converter_or_reports_converter_missing` — with `program_overrides` set to a bogus
   office program (the documented seam), `report pdf` answers a `log` warning naming `pandoc` and
   `libreoffice` and an `error` with `code == "converter-missing"`, and no `report/2024.pdf` exists;
   without overrides, `report pdf` on this machine (LibreOffice present, pandoc absent) answers a `result`
   whose bytes written to `report/2024.pdf` start with `%PDF-`.
9. `report_cancelled_before_the_write_leaves_no_file` — an in-process test that calls the private
   `run_report` with a stream whose cancel flag is already set (`writer::request_cancel`) → exactly one
   `cancelled` event, no `result`, and no file.
10. `every_other_verb_still_comes_from_import_work` — a session running `import` through `AnalysisWork`
   produces the same payload keys (`added`, `annotations`, `linked`) as the `import_work` tests, and
   `core/tests/import_lifecycle.rs` passes unchanged (its daemon path now runs `AnalysisWork`).
11. `the_documented_report_example_payload_shape_holds` — the T11-doc example
   `core-event-result-report` keys (`format`, `year`, `path`) are exactly the keys this task emits, and
   `report_path("html", Some(2024))` returns `("report/2024.html".to_string(), "html".to_string())` — the
   doc's own path value (test the doc string, don't import the proto helper; it is `#[cfg(test)]`-private
   to proto).
**Success check:** `cargo test -p core analysis_work` green and `cargo test -p core --test import_lifecycle`
green, then `cargo test --workspace` green.
**Review focus:** one terminal event per command (writer discipline: `succeed`/`fail`/`cancel` exactly
once); the `report/` path rule and the doc's `report/2024.html` example agree; the payload keys match the
doc examples; nothing written before the conversion succeeds; the `Work` seam was not widened
(amendment A6 still holds); `ImportWork` untouched.
**Out of scope:** the analysis maths (T2-T4), the renderer/export internals, the CLI process (plan 6.1),
`set-category`/`confirm-annotation`/`list-pending` wiring (plan 6.2/6.3), the daemon-side handshake gate
(issue #1).

### T13 — Hygiene sweep: review nits #12, stale comments, `.gitignore`

**Depends on:** T12
**Size:** 4-5 files, ~40 LOC, one commit naming #12. Trivial by itself, but it must be a separate gate so a
reviewer can see it is documentation-only.
**Why:** the Phase-4 hand-off table (issue #12: `file_stamp` doc says "before the Unix epoch";
`warnings[0].text.len() > 0` should be `!is_empty()`; the unreadable-stamp test skips silently on a
filesystem that accepts a pre-epoch mtime), plus the unfixed Phase-3 hand-off "3.6a's stale test-comment
line numbers … STILL UNFIXED and it names that as one-comment change for whoever owns proto next"
(Phase-3 log:185) — that owner is whoever touches `proto` next, and T11/T12 did.
**Current state (verified):** the exact current lines are found by `grep -n 'text.len() > 0'` and
`grep -n 'before the Unix epoch'` in `core/src/protocol/import_work.rs`, and by reading the stale test
comments in `proto/src/handshake.rs` (they cite line numbers from the 3.6a commit). `.gitignore` is 14
bytes and lists `*.bak` but not `*.phiptmp` (the Phase-2 finding "`.gitignore` covers `*.bak` but not
`*.phiptmp`" is still open).
**Already done / do not redo:** anything the earlier tasks already changed (`export.rs`, `analysis_work.rs`).
**Files:** Modify `core/src/protocol/import_work.rs` (doc text + the `!is_empty()` nit + the
skips-silently test note), `core/src/protocol/interpret.rs` (the two `WiredNowhere` refusal texts for
`report`/`analyze` and the pinned test's expected substrings), `proto/src/handshake.rs` (test-region
comments only), `.gitignore` (add `*.phiptmp` and `.phin-export-*`).
**Interfaces:** none — no public item changes signature or behaviour.
**Implementation brief:**
1. `warnings[0].text.len() > 0` → `!warnings[0].text.is_empty()` (behaviour identical).
2. `file_stamp`'s doc: replace the "before the Unix epoch" claim with an accurate sentence and keep the
   test's note that on a filesystem that accepts a pre-epoch mtime the assertion cannot pin the
   unreadability — the test must **assert** the typed complaint when the mtime can be made unreadable and
   otherwise assert the fallback path, so it never passes vacuously: the honest form is a
   `#[cfg(unix)]`-style explicit `chmod 0o000` (via `PermissionsExt`) for the unreadable-stamp case on
   Linux, and no silent return.
3. `WiredNowhere` refusal texts for `report` and `analyze` change to name the wiring state:
   `format!("the analysis of {name} is wired by plan item 5.8 in a running core; this fallback build of the interpreter does not wire anything")`
   and the same shape for `report`; update the pinned test's substring assertions from `"5.1-5.8"` to
   `"5.8"` and its blame-verb list stays the same verbs.
4. `proto/src/handshake.rs`: fix the stale test comments that cite line numbers (replace the numbers with
   the test/behaviour names so they cannot go stale again).
5. `.gitignore`: add `*.phiptmp` and `.phin-export-*/`.
**Pinned decisions:** D-G (issue numbers in the commit message: `hygiene: review nits and ignore entries (refs #12)`),
decision 27 (the stale-comment fix belongs to the proto owner's sweep).
**Edge cases / failure modes:** no behaviour change is allowed; if a pinned test's text must change, that
change belongs in this task and nowhere else (a reviewer must see the diff of the test file here).
**Tests / acceptance criteria:**
1. `cargo test --workspace` green with no test name changed.
2. `RUSTFLAGS=-Dwarnings cargo build --workspace --all-targets` shows no warnings.
3. `git status --porcelain` in a scratch store test: a `*.phiptmp` sibling left behind by an interrupted
   atomic write is ignored (verify by touching a file named `x.phiptmp` and `git check-ignore -q x.phiptmp`).
**Success check:** `cargo test --workspace` green + the 0-warning build.
**Review focus:** this task is documentation/nit-only; a reviewer must see no behaviour change and no
pinned-test relaxation (a test that stops asserting is a rejection).
**Out of scope:** issue #11 (the flaky timing tests), issue #1 (the daemon-side handshake gate), any
production behaviour change.

## Findings log (planning)

All quotes are verbatim from the sources named; `[scout]` marks who confirmed it (four scout rounds: state
logs, implemented interfaces, spec docs, environment/tests, plus a decisive category/fixture round).

- [scout] `doc/implementation/plan.md:243-277` — Phase 5 items 5.1–5.8 verbatim (quoted in Context);
  `:326-327`: "Phase 5 needs 2.6 + 3.x"; `:331-335` Definition of Done: "All tasks checked, `cargo test`
  green at the workspace root. Requirements 001–005 demonstrable through the CLI alone, on fixtures. Every
  protocol and file format in `doc/architecture/` matches the code (checked by the doc-reading tests of 3.1
  and 7.5). No task was closed without unit tests added in the same commit."
- [scout] `doc/architecture/architecture.md:15` — R005: "Creates human readable documents with analysis
  results | Analyses are rendered as Markdown or HTML; PDF and LibreOffice/ODS output is delegated to
  external converters."; `:21-23` Q1/Q2/Q3; `:421`: "adding a new analysis must mean adding one analysis
  plugin"; `:220` built-in analyses named in prose; `:222-224` Document Generation; `:300`: "Missing
  converters mean missing export formats, which is reported as a warning, not as an error."; `:284`:
  "converter processes are started for export only"; `:387-391` accepted decision "Delegated PDF and ODS
  export … produced by calling `pandoc` / LibreOffice"; `:336-338` Money: "integer minor units (`i64`),
  never floating point".
- [scout] `core-protocol.md:96-105` command table (only `report` is documented: "`report` | `format`,
  optional `year` | Render an analysis document (`markdown`, `html`, `pdf`, `ods`)"); `:88-90`: "The
  vocabulary is **data driven** … command names and their parameters are described in a table that both
  sides read"; `:130-137` events and the "exactly one `result` (or `error`/`cancelled`) per command" rule;
  `:168-170` example `core-event-result-report`:
  `{"v":1,"event":"result","cmd":"report","format":"html","year":2024,"path":"report/2024.html"}`;
  `:180-181` the five draft codes; `:183-201` cancellation ("finishes the **current item**, discards the
  partial result … Nothing is half written to the data files").
- [scout] **there is no `analyze` command anywhere in the docs or code**, and no documented analysis-name
  list; `plugin-protocol.md:118` uses the slug `"biggest-spendings"` while `:126` gives the display text
  `"biggest spendings 2024"`. Resolved by user decision 1 (D-A) + decision 16.
- [scout] `data-files.md` documents **no `report/` directory** and no analysis output/cache
  (`:139-144` names `state/core.lock` and `state/markers.csv` only; `architecture.md:296` lists the data
  directory contents without it). Resolved by user decision 2 (D-B).
- [scout] `plugin-protocol.md:112-132` `analyse` request/response examples (quoted in T10); `:18` gives
  `probe` for importers only — no probe step for analyses; `:15` "one process per plugin per run";
  `:39` "`amount` is an integer in minor units, negative = expense"; `:126` `columns` are plain strings and
  `rows` untyped arrays, so typed cells (plan 5.1) are unrepresentable. Resolved by user decision 3 (D-C,
  `analysis_plugins`) and user decision 4 (D-D, `column_types`).
- [scout] `data-files.md:155-161`: `plugins = ["./importers/bank-x"]` "one entry per plugin importer of
  `plugin-protocol.md`. Relative paths resolve against the data directory, absolute paths are kept as
  given"; "which converters" is listed as a key concept but **no literal key exists**. Resolved by user
  decision 3 (new key) and user decision 5 (no converter key this phase).
- [scout] Phase-4 `02_impl.md:61-72` issue table, verbatim rows for the issues this phase folds in:
  `#2`: "`exec_importers checks existence only; a non-executable plugins entry is invisible because a probe
  cannot fail by contract | T4/T5 review, decision I4 ("deferred to 4.8" - it never landed)";
  `#3`: "stderr drained only after reaping: > 64 KB of stderr mid-run fills the pipe and the plugin is
  reported as timed out"; `#4`: "every read is bounded by timeout_secs on its own, so a plugin emitting one
  line per just-under-timeout_secs never ends";
  `#11`: "transport::a_session_that_used_its_window_.., fs_atomic::scratch_parent_is_module_private /
  an_empty_target_.. fail under concurrent test pressure";
  `#12`: "file_stamp doc says 'before the Unix epoch'; warnings[0].text.len() > 0 should be !is_empty(); the
  unreadable-stamp test skips silently on a filesystem that accepts a pre-epoch mtime".
  And `:50`: "T9 follow-ups for the next plan: a daemon-side handshake gate (… #1); an occurrence-id-aware
  append inside technical::store_transactions (… #5); re-annotation of an in-place EDITED message is out of
  the marker contract (… #7)". Decided: fold #2/#3/#4 (user decision 7); leave #1 (decision 24), #11
  (decision 25), #5/#6/#7/#8/#9/#10 stay in their own domain (data-files/store/mail) and do not block
  Phase 5.
- [scout] Phase-3 `02_impl.md:79` (MAJOR carry-forward, verbatim): "`run takes the result payload immutably
  up front and CommandWork::do_item returns only an ItemOutcome, so a command whose payload summarizes its
  items (set-category→affected, **report→path/format/year**, import→written path …) cannot be expressed
  through `run`; 3.4 must extend do_item or drive start/progress/log/succeed itself" — the `report` payload
  named there is Phase 5's, and the pinned note at `interpret.rs:192-201` is what T12 follows.
- [scout] Phase-3 `02_impl.md:121` (SECURITY-RELEVANT carry-forward): "there is NO daemon-side ordering
  enforcement — version at any position is answered with hello and commands arriving before any handshake
  are served normally"; Phase-4 `:49`: "The plan's Context claim ('transport already gates it') is wrong -
  source (transport.rs:931,1346; core-protocol.md Handshake section is worded as an interface rule). Filed:
  https://github.com/ThomasFeher/phin/issues/1". Decision 24: out of scope here.
- [scout] Phase-3 `02_impl.md:185` (still-open one-comment change): "3.6a's stale test-comment line numbers
  … STILL UNFIXED and it names that as a one-comment change for whoever owns proto next". → T13.
- [scout] Phase-3 `02_impl.md:209` (flake lesson): "I MEASURED A FLAKE … 2 of 15 runs FAILED, both
  technical::transport::a_hello_answer_echoing_another_nonce_is_refused_as_not_our_daemon"; Phase-4 `:51`:
  "concurrent cargo test invocations trip the known timing-sensitive technical::transport::.. tests only";
  Phase-3 reviewer gate: "probe with `timeout 120 cargo test -p core --test <name>`". → Context gate.
- [scout] Phase-3 `02_impl.md:248` decision 32 (dependency precedent): "The TOML crate for 3.6 = basic-toml
  v0.1.10 (dtolnay), NOT toml v1.2.2+spec-1.1.0 | measured supply-chain delta with a scratch package (cargo
  tree in /tmp/depcheck): basic-toml adds exactly ONE new package … while toml … adds FIVE"; `:247`
  decision 31: "adding a TOML crate is a supply-chain choice (precedent: planning decision 3, where the USER
  chose the JSON backend for 3.1)"; boundary rule (`:155/:159/:221`): "the TOML dep pair MUST land WITH
  3.6b's consuming file, NEVER before it" (an `unused dependency` warning otherwise). → T5 step 0.
- [scout] Phase-4 amendment A6 (`02_impl.md:133-138`): "do NOT widen Work::do_command. ImportWork
  implements the existing seam … creates its own writer::start('import', total), drives progress/log, checks
  writer::cancel_requested(&stream) between files, and ends with writer::succeed (payload keys
  added/annotations/linked) or writer::cancel … decision 26 stays: WiredNowhere and its pinned refusal test
  are untouched". → T12's shape.
- [scout] Phase-4 amendment A4 (`:119-125`): "`ImportError` … extend … with a third variant
  ConverterMissing … map PdfTextError::ToolMissing to it in import_error_of instead of the current
  Failed('converter-missing: …') string … (inject the missing program through
  pdf_text::extract_text_with or a documented test hook - do not hack PATH)". → T6/T12's test seams.
- [scout] Phase-2 `02_impl.md:47`: "on BackupWriteFailed/RenameFailed the .phiptmp sibling is left on disk
  and .gitignore covers *.bak but not *.phiptmp". → T13.
- [scout] Phase-3 `02_impl.md:44`: "cargo fmt --check is not clean repo-wide (core 286 diffs, workspace 340)
  → fmt is not a gate"; Phase-4 `:29`: "repo has no clippy gate"; reviewer gate
  "RUSTFLAGS=-Dwarnings". → Context gates.
- [scout] split convention: Phase-3 item table `02_impl.md:5-12` lists `3.6a/3.6b/3.6c/3.6d` as separate
  rows with their own cycles and commits (parent row 3.6 exists only as decision 38: "split the row into
  3.6a / 3.6b / 3.6c, committed and checked separately"); Phase-4 `01_plan.md:55-56` splits 4.4 into T4
  (contract/orchestration) and T5 (executable-process host) "on content grounds". → this plan splits 5.8
  into T11/T12 and the issue fold into T7/T8/T9.
- [scout] `proto/src/commands.rs:182 command_rows()` verbatim rows (Context); `:631-697`
  `appending_a_row_to_the_command_table_validates_the_new_command_with_no_other_change` with
  `assert!(documented_rows.len() == 8)`, and its doc note: "a new documented command still needs one
  there" (the typed view). `proto/src/command.rs:52-72` `Command` enum; `:146-162 command_parameters` and
  `:196-215 read_typed_command` relate to rows by POSITION 0..7.
- [scout] `core/src/protocol/interpret.rs:663-701 WiredNowhere` arms (report text: "no analysis or renderer
  is wired for {format} yet: plan items 5.1-5.8 connect them to the interpreter"), pinned by
  `every_command_of_the_table_is_refused_with_a_documented_code_naming_its_phase` (`:1095`), which asserts
  the code list and `texts[1].contains("html")` and `texts[1].contains("2024") == false`.
- [scout] `core/src/protocol/writer.rs` signatures (Context) and `core/src/protocol/import_work.rs:146-161`
  (`impl interpret::Work for ImportWork`, the `other => self.fallback.do_command(other, sink)` delegation,
  `fallback: interpret::wired_nowhere()`); `main.rs:38/:279-280/:355-356` work wiring;
  `transport::serve_daemon(…, work: &mut dyn interpret::Work, …)` at `:1422`.
- [scout] `core/src/technical/fs_atomic.rs`: `write_atomic(:143)`, `write_temp` (:98, `<path>.phiptmp`),
  `back_up` (:114, `<path>.bak`), `commit` (:130), `BackupOutcome { PreviousKept, NothingToKeep }`,
  `WriteError { DirectoryRejected, PreviousUnreadable, TempWriteFailed, BackupWriteFailed,
  RenameFailed }` (each `(PathBuf, ErrorKind, String)`); text-only signature.
- [scout] `core/src/technical/pdf_text.rs:79`: `Command::new(program).arg("-layout").arg(path).arg("-").output()`,
  `ErrorKind::NotFound → PdfTextError::ToolMissing`; missing tool injected via
  `extract_text_with("definitely-not-pdftotext-xyz", …)`, "do not hack PATH".
- [scout] `core/src/technical/view.rs:139-144`: `read_view` reads `transactions.csv`, `annotations.csv`,
  `overrides.csv`, `rules/` only; it does **not** read `categories.csv` and never calls
  `rule::validate_rule_categories` (defined at `rule.rs:136`, called nowhere in `view.rs`);
  `View.rule_errors` stays `[]` for the fixtures (test `fixture_rules_and_categories_drive_classification`
  asserts `view.rule_errors.len() == 0`).
- [scout] `core/src/domain/category.rs:146-148`: "Forward parent references are allowed: parents are
  resolved once the whole text has been read"; `from_csv` on the fixture `categories.csv` returns
  `Ok(Tree)` (no `MissingParent` for the `expense;transport` rows) and
  `is_valid(tree, "finance:expense:transport") == true` (test `three_level_path_is_valid` passes). So the
  sample rules' three-level path is a real tree path — the T2 unknown bucket covers
  `Classification::Unknown` and genuinely-invalid paths only.
- [scout] fixture arithmetic (running sum per row, cross-checked against the passing view test and a
  Python emulation of `classify`): total of the 22 `testdata/transactions.csv` rows = **-38956**;
  per source: `bank_a.pdf -106282`, `bank_b1.pdf 70054`, `bank_b2.pdf -11379`, `mail_paypal.mbox 8651`;
  positives `145948`, negatives `-184904`; classification buckets `food:supermarket -32435 (6 rows)`,
  `food:restaurant -7580 (2)`, `finance:income +139000 (3)`, `finance:expense:transport -5499 (2)`,
  `leisure:books -2646 (2)`, `leisure:games -6789 (1)`, unknown `-123007 (6 unknown rows: -123456, 2599,
  -1999, 899, -4500, 3450)`; monthly nets `-106282, 70054, -11379, 8651`; expense counts per month `5, 3,
  4, 4`; 16 expense rows, 6 income rows. An earlier scout reported `-21620` for the total — it is wrong;
  the scout that produced `-38956` shows the per-row running sum and cross-checks two ways, and T2's test 8
  pins the identity.
- [scout] testdata inventory: `generate.py` (byte-identical regeneration), `bank_a.pdf`, `bank_b1.pdf`,
  `bank_b2.pdf`, `mail_paypal.mbox`, `mail_with_pdf.mbox`, `transactions.csv` (22 rows), `categories.csv`
  (11 rows), `rules/sample.rules` (10 rules), `README.md`; `core/tests/fixtures/fake_importer.py`
  (`-rw-r--r--`, run as `python3 <script> <mode>`), `find_fixture_dir()` at `importer_host.rs:1557`.
- [scout] environment: `pdftotext 26.11.0`, `libreoffice 26.2.7.3` (`soffice` same), `pandoc` **not
  installed** (exit 127), `python3 3.14.10`, `cargo 1.100.0`, `rustc 1.100.0 (2026-11-09)`; network to
  `index.crates.io` → HTTP/2 200; `cargo test --offline -p core --lib` exits 0 (290 passed);
  `~/.cargo/registry/cache` 29 crates; `Cargo.lock` 31 packages, no markdown/HTML parser; no
  `.cargo/config*`, no `vendor/`, no `rust-toolchain`, no CI config.
- [scout] test suite shape today: proto lib 33, core lib 290, core bin 4, `core/tests/import_lifecycle.rs` 4,
  `core/tests/transport_lifecycle.rs` 4, cli/tui/gui 1 each; **zero `#[ignore]`, zero `cfg(feature`,
  zero env-gated tests**; `std::os::unix::fs::PermissionsExt` appears nowhere; no test changes file
  permissions; `scratch_plugin_file` deliberately leaves the exec bit off.
- [scout] `proto/src/codec.rs:358/374/399/438` doc helpers are `#[cfg(test)] pub` — **not** callable from
  `core`'s tests (`#[cfg(test)]` items are not compiled into a dependency build); decision 36 keeps one
  owner inside proto. So T10's plugin-protocol doc test needs its own small private reader in `core`.
- [scout] `importer_host.rs` plumbing regions: `spawn_child:377`, `probe:410` (spawn failure →
  `ProbeKind::Unsupported`), `extract_transactions:470` (timeout text `"plugin {name} timed out after
  {secs} s"`), `PluginChild:566`, `read_line:627`, `reap:654` (kill → wait → bounded join → stderr drain),
  `Drop:688`, `spawn_line_reader:730`, `read_plugin_lines:746`, `drain_stderr_bounded:782`,
  `plugin_child_failure:954` (text `"plugin {name} {what}"` + `"; the plugin wrote on stderr: "` + exit
  status), `exec_importers:995`, `exec_importer_for:1030`, `plugin_program_path:1042`,
  `DEFAULT_PLUGIN_TIMEOUT_SECS:85`, `REAPER_GRACE_SECS:96`; `transport_cfg::plugins_from_config:547`
  (missing file → no key and no complaints; malformed list → no plugins handed out).
- UNVERIFIED (stated in the tasks, each with a pinned verification step): resolved versions/APIs of
  `pulldown-cmark`/`quick-xml` (T5 step 0); LibreOffice's HTML→ODS infilter string and PDF-from-HTML
  behaviour (T6 step 1); whether `cargo`'s `unused_dependencies` lint fires on a dev-dependency used only
  by `#[cfg(test)]` code (T5 success check covers it); whether pandoc's default PDF engine exists on this
  machine (irrelevant: pandoc is absent, and the fallback is pinned).

## Decision log (planning)

| # | decision | rationale | decider (agent|user) | status |
|---|----------|-----------|----------------------|--------|
| 1 | (D-A) Add a documented `analyze` command (`name`, optional `year`) with a `tables` result payload; `report` stays as documented; an unknown analysis name reuses `unknown-command`. | Plan `5.8` names an `analyze` command but `core-protocol.md:96-105` documents only `report`; plan 5.8's own test ("unknown analysis name → error event") needs a verb that takes a name; Q3 + DoD demand that a plugin-only analysis be reachable through the protocol. Offered as Q1, choice A. | user | NEEDS_USER resolved |
| 2 | (D-B) Reports go to `<data_dir>/report/<year>.<ext>` (`report/all.<ext>` when no year), overwritten in place by the atomic writer (`.bak` kept), documented in `data-files.md` as generated output. | The only documented path is `report/2024.html` (`core-protocol.md:170`); `architecture.md:296` lists no `report/`; overwriting is fine because reports are output, not facts (append-only protects facts, `data-files.md:43-56`). Offered as Q2, choice A. | user | NEEDS_USER resolved |
| 3 | (D-C) Analysis plugins come from a new config key `analysis_plugins` (same relative-path rule as `plugins`); canonical built-in slugs `categories`, `cashflow`, `monthly-stats`, `biggest-spendings`. | `data-files.md:155-61` scopes `plugins` to importers; without a second key the host could not be configured at all; `biggest-spendings` already appears in `plugin-protocol.md:118`. Offered as Q3, choice A. | user | NEEDS_USER resolved |
| 4 | (D-D) The plugin `table` event gains optional `column_types` (`money|count|date|text`); when absent the core infers from the column name (exact table in T10 step 3). | Plan 5.1 requires typed cells but `plugin-protocol.md:126` has untyped columns/rows; name-inference alone would silently guess at a contract. Offered as Q4, choice A. | user | NEEDS_USER resolved |
| 5 | (D-E) PDF: `pandoc` first, LibreOffice-from-HTML fallback; ODS: LibreOffice only; no converter config key this phase. | `architecture.md:387-391`/`:54` already delegate PDF/ODS; this machine has LibreOffice and **no pandoc**, so the fallback keeps the PDF path actually tested. Offered as Q5, choice A. | user | NEEDS_USER resolved |
| 6 | (D-F) 5.6's output is verified by parsing it with `dev-dependencies` (`pulldown-cmark`, `quick-xml`). | No markdown/HTML parser exists in the workspace and the crate choice was a recorded supply-chain event in earlier phases; the user chose the dependency option over a hand-rolled checker. Offered as Q6, choice B. | user | NEEDS_USER resolved |
| 7 | (D-G) Fold the upstream issues Phase 5 would inherit: extract the plugin plumbing (T7), fix #3 and #4 (T8), fix #2 (T9) before the analysis host. | Leaving them open would duplicate the defective plumbing into a second host (analysis payloads are larger than a pipe buffer), which the user's instruction "incorporate them if the fit or if they'd block anything" covers. Offered as Q7, choice A. | user | NEEDS_USER resolved |
| 8 | Commits: one per issue where possible, and any commit that carries an issue fix names its issue number; other commits follow ground rule 7 (`task id + title`). | The user's own words: "one commit per issue, if possible, or at least mention issue numbers in commit message". | user | NEEDS_USER resolved |
| 9 | Insert the `analyze` row **immediately after `report`** in the doc and in `command_rows()`, and renumber the typed-view positions 3..7 → 4..8 in both `command_parameters` and `read_typed_command`. | `commands.rs:173` states the rows are in the doc's order and `command.rs` relates a typed command to its row by POSITION, so doc order and code order must be one list; appending at the end would split the analysis verbs apart. | agent | JUSTIFIED |
| 10 | Year filter: `year: Option::<i64>` everywhere and `belongs_to_year(date, year)` = the four-digit prefix of the canonical `YYYY-MM-DD` date. | The protocol parameter is `ValueKind::Integer` (`commands.rs` `report` row) and dates are canonical strings (`domain::date`); one helper in T2 is reused by T3/T4/T10 so no second filter appears. | agent | JUSTIFIED |
| 11 | Table cells store amounts as `i64` minor units; the only money formatter is `domain::money::print`. | `architecture.md:336-338` (i64, never float) and plan 5.6 ("formatted per the currency format of 1.1"); formatting inside the domain table would make the plugin host re-format. | agent | JUSTIFIED |
| 12 | Category roll-up: a parent row covers itself plus all descendants; a category (or subtree) with no matching transaction is omitted; `Classification::Unknown` **and** tree-invalid rule categories share the last `unknown` row. | Plan 5.2 ("unknown category grouped separately; subcategories roll up into parents"); a rule naming a category the tree lacks is user error and must surface (T2 also returns `unknown_category_paths`), and omitting empty rows keeps a report readable. | agent | JUSTIFIED |
| 13 | `average expense (truncated)` = expense sum ÷ **expense count**, `i64` division truncating toward zero, `0` when the month has no expenses; the truncation is stated in the column header. | Q1 forbids floats and no exact integer form of an average exists; stating it in the header avoids a silently wrong number in a printed report. | agent | JUSTIFIED |
| 14 | `biggest-spendings` shows expenses only (`amount < 0`), sorted by `abs(amount)` descending, tie-broken by date, description, id; `TOP_DEFAULT = 10`. | Plan 5.4 ("top-N with ties"); the fixture's duplicate pair (`80c63211a8`, `…#2`) proves the order is total, which Q1's idempotence requires. | agent | JUSTIFIED |
| 15 | No analysis cache and no analysis output on disk other than `report/`. | R003/"Classification is Derived, never Stored" plus `architecture.md:421`; a cache would be a second source of truth for something the docs insist is derived on read. | agent | JUSTIFIED |
| 16 | `report` renders the four built-ins in the order categories → cashflow → monthly-stats → biggest-spendings, then plugin tables in config order; display name = `"<base name>< year>"` when a year is given (`"biggest spendings 2024"`). | `architecture.md:220` names the four built-ins; `plugin-protocol.md:126` already spells the display text exactly `"biggest spendings 2024"`, so pinning that shape keeps the built-in and the plugin form of a table indistinguishable to the renderer. | agent | JUSTIFIED |
| 17 | A report/analyze payload puts `year` only when a year was given, and the no-year document is `report/all.<ext>`. | `core-protocol.md:170` shows the year form (`report/2024.html`); `all` cannot collide with a 4-digit year name, and omitting the key keeps the payload honest about "no year chosen". | agent | JUSTIFIED |
| 18 | Add `fs_atomic::write_atomic_bytes(path, &[u8])` instead of forking the atomic writer or letting `export` rename files itself. | Plan 5.7 requires "output written atomically" for PDF/ODS, which are binary, and `write_atomic` takes `&str`; ground rule 2 keeps the store's atomicity in one module. | agent | JUSTIFIED |
| 19 | `export` writes its intermediates into `<parent-of-target>/.phin-export-<pid>/` and passes LibreOffice `-env:UserInstallation=file://{workspace}/lo-profile`. | LibreOffice locks a per-user profile, so a shared profile would serialize or fail concurrent conversions; a per-run directory next to the target keeps everything the process creates in one place the caller can clean. | agent | JUSTIFIED |
| 20 | Converter presence is decided by attempting the spawn and mapping `ErrorKind::NotFound`; never a `which`/`PATH` search. | `pdf_text.rs:79` is the accepted precedent (and its test injects a bogus program name rather than hacking `PATH`, amendment A4). | agent | JUSTIFIED |
| 21 | `AnalysisWork { data_dir, fallback: ImportWork }` wraps the existing work chain; `main.rs` hands `AnalysisWork` to the daemon and pipe paths; the `Work` seam is not widened. | Phase-4 amendment A6 pinned "do NOT widen Work::do_command" and the `ImportWork → WiredNowhere` delegation pattern; a wrapper keeps that intact with a one-line change at each injection site. | agent | JUSTIFIED |
| 22 | No probe step for analyses: every requested analysis name is sent to every configured analysis plugin; a plugin that answers `end` with no table declines silently. | `plugin-protocol.md:18` documents `probe` for importers only, and inventing a probe for analyses would change a protocol the doc calls accepted; declining silently is the least surprising reading of "answers end". | agent | JUSTIFIED |
| 23 | The dev-dependencies land in `core/Cargo.toml` in the same commit as the tests that use them, with the transitive delta measured in a throwaway crate and recorded in the commit message. | Precedent: decision 32 measured the "supply-chain delta with a scratch package"; a stray dependency produced an `unused dependency` warning that `RUSTFLAGS=-Dwarnings` makes fatal. | agent | JUSTIFIED |
| 24 | Issue #1 (daemon-side handshake gate) is out of this phase: no transport change here. | Phase 5 changes no transport behaviour and blocks on nothing from #1; the fix needs a doc decision (Phase-3 reviewer: "Interpreter-side enforcement would need an error code the DRAFT list lacks (doc change, ground rule 8)"), so it belongs to a transport-phase decision. Raised to the user in the plan summary as a recommendation. | agent | JUSTIFIED (scope) |
| 25 | Issue #11 (flaky timing tests) is not fixed here; instead the gate pins "never run `cargo test` concurrently" and `timeout 120 cargo test -p core --test <name>` for timing-sensitive suites. | A prior reviewer explicitly called the honest fix (a ≥ 100 s soak test) a USER decision, and no new user decision was given for it; the cheap mitigation is procedural and keeps the `cargo test --workspace` gate usable. | agent | JUSTIFIED (scope) |
| 26 | A converter that ran and failed answers `plugin-failed` (external process, same class as a plugin); a converter **timeout** is out of scope this phase; `WiredNowhere`'s new `Analyze` refusal also uses `plugin-failed`. | The five draft codes (`core-protocol.md:180-181`) are the only codes allowed (interpret.rs:60-62 keeps that list); `io` would blame the core's own files for an external process, and inventing a code needs a user decision. pdf_text has no converter timeout either, so adding one now would be unprecedent-ed. | agent | JUSTIFIED |
| 27 | The hygiene sweep (T13) belongs to the task that touches that crate last, and it is documentation/nit-only with its own gate. | The Phase-3 hand-off assigns the stale-comment fix to "whoever owns proto next" (log:185); mixing it into a feature task hides it in a large diff. | agent | JUSTIFIED |
| 28 | The `analyze` result payload keys are `name`, `year` (when given), `tables`; the table JSON is `{name, columns, column_types, rows}` (T11 pins them in a doc example). | `proto::Event::Result` takes a free map (no schema), so the only defence against drift is a doc example that the code-reading tests sweep; the same object shape as the plugin `table` event keeps one codec for both. | agent | JUSTIFIED |
| 29 | The exec-bit check is `#[cfg(unix)]` with the existence-only path documented for non-unix. | `std::os::unix::fs::PermissionsExt` is unix-only and the repo has never used it; Linux is the main target (`architecture.md` egui decision names Linux as the main target). | agent | JUSTIFIED |
| 30 | The host never deduplicates or name-matches: built-in tables and plugin tables are both included, each once per plugin per name. | There is no probe step (decision 22) so the core cannot know which plugin provides which analysis; silently dropping a duplicate table would hide a plugin that returns nothing. | agent | JUSTIFIED |
| 31 | The issue fold-in is split: T7 (move-only extraction) → T8 (#3 + #4, one commit each) → T9 (#2, its own commit) before T10. | Extraction deserves its own reviewer question ("did behaviour change?"); #3/#4 both live in the pipe/timeout internals while #2 is discovery-time validation, i.e. two independently reviewable concerns. | agent | JUSTIFIED |

## Coverage check

| requirement / user goal | task(s) | how verified |
|-------------------------|---------|--------------|
| Plan 5.1 (typed table, money column sums exactly, empty table = header only) | T1 (types, `money_sum` overflow-typed), T5 (header-only rendering) | `cargo test -p core result` (tests 1-8), T5 tests 3/4 |
| Plan 5.2 (income/expense per category per year; hand-computed totals; unknown grouped; subcategories roll up) | T2 | T2 tests 1, 2, 4, 7, 8 with the pinned tables above |
| Plan 5.3 (cash flow, cumulative balance per month; income−expenses = final balance; no clamping) | T3 | T3 tests 1, 2, 3, 5 |
| Plan 5.4 (monthly statistics + biggest spendings; top-N with ties; empty month; single-transaction month) | T4 | T4 tests 1, 2, 3, 4, 5, 7, 9 |
| Plan 5.5 (analysis plugin host; built-in and plugin tables equal; plugin error → warning, built-ins still run) | T10 (+T7/T8 plumbing it needs) | T10 tests 1, 2, 3, 4, 9 |
| Plan 5.6 (Markdown + HTML renderer; `|`/`<` escaping; empty table; 1.1's currency format; HTML parses) | T5 | T5 tests 1-7, 9, 10 (`pulldown-cmark`/`quick-xml` parse their own output) |
| Plan 5.7 (pandoc/LibreOffice export; converter missing → warning, no partial file; conversion when tools present, skipped-not-failed when absent; atomic output) | T6 (+T12's warning event and file write) | T6 tests 1-6; T12 test 8 |
| Plan 5.8 (`analyze` + `report` commands; unknown analysis name → error event) | T11 (vocabulary) + T12 (wiring) | T11 tests 1-5; T12 tests 1-8, 10, 11 |
| R005 / Document Generation (`architecture.md:15,222-224`) | T2-T6, T10, T12 | T12 tests 4-6 (a report document on fixtures through the protocol alone) |
| Q3 flexibility: a new analysis is one plugin (`architecture.md:421`) | T10 | T10 test 1 (a plugin's table equals the built-in table, no core change) |
| Q1 correctness: exact integer amounts, idempotence, nothing half-written | T1, T2-T4, T5, T6, T12 | T1 tests 1-2; T2 test 8; T3 test 1; T5 test 5; T6 test 4-5; T12 tests 6, 9 |
| Docs stay the source of truth (ground rule 6) | T6 (`architecture.md` export decision), T10 (`plugin-protocol.md` + `data-files.md` key), T11 (`core-protocol.md` row + 2 examples), T12 (`data-files.md` generated documents + `architecture.md` data-dir list) | T11 tests 1-2 (doc sweeps), T10 test 5, T12 test 11 |
| "incorporate upstream issues if the fit or if they'd block anything" (user request) | T7, T8 (#3, #4), T9 (#2), T13 (#12 + the Phase-3 stale-comment hand-off + `*.phiptmp`/`.phin-export-*` ignore entries); #1/#11 explicitly out of scope with decisions 24/25 | T8/T9/T13 named tests; T13 success check (0-warning build + `git check-ignore`) |
| Ground rules 1-8 (one task at a time, tests in the same task, no user data in tests, commit per task, stop on architecture conflicts) | every task | each task's success check is a `cargo test` gate on fixtures in `test-scratch-*` dirs; the commit rule with the issue-number exception is decision 8 |
