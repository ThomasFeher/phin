# Plugin Protocol

Between the **core** (host side) and a **plugin process** (external
executable). Same JSON-Lines encoding as the Core Protocol, different
vocabulary: `probe`, `extract` for importers, `analyse` for analyses.
Restates the accepted decision "Plugin interface as external process" in
`doc/architecture/architecture.md`.

## Process lifecycle

- Plugins are discovered from the configuration (list of executables).
- A plugin must be executable; it is started as a separate process by the
  core, one process per plugin per run. Draft, not yet accepted: "one process
  per plugin per run" is fixed here; architecture.md only says plugin
  processes are "started and stopped on demand by the core".
- The core writes requests to the plugin's stdin, reads responses from its
  stdout, one JSON object per line.
- The core `probe`s every configured importer for a file, then `extract`s
  from the ones that answered `supported: true`.
- A plugin that crashes or answers with an invalid line is reported as an
  `error` event on the Core Protocol **naming the plugin**; a failing
  importer must not break the whole run (risk: "Plugin protocol is untyped",
  "PDF text extraction quality").
- Draft, not yet accepted: the core kills a plugin after a configurable
  timeout (default 60 s) and reports it as `plugin-failed`.

## Transactions on the wire

A transaction object as exchanged with plugins and analyses (fields per the
Domain Model in architecture.md):

<!-- test-example id=plugin-transaction-object -->
```json
{"date":"2024-01-03","amount":-3999,"currency":"EUR","description":"REWE MARKT 12","counterparty":"REWE MARKET","source":"bank_a.pdf"}
```

- `amount` is an integer in minor units, negative = expense.
- `source` is the provenance: the file the transaction was extracted from.
- Plugins need not send `id`: the core computes the content-hash id and the
  occurrence suffix (see `data-files.md`). Draft, not yet accepted: plugins
  may omit `id`; if one sends it, the core ignores it.

## `probe` (importer)

Core → plugin:

<!-- test-example id=plugin-probe-request -->
```json
{"v":1,"cmd":"probe","file":"/data/statements/2024/bank_a.pdf","kind":"bank-statement"}
```

Plugin → core:

<!-- test-example id=plugin-probe-response -->
```json
{"v":1,"event":"probe","supported":true,"kind":"bank_a","name":"bank-a-importer"}
```

`supported` is mandatory. `kind` names the concrete dialect (used for
logging and for choosing among several importers); `name` is the configured
plugin name. Draft, not yet accepted: `name` in the probe response and `kind`
on the request are additions fixed here; architecture.md only requires that
importers answer `supported` for a file. `kind` on the request is optional
advice; the core normally sends only `file`.

## `extract` (importer)

Core → plugin:

<!-- test-example id=plugin-extract-request -->
```json
{"v":1,"cmd":"extract","file":"/data/statements/2024/bank_a.pdf","kind":"bank_a"}
```

Plugin → core, one transaction per line, then a terminator:

<!-- test-example id=plugin-extract-response -->
```json
{"v":1,"event":"transactions","date":"2024-01-03","amount":-3999,"currency":"EUR","description":"REWE MARKT 12","counterparty":"REWE MARKET","source":"bank_a.pdf"}
{"v":1,"event":"transactions","date":"2024-01-09","amount":-5990,"currency":"EUR","description":"DINNER ZUR LEITZ","counterparty":"ZUR LEITZ","source":"bank_a.pdf"}
{"v":1,"event":"end"}
```

Draft, not yet accepted: `kind` on the extract request is fixed here, taken
from the `kind` the plugin reported in its `probe` response; architecture.md
only fixes that the core `probe`s and then `extract`s from the importers that
answered `supported`. The terminator line `{"event":"end"}` is fixed here;
architecture.md only says "`extract` returns candidate transactions as
JSON-Lines". The core deduplicates by id and appends only unknown ids, so
`extract` may return duplicates and need not be exact about them.

Mail importers use the same `extract` shape but emit **annotation** objects
instead (mail never creates a transaction, R002):

<!-- test-example id=plugin-extract-annotation -->
```json
{"v":1,"event":"annotation","text":"PayPal payment to Amazon Services","items":["1x book, 1x ebook"],"source":"mail_paypal.mbox","confidence":0.9}
{"v":1,"event":"end"}
```

Draft, not yet accepted: annotation objects on the wire carry no link yet;
the core performs the automatic link (date, amount, counterparty) and
assigns the final confidence.

## `analyse` (analysis plugin)

Core → plugin, the full current dataset in one line:

<!-- test-example id=plugin-analyse-request -->
```json
{"v":1,"cmd":"analyse","name":"biggest-spendings","transactions":[{"date":"2024-01-03","amount":-3999,"currency":"EUR","description":"REWE MARKT 12","counterparty":"REWE MARKET","source":"bank_a.pdf","category":"food:supermarket"}],"categories":[{"parent":"","name":"food"},{"parent":"food","name":"supermarket"}],"year":2024}
```

Plugin → core, a table the core renders natively as Markdown/HTML (PDF and
ODS go through the external converters):

<!-- test-example id=plugin-analyse-response -->
```json
{"v":1,"event":"table","name":"biggest spendings 2024","columns":["date","description","amount"],"rows":[["2024-01-03","REWE MARKT 12",-3999]]}
{"v":1,"event":"end"}
```

Draft, not yet accepted: `year` on the analyse request and the table shape
(`columns` of strings, `rows` of arrays, amounts as minor units) are fixed
here; architecture.md does not name these fields.

## Versioning

Same rule as the Core Protocol: every line carries an integer `v`. Draft,
not yet accepted: absent `v` means `1`; a plugin that receives an unknown
`cmd` must answer an `error` event and keep running. The event names the
plugin, as architecture.md requires for plugin errors:

<!-- test-example id=plugin-error -->
```json
{"v":1,"event":"error","plugin":"bank-a-importer","code":"unknown-command","text":"unknown command: frob"}
```
