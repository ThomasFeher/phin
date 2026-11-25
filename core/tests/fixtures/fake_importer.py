#!/usr/bin/env python3
"""Test-only fake plugin importer for plan item 4.4b (T5).

A JSON-Lines plugin per `doc/architecture/plugin-protocol.md`: it reads one
request object per line on stdin and answers on stdout. The BEHAVIOR comes from
argv[1] so this ONE fixture serves every ExecImporter test:

  probe-false <marker-file>
        answers `{"supported": false}` to every probe and then reads forever;
        it writes <marker-file> ONLY if it ever receives an `extract` request,
        which is the marker mechanism `fake_plugin_probe_false_never_extracts`
        asserts on (the file must never appear).
  exit-after-probe
        answers a supported probe, writes one line to stderr, and exits 1
        without waiting for the extract request.
  sleep-after-probe
        answers a supported probe, then sleeps 30 s without answering the
        extract request; the host's timeout must kill it (the sleep is long so
        a killed child cannot answer late; the core's bound, not this sleep,
        bounds the test — a too-short core timeout leaves the sleep running and
        the kill catches it).
  transactions
        answers a supported probe, then — on the extract request — emits two
        `transactions` lines (the first carries a bogus `"id":"deadbeef"` the
        core must discard, planning decision 16) and the `end` terminator.
  annotation
        answers a supported probe (kind `annotating`), then — on the extract
        request — emits one `annotation` line and the `end` terminator that
        follows it; the host's refusal of that event is planning decision 17.
  probe-no-kind
        answers a supported probe WITHOUT the optional `kind` field, then
        answers the extract request with one `transactions` line whose
        `description` reports the `kind` the request carried (or `<none>` when
        it carried none) and the `end` terminator — the fixture that lets a
        test pin which `kind` the core sends when the plugin named none.
  grandchild-holds-stdout
        answers a supported probe, starts a GRANDCHILD that INHERITS this
        plugin's stdout (its stdin/stderr go to /dev/null), and exits 0 right
        away. The core's stdout reader thread therefore never sees EOF, ever,
        and reaping the child must not wait for it — that is the scenario the
        bounded `PluginChild::reap` join exists for.

The plugin name it reports in probe responses is `fake-importer`; the host's
configured importer name is the configured path string, not this value.

Started as `python3 fake_importer.py <mode> [marker]`, which is how the core
spawns it (the shebang is never relied on; the fixture is not executable).
"""

import json
import subprocess
import sys
import time


def send(obj):
    sys.stdout.write(json.dumps(obj, separators=(",", ":")) + "\n")
    sys.stdout.flush()


def main():
    if len(sys.argv) < 2:
        sys.stderr.write("fake_importer: no mode given\n")
        return 2
    mode = sys.argv[1]
    marker = sys.argv[2] if len(sys.argv) > 2 else ""

    if mode == "sleep-after-probe":
        # Read the probe request, answer it, then go silent for half a minute.
        for line in sys.stdin:
            request = json.loads(line)
            if request.get("cmd") == "probe":
                send({"v": 1, "event": "probe", "supported": True,
                      "kind": "sleepy", "name": "fake-importer"})
                time.sleep(30)
            else:
                send({"v": 1, "event": "error", "plugin": "fake-importer",
                      "code": "bad-sequence", "text": "expected probe"})
        return 0

    if mode == "exit-after-probe":
        for line in sys.stdin:
            request = json.loads(line)
            if request.get("cmd") == "probe":
                send({"v": 1, "event": "probe", "supported": True,
                      "kind": "dying", "name": "fake-importer"})
                sys.stderr.write("fake importer is leaving early\n")
                sys.stderr.flush()
                return 1
            send({"v": 1, "event": "error", "plugin": "fake-importer",
                  "code": "bad-sequence", "text": "expected probe"})
        return 0

    if mode == "transactions":
        for line in sys.stdin:
            request = json.loads(line)
            if request.get("cmd") == "probe":
                send({"v": 1, "event": "probe", "supported": True,
                      "kind": "bank_x", "name": "fake-importer"})
            elif request.get("cmd") == "extract":
                send({"v": 1, "event": "transactions", "id": "deadbeef",
                      "date": "2024-01-03", "amount": -3999, "currency": "EUR",
                      "description": "REWE MARKT 12",
                      "counterparty": "REWE MARKET", "source": "statement.pdf"})
                send({"v": 1, "event": "transactions",
                      "date": "2024-01-09", "amount": -5990, "currency": "EUR",
                      "description": "DINNER ZUR LEITZ",
                      "counterparty": "ZUR LEITZ", "source": "statement.pdf"})
                send({"v": 1, "event": "end"})
            else:
                send({"v": 1, "event": "error", "plugin": "fake-importer",
                      "code": "bad-sequence", "text": "expected probe"})
        return 0

    if mode == "annotation":
        for line in sys.stdin:
            request = json.loads(line)
            if request.get("cmd") == "probe":
                send({"v": 1, "event": "probe", "supported": True,
                      "kind": "annotating", "name": "fake-importer"})
            elif request.get("cmd") == "extract":
                # The annotation variant of `plugin-protocol.md` ("mail
                # importers use the same extract shape"): Phase 4 has no
                # annotation-plugin story, so the host must refuse this line.
                send({"v": 1, "event": "annotation",
                      "text": "PayPal payment to a fake counterparty",
                      "items": ["1x book, 1x ebook"],
                      "source": "mail_paypal.mbox", "confidence": 0.9})
                send({"v": 1, "event": "end"})
            else:
                send({"v": 1, "event": "error", "plugin": "fake-importer",
                      "code": "bad-sequence", "text": "expected probe"})
        return 0

    if mode == "probe-no-kind":
        for line in sys.stdin:
            request = json.loads(line)
            if request.get("cmd") == "probe":
                # Deliberately NO `kind` field: it is optional on the wire.
                send({"v": 1, "event": "probe", "supported": True,
                      "name": "fake-importer"})
            elif request.get("cmd") == "extract":
                # Report what the extract request asked for, in the only field
                # this line carries text in.
                send({"v": 1, "event": "transactions",
                      "date": "2024-01-03", "amount": -3999, "currency": "EUR",
                      "description": "kind=[%s]"
                      % request.get("kind", "<none>"),
                      "counterparty": "KIND ECHO", "source": "statement.pdf"})
                send({"v": 1, "event": "end"})
            else:
                send({"v": 1, "event": "error", "plugin": "fake-importer",
                      "code": "bad-sequence", "text": "expected probe"})
        return 0

    if mode == "grandchild-holds-stdout":
        for line in sys.stdin:
            request = json.loads(line)
            if request.get("cmd") == "probe":
                send({"v": 1, "event": "probe", "supported": True,
                      "kind": "leaky", "name": "fake-importer"})
                # The leak: a grandchild inherits OUR stdout, so the pipe stays
                # open past our own death and the core's reader thread never
                # reads EOF. Its own stdin/stderr go nowhere, so it never keeps
                # the request or error pipe open either.
                subprocess.Popen(
                    [sys.executable, "-c", "import time; time.sleep(5)"],
                    stdin=subprocess.DEVNULL,
                    stdout=sys.stdout,
                    stderr=subprocess.DEVNULL)
                # Exit at once; the core's next thing is the extract request.
                return 0
        return 0

    if mode == "probe-false":
        if marker == "":
            sys.stderr.write("fake_importer: probe-false needs a marker file\n")
            return 2
        for line in sys.stdin:
            request = json.loads(line)
            if request.get("cmd") == "probe":
                send({"v": 1, "event": "probe", "supported": False})
            elif request.get("cmd") == "extract":
                # The marker is the whole point of this mode: reaching it means
                # the core extracted from a plugin that never supported.
                with open(marker, "w") as held:
                    held.write("extract requested\n")
                send({"v": 1, "event": "error", "plugin": "fake-importer",
                      "code": "refused", "text": "extract refused"})
            else:
                send({"v": 1, "event": "error", "plugin": "fake-importer",
                      "code": "unknown-command",
                      "text": "unknown command: %s" % request.get("cmd")})
        return 0

    sys.stderr.write("fake_importer: unknown mode [%s]\n" % mode)
    return 2


sys.exit(main())
