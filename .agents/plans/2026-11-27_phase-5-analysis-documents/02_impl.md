# Implementation state
Plan: ./01_plan.md (read-only, never edit)
Plan format: detailed-v1
VCS: git | branch: main | base: 69b57518608cf27b9e42b83848247d3e885be35f | plan commit: 69b57518608cf27b9e42b83848247d3e885be35f

## Item state
| id | status (TODO | DOING | REVIEW | DONE | BLOCKED(user-decision)) | cycles | commit (sha) |
|----|-------------------------------|--------|--------------|
| T1 | DONE | 1 | 7cd0699 |
| T2 | DONE | 1 | |
| T3 | TODO | 0 | |
| T4 | TODO | 0 | |
| T5 | TODO | 0 | |
| T6 | TODO | 0 | |
| T7 | TODO | 0 | |
| T8 | TODO | 0 | |
| T9 | TODO | 0 | |
| T10 | TODO | 0 | |
| T11 | TODO | 0 | |
| T12 | TODO | 0 | |
| T13 | TODO | 0 | |

## Findings log (implementation)
- [reviewer] T2 APPROVE, cycle 1, no BLOCKER/MAJOR. Re-runs: `cargo test -p core analyse_categories` 8 passed / 0 failed; `cargo test --workspace` proto 33, core lib 308 (300 + 8 new), core bin 4, import_lifecycle 4, transport_lifecycle 4, cli/tui/gui 1 each, 0 failed; `RUSTFLAGS=-Dwarnings cargo build --workspace --all-targets` 0 warnings; no `test-scratch-*` left; two repeat runs stable. Source: reviewer output.
- [reviewer] Independent python3 re-derivation reproduces T2's pinned table exactly (food 0/-40015, food:supermarket -32435, food:restaurant -7580, finance 139000/-5499/133501, finance:income 139000, finance:expense -5499, finance:expense:transport -5499, leisure -9435, leisure:books -2646, leisure:games -6789, unknown 6948/-129955/-123007); roots+unknown net = -38956; test 2's no-categories totals (145948, -184904, -38956); `finance:expense:utilities` omitted. Source: reviewer python3 run; corroborated by [orchestrator]'s own rule parse.
- [developer/reviewer/orchestrator] PLAN FACT CORRECTION (A2): the sample rules produce **6** distinct categories, not 7 — no rule writes plain `finance:expense`; the plan's list conflated the roll-up row `finance:expense` (test 1 row 6, which exists because `transport` rolls up into it) with rule output. Verified three ways: `testdata/rules/sample.rules` read directly (10 rules), the [orchestrator]'s parsed rule list (`['finance:expense:transport', 'finance:income', 'food:restaurant', 'food:supermarket', 'leisure:books', 'leisure:games']`), and the reviewer's re-classification. `unknown_category_paths` therefore reports those 6 sorted. Source: testdata/rules/sample.rules; 02_impl.md amendment A2.
- [developer/reviewer] PLAN CONTRADICTION (logged, not a deviation): the plan's pinned overflow convention — three `Cell::Text("overflow")` in `Kind::Money` columns — cannot pass through `result::add_row`, which rejects a kind mismatch by design. The only available mechanism is appending the marker row to `Table.rows` directly (fields are public by T1's design), and the consequence is that `money_sum` on such a column refuses with `NotMoney(column, Text)` — the intended T12 warning signal. T1's own docs already anticipated marker rows (`Kind::Text`, `Cell::Text`, `money_sum`'s hand-built-table doc); its module doc phrase "rows enter a Table only through `add_row`" needs a clause (carried to T13). The overflow path currently has **no test** — carried to T12 (pin an end-to-end overflow test there). Source: core/src/technical/analyse_categories.rs:337-365; core/src/domain/result.rs module doc.
- [reviewer] PLAN GAP (MINOR, not blocking T2): if a user's `categories.csv` contains a valid path literally named `unknown`, the pseudo-category bucket collides with it — the tree walk emits a row for it AND the grouped scan emits a second `unknown` row, double counting. Artifact of the pinned design (label + single bucket), not a deviation. Guard ("skip the grouped row if the walk already emitted it") or document the limitation. Source: core/src/technical/analyse_categories.rs:196-201 vs :253-260.
- [reviewer] `category_tree_at`'s `BuildError` arm builds its empty tree with an `if`-guarded `from_csv(""").unwrap()` (`Tree` has no `Default`/`Clone`, `from_csv` is its only constructor, so no non-panicking alternative exists in T2's file scope; the panic is provably unreachable — `category.rs:150-175`). The guard shape differs from the cited `state.rs:122-127` precedent, which returns before unwrapping. Accepted as MINOR; a `Tree: Default` derive would remove it later. Source: core/src/technical/analyse_categories.rs:108-115.
- [reviewer] NIT for T13: `match add_row(table, row) { Ok(()) => {}, Err(_) => {} }` swallows an impossible error (commented); `debug_assert_eq!` would be crisper. Source: core/src/technical/analyse_categories.rs:360-363.
- [reviewer] NIT for T13 / literal-fidelity: T2 test 1 omits the plan's in-test assertion that rows 1/4/8/11 net-sum to -38956 — the identical assertion lives in test 8, so the pin is semantically covered. Source: core/src/technical/analyse_categories.rs:575-590.
- [orchestrator] T13 must use the corrected 6-path `unknown_category_paths` value anywhere it is quoted; T12's warning path must quote it too. Source: amendment A2.
- [developer] The plan's T1 test 1 expectation is arithmetically wrong: `123456 - 123456 + 100050 - 3999 = 96051`, not `100051` — source: `python3 -c "print(123456-123456+100050-3999)"` → `96051`. Amendment A1 replaces the pinned value; unrelated to T2's fixture total `-38956`.
- [developer] `money_sum`'s defensive branch is not fully pinned by the plan: `Table`'s fields are public so a hand-built table can hold a non-`Money` cell in a `Money` column, or a row shorter than the columns. Developer reuses the pinned variants (`NotMoney(column, kind_of_cell)` / `NoSuchColumn(column)`) rather than inventing a fourth `SumError` variant. Source: `core/src/domain/result.rs`.
- [reviewer] T1 APPROVE, cycle 1, no BLOCKER/MAJOR/MINOR. Independent re-runs: `cargo test -p core result` 16 passed / core lib (`domain::result` 10 of them); `cargo test --workspace` proto 33, core lib 300 (290 + 10 new), core bin 4, import_lifecycle 4, transport_lifecycle 4, cli/tui/gui 1 each, 0 failed; `RUSTFLAGS=-Dwarnings cargo build --workspace --all-targets` zero warnings. Source: reviewer output.
- [reviewer] NIT for the log: the plan's T1 test 3 wording (`NotMoney(1, Kind::Count)`) implies a count column at index 1, the implementation's fixture puts it at index 0 and asserts `NotMoney(0, Kind::Count)` — semantics identical, no change requested. Source: core/src/domain/result.rs (test `money_sum_refuses_a_count_column`).
- [reviewer] NIT carried to T13: `core/src/domain/mod.rs` module doc still lists only the Phase-1 six modules (stale before T1). Source: core/src/domain/mod.rs:1-11.
- [reviewer] NIT for a later task: `Table`/`Cell` have no `Clone` (plan-pinned derives) — T2-T5/T10 must pass tables by value or the plan revisits the derives. Source: core/src/domain/result.rs:66,97.
- [developer] `money_sum` of a hand-built inconsistent table is handled with the pinned variants (`NotMoney(column, kind_of_cell)` / `NoSuchColumn(column)`), no fourth variant, no cell skipped, no `unreachable!()`. Source: core/src/domain/result.rs `money_sum`.
- [orchestrator] The T1 developer brief was NOT delivered verbatim (the dispatch carried a literal `%(brief)s` placeholder); the developer worked from the plan file's own Context + T1 section, which is the same content. Every later dispatch carries its brief inline or by exact plan line ranges. Source: this session's dispatch text.

## Decision log (implementation)
| # | decision | rationale | decider (agent|user) | status (JUSTIFIED | NEEDS_USER resolved) |
| 32 | T1 test 1's pinned sum is `96051`, not the plan's `100051` (amendment A1). | Objective arithmetic, not a judgement call: `123456-123456+100050-3999 = 96051` (measured). D-I demands a hand-computed value, so the wrong hand-computation is corrected rather than asked. | agent | JUSTIFIED |
| 33 | T2 test 2's pinned `unknown_category_paths` list is the 6 sorted rule categories, not the plan's 7 including `finance:expense` (amendment A2). | Fixture fact, verified three ways (direct read of `testdata/rules/sample.rules`, parsed rule list, reviewer re-classification): no rule writes plain `finance:expense`. Same class as decision 32 — objective, not a judgement the user owes. | agent | JUSTIFIED |
| 34 | The plan's overflow marker row (three `Cell::Text("overflow")` in Money columns) is appended to `Table.rows` directly, bypassing `add_row`, and makes `money_sum` refuse that column with `NotMoney`. | The pinned convention admits no other mechanism under T1's API (`KindMismatch` would reject it) and `money_sum` → `NotMoney` is the designed downstream warning signal; T1's docs anticipated marker rows. Reviewer: no user decision needed. Overflow end-to-end test is pinned into T12. | agent | JUSTIFIED |

## Plan amendments

### A1 — replaces T1 (single pinned value; all other T1 fields unchanged)
- reason: [developer finding, confirmed by the reviewer] the plan's T1 test 1 pins a sum that is
  arithmetically wrong. Measured: `python3 -c "print(123456-123456+100050-3999)"` → `96051`.
  Decision 32: objective arithmetic, corrected not asked (D-I demands a hand-computed value, so the
  wrong hand-computation is fixed rather than raised as a user decision). Independent of T2's fixture
  total `-38956`, which stays as pinned.
- replacement brief (only the affected acceptance criterion, same field as 01_plan.md):
  1. `money_column_sums_exactly_no_float` — a `Money` column `[123456, -123456, 100050, -3999]` sums to
     `96051`.
- status: implemented and approved in T1 cycle 1.

### A2 — replaces T2 test 2's pinned `unknown_category_paths` list (all other T2 fields unchanged)
- reason: [developer finding, confirmed by the reviewer and by the orchestrator] the plan pins 7 paths
  including `finance:expense`, but the sample rules write only 6 distinct categories — no rule writes
  plain `finance:expense`; the plan conflated the roll-up row (test 1 row 6) with rule output.
- replacement brief (only the affected assertion, same field as 01_plan.md):
  2. `unknown_categories_are_grouped_and_reported` — same store, `categories.csv` deleted → exactly one row
   named `unknown` with `(145948, -184904, -38956)`; `unknown_category_paths` returns
   `["finance:expense:transport", "finance:income", "food:restaurant", "food:supermarket",
   "leisure:books", "leisure:games"]` sorted (the 6 distinct paths the sample rules produce).
- status: implemented and approved in T2 cycle 1.

