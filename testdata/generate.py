#!/usr/bin/env python3
# Phin test fixtures -- deterministic generator (plan item 0.3).
# Usage: python3 testdata/generate.py [target-dir]   (default: testdata)
# Stdlib only. No timestamps, no randomness: two runs produce byte-identical
# files (verified with sha256sum manifests).

import hashlib
import sys
from base64 import b64encode

# ---------------------------------------------------------------------------
# Data: the transactions behind the fixtures.
# amount is in minor units (cents); negative = paid out (expense).
# ---------------------------------------------------------------------------

# Layout A statement (bank_a.pdf): date DD.MM.YYYY, amount "1.234,56 EUR"
BANK_A = [
    ("03.01.2024", -3999, "REWE MARKT 12", "REWE MARKET", "bank_a.pdf"),
    ("09.01.2024", -5990, "DINNER ZUR LEITZ", "ZUR LEITZ", "bank_a.pdf"),
    ("15.01.2024", 42000, "PAYMENT TO TOBIAS", "TAXI BERLIN", "bank_a.pdf"),
    ("21.01.2024", -1847, "BOOKS ONLINE", "THONIX", "bank_a.pdf"),
    # >= 1000 EUR: exercises the dot-thousands grouping of layout A
    ("24.01.2024", -123456, "MEGA MART 04", "MEGA MART", "bank_a.pdf"),
    ("28.01.2024", -12990, "LIDL 0815", "LIDL", "bank_a.pdf"),
]

# Layout B statement (bank_b1.pdf / bank_b2.pdf): date YYYY-MM-DD,
# amount "-45.00 EUR", plus a card number column.
BANK_B1 = [
    ("2024-02-03", -8990, "ALDI 4711", "ALDI", "4711-0000-4711", "bank_b1.pdf"),
    ("2024-02-11", -2500, "TAXI AIRPORT; NIGHT", "TAXI BERLIN", "4711-0000-4711", "bank_b1.pdf"),
    ("2024-02-17", 85000, "CLIENT PAYMENT", "ALICE", "4711-0000-4711", "bank_b1.pdf"),
    ("2024-02-24", -3456, "REWE MARKT 12", "REWE MARKET", "4711-0000-4711", "bank_b1.pdf"),
]

BANK_B2 = [
    ("2024-03-02", -1590, "CAFETARIA", "CAFETARIA", "4711-0000-4711", "bank_b2.pdf"),
    # identical content twice on one statement -> id gets occurrence suffix #2
    ("2024-03-05", -1500, "REWE MARKT 12", "REWE MARKET", "4711-0000-4711", "bank_b2.pdf"),
    ("2024-03-05", -1500, "REWE MARKT 12", "REWE MARKET", "4711-0000-4711", "bank_b2.pdf"),
    ("2024-03-19", -6789, "AMAZON MARKETPLACE", "AMZN", "4711-0000-4711", "bank_b2.pdf"),
]

# PayPal notifications (mail_paypal.mbox): date, cents, what, who
PAYPAL = [
    ("2024-04-02", 2599, "LEGO STORE DE", "LEGO STORE"),
    ("2024-04-05", -1999, "NETFLIX.COM", "NETFLIX"),
    ("2024-04-09", 899, "BAKERY BERLIN", "BAKERY BERLIN"),
    ("2024-04-12", -4500, "STEAM PURCHASE", "VALVE"),
    ("2024-04-15", 12000, "CLIENT PAYMENT", "ALICE"),
    ("2024-04-18", -799, "BOOKS ONLINE", "THONIX"),
    ("2024-04-21", 3450, "SUPERMARKET; DELICATES", "DELICATES", "quoting demo"),
    ("2024-04-26", -2999, "TAXI AIRPORT; NIGHT", "TAXI BERLIN"),
]

# Plain mail thread around the PDF attachment (mail_with_pdf.mbox)
PLAIN_MAIL = [
    ("Anna", "2024-03-30 09:12:11 +0100", "bank statement feb", "plain"),
    ("Tobias", "2024-03-30 10:02:44 +0100", "Re: bank statement feb", "plain"),
    ("Anna", "2024-03-30 10:30:03 +0100", "Re: bank statement feb", "plain"),
    ("Tobias", "2024-03-31 18:45:20 +0100", "statement attachment", "pdf"),
    ("Anna", "2024-04-01 08:05:55 +0100", "Re: statement attachment", "plain"),
    ("Tobias", "2024-04-02 12:00:01 +0100", "Re: statement attachment", "plain"),
    ("Anna", "2024-04-03 19:33:10 +0100", "Re: statement attachment", "plain"),
    ("Tobias", "2024-04-04 07:41:09 +0100", "Re: statement attachment", "plain"),
    ("Anna", "2024-04-05 20:15:37 +0100", "Re: statement attachment", "plain"),
    ("Tobias", "2024-04-06 09:59:59 +0100", "Re: statement attachment", "plain"),
    ("Anna", "2024-04-07 11:23:45 +0100", "Re: statement attachment", "plain"),
    ("Tobias", "2024-04-08 16:50:02 +0100", "Re: statement attachment", "plain"),
]


def fmt_amount(minor, style):
    """style 'a': -1.234,56 EUR   style 'b': -1234.56 EUR"""
    sign = "-" if minor < 0 else ""
    v = abs(minor)
    whole, cents = divmod(v, 100)
    if style == "a":
        s = f"{whole:,}".replace(",", ".")
        return f"{sign}{s.replace(',', '.')},{cents:02d} EUR"
    return f"{sign}{whole}.{cents:02d} EUR"


def tx_id(date, minor, desc, party, source):
    norm = f"{date}|{minor}|EUR|{desc}|{party}|{source}".encode("utf-8")
    return hashlib.sha256(norm).hexdigest()[:10]


def quote(field):
    if any(c in field for c in ';"') or field != field.strip():
        return '"' + field.replace('"', '""') + '"'
    return field


def iso_date(d):
    """fixture date -> internal YYYY-MM-DD (see plan item 1.2)."""
    if "." in d:
        dd, mm, yy = d.split(".")
        return f"{yy}-{mm}-{dd}"
    return d


def csv_row(tx):
    date, minor, desc, party, source = tx[0], tx[1], tx[2], tx[3], tx[4]
    date = iso_date(date)
    return ";".join([
        tx_id(date, minor, desc, party, source),
        date, str(minor), "EUR", quote(desc), quote(party), source,
    ])


# ---------------------------------------------------------------------------
# Minimal hand-built PDF with a real text layer (Helvetica, Td/Tj ops).
# ---------------------------------------------------------------------------

PAGE_W, PAGE_H, MARGIN, TOP, LEAD = 595, 842, 40, 790, 16


def pdf_build(pages):
    """pages: list of pages, each a list of text lines. Returns PDF bytes."""
    def esc(s):
        return s.replace("\\", r"\\").replace("(", r"\(").replace(")", r"\)")

    def content(lines):
        parts = []
        for i, line in enumerate(lines):
            y = TOP - i * LEAD
            parts.append(f"BT\n1 0 0 1 {MARGIN} {y} Tm\n/F1 10 Tf\n"
                         f"({esc(line)}) Tj\nET")
        stream = "\n".join(parts).encode("ascii")
        return (f"<< /Length {len(stream)} >>\nstream\n".encode("ascii")
                + stream + b"\nendstream")

    npages = len(pages)
    page_ids = [3 + 2 * i for i in range(npages)]
    cont_ids = [4 + 2 * i for i in range(npages)]
    font_id = 3 + 2 * npages
    info_id = 4 + 2 * npages
    kids = " ".join(f"{pid} 0 R" for pid in page_ids)

    objs = [
        b"<< /Type /Catalog /Pages 2 0 R >>",
        (f"<< /Type /Pages /Kids [{kids}] /Count {npages} >>").encode("ascii"),
    ]
    for i in range(npages):
        objs.append((f"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595 842] "
                     f"/Resources << /Font << /F1 {font_id} 0 R >> >> "
                     f"/Contents {cont_ids[i]} 0 R >>").encode("ascii"))
        objs.append(content(pages[i]))
    objs.append(b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica "
                b"/Encoding /WinAnsiEncoding >>")
    objs.append(b"<< /Title (Phin testdata fixture) /Author (Phin generator) >>")

    out = bytearray(b"%PDF-1.4\n")
    offsets = []
    for num, body in enumerate(objs, start=1):
        offsets.append(len(out))
        out += f"{num} 0 obj\n".encode("ascii")
        out += body
        out += b"\nendobj\n"
    xref = len(out)
    n = len(objs) + 1
    out += f"xref\n0 {n}\n".encode("ascii")
    out += b"0000000000 65535 f \n"
    for off in offsets:
        out += f"{off:010d} 00000 n \n".encode("ascii")
    out += (f"trailer\n<< /Size {n} /Root 1 0 R /Info {info_id} 0 R >>\n"
            f"startxref\n{xref}\n%%EOF\n").encode("ascii")
    return bytes(out)


def bank_a_lines(tx_rows, title, period):
    lines = [title, "Account: 1234567890", "Period: " + period, ""]
    lines.append("Date        Description              Amount (EUR)")
    for date, minor, desc, party, source in tx_rows:
        lines.append(f"{date}  {desc:<24} {fmt_amount(minor, 'a'):>15}")
    lines.append("")
    lines.append("Generated by Phin testdata generator. Not a real bank.")
    total = sum(r[1] for r in tx_rows)
    page2 = [title + " -- Summary", "Period: " + period, "",
             "Total: " + fmt_amount(total, "a"),
             "Generated by Phin testdata generator. Not a real bank."]
    return [lines, page2]


def bank_b_lines(tx_rows, title, period):
    lines = [title, "Kredit: ****-****-4711", "Monat: " + period, ""]
    lines.append("Date        Card-Number     Description              Amount (EUR)")
    for date, minor, desc, party, card, source in tx_rows:
        lines.append(f"{date}  {card:<16} {desc:<24} {fmt_amount(minor, 'b'):>15}")
    lines.append("")
    lines.append("Generated by Phin testdata generator. Not a real bank.")
    total = sum(r[1] for r in tx_rows)
    page2 = [title + " -- Summary", "Monat: " + period, "",
             "Total: " + fmt_amount(total, "b"),
             "Generated by Phin testdata generator. Not a real bank."]
    return [lines, page2]


# ---------------------------------------------------------------------------
# mbox fixtures
# ---------------------------------------------------------------------------

# RFC 2045: the boundary value carries no leading dashes; the delimiter line
# is "--" + boundary.
PDF_MIME_BOUNDARY = "PhinBoundary0001"
PAYPAL_MIME_BOUNDARY = "PayPalBoundary0002"


def wrap_b64(data, width=76):
    enc = b64encode(data).decode("ascii")
    return [enc[i:i + width] for i in range(0, len(enc), width)]


def mail_with_pdf_body(attachment_pdf):
    delim = "--" + PDF_MIME_BOUNDARY
    return [
        delim,
        "Content-Type: text/plain; charset=us-ascii",
        "",
        "Hi Anna,",
        "",
        "please find attached the February statement of Bank B.",
        "The file is the same bank_b1.pdf fixture used by the parser tests.",
        "",
        "Tobias",
        "",
        delim,
        "Content-Type: application/pdf; name=bank_b1.pdf",
        "Content-Disposition: attachment; filename=bank_b1.pdf",
        "Content-Transfer-Encoding: base64",
        "",
    ] + wrap_b64(attachment_pdf) + [delim + "--"]


def paypal_body(what, amount_str):
    delim = "--" + PAYPAL_MIME_BOUNDARY
    return [
        delim,
        "Content-Type: text/plain; charset=UTF-8",
        "",
        "Hello Tobias,",
        "",
        "Wir haben erhalten Ihre Zahlung.",
        "",
        "Betrag: " + amount_str,
        "Grund: " + what,
        "Payment-Provider: PAYPAL *NAMEOFPAYEE",
        "Details: Einzeluebersicht",
        "",
        "Bitte fragen uns, falls es Fragen.",
        "PayPal",
        "",
        delim,
        "Content-Type: text/html; charset=UTF-8",
        "",
        "<html><body><p>Payment received. Fixture only.</p></body></html>",
        delim + "--",
    ]


def mail_plain_body(who):
    return [
        "Hello " + who + ",",
        "",
        "this is one of the plain messages in the mbox fixture.",
        "No attachment here; the thread contains one message with a PDF.",
        "",
        "Best regards",
        who,
    ]


def rfc_date(datestr):
    """'2024-03-30 09:12:11 +0100' -> '30 Mar 2024 09:12:11 +0100' (RFC 53222)."""
    import datetime
    ymd, hms, tz = datestr.split(" ")
    dt = datetime.datetime.strptime(ymd + " " + hms, "%Y-%m-%d %H:%M:%S")
    return dt.strftime("%d %b %Y %H:%M:%S") + " " + tz


def mbox_ctime(datestr):
    """'2024-03-30 09:12:11 +0100' -> 'Mon Mar 30 09:12:11 2024' (deterministic)."""
    import datetime
    dt = datetime.datetime.strptime(datestr.split(" ")[0] + " "
                                    + datestr.split(" ")[1], "%Y-%m-%d %H:%M:%S")
    return dt.strftime("%a %b %d %H:%M:%S %Y")


def build_mbox(messages):
    """messages: dicts with sender_email, from_, date, subject, content_type, body."""
    chunks = []
    for msg in messages:
        lines = [
            f"From {msg['sender_email']} {mbox_ctime(msg['date'])}",
            f"From: {msg['from_']}",
            f"To: {msg['to']}",
            f"Subject: {msg['subject']}",
            "Date: " + rfc_date(msg["date"]),
            "Content-Type: " + msg["content_type"],
            "",
        ]
        body = msg["body"]
        # mbox "From " escaping in the body
        lines += [(">" + ln) if ln.startswith("From ") else ln for ln in body]
        lines.append("")
        chunks.append("\n".join(lines) + "\n")
    return "".join(chunks)


def main():
    out = sys.argv[1] if len(sys.argv) > 1 else "testdata"
    import os
    os.makedirs(out, exist_ok=True)

    def write(name, data):
        path = os.path.join(out, name)
        os.makedirs(os.path.dirname(path), exist_ok=True)
        with open(path, "wb") as fh:
            fh.write(data)

    # --- PDFs -------------------------------------------------------------
    bank_a_pdf = pdf_build(bank_a_lines(
        BANK_A, "Bank A AG -- Account Statement", "January 2024"))
    bank_b1_pdf = pdf_build(bank_b_lines(
        BANK_B1, "Bank B N -- Monthly Statement", "February 2024"))
    bank_b2_pdf = pdf_build(bank_b_lines(
        BANK_B2, "Bank B N -- Monthly Statement", "March 2024"))
    write("bank_a.pdf", bank_a_pdf)
    write("bank_b1.pdf", bank_b1_pdf)
    write("bank_b2.pdf", bank_b2_pdf)

    # --- mbox files -------------------------------------------------------
    msgs = []
    for (who, date, subject, kind) in PLAIN_MAIL:
        email = ("anna@example.com" if who == "Anna" else "tobias@example.com")
        if kind == "pdf":
            body = mail_with_pdf_body(bank_b1_pdf)
            ctype = "multipart/mixed; boundary=" + PDF_MIME_BOUNDARY
        else:
            body = mail_plain_body(who)
            ctype = "text/plain; charset=us-ascii"
        msgs.append({"sender_email": email, "from_": who + " <" + email + ">",
                     "to": ("tobias@example.com" if who == "Anna" else "anna@example.com"),
                     "date": date, "subject": subject, "body": body,
                     "content_type": ctype})
    write("mail_with_pdf.mbox", build_mbox(msgs).encode("utf-8"))

    paypal_msgs = []
    for row in PAYPAL:
        date, minor, what, party = row[0], row[1], row[2], row[3]
        paypal_msgs.append({
            "sender_email": "payments@example.com",
            "from_": "PAYPAL *NameOfPayee <payments@example.com>",
            "to": "tobias@example.com",
            "date": date + " 12:00:00 +0100",
            "subject": "Your PayPal payment receipt",
            "body": paypal_body(what, fmt_amount(minor, "a")),
            "content_type": "multipart/mixed; boundary=" + PAYPAL_MIME_BOUNDARY,
        })
    write("mail_paypal.mbox", build_mbox(paypal_msgs).encode("utf-8"))

    # --- CSV files --------------------------------------------------------
    all_tx = []
    for rows in (BANK_A,):
        for row in rows:
            all_tx.append(row[:5])
    for rows in (BANK_B1, BANK_B2):
        for row in rows:
            # layout B rows carry the card number before the source file name
            all_tx.append((row[0], row[1], row[2], row[3], row[5]))
    for row in PAYPAL:
        all_tx.append((row[0], row[1], row[2], row[3], "mail_paypal.mbox"))

    tx_lines = [
        "# Phin transactions -- append-only facts, never edited in place.",
        "# columns: id;date;amount;currency;description;counterparty;source",
        "# amount is an integer in minor units (cents); negative = expense.",
        "# dates are normalised to YYYY-MM-DD; id = short content hash.",
    ]
    seen = {}
    for row in all_tx:
        line = csv_row(row)
        rid = line.split(";", 1)[0]
        seen[rid] = seen.get(rid, 0) + 1
        if seen[rid] > 1:
            line = rid + "#" + str(seen[rid]) + line[len(rid):]
        tx_lines.append(line)
    write("transactions.csv", ("\n".join(tx_lines) + "\n").encode("utf-8"))

    cat_lines = [
        "# Phin categories -- tree of parent:child names, order is preserved.",
        "# columns: parent;name",
        ";food",
        ";finance",
        ";leisure",
        "food;supermarket",
        "food;restaurant",
        "finance;income",
        "finance;expense",
        "expense;transport",
        "expense;utilities",
        "leisure;books",
        "leisure;games",
    ]
    write("categories.csv", ("\n".join(cat_lines) + "\n").encode("utf-8"))

    rules = [
        "# Phin sample rules -- one rule per line: pattern -> category",
        "# '#' starts a comment, empty lines are ignored, file order = precedence.",
        "# The exact pattern syntax is finalised in plan item 1.5; this sample",
        "# shows literal, word-list and regex patterns.",
        "",
        "REWE MARKT -> food:supermarket",
        "ALDI -> food:supermarket",
        "LIDL -> food:supermarket",
        "ZUR LEITZ -> food:restaurant",
        "CAFETARIA -> food:restaurant",
        "TAXI -> finance:expense:transport",
        "BOOKS ONLINE -> leisure:books",
        "AMAZON MARKETPLACE -> leisure:games",
        "regex:^PAYMENT TO -> finance:income",
        "CLIENT PAYMENT -> finance:income",
    ]
    write(os.path.join("rules", "sample.rules"),
          ("\n".join(rules) + "\n").encode("utf-8"))


if __name__ == "__main__":
    main()
