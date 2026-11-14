# Implementation state
Plan: ./01_plan.md (read-only, never edit)
VCS: git | branch: main (never changed by this run) | base: d076a91294493be26a3d4619619d16c7f48e7598 | plan commit: e116299

## Item state
| id | status | cycles | commit |
|----|--------|--------|--------|
| 1.1 | DONE | 1 | 7d9af6e |
| 1.2 | DONE | 3 | bc46de3 |
| 1.3 | DONE | 2 | 4dd0c77 |
| 1.4 | TODO | 0 | — |
| 1.5 | TODO | 0 | — |
| 1.6 | TODO | 0 | — |
| 1.7 | TODO | 0 | — |
| 1.8 | TODO | 0 | — |

## Findings log (implementation)
- [scout] git repo, branch `main`, HEAD at run start `d076a91`; tree clean except untracked `.agents/plans/2026-11-14_phase-1-domain-model/` and untracked `Cargo.lock` — source: git status --short
- [scout] plan dir not hidden by ignore rules (`git check-ignore` exit 1); committed as `e116299 Add Phase 1 plan (domain model)` before any implementation — source: git log
- [scout] commit convention from history: subject `<id>: <title>` (e.g. `0.5: interface docs`), body `<id>: <title> review fixes`; plan-dir commits use `Add Phase N plan (...)` — source: git log --format
- [scout] `core/src/domain/` currently holds only `mod.rs` + `README.md`; purity test greps domain `.rs` files for the literal strings `std::fs` / `std::net` (FORBIDDEN @ core/src/technical/mod.rs:46) — source: ls, core/src/technical/mod.rs
- [scout] `core/src/domain/mod.rs` declares no submodules (only a doc comment + `domain_module_compiles` test), so each new domain file needs a `pub mod <name>;` line — source: core/src/domain/mod.rs
- [developer] 1.1 layout-B disambiguation implemented as: a comma, when present, is always the decimal separator; without a comma, dots are thousands separators iff they form valid 3-digit groups, otherwise the last dot is the decimal separator — so `"1.000"`→100000 and `"-1234.56 EUR"`→-123456 — source: core/src/domain/money.rs (`parse` doc)
- [developer] 1.1 `>2` decimal digits → `Malformed`; `Overflow` reserved for values not fitting i64 minor units (tested at i64.max) — source: core/src/domain/money.rs
- [developer] 1.1 production LOC (non-blank/non-comment up to first `#[test]`) = 157, marginally above the ~150 ground-rule figure, reported not hidden — source: developer report + reviewer count
- [reviewer] 1.1 round-trip asymmetry: `print(i64.min)` = `"-92.233.720.368.547.758,08 €"` but `parse` of it → `Err(Overflow)` (magnitude 2^63 ∉ i64); only unrepresentable value, plan vectors unaffected — source: reviewer harness, core/src/domain/money.rs:88
- [reviewer] 1.1 trailing separators silently accepted: `"1,"`→100, `"1."`→100, `"1.000.000."`→100000000 — source: reviewer harness, core/src/domain/money.rs:33
- [reviewer] 1.1 no panics: slicing at validated 1-byte ASCII boundaries, `ends_with("€")` guards the 3-byte cut, `checked_mul/add/neg` + `0u64.wrapping_sub`, no `unwrap`/`panic!`; 47 adversarial inputs returned typed results, zero panics; no float literals; purity grep clean (exit 1) — source: reviewer harness + grep
- [reviewer] 1.1 `pub mod money;` appended after the existing `#[test] fn domain_module_compiles() {}` in core/src/domain/mod.rs:14 — valid Rust, minor style divergence, not fixed — source: reviewer finding
- [reviewer] 1.1 `cargo test --workspace` green, core lib `10 passed; 0 failed` (6 new money tests + `domain_module_compiles` + `domain_sources_are_pure`); the four plan vectors verified independently — source: reviewer checks
- [developer] 1.2 cycle 1 ended truncated (hit the subagent output-token cap mid-edit): `core/src/domain/date.rs` written (218 lines) but `cargo test --workspace` RED — 19 errors incl. E0308 at date.rs:62/63/64/70/71/72, missing `PartialEq` on `date::ParseError` (tests compare errors with `==`), E0369 `cannot add String to &str`, E0277 `[{integer}]` indexed by `u32`, E0384 `cannot assign twice to immutable value`, E0382 borrow of moved `stored` in round-trip tests — source: cargo test output
- [developer] 1.2 `core/src/domain/mod.rs` already carries the style fix: `pub mod date;` + `pub mod money;` moved above the `domain_module_compiles` test — source: core/src/domain/mod.rs
- [developer] 1.2 cycle 2 reached green: `cargo test --workspace` core lib 16 passed / 0 failed; deviations from the prescribed fix list were required — `#[derive(Debug, Eq, PartialEq)]` (rustc 1.99: `Eq` alone fails E0277, `PartialEq` is its supertrait), `parts.remove(0)` x3 instead of indexing (E0507 cannot move String out of index; length checked first), `print(date: &str) -> String` body `date.to_string()`, digit-width loop over `[&day, &month, &year]`, `let mut numeric`/`let mut parts` — source: developer handoff
- [developer] 1.2 cycle 3 ticked only the 1.2 box in doc/implementation/plan.md (line 72); workspace total 23 passed / 0 failed — source: developer handoff
- [reviewer] 1.2 cycle-2 checks: scratch harness (standalone rustc copy in /tmp) → 30+ assertions OK incl. both fixture formats, `01/02/2024`→Ambiguous, `31.02.2024`→InvalidDate, leap set (2024 ok, 2023/1900 err, 2000 ok), round-trip identity, adversarial inputs typed Err with zero panics; purity grep clean; production LOC 113 ≤ 150; only the 1.2 box ticked — source: reviewer Checks run
- [reviewer] 1.2 REQUEST_CHANGES (medium): `parse` calls `text.trim()` so `"2024-01-03 "` / `" 2024-01-03"` return Ok instead of a typed error; leniency masks PDF extraction noise — source: core/src/domain/date.rs:36, reviewer harness (2 failures)
- [developer] 1.2 cycle 3 removed the `trim()` (parameter renamed `body`, parsed raw), documented strictness on `parse`, added test `whitespace_padded_input_is_malformed` (`"2024-01-03 "`, `" 2024-01-03"`, `"2024 . 01 . 03"`, `""` → Malformed); core lib now 17 passed / 0 failed; production LOC 112 — source: developer handoff
- [reviewer] 1.2 cycle 3 APPROVE: standalone harness ALL assertions passed (formats, Ambiguous, InvalidDate, leap set 2024 ok / 2023+1900 err / 2000 ok, round-trip identity, 10 adversarial inputs typed Err zero panics); purity grep clean, no float literals, LOC ≈112, `///` on all public items; remaining notes low/info only (redundant empty-string guard = dead code; `year_value == 0` → InvalidDate is a defensible proleptic-Gregorian choice) — source: reviewer verdict
- [orchestrator] 1.2 committed `bc46de3 1.2: dates` (date.rs + mod.rs + plan.md 1.2 tick + plan dir); cycles 3/5 — source: git log
- [developer] 1.3 cycle 1: `Transaction` with the 7 contract fields, `#[derive(Debug, Eq, PartialEq)]`, `BuildError` wrapping date/money parse errors (Debug only — money.rs `ParseError` has no Eq), `from_parts` (display-form text → money::parse + date::parse), `is_income()` = `amount > 0`, 6 inline tests; core lib 23 passed / 0 failed; production section 101 lines — source: developer handoff
- [reviewer] 1.3 cycle 1 REQUEST_CHANGES: (medium) `from_parts` doc cited plan items 2.1/2.2 for the store reader, but 2.1 is the generic `;` line codec and 2.2 the id hash — the item building `Transaction` from stored `transactions.csv` lines is 2.3, and `money::parse("-3999")` → `-399900` proves the double-scaling trap; (medium) `from_parts` was the only constructor and takes display-form text, so the store reader would build `Transaction { ... }` literals bypassing validation; (low) explicit-self `is_income(self: &Transaction)`; (low) 7 positional `&str` params transpose silently with no documented column order; (low, report-only) `MoneyParseError` in money.rs:13 derives only `Debug` while `DateParseError` derives `Eq, PartialEq` — inconsistent, later items (1.6/1.8/2.x) will need `Eq` on error enums — source: reviewer findings
- [developer] 1.3 cycle 2 fixed both mediums: corrected citation to 2.3 with an explicit INVARIANT rule (stored minor-unit text must never go through `from_parts`, `-3999` → `-399900` example), added `from_stored(id, date, amount: i64, ...)` taking the stored minor-unit count as-is with a canonical `YYYY-MM-DD` round-trip check (new `BuildError.NonCanonicalDate` variant) and made `from_parts` delegate to it; `is_income(&self)`; documented exact contract order at the signature; 4 new tests (raw fixture rows incl. `80c63211a8#2`, non-canonical date rejection, `from_parts` == `from_stored`, description/counterparty swap inequality); core lib 27 passed / 0 failed; production LOC 147 (55 excluding doc comments) — source: developer handoff
- [reviewer] 1.3 cycle 2 APPROVE: both mediums resolved, all 7 fields named per contract and `///`-documented, no `category` field, `is_income` correct for +/−/0 and tested, structural Eq verified, no panic path (all failures typed `BuildError`, unwraps only in tests), no scope creep, purity grep clean, no float literals, production LOC 55 excluding doc comments; remaining low finding NOT fixed: doc comments cite the store reader as `core/src/domain/store_transactions.rs` but plan item 2.3 puts it at `core/src/technical/store_transactions.rs` — carry into the item-2.x plan — source: reviewer verdict
- [orchestrator] 1.3 committed `4dd0c77 1.3: transaction type` (transaction.rs + mod.rs + plan.md 1.3 tick + plan dir); cycles 2/5 — source: git log

## Decision log (implementation)
| # | decision | rationale | decider | status |
| 12 | Handling of the pre-existing untracked `Cargo.lock` outside the plan dir | tree dirty outside the plan dir before item 1; commit rules forbid sweeping foreign files into an item commit on my own judgement, so the user decides: commit, ignore, or leave it | user | NEEDS_USER resolved ("A": leave untracked, never stage it into an item commit) |
| 13 | 1.1 amount disambiguation: comma = decimal; without a comma, dots are thousands separators iff valid 3-digit groups, else last dot = decimal | must satisfy both plan vector `"1.000"`→100000 and fixture layout B `"-1234.56 EUR"`→-123456; no spec existed (planning finding: no amount-format spec in interface docs) | developer | JUSTIFIED |
| 14 | 1.1 `>2` decimal digits → `Malformed`, `Overflow` only for i64-fit failures | keeps the typed-error taxonomy small and testable at i64.max | developer | JUSTIFIED |
| 15 | 1.1 accepted at APPROVE with 4 low-severity reviewer notes left unfixed (trailing separators, i64.min round-trip, 157 LOC, mod.rs declaration order) | reviewer verdict was APPROVE and none of the notes touch a plan vector or a repo invariant; protocol only mandates fix cycles on REQUEST_CHANGES — logged here so later phases can revisit | orchestrator | JUSTIFIED |
| 16 | 1.2 `parse` is strict about surrounding whitespace (no `trim()`; padded input → `Malformed`) | reviewer REQUEST_CHANGES cycle 2: leniency masks PDF text-extraction noise; the technical/importer layer owns whitespace stripping, the domain stays strict | reviewer→developer (decided by reviewer finding) | JUSTIFIED |
| 17 | 1.2 year 0 (`0000-00-00`) → `InvalidDate` | reviewer info note: defensible proleptic-Gregorian choice, consistent with the 4-digit canonical range; left as implemented | developer | JUSTIFIED |
| 18 | 1.3 two constructors: `from_parts` (statement-printed display text) delegating to `from_stored` (stored i64 minor units + canonical-date check) | reviewer cycle-1 medium: with only a display-form constructor, the store reader (plan item 2.3) would build struct literals and bypass validation, and `money::parse("-3999")` → `-399900` double-scales stored values | reviewer→developer (decided by reviewer finding) | JUSTIFIED |
| 19 | 1.3 `BuildError` derives only `Debug` (error tests assert via `match`, not `==`) | adding `Eq` would require re-touching committed money.rs (`MoneyParseError` has no `Eq`), which is out of 1.3's scope; logged so a later item that needs error equality adds the derive to money.rs | developer | JUSTIFIED |
| 20 | 1.3 `is_income(&self)` (not `self: &Transaction`) | reviewer low style note: the domain layer uses no explicit-self forms | developer | JUSTIFIED |

## Plan amendments
| id | original row | amended row | rationale |
|----|--------------|-------------|-----------|
