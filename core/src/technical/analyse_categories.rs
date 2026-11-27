//! Income and expense per category, per year — plan item 5.2, the first
//! built-in analysis of requirement 005 (`doc/architecture/architecture.md:220`).
//!
//! An analysis does NOT read a store on its own: [`income_expense_by_category`]
//! takes the entries [`crate::technical::view::read_view`] already derived and
//! the category tree, and hands back one [`crate::domain::result::Table`] — the
//! typed value plan 5.1 defines. It performs no I/O and writes nothing: the
//! category is derived on read (R003, "Classification is Derived, never
//! Stored"), so two runs over the same store return the same numbers (Q1).
//! [`category_tree_at`] is the ONE function here that touches the file system,
//! because `read_view` never reads `categories.csv`
//! ([`crate::technical::view::read_view`] reads transactions, annotations,
//! overrides and rules only) and the tree has to come from somewhere.
//!
//! ROLL-UP (planning decision 12): a parent row covers that category AND every
//! descendant, and a transaction lands in a parent's numbers exactly ONCE —
//! each transaction is attached to the deepest category it names and then
//! folded up the tree one level at a time, so `finance:expense:transport`
//! contributes to `finance:expense:transport`, to `finance:expense` and to
//! `finance`, never twice to the same row. A category (or a whole subtree) that
//! no transaction of the year reaches is OMITTED, which keeps a printed report
//! readable; `finance:expense:utilities` therefore usually has no row.
//!
//! THE `unknown` ROW (planning decision 12, plan 5.2 "unknown category grouped
//! separately"): `Classification::Unknown` and a rule category the tree does
//! not know share the LAST row, whose category cell is [`UNKNOWN_LABEL`].
//! Those rules are user error and must surface, not vanish into a category the
//! user never defined — [`unknown_category_paths`] hands exactly those paths to
//! the caller (plan 5.8 logs them as a warning). A store without
//! `categories.csv` has an empty tree, so EVERY classified path is unknown and
//! the whole store collapses into that one row.
//!
//! YEAR FILTER (planning decision 10): [`belongs_to_year`] is the single year
//! filter of this phase, the four-digit prefix of the canonical `YYYY-MM-DD`
//! date [`crate::domain::transaction`] stores; the plan pins that later
//! analyses reuse it rather than each inventing its own.
//!
//! OVERFLOW (Q1 exactness): every sum uses [`i64`::checked_add]. If a
//! category's income, expense or net did not fit, the row for THAT category
//! carries three [`crate::domain::result::Cell::Text`] `"overflow"` markers
//! instead of numbers — no row is dropped, no number is silently wrapped
//! around, and the caller (plan 5.8) logs a warning. A table holding such a
//! marker row refuses [`crate::domain::result::money_sum`]
//! (`NotMoney`), which is how the caller notices.

use crate::domain::category;
use crate::domain::classify::Classification;
use crate::domain::result;
use crate::domain::result::add_row;
use crate::domain::result::Cell;
use crate::domain::result::Kind;
use crate::domain::result::Table;
use super::view;

/// The canonical slug of this analysis: the built-in name the `analyze`
/// command, the `report` document and an analysis plugin all name (planning
/// decision: `categories`).
pub const NAME: &str = "categories";

/// The category cell of the single row that groups everything the tree cannot
/// place: `Classification::Unknown` and the rule categories the tree does not
/// know (planning decision 12).
pub const UNKNOWN_LABEL: &str = "unknown";

/// `categories.csv` of the data directory, the only file this module reads.
const CATEGORIES_FILE: &str = "categories.csv";

/// The text `category::from_csv` reads for "no categories at all": it holds no
/// line, and `from_csv` rejects only CONTENT lines, so it yields a tree with no
/// node (see [`category_tree_at`]).
const EMPTY_CATEGORIES_TEXT: &str = "";

/// The marker text an overflowed category row carries in place of a number.
const OVERFLOW_MARKER: &str = "overflow";

/// The category tree of the store below `data_dir`, plus the complaints
/// reading it produced.
///
/// The tree is NOT part of the merge-on-read view
/// ([`crate::technical::view::read_view`] reads transactions, annotations,
/// overrides and rules, never `categories.csv`), so this is where the
/// categories of plan 5.2 come from. Nothing here aborts: a missing,
/// unreadable or unparseable file leaves the analysis working on an EMPTY tree,
/// in which every classified path is unknown and every transaction falls into
/// the `unknown` row, and the reason travels back as a complaint string for
/// the caller to log (D-H: this phase adds no error code).
pub fn category_tree_at(data_dir: &std::path::Path) -> (category::Tree, Vec::<String>) {
    let path = data_dir.join(CATEGORIES_FILE);
    let mut complaints = Vec::<String>::new();
    let text = match std::fs::read_to_string(&path) {
        Err(reason) => {
            complaints.push(format!("categories.csv could not be read: {reason}"));
            String::new()
        }
        Ok(text) => { text }
    };
    match category::from_csv(&text) {
        Ok(tree) => { return (tree, complaints); }
        Err(reason) => {
            complaints.push(format!("categories.csv is not a valid category tree: {reason:?}"));
            // A tree that does not parse is NO tree: the analysis runs on the
            // empty one, i.e. everything ends up in the `unknown` row.
            // `category::from_csv` rejects content lines and nothing else, and
            // the empty text carries no line at all, so building the empty
            // tree cannot fail: the invariant is established by the `if` below
            // and the value then taken, the checked-`unwrap` shape this repo
            // already uses in `decide` (`core/src/technical/state.rs:122-127`).
            // A `Tree` is only ever produced by `from_csv`, so handing back a
            // different value here is not possible, and this module must not
            // panic on a caller who merely asked to read a store.
            let empty = category::from_csv(EMPTY_CATEGORIES_TEXT);
            if empty.is_err() {
                complaints.push(format!("categories.csv is not a valid category tree: even the empty tree was rejected: {EMPTY_CATEGORIES_TEXT:?}"));
            }
            return (empty.unwrap(), complaints);
        }
    }
}

/// Whether a stored `date` belongs to `year`.
///
/// `None` means "no year was asked for", so every date passes. `Some(y)` is
/// the four-digit prefix of the canonical `YYYY-MM-DD` form
/// ([`crate::domain::transaction`]'s stored dates): the first four code points
/// equal `y` rendered as four digits and the fifth character is `-`. A date in
/// a shape that is not canonical therefore passes no year, and a year outside
/// the four-digit range (e.g. `Some(10000)`) selects nothing.
///
/// `Option::<i64>` because the protocol's `year` parameter is a
/// `ValueKind::Integer` (`proto/src/commands.rs`).
pub fn belongs_to_year(date: &str, year: Option::<i64>) -> bool {
    let value = match year {
        None => { return true; }
        Some(value) => { value }
    };
    let prefix = format!("{:04}", value);
    if prefix.len() != 4 {
        return false;
    }
    let mut characters = date.chars();
    let mut head: String = String::new();
    let mut taken: usize = 0;
    loop {
        match characters.next() {
            None => { return false; }
            Some(character) => {
                head.push(character);
                taken = taken + 1;
                if taken == 4 { break; }
            }
        }
    }
    return head == prefix && characters.next() == Some('-');
}

/// The income/expense/net table per category of the store.
///
/// The buckets: an entry of the year (its date passes [`belongs_to_year`])
/// falls into its own category path when the tree knows it, into
/// [`UNKNOWN_LABEL`] when it is `Unknown` or the tree does not know the path
/// (decision 12). Income is the sum of the positive amounts, expense the sum of
/// the negative ones (kept NEGATIVE), net `income + expense` — every sum of
/// minor units, exact, never a float (planning decision 11).
///
/// A category's row covers that category AND every descendant, each
/// transaction exactly once (see the module doc), and the row is emitted only
/// when that roll-up holds at least one transaction. Rows come in depth-first
/// pre-order over the tree in file order ([`category::roots`] then
/// [`category::children`]), so a parent appears directly before its children;
/// the `unknown` row, when it holds anything, is LAST.
///
/// The table name carries the year when one was given, e.g.
/// `"income and expenses by category 2024"` (planning decision 16).
pub fn income_expense_by_category(entries: &[view::Entry], tree: &category::Tree, year: Option::<i64>) -> result::Table {
    let suffix = match year {
        None => { String::new() }
        Some(value) => { format!(" {value}") }
    };
    let mut table = result::table(
        &format!("income and expenses by category{suffix}"),
        &[
            ("category", Kind::Text),
            ("income", Kind::Money),
            ("expense", Kind::Money),
            ("net", Kind::Money),
        ]);
    let mut buckets = Vec::<Bucket>::new();
    for entry in entries.iter() {
        if !belongs_to_year(&entry.transaction.date, year) { continue; }
        let path = bucket_path(&entry.category, tree);
        let bucket = bucket_with(&mut buckets, path);
        add_amount(&mut bucket.totals, entry.transaction.amount);
    }
    for root in category::roots(tree).iter() {
        add_rows_of_subtree(tree, root, root, &buckets, &mut table);
    }
    // The grouped unknown: last, and only when it holds a transaction.
    match buckets.iter().find(|bucket| { bucket.path == UNKNOWN_LABEL }) {
        None => { }
        Some(bucket) => {
            if bucket.totals.count > 0 { push_row(&mut table, UNKNOWN_LABEL, &bucket.totals); }
        }
    }
    return table;
}

/// The distinct `Classification::Category` paths that the tree does not know,
/// sorted ascending and deduplicated.
///
/// A rule may name a category the user never defined (`travel:taxi` without a
/// `travel` node); such a transaction still lands in the `unknown` row, and
/// this is how the caller can name the rule that produced it (plan 5.8 logs the
/// list as a warning). `Classification::Unknown` itself is not reported: no
/// rule is at fault there.
pub fn unknown_category_paths(entries: &[view::Entry], tree: &category::Tree) -> Vec::<String> {
    let mut found = Vec::<String>::new();
    for entry in entries.iter() {
        match entry.category {
            Classification::Category(ref path) => {
                if category::is_valid(tree, path) { continue; }
                if found.iter().any(|known| { known == path }) { continue; }
                found.push(path.to_string());
            }
            Classification::Unknown => { }
        }
    }
    found.sort();
    return found;
}

/// The sums of one category, and how many transactions reached it.
///
/// `overflowed` is set when a fold exceeded `i64`; the numbers then are not
/// read, only the marker row is written.
#[derive(Copy, Clone)]
struct Totals {
    income: i64,
    expense: i64,
    count: usize,
    overflowed: bool,
}

/// A category and the transactions that named it as their own deepest category
/// (the `unknown` pseudo-category is carried by the same list).
struct Bucket {
    path: String,
    totals: Totals,
}

/// The bucket `path` is the row that groups a transaction whose classification
/// falls there: the path itself when the tree knows it, [`UNKNOWN_LABEL`] for
/// `Unknown` AND for a path the tree does not know (decision 12).
fn bucket_path<'a>(classification: &'a Classification, tree: &category::Tree) -> &'a str {
    match classification {
        Classification::Category(path) => {
            if category::is_valid(tree, path) { return path.as_str(); }
            return UNKNOWN_LABEL;
        }
        Classification::Unknown => { return UNKNOWN_LABEL; }
    }
}

/// The bucket with that path, appended on first use.
fn bucket_with<'a>(buckets: &'a mut Vec::<Bucket>, path: &str) -> &'a mut Bucket {
    let index = match buckets.iter().position(|bucket| { bucket.path == path }) {
        Some(index) => { index }
        None => {
            buckets.push(Bucket { path: path.to_string(), totals: Totals { income: 0, expense: 0, count: 0, overflowed: false } });
            buckets.len() - 1
        }
    };
    return &mut buckets[index];
}

/// Fold one amount into a category's sums, exactly, with
/// [`i64`]::checked_add. Income sums the positive amounts, expense the negative
/// ones (kept negative); a zero amount counts as a transaction of the category
/// and contributes to neither sum.
fn add_amount(totals: &mut Totals, amount: i64) {
    if amount > 0 {
        match totals.income.checked_add(amount) {
            Some(sum) => { totals.income = sum; }
            None => { totals.overflowed = true; }
        }
    } else if amount < 0 {
        match totals.expense.checked_add(amount) {
            Some(sum) => { totals.expense = sum; }
            None => { totals.overflowed = true; }
        }
    }
    totals.count = totals.count + 1;
}

/// Fold a subtree's totals into its parent's, once each (decision 12: a parent
/// covers itself plus every descendant, never a transaction twice).
fn merge(target: &mut Totals, sub: &Totals) {
    if sub.count == 0 { return; }
    target.count = target.count + sub.count;
    match target.income.checked_add(sub.income) {
        Some(sum) => { target.income = sum; }
        None => { target.overflowed = true; }
    }
    match target.expense.checked_add(sub.expense) {
        Some(sum) => { target.expense = sum; }
        None => { target.overflowed = true; }
    }
    if sub.overflowed { target.overflowed = true; }
}

/// The roll-up totals of the subtree rooted at `path`: the transactions of the
/// node itself plus those of every descendant, each counted exactly once.
fn subtree_totals(tree: &category::Tree, name: &str, path: &str, buckets: &[Bucket]) -> Totals {
    let mut totals = match buckets.iter().find(|bucket| { bucket.path == path }) {
        None => { Totals { income: 0, expense: 0, count: 0, overflowed: false } }
        Some(bucket) => { bucket.totals }
    };
    for child in category::children(tree, name).iter() {
        let child_totals = subtree_totals(tree, child, &format!("{path}:{child}"), buckets);
        merge(&mut totals, &child_totals);
    }
    return totals;
}

/// Add the rows of the subtree rooted at `path` in depth-first PRE-order, i.e.
/// the parent first, then its children in file order; a subtree whose roll-up
/// is empty contributes no row at all (decision 12).
fn add_rows_of_subtree(tree: &category::Tree, name: &str, path: &str, buckets: &[Bucket], table: &mut Table) {
    let totals = subtree_totals(tree, name, path, buckets);
    if totals.count > 0 { push_row(table, path, &totals); }
    for child in category::children(tree, name).iter() {
        add_rows_of_subtree(tree, child, &format!("{path}:{child}"), buckets, table);
    }
}

/// Add the row of one category: its path, income, expense, net — or the pinned
/// `overflow` marker row when a fold of this category exceeded `i64`.
fn push_row(table: &mut Table, path: &str, totals: &Totals) {
    let mut overflowed = totals.overflowed;
    let net = match totals.income.checked_add(totals.expense) {
        Some(net) => { net }
        None => { overflowed = true; 0 }
    };
    if overflowed {
        let row = vec![
            Cell::Text(path.to_string()),
            Cell::Text(OVERFLOW_MARKER.to_string()),
            Cell::Text(OVERFLOW_MARKER.to_string()),
            Cell::Text(OVERFLOW_MARKER.to_string()),
        ];
        // The marker REPLACES the three money cells of this row, so the row
        // cannot pass through [`add_row`] (which checks the kinds): it enters
        // through the table's public `rows` field instead, and the whole
        // column's [`money_sum`] then refuses with `NotMoney` — the exact
        // signal plan 5.8 logs as a warning. No number is dropped or wrapped.
        table.rows.push(row);
        return;
    }
    let row = vec![
        Cell::Text(path.to_string()),
        Cell::Money(totals.income),
        Cell::Money(totals.expense),
        Cell::Money(net),
    ];
    // Four cells, one per column, of the four pinned kinds: the check inside
    // `add_row` cannot fail here, and there is nothing else an analysis that
    // returns a table could do with the typed rejection.
    match add_row(table, row) {
        Ok(()) => { }
        Err(_) => { }
    }
}

// ============================================================================
// Tests: a scratch store the test builds and removes. The fixture texts are
// copied HERE (the consts of `view.rs` are `#[cfg(test)]`-private to that
// module and cannot be reused), verbatim from `testdata/`.
// ============================================================================

/// The SCRATCH PARENT of THIS module, module-private like every other module's
/// (`test-scratch-view`, `test-scratch-transport`, …), so no other module's
/// cleanup can ever delete this test's scratch tree.
#[cfg(test)]
const SCRATCH_PARENT: &str = "test-scratch-analyse-categories";

/// A scratch store below `test-scratch-analyse-categories/<name>`; the test
/// removes it with [`remove_scratch`] before it finishes.
#[cfg(test)]
fn make_store(name: &str) -> std::path::PathBuf {
    let store = std::path::PathBuf::from(SCRATCH_PARENT).join(name);
    match std::fs::create_dir_all(&store) {
        Err(reason) => { panic!("cannot create the scratch store of an analysis test: {reason}"); }
        Ok(_) => { }
    }
    return store;
}

/// Write `text` to `<store>/<rel>`, creating the directory of `rel` (the
/// `rules/` subdirectory) first.
#[cfg(test)]
fn write_file(store: &std::path::PathBuf, rel: &str, text: &str) {
    let path = store.join(rel);
    match path.parent() {
        None => { }
        Some(parent) => {
            match std::fs::create_dir_all(parent) {
                Err(reason) => { panic!("cannot create an analysis fixture directory: {reason}"); }
                Ok(_) => { }
            }
        }
    }
    match std::fs::write(&path, text) {
        Err(reason) => { panic!("cannot write an analysis fixture: {reason}"); }
        Ok(_) => { }
    }
}

/// Remove the scratch store: its fixture files, its `rules/` directory, the
/// store directory, and this module's OWN parent (`SCRATCH_PARENT`) once the
/// last analysis test finished (still in use while other tests of this module
/// run, so a failure is ignored; no OTHER module shares that parent).
#[cfg(test)]
fn remove_scratch(store: std::path::PathBuf) {
    for name in ["transactions.csv", "overrides.csv", "annotations.csv", "categories.csv", "rules/10-built-in.rules"] {
        match std::fs::remove_file(&store.join(name)) { Err(_) => { } Ok(_) => { } }
    }
    match std::fs::remove_dir(&store.join("rules")) { Err(_) => { } Ok(_) => { } }
    match std::fs::remove_dir(&store) { Err(_) => { } Ok(_) => { } }
    match std::fs::remove_dir(std::path::PathBuf::from(SCRATCH_PARENT)) { Err(_) => { } Ok(_) => { } }
}

/// The verbatim text of `testdata/rules/sample.rules`.
#[cfg(test)]
const FIXTURE_SAMPLE_RULES_TEXT: &str = r#"# Phin sample rules -- one rule per line: pattern -> category
# '#' starts a comment, empty lines are ignored, file order = precedence.
# The exact pattern syntax is finalised in plan item 1.5; this sample
# shows literal, word-list and regex patterns.

REWE MARKT -> food:supermarket
ALDI -> food:supermarket
LIDL -> food:supermarket
ZUR LEITZ -> food:restaurant
CAFETARIA -> food:restaurant
TAXI -> finance:expense:transport
BOOKS ONLINE -> leisure:books
AMAZON MARKETPLACE -> leisure:games
regex:^PAYMENT TO -> finance:income
CLIENT PAYMENT -> finance:income
"#;

/// The verbatim text of `testdata/categories.csv`.
#[cfg(test)]
const FIXTURE_CATEGORIES_TEXT: &str = r#"# Phin categories -- tree of parent:child names, order is preserved.
# columns: parent;name
;food
;finance
;leisure
food;supermarket
food;restaurant
finance;income
finance;expense
expense;transport
expense;utilities
leisure;books
leisure;games
"#;

/// The verbatim text of `testdata/transactions.csv`: the four `#` preamble
/// comment lines, then the 22 real rows, each line ending with LF.
#[cfg(test)]
const FIXTURE_TRANSACTIONS_TEXT: &str = r#"# Phin transactions -- append-only facts, never edited in place.
# columns: id;date;amount;currency;description;counterparty;source
# amount is an integer in minor units (cents); negative = expense.
# dates are normalised to YYYY-MM-DD; id = short content hash.
39945b2fed;2024-01-03;-3999;EUR;REWE MARKT 12;REWE MARKET;bank_a.pdf
f596357483;2024-01-09;-5990;EUR;DINNER ZUR LEITZ;ZUR LEITZ;bank_a.pdf
dc0db110c4;2024-01-15;42000;EUR;PAYMENT TO TOBIAS;TAXI BERLIN;bank_a.pdf
3d7478d7cc;2024-01-21;-1847;EUR;BOOKS ONLINE;THONIX;bank_a.pdf
2b5a9d2799;2024-01-24;-123456;EUR;MEGA MART 04;MEGA MART;bank_a.pdf
7a8051207a;2024-01-28;-12990;EUR;LIDL 0815;LIDL;bank_a.pdf
a86d11da9a;2024-02-03;-8990;EUR;ALDI 4711;ALDI;bank_b1.pdf
140ef279a4;2024-02-11;-2500;EUR;"TAXI AIRPORT; NIGHT";TAXI BERLIN;bank_b1.pdf
e86f3fc214;2024-02-17;85000;EUR;CLIENT PAYMENT;ALICE;bank_b1.pdf
fbd45cccf1;2024-02-24;-3456;EUR;REWE MARKT 12;REWE MARKET;bank_b1.pdf
58ed99aa4c;2024-03-02;-1590;EUR;CAFETARIA;CAFETARIA;bank_b2.pdf
80c63211a8;2024-03-05;-1500;EUR;REWE MARKT 12;REWE MARKET;bank_b2.pdf
80c63211a8#2;2024-03-05;-1500;EUR;REWE MARKT 12;REWE MARKET;bank_b2.pdf
5b35bef1fd;2024-03-19;-6789;EUR;AMAZON MARKETPLACE;AMZN;bank_b2.pdf
79caf386d4;2024-04-02;2599;EUR;LEGO STORE DE;LEGO STORE;mail_paypal.mbox
4fd4783f1e;2024-04-05;-1999;EUR;NETFLIX.COM;NETFLIX;mail_paypal.mbox
30fc834ea9;2024-04-09;899;EUR;BAKERY BERLIN;BAKERY BERLIN;mail_paypal.mbox
bad5886754;2024-04-12;-4500;EUR;STEAM PURCHASE;VALVE;mail_paypal.mbox
0cfe0127e0;2024-04-15;12000;EUR;CLIENT PAYMENT;ALICE;mail_paypal.mbox
3d3a5287f1;2024-04-18;-799;EUR;BOOKS ONLINE;THONIX;mail_paypal.mbox
06113c369c;2024-04-21;3450;EUR;"SUPERMARKET; DELICATES";DELICATES;mail_paypal.mbox
f9aeec83dc;2024-04-26;-2999;EUR;"TAXI AIRPORT; NIGHT";TAXI BERLIN;mail_paypal.mbox
"#;

/// The fixture store of `testdata/`: 22 transactions, the sample rules as
/// `rules/10-built-in.rules`, `categories.csv` copied verbatim.
#[cfg(test)]
fn fill_fixture_store(store: &std::path::PathBuf) {
    write_file(store, "transactions.csv", FIXTURE_TRANSACTIONS_TEXT);
    write_file(store, "rules/10-built-in.rules", FIXTURE_SAMPLE_RULES_TEXT);
    write_file(store, "categories.csv", FIXTURE_CATEGORIES_TEXT);
}

/// The rows of a table as `(category, income, expense, net)`, the four pinned
/// cells of every row of this analysis. Money cells are read as the minor
/// units the table holds; a marker row fails the test rather than being
/// silently read as a number.
#[cfg(test)]
fn rows_of(table: &Table) -> Vec::<(String, i64, i64, i64)> {
    let mut found = Vec::<(String, i64, i64, i64)>::new();
    for row in table.rows.iter() {
        if row.len() != 4 { panic!("A TEST TABLE ROW MUST HOLD FOUR CELLS: {row:?}"); }
        let category = row_text(&row[0]);
        let income = money_cell(1, &row[1]);
        let expense = money_cell(2, &row[2]);
        let net = money_cell(3, &row[3]);
        found.push((category, income, expense, net));
    }
    return found;
}

/// The text of a category cell, with the shape checked.
#[cfg(test)]
fn row_text(cell: &Cell) -> String {
    return match cell {
        Cell::Text(text) => { text.to_string() }
        other => { panic!("THE CATEGORY CELL MUST BE TEXT: {other:?}") }
    };
}

/// The minor units of a money cell, with the position named for a failure.
#[cfg(test)]
fn money_cell(index: usize, cell: &Cell) -> i64 {
    return match cell {
        Cell::Money(amount) => { *amount }
        other => { panic!("THE CELL AT INDEX {index} MUST BE MONEY: {other:?}") }
    };
}

/// Hand-computed rows of the fixture store WITHOUT a year (decision: D-I, the
/// numbers are pinned by the plan and derived by hand, never read back): 6 rows
/// in `food:supermarket` (-3999, -12990, -8990, -3456, -1500, -1500 = -32435),
/// 2 in `food:restaurant` (-5990, -1590 = -7580), 3 in `finance:income`
/// (42000, 85000, 12000 = 139000), 2 in `finance:expense:transport`
/// (-2500, -2999 = -5499), 2 in `leisure:books` (-1847, -799 = -2646), 1 in
/// `leisure:games` (-6789), and 6 `unknown` rows (income 2599 + 899 + 3450 =
/// 6948, expense -123456 - 1999 - 4500 = -129955). A parent is its own
/// transactions plus its descendants' (`finance:expense` covers `transport`
/// only, `food` covers `supermarket` and `restaurant`).
#[cfg(test)]
fn expected_fixture_rows() -> Vec::<(String, i64, i64, i64)> {
    return vec![
        ("food".to_string(), 0, -40015, -40015),
        ("food:supermarket".to_string(), 0, -32435, -32435),
        ("food:restaurant".to_string(), 0, -7580, -7580),
        ("finance".to_string(), 139000, -5499, 133501),
        ("finance:income".to_string(), 139000, 0, 139000),
        ("finance:expense".to_string(), 0, -5499, -5499),
        ("finance:expense:transport".to_string(), 0, -5499, -5499),
        ("leisure".to_string(), 0, -9435, -9435),
        ("leisure:books".to_string(), 0, -2646, -2646),
        ("leisure:games".to_string(), 0, -6789, -6789),
        (UNKNOWN_LABEL.to_string(), 6948, -129955, -123007),
    ];
}

#[test]
fn fixture_store_yields_the_hand_computed_category_totals() {
    let store = make_store("analyse-categories-fixture");
    fill_fixture_store(&store);
    let view = view::read_view(&store);
    let (tree, complaints) = category_tree_at(&store);
    assert!(complaints.len() == 0);
    assert!(tree_is_the_fixture_tree(&tree));
    let table = income_expense_by_category(&view.entries, &tree, None);
    // Display name: no year, so no suffix (decision 16).
    assert!(table.name == "income and expenses by category");
    assert!(result::header(&table) == vec!["category".to_string(), "income".to_string(), "expense".to_string(), "net".to_string()]);
    assert!(table.rows.len() == 11);
    assert_eq!(rows_of(&table), expected_fixture_rows());
    // A real node of the tree that no transaction reaches is omitted.
    assert!(!table.rows.iter().any(|row| { row_text(&row[0]) == "finance:expense:utilities".to_string() }));
    remove_scratch(store);
}

#[test]
fn unknown_categories_are_grouped_and_reported() {
    let store = make_store("analyse-categories-no-categories");
    fill_fixture_store(&store);
    match std::fs::remove_file(&store.join("categories.csv")) {
        Err(reason) => { panic!("cannot delete the scratch categories.csv: {reason}"); }
        Ok(_) => { }
    }
    let view = view::read_view(&store);
    let (tree, complaints) = category_tree_at(&store);
    // The whole store collapses into the one `unknown` row.
    let table = income_expense_by_category(&view.entries, &tree, None);
    assert!(table.rows.len() == 1);
    // Hand-computed: income 42000 + 85000 + 2599 + 899 + 12000 + 3450 = 145948,
    // expense -184904, and the two together are the fixture's -38956.
    assert_eq!(rows_of(&table), vec![(UNKNOWN_LABEL.to_string(), 145948, -184904, -38956)]);
    // The distinct rule categories the tree does not know, sorted ascending:
    // the SIX categories the sample rules can produce (hand-derived from the
    // file: `grep -o '\-> [^ ]*' testdata/rules/sample.rules` = food:supermarket,
    // food:restaurant, finance:expense:transport, finance:income, leisure:books,
    // leisure:games — no rule writes plain `finance:expense`).
    assert_eq!(
        unknown_category_paths(&view.entries, &tree),
        vec![
            "finance:expense:transport".to_string(),
            "finance:income".to_string(),
            "food:restaurant".to_string(),
            "food:supermarket".to_string(),
            "leisure:books".to_string(),
            "leisure:games".to_string(),
        ]);
    assert!(complaints.len() == 1);
    assert!(complaints[0].starts_with("categories.csv could not be read:"));
    remove_scratch(store);
}

#[test]
fn year_filter_selects_rows_by_the_date_prefix() {
    let store = make_store("analyse-categories-year");
    fill_fixture_store(&store);
    let view = view::read_view(&store);
    let (tree, _) = category_tree_at(&store);
    // Every fixture date is 2024, so 2024 selects the whole store ...
    let all = income_expense_by_category(&view.entries, &tree, None);
    let in_2024 = income_expense_by_category(&view.entries, &tree, Some(2024));
    assert!(in_2024.name == "income and expenses by category 2024");
    assert_eq!(rows_of(&all), expected_fixture_rows());
    assert_eq!(rows_of(&in_2024), expected_fixture_rows());
    // ... and 2023 selects nothing: an empty table with a header.
    let in_2023 = income_expense_by_category(&view.entries, &tree, Some(2023));
    assert!(result::is_empty(&in_2023));
    assert!(result::header(&in_2023).len() == 4);
    // The helper itself, on canonical dates.
    assert!(belongs_to_year("2024-01-03", None));
    assert!(belongs_to_year("2024-01-03", Some(2024)));
    assert!(!belongs_to_year("2024-01-03", Some(2023)));
    assert!(!belongs_to_year("2024-01-03", Some(24)));
    assert!(!belongs_to_year("2024", Some(2024)));
    remove_scratch(store);
}

#[test]
fn subcategory_rollup_is_a_parent_plus_descendants_only_once() {
    let store = make_store("analyse-categories-rollup");
    write_file(&store, "transactions.csv", "# columns: id;date;amount;currency;description;counterparty;source\n1111aaaabbbb;2024-05-01;-1000;EUR;REWE MARKT 12;REWE;test.pdf\n2222aaaacccc;2024-05-02;-250;EUR;GARDEN CENTRE;GARDEN;test.pdf\n");
    write_file(&store, "rules/10-built-in.rules", "REWE MARKT -> food:supermarket\nGARDEN CENTRE -> food\n");
    write_file(&store, "categories.csv", FIXTURE_CATEGORIES_TEXT);
    let view = view::read_view(&store);
    let (tree, complaints) = category_tree_at(&store);
    assert!(complaints.len() == 0);
    let table = income_expense_by_category(&view.entries, &tree, None);
    // `food` carries its OWN transaction (-250) plus the child's (-1000), once
    // each; the child row carries the child's amount and nothing else.
    assert_eq!(rows_of(&table), vec![
        ("food".to_string(), 0, -1250, -1250),
        ("food:supermarket".to_string(), 0, -1000, -1000),
    ]);
    remove_scratch(store);
}

#[test]
fn empty_store_is_an_empty_table_with_a_header() {
    let store = make_store("analyse-categories-empty");
    write_file(&store, "transactions.csv", "");
    write_file(&store, "rules/10-built-in.rules", FIXTURE_SAMPLE_RULES_TEXT);
    write_file(&store, "categories.csv", FIXTURE_CATEGORIES_TEXT);
    let view = view::read_view(&store);
    let (tree, complaints) = category_tree_at(&store);
    assert!(complaints.len() == 0);
    let table = income_expense_by_category(&view.entries, &tree, None);
    assert!(result::is_empty(&table));
    assert_eq!(result::header(&table), vec!["category".to_string(), "income".to_string(), "expense".to_string(), "net".to_string()]);
    remove_scratch(store);
}

#[test]
fn missing_categories_csv_is_reported_as_a_complaint() {
    let store = make_store("analyse-categories-missing-categories");
    write_file(&store, "transactions.csv", "");
    write_file(&store, "rules/10-built-in.rules", FIXTURE_SAMPLE_RULES_TEXT);
    let (tree, complaints) = category_tree_at(&store);
    assert!(complaints.len() == 1);
    assert!(complaints[0].starts_with("categories.csv could not be read:"));
    assert!(category::roots(&tree).len() == 0);
    remove_scratch(store);
}

#[test]
fn rule_category_outside_the_tree_is_reported() {
    let store = make_store("analyse-categories-unknown-rule");
    write_file(&store, "transactions.csv", "# columns: id;date;amount;currency;description;counterparty;source\n2b5a9d2799;2024-01-24;-123456;EUR;MEGA MART 04;MEGA MART;bank_a.pdf\n");
    write_file(&store, "rules/10-built-in.rules", "MEGA MART -> travel:taxi\n");
    write_file(&store, "categories.csv", FIXTURE_CATEGORIES_TEXT);
    let view = view::read_view(&store);
    let (tree, complaints) = category_tree_at(&store);
    assert!(complaints.len() == 0);
    let table = income_expense_by_category(&view.entries, &tree, None);
    // The transaction is not lost and not filed under a category the user never
    // defined: it sits in the grouped `unknown` row.
    assert_eq!(rows_of(&table), vec![(UNKNOWN_LABEL.to_string(), 0, -123456, -123456)]);
    assert_eq!(unknown_category_paths(&view.entries, &tree), vec!["travel:taxi".to_string()]);
    remove_scratch(store);
}

#[test]
fn fixture_store_total_matches_the_hand_checked_minus_38_956() {
    let store = make_store("analyse-categories-total");
    fill_fixture_store(&store);
    let view = view::read_view(&store);
    let (tree, _) = category_tree_at(&store);
    let table = income_expense_by_category(&view.entries, &tree, None);
    // Q1 exactness: the three roots plus the grouped unknown add up to the
    // hand-added total of the 22 fixture rows:
    // -3999-5990+42000-1847-123456-12990-8990-2500+85000-3456-1590-1500-1500
    // -6789+2599-1999+899-4500+12000-799+3450-2999 = -38956.
    let mut total: i64 = 0;
    let roots = ["food".to_string(), "finance".to_string(), "leisure".to_string(), UNKNOWN_LABEL.to_string()];
    for row in table.rows.iter() {
        let text = row_text(&row[0]);
        if roots.contains(&text) {
            total = total + money_cell(3, &row[3]);
        }
    }
    assert!(total == -38956);
    remove_scratch(store);
}

/// True when the tree is the fixture tree: the same roots and the same children
/// of `food`, so the test does not rely on a tree it wrote itself.
#[cfg(test)]
fn tree_is_the_fixture_tree(tree: &category::Tree) -> bool {
    return category::roots(tree) == vec!["food".to_string(), "finance".to_string(), "leisure".to_string()]
        && category::children(tree, "food") == vec!["supermarket".to_string(), "restaurant".to_string()];
}
