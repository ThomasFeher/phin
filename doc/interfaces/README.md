# Phin Interface Documentation

This directory is the **source of truth** for the two wire protocols and for
the on-disk data files. Where `doc/architecture/architecture.md` records an
accepted decision, these files restate it; where a detail was not decided
there, the text marks it as **draft, not yet accepted**. If code and these
files disagree, these files win (see ground rule 6 in the implementation plan:
a task that changes a protocol or file format must update these files).

## Contents

| File | Covers |
|------|--------|
| [`core-protocol.md`](core-protocol.md) | The Core Protocol: transport, lock file, handshake, commands, events, `cancel`, versioning, JSON-Lines examples. |
| [`plugin-protocol.md`](plugin-protocol.md) | The Plugin Protocol: `probe`, `extract`, `analyse` for external importer and analysis processes. |
| [`data-files.md`](data-files.md) | The Data File Interface: every file, every column, the append-only rule, the transaction id format, the CSV codec. |

## Scope

- Both protocols use exactly one interchange format: **JSON-Lines** — one
  JSON object per line, `\n` terminated, UTF-8, no embedded raw newlines.
  Only the vocabulary differs between the two (architecture.md,
  "One Interchange Format").
- Amounts are always integer minor units (`i64`), never floating point
  (architecture.md, "Money").
- Categories are written as `parent:child` paths into the category tree
  (architecture.md, "Category Tree").
- Every JSON example in these files sits in a fenced block preceded by an
  HTML marker `<!-- test-example id=… -->` so that the doc-reading tests of
  plan item 3.1 can extract them.
