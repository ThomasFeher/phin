# Phin test fixtures

All files in this directory (except `README.md` and `generate.py`) are
generated. Do not edit them by hand; regenerate instead:

```sh
python3 testdata/generate.py [target-dir]   # target-dir defaults to "testdata"
```

The generator uses only the Python standard library, contains no timestamps
and no randomness, so two runs produce byte-identical files. Determinism is
checked by running it into two directories and comparing `sha256sum`
manifests.

## Files

| File | Simulates |
| --- | --- |
| `bank_a.pdf` | Bank statement, **layout A** ("Bank A AG"): 2 pages, table with `Date / Description / Amount (EUR)`, dates `DD.MM.YYYY`, amounts `-1.234,56 EUR` (comma decimal, dot thousands). Real text layer (Helvetica, `Td`/`Tj` operators), no OCR needed. 6 transactions, one of them >= 1000 EUR (`-1.234,56 EUR`) so the dot-thousands grouping path is exercised. |
| `bank_b1.pdf` | Bank statement, **layout B** ("Bank B N"): columns `Date / Card-Number / Description / Amount (EUR)`, dates `YYYY-MM-DD`, amounts `-1234.56 EUR` (dot decimal). February 2024, 4 transactions. |
| `bank_b2.pdf` | Same layout B, March 2024, 4 transactions — two of them have identical content, which exercises the occurrence suffix `#2` of the content-hash ids. |
| `mail_with_pdf.mbox` | Plain mail thread, 12 messages between two people; one message carries `bank_b1.pdf` as a `application/pdf` attachment (base64, byte-identical to the generated PDF), as proper `multipart/mixed` MIME with valid `--<boundary>` delimiter lines, so standard parsers (e.g. Python `mailbox` + `email`) find exactly one `application/pdf` part and decode it byte-identically. One plain message body contains a column-0 `From ` line, which the generator escapes to `>From the internet: hello` (mbox quoting), so the reader of plan item 4.5 must un-escape it without ending the message there. Tests the mbox parser and the "extract attachment, then run the PDF parser" path. |
| `mail_paypal.mbox` | 8 PayPal-style payment notifications (each a proper `multipart/mixed` message with a visible `text/plain` part and a `text/html` part): date, amount, payment reason, payment provider. Each carries a Thunderbird `X-Mozilla-Status:` header, `0001` (unread) on the third message and `0000` on the others, exercising that flag read of plan item 4.5. Tests the notification parser. |
| `transactions.csv` | All fixture transactions (22 lines: 21 distinct transactions, one duplicate pair) in storage format: `;` separator, `#` comment header, quoted fields where the description contains `;`, amounts as integer minor units (negative = expense), dates normalised to `YYYY-MM-DD`, first column = short content-hash id (duplicate pair carries `#2`). |
| `categories.csv` | Example category tree (`parent;name`, exactly two columns, roots have an empty parent); deeper paths are built by chaining immediate parents, e.g. the 3-level branch `finance:expense:transport` is the rows `finance;expense` plus `expense;transport`. Order and comments must survive a round-trip. |
| `rules/sample.rules` | Sample rule file: one rule per line `pattern -> category`, `#` comments, empty lines, file order = precedence. Shows literal patterns and one explicitly marked `regex:` pattern. The final pattern syntax is fixed in plan item 1.5; this sample is provisional. |

Message total across the two mbox files: 20. All `Date:` headers use the RFC 53222 / US-ASCII form (`31 Mar 2024 18:45:20 +0100`), as do the `From ` separator lines. `From `-starting body lines are escaped to `>From ` (exactly one such line exists, in `mail_with_pdf.mbox`), and only the `From ` envelope line of each message sits at column 0 — so a reader splits at column-0 `From ` lines and un-escapes the `>From ` body line.

## Regenerating

```sh
python3 testdata/generate.py            # writes into ./testdata
python3 testdata/generate.py /tmp/run1  # any target directory
```
