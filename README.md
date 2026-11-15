# Phin

## Description

Phin is an accounting software for personal finance management. It imports
bank statements from PDF files and mail notifications from mbox files, then
stores the resulting transactions in plain, human-editable CSV files. Categories
are derived on every read from rule files and per-transaction overrides, never
stored as a fixed enumeration in code. A single core daemon holds the pipeline
and talks to user interfaces over a JSON-Lines protocol on a local TCP address.
Everything is idempotent: the data directory is the full state, and re-running
an import produces the same result.

## Interfaces

There are three user interfaces plus one core daemon:

- **CLI** (`cli/`) — command-line client.
- **TUI** (`tui/`) — terminal user interface.
- **GUI** (`gui/`) — graphical user interface.
- **Core daemon** (`core/`) — the engine; interfaces are thin clients that
  speak the Core Protocol (JSON-Lines commands in, events out) over a local
  TCP stream on `127.0.0.1`, default port `47623` (configurable via
  `core.address` / `core.port`).

## Build

```sh
cargo build --workspace
```

The workspace contains the packages `proto`, `core`, `cli`, `tui` and `gui`.

## Running the daemon + CLI

Both are still stubs from the scaffolding phase:

- `core` (the daemon) currently reads lines from stdin instead of listening on
  TCP: lines starting with `echo ` are echoed back, all other lines are ignored.
  The real Core Protocol replaces this in phase 2.
- `cli` is an empty `main()` placeholder.

To try the daemon interactively:

```sh
echo 'echo ping' | ./target/debug/core   # prints: ping
```

## Data files

All state lives in the data directory (default: the current working directory,
configurable): `transactions.csv`, `annotations.csv`, `overrides.csv`,
`categories.csv`, `rules/*.txt` and `state/` (import markers, `core.lock`).
Storage is append-only CSV; deleting the data directory loses everything,
copying it to another machine is a full backup.

Test fixtures live in `testdata/` (transactions/categories CSVs, sample rules,
bank-statement PDFs, mbox files). They are generated, never edited by hand:

```sh
python3 testdata/generate.py [target-dir]   # defaults to testdata/
```

See `testdata/README.md` for what each fixture simulates.

## Docs

- `doc/architecture/architecture.md` — architecture and design decisions.
- `doc/architecture/{core-protocol,plugin-protocol,data-files}.md` — interface
  documentation (Core Protocol, Plugin Protocol, Data File Interface); part of
  the architecture, source of truth for the protocols and the data files.
- `doc/implementation/plan.md` — implementation plan (ground rules).
