# Implementation state
Plan: ./01_plan.md (read-only, never edit)
Plan format: detailed-v1
VCS: git | branch: main | base: 69b57518608cf27b9e42b83848247d3e885be35f | plan commit: 69b57518608cf27b9e42b83848247d3e885be35f

## Item state
| id | status (TODO | DOING | REVIEW | DONE | BLOCKED(user-decision)) | cycles | commit (sha) |
|----|-------------------------------|--------|--------------|
| T1 | DONE | 1 | a5079ba |
| T2 | TODO | 0 | |
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
- [developer] The plan's T1 test 1 expectation is arithmetically wrong: `123456 - 123456 + 100050 - 3999 = 96051`, not `100051` — source: `python3 -c "print(123456-123456+100050-3999)"` → `96051`. Amendment A1 replaces the pinned value; unrelated to T2's fixture total `-38956`.
- [developer] `money_sum`'s defensive branch is not fully pinned by the plan: `Table`'s fields are public so a hand-built table can hold a non-`Money` cell in a `Money` column, or a row shorter than the columns. Developer reuses the pinned variants (`NotMoney(column, kind_of_cell)` / `NoSuchColumn(column)`) rather than inventing a fourth `SumError` variant. Source: `core/src/domain/result.rs`.
- [reviewer] T1 APPROVE, cycle 1, no BLOCKER/MAJOR/MINOR. Independent re-runs: `cargo test -p core result` 16 passed / core lib (`domain::result` 10 of them); `cargo test --workspace` proto 33, core lib 300 (290 + 10 new), core bin 4, import_lifecycle 4, transport_lifecycle 4, cli/tui/gui 1 each, 0 failed; `RUSTFLAGS=-Dwarnings cargo build --workspace --all-targets` zero warnings. Source: reviewer output.
- [reviewer] NIT for the log: the plan's T1 test 3 wording (`NotMoney(1, Kind::Count)`) implies a count column at index 1, the implementation's fixture puts it at index 0 and asserts `NotMoney(0, Kind::Count)` — semantics identical, no change requested. Source: core/src/domain/result.rs (test `money_sum_refuses_a_count_column`).
- [reviewer] NIT carried to T13: `core/src/domain/mod.rs` module doc still lists only the Phase-1 six modules (stale before T1). Source: core/src/domain/mod.rs:1-11.
- [reviewer] NIT for a later task: `Table`/`Cell` have no `Clone` (plan-pinned derives) — T2-T5/T10 must pass tables by value or the plan revisits the derives. Source: core/src/domain/result.rs:66,97.
- [developer] `money_sum` of a hand-built inconsistent table is handled with the pinned variants (`NotMoney(column, kind_of_cell)` / `NoSuchColumn(column)`), no fourth variant, no cell skipped, no `unreachable!()`. Source: core/src/domain/result.rs `money_sum`.
 (the dispatch carried a literal `%(brief)s` placeholder); the developer worked from the plan file's own Context + T1 section, which is the same content. Every later dispatch carries its brief verbatim. Source: this session's dispatch text.

## Decision log (implementation)
| # | decision | rationale | decider (agent|user) | status (JUSTIFIED | NEEDS_USER resolved) |
| 32 | T1 test 1's pinned sum is `96051`, not the plan's `100051` (amendment A1). | Objective arithmetic, not a judgement call: `123456-123456+100050-3999 = 96051` (measured). D-I demands a hand-computed value, so the wrong hand-computation is corrected rather than asked. | agent | JUSTIFIED |

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
