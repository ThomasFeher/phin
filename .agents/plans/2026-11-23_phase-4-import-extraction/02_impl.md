# Implementation state
Plan: ./01_plan.md (read-only, never edit)
Plan format: detailed-v1
VCS: git | branch: main (never changed by me) | base: 579e3eca419918254e0384c69d029e2c0f2ecd30 | plan commit: 3c59a40

## Item state
| id | status | cycles | commit (sha) |
|----|--------|--------|--------------|
| T1 | DONE | 1 | 98877af |
| T2 | DONE | 2 | 64d3150 |
| T3 | DONE | 1 | 020d522 |
| T4 | DOING | 0 | |
| T3 | TODO | 0 | |
| T4 | TODO | 0 | |
| T5 | TODO | 0 | |
| T6 | TODO | 0 | |
| T7 | TODO | 0 | |
| T8 | TODO | 0 | |
| T9 | TODO | 0 | |

## Findings log (implementation)
- [developer] repo convention forbids the shared `test-scratch/` parent; each module has its own (`test-scratch-view`, `test-scratch-fs-atomic`, `test-scratch-transport-cfg`) - source (core/src/technical/view.rs:181-189)
- [developer] hand-crafted blank one-page PDF byte constant is accepted by poppler 26.11.0 (`pdftotext -layout` exit 0, empty stdout) - command output (T1 test `empty_pdf_yields_empty_result`)
- [reviewer] spawning a missing program yields `ErrorKind::NotFound` on this platform, so `ToolMissing` is real, not assumed - command output (T1)
- [reviewer] `Command::new(program).arg(path)` passes the Path directly; on Unix identical to the brief's "path-as-posix-path" - source (core/src/technical/pdf_text.rs:84)
- [developer] `testdata/transactions.csv` is self-consistent but its counterparty column is HAND-ANNOTATED (`REWE MARKT 12` -> `REWE MARKET`, `PAYMENT TO TOBIAS` -> `TAXI BERLIN`); `base_id` hashes the counterparty, so decision 18 (`counterparty == description`) cannot reproduce the CSV ids - source (testdata/generate.py:17-54 CSV writer; id.rs:88-103 payload; sha256 recomputed in python)
- [developer/reviewer] under decision 18 the bank A base ids are `1a0aa13ad9, 8111d8fe49, 59d934f85e, 3f77f63298, 845d4e59d4, 89e081d0a4` and bank B1 are `c402e912d8, 2085317984, ddbde11186, 58746a5f8f`; the CSV's `39945b2fed...`/`a86d11da9a...` come from the annotated counterparties - command output (python sha256, both variants)
- [reviewer] `id.rs:203-228` recomputes ids FROM the CSV rows (self-consistent, no parser involved); but `id.rs:256,264-265,271,282,293,315-341` hard-code the literals `80c63211a8`/`39945b2fed` in committed Phase-2 tests, so regenerating the fixture counterparties would force edits to already-shipped tests - source (core/src/technical/id.rs)
- [reviewer] T2 code: interface shapes exact, no `#N` from the parser, no minor-unit re-scaling, four brief-named tests present, no panic path; dead-code nit at parse_bank_a.rs:222 (duplicate-token guard) - command output (cargo test -p core parse_bank_a -> 9 passed; build 0 warnings; workspace green)
- [reviewer] cycle-2 T2: empty-counterparty rule applied everywhere a transaction is built; date-only candidate row takes the `next_back() -> None` arm and warns without panicking (reproduced with a scratch rustc program); parser never calls `occurrence_id`, so no `#N` can appear - command output (cargo test -p core parse_bank_a -> 9 passed; independent sha256 recomputation of all six ids)
- [reviewer] T2 nit not blocking: `warnings[0].text.len() > 0` should be `!is_empty()` (repo has no clippy gate) - source (core/src/technical/parse_bank_a.rs:341)
- [reviewer] T3 approved at cycle 1: interfaces exact, report/warning types imported from parse_bank_a (not duplicated), no shared generic parser, card-token policy (non-matching second token -> typed warning) reviewed and endorsed as correct for this scope - command output (cargo test -p core parse_bank_b -> 10 passed; sha256 recomputation of all 8 ids; build 0 warnings; workspace green)

## Decision log (implementation)
| # | decision | rationale | decider (agent|user) | status (JUSTIFIED / NEEDS_USER / resolved) |
|---|---|---|---|---|
| I2 | DECISION 18 IS REPLACED: bank parsers set `counterparty = ""` (nothing recognized in Phase 4), not `counterparty = description`; imported ids are therefore neither the fixture ids nor decision-18 ids, and the plan's Context id list is retired — tests assert the ids the pipeline produces (amendments A1/A2/A3). `testdata/transactions.csv` stays a hand-annotated STORE example (mail rows + curated counterparties are fixture-side only), so it is left untouched | user asked "why should counterparty equal description?"; `transaction.rs:60` "empty when unknown" and `data-files.md:71` "as recognized by the importer" make empty the honest value, and copying the description into the counterparty slot contradicts `transaction.rs:70-72` ("the description and counterparty slots are NOT interchangeable"); keeping the fixture keeps committed Phase-2 tests (`id.rs:203-228,256-341`) valid | user ("yes, D") | resolved |
| I1 | T1 test scratch dir = module-private `test-scratch-pdf-text` instead of the plan's `test-scratch/pdf_text/` | repo convention after the 3.6b review (view.rs:181-189 documents removal of the shared parent); reviewer independently verified | agent | JUSTIFIED |

## Plan amendments

### A1 — replaces T2 (counterparty empty; new expected ids)
- reason: user decision I2 replaces planning decision 18; the fixture-CSV id expectation is un-derivable (see findings log)
- replacement brief:

**Depends on:** T1 (consumes `pdf_text::extract_text`)
**Files:** create `core/src/technical/parse_bank_a.rs`; modify `core/src/technical/mod.rs` (`pub mod parse_bank_a;`).
**What stays as the plan says:** `BankRowWarning { line: usize, text: String }`, `ParseReport { transactions: Vec<Transaction>, warnings: Vec<BankRowWarning> }`, `parse_bank_a_text(&str,&str) -> ParseReport`, `parse_bank_a(&Path,&str) -> Result<ParseReport,PdfTextError>`; candidate rows start with `DD.MM.YYYY`; header/summary lines skipped; missing `Bank A AG` on the first line adds a line-0 warning and still parses; amount token = last token before ` EUR` (also tolerate `€`), canonicalized through `money::print(money::parse(token))` into `transaction::from_parts` (never a minor-unit count); provisional `id::transaction_base_id` with NO `#N`; unparsable row/warning ⇒ typed `BankRowWarning`, never a panic; pure function.
**CHANGED (decision I2 supersedes decision 18):** `counterparty` is the EMPTY string — Phase 4 importers recognize no counterparty. Add a `///` comment on the parser saying so and referencing `data-files.md:71` ("as recognized by the importer") and `transaction.rs:60` ("empty when unknown"). Do not copy the description into it.
**Expected ids (verified by sha256 of `date|amount-minor-units|EUR|description||bank_a.pdf`, computed 2026-11-23):** in statement/CSV row order `f694f9c9cf, b9e2c15681, beb2aa2192, a65e62e0a5, 04dfddfd6f, 967d1a6610` for descriptions `REWE MARKT 12`, `DINNER ZUR LEITZ`, `PAYMENT TO TOBIAS`, `BOOKS ONLINE`, `MEGA MART 04`, `LIDL 0815` (amounts -3999, -5990, 42000, -1847, -123456, -12990; dates 2024-01-03/09/15/21/24/28).
**Tests (same names as the plan's T2 section):** `fixture_a_yields_exact_transactions` asserts 6 transactions, the amounts/dates above, descriptions as printed, `counterparty == ""` for every row, and the id list above (order asserted); plus keep the self-consistency assert `row.id == id::transaction_base_id(row)` and no `#` in any id. `broken_variant_warns_and_keeps_rows` (5 txs + 1 warning), `twice_identical`, `two_column_number_stays_on_row` unchanged.
**Success check:** `cargo test -p core parse_bank_a` green + `cargo build --workspace --all-targets` 0 warnings.
**Out of scope:** no store writes, no host, no fixture/`transactions.csv` edits, no counterparty recognition of any kind.

### A2 — replaces T3 (same rule: counterparty empty)
- reason: user decision I2; T3's plan section pins decision 18 which I2 replaces
- replacement brief: identical to the plan's T3 section (interfaces `parse_bank_b_text(&str,&str) -> ParseReport` / `parse_bank_b(path,source)`, reuse `parse_bank_a::{BankRowWarning, ParseReport}`, candidate rows start `YYYY-MM-DD`, card token `^\d{4}(-\d{4}){2}$` consumed and dropped from the description, `;` inside descriptions survives, duplicates kept in order with NO `#N` suffix, warning policy as T2) **except**: `counterparty = ""` (decision I2, not `counterparty == description`).
**Expected ids (verified by sha256 with empty counterparty):** `bank_b1` → `42b62ea5c6, 88a5a3a524, 9f6679bc82, 2b67dab066`; `bank_b2` → `14b233a536, 96c4015370, 96c4015370, 0701248af6` (the parser emits the bare base twice; the `#2` suffix belongs to the store, task T9). Assert `counterparty == ""` on every row, and `id == transaction_base_id(row)` self-consistency.
**Tests keep the plan's names:** `fixture_b1_exact`, `fixture_b2_duplicate_pair_kept` (assert no `#` in any id), `broken_variant_warns_and_keeps_rows`, `twice_identical`.
**Success check:** `cargo test -p core parse_bank_b` green.

### A3 — amends T9's expectations (id source + R002 assertion)
- reason: user decision I2; the plan's T9 section asserted imported ids against `testdata/transactions.csv`, which is un-derivable (fixture counterparties are hand-curated post-import data)
- the amendment changes the expectations only, not the structure of T9:
  - `import_directory_appends_exactly_expected`: `added == 14`; the 14 ids are the pipeline-produced ones (empty counterparty): bank A `f694f9c9cf, b9e2c15681, beb2aa2192, a65e62e0a5, 04dfddfd6f, 967d1a6610`; bank B1 `42b62ea5c6, 88a5a3a524, 9f6679bc82, 2b67dab066`; bank B2 `14b233a536, 96c4015370, 96c4015370#2, 0701248af6` (the `#2` occurrence suffix comes from the store's `id::occurrence_id`, decision 24). Do NOT assert against `testdata/transactions.csv` ids.
  - R002 assertion: no row in the produced `transactions.csv` has `source == mail_paypal.mbox` (or any `.mbox` source) — this replaces "none of the 8 fixture ids appear", which only works if the fixture were treated as import output.
  - The fixture `testdata/transactions.csv` stays byte-unchanged in the whole phase (verify with `git diff testdata/`).

- NEEDS_USER (T2, blocking T3/T8/T9 expectations): the plan's Context claims imported ids equal the `testdata/transactions.csv` ids, but that fixture's counterparty column is hand-curated and un-derivable from the printed statement text, so pinned decision 18 (`counterparty == description`) yields different ids. Options: (A) regenerate the fixture with party==description for the CSV rows (forces edits to committed Phase-2 `id.rs` literals), (B) keep the fixture as a hand-annotated store example and assert the ids the import pipeline actually produces (plan Context's id list retired; no fixture churn), (C) hard-code per-bank counterparty recognition to match the CSV (invented recognition - rejected by both agents). Recommendation: B.
- RESOLVED 2026-11-23: the user asked why counterparty should equal the description at all, and chose option D -> decision I2 (counterparty empty) with amendments A1/A2/A3; the NEEDS_USER is closed.
