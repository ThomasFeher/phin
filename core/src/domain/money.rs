//! Money: amounts in the single settlement currency (EUR) as `i64` minor units.
//!
//! An amount is a count of cents, never a floating point number: this module
//! contains no floating point literals or conversions. Parsing accepts the two
//! layouts the fixture bank statements use — layout A writes `1.234,56 €` (dot
//! thousands separator, comma decimal separator) and layout B writes `-1234.56
//! EUR` (dot decimal separator, no thousands separator). The rule that
//! disambiguates a lone dot is documented on [`parse`]; every input that does
//! not fit it yields a typed error rather than a panic.

/// Reason an amount string could not be read as a count of cents.
#[derive(Debug)]
pub enum ParseError {
    /// Empty, or a character that is not a digit, a separator or a currency
    /// marker; also a thousands grouping that is not groups of three digits.
    Malformed,
    /// The digits do not fit in an `i64` of minor units.
    Overflow,
}

/// Parse an amount string into minor units (cents) of the settlement currency.
///
/// Accepts optional surrounding spaces, an optional leading `-`, an optional
/// trailing `€` or `EUR` marker, dot thousands separators and a comma decimal
/// separator: `"1.234,56 €"` → `123456`, `"-12,00"` → `-1200`, `"0,00"` → `0`,
/// `"1.000"` → `100000`.
///
/// A comma, when present, is always the decimal separator (layout A). Without a
/// comma, dots are thousands separators only when they split the digits into
/// valid groups of three; otherwise the last dot is the decimal separator,
/// which is how layout B's `-1234.56 EUR` is read as `-123456`. `"1.000"`
/// therefore means one thousand euros, not one euro.
pub fn parse(text: &str) -> Result::<i64, ParseError> {
    let stripped = strip_currency(text.trim());
    let body = stripped.as_str();
    if body.len() == 0 {
        return Err(ParseError::Malformed);
    }
    let negative = body.starts_with("-");
    let digits_part = if negative { &body[1..] } else { body };
    if digits_part.len() == 0 {
        return Err(ParseError::Malformed);
    }

    let comma_at = digits_part.rfind(',');
    let dot_at = digits_part.rfind('.');
    let split_at = match comma_at {
        Some(at) => { Some(at) }
        None => {
            match dot_at {
                Some(at) => { if groups_are_valid(digits_part) { None } else { Some(at) } }
                None => { None }
            }
        }
    };

    let whole: &str;
    let frac: &str;
    match split_at {
        None => { whole = digits_part; frac = ""; }
        Some(at) => { whole = &digits_part[..at]; frac = &digits_part[(at + 1)..]; }
    }

    if !all_digits(whole, true) || !all_digits(frac, false) || frac.len() > 2 {
        return Err(ParseError::Malformed);
    }
    if !groups_are_valid(whole) {
        return Err(ParseError::Malformed);
    }

    let units = to_minor(whole, frac);
    match units {
        None => { return Err(ParseError::Overflow); }
        Some(value) => {
            if negative {
                match value.checked_neg() {
                    None => { return Err(ParseError::Overflow); }
                    Some(negated) => { return Ok(negated); }
                }
            }
            return Ok(value);
        }
    }
}

/// Render minor units back into the canonical layout A form, e.g. `-123456` →
/// `"-1.234,56 €"`. Printing then parsing is the identity on canonical strings.
pub fn print(amount: i64) -> String {
    let negative = amount < 0;
    let magnitude = if negative { 0u64.wrapping_sub(amount as u64) } else { amount as u64 };
    let whole = magnitude / 100;
    let cents = magnitude % 100;
    let grouped = group_thousands(whole);
    let cents_text = format!("{cents:02}");
    let sign = if negative { "-" } else { "" };
    return format!("{sign}{grouped},{cents_text} €");
}

/// Drop the optional trailing `€` or `EUR` marker and the space around it.
fn strip_currency(text: &str) -> String {
    let body = text.trim();
    if body.ends_with("€") {
        return body[..body.len() - "€".len()].trim().to_string();
    }
    if body.ends_with("EUR") {
        return body[..body.len() - "EUR".len()].trim().to_string();
    }
    return body.to_string();
}

/// True when every character is an ASCII digit, dots being allowed only when
/// `allow_dots` is set (empty strings are vacuously true).
fn all_digits(text: &str, allow_dots: bool) -> bool {
    for byte in text.bytes() {
        if is_digit(byte) {
            continue;
        }
        if allow_dots && byte == b'.' {
            continue;
        }
        return false;
    }
    return true;
}

/// True when one byte is an ASCII digit.
fn is_digit(byte: u8) -> bool {
    return byte >= b'0' && byte <= b'9';
}

/// Validate the dot thousands grouping: a leading group of one to three digits
/// followed by groups of exactly three digits. Dot-free digits are valid.
fn groups_are_valid(text: &str) -> bool {
    if text.len() == 0 || !all_digits(text, true) {
        return false;
    }
    if !text.contains('.') {
        return true;
    }
    let mut group_size = 0;
    let mut dots_seen = 0;
    for byte in text.bytes() {
        if byte != b'.' {
            group_size = group_size + 1;
            continue;
        }
        dots_seen = dots_seen + 1;
        if group_size < 1 || group_size > 3 {
            return false;
        }
        group_size = 0;
    }
    if dots_seen > 0 && group_size != 3 {
        return false;
    }
    return group_size >= 1 && group_size <= 3;
}

/// Combine the euro digits and the (at most two) decimal digits into minor
/// units, or `None` when the value does not fit an `i64`.
fn to_minor(whole: &str, frac: &str) -> Option::<i64> {
    let plain = whole.replace(".", "").to_string();
    if plain.len() == 0 {
        return None;
    }
    let mut combined = plain + frac;
    if frac.len() == 0 {
        combined = combined + "00";
    }
    if frac.len() == 1 {
        combined.push('0');
    }

    let mut value = 0i64;
    for byte in combined.bytes() {
        let digit = (byte - b'0') as i64;
        let shifted = match value.checked_mul(10) {
            None => { return None; }
            Some(moved) => { moved.checked_add(digit) }
        };
        match shifted {
            None => { return None; }
            Some(total) => { value = total; }
        }
    }
    return Some(value);
}

/// Insert dot thousands separators into a non-negative integer.
fn group_thousands(whole: u64) -> String {
    let digits = whole.to_string();
    let mut shown = Vec::<String>::new();
    let head = digits.len() % 3;
    let mut index = 0;
    if head > 0 {
        shown.push(digits[..head].to_string());
        index = head;
    }
    while index < digits.len() {
        if index > 0 {
            shown.push(".".to_string());
        }
        shown.push(digits[index..index + 3].to_string());
        index = index + 3;
    }
    return shown.join("");
}

#[test]
fn parses_layout_a_vectors() {
    assert!(parse("1.234,56 €").unwrap() == 123456);
    assert!(parse("-12,00").unwrap() == -1200);
    assert!(parse("0,00").unwrap() == 0);
    assert!(parse("1.000").unwrap() == 100000);
}

#[test]
fn parses_layout_b_vector() {
    assert!(parse("-1234.56 EUR").unwrap() == -123456);
    assert!(parse("1234.56 EUR").unwrap() == 123456);
}

#[test]
fn garbage_yields_typed_error() {
    assert!(parse("12,34 €x").is_err());
    assert!(parse("").is_err());
    assert!(parse("-").is_err());
    assert!(parse("1,2,3").is_err());
    assert!(parse("1.2345").is_err());
    assert!(parse("1,234").is_err());
    assert!(parse("12 34").is_err());
}

#[test]
fn overflow_is_typed() {
    assert!(parse("92233720368547758.08").is_err());
    assert!(parse("92233720368547758.07").unwrap() == 9223372036854775807);
}

#[test]
fn print_round_trip_is_identity() {
    for amount in [-123456, 0, 123456, 100, -5] {
        let text = print(amount);
        assert!(parse(&text).unwrap() == amount);
    }
    for text in ["-1.234,56 €", "0,00 €", "1.234,56 €", "1,00 €", "-0,05 €"] {
        assert!(print(parse(&text).unwrap()) == text);
    }
}

#[test]
fn sign_and_grouping_are_rendered() {
    assert!(print(-123456) == "-1.234,56 €");
    assert!(print(123456) == "1.234,56 €");
    assert!(print(100000000) == "1.000.000,00 €");
    assert!(print(5) == "0,05 €");
    assert!(print(0) == "0,00 €");
}
