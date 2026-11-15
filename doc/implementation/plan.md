---
title: Phin Implementation Plan
---

# Ground Rules

These rules apply to every task in this plan and are the contract for the agent
that implements it.

1. **One task at a time.** Work on the lowest-numbered unfinished task whose
   dependencies are all done. Do not combine tasks.
2. **Task size.** A task produces at most ~150 lines of production code plus
   its tests. If a task grows beyond that, stop and report; do not silently
   expand it.
3. **Tests are mandatory and are written in the same task.** A task is not done
   if `cargo test` (workspace-wide) does not pass with new tests added for the
   new code. No task may be marked done with a red test.
4. **Test the domain, not the process.** Every module must be unit-testable
   without starting the core, without a GUI, and without file access, except for
   the file/protocol modules themselves.
5. **Never use the user's real data.** All tests run on fixtures under
   `testdata/` (see task 0.3). Reading `~/…/thunderbird` in code or in tests is
   forbidden.
6. **Document public items** with `///` doc comments; the interface docs in
   `doc/architecture/` (`core-protocol.md`, `plugin-protocol.md`,
   `data-files.md`) are the source of truth for the protocols and must be
   updated in the task that changes a protocol or file format.
7. **Commit per task**, message = task id and title (e.g. `2.4 transaction id`).
8. **If a decision in the architecture doc has to be changed to finish a task,
   stop and report** instead of inventing a decision.

Task status legend: `[ ]` open, `[x]` done, `[~]` in progress / blocked.

# Phase 0 — Scaffolding and Documentation

- [x] **0.1 Workspace skeleton** — Files: root `Cargo.toml` (workspace),
      packages `proto/`, `core/`, `cli/`, `tui/`, `gui/` with empty libs and
      `main` stubs, `plugins/`, `testdata/`, `doc/`. Depends: –.
      Tests: one trivial `#[test]` per package that the package compiles and
      links. Est. 60 LOC.
- [x] **0.2 Module layout inside `core`** — Files: `core/src/domain/{mod.rs,
      README.md}`, `core/src/technical/{mod.rs}`, `core/src/main.rs` (daemon
      stub reading lines from stdin). Depends: 0.1.
      Tests: `domain` module compiles with no `std::fs`, no `std::net` imports
      (enforced by a test that greps the sources — see 0.3 helper). Est. 50 LOC.
- [x] **0.3 Test fixtures** — Files: `testdata/README.md`, `testdata/generate.py`
      (or a Rust generator) that creates: 3 fake bank-statement PDFs of two
      different bank layouts (text layer, no OCR), 2 small mbox files
      (≈ 20 messages, one with a PDF attachment, one PayPal-style
      notification), a `transactions.csv`, `categories.csv`, `rules/` sample.
      Depends: 0.1. Tests: generator is deterministic (same output for two runs,
      checked by hash). Est. 200 LOC of generator, no Rust.
- [x] **0.4 README** — Files: `README.md` (extend the existing one). Content:
      what the tool does in 5 sentences, the three interfaces, how to build
      (`cargo build --workspace`), how to run the daemon + CLI, where the data
      files live, where the docs live. Depends: 0.1. Est. 40 lines.
- [x] **0.5 Interface documentation, part 1** — Files: `doc/interfaces/README.md`
      (index), `doc/interfaces/core-protocol.md` (commands, events, cancel,
      … — later moved to `doc/architecture/`, index content merged into
      `doc/architecture/architecture.md`, see "Technical Context")
      versioning, JSON-Lines examples), `doc/interfaces/plugin-protocol.md`
      (`probe`, `extract`, `analyse`), `doc/interfaces/data-files.md` (every
      file, every column, append-only rule, id format). Depends: 0.4.
      Tests: the JSON examples in the docs are parsed by a Rust test that reads
      them (see 3.x) — leave a marker so this can be added later. Est. 250 lines.

# Phase 1 — Domain Model (pure, no I/O)

- [x] **1.1 Money** — Files: `core/src/domain/money.rs`. Parse and print
      amounts in one settlement currency as `i64` minor units.
      Depends: 0.2. Tests: `"1.234,56 €"` → 123456; `"-12,00"` → -1200;
      `"0,00"` → 0; `"1.000"` (thousands sep) → 100000; garbage input → typed
      error, never a panic; round-trip print/parse identity; no float anywhere.
      Est. 90 LOC + 15 tests.
- [x] **1.2 Dates** — Files: `core/src/domain/date.rs`. Parse the date formats
      that occur in the fixture bank layouts; store as `YYYY-MM-DD` string
      internally. Depends: 0.2. Tests: each fixture format parses; ambiguous
      `01/02/2024` rejected explicitly; invalid calendar dates rejected;
      round-trip. Est. 80 LOC.
- [x] **1.3 Transaction type** — Files: `core/src/domain/transaction.rs`.
      Fields: id, date, amount (minor units), currency, description,
      counterparty, source. Constructor + `is_income()` (amount > 0).
      Depends: 1.1, 1.2. Tests: construction from parsed parts; sign
      convention documented and tested; struct is `#[derive(Eq)]`-able. Est. 70 LOC.
- [x] **1.4 Category tree** — Files: `core/src/domain/category.rs`.
      `parent:child` strings form a tree; validate against `categories.csv`
      content given as a string. Depends: 0.2. Tests: `food:supermarket` valid
      iff `food` exists; unknown category → error listing candidates; depth ≥ 3
      allowed; cycle detection. Est. 100 LOC.
- [x] **1.5 Rule file parsing** — Files: `core/src/domain/rule.rs`. Parse the
      text rule format (`pattern → category`, `#` comments, empty lines, file
      order = precedence). Depends: 1.4. Tests: comment-only file; malformed
      rule line → error with line number, not a panic; pattern types: literal,
      word list, regex (explicitly marked); ordering preserved. Est. 110 LOC.
- [x] **1.6 Rule engine** — Files: `core/src/domain/classify.rs`. Pure function
      `(transaction, rules, overrides) → category | unknown`. Precedence:
      override > learned rule > built-in rule. Depends: 1.3, 1.4, 1.5.
      Tests: each precedence level wins in a crafted conflict; no match →
      `unknown`; same input always same output (called twice); 500 rules ×
      1000 transactions completes in a test without timeout; regex catastrophic
      case guarded (length limit). Est. 120 LOC + 12 tests.
- [x] **1.7 Annotation + confidence** — Files: `core/src/domain/annotation.rs`.
      Annotation references a transaction id, has text/items, source, and a
      confidence 0…1. Depends: 1.3. Tests: confidence clamped; annotation
      without existing transaction id is representable (orphan case); items
      list may be empty. Est. 80 LOC.
- [x] **1.8 Learning a rule from a correction** — Files:
      `core/src/domain/learn.rs`. Given a transaction and a category, propose a
      rule (merchant/description pattern) and report how many transactions it
      would affect. Depends: 1.6. Tests: pattern is not the full description
      (too broad / too narrow cases); proposed rule re-parses as valid rule;
      affected count matches a fixture set; refusing to learn when the
      description is unique (no generalization possible). Est. 120 LOC.

# Phase 2 — Data Store (append-only text files)

- [ ] **2.1 CSV line codec** — Files: `core/src/technical/csv.rs`. Read/write
      one record, `;` separator, `#` comment lines, quoted fields. Depends: 0.2.
      Tests: field with `;`, with `"`, with newline rejected explicitly; empty
      optional columns; CRLF tolerated; round-trip identity for 20 random
      records (generated in the test). Est. 120 LOC.
- [ ] **2.2 Content-hash transaction id** — Files: `core/src/technical/id.rs`.
      `id = short hash(date, amount, currency, description, counterparty,
      source identity)` + occurrence suffix `#2` for identical content.
      Depends: 1.3, 2.1. Tests: same transaction from a different path → same
      id; two identical transactions in one file get `…` and `…#2`; hash length
      stable; hand-edited field → different id (documented in test name). Est. 90 LOC.
- [ ] **2.3 `transactions.csv` reader/writer** — Files:
      `core/src/technical/store_transactions.rs`. Append-only writer (never
      rewrites existing lines), reader into `Transaction` values, skip known
      ids on append. Depends: 2.1, 2.2, 1.3. Tests: appending an existing
      transaction adds nothing (idempotence); appending a new one keeps old
      lines byte-identical; corrupt line reported with line number and does not
      abort the rest. Est. 120 LOC.
- [ ] **2.4 Side files: `annotations.csv`, `overrides.csv`, `categories.csv`** —
      Files: `core/src/technical/store_sidefiles.rs`. Read/append for each.
      Depends: 2.1. Tests: override for unknown id is stored and reported as
      orphan on read; duplicate override → last one wins, both kept in file;
      `categories.csv` round-trip preserves order and comments. Est. 130 LOC.
- [ ] **2.5 Atomic write helper** — Files: `core/src/technical/fs_atomic.rs`.
      Write temp file in same directory, `rename`, keep `.bak` of previous
      version. Depends: 2.3. Tests: simulated crash (write then kill before
      rename) leaves original file intact; no data loss on empty target;
      directory creation. Est. 80 LOC.
- [ ] **2.6 Merge-on-read view** — Files: `core/src/technical/view.rs`.
      Produce the effective transaction list: facts + annotations + derived
      category (calls domain). Depends: 2.3, 2.4, 1.6. Tests: changing one rule
      file changes the category of all matching transactions without touching
      `transactions.csv`; orphaned override reported; re-read twice gives
      identical result. Est. 120 LOC.
- [ ] **2.7 Import markers (`state/`)** — Files: `core/src/technical/state.rs`.
      Per source file: path, mtime, size, last processed offset. Depends: 2.1.
      Tests: unchanged file → marker says "skip"; changed mtime → reprocess;
      marker file corrupt → treated as empty with a warning. Est. 90 LOC.

# Phase 3 — Core Protocol and Command Interpreter

- [ ] **3.1 JSON-Lines codec in `proto`** — Files: `proto/src/{command.rs,
      event.rs, codec.rs}`. Encode/decode one JSON object per line, `v` field.
      Depends: 0.5. Tests: unknown command → typed error, not panic; unknown
      field ignored; multi-line input split correctly; unicode preserved;
      decoding the examples from `doc/architecture/core-protocol.md` (read the
      file in the test). Est. 130 LOC.
- [ ] **3.2 Command table (single source of vocabulary)** — Files:
      `proto/src/commands.rs`. Data-driven table of command name → parameter
      spec, used by both sides. Depends: 3.1. Tests: adding a command to the
      table requires no other change (test adds one and checks it validates);
      missing required parameter → error naming the parameter. Est. 100 LOC.
- [ ] **3.3 Event stream writer on core side** — Files: `core/src/protocol/
      writer.rs`. Emit `progress`, `log`, `result`, `error`, `cancelled`
      events; check a cancel flag between items. Depends: 3.1. Tests: cancel
      set after 3 of 10 items → exactly one `cancelled` event, no `result`;
      progress numbers monotonic; error event carries command name. Est. 110 LOC.
- [ ] **3.4 Command interpreter (daemon main loop)** — Files: `core/src/main.rs`
      + `core/src/protocol/interpret.rs`. Read stdin lines, dispatch, write
      events to stdout, flush per line. Depends: 3.2, 3.3. Tests: feed a
      scripted command sequence into a `StringInput` harness and assert the
      event sequence; unknown command → `error` event and loop continues;
      EOF → clean exit. Est. 130 LOC.
- [ ] **3.5 `ping`, `version`, `quit` commands** — Files: as above. Depends: 3.4.
      Tests: round-trip ping/pong; version matches `proto` constant. Est. 40 LOC.
- [ ] **3.6 Transport: connect-or-spawn** — Files: `core/src/technical/
      transport.rs`, `proto/src/handshake.rs`. Bind `127.0.0.1:port` (from
      config, default `47623`), write `state/core.lock` (address + nonce),
      handshake (`version` + nonce), remove lock on exit, idle timeout shutdown.
      Depends: 3.4, 2.7. Tests: no daemon → spawn path taken; existing daemon
      reused (second interface does not start a second core); stale lock file →
      new daemon and lock overwritten; listener on the port without our nonce →
      error naming the address, no command sent; two interfaces share one daemon
      and their commands interleave; idle timeout exits the daemon; lock file
      removed on exit. Est. 140 LOC.

# Phase 4 — Import & Extraction

- [ ] **4.1 `pdftotext -layout` call helper** — Files: `core/src/technical/
      pdf_text.rs`. Calls the external `pdftotext -layout` (decision recorded in
      the architecture doc; native `pdfium-render` is the recorded alternative),
      returns text in reading order with columns preserved; detect a missing
      tool once and report it as a clear error. Depends: 0.3. Tests: fixture
      PDF text extracted in reading order; two-column layout keeps numbers
      attached to the right row (assert on fixture values); missing `pdftotext`
      → typed error, not a panic; empty PDF → empty result. Est. 90 LOC.
- [ ] **4.2 Bank statement parser (bank A)** — Files: `core/src/technical/
      parse_bank_a.rs` → `Vec<Transaction>`. Depends: 4.1, 1.1, 1.2, 1.3.
      Tests: all transactions of fixture A extracted with exact amounts and
      dates; a deliberately broken variant fixture reports a warning and still
      yields the parseable rows; running twice yields identical output. Est. 130 LOC.
- [ ] **4.3 Bank statement parser (bank B)** — same as 4.2 for the second
      fixture layout. Depends: 4.2. Tests: as above with fixture B. Est. 130 LOC.
- [ ] **4.4 Importer host + plugin contract** — Files: `core/src/technical/
      importer_host.rs`. `probe` every configured importer (built-in and plugin
      executable), then `extract`; plugin process started, JSON-Lines in/out.
      Depends: 3.1, 4.2, 4.3. Tests: a plugin that answers `probe: false` is
      never asked to extract; a plugin that exits non-zero → `error` event with
      plugin name, other importers still run; timeout kills plugin. Est. 140 LOC.
- [ ] **4.5 mbox reader** — Files: `core/src/technical/mbox.rs`. Split on
      `^From ` lines, parse headers (`X-Mozilla-Status`), MIME parts, base64
      attachments. Depends: 0.3. Tests: fixture mbox yields exactly N messages;
      escaped `>From ` in body does not split; attachment decoded to bytes;
      message without attachment; resume from byte offset (uses 2.7). Est. 140 LOC.
- [ ] **4.6 Mail annotation extraction** — Files: `core/src/technical/
      mail_annotate.rs` → `Vec<Annotation>` (merchant behind a "paypal"
      transaction, items from an attached invoice PDF via 4.1/4.2). Depends:
      4.5, 4.1, 1.7. Tests: PayPal notification yields annotation with items;
      attached invoice yields items; annotation carries confidence; no
      transaction is ever produced by this code (assert on return type). Est. 130 LOC.
- [ ] **4.7 Automatic annotation → transaction linking** — Files:
      `core/src/technical/link.rs`. Match by date window, amount, counterparty;
      set confidence. Depends: 1.3, 1.7, 2.6. Tests: unique match → high
      confidence; two candidates → lower confidence and both recorded; no
      candidate → orphan annotation; date window boundary cases. Est. 120 LOC.
- [ ] **4.8 `import` command wired into interpreter** — Files: interpreter +
      importer host. Depends: 3.4, 4.4, 4.5, 2.7. Tests: import of fixture
      directory appends exactly the expected transactions and emits progress;
      second import appends nothing; interrupted import leaves files valid. Est. 100 LOC.

# Phase 5 — Analysis and Documents

- [ ] **5.1 Analysis result data type** — Files: `core/src/domain/result.rs`.
      Table/series type with typed cells (money, count, date). Depends: 1.1.
      Tests: money column sums exactly (no float); empty table renders as
      header only. Est. 90 LOC.
- [ ] **5.2 Income/expense per category per year** — Files:
      `core/src/technical/analyse_categories.rs`. Depends: 2.6, 1.4, 5.1.
      Tests: fixture data yields hand-computed totals; unknown category grouped
      separately; subcategories roll up into parents. Est. 120 LOC.
- [ ] **5.3 Cash flow analysis** — Files: `core/src/technical/analyse_cashflow.rs`
      (cumulative balance per month). Depends: 5.2. Tests: sum of income minus
      expenses equals final balance in fixture; negative balance represented
      (no clamp). Est. 90 LOC.
- [ ] **5.4 Monthly statistics + biggest spendings** — Files:
      `core/src/technical/analyse_stats.rs`. Depends: 5.1. Tests: top-N with
      ties; empty month; single transaction month. Est. 110 LOC.
- [ ] **5.5 Analysis plugin host** — Files: `core/src/technical/analysis_host.rs`.
      Start plugin process, feed transactions + category tree as JSON-Lines,
      read table. Depends: 5.1, 3.1, 4.4. Tests: built-in and plugin analysis
      produce the same table for the same input (test compares them); plugin
      error → warning, built-ins still run. Est. 130 LOC.
- [ ] **5.6 Markdown and HTML renderer** — Files: `core/src/technical/render.rs`.
      Depends: 5.1. Tests: escaping of `|` and `<`; empty table; long numbers
      formatted per the currency format of 1.1; HTML is valid enough to be
      parsed by a test parser. Est. 130 LOC.
- [ ] **5.7 Export delegation (pandoc / LibreOffice)** — Files:
      `core/src/technical/export.rs`. Depends: 5.6, 2.5. Tests: converter
      missing → clear warning and no partial file; conversion of fixture
      Markdown to PDF/ODS when tools are present (skipped, not failed, when
      absent); output written atomically. Est. 110 LOC.
- [ ] **5.8 `analyze` and `report` commands** — Files: interpreter. Depends:
      3.4, 5.2–5.7. Tests: scripted sequence `analyze` + `report markdown`
      yields the expected file; unknown analysis name → error event. Est. 80 LOC.

# Phase 6 — User Interfaces

- [ ] **6.1 CLI interface process** — Files: `cli/`. Connect-or-spawn the core
      daemon (task 3.6), map argv → command, render events as text. Depends:
      3.2, 3.4, 3.6. Tests: argument parsing table (each command, missing arg,
      unknown flag); rendering of an event stream captured from the core
      harness; exit code non-zero on `error` event; a second CLI invocation
      while a daemon runs reuses it. Est. 140 LOC.
- [ ] **6.2 `refine` command round-trip (R004)** — Files: interpreter + domain
      + store. Depends: 1.8, 2.4, 6.1. Tests: correcting one transaction
      changes its category and the reported affected count; the learned rule
      file is valid and re-applies on next read; override beats learned rule.
      Est. 100 LOC.
- [ ] **6.3 Pending-annotation confirmation (R002)** — Files: interpreter +
      store + CLI. Depends: 4.7, 2.4, 6.1. Tests: low-confidence links listed;
      confirming writes the link, rejecting keeps the annotation orphan;
      re-listing shows nothing. Est. 110 LOC.
- [ ] **6.4 TUI interface** — Files: `tui/` (plain terminal, no extra toolkit
      required at first). Depends: 6.1. Tests: the TUI reuses the CLI event
      renderer; scripted session yields expected screen model (test the
      renderer, not the terminal). Est. 150 LOC.
- [ ] **6.5 GUI interface (`egui`)** — Files: `gui/`. Depends: 6.1. Tests:
      model/view split so the view is testable without a window; command
      dispatch and event handling tested headless. Est. 200 LOC.

# Phase 7 — Plugins and Integration

- [ ] **7.1 Importer plugin example in Rust** — Files: `plugins/importer-rust/`.
      Depends: 4.4. Tests: plugin passes the same importer contract test as the
      built-ins (shared test harness). Est. 100 LOC.
- [ ] **7.2 Importer plugin example in Python** — Files:
      `plugins/importer-python/`. Depends: 7.1. Tests: same contract test
      harness passes. Est. 60 lines.
- [ ] **7.3 Analysis plugin example** — Files: `plugins/analysis-*/`.
      Depends: 5.5. Tests: contract test harness; output renders through 5.6.
      Est. 80 LOC.
- [ ] **7.4 End-to-end integration test** — Files: `tests/` (workspace-level).
      Depends: all of 4–6. Runs: import fixtures → refine one classification →
      confirm one annotation → analyze → report markdown; asserts the final
      table and that a second run is a no-op (idempotence). Est. 120 LOC.
- [ ] **7.5 Documentation update pass** — Files: `doc/architecture/{core-protocol,plugin-protocol,data-files}.md` (transport,
      handshake, lock file), `README.md`, `doc/architecture/architecture.md`
      (decisions already recorded: egui, transport, `pdftotext`). Depends: 7.4.
      Est. 150 lines.

# Parallelizability

Independent after Phase 0–2: 1.x (domain) ∥ 2.x (store) ∥ 3.1–3.2 (proto).
Phase 4 needs 1.x + 2.x + 3.4. Phase 5 needs 2.6 + 3.x. Phase 6 needs 3.x and
can start with 6.1 as soon as 3.4 and 3.6 are done. Phase 7 last.

# Definition of Done (whole project)

- All tasks checked, `cargo test` green at the workspace root.
- Requirements 001–005 demonstrable through the CLI alone, on fixtures.
- Every protocol and file format in `doc/architecture/` matches the code
  (checked by the doc-reading tests of 3.1 and 7.5).
- No task was closed without unit tests added in the same commit.

# Review notes — Phase 1 → Phase 2

Left open by the Phase-1 reviewers on their own merits (low-severity notes at an
APPROVE, or changes out of an item's declared scope). A Phase-2 item must act on
these, not re-decide them. Full review logs:
`.agents/plans/2026-11-14_phase-1-domain-model/02_impl.md` (Findings log and
Decision log (implementation)).

- **1.3 → 2.1/2.3/3.x — error enums are inconsistent.** `MoneyParseError`
  (`core/src/domain/money.rs`) derives only `Debug`, while `DateParseError`
  derives `Eq, PartialEq`. Any item that compares parse errors with `==` must
  add `Eq, PartialEq` to `MoneyParseError` first; all its variants are empty, so
  the derive is harmless.
- **1.3 → 2.3 — double scaling is convention-only.** The store reader must build
  `Transaction` values with `from_stored` (stored `i64` minor-unit count +
  canonical `YYYY-MM-DD`), never `from_parts`: `money::parse("-3999")` yields
  `-399900`. Stated as an INVARIANT on `from_parts`; not type-enforced.
- **1.6 — regex patterns are recompiled per (rule, transaction) pair.** Measured:
  500 regex rules × 1000 transactions = 0.26 s debug / ~15 ms release. If 2.x/3.x
  profiling shows this hot, hoist compiled patterns to the caller
  (`Vec::<(Rule, Option::<regex::Regex>)>`); do not add a hidden cache to the pure
  domain.
- **1.6 — the regex input guard is silent.** A description longer than
  `MAX_REGEX_INPUT` = 512 code points skips ALL `regex:` rules (Literal and
  `words:` still match), so the same transaction can classify differently purely
  by description length with no signal to the caller. Documented in
  `classify.rs`; it must also be visible where users write `regex:` rules
  (`rule.rs` module doc — done in this sweep).
- **1.6 — `Rule`'s fields are `pub`** (needed by the sibling module `classify`), so
  a caller can build a `Words` rule with an empty `words` list, which never
  matches. Add smart constructors (`Rule::literal/words/regex`) in `rule.rs` when a
  later item constructs rules outside `parse_rules`; do NOT privatise the fields —
  that ripples into 1.5's committed code.
- **1.6/1.8 — `words:` matching is substring containment**, so `words:NIGHT`
  matches `MIDNIGHT`; learned proposals (1.8) are always `Literal` to keep a
  merchant-name prefix contiguous. Token-accurate matching needs a `regex:` rule.
- **1.8 → 2.x — `rule_to_text` round-trips only canonical patterns.** Proposals are
  token-join Literals (ASCII alnum + single spaces) and Words render as
  `words: a, b`; a pattern containing `->` or a leading `words:`/`regex:` marker
  does not survive render→parse. 2.x must not render arbitrary user rules through
  it.
- **Fixed in this same commit** (were stale/wrong before): `doc/architecture/architecture.md`
  said `rules/learned.txt` → now `rules/90-learned.txt` (the finalized numeric-prefix
  convention of plan item 1.5); `transaction.rs` doc comments cited the store reader
  as `core/src/domain/store_transactions.rs` → now `core/src/technical/store_transactions.rs`
  (plan item 2.3 is in the technical layer).
