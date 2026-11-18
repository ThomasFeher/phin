# Implementation state
Plan: ./01_plan.md (read-only, never edit)
VCS: git | branch: main | base: 3a230dd972673482871d0d0fa3fae1a277d1254c | plan commit: eca894325f52e4a3a22dcd8533a688b4c717b448

## Item state
| id | status | cycles | commit |
|----|--------|--------|-------------|
| 3.1 | DONE | 2 | PENDING (sha written below) |
| 3.2 | TODO | 0 |  |
| 3.3 | TODO | 0 |  |
| 3.4 | TODO | 0 |  |
| 3.5 | TODO | 0 |  |
| 3.6 | TODO | 0 |  |

## Findings log (implementation)
- [developer|c1] production LOC for 3.1 = 439 (codec 192, command 110, event 132, lib 5) vs plan ground rule 2 (~150 production LOC per task) — source: developer c1 handoff
- [developer|c1] codec dispatches `event` before `cmd` (doc puts `cmd` inside events, so `cmd` cannot be the discriminator); a line with both read as an event — source: codec.rs `decode_fields`
- [developer|c1] `confirm-annotation` has no doc test-example; wire form chosen by developer: `{annotation,id}` accept / `{annotation,reject:true}` reject — source: developer c1 handoff
- [developer|c1] `log` event typed as `level`+`text` only; a `cmd` field on log lines would be dropped on read — source: developer c1 handoff
- [developer|c1] `cargo test --workspace` 141 passed / 0 failed; `cargo build --workspace` 0 warnings — source: developer c1
- [reviewer|c1] independently confirmed: 12 `test-example id=` markers; doc-reading test non-vacuous (count cross-check + handshake pair required); every success-check clause asserted; no `panic!`/`as` in production code — source: reviewer c1
- [reviewer|c1] findings 2/3/4 (misleading sorted-keys comment, `from_i128().unwrap()`, silent drop of `id` when both `id`+`reject`) — all addressed in c2 — source: reviewer c1 table
- [reviewer|c1] serde_json `Map` sorts keys lexicographically (no `preserve_order`): frames are value-equal, never byte-equal, to doc examples — source: reviewer c1
- [reviewer|c1] deliberate design to preserve: `Event::Result` verbatim payload, `ErrorReason::Other` lenient fallback, `Framing`/`feed` for 3.6, hello `version` vs line `v` comparison deferred to 3.5 — source: reviewer c1
- [reviewer|c1] `PROTOCOL_VERSION` currently lives in `codec.rs`; 3.5 row expects it in `proto/src/commands.rs` — must be re-exported or moved in 3.2 — source: reviewer c1
- [developer|c2] remaining levers deliberately not taken: grouped imports (repo convention is one-per-line, ~17 LOC) and folding `command_fields`/`command_from_fields`/`command_name` into 3.2's data-driven table (next item's scope) — source: developer c2
- [developer|c2] production LOC 437 -> 301 (codec 112, command 85, event 101, lib 3) after idiomatic compaction; still over ground rule 2's ~150 — source: developer c2 recount
- [developer|c2] locked serde_json 1.0.154 has NO `Number::from_i64`; used total `serde_json::Number::from(i64)` instead of reviewer's suggestion — zero unwrap/panic/`as` in production code — source: developer c2 note + grep
- [developer|c2] new `CodecError.ConflictingFields`; `confirm-annotation` with both `id` and `reject` → typed error (conflict decided by key presence, so `id`+`"reject":false` now → `ConflictingFields`, `reject` alone non-true → `WrongType`) — source: proto/src/command.rs `confirm_annotation`
- [developer|c2] `cargo test --workspace` 142 passed / 0 failed (proto 12); `cargo build --workspace` 0 warnings — source: developer c2
- [reviewer|c2] verdict BLOCKED(user-decision): every functional clause re-verified green (12 markers, 0 warnings, `cargo test -p proto` 12/0, `cargo test --workspace` 142/0, LOC recount 301 = codec 112 + command 85 + event 101 + lib 3, zero panic/unwrap/assert/`as` in production regions); NO further code cycle needed — source: reviewer c2
- [reviewer|c2] serde_json 1.0.154 `impl From<i64> for Number` verified in the locked crate source → `Number::from(i64)` is total, no failure path; no `from_i64` constructor exists — source: serde_json src/number.rs:735-795
- [reviewer|c2] serde_json depth limit means deeply-nested untrusted input → `UnparseableLine`, not stack overflow — source: serde_json de.rs
- [reviewer|c2] `split_lines` uses `split('\n')` so `parts.len()-1` cannot underflow; `""` → no lines + empty pending; stray empty line → `UnparseableLine`, matching doc :203-209 "never resynchronizes" — source: codec.rs:263/274
- [reviewer|c2] minor #2: `codec::read_version` is `pub` but only used internally (make private, or record 3.4 as its public consumer) — source: codec.rs:179
- [reviewer|c2] minor #3: a `result` payload key named `v`/`event` is silently dropped/overwritten on re-encode (decode_fields strips `event`/`v`, frame_of overwrites `v`) → 3.2 must forbid `v`/`event`/`cmd` as payload keys or raise `ConflictingFields` — source: codec.rs:167, event.rs:162
- [reviewer|c2] minor #4: `Framing.partial` is unbounded (a peer streaming without `\n` grows memory without limit) → 3.6's transport must bound the pending line length and drop the connection; codec stays pure — source: codec.rs:275
- [reviewer|c2] note #5: serde_json 1.0.154 pulls in `itoa`, `memchr`, `serde_core`, **`zmij`** (lock-only serde_derive/proc-macro2/quote/syn/unicode-ident); the planning scout had recorded the lock as containing no serde/JSON crate at all → supply-chain surface grew, covered by planning decision 3 (user chose serde_json) — source: `cargo tree -p proto`, Cargo.lock
- [reviewer|c2] note #7: all 11 codec tests live in `codec.rs`; `command.rs`/`event.rs` have no module tests, diverging from repo convention (each module tests itself, e.g. `core/src/domain/*`, `core/src/technical/csv.rs`) — acceptable now, 3.2's tests must live in `commands.rs` — source: reviewer c2
- [reviewer|c2] note #8: `proto/src/lib.rs` was modified though the row's file list omits it (unavoidable to make the new modules reachable) → amend the file list — source: reviewer c2
- [reviewer|c2] `cargo fmt --check` is not clean repo-wide (core 286 diffs, workspace 340) → fmt is not a gate; proto's single-line match arms match repo style — source: cargo fmt --check
- [reviewer|c2] forward obligations: 3.2 must move/re-export `PROTOCOL_VERSION` into `commands.rs`, express `id | reject` exclusivity in the table, make command.rs's three match functions table entries (~85 LOC reducible); 3.3 must reuse `event_fields`+`encode_frame` (lexicographic key order → 7.5 byte comparisons must canonicalize); 3.4 must map every `CodecError` variant to an error `code`; 3.5 owns hello-`version` vs line-`v` cross-check; 3.6 owns `Framing` use + pending-line bound — source: reviewer c2 notes

## Decision log (implementation)
| # | decision | rationale | decider | status |
|---|----------|-----------|---------|--------|
| 12 | 3.1 codec dispatches on `event` first, then `cmd`; `confirm-annotation` wire form `id` / `reject:true` | doc has no test-example for confirm-annotation and uses `cmd` inside events, so discriminator order is forced; developer choice recorded for 3.2's table to confirm | agent | JUSTIFIED |
| 13 | Ground-rule LOC budget (~150 production LOC) exceeded by 3.1: 439 → 301 after c2 compaction; reviewer says no clean split keeps sub-rows under cap (codec+command=197, codec+event=213; shared CodecError/field/framing helpers needed by every part; doc round-trip spans 5 command + 7 event examples) | rule 2 is a project ground rule (plan.md:1-30); splitting vs accepting is a user-visible tradeoff | pending | NEEDS_USER resolved by decision 16 |
| 14 | `Number::from_i64` unavailable in locked serde_json 1.0.154; used total `Number::from(i64)` | developer verified the actual serde_json 1.0.154 API (impl_from_signed!(…i64…) in the locked source); stronger than the finding asked (no unwrap possible) | agent | JUSTIFIED |
| 15 | `confirm-annotation` both-`id`-and-`reject` → new typed `CodecError.ConflictingFields` (conflict by key presence) | doc core-protocol.md:101 `id | reject` implies exclusivity; silent drop broke round-trip fidelity | agent | JUSTIFIED |
| 16 | 3.1 accepted at 301 production LOC with an amendment row recording the deviation from ground rule 2's ~150, instead of splitting the row; 3.1's file list amended to include `proto/src/lib.rs` | reviewer c2 arithmetic: no split keeps a sub-row under the cap while preserving 3.1's own success check; moving the name/field tables into 3.2 would weaken 3.1's check; 3.2 is expected to reduce the count later | user (choice A) | resolved |

## Plan amendments
| id | original row | amended row | rationale |
|----|--------------|-------------|-----------|
| 3.1 | `3.1 | Encode/decode the core protocol as one JSON object per line in `proto` using serde_json: typed `command`/`event` messages, integer `v` on every line, typed errors for unknown commands and unparseable lines. | `proto/src/command.rs`, `proto/src/event.rs`, `proto/src/codec.rs`, `proto/Cargo.toml` (new `serde_json` dep) — all new files (scout: `proto/src/` currently only `lib.rs`) | `cargo test -p proto` green with: unknown command → typed error not panic; unknown field ignored; multi-line input split correctly; unicode preserved; every `test-example` message read from `doc/architecture/core-protocol.md` at test time decodes and round-trips (plan.md:328 "doc-reading tests of 3.1") | — (0.5 done)` | `3.1 | Encode/decode the core protocol as one JSON object per line in `proto` using serde_json: typed `command`/`event` messages, integer `v` on every line, typed errors for unknown commands and unparseable lines. DEVIATION RECORDED: 301 production LOC (codec 112 + command 85 + event 101 + lib 3), over rule 2's ~150 and the plan's 130 estimate — accepted because no split of this row keeps a sub-row under the cap while preserving its own success check (codec+command=197, codec+event=213; shared CodecError/field/framing helpers are needed by every part, and the doc round-trip check spans 5 command + 7 event examples). 3.2 is expected to REDUCE this by replacing command_name/command_fields/command_from_fields (~85 LOC) with its data-driven table. | `proto/src/command.rs`, `proto/src/event.rs`, `proto/src/codec.rs`, `proto/src/lib.rs` (add `pub mod codec; pub mod command; pub mod event;` + package doc), `proto/Cargo.toml` (new `serde_json` dep) | `cargo test -p proto` green with: unknown command → typed error not panic; unknown field ignored; multi-line input split correctly incl. a partial line kept pending; unicode preserved; every `test-example` message read from `doc/architecture/core-protocol.md` at test time decodes and round-trips — PLUS `cargo build --workspace` at 0 warnings, and the three files' production LOC recorded with the composition breakdown (25 `use`, 32 enum-variant lines, ~32 three name/wire-text tables, 12 `error_text`, ~16 framing, rest encode/decode/validate helpers) | — (0.5 done)` | user chose option A after cycle 2: accept 301 with the deviation recorded, plus reviewer c2 finding #8 (lib.rs had to change to make the new modules reachable, so the file list is amended). No protocol surface or check was weakened. |
