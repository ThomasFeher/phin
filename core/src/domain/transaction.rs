//! A transaction: one normalized movement of money, as recorded in
//! `transactions.csv`.
//!
//! The struct mirrors the contract columns exactly: `id;date;amount;currency;
//! description;counterparty;source`. The category is NOT a field here — it is
//! derived from the rules and annotations, not stored here.
//! There are two ways to build one, one per caller: [`from_parts`] takes the
//! raw display-form text a bank parser produced and runs the `date` and
//! `amount` fields through [`core::domain::date`] and [`core::domain::money`];
//! [`from_stored`] takes the values exactly as `transactions.csv` stores them
//! — an already-canonical `YYYY-MM-DD` date and an `i64` minor-unit count —
//! and delegates the shared validation. Either way a stored transaction always
//! holds a canonical `YYYY-MM-DD` date and an `i64` amount in minor units.
//! Reading the CSV lines themselves, and hashing the `id`, belong to the
//! technical layer.

use super::date::parse as date_parse;
use super::date::ParseError as DateParseError;
use super::money::parse as money_parse;
use super::money::ParseError as MoneyParseError;

/// Reason a transaction could not be built from its raw text fields.
#[derive(Debug)]
pub enum BuildError {
    /// The date field was rejected by [`core::domain::date::parse`]; the reason
    /// is carried through unchanged.
    InvalidDate(DateParseError),
    /// The amount field was rejected by [`core::domain::money::parse`].
    InvalidAmount(MoneyParseError),
    /// [`from_stored`] was handed a date that names a real day but is not
    /// already in canonical `YYYY-MM-DD` storage form, e.g. `03.01.2024`; the
    /// stored form is never renormalized, so the row is rejected as-is.
    NonCanonicalDate,
}

/// One movement of money, with the sign convention that runs through all of
/// phin: `amount` is a count of minor units (cents) of the settlement currency,
/// never a float. `amount > 0` is income (money coming in, e.g. a salary or a
/// sale to the account owner), `amount < 0` is an expense (money spent from the
/// account), and `amount == 0` is neutral: neither income nor expense.
#[derive(Debug, Eq, PartialEq)]
pub struct Transaction {
    /// Opaque short content hash of the source record, plus an optional `#N`
    /// occurrence suffix for duplicates (e.g. `80c63211a8#2`). It is assigned
    /// by the technical layer (see `doc/implementation/plan.md` item 2.2)
    /// and only carried here.
    pub id: String,
    /// When the movement happened, in canonical `YYYY-MM-DD` storage form.
    pub date: String,
    /// The movement in minor units (cents) of the settlement currency; see the
    /// sign convention on the struct: positive is income, negative is expense,
    /// zero is neutral.
    pub amount: i64,
    /// ISO-4217 code of the currency the amount is counted in (e.g. `EUR`).
    /// Carried per record even though only one settlement currency is
    /// supported; a foreign code is rejected by the importer, not converted.
    pub currency: String,
    /// Merchant text as printed in the source document.
    pub description: String,
    /// Recognized counterparty name (e.g. `REWE MARKET`), empty when unknown.
    pub counterparty: String,
    /// Provenance: the source file the record was extracted from
    /// (e.g. `bank_a.pdf`).
    pub source: String,
}

/// Build a transaction from the raw display-form text of one parsed source
/// record — this is the constructor the importers of plan item 4.x must use.
/// Parameter order is the contract column order
/// `id;date;amount;currency;description;counterparty;source`
/// (doc/interfaces/data-files.md); the `description` and `counterparty` slots
/// are NOT interchangeable, so a caller that transposes them builds a
/// different transaction.
///
/// The `date` and `amount` texts are validated and normalized by the `date`
/// and `money` modules, so `"03.01.2024"` becomes `"2024-01-03"` and `"-39,99
/// €"` becomes the minor-unit count `-3999`. The remaining fields are carried
/// through as given: the `id` is already hashed by the technical layer and the
/// text fields are already decoded from the source line. After normalizing the
/// text fields this delegates to [`from_stored`]. A rejected date or amount is
/// a typed [`BuildError`], never a panic.
///
/// INVARIANT: `from_parts` accepts statement-printed text only; a stored
/// minor-unit count (e.g. the CSV field `-3999`) must NEVER be passed to it,
/// since `money::parse("-3999")` would yield `-399900`. The store reader of
/// plan item 2.3 (`core/src/technical/store_transactions.rs`, "reader into
/// `Transaction` values") reads `transactions.csv` lines and must build rows
/// with [`from_stored`] instead, which takes the stored i64 count as-is.
pub fn from_parts(id: &str, date: &str, amount: &str, currency: &str, description: &str, counterparty: &str, source: &str) -> Result::<Transaction, BuildError> {
    let stored_date = match date_parse(date) {
        Err(reason) => { return Err(BuildError::InvalidDate(reason)); }
        Ok(normalized) => { normalized }
    };
    let stored_amount = match money_parse(amount) {
        Err(reason) => { return Err(BuildError::InvalidAmount(reason)); }
        Ok(minor_units) => { minor_units }
    };
    return from_stored(id, &stored_date, stored_amount, currency, description, counterparty, source);
}

/// Build a transaction from the values exactly as `transactions.csv` stores
/// them — this is the constructor the store reader of plan item 2.3
/// (`core/src/technical/store_transactions.rs`, "reader into `Transaction` values")
/// must use, since the storage contract writes `amount` as an `i64` minor-unit
/// count and a canonical `YYYY-MM-DD` date, not display-form text.
/// Parameter order is the contract column order
/// `id;date;amount;currency;description;counterparty;source`
/// (doc/interfaces/data-files.md); the `description` and `counterparty` slots
/// are NOT interchangeable, so a caller that transposes them builds a
/// different transaction.
///
/// The `amount` is taken as the stored minor-unit count as-is; no textual
/// parsing happens, so the CSV field `-3999` stays `-3999`. The `date` must
/// already be canonical `YYYY-MM-DD`: it is round-tripped through
/// [`core::domain::date::parse`] and rejected with [`BuildError.NonCanonicalDate`]
/// if the round trip changes it (e.g. `03.01.2024`), and with the parser's own
/// reason if it does not parse at all. A rejected date is a typed [`BuildError`],
/// never a panic.
pub fn from_stored(id: &str, date: &str, amount: i64, currency: &str, description: &str, counterparty: &str, source: &str) -> Result::<Transaction, BuildError> {
    match date_parse(date) {
        Err(reason) => { return Err(BuildError::InvalidDate(reason)); }
        Ok(normalized) => {
            if normalized != date {
                return Err(BuildError::NonCanonicalDate);
            }
        }
    }
    return Ok(Transaction {
        id: id.to_string(),
        date: date.to_string(),
        amount: amount,
        currency: currency.to_string(),
        description: description.to_string(),
        counterparty: counterparty.to_string(),
        source: source.to_string(),
    });
}

impl Transaction {
    /// True when the transaction is income, that is when `amount > 0`.
    ///
    /// Expenses (`amount < 0`) and neutral records (`amount == 0`) are both false.
    pub fn is_income(&self) -> bool {
        return self.amount > 0;
    }
}

#[test]
fn builds_fixture_rows_of_both_signs() {
    let expense = from_parts("39945b2fed", "2024-01-03", "-39,99 €", "EUR", "REWE MARKT 12", "REWE MARKET", "bank_a.pdf");
    assert!(expense.unwrap().amount == -3999);
    let income = from_parts("dc0db110c4", "2024-01-15", "420,00 €", "EUR", "PAYMENT TO TOBIAS", "TAXI BERLIN", "bank_a.pdf");
    assert!(income.unwrap().amount == 42000);
}

#[test]
fn builds_normalize_the_layout_a_date() {
    let row = from_parts("140ef279a4", "11.02.2024", "-25,00 €", "EUR", "TAXI AIRPORT; NIGHT", "TAXI BERLIN", "bank_b1.pdf");
    let built = row.unwrap();
    assert!(built.date == "2024-02-11");
    assert!(built.amount == -2500);
    assert!(built.id == "140ef279a4");
    assert!(built.currency == "EUR");
    assert!(built.description == "TAXI AIRPORT; NIGHT");
    assert!(built.counterparty == "TAXI BERLIN");
    assert!(built.source == "bank_b1.pdf");
}

#[test]
fn is_income_covers_both_signs_and_zero() {
    let income = from_parts("dc0db110c4", "2024-01-15", "420,00 €", "EUR", "PAYMENT TO TOBIAS", "TAXI BERLIN", "bank_a.pdf").unwrap();
    assert!(income.is_income());
    let expense = from_parts("80c63211a8#2", "2024-03-05", "-15,00 €", "EUR", "REWE MARKT 12", "REWE MARKET", "bank_b2.pdf").unwrap();
    assert!(!expense.is_income());
    let neutral = from_parts("80c63211a8", "2024-03-05", "0,00 €", "EUR", "REWE MARKT 12", "REWE MARKET", "bank_b2.pdf").unwrap();
    assert!(neutral.amount == 0);
    assert!(!neutral.is_income());
}

#[test]
fn amount_and_date_survive_a_round_trip() {
    let row = from_parts("39945b2fed", "03.01.2024", "-39,99 €", "EUR", "REWE MARKT 12", "REWE MARKET", "bank_a.pdf").unwrap();
    assert!(super::money::print(row.amount) == "-39,99 €");
    assert!(money_parse(&super::money::print(row.amount)).unwrap() == row.amount);
    assert!(super::date::print(&row.date) == row.date);
    assert!(date_parse(&super::date::print(&row.date)).unwrap() == row.date);
}

#[test]
fn bad_date_and_amount_are_typed_errors() {
    let invalid_date = from_parts("39945b2fed", "31.02.2024", "-39,99 €", "EUR", "x", "y", "bank_a.pdf");
    match invalid_date {
        Err(BuildError::InvalidDate(DateParseError::InvalidDate)) => { }
        _ => { assert!(false); }
    }
    let ambiguous = from_parts("39945b2fed", "01/02/2024", "-39,99 €", "EUR", "x", "y", "bank_a.pdf");
    match ambiguous {
        Err(BuildError::InvalidDate(DateParseError::Ambiguous)) => { }
        _ => { assert!(false); }
    }
    let bad_amount = from_parts("39945b2fed", "2024-01-03", "12,34 €x", "EUR", "x", "y", "bank_a.pdf");
    match bad_amount {
        Err(BuildError::InvalidAmount(MoneyParseError::Malformed)) => { }
        _ => { assert!(false); }
    }
    let overflow = from_parts("39945b2fed", "2024-01-03", "92233720368547758.08", "EUR", "x", "y", "bank_a.pdf");
    match overflow {
        Err(BuildError::InvalidAmount(MoneyParseError::Overflow)) => { }
        _ => { assert!(false); }
    }
}

#[test]
fn from_stored_accepts_raw_csv_row_values() {
    let expense = from_stored("39945b2fed", "2024-01-03", -3999, "EUR", "REWE MARKT 12", "REWE MARKET", "bank_a.pdf");
    assert!(expense.unwrap().amount == -3999);
    let second_occurrence = from_stored("80c63211a8#2", "2024-03-05", -1500, "EUR", "REWE MARKT 12", "REWE MARKET", "bank_b2.pdf");
    let built = second_occurrence.unwrap();
    assert!(built.id == "80c63211a8#2");
    assert!(built.date == "2024-03-05");
    assert!(built.amount == -1500);
}

#[test]
fn from_stored_rejects_non_canonical_dates() {
    let not_canonical = from_stored("39945b2fed", "03.01.2024", -3999, "EUR", "x", "y", "bank_a.pdf");
    match not_canonical {
        Err(BuildError::NonCanonicalDate) => { }
        _ => { assert!(false); }
    }
    let impossible = from_stored("39945b2fed", "31.02.2024", -3999, "EUR", "x", "y", "bank_a.pdf");
    match impossible {
        Err(BuildError::InvalidDate(DateParseError::InvalidDate)) => { }
        _ => { assert!(false); }
    }
}

#[test]
fn from_parts_equals_from_stored_for_the_same_transaction() {
    let displayed = from_parts("39945b2fed", "03.01.2024", "-39,99 €", "EUR", "REWE MARKT 12", "REWE MARKET", "bank_a.pdf").unwrap();
    let stored = from_stored("39945b2fed", "2024-01-03", -3999, "EUR", "REWE MARKT 12", "REWE MARKET", "bank_a.pdf").unwrap();
    assert!(displayed == stored);
}

#[test]
fn description_and_counterparty_are_not_interchangeable() {
    let as_contract = from_stored("39945b2fed", "2024-01-03", -3999, "EUR", "REWE MARKT 12", "REWE MARKET", "bank_a.pdf").unwrap();
    let transposed = from_stored("39945b2fed", "2024-01-03", -3999, "EUR", "REWE MARKET", "REWE MARKT 12", "bank_a.pdf").unwrap();
    assert!(as_contract != transposed);
}

#[test]
fn equality_follows_every_field() {
    let first = from_parts("39945b2fed", "2024-01-03", "-39,99 €", "EUR", "REWE MARKT 12", "REWE MARKET", "bank_a.pdf").unwrap();
    let same = from_parts("39945b2fed", "03.01.2024", "-39.99", "EUR", "REWE MARKT 12", "REWE MARKET", "bank_a.pdf").unwrap();
    assert!(first == same);
    let other = from_parts("80c63211a8#2", "2024-01-03", "-39,99 €", "EUR", "REWE MARKT 12", "REWE MARKET", "bank_a.pdf").unwrap();
    assert!(first != other);
}
