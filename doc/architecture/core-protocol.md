# Core Protocol

Between an **interface process** (CLI, TUI, GUI) and the **core daemon**.
Commands in, events out. Restates the accepted decisions of
`doc/architecture/architecture.md` ("Daemon transport and discovery",
"Separate user interface / two processes", "One Interchange Format").

## Transport

| Fact | Value | Source |
|------|-------|--------|
| Framing | JSON-Lines: one JSON object per line, `\n` terminated, UTF-8. The framing is **not** configurable. | accepted |
| Carrier | TCP stream on `127.0.0.1`, one connection per session. TCP, not a Unix domain socket, so Windows and macOS work without platform-specific code. | accepted |
| Address | `core.address` and `core.port` in the TOML configuration, default `127.0.0.1:47623`. Only the address is configurable. | accepted |
| Sharing | Several interface processes may share one daemon; the core serializes commands, so commands from different interfaces interleave **whole-command-by-whole-command**, never line by line. | accepted |
| Shutdown | The core exits after an idle timeout — configurable, default **15 minutes** without commands — or on an explicit `shutdown` command. The lock file is removed on exit. | accepted |

## Lock file and discovery

The core daemon writes `state/core.lock` inside the data directory at start-up.
It contains the address the daemon listens on and a **nonce** that identifies
this daemon instance. Format (plain text, one value per line, `#` comments
allowed):

```text
address=127.0.0.1:47623
nonce=6f2c1a48e0b7d9c3
```

Draft, not yet accepted: the key names and encoding of `state/core.lock` are
fixed here; architecture.md only says the lock file "contains the address and
a nonce".

Discovery strategy (accepted): **connect to an existing daemon, otherwise
start one.**

1. The interface reads `core.address`/`core.port` from the configuration and
   `state/core.lock` from the data directory.
2. If a lock file exists, the interface connects to the **configured** address
   and performs the handshake, using the nonce read from the lock file. The
   lock file is read for the nonce only, never for the address: the daemon
   listens on the configured address, so a lock file that disagrees with the
   configuration fails the handshake and is treated as stale. On success the
   interface reuses the running daemon and does **not** remove the lock file
   on exit. Draft, not yet accepted: architecture.md says only that the
   interface "reads the lock file, connects and performs the handshake"; the
   choice of the configured address over the address in the lock file is
   fixed here.
3. On a failed or missing handshake the interface starts a core daemon
   process, which connects to the configured address, performs the handshake
   and writes the lock file.
4. A port occupied by an unrelated process fails the handshake and is
   reported as an error naming the configured address; the interface never
   sends commands to an unknown process. Liveness of a process id is never
   used as the test, because a port can be reused.

## Handshake

The handshake is the `version` command plus the nonce from the lock file,
performed **before any other command**. The daemon answers with the same
nonce; a mismatch or a missing answer means the listener is not our daemon.

Interface → daemon:

<!-- test-example id=core-handshake-request -->
```json
{"v":1,"cmd":"version","nonce":"6f2c1a48e0b7d9c3"}
```

Daemon → interface:

<!-- test-example id=core-handshake-response -->
```json
{"v":1,"event":"hello","version":1,"nonce":"6f2c1a48e0b7d9c3","session":"core-1"}
```

Draft, not yet accepted: the nonce is simply echoed; the `session` field is
informational. The exact shape of the `hello` payload is not fixed by
architecture.md.

## Versioning

Every line — command and event alike — carries an integer `v` field, the
protocol version. Draft, not yet accepted: absent `v` means `1`; a line whose
`v` is higher than the daemon's supported version is rejected with an `error`
event. The handshake compares versions before any command is sent.

The vocabulary is **data driven**: command names and their parameters are
described in a table that both sides read, so adding a command does not
require changes in the interface layer. The tables below are that table.

## Commands

Commands are verb first. All known commands:

| Command | Parameters | Meaning |
|---------|------------|---------|
| `version` | `nonce` | Handshake only; proves the listener is our daemon. |
| `import` | `path` | Import one file **or a directory of files** (bank statement PDF, mbox) via the importers. |
| `report` | `format`, optional `year` | Render an analysis document (`markdown`, `html`, `pdf`, `ods`). |
| `set-category` | `id`, `category`, `reason` | Override the classification of one transaction and learn a rule. |
| `confirm-annotation` | `annotation`, `id` \| `reject` | Accept or reject an automatic annotation link. |
| `list-pending` | – | List low-confidence (pending) annotation links. |
| `cancel` | – | Cancel the command currently running. |
| `shutdown` | – | Exit the daemon and remove the lock file. |

Draft, not yet accepted: the exact parameter names of `confirm-annotation`
and `list-pending` are fixed here for the first time; architecture.md only
requires that pending links are "listed by the UI so the user can confirm or
reject them".

Import example (`path` names one file, or a directory whose files are imported
one by one; re-importing a file or a directory whose content was already
imported appends nothing a second time):

<!-- test-example id=core-command-import -->
```json
{"v":1,"cmd":"import","path":"/data/statements/2024/bank_a.pdf"}
```

Import result: `added` counts the transactions the run appended, `annotations`
the annotations it appended, and `linked` the annotations among those it
appended that ended linked.

<!-- test-example id=core-event-result-import -->
```json
{"v":1,"event":"result","cmd":"import","added":12,"annotations":3,"linked":2}
```

## Events

Events the interface consumes: `result`, `progress`, `log`, `error`,
`cancelled`. The core emits exactly one `result` (or `error`/`cancelled`) per
command; `progress` and `log` may appear any number of times in between.
Draft, not yet accepted: the "exactly one" rule is not stated in architecture.md;
it is inferred from the Runtime View examples, where every command ends with a
single `result` event.

Progress:

<!-- test-example id=core-event-progress -->
```json
{"v":1,"event":"progress","done":12,"total":30,"file":"bank_a.pdf","cmd":"import"}
```

Log:

<!-- test-example id=core-event-log -->
```json
{"v":1,"event":"log","level":"info","text":"probe: bank_a importer supported"}
```

Result of a classification correction (matches the Runtime View example in
architecture.md; `affected` is the number of transactions the learned rule
now covers):

<!-- test-example id=core-command-set-category -->
```json
{"v":1,"cmd":"set-category","id":"3f9a1c7e","category":"food:supermarket","reason":"user"}
```
<!-- test-example id=core-event-result-affected -->
```json
{"v":1,"event":"result","cmd":"set-category","affected":183}
```

Result of a report:

<!-- test-example id=core-event-result-report -->
```json
{"v":1,"event":"result","cmd":"report","format":"html","year":2024,"path":"report/2024.html"}
```

Error:

<!-- test-example id=core-event-error -->
```json
{"v":1,"event":"error","cmd":"import","code":"unknown-command","text":"unknown command: frob"}
```

Draft, not yet accepted: `code` values (`unknown-command`, `plugin-failed`,
`converter-missing`, `io`, `busy`) are named here for the first time.

## Cancellation

The interface sends `cancel` while a command is running. The core finishes
the **current item**, discards the partial result and answers `cancelled`.
Nothing is half written to the data files, because writes happen per file,
atomically (write temporary file, rename).

<!-- test-example id=core-command-cancel -->
```json
{"v":1,"cmd":"cancel"}
```
<!-- test-example id=core-event-cancelled -->
```json
{"v":1,"event":"cancelled","cmd":"import"}
```

Draft, not yet accepted: `cancel` applies to the most recent command of this
interface process; with several interfaces sharing one daemon, `cancel`
cancels the command that the same connection most recently sent.

## Shutdown

<!-- test-example id=core-command-shutdown -->
```json
{"v":1,"cmd":"shutdown"}
```

The daemon exits and removes `state/core.lock`. Draft, not yet accepted: the
daemon does not answer the `shutdown` command; the closed stream is the
signal.

## Unknown input

An unknown command or an unparseable line is answered with an `error` event
and the daemon stays alive; the stream never resynchronizes on a partial
line. Draft, not yet accepted: this error-recovery behaviour is implied by
architecture.md's "validate strictly on the core side" risk mitigation and
is fixed here.
