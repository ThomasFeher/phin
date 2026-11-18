//! The merge-on-read view of the store — plan item 2.6.
//!
//! [`read_view`] opens the store's data directory and returns what the store
//! MEANS, without ever changing what it HOLDS: it reads `transactions.csv`
//! with [`crate::technical::store_transactions::read`], `annotations.csv` and
//! `overrides.csv` with [`crate::technical::store_sidefiles`], and the `rules/`
//! directory, then derives each transaction's category by calling
//! [`crate::domain::classify::classify`]. Not one byte is written back: the
//! view is read-only, so two consecutive reads return identical results
//! (`doc/architecture/data-files.md`, "facts, never edited in place" —
//! precedence lives in the read, not in the file).
//!
//! RULE ORDER (decision 9, `doc/architecture/data-files.md` "rules/*"): files
//! under `rules/` are taken in LEXICAL filename order, which is the numeric
//! prefix order the contract fixes (`10-built-in.rules` before `50-user.rules`
//! before `90-learned.txt`), and BOTH extensions are accepted — the user
//! convention `.rules` and the core-appended learned file `.txt`. Their rules
//! are concatenated in that order, so the LAST-wins scan in
//! [`crate::domain::classify::classify`] gives learned > user > built-in
//! without this module knowing anything but the filename order.
//!
//! ORPHANS AND DANGLING REFS (decision 10: typed return values, no events):
//! an override naming no stored transaction id comes back in
//! [`View.orphan_overrides`] — [`crate::technical::store_sidefiles::read_overrides`]
//! already reports it, this module only forwards it — and an annotation whose
//! non-empty `ref` names no stored transaction id comes back in
//! [`View.dangling_refs`]. An EMPTY `ref` is the orphan/pending form of
//! `annotations.csv` (`doc/architecture/data-files.md:84`), not a dangling one,
//! so it is not reported. Both checks use EXACT id equality, the same comparison
//! [`crate::domain::classify::classify`] applies.
//!
//! A file that cannot be opened contributes no text: the store may legitimately
//! be empty before the first import. A line that fails to parse is counted in
//! [`View.corrupt_lines`] (the per-line detail stays with the store readers),
//! and a rule file that fails to parse contributes its [`crate::domain::rule::RuleError`]
//! to [`View.rule_errors`] instead of half its rules.

use super::store_transactions::read as read_transactions;
use super::store_sidefiles::read_annotations;
use super::store_sidefiles::read_overrides;

use crate::domain::transaction::Transaction;
use crate::domain::rule::Rule;
use crate::domain::rule::parse_rules;
use crate::domain::rule::RuleError;
use crate::domain::classify::classify;
use crate::domain::classify::Classification;
use crate::domain::classify::Override;

/// One effective transaction: the stored fact plus the category the view derived
/// for it. The category is derived on read and never stored.
#[derive(Debug, Eq, PartialEq)]
pub struct Entry {
    pub transaction: Transaction,
    pub category: Classification,
}

/// An annotation whose non-empty `ref` names no transaction on file.
#[derive(Debug, Eq, PartialEq)]
pub struct DanglingRef {
    /// The annotation's own id, so the report can name the note.
    pub annotation: String,
    /// The transaction id it names but that does not exist.
    pub r#ref: String,
}

/// What one merge-on-read pass returns.
#[derive(Debug, Eq, PartialEq)]
pub struct View {
    /// The effective transactions, in `transactions.csv` order.
    pub entries: Vec::<Entry>,
    /// Overrides naming an id that is not on file (`doc/architecture/data-files.md:104`).
    pub orphan_overrides: Vec::<Override>,
    /// Annotations whose `ref` names no transaction.
    pub dangling_refs: Vec::<DanglingRef>,
    /// Rule files that did not parse, one [`crate::domain::rule::RuleError`] each.
    pub rule_errors: Vec::<RuleError>,
    /// Lines the three store readers rejected; the detail stays with them.
    pub corrupt_lines: usize,
}

/// The text of `path`, or an empty string when it cannot be opened — a store
/// that has not been written yet is an empty store, not an error.
fn read_optional(path: &std::path::Path) -> String {
    match std::fs::read_to_string(path) {
        Err(_) => { return String::new(); }
        Ok(text) => { return text; }
    }
}

/// The rules of one `rules/` directory, in lexical filename order.
///
/// Only regular files whose name ends in `.rules` or `.txt` are read
/// (decision 9); the lexical sort of the names is the numeric-prefix order the
/// contract fixes. A file whose text does not parse yields its
/// [`crate::domain::rule::RuleError`] and contributes no rules at all, so a
/// malformed file cannot silently reorder the rules after it.
fn load_rules(dir: &std::path::Path) -> (Vec::<Rule>, Vec::<RuleError>) {
    let mut names = Vec::<std::path::PathBuf>::new();
    match std::fs::read_dir(dir) {
        Err(_) => { }
        Ok(entries) => {
            for entry in entries {
                match entry {
                    Err(_) => { continue; }
                    Ok(handle) => {
                        let path = handle.path();
                        let name = path.to_string_lossy().to_string();
                        if path.is_file() && (name.ends_with(".rules") || name.ends_with(".txt")) {
                            names.push(path);
                        }
                    }
                }
            }
        }
    }
    names.sort();
    let mut rules = Vec::<Rule>::new();
    let mut errors = Vec::<RuleError>::new();
    for name in names.iter() {
        match parse_rules(&read_optional(name)) {
            Err(reason) => { errors.push(reason); }
            Ok(parsed) => { for rule in parsed { rules.push(rule); } }
        }
    }
    return (rules, errors);
}

/// Read the whole store below `dir` and return what it means; read-only and
/// deterministic.
///
/// `dir` holds `transactions.csv`, `annotations.csv`, `overrides.csv` and the
/// `rules/` directory (`doc/architecture/data-files.md`). Every stored
/// transaction becomes one [`Entry`] whose category is derived by
/// [`crate::domain::classify::classify`] from the rules of `rules/` and the
/// overrides on file, so an override beats a rule and a later rule file beats
/// an earlier one without any store file being rewritten.
pub fn read_view(dir: &std::path::Path) -> View {
    let (rows, transaction_errors) = read_transactions(&read_optional(&dir.join("transactions.csv")));
    let (annotations, annotation_errors) = read_annotations(&read_optional(&dir.join("annotations.csv")));
    let mut known_ids = Vec::<String>::new();
    for row in rows.iter() { known_ids.push(row.id.to_string()); }
    let (overrides, orphans, override_errors) = read_overrides(&read_optional(&dir.join("overrides.csv")), &known_ids);
    let (rules, rule_errors) = load_rules(&dir.join("rules"));
    let mut entries = Vec::<Entry>::new();
    for row in rows {
        let category = classify(&row, &rules, &overrides);
        entries.push(Entry { transaction: row, category: category });
    }
    let mut dangling = Vec::<DanglingRef>::new();
    for annotation in annotations.iter() {
        if annotation.r#ref.len() == 0 { continue; }
        let linked = known_ids.iter().any(|known| *known == annotation.r#ref);
        if !linked {
            dangling.push(DanglingRef { annotation: annotation.id.to_string(), r#ref: annotation.r#ref.to_string() });
        }
    }
    return View {
        entries: entries,
        orphan_overrides: orphans,
        dangling_refs: dangling,
        rule_errors: rule_errors,
        corrupt_lines: transaction_errors.len() + annotation_errors.len() + override_errors.len(),
    };
}

// ============================================================================
// Tests: a scratch store the test builds and removes.
// ============================================================================

#[cfg(test)]
const TRANSACTIONS_TEXT: &str = "# columns: id;date;amount;currency;description;counterparty;source\n39945b2fed;2024-01-03;-3999;EUR;REWE MARKT 12;REWE MARKET;bank_a.pdf\ndc0db110c4;2024-01-15;42000;EUR;PAYMENT TO TOBIAS;TAXI BERLIN;bank_a.pdf\n";

#[cfg(test)]
const OVERRIDES_TEXT: &str = "# columns: id;category;reason\n39945b2fed;leisure:games;user chose this\nffffffffff;food:supermarket;stale guess\n";

#[cfg(test)]
const ANNOTATIONS_TEXT: &str = "# columns: id;ref;text;items;source;confidence\nann00000001;39945b2fed;delivery note;1x groceries;mail_rewe.mbox;0.9\nann00000002;eeeeeeeeee;note about a deleted row;;mail_x.mbox;0.5\n";

#[cfg(test)]
const RULES_TEXT: &str = "REWE MARKT -> food:supermarket\n";

/// A scratch store below `test-scratch/view-<name>`; the test removes it with
/// [`remove_scratch`] before it finishes.
#[cfg(test)]
fn make_store(name: &str) -> std::path::PathBuf {
    let store = std::path::PathBuf::from("test-scratch").join(name);
    match std::fs::create_dir_all(&store) {
        Err(reason) => { panic!("cannot create the scratch store of a view test: {reason}"); }
        Ok(_) => { }
    }
    return store;
}

/// Write `text` to `<store>/<name>`, creating the directory of `name` (the
/// `rules/` subdirectory) first.
#[cfg(test)]
fn write_fixture(store: &std::path::PathBuf, name: &str, text: &str) {
    let path = store.join(name);
    match path.parent() {
        None => { }
        Some(parent) => {
            match std::fs::create_dir_all(parent) {
                Err(reason) => { panic!("cannot create a view fixture directory: {reason}"); }
                Ok(_) => { }
            }
        }
    }
    match std::fs::write(&path, text) {
        Err(reason) => { panic!("cannot write a view fixture: {reason}"); }
        Ok(_) => { }
    }
}

/// Remove the scratch store: its fixture files, its `rules/` directory, the
/// store directory, and the shared `test-scratch` parent once the last test
/// finished (still in use while other view tests run, so a failure is ignored).
#[cfg(test)]
fn remove_scratch(store: std::path::PathBuf) {
    for name in ["transactions.csv", "overrides.csv", "annotations.csv", "rules/10-built-in.rules", "rules/50-user.rules"] {
        match std::fs::remove_file(&store.join(name)) { Err(_) => { } Ok(_) => { } }
    }
    match std::fs::remove_dir(&store.join("rules")) { Err(_) => { } Ok(_) => { } }
    match std::fs::remove_dir(&store) { Err(_) => { } Ok(_) => { } }
    match std::fs::remove_dir(std::path::PathBuf::from("test-scratch")) { Err(_) => { } Ok(_) => { } }
}

#[cfg(test)]
fn fill_store(name: &str) -> std::path::PathBuf {
    let store = make_store(name);
    write_fixture(&store, "transactions.csv", TRANSACTIONS_TEXT);
    write_fixture(&store, "overrides.csv", OVERRIDES_TEXT);
    write_fixture(&store, "annotations.csv", ANNOTATIONS_TEXT);
    write_fixture(&store, "rules/10-built-in.rules", RULES_TEXT);
    return store;
}

#[test]
fn orphaned_override_and_dangling_annotation_ref_are_reported() {
    let store = fill_store("view-orphans");
    let view = read_view(&store);
    // Both stored rows survive, in file order, with a derived category.
    assert!(view.entries.len() == 2);
    assert!(view.entries[0].transaction.id == "39945b2fed");
    // The override beats the rule for the stored id; the second row matches no
    // rule, so it stays Unknown.
    assert!(view.entries[0].category == Classification::Category("leisure:games".to_string()));
    assert!(view.entries[1].category == Classification::Unknown);
    // The override for `ffffffffff` is reported, not dropped and not applied.
    assert!(view.orphan_overrides.len() == 1);
    assert!(view.orphan_overrides[0].id == "ffffffffff");
    // The annotation naming `eeeeeeeeee` is dangling; the empty-ref form is not.
    assert!(view.dangling_refs.len() == 1);
    assert!(view.dangling_refs[0].annotation == "ann00000002");
    assert!(view.dangling_refs[0].r#ref == "eeeeeeeeee");
    assert!(view.rule_errors.len() == 0);
    assert!(view.corrupt_lines == 0);
    remove_scratch(store.to_path_buf());
}

#[test]
fn two_consecutive_reads_give_identical_results_and_no_store_file_changes() {
    let store = fill_store("view-stable");
    let first = read_view(&store);
    let second = read_view(&store);
    assert!(first == second);
    // The view is read-only: every fixture file still holds its exact text.
    assert!(read_optional(&store.join("transactions.csv")) == TRANSACTIONS_TEXT);
    assert!(read_optional(&store.join("overrides.csv")) == OVERRIDES_TEXT);
    assert!(read_optional(&store.join("annotations.csv")) == ANNOTATIONS_TEXT);
    assert!(read_optional(&store.join("rules").join("10-built-in.rules")) == RULES_TEXT);
    remove_scratch(store.to_path_buf());
}

// ============================================================================
// Tests: rule-file precedence visible in the read, and the real fixtures.
// ============================================================================

#[cfg(test)]
const SWAP_TRANSACTIONS_TEXT: &str = "# columns: id;date;amount;currency;description;counterparty;source\n39945b2fed;2024-01-03;-3999;EUR;REWE MARKT 12;REWE MARKET;bank_a.pdf\nfbd45cccf1;2024-02-24;-3456;EUR;REWE MARKT 12;REWE MARKET;bank_b1.pdf\n2b5a9d2799;2024-01-24;-123456;EUR;MEGA MART 04;MEGA MART;bank_a.pdf\n";

#[cfg(test)]
const SWAP_BUILT_IN_RULES_TEXT: &str = "REWE MARKT -> food:supermarket\n";

#[cfg(test)]
const SWAP_USER_RULES_TEXT: &str = "REWE MARKT -> food:restaurant\n";

#[cfg(test)]
const SWAP_USER_RULES_SWAPPED_TEXT: &str = "REWE MARKT -> food:supermarket\n";

#[test]
fn rule_file_swap_changes_categories_and_leaves_transactions_csv_bytes_unchanged() {
    let store = make_store("view-rule-swap");
    write_fixture(&store, "transactions.csv", SWAP_TRANSACTIONS_TEXT);
    write_fixture(&store, "rules/10-built-in.rules", SWAP_BUILT_IN_RULES_TEXT);
    write_fixture(&store, "rules/50-user.rules", SWAP_USER_RULES_TEXT);
    // Snapshot the stored facts as raw BYTES before the swap — the assertion
    // below is byte-for-byte, not "same text", so even a line ending or a
    // re-serialised field would show up.
    let bytes_before = match std::fs::read(&store.join("transactions.csv")) {
        Err(_) => { panic!("cannot snapshot the scratch transactions.csv bytes"); }
        Ok(bytes) => { bytes }
    };
    let before = read_view(&store);
    assert!(before.entries.len() == 3);
    assert!(before.rule_errors.len() == 0);
    assert!(before.corrupt_lines == 0);
    // The later file wins (decision 9 + last-wins scan): both REWE rows carry
    // the 50-user category, the MEGA MART row matches neither rule.
    assert!(before.entries[0].category == Classification::Category("food:restaurant".to_string()));
    assert!(before.entries[1].category == Classification::Category("food:restaurant".to_string()));
    assert!(before.entries[2].category == Classification::Unknown);
    // Replace ONE rule file: the 50-user rule now names the built-in category,
    // i.e. the winning rule effectively moves to the 10-built-in meaning.
    write_fixture(&store, "rules/50-user.rules", SWAP_USER_RULES_SWAPPED_TEXT);
    let after = read_view(&store);
    // The categories of ALL matching transactions changed ...
    assert!(after.entries[0].category == Classification::Category("food:supermarket".to_string()));
    assert!(after.entries[1].category == Classification::Category("food:supermarket".to_string()));
    assert!(after.entries[0].category != before.entries[0].category);
    assert!(after.entries[1].category != before.entries[1].category);
    // ... the non-matching row is untouched, and transactions.csv did not move
    // by a single byte: the view is read-only, precedence lives in the read.
    assert!(after.entries[2].category == Classification::Unknown);
    let bytes_after = match std::fs::read(&store.join("transactions.csv")) {
        Err(_) => { panic!("cannot re-read the scratch transactions.csv bytes"); }
        Ok(bytes) => { bytes }
    };
    assert!(bytes_after == bytes_before);
    remove_scratch(store.to_path_buf());
}

/// The verbatim text of `testdata/rules/sample.rules`, embedded so the fixture
/// test does not depend on the file being present at the test's directory.
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

#[test]
fn fixture_rules_and_categories_drive_classification() {
    let store = make_store("view-fixtures");
    write_fixture(&store, "transactions.csv", FIXTURE_TRANSACTIONS_TEXT);
    write_fixture(&store, "rules/10-built-in.rules", FIXTURE_SAMPLE_RULES_TEXT);
    let view = read_view(&store);
    assert!(view.entries.len() == 22);
    assert!(view.corrupt_lines == 0);
    assert!(view.rule_errors.len() == 0);
    // Categories only the sample rules can produce: `PAYMENT TO TOBIAS` matches
    // just `regex:^PAYMENT TO` — `TAXI` sits in the counterparty column, which
    // matching never reads — so `finance:income` comes from that regex rule
    // alone, and `leisure:games` only from `AMAZON MARKETPLACE`.
    assert!(view.entries[2].transaction.id == "dc0db110c4");
    assert!(view.entries[2].category == Classification::Category("finance:income".to_string()));
    assert!(view.entries[13].transaction.id == "5b35bef1fd");
    assert!(view.entries[13].category == Classification::Category("leisure:games".to_string()));
    // The three-level transport path and the restaurant path come from the
    // sample rules' own category columns.
    assert!(view.entries[1].transaction.id == "f596357483");
    assert!(view.entries[1].category == Classification::Category("food:restaurant".to_string()));
    assert!(view.entries[7].transaction.id == "140ef279a4");
    assert!(view.entries[7].category == Classification::Category("finance:expense:transport".to_string()));
    // Rows no sample rule matches stay Unknown — exactly six of the 22.
    assert!(view.entries[4].transaction.id == "2b5a9d2799");
    assert!(view.entries[4].category == Classification::Unknown);
    let mut unknown = 0;
    for entry in view.entries.iter() {
        if entry.category == Classification::Unknown { unknown = unknown + 1; }
    }
    assert!(unknown == 6);
    // Every derived path is a real path of the categories.csv fixture tree,
    // so the classification is driven by the fixtures, not by a constant here.
    match crate::domain::category::from_csv(&FIXTURE_CATEGORIES_TEXT) {
        Err(reason) => { panic!("the embedded categories fixture does not build a tree: {reason:?}"); }
        Ok(tree) => {
            for entry in view.entries.iter() {
                match entry.category {
                    Classification::Category(ref path) => { assert!(crate::domain::category::is_valid(&tree, path)); }
                    Classification::Unknown => { }
                }
            }
        }
    }
    remove_scratch(store.to_path_buf());
}
