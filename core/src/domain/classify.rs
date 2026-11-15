//! Classification: deriving a category for one transaction from rules and
//! overrides — the pure heart of plan item 1.6.
//!
//! Everything this function needs arrives as already-parsed DATA: the
//! [`Transaction`] and the [`Rule`] list come from the technical layer, which
//! reads `transactions.csv` and the `rules/` files; the [`Override`] list is
//! the parsed content of `overrides.csv` (planning decision 11: the domain
//! never reads a file). The category is therefore a pure function of data,
//! never of the clock, the environment or a hash seed.
//!
//! PRECEDENCE (planning decision 11, run decision 26 "later beats earlier"):
//! 1. An [`Override`] whose `id` equals the transaction's `id` wins over EVERY
//!    rule. If several overrides name the same id, the LAST one in the list
//!    wins, because `overrides.csv` is append-only and a superseded entry stays
//!    in the file (doc/interfaces/data-files.md).
//! 2. Otherwise the LAST matching [`Rule`] in list order wins. A caller that
//!    passes the built-in rules first and the learned rules last therefore gets
//!    learned > built-in, exactly as plan item 1.6 requires, without this module
//!    knowing anything about rule files or their numeric filename prefixes.
//! 3. Nothing matched → [`Classification::Unknown`].
//!
//! MATCHING is against `transaction.description` ONLY — the merchant text the
//! plan's "merchant/description pattern" wording and plan item 1.8 build on.
//! [`PatternKind::Literal`] is a substring test, [`PatternKind::Words`] needs
//! EVERY listed word to be present, [`PatternKind::Regex`] is
//! `regex::Regex::is_match`. Word presence is a plain substring test, so
//! `words:NIGHT` also matches `MIDNIGHT`; tokenizing on punctuation is left to
//! the pattern author, who can use a `regex:` rule with `\b` instead.
//! Case handling is documented on [`classify`]:
//! Literal and Words are ASCII case-INSENSITIVE (fixture descriptions and rule
//! patterns are UPPER-case while user rules are usually lower-case), Regex is
//! applied EXACTLY as written, so a regex stays case-sensitive unless its own
//! pattern says otherwise (e.g. `regex:(?i)rewe`). Regex patterns are left
//! alone because silently prefixing `(?i)` would change a pattern the user
//! wrote and validated in plan item 1.5.
//!
//! REGEX INPUT GUARD: a description longer than [`MAX_REGEX_INPUT`] code points
//! is never handed to the regular-expression engine, so a pathological pattern
//! such as `(a+)+b` cannot be driven into catastrophic backtracking by an
//! arbitrarily long OCR description. Such a transaction can still match
//! Literal and Words rules; only its Regex rules are skipped. The guard is a
//! length limit, not a timeout, so it is deterministic and pure.

use crate::domain::rule;
use crate::domain::transaction;

/// Maximum description length, in Unicode code points, that is offered to a
/// regular-expression engine. Longer descriptions skip every [`PatternKind::Regex`]
/// rule (see the module doc for why).
pub const MAX_REGEX_INPUT: usize = 512;

/// One parsed `overrides.csv` record: the columns are `id;category;reason`
/// (doc/interfaces/data-files.md), so the struct mirrors them exactly. It is
/// built by the technical layer from the CSV line; this module only reads it.
#[derive(Debug, Eq, PartialEq)]
pub struct Override {
    /// The transaction whose classification this record fixes.
    pub id: String,
    /// The category path the user chose, e.g. `food:supermarket`.
    pub category: String,
    /// Free text why; carried for the UI and never used in matching.
    pub reason: String,
}

/// The outcome of [`classify`]: a category path, or no category at all.
#[derive(Debug, Eq, PartialEq)]
pub enum Classification {
    /// A category path, e.g. `finance:expense:transport`.
    Category(String),
    /// No override and no rule matched.
    Unknown,
}

/// Derive a transaction's category from rules and overrides; pure and
/// deterministic.
///
/// Precedence is documented in the module doc: an [`Override`] for this `id`
/// beats every rule, otherwise the LAST matching [`Rule`] in list order wins,
/// otherwise [`Classification::Unknown`].
///
/// Matching reads `transaction.description` only. Literal and Words patterns
/// are ASCII case-INSENSITIVE; a Regex pattern is applied exactly as written
/// and only when the description is at most [`MAX_REGEX_INPUT`] code points
/// long, which keeps a catastrophic pattern such as `(a+)+b` from ever seeing
/// an unbounded input.
pub fn classify(transaction: &transaction::Transaction, rules: &Vec::<rule::Rule>, overrides: &Vec::<Override>) -> Classification {
    for entry in overrides.iter().rev() {
        if entry.id == transaction.id {
            return Classification::Category(entry.category.to_string());
        }
    }
    let regex_allowed: bool = count_code_points(&transaction.description) <= MAX_REGEX_INPUT;
    for rule in rules.iter().rev() {
        if matches(rule, &transaction.description, regex_allowed) {
            return Classification::Category(rule.category.to_string());
        }
    }
    return Classification::Unknown;
}

/// Does one rule match the description, honouring the regex length guard?
///
/// `false` for a [`PatternKind::Regex`] rule when `regex_allowed` is false. A
/// Regex rule whose pattern fails to compile is treated as not matching rather
/// than a panic; [`crate::domain::rule::parse_rules`] already rejects such
/// patterns, so this is only defensive.
fn matches(rule: &rule::Rule, description: &str, regex_allowed: bool) -> bool {
    match rule.kind {
        rule::PatternKind::Literal => { return contains_ignore_case(description, &rule.pattern); }
        rule::PatternKind::Words => {
            if rule.words.len() == 0 { return false; }
            for word in rule.words.iter() {
                if !contains_ignore_case(description, word) { return false; }
            }
            return true;
        }
        rule::PatternKind::Regex => {
            if !regex_allowed { return false; }
            match regex::Regex::new(&rule.pattern) {
                Err(_) => { return false; }
                Ok(expression) => { return expression.is_match(description); }
            }
        }
    }
}

/// Case-insensitive substring test on ASCII only.
///
/// Non-ASCII code points are compared as-is; only `A`-`Z` fold to `a`-`z`.
/// An empty needle matches, which cannot happen for rules that
/// [`crate::domain::rule::parse_rules`] produced.
fn contains_ignore_case(description: &str, needle: &str) -> bool {
    let haystack = fold_ascii(description);
    let needle_folded = fold_ascii(needle);
    return haystack.contains(needle_folded.as_str());
}

/// ASCII-fold `A`-`Z` to lower case, leaving every other code point alone.
fn fold_ascii(text: &str) -> String {
    let mut folded = String::new();
    for character in text.chars() {
        if character >= 'A' && character <= 'Z' {
            let lowered = char::from_u32((character as u32) + 32).unwrap_or(character);
            folded.push(lowered);
        } else {
            folded.push(character);
        }
    }
    return folded;
}

/// Code points in `text`, the unit [`MAX_REGEX_INPUT`] is counted in.
fn count_code_points(text: &str) -> usize {
    return text.chars().count();
}
#[cfg(test)]
fn build_row(id: &str, description: &str) -> transaction::Transaction {
    return transaction::from_parts(id, "2024-01-03", "-39,99 €", "EUR", description, "REWE MARKET", "bank_a.pdf").unwrap();
}

#[cfg(test)]
fn make_rule(line: usize, kind: rule::PatternKind, pattern: &str, category: &str) -> rule::Rule {
    return rule::Rule { line: line, kind: kind, pattern: pattern.to_string(), words: Vec::<String>::new(), category: category.to_string() };
}

#[cfg(test)]
fn make_words_rule(line: usize, pattern: &str, words: Vec::<String>, category: &str) -> rule::Rule {
    return rule::Rule { line: line, kind: rule::PatternKind::Words, pattern: pattern.to_string(), words: words, category: category.to_string() };
}

#[cfg(test)]
fn make_override(id: &str, category: &str) -> Override {
    return Override { id: id.to_string(), category: category.to_string(), reason: "user chose this".to_string() };
}

#[cfg(test)]
fn no_rules() -> Vec::<rule::Rule> {
    return Vec::<rule::Rule>::new();
}

#[cfg(test)]
fn no_overrides() -> Vec::<Override> {
    return Vec::<Override>::new();
}

#[test]
fn override_beats_every_rule() {
    let row = build_row("39945b2fed", "REWE MARKT 12");
    let mut rules = Vec::<rule::Rule>::new();
    rules.push(make_rule(1, rule::PatternKind::Literal, "REWE MARKT", "food:supermarket"));
    rules.push(make_rule(2, rule::PatternKind::Literal, "REWE", "finance:expense"));
    let mut overrides = Vec::<Override>::new();
    overrides.push(make_override("39945b2fed", "leisure:games"));
    assert_eq!(classify(&row, &rules, &overrides), Classification::Category("leisure:games".to_string()));
}

#[test]
fn later_learned_rule_beats_earlier_builtin_rule() {
    let row = build_row("39945b2fed", "REWE MARKT 12");
    let mut rules = Vec::<rule::Rule>::new();
    rules.push(make_rule(1, rule::PatternKind::Literal, "REWE MARKT", "food:supermarket"));
    rules.push(make_rule(2, rule::PatternKind::Literal, "REWE", "finance:expense"));
    assert_eq!(classify(&row, &rules, &no_overrides()), Classification::Category("finance:expense".to_string()));
    let mut reversed = Vec::<rule::Rule>::new();
    reversed.push(make_rule(2, rule::PatternKind::Literal, "REWE", "finance:expense"));
    reversed.push(make_rule(1, rule::PatternKind::Literal, "REWE MARKT", "food:supermarket"));
    assert_eq!(classify(&row, &reversed, &no_overrides()), Classification::Category("food:supermarket".to_string()));
}

#[test]
fn earlier_rule_wins_when_it_is_the_only_match() {
    let row = build_row("39945b2fed", "REWE MARKT 12");
    let mut rules = Vec::<rule::Rule>::new();
    rules.push(make_rule(1, rule::PatternKind::Literal, "REWE MARKT", "food:supermarket"));
    rules.push(make_rule(2, rule::PatternKind::Literal, "ALDI", "food:supermarket"));
    assert_eq!(classify(&row, &rules, &no_overrides()), Classification::Category("food:supermarket".to_string()));
}

#[test]
fn last_override_for_an_id_wins() {
    let row = build_row("39945b2fed", "REWE MARKT 12");
    let mut overrides = Vec::<Override>::new();
    overrides.push(make_override("39945b2fed", "food:restaurant"));
    overrides.push(make_override("39945b2fed", "leisure:books"));
    assert_eq!(classify(&row, &no_rules(), &overrides), Classification::Category("leisure:books".to_string()));
}

#[test]
fn no_match_is_unknown_and_stable_across_calls() {
    let row = build_row("dc0db110c4", "PAYMENT TO TOBIAS");
    let mut rules = Vec::<rule::Rule>::new();
    rules.push(make_rule(1, rule::PatternKind::Literal, "REWE MARKT", "food:supermarket"));
    let first = classify(&row, &rules, &no_overrides());
    let second = classify(&row, &rules, &no_overrides());
    assert_eq!(first, Classification::Unknown);
    assert_eq!(first, second);
}

#[test]
fn override_for_another_id_does_not_apply() {
    let row = build_row("39945b2fed", "REWE MARKT 12");
    let mut rules = Vec::<rule::Rule>::new();
    rules.push(make_rule(1, rule::PatternKind::Literal, "REWE MARKT", "food:supermarket"));
    let mut overrides = Vec::<Override>::new();
    overrides.push(make_override("ffffffffff", "leisure:games"));
    assert_eq!(classify(&row, &rules, &overrides), Classification::Category("food:supermarket".to_string()));
}

#[test]
fn words_rule_needs_every_word_present() {
    let row = build_row("140ef279a4", "TAXI AIRPORT; NIGHT");
    let mut words = Vec::<String>::new();
    words.push("taxi".to_string());
    words.push("night".to_string());
    let mut rules = Vec::<rule::Rule>::new();
    rules.push(make_words_rule(1, "taxi,night", words, "finance:expense:transport"));
    assert_eq!(classify(&row, &rules, &no_overrides()), Classification::Category("finance:expense:transport".to_string()));
    let missing = build_row("140ef279a4", "TAXI AIRPORT");
    assert_eq!(classify(&missing, &rules, &no_overrides()), Classification::Unknown);
}

#[test]
fn literal_and_words_are_case_insensitive_but_regex_is_not() {
    let row = build_row("39945b2fed", "rewe markt 12");
    let mut rules = Vec::<rule::Rule>::new();
    rules.push(make_rule(1, rule::PatternKind::Literal, "REWE MARKT", "food:supermarket"));
    assert_eq!(classify(&row, &rules, &no_overrides()), Classification::Category("food:supermarket".to_string()));
    let mut regex_rules = Vec::<rule::Rule>::new();
    regex_rules.push(make_rule(1, rule::PatternKind::Regex, "^payment to", "finance:income"));
    let upper = build_row("39945b2fed", "PAYMENT TO TOBIAS");
    assert_eq!(classify(&upper, &regex_rules, &no_overrides()), Classification::Unknown);
    let lowered = build_row("39945b2fed", "payment to tobius");
    assert_eq!(classify(&lowered, &regex_rules, &no_overrides()), Classification::Category("finance:income".to_string()));
}

#[test]
fn over_long_description_skips_regex_rules_without_backtracking() {
    let mut rules = Vec::<rule::Rule>::new();
    rules.push(make_rule(1, rule::PatternKind::Regex, "(a+)+b", "finance:income"));
    let short_text = "a".repeat(60);
    let long_text = "a".repeat(MAX_REGEX_INPUT + 1);
    let truncated = build_row("aaaaaaaaaa", short_text.as_str());
    let over_long = build_row("bbbbbbbbbb", long_text.as_str());
    assert_eq!(classify(&truncated, &rules, &no_overrides()), Classification::Unknown);
    assert_eq!(classify(&over_long, &rules, &no_overrides()), Classification::Unknown);
    let mut literal_rules = Vec::<rule::Rule>::new();
    literal_rules.push(make_rule(1, rule::PatternKind::Literal, "aaa", "food:supermarket"));
    assert_eq!(classify(&over_long, &literal_rules, &no_overrides()), Classification::Category("food:supermarket".to_string()));
}

#[test]
fn performance_many_rules_over_many_transactions() {
    let mut rules = Vec::<rule::Rule>::new();
    for index in 0..500 {
        if index % 3 == 0 {
            rules.push(make_rule(index + 1, rule::PatternKind::Literal, "REWE MARKT", "food:supermarket"));
        } else if index % 3 == 1 {
            let mut words = Vec::<String>::new();
            words.push("taxi".to_string());
            words.push("night".to_string());
            rules.push(make_words_rule(index + 1, "taxi,night", words, "finance:expense:transport"));
        } else {
            rules.push(make_rule(index + 1, rule::PatternKind::Literal, "PAYMENT TO", "finance:income"));
        }
    }
    let mut rows = Vec::<transaction::Transaction>::new();
    for index in 0..1000 {
        if index % 3 == 0 {
            rows.push(build_row(&index.to_string(), "REWE MARKT 12"));
        } else if index % 3 == 1 {
            rows.push(build_row(&index.to_string(), "TAXI AIRPORT; NIGHT"));
        } else {
            rows.push(build_row(&index.to_string(), "PAYMENT TO TOBIAS"));
        }
    }
    let mut overrides = Vec::<Override>::new();
    overrides.push(make_override("no-such-id", "leisure:games"));
    let mut categories = 0;
    for row in rows.iter() {
        match classify(row, &rules, &overrides) {
            Classification::Category(_) => { categories += 1; }
            Classification::Unknown => { }
        }
    }
    assert_eq!(categories, 1000);
}
