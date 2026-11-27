//! Analysis results: one typed table value that every analysis, the renderer,
//! the plugin host and the `analyze` payload share (plan item 5.1).
//!
//! A table is a name, a header of typed [`Column`]s and rows of typed [`Cell`]s.
//! The cell types exist because of Q1 (exactness): a renderer or a plugin that
//! had to guess which column carries money would eventually print a wrong
//! number, so a [`Kind::Money`] cell holds an `i64` count of minor units and
//! nothing else (planning decision 11). A [`Kind::Count`] cell is never summed
//! as money and a [`Kind::Date`] cell is a canonical `YYYY-MM-DD` string, so the
//! analysis that produced a value — not the code that displays it — decides what
//! it means.
//!
//! NOTHING HERE FORMATS OR PARSES TEXT. Amount-to-text rendering belongs to
//! [`crate::domain::money::print`] (and only to it) at the moment a document is
//! produced; the JSON form of a table belongs to the analysis plugin host. This
//! module is a pure value type: it does no I/O, and it sums exactly, with
//! [`i64::checked_add`], because a silent wraparound would be a wrong number on
//! paper.
//!
//! Rows enter a [`Table`] only through [`add_row`], which checks the cell types
//! against the column types first. A rejected row never leaves a half-added row
//! behind, so a table that exists is type-consistent by construction.

/// What kind of value a [`Column`] holds; the shared vocabulary of the analysis
/// table, the plugin protocol's `column_types` list and the renderer.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum Kind {
    /// An amount in minor units (cents) of the single settlement currency;
    /// rendered only by [`crate::domain::money::print`], never as a float.
    Money,
    /// A count of things — transactions, months, entries. It is a whole number,
    /// but never an amount of money, and [`money_sum`] refuses it.
    Count,
    /// A calendar date in canonical `YYYY-MM-DD` form, as stored by
    /// [`crate::domain::transaction`].
    Date,
    /// Free text: a category path, a description, an `overflow` marker.
    Text,
}

/// The lower-case wire name of a [`Kind`], which is how the plugin protocol and
/// the `analyze` payload spell a column type.
pub fn kind_name(kind: Kind) -> &'static str {
    return match kind {
        Kind::Money => { "money" }
        Kind::Count => { "count" }
        Kind::Date => { "date" }
        Kind::Text => { "text" }
    };
}

/// [`kind_name`] backwards. The four lower-case names map to their [`Kind`];
/// anything else — an unknown word, an empty name, or an upper-case `"MONEY"`
/// that the wire form never writes — is [`Kind::Text`], the least assuming
/// reading of a column whose type was not named.
pub fn kind_of_name(name: &str) -> Kind {
    return match name {
        "money" => { Kind::Money }
        "count" => { Kind::Count }
        "date" => { Kind::Date }
        _ => { Kind::Text }
    };
}

/// One value of a [`Table`]. The variants mirror [`Kind`] one to one; a money
/// cell is an `i64` count of minor units (planning decision 11), never a float
/// and never a formatted string.
#[derive(Debug, Eq, PartialEq)]
pub enum Cell {
    /// Minor units of the settlement currency, e.g. `-123456` for the printed
    /// `"-1.234,56 €"`.
    Money(i64),
    /// A whole count, e.g. how many transactions a month holds.
    Count(i64),
    /// A canonical `YYYY-MM-DD` date, carried as the stored text.
    Date(String),
    /// Any other text: a category path, a merchant description, or the
    /// `overflow` marker an analysis writes when a fold exceeded `i64`.
    Text(String),
}

/// One column header: the display name a renderer shows and the [`Kind`] every
/// cell below it must have.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Column {
    /// The display name, e.g. `income`; already human-readable, so a renderer
    /// prints it as-is.
    pub name: String,
    /// The type every cell in this column position holds.
    pub kind: Kind,
}

/// A named analysis result: typed columns plus rows, with no formatting and no
/// JSON. [`add_row`] is the only way a row enters, which is what keeps the table
/// type-consistent; an analysis that needs its own ordering sorts the rows it
/// hands to [`add_row`].
///
/// A table with zero rows is a legitimate value, not a failure: it renders as
/// its header only (plan item 5.1).
#[derive(Debug, Eq, PartialEq)]
pub struct Table {
    /// The display name of the whole table, e.g. `"income and expenses by
    /// category 2024"`; a document uses it as the heading over the table.
    pub name: String,
    /// The columns, left to right. An empty header is allowed but degenerate.
    pub columns: Vec::<Column>,
    /// The rows, each holding exactly one [`Cell`] per column, in column order.
    pub rows: Vec::<Vec::<Cell>>,
}

/// Reason a row did not fit the table it was added to.
#[derive(Debug, Eq, PartialEq)]
pub enum AddError {
    /// The row had the wrong number of cells: the count the row had, then the
    /// number of columns the table has.
    ColumnCount(usize, usize),
    /// The cell at the given column index had the wrong type: the kind the
    /// column declared, then the kind the cell carried.
    KindMismatch(usize, Kind, Kind),
}

/// Reason a column could not be summed.
#[derive(Debug, Eq, PartialEq)]
pub enum SumError {
    /// No column at that index exists — and for a row built by hand, that the
    /// row is too short for the columns is reported here too.
    NoSuchColumn(usize),
    /// The column at that index is not a money column: the index, then the kind
    /// found (that of the column, or that of the offending cell within the
    /// column).
    NotMoney(usize, Kind),
    /// The exact sum did not fit an `i64` of minor units; the carried index
    /// tells the caller which column overflowed. A wrapped-around total is never
    /// returned.
    Overflow(usize),
}

/// Build an empty table with the given name and columns, given as
/// `(display name, kind)` pairs left to right. The caller adds every row with
/// [`add_row`].
pub fn table(name: &str, columns: &[(&str, Kind)]) -> Table {
    let mut built: Vec::<Column> = Vec::new();
    for (column_name, kind) in columns {
        built.push(Column { name: column_name.to_string(), kind: *kind });
    }
    return Table { name: name.to_string(), columns: built, rows: Vec::<Vec::<Cell>>::new() };
}

/// Add one row, checking it against the columns first.
///
/// First the length: a row with a different number of cells than the table has
/// columns fails with [`AddError::ColumnCount`], carrying the count the row had
/// and the count the table expects. Then, left to right, every cell's kind
/// against its column's kind; the first mismatch fails with
/// [`AddError::KindMismatch`]. Only when both checks pass is the row pushed, so
/// a rejected row never leaves a half-added row behind.
pub fn add_row(table: &mut Table, row: Vec::<Cell>) -> Result::<(), AddError> {
    if row.len() != table.columns.len() {
        return Err(AddError::ColumnCount(row.len(), table.columns.len()));
    }
    let mut index: usize = 0;
    for cell in row.iter() {
        let expected = table.columns[index].kind;
        let got = kind_of_cell(cell);
        if got != expected {
            return Err(AddError::KindMismatch(index, expected, got));
        }
        index += 1;
    }
    table.rows.push(row);
    return Ok(());
}

/// The display names of the columns, left to right — the header of a rendered
/// table.
pub fn header(table: &Table) -> Vec::<String> {
    return table.columns.iter().map(|column| column.name.clone()).collect();
}

/// True when the table has no rows. A header-only table is empty, not broken.
pub fn is_empty(table: &Table) -> bool {
    return table.rows.is_empty();
}

/// The index of the first column with that display name, if there is one.
pub fn column_of(table: &Table, name: &str) -> Option::<usize> {
    for (index, column) in table.columns.iter().enumerate() {
        if column.name == name {
            return Some(index);
        }
    }
    return None;
}

/// Sum a money column exactly.
///
/// A column index the table does not have fails with [`SumError::NoSuchColumn`];
/// a column that is not [`Kind::Money`] fails with
/// [`SumError::NotMoney(column, kind)`]. The remaining cells are folded with
/// [`i64::checked_add`], and a total that would exceed `i64` fails with
/// [`SumError::Overflow(column`]`] instead of wrapping around (Q1). A table with
/// no rows sums to `Ok(0)`.
///
/// [`add_row`] rejects a mismatched cell, so a table built through it cannot
/// hold a non-money cell in a money column. The [`Table`] fields are public, so
/// a test or a plugin host can still build one by hand; the same two refusals
/// are what such a table meets, never a skipped cell and never an assumption: a
/// row too short for the columns is `NoSuchColumn`, a cell of the wrong kind is
/// `NotMoney(column, kind_of_that_cell)`.
pub fn money_sum(table: &Table, column: usize) -> Result::<i64, SumError> {
    if column >= table.columns.len() {
        return Err(SumError::NoSuchColumn(column));
    }
    let kind = table.columns[column].kind;
    if kind != Kind::Money {
        return Err(SumError::NotMoney(column, kind));
    }
    let mut total: i64 = 0;
    for row in table.rows.iter() {
        let cell = match row.get(column) {
            Some(cell) => { cell }
            None => { return Err(SumError::NoSuchColumn(column)); }
        };
        let amount = match cell {
            Cell::Money(amount) => { *amount }
            other => { return Err(SumError::NotMoney(column, kind_of_cell(other))); }
        };
        match total.checked_add(amount) {
            None => { return Err(SumError::Overflow(column)); }
            Some(sum) => { total = sum; }
        }
    }
    return Ok(total);
}

/// The [`Kind`] a [`Cell`] carries; the mirror image of the cell's variant.
fn kind_of_cell(cell: &Cell) -> Kind {
    return match cell {
        Cell::Money(_) => { Kind::Money }
        Cell::Count(_) => { Kind::Count }
        Cell::Date(_) => { Kind::Date }
        Cell::Text(_) => { Kind::Text }
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::money;

    /// Add one row, and say so rather than assert-barely if the row does not
    /// fit: these tests want a table with rows, so a failure here is a bug in
    /// the test, not the value being checked.
    fn fill(table: &mut Table, row: Vec::<Cell>) {
        match add_row(table, row) {
            Ok(()) => {}
            Err(error) => { panic!("ADD_ROW FAILED A TEST ROW THAT MUST FIT: {error:?}") }
        }
    }

    /// A table whose first column counts and whose second sums money.
    fn count_then_money_table() -> Table {
        let mut built = table("sums", &[("count", Kind::Count), ("net", Kind::Money)]);
        fill(&mut built, vec![Cell::Count(3), Cell::Money(-40015)]);
        return built;
    }

    #[test]
    fn money_column_sums_exactly_no_float() {
        let mut built = table("net", &[("net", Kind::Money)]);
        for amount in [123456i64, -123456, 100050, -3999] {
            fill(&mut built, vec![Cell::Money(amount)]);
        }
        // Hand-computed, never read back from the code under test:
        // 123456 - 123456 = 0, 0 + 100050 = 100050, 100050 - 3999 = 96051.
        assert_eq!(money_sum(&built, 0), Ok(96051));
        // The very number survives the money module's own round trip, which a
        // binary float sum never guarantees.
        let printed = money::print(96051);
        assert_eq!(money::parse(&printed), Ok(96051));
    }

    #[test]
    fn money_sum_overflows_as_a_typed_error() {
        let mut built = table("net", &[("net", Kind::Money)]);
        for amount in [i64::MAX, 1i64] {
            fill(&mut built, vec![Cell::Money(amount)]);
        }
        assert_eq!(money_sum(&built, 0), Err(SumError::Overflow(0)));
    }

    #[test]
    fn money_sum_refuses_a_count_column() {
        let built = count_then_money_table();
        assert_eq!(money_sum(&built, 1), Ok(-40015));
        assert_eq!(money_sum(&built, 0), Err(SumError::NotMoney(0, Kind::Count)));
        assert_eq!(money_sum(&built, 7), Err(SumError::NoSuchColumn(7)));
    }

    #[test]
    fn add_row_rejects_wrong_column_count() {
        let mut built = table("four", &[
            ("a", Kind::Text),
            ("b", Kind::Text),
            ("c", Kind::Text),
            ("d", Kind::Text),
        ]);
        let given = vec![
            Cell::Text("x".to_string()),
            Cell::Text("y".to_string()),
            Cell::Text("z".to_string()),
        ];
        assert_eq!(add_row(&mut built, given), Err(AddError::ColumnCount(3, 4)));
        assert_eq!(built.rows.len(), 0);
    }

    #[test]
    fn add_row_rejects_wrong_cell_kind_and_keeps_the_row_out() {
        let mut built = table("mixed", &[
            ("category", Kind::Text),
            ("transactions", Kind::Count),
            ("net", Kind::Money),
        ]);
        let given = vec![
            Cell::Text("food".to_string()),
            Cell::Count(2),
            Cell::Count(2),
        ];
        assert_eq!(add_row(&mut built, given), Err(AddError::KindMismatch(2, Kind::Money, Kind::Count)));
        assert!(built.rows.is_empty());
    }

    #[test]
    fn empty_table_is_empty_and_its_header_survives() {
        let built = table("t", &[("a", Kind::Money)]);
        assert!(is_empty(&built));
        assert_eq!(header(&built), vec!["a".to_string()]);
        // An empty money column is a sum of nothing, which is zero, not an error.
        assert_eq!(money_sum(&built, 0), Ok(0));
        assert_eq!(column_of(&built, "a"), Some(0));
        assert_eq!(column_of(&built, "b"), None);
    }

    #[test]
    fn kind_names_round_trip() {
        for kind in [Kind::Money, Kind::Count, Kind::Date, Kind::Text] {
            assert_eq!(kind_of_name(kind_name(kind)), kind);
        }
        assert_eq!(kind_of_name("dmark"), Kind::Text);
        // The wire form is lower-case, so an upper-case spelling is not one of
        // ours and falls back to text.
        assert_eq!(kind_of_name("MONEY"), Kind::Text);
        assert_eq!(kind_of_name(""), Kind::Text);
        assert_eq!(kind_name(Kind::Money), "money");
        assert_eq!(kind_name(Kind::Count), "count");
        assert_eq!(kind_name(Kind::Date), "date");
        assert_eq!(kind_name(Kind::Text), "text");
    }

    #[test]
    fn table_is_comparable_and_stores_minor_units() {
        let mut left = table("net", &[("net", Kind::Money), ("day", Kind::Date)]);
        let mut right = table("net", &[("net", Kind::Money), ("day", Kind::Date)]);
        fill(&mut left, vec![Cell::Money(-123456), Cell::Date("2024-01-24".to_string())]);
        fill(&mut right, vec![Cell::Money(-123456), Cell::Date("2024-01-24".to_string())]);
        assert_eq!(left, right);
        // Stored verbatim: nothing here scales, rounds or formats an amount.
        assert_eq!(left.rows[0][0], Cell::Money(-123456));
        right.rows[0][0] = Cell::Money(-123457);
        assert_ne!(left, right);
    }

    #[test]
    fn money_sum_of_a_hand_built_inconsistent_table_refuses_instead_of_guessing() {
        // A hand-built table (the fields are public) holding a Count cell in a
        // Money column, and a row too short for the columns: both are refused,
        // neither is summed and neither is skipped over.
        let built = Table {
            name: "handbuilt".to_string(),
            columns: vec![Column { name: "net".to_string(), kind: Kind::Money }],
            rows: vec![vec![Cell::Money(100)], vec![Cell::Count(1)]],
        };
        assert_eq!(money_sum(&built, 0), Err(SumError::NotMoney(0, Kind::Count)));

        let short_rows = Table {
            name: "short".to_string(),
            columns: vec![Column { name: "net".to_string(), kind: Kind::Money }],
            rows: vec![vec![]],
        };
        assert_eq!(money_sum(&short_rows, 0), Err(SumError::NoSuchColumn(0)));
    }

    #[test]
    fn header_and_table_name_are_independent_of_the_rows() {
        let mut built = table("biggest spendings 2024", &[
            ("date", Kind::Date),
            ("description", Kind::Text),
            ("amount", Kind::Money),
        ]);
        fill(&mut built, vec![
            Cell::Date("2024-01-24".to_string()),
            Cell::Text("MEGA MART 04".to_string()),
            Cell::Money(-123456),
        ]);
        assert_eq!(built.name, "biggest spendings 2024");
        assert_eq!(header(&built), vec![
            "date".to_string(),
            "description".to_string(),
            "amount".to_string(),
        ]);
        assert_eq!(column_of(&built, "amount"), Some(2));
        assert_eq!(money_sum(&built, 2), Ok(-123456));
        assert_eq!(money_sum(&built, 1), Err(SumError::NotMoney(1, Kind::Text)));
        assert!(!is_empty(&built));
    }
}
