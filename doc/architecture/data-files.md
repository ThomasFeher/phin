# Data Files

The **Data File Interface**: everything the user can see and edit. The data
directory (default: the current directory, configurable via the TOML
configuration) holds all state; deleting it loses everything, copying it to
another machine is a full backup. All files are plain text, human editable,
diff-able. Restates the accepted decisions "File based storage" (append-only
variant) and "Content-hash transaction ids".

## CSV codec (all `.csv` files)

Draft, not yet accepted (fixture-backed parts marked): the rules below are
fixed here for the first time; only the separator and the `# columns:` header
comment appear in the `testdata/` fixtures of plan item 0.3.

| Rule | Detail |
|------|--------|
| Separator | `;` (semicolon) — **fixture-backed**. |
| Comments | Lines starting with `#` are comments; also the header preamble. Empty lines ignored (draft). |
| Quoting | Fields are quoted with `"` when they contain `;`, `"` or leading/trailing spaces; `"` inside a quoted field is doubled (`""`) (draft). |
| Newlines | A field must never contain a raw newline; a record split over two lines is a hard error (draft). |
| Line endings | CRLF tolerated on read, LF written on write (draft). |
| Empty columns | Optional columns may be empty (empty string between separators) (draft). |
| Header | There is **no** header line; the column order is fixed by this spec and documented in a `# columns:` comment at the top of each file — **fixture-backed**. |

## Transaction id format

- `id` = short content hash over the normalized fields
  (`date`, `amount`, `currency`, `description`, `counterparty`, source
  identity), written as **lowercase hexadecimal** in the first column of
  every file and used as the join key in every protocol message.
- Hash length is fixed per implementation (the fixtures use 10 hex digits,
  e.g. `39945b2fed`). Draft, not yet accepted: the exact length and hash
  function are chosen in plan item 2.2.
- Identical content gets an **occurrence suffix**: the first occurrence keeps
  the bare hash, the second gets `<hash>#2`, the third `<hash>#3`.
- Consequences (accepted): re-importing the same statement from a different
  path yields the same id and deduplicates; a hand-edited transaction line
  silently becomes a *different* transaction, orphaning its overrides and
  annotations — the core lists orphans and reports them. Refine only through
  overrides and annotations, never by editing a transaction line.

## Append-only rule

- `transactions.csv` holds **facts and is only ever appended to**; existing
  lines are never rewritten, re-sorted or deleted by the core. Refinements
  (annotations, overrides) live in side files keyed by transaction id.
- `annotations.csv` and `overrides.csv` are append-only as well: a
  superseded entry stays in the file, the **last** entry for an id wins.
- `categories.csv` and `rules/*` are directly editable (rewritten by
  hand; only `rules/90-learned.txt` is appended by the core).
- Every write is atomic: write a temporary file, rename it into place. A
  crash therefore never leaves a half-written file, and a cancelled command
  discards its partial result.
- Idempotence: before appending, the core skips candidates whose id is
  already known.

## Files

### `transactions.csv` — facts, never edited in place

Columns: `id;date;amount;currency;description;counterparty;source`

| Column | Type | Meaning |
|--------|------|---------|
| `id` | hex + optional `#N` | Content-hash transaction id, join key. |
| `date` | `YYYY-MM-DD` | Normalized date. |
| `amount` | `i64` minor units | Integer cents; negative = expense, positive = income. Never floating point. |
| `currency` | ISO-4217 code | Carried as a field although only one settlement currency is supported (provisional decision "Single currency"). |
| `description` | text | Merchant/reference text as printed on the statement. |
| `counterparty` | text | Counterparty name as recognized by the importer. |
| `source` | text | Provenance: the file the transaction was extracted from. |

The category of a transaction is **not** a column: classification is derived
on every read from `rules/` plus `overrides.csv`.

### `annotations.csv` — information from other sources (mail)

Columns: `id;ref;text;items;source;confidence`

| Column | Type | Meaning |
|--------|------|---------|
| `id` | hex + optional `#N` | Annotation id, its own content hash. |
| `ref` | transaction `id` or empty | Linked transaction; empty = orphan/pending, not linked. |
| `text` | text | The annotation as extracted from the mail. |
| `items` | text | Purchased items, `|`-separated; may be empty. |
| `source` | text | Provenance: the mbox file (and folder) it came from. |
| `confidence` | `0…1` decimal | Confidence of the automatic link; low-confidence links stay pending for the user to confirm or reject. |

Mail data never creates a transaction (R002). Draft, not yet accepted: the
column set and the `|` separator inside `items` are fixed here.

### `overrides.csv` — user-made classification of one transaction

Columns: `id;category;reason`

| Column | Type | Meaning |
|--------|------|---------|
| `id` | transaction `id` | The transaction this override fixes. |
| `category` | `parent:child` | Category path in the tree. |
| `reason` | text | Why the user chose it (shown in listings). |

Precedence: **user override > learned rule > built-in rule**. An override
for an unknown id is stored and reported as an orphan on read. Draft, not yet
accepted: the `overrides.csv` column set is fixed here; architecture.md only
requires that the user can override the classification of one transaction.

### `categories.csv` — the category tree

Columns: `parent;name`

| Column | Type | Meaning |
|--------|------|---------|
| `parent` | category name or empty | Parent node; empty = root node. |
| `name` | text | Node name; the full path is `parent:child` (e.g. `food:supermarket`). |

Order in the file is the display order and is preserved.

### `rules/*` — one rule per line

- Extension convention: user files use the `.rules` extension, while the core-appended learned-rule file is `rules/90-learned.txt`.
- Syntax: `pattern -> category`, `#` starts a comment, empty lines ignored.
  The separator is the ASCII `->`; the unicode arrow `→` is NOT accepted.
- Pattern markers on the pattern side: a leading `words:` marks a comma-separated
  word list, a leading `regex:` marks a Rust `regex` pattern, no marker means
  literal text.
- Precedence: a LATER rule beats an EARLIER one within a file. Files are read
  in the order given by a NUMERIC FILENAME PREFIX (`rules/10-built-in.rules`,
  `rules/50-user.rules`, `rules/90-learned.txt`, names sorted lexically):
  built-in rules first, user files in between, learned rules last. Learned
  rules therefore sit in the highest-prefix file and stay distinguishable from
  hand-written ones.
- Learned rules go to the highest-prefix file so that hand-written and generated
  rules stay distinguishable; learned rules are shown to the user and can be
  disabled.
- Draft, not yet accepted: the fixtures still use a single `rules/sample.rules`
  without a numeric prefix.

### `state/` — machine state, not user data

| Path | Meaning |
|------|---------|
| `state/core.lock` | Lock file of the running core daemon: the address it listens on plus its nonce (see `core-protocol.md`). Written at daemon start-up, removed on exit. |
| `state/markers.csv` | Import markers, one record per source file so the mail importer can resume incrementally. Uses the CSV codec above; columns `path;mtime;size;offset`, documented in a `# columns:` comment. A corrupt marker file is treated as empty, i.e. everything is reprocessed. Fixed here (decision 8, plan item 2.7); the deferral to plan item 4.5 is superseded. |

### Configuration (TOML)

Keys: which interface, which mail folders to scan, which plugins, which
converters, `core.address`, `core.port`, `core.idle_timeout` (idle timeout
of the core daemon, in whole seconds; absent means 900, the 15 minutes
without commands of `core-protocol.md`), data directory (default: current
directory). Draft, not yet accepted: the file name and location
(`phin.toml` in the data directory) are fixed here.

## Fixture note

The `testdata/` fixtures of plan item 0.3 use the columns documented above
(`transactions.csv`, `categories.csv`, `rules/sample.rules`) but are
**provisional**: if a fixture and this file disagree, this file is the
contract and the fixtures get fixed.
