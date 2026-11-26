//! Automatic annotation → transaction linking of plan item 4.7: the mail
//! facts [`crate::technical::mail_annotate`] extracted get the FINAL
//! confidence (decision 5, the numbers pinned by the user):
//!
//! * exactly ONE candidate transaction ⇒ confidence [`CONF_LINKED`], the
//!   annotation's `ref` set to that transaction's id, status
//!   [`LinkStatus::Linked`];
//! * MORE THAN ONE candidate ⇒ confidence [`CONF_PENDING`], `ref` left EMPTY
//!   (a pending link is never persisted pointed at one of them), the candidate
//!   ids recorded in [`LinkStatus::Pending`] for the caller (plan item 4.8
//!   returns them; `list-pending` of Phase 6 lists them later);
//! * ZERO candidates ⇒ an ORPHAN at confidence [`CONF_ORPHAN`]
//!   (`doc/architecture/data-files.md`: an empty `ref` in `annotations.csv`
//!   means orphan/pending; `doc/architecture/architecture.md` "Mail as
//!   annotation": a low-confidence link stays pending, and mail NEVER creates
//!   a transaction — an unmatched mail is simply kept as an orphan).
//!
//! A candidate is a transaction whose date is within [`DATE_WINDOW_DAYS`]
//! (±3 days, boundaries INCLUDED) of the fact date, whose amount EQUALS the
//! fact amount in minor units, and whose counterparty check passes: the
//! fact's `counterparty_hint` must be a case-INSENSITIVE substring of the
//! annotation's text or of one of its items; an EMPTY hint passes vacuously
//! (decision 5).
//!
//! The module is PURE: it performs no I/O of any kind and writes nothing —
//! appending to `annotations.csv` is plan item 4.8's job, and this module's
//! own check test proves the absence of file-system usage by scanning its own
//! source. The output order equals the input order, and the candidate ids a
//! `Pending` carries are sorted ascending, so the whole result is
//! deterministic (the review focus of the task).
//!
//! Date math is the private civil-days algorithm of decision 21: both sides
//! carry the canonical `YYYY-MM-DD` form, converted to integer day counts by
//! proleptic civil-date math; `core` has no date crate and this module adds
//! none. A fact whose date is not canonical (the empty date
//! [`crate::technical::mail_annotate`] documents for a message without a
//! usable `Date:` header, or any other non-canonical text) can NEVER match:
//! it degrades to an orphan, never to a panic. Transaction dates are already
//! canonical — `domain::transaction::from_stored` guarantees it — but the
//! guard on the transaction side is kept anyway, and a non-canonical one
//! would simply never be a candidate.

use crate::domain::annotation::Annotation;
use crate::domain::annotation::clamp_confidence;
use crate::domain::transaction::Transaction;
use crate::technical::mail_annotate::AnnotatedMail;

/// The inclusive date window in days around the fact date within which a
/// transaction is a candidate (decision 5: ±3 days; exactly ±3 matches, ±4
/// does not).
pub const DATE_WINDOW_DAYS: i64 = 3;

/// The final confidence of a unique candidate: the annotation links to it
/// (decision 5).
pub const CONF_LINKED: f64 = 0.9;

/// The final confidence of an ambiguous link: the annotation stays PENDING
/// with every candidate recorded (decision 5).
pub const CONF_PENDING: f64 = 0.4;

/// The final confidence of an orphan: no transaction is a candidate (decision
/// 5).
pub const CONF_ORPHAN: f64 = 0.0;

/// How many candidate transactions [`auto_link`] found for one mail fact.
///
/// The variant order mirrors the three outcomes of decision 5; equality
/// matters because tests and plan item 4.8's integration compare against it.
#[derive(Debug, Eq, PartialEq)]
pub enum LinkStatus {
    /// Exactly one candidate; the annotation's `ref` names it and its
    /// confidence is [`CONF_LINKED`].
    Linked,
    /// More than one candidate: the candidate transaction ids, sorted
    /// ASCENDING; the `ref` stays EMPTY and the confidence is
    /// [`CONF_PENDING`].
    Pending(Vec<String>),
    /// No candidate: `ref` stays EMPTY and the confidence is [`CONF_ORPHAN`].
    Orphan,
}

/// One annotated mail after linking: the annotation with its FINAL `ref` and
/// confidence assigned, plus how the link was decided.
#[derive(Debug)]
pub struct LinkedMail {
    /// The annotation of the mail fact, with `ref` and `confidence` assigned
    /// by the link outcome: [`CONF_LINKED`] with the candidate's id when
    /// [`LinkStatus::Linked`], [`CONF_PENDING`] or [`CONF_ORPHAN`] with `ref`
    /// left EMPTY otherwise. All other fields (id, text, items, source) are
    /// unchanged from the extraction.
    pub annotation: Annotation,
    /// The link outcome — see [`LinkStatus`].
    pub status: LinkStatus,
}

/// Link every annotated mail to the transactions, PURELY and DETERMINISTICALLY.
///
/// The output has the SAME LENGTH AND ORDER as the `mail` input; for each
/// fact the candidate set is every transaction with an equal amount, a date
/// within [`DATE_WINDOW_DAYS`] days (inclusive) of the fact date, and a
/// passing counterparty check (see [`hint_satisfied`]). Exactly one candidate
/// links ( [`LinkStatus::Linked`], `ref` = that transaction id,
/// [`CONF_LINKED`]); several keep the annotation pending with the candidate
/// ids sorted ascending ([`LinkStatus::Pending`], `ref` EMPTY,
/// [`CONF_PENDING`]); none leave an orphan ([`LinkStatus::Orphan`], `ref`
/// EMPTY, [`CONF_ORPHAN`]).
///
/// Nothing here writes: `annotations.csv` appending is plan item 4.8; the
/// input is never touched and no clock, id generator, or file participates.
pub fn auto_link(mail: &[AnnotatedMail], transactions: &[Transaction]) -> Vec<LinkedMail> {
    let mut linked: Vec<LinkedMail> = Vec::new();
    for fact in mail.iter() {
        // A fact whose date is not canonical can never match: the candidate
        // set stays empty and the fact degrades to an orphan, never a panic
        // (mail_annotate hands out exactly such an empty date for a message
        // without a usable Date header).
        let candidates: Vec<String> = match civil_days(&fact.date) {
            None => { Vec::new() }
            Some(fact_days) => {
                let mut candidates: Vec<String> = Vec::new();
                for transaction in transactions.iter() {
                    if is_candidate(fact, transaction, fact_days) {
                        candidates.push(transaction.id.clone());
                    }
                }
                // Ascending by id, so the Pending payload is deterministic
                // regardless of the order the store handed the candidates in.
                candidates.sort();
                candidates
            }
        };
        if candidates.len() == 1 {
            let reference = candidates[0].clone();
            linked.push(LinkedMail {
                annotation: linked_annotation(fact, reference, CONF_LINKED),
                status: LinkStatus::Linked,
            });
            continue;
        }
        if candidates.len() > 1 {
            linked.push(LinkedMail {
                annotation: linked_annotation(fact, String::new(), CONF_PENDING),
                status: LinkStatus::Pending(candidates),
            });
            continue;
        }
        linked.push(LinkedMail {
            annotation: linked_annotation(fact, String::new(), CONF_ORPHAN),
            status: LinkStatus::Orphan,
        });
    }
    return linked;
}

/// True when `transaction` is a candidate for mail fact `fact`, given the
/// fact's day count: the date difference (transaction days minus fact days)
/// must lie within `-DATE_WINDOW_DAYS ..= DATE_WINDOW_DAYS` (the boundaries
/// included), the amounts must be EQUAL in minor units, and the counterparty
/// check ([`hint_satisfied`]) must pass.
///
/// A transaction with a non-canonical date has no candidate status:
/// `from_stored` normally guarantees canonical input, but the guard keeps
/// this function total for any Transaction an unrevised caller hands in.
fn is_candidate(fact: &AnnotatedMail, transaction: &Transaction, fact_days: i64) -> bool {
    let transaction_days = match civil_days(&transaction.date) {
        None => { return false; }
        Some(days) => { days }
    };
    // Both day counts lie in the range civil-days math over years 0001-9999
    // produces (roughly ±3.7 million days), so the difference can never
    // approach the i64 bounds and the subtraction cannot overflow.
    let difference = transaction_days - fact_days;
    if difference > DATE_WINDOW_DAYS || difference < -DATE_WINDOW_DAYS {
        return false;
    }
    if transaction.amount != fact.amount {
        return false;
    }
    return hint_satisfied(fact);
}

/// The counterparty check of decision 5: the fact's `counterparty_hint` is a
/// substring (case-INSENSITIVE, i.e. compared after ASCII-and-Unicode
/// lower-casing) of the annotation's `text` or of any of its `items`. An
/// EMPTY hint passes VACUOUSLY — such a fact (e.g. a receipt without a
/// `Grund:` line) is matched on date and amount alone.
fn hint_satisfied(fact: &AnnotatedMail) -> bool {
    let hint = fact.counterparty_hint.to_lowercase();
    if hint.len() == 0 {
        return true;
    }
    if fact.annotation.text.to_lowercase().contains(&hint) {
        return true;
    }
    for item in fact.annotation.items.iter() {
        if item.to_lowercase().contains(&hint) {
            return true;
        }
    }
    return false;
}

/// The annotation as the link leaves it: every field of the extracted
/// annotation CLONED unchanged except `r#ref` and `confidence` — the `ref`
/// only ever set for the unique-candidate case, empty for pending and orphan
/// (`data-files.md` renders an empty `ref` as the orphan/pending form), and
/// the confidence the final one of decision 5, taken through
/// [`crate::domain::annotation::clamp_confidence`] like every stored
/// confidence is.
fn linked_annotation(fact: &AnnotatedMail, reference: String, confidence: f64) -> Annotation {
    return Annotation {
        id: fact.annotation.id.clone(),
        r#ref: reference,
        text: fact.annotation.text.clone(),
        items: fact.annotation.items.clone(),
        source: fact.annotation.source.clone(),
        confidence: clamp_confidence(confidence),
    };
}

#[cfg(test)]
fn module_source() -> &'static str {
    // The path is relative to THIS file, i.e. it embeds this very module's
    // source; include_str! is a compile-time include, so the test that scans
    // it stays as I/O-free as the production code.
    return include_str!("link.rs");
}

/// The canonical `YYYY-MM-DD` date `date` as an integer day count, [`None`]
/// for anything that is not the canonical storage form (the empty fact date,
/// a stray `03.01.2024`, any garbage): the callers degrade such a date to
/// "never matching", never to a panic.
///
/// The strict shape mirrors what [`crate::domain::date::parse`] accepts for
/// the `-` layout and what `from_stored` guarantees for stored transaction
/// dates: exactly four year digits (the year 0000 is as invalid to
/// `domain::date` as it is here), two month digits `01..12`, two day digits
/// `01..days_in_month`, so an impossible calendar day like `2024-02-30` is
/// also treated as non-canonical and never matches.
fn civil_days(date: &str) -> Option<i64> {
    let bytes = date.as_bytes();
    if bytes.len() != 10 {
        return None;
    }
    if bytes[4] != b'-' || bytes[7] != b'-' {
        return None;
    }
    let digits = [bytes[0], bytes[1], bytes[2], bytes[3], bytes[5], bytes[6], bytes[8], bytes[9]];
    for digit in digits.iter() {
        if !digit.is_ascii_digit() {
            return None;
        }
    }
    let year = digit_of(digits[0]) * 1000 + digit_of(digits[1]) * 100 + digit_of(digits[2]) * 10 + digit_of(digits[3]);
    let month = digit_of(digits[4]) * 10 + digit_of(digits[5]);
    let day = digit_of(digits[6]) * 10 + digit_of(digits[7]);
    if year == 0 {
        return None;
    }
    if month < 1 || month > 12 {
        return None;
    }
    if day < 1 || day > days_in_month(year, month) {
        return None;
    }
    return Some(days_from_civil(year, month, day));
}

/// The value of the ASCII digit `byte` as an `i64`; the caller guarantees it
/// is a digit, and no `as` cast appears (decision 21 and the Phase 2/3
/// convention).
fn digit_of(byte: u8) -> i64 {
    return i64::from(byte - b'0');
}

/// Days from 1970-01-01 to the proleptic Gregorian date `y-m-d` (m `1..12`,
/// d `1..days_in_month`): the civil-days algorithm, pure integer math and the
/// only reason a date ever becomes comparable (decision 21 — `core` has no
/// date crate and this module adds none).
///
/// Only validated dates ever reach this function: years `0001..9999` make the
/// shifted year below non-negative, so the `/ 400` division floors towards
/// zero correctly, and every intermediate stays far inside the `i64` range.
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    // The year starts in March: the two-month shift is what makes the
    // leap-day rule fall at the end of the shifted year.
    let shifted_year = if m <= 2 { y - 1 } else { y };
    let era = shifted_year / 400;
    let year_of_era = shifted_year - era * 400; // [0, 399]
    let month_from_march = if m > 2 { m - 3 } else { m + 9 }; // [0, 11]
    let day_of_year = (153 * month_from_march + 2) / 5 + d - 1; // [0, 365]
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year; // [0, 146096]
    return era * 146097 + day_of_era - 719468;
}

/// Whether year `year` is a Gregorian leap year: divisible by 400, or by 4
/// but not 100 (the same rule [`crate::domain::date`] applies).
fn is_leap(year: i64) -> bool {
    return (year % 4 == 0 && year % 100 != 0) || year % 400 == 0;
}

/// Number of days in `month` (`1..12`) of `year`.
fn days_in_month(year: i64, month: i64) -> i64 {
    let common: i64 = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => { 31 }
        4 | 6 | 9 | 11 => { 30 }
        _ => { 28 }
    };
    if month == 2 && is_leap(year) {
        return 29;
    }
    return common;
}

// ---------------------------------------------------------------------------
// Tests. Everything is built INLINE from `domain::transaction::from_stored`
// and inline AnnotatedMail values: this module performs no I/O, so it must
// not create a scratch directory or read testdata/ (the pinned fixture pair
// 80c63211a8 / 80c63211a8#2 is reproduced field by field from the plan
// context, not read from disk). Amounts follow amendment A5: mail fact
// amounts keep the printed sign — no blanket negation anywhere.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::auto_link;
    use super::CONF_LINKED;
    use super::CONF_ORPHAN;
    use super::CONF_PENDING;
    use super::DATE_WINDOW_DAYS;
    use super::LinkStatus;
    use super::LinkedMail;
    use crate::domain::annotation::Annotation;
    use crate::domain::transaction::Transaction;
    use crate::domain::transaction::from_stored;
    use crate::technical::mail_annotate::AnnotatedMail;
    use crate::technical::mail_annotate::CONF_EXTRACT;

    /// A transaction built exactly as the store reader of plan item 2.3
    /// builds one — canonical date, stored minor-unit amount; the fixture
    /// shapes come from the plan context's bank_a/bank_b rows.
    fn stored_tx(id: &str, date: &str, amount: i64, description: &str, source: &str) -> Transaction {
        return match from_stored(id, date, amount, "EUR", description, "", source) {
            Ok(transaction) => { transaction }
            Err(reason) => { panic!("test transaction {id} @{date} must build: {reason:?}"); }
        };
    }

    /// One annotated mail as plan item 4.6 hands it out: EMPTY `ref`,
    /// [`CONF_EXTRACT`], no items unless the caller adds them.
    fn mail(id: &str, date: &str, amount: i64, hint: &str, text: &str) -> AnnotatedMail {
        return mail_with_items(id, date, amount, hint, text, Vec::new());
    }

    /// [`mail`] with an explicit item list for the items-substring path.
    fn mail_with_items(id: &str, date: &str, amount: i64, hint: &str, text: &str, items: Vec<String>) -> AnnotatedMail {
        return AnnotatedMail {
            annotation: Annotation {
                id: id.to_string(),
                r#ref: String::new(),
                text: text.to_string(),
                items: items,
                source: "test.mbox".to_string(),
                confidence: CONF_EXTRACT,
            },
            date: date.to_string(),
            amount: amount,
            counterparty_hint: hint.to_string(),
        };
    }

    /// The bank_a row the unique-match tests are built around.
    fn rewe_row() -> Transaction {
        return stored_tx("39945b2fed", "2024-01-03", -3999, "REWE MARKT 12", "bank_a.pdf");
    }

    #[test]
    fn constants_are_exactly_the_pinned_numbers() {
        // Decision 5, pinned to the letter by the task.
        assert!(DATE_WINDOW_DAYS == 3);
        assert!(CONF_LINKED == 0.9);
        assert!(CONF_PENDING == 0.4);
        assert!(CONF_ORPHAN == 0.0);
    }

    #[test]
    fn unique_match_high_confidence() {
        // A paypal-shaped fact: date 2024-01-03, amount -3999 (A5: printed
        // negative sign), hint REWE MARKET. Exactly one transaction matches;
        // the second row is off by amount, the third by 10 days of date.
        let mails = [mail("a0a0a0a001", "2024-01-03", -3999, "REWE MARKET", "PayPal payment to REWE MARKET")];
        let transactions = [
            rewe_row(),
            stored_tx("f596357483", "2024-01-03", -5990, "DINNER ZUR LEITZ", "bank_a.pdf"),
            stored_tx("3d7478d7cc", "2024-01-13", -3999, "BOOKS ONLINE", "bank_a.pdf"),
        ];
        let linked = auto_link(&mails, &transactions);
        assert!(linked.len() == 1, "one mail in, one LinkedMail out: {linked:?}");
        assert!(linked[0].status == LinkStatus::Linked, "status: {:?}", linked[0].status);
        assert!(linked[0].annotation.r#ref == "39945b2fed", "ref names the unique candidate: {}", linked[0].annotation.r#ref);
        assert!(linked[0].annotation.confidence == CONF_LINKED, "confidence: {}", linked[0].annotation.confidence);
        // Every other field survived unchanged.
        assert!(linked[0].annotation.id == "a0a0a0a001", "id: {}", linked[0].annotation.id);
        assert!(linked[0].annotation.source == "test.mbox", "source: {}", linked[0].annotation.source);
    }

    #[test]
    fn two_candidates_lower_confidence_both_recorded() {
        // The bank_b2 duplicate PAIR — two distinct ids, identical fields
        // (80c63211a8 and 80c63211a8#2). They are TWO candidates even though
        // every field but the id is equal. Given in reverse order on purpose:
        // the Pending payload is sorted ascending, not input order.
        let mails = [mail("b1b1b1b101", "2024-03-05", -1500, "REWE MARKET", "PayPal payment to REWE MARKET")];
        let transactions = [
            stored_tx("80c63211a8#2", "2024-03-05", -1500, "REWE MARKT 12", "bank_b2.pdf"),
            stored_tx("80c63211a8", "2024-03-05", -1500, "REWE MARKT 12", "bank_b2.pdf"),
        ];
        let linked = auto_link(&mails, &transactions);
        assert!(linked.len() == 1, "{linked:?}");
        let expected = vec!["80c63211a8".to_string(), "80c63211a8#2".to_string()];
        assert!(linked[0].status == LinkStatus::Pending(expected), "both candidates recorded ascending: {:?}", linked[0].status);
        assert!(linked[0].annotation.r#ref == *"", "the ref stays EMPTY while pending: {:?}", linked[0].annotation.r#ref);
        assert!(linked[0].annotation.confidence == CONF_PENDING, "confidence: {}", linked[0].annotation.confidence);
    }

    #[test]
    fn no_candidate_orphan() {
        // Date and amount match a transaction, but the hint is in neither the
        // text nor the items: zero candidates ⇒ orphan at 0.0.
        let mails = [mail("c2c2c2c201", "2024-01-03", -3999, "NOTHING MATCHES", "PayPal payment to REWE MARKET")];
        let transactions = [rewe_row()];
        let linked = auto_link(&mails, &transactions);
        assert!(linked.len() == 1, "{linked:?}");
        assert!(linked[0].status == LinkStatus::Orphan, "status: {:?}", linked[0].status);
        assert!(linked[0].annotation.r#ref == *"", "orphan ref: {:?}", linked[0].annotation.r#ref);
        assert!(linked[0].annotation.confidence == CONF_ORPHAN, "confidence: {}", linked[0].annotation.confidence);
    }

    #[test]
    fn window_boundaries() {
        // 2024 is a leap year: Feb 28 -> Mar 2 are 3 days (Feb 29 exists),
        // Feb 27 -> Mar 2 are 4. The window is inclusive at ±3, and ±4 is
        // outside. In leap-year terms this one test also proves the civil-days
        // math counts the extra day.
        let mails = [
            mail("d3d3d3d301", "2024-02-28", -1500, "REWE MARKET", "PayPal payment to REWE MARKET"),
            mail("d3d3d3d302", "2024-02-27", -1500, "REWE MARKET", "PayPal payment to REWE MARKET"),
        ];
        let transactions = [stored_tx("80c63211a8", "2024-03-02", -1500, "REWE MARKT 12", "bank_b2.pdf")];
        let linked = auto_link(&mails, &transactions);
        assert!(linked.len() == 2, "{linked:?}");
        assert!(linked[0].status == LinkStatus::Linked, "3 days inside the window: {:?}", linked[0].status);
        assert!(linked[0].annotation.r#ref == "80c63211a8", "ref: {}", linked[0].annotation.r#ref);
        assert!(linked[1].status == LinkStatus::Orphan, "4 days outside the window: {:?}", linked[1].status);
        assert!(linked[1].annotation.confidence == CONF_ORPHAN, "orphan confidence: {}", linked[1].annotation.confidence);
    }

    #[test]
    fn unique_match_on_2024_02_29_vs_2024_02_27() {
        // The leap-year window math the task pins: fact 2024-02-26,
        // transaction 2024-02-29 ⇒ exactly 3 days ⇒ match. The same fact one
        // day earlier (2024-02-25 ⇒ 4 days) must NOT match — without counting
        // Feb 29, both would land on the wrong side of the boundary.
        let mails = [
            mail("e4e4e4e401", "2024-02-26", -1500, "REWE MARKET", "PayPal payment to REWE MARKET"),
            mail("e4e4e4e402", "2024-02-25", -1500, "REWE MARKET", "PayPal payment to REWE MARKET"),
        ];
        let transactions = [stored_tx("a86d11da9a", "2024-02-29", -1500, "ALDI 4711", "bank_b1.pdf")];
        let linked = auto_link(&mails, &transactions);
        assert!(linked[0].status == LinkStatus::Linked, "2024-02-26 -> 2024-02-29 is 3 days: {:?}", linked[0].status);
        assert!(linked[0].annotation.confidence == CONF_LINKED, "confidence: {}", linked[0].annotation.confidence);
        assert!(linked[1].status == LinkStatus::Orphan, "2024-02-25 -> 2024-02-29 is 4 days: {:?}", linked[1].status);
    }

    #[test]
    fn amount_off_by_one_minor_unit_is_no_candidate() {
        // The amount must match EXACTLY (decision 5): -4000 against a fact
        // of -3999 is not a candidate, whatever the date does.
        let mails = [mail("f5f5f5f501", "2024-01-03", -3999, "REWE MARKET", "PayPal payment to REWE MARKET")];
        let transactions = [stored_tx("39945b2fed", "2024-01-03", -4000, "REWE MARKT 12", "bank_a.pdf")];
        let linked = auto_link(&mails, &transactions);
        assert!(linked[0].status == LinkStatus::Orphan, "1 minor unit off: {:?}", linked[0].status);
        assert!(linked[0].annotation.confidence == CONF_ORPHAN, "confidence: {}", linked[0].annotation.confidence);
    }

    #[test]
    fn counterparty_substring_is_case_insensitive() {
        // A mixed-case hint against an upper-case annotation text is a hit,
        // and it may live in an ITEM rather than the text; a hint that is in
        // neither leaves the fact an orphan.
        let hit = mail("a6a6a6a601", "2024-01-03", -3999, "reWe marKet", "PayPal payment to REWE MARKET");
        let via_item = mail_with_items(
            "a6a6a6a602",
            "2024-01-03",
            -3999,
            "butter",
            "Invoice items from attached x.pdf",
            vec!["2024-01-03 REWE MARKT 12 -39,99 €".to_string(), "Bread and BUTTER".to_string()],
        );
        let miss = mail("a6a6a6a603", "2024-01-03", -3999, "LIDL", "PayPal payment to REWE MARKET");
        let mails = [hit, via_item, miss];
        let transactions = [rewe_row()];
        let linked = auto_link(&mails, &transactions);
        assert!(linked[0].status == LinkStatus::Linked, "case-insensitive substring in the text: {:?}", linked[0].status);
        assert!(linked[1].status == LinkStatus::Linked, "the hint may live in an item: {:?}", linked[1].status);
        assert!(linked[2].status == LinkStatus::Orphan, "a substring miss is not a candidate: {:?}", linked[2].status);
    }

    #[test]
    fn empty_hint_matches_vacuously() {
        // A receipt without a Grund: line has an empty hint: the counterparty
        // check passes vacuously, so date+amount alone decide.
        let mails = [mail("a7a7a7a701", "2024-01-03", -3999, "", "PayPal payment to")];
        let transactions = [rewe_row()];
        let linked = auto_link(&mails, &transactions);
        assert!(linked[0].status == LinkStatus::Linked, "empty hint passes vacuously: {:?}", linked[0].status);
        assert!(linked[0].annotation.confidence == CONF_LINKED, "confidence: {}", linked[0].annotation.confidence);
    }

    #[test]
    fn fact_with_empty_or_non_canonical_date_never_matches() {
        // The documented degradation: an empty fact date (no usable Date
        // header) or any non-canonical text can NEVER be a candidate, even
        // when amount and hint fit the transaction exactly.
        let mails = [
            mail("a8a8a8a801", "", -3999, "REWE MARKET", "PayPal payment to REWE MARKET"),
            mail("a8a8a8a802", "03.01.2024", -3999, "REWE MARKET", "PayPal payment to REWE MARKET"),
            mail("a8a8a8a803", "garbage", -3999, "REWE MARKET", "PayPal payment to REWE MARKET"),
            mail("a8a8a8a804", "2024-02-30", -3999, "REWE MARKET", "PayPal payment to REWE MARKET"),
        ];
        let transactions = [rewe_row()];
        let linked = auto_link(&mails, &transactions);
        assert!(linked.len() == 4, "{linked:?}");
        for one in linked.iter() {
            assert!(one.status == LinkStatus::Orphan, "an unusable fact date never matches: {:?}", one.status);
            assert!(one.annotation.r#ref == *"", "ref: {:?}", one.annotation.r#ref);
            assert!(one.annotation.confidence == CONF_ORPHAN, "confidence: {}", one.annotation.confidence);
        }
    }

    #[test]
    fn deterministic_output_order() {
        // Same LENGTH AND ORDER as the input, and identical repeated runs:
        // no clock, no counter, no file participates. The Pending payload is
        // sorted, so even the candidate order from the store cannot leak in.
        let mails = [
            mail("b9b9b9b901", "2024-01-03", -3999, "REWE MARKET", "PayPal payment to REWE MARKET"),
            mail("b9b9b9b902", "2024-03-05", -1500, "REWE MARKET", "PayPal payment to REWE MARKET"),
            mail("b9b9b9b903", "2024-01-03", -3999, "NOTHING MATCHES", "PayPal payment to REWE MARKET"),
        ];
        let transactions = [
            rewe_row(),
            stored_tx("80c63211a8#2", "2024-03-05", -1500, "REWE MARKT 12", "bank_b2.pdf"),
            stored_tx("80c63211a8", "2024-03-05", -1500, "REWE MARKT 12", "bank_b2.pdf"),
        ];
        let first = auto_link(&mails, &transactions);
        let second = auto_link(&mails, &transactions);
        assert!(first.len() == 3, "{first:?}");
        for (index, one) in first.iter().enumerate() {
            assert!(one.annotation.id == mails[index].annotation.id, "output {index} keeps the input's annotation: {}", one.annotation.id);
        }
        assert!(first[0].status == LinkStatus::Linked, "first: {:?}", first[0].status);
        assert!(first[2].status == LinkStatus::Orphan, "third: {:?}", first[2].status);
        assert!(format!("{first:?}") == format!("{second:?}"), "a repeated run is identical");
        // A reversed transaction order changes nothing, not even the
        // ascending Pending payload. (Built literal-first: Transaction is
        // not Clone, and hand-building it is exactly what the store reader
        // does anyway.)
        let reversed = [
            stored_tx("80c63211a8", "2024-03-05", -1500, "REWE MARKT 12", "bank_b2.pdf"),
            stored_tx("80c63211a8#2", "2024-03-05", -1500, "REWE MARKT 12", "bank_b2.pdf"),
            rewe_row(),
        ];
        let third = auto_link(&mails, &reversed);
        assert!(format!("{first:?}") == format!("{third:?}"), "the store's order cannot leak into the result");
    }

    #[test]
    fn module_performs_no_io() {
        // The compile-level proof: this COERCE pins auto_link's pure
        // signature — inputs by shared borrow, so it cannot capture or write
        // anything outside its arguments.
        let _: fn(&[AnnotatedMail], &[Transaction]) -> Vec<LinkedMail> = auto_link;
        // The assert-by-absence: the module's OWN source (embedded at
        // compile time by include_str!, so this test itself performs no I/O)
        // names no file-system module. The search keys are built at RUN TIME
        // so the scan target never appears in the scanned source.
        let source = super::module_source();
        let forbidden = [format!("{}::{}", "std", "fs"), format!("{}::{}", "std", "net"), format!("{}::{}", "std", "process")];
        for (index, line) in source.lines().enumerate() {
            for word in forbidden.iter() {
                assert!(!line.contains(word.as_str()), "link.rs must not name any I/O, line {}: {}", index + 1, line);
            }
        }
    }
}
