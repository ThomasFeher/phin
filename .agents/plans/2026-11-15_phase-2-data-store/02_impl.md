# Implementation state
Plan: ./01_plan.md (read-only, never edit)
VCS: git | branch: main (never changed by me) | base: 33308d525f2bac867aaec664d29edf5e34aaf640 | plan commit: dbe8ca98d8677acd7229d32797c41c2a75217de9

## Item state
| id | status | cycles | commit |
|----|--------|--------|--------|
| 2.1 | DONE | 1 | ae2782979fde4e470c65b227528c5c68d26e633d |
| 2.2 | DONE | 2 | pending |
| 2.3 | TODO | 0 | – |
| 2.4 | TODO | 0 | – |
| 2.5 | TODO | 0 | – |
| 2.6 | TODO | 0 | – |
| 2.7 | TODO | 0 | – |

## Findings log (implementation)
- [scout] tree dirty outside plan dir before item 1: `Cargo.lock` untracked, not matched by `.gitignore` (which lists only `/target`, `*.bak`); it is cargo-generated (`version = 4`, header comment) — source: git status --short / cat .gitignore / head Cargo.lock
- [scout] commit convention in this repo: `<id>: <lowercase intent>` for plan items (e.g. `1.7: annotation + confidence`), plus `plan:` prefix commits for plan dirs; body carries `Success check:` and `Cycles:` paragraphs; item commits tick the `- [ ]` box in doc/implementation/plan.md — source: git log --oneline / git log 64938f2 / doc/implementation/plan.md:28 (rule 7)
- [developer] 2.1: codec implemented as parse_record/render_record + read_document/write_document + records_of/comments_of, `CodecError` typed enum; 9 inline tests; `pub mod csv;` needed in core/src/technical/mod.rs (third file beyond the plan row's two paths, required to compile) — source: core/src/technical/csv.rs, core/src/technical/mod.rs:8
- [developer] 2.1 consequence of the contract made explicit: a record whose FIRST field starts with `#` is force-quoted by render_record, otherwise it would render as a comment and be lost on re-read — source: core/src/technical/csv.rs (render_record doc)
- [reviewer] 2.1 APPROVE, cycle 1; low: single-field record `[""]` renders blank and is dropped by read (unreachable: all real column sets have >=2 columns, `["",""]` -> `";"` round-trips fine); low: write_document emits `Line::Comment` text verbatim without checking for embedded newlines (only reachable for hand-built comments) — source: core/src/technical/csv.rs:118, :251
- [reviewer] 2.1 read ignores whitespace-only lines (space/tab), slightly broader than the draft wording "Empty lines ignored"; fixtures contain none, byte-identical round trip unaffected — source: core/src/technical/csv.rs:341 (is_blank)
- [reviewer] 2.1 verified against fixtures: testdata/transactions.csv (22 records + 4 comments) and testdata/categories.csv (10 records + 2 comments) round-trip byte-identically through the codec; core lib went 79 -> 88 tests — source: cargo test --workspace output
- [developer] 2.2 cycle 1 handoff TRUNCATED by the harness (emitted only a partial sentence); files touched: new core/src/technical/id.rs, core/Cargo.toml (+`sha2 = "0.11.1"` :7), core/src/technical/mod.rs (+`pub mod id;` :9), Cargo.lock regenerated — source: git status --short
- [reviewer] 2.2 cycle 1 REQUEST_CHANGES: 6 compile blockers (E0308 String-> &str at id.rs:90, :97, :232, :250; E0507 move out of Vec at :176; E0382 borrow-after-move at :174/:186) — `cargo build`/`cargo test --workspace` exit 101, no test ran — source: cargo output quoted by reviewer
- [reviewer] 2.2 medium: `base_id` hardcodes the `|EUR|` literal and `transaction_base_id` ignores `tx.currency`, so a USD Transaction hashes as EUR and collides with its EUR twin; data-files.md:26-30 lists currency among the hashed fields — source: core/src/technical/id.rs:78-91, :96-98
- [reviewer] 2.2 low: `base_id_of_id`/`matches_base` private but item 2.3's reader is the consumer that must strip `#N`; `source_basename` also cuts on `\` (POSIX filenames may contain backslash) and yields "" for `dir/`; `occurrence_id` derives next suffix from a COUNT of matching ids, not max(N)+1, so a hand-edited store (`base`, `base#5`) yields `base#3`; 2 unused_parens warnings; 208 non-comment LOC vs plan estimate 90 — source: core/src/technical/id.rs:245-257, :45-66, :104-122, :153-154
- [reviewer] 2.2 cycle 1 algorithm verified correct despite the compile failure: python3 `hashlib.sha256(f"{date}|{amt}|EUR|{desc}|{party}|{src}").hexdigest()[:10]` over all 22 fixture rows matches all 22 stored ids incl. `80c63211a8` (line 16) and `80c63211a8#2` (line 17); digest vectors ("", "abc", "ä") match hashlib — source: reviewer python check vs testdata/transactions.csv
- [developer] 2.2 cycle 2: currency moved into the hashed payload (`base_id(date, amount, currency, description, counterparty, source)`, `transaction_base_id` passes `&tx.currency`) — no hardcoded `|EUR|`; `occurrence_id` now max(N)+1 via new `pub occurrence_number`; `base_id_of_id`/`matches_base`/`occurrence_number`/`transaction_base_id` made pub for item 2.3; `source_basename` strips trailing separators (never an empty basename), backslash precondition documented; 106 non-test LOC vs estimate 90; core lib 88 -> 94 tests — source: core/src/technical/id.rs
- [reviewer] 2.2 cycle 2 APPROVE: `cargo build --workspace` exit 0 / 0 warnings, `cargo test --workspace` exit 0 core lib 94 passed 0 failed, all 6 new id tests pass (`fixture_ids_are_reproduced_from_row_content` asserts records[11][0]=="80c63211a8" and records[12][0]=="80c63211a8#2", `same_transaction_with_different_source_path_keeps_the_id`, `id_hash_length_is_stable_at_ten_hex_chars`, `hand_edited_field_changes_the_id`, `repeated_content_gets_increasing_occurrence_suffixes`, `digest_matches_python_hashlib_on_known_vectors`); independent python hashlib re-check over 22 rows ALL MATCH incl. lines 16/17 — source: reviewer cargo + python output
- [reviewer] 2.2 notes carried forward: currency-in-payload matches data-files.md:25-33 (generator's hardcoded EUR is a fixture special case, all 22 rows EUR); max(N)+1 vs generate.py count-based agree on generated output; `find_testdata_dir` walks up from cwd (cargo test cwd is `core/`) — an unrelated ancestor `testdata/` could shadow the fixture; `source_basename` pub is broader than needed — source: core/src/technical/id.rs:34, :44, :246
- [scout] commit convention in this repo: `<id>: <lowercase intent>` for plan items (e.g. `1.7: annotation + confidence`), plus `plan:` prefix commits for plan dirs — source: git log --oneline

## Decision log (implementation)
| # | decision | rationale | decider (agent|user) | status (JUSTIFIED | NEEDS_USER resolved) |
| 12 | pre-run dirty tree: `Cargo.lock` untracked outside the plan dir — how to handle (commit / gitignore / leave it) | skill rule: never sweep foreign pre-existing changes into an item commit on my own judgement; must ask | user | NEEDS_USER resolved: user chose (c) standalone commit before item 1 -> committed as `bc23d2026fceab7226ed6cbe70a19dd16251875a` (`chore: track Cargo.lock (pre-run dirty tree, user decision c)`) |

## Plan amendments
| id | original row | amended row | rationale |
|----|--------------|-------------|-----------|
