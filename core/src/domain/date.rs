//! Dates as normalized `YYYY-MM-DD` strings.
//!
//! The fixture bank statements write dates in two layouts: layout A
//! (`bank_a.pdf`) uses `DD.MM.YYYY`, layout B (`bank_b1.pdf`, `bank_b2.pdf`)
//! uses `YYYY-MM-DD`. Those are the only accepted input formats. Slash-form
//! dates such as `01/02/2024` are ambiguous between day-first and month-first
//! reading and occur in no fixture layout, so they are rejected with an
//! explicit [`ParseError.Ambiguous`] instead of being guessed. Inputs that are
//! well-formed but impossible on the calendar (e.g. `31.02.2024`) get
//! [`ParseError.InvalidDate`]; everything else gets [`ParseError.Malformed`].
//! Every rejection is a typed error, never a panic.

/// Reason a date string could not be read as a normalized date.
#[derive(Debug, Eq, PartialEq)]
pub enum ParseError {
    /// Not three numeric fields in a recognized layout; also a field with the
    /// wrong number of digits or an unrecognized separator.
    Malformed,
    /// A slash-separated date such as `01/02/2024`: day-first or month-first
    /// cannot be told apart, and no fixture layout uses this form, so it is
    /// never guessed.
    Ambiguous,
    /// The fields parse but name an impossible day of the month, e.g.
    /// `31.02.2024` or `29.02.2025`.
    InvalidDate,
}

/// Parse a date string into the canonical `YYYY-MM-DD` storage form.
///
/// Accepts the two fixture layouts: `DD.MM.YYYY` (layout A) and `YYYY-MM-DD`
/// (layout B). `"03.01.2024"` and `"2024-01-03"` both yield `"2024-01-03"`.
/// Fields must have exactly the widths the layout prescribes; the calendar
/// date must exist, leap years included.
///
/// Surrounding whitespace is NOT tolerated: the input must be a strict token
/// in one of the accepted layouts, so `"2024-01-03 "` and `" 2024-01-03"` are
/// [`ParseError.Malformed`]. Whitespace stripping is the importer/technical
/// layer's job, not this parser's; leniency here would mask PDF text-extraction
/// noise.
pub fn parse(body: &str) -> Result::<String, ParseError> {
    if body.len() == 0 {
        return Err(ParseError::Malformed);
    }

    if body.contains('/') {
        let parts = split_on(body, '/');
        let mut numeric = parts.len() == 3;
        for part in &parts {
            if part.len() == 0 || !all_digits(part) {
                numeric = false;
            }
        }
        if numeric {
            return Err(ParseError::Ambiguous);
        }
        return Err(ParseError::Malformed);
    }

    let day: String;
    let month: String;
    let year: String;
    if body.contains('.') {
        let mut parts = split_on(body, '.');
        if parts.len() != 3 {
            return Err(ParseError::Malformed);
        }
        day = parts.remove(0);
        month = parts.remove(0);
        year = parts.remove(0);
    } else if body.contains('-') {
        let mut parts = split_on(body, '-');
        if parts.len() != 3 {
            return Err(ParseError::Malformed);
        }
        year = parts.remove(0);
        month = parts.remove(0);
        day = parts.remove(0);
    } else {
        return Err(ParseError::Malformed);
    }

    if day.len() != 2 || month.len() != 2 || year.len() != 4 {
        return Err(ParseError::Malformed);
    }
    for part in [&day, &month, &year] {
        if !all_digits(part) {
            return Err(ParseError::Malformed);
        }
    }

    let year_value = to_number(&year);
    let month_value = to_number(&month);
    let day_value = to_number(&day);

    if year_value == 0 {
        return Err(ParseError::InvalidDate);
    }
    if month_value < 1 || month_value > 12 {
        return Err(ParseError::InvalidDate);
    }
    if day_value < 1 || day_value > days_in_month(year_value, month_value) {
        return Err(ParseError::InvalidDate);
    }
    return Ok(format!("{year}-{month}-{day}"));
}

/// Render an internally stored date back into text.
///
/// The internal representation is already the canonical `YYYY-MM-DD` string,
/// so printing is the identity; `parse(print(s))` equals `s` for every
/// canonical string `s`, and `print(parse(t))` normalizes any accepted `t`.
pub fn print(date: &str) -> String {
    return date.to_string();
}

/// Split a string on every occurrence of `separator`.
fn split_on(text: &str, separator: char) -> Vec::<String> {
    let mut parts = Vec::<String>::new();
    let mut current = String::new();
    for character in text.chars() {
        if character == separator {
            parts.push(current);
            current = String::new();
            continue;
        }
        current.push(character);
    }
    parts.push(current);
    return parts;
}

/// True when the text is non-empty and every character is an ASCII digit.
fn all_digits(text: &str) -> bool {
    if text.len() == 0 {
        return false;
    }
    for byte in text.bytes() {
        if byte < b'0' || byte > b'9' {
            return false;
        }
    }
    return true;
}

/// Read a short digit string into a `u32`; the caller guarantees the digits.
fn to_number(text: &str) -> u32 {
    let mut value = 0u32;
    for byte in text.bytes() {
        value = value * 10 + (byte - b'0') as u32;
    }
    return value;
}

/// True for Gregorian leap years: divisible by 400, or by 4 but not by 100.
fn is_leap(year: u32) -> bool {
    return (year % 4 == 0 && year % 100 != 0) || year % 400 == 0;
}

/// Number of days in `month` (1..12) of `year`.
fn days_in_month(year: u32, month: u32) -> u32 {
    let table: [u32; 12] = [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    if month == 2 && is_leap(year) {
        return 29;
    }
    return table[(month - 1) as usize];
}

#[test]
fn parses_layout_a_dates() {
    assert!(parse("03.01.2024").unwrap() == "2024-01-03");
    assert!(parse("31.12.1999").unwrap() == "1999-12-31");
    assert!(parse("29.02.2024").unwrap() == "2024-02-29");
}

#[test]
fn parses_layout_b_dates() {
    assert!(parse("2024-01-03").unwrap() == "2024-01-03");
    assert!(parse("1999-12-31").unwrap() == "1999-12-31");
}

#[test]
fn ambiguous_slash_form_is_rejected_explicitly() {
    assert!(parse("01/02/2024").is_err());
    assert!(parse("01/02/2024").unwrap_err() == ParseError::Ambiguous);
    assert!(parse("2024/01/02").unwrap_err() == ParseError::Ambiguous);
}

#[test]
fn invalid_calendar_dates_are_rejected() {
    assert!(parse("31.02.2024").unwrap_err() == ParseError::InvalidDate);
    assert!(parse("29.02.2025").is_err());
    assert!(parse("2023-02-29").unwrap_err() == ParseError::InvalidDate);
    assert!(parse("2025-02-29").unwrap_err() == ParseError::InvalidDate);
    assert!(parse("00.01.2024").unwrap_err() == ParseError::InvalidDate);
    assert!(parse("01.13.2024").unwrap_err() == ParseError::InvalidDate);
    assert!(parse("0000-01-01").unwrap_err() == ParseError::InvalidDate);
}

#[test]
fn whitespace_padded_input_is_malformed() {
    assert!(parse("2024-01-03 ").unwrap_err() == ParseError::Malformed);
    assert!(parse(" 2024-01-03").unwrap_err() == ParseError::Malformed);
    assert!(parse("2024 . 01 . 03").unwrap_err() == ParseError::Malformed);
    assert!(parse("").unwrap_err() == ParseError::Malformed);
}

#[test]
fn garbage_yields_typed_error() {
    assert!(parse("").is_err());
    assert!(parse("garbage").is_err());
    assert!(parse("2024-1-3").is_err());
    assert!(parse("24-01-03").is_err());
    assert!(parse("2024.01.03").is_err());
    assert!(parse("01.02.24").is_err());
    assert!(parse("2024-01-03-05").is_err());
    assert!(parse("2024 01 03").is_err());
    assert!(parse("a/b/c").is_err());
    assert!(parse("2024-01-03 €").is_err());
}

#[test]
fn print_round_trip_is_identity() {
    for text in ["03.01.2024", "2024-01-03", "29.02.2024", "31.12.1999"] {
        let stored = parse(text).unwrap();
        assert!(print(&stored) == stored);
        assert!(parse(&print(&stored)).unwrap() == stored);
        if text.contains("-") {
            assert!(print(&parse(text).unwrap()) == text);
        }
    }
}
