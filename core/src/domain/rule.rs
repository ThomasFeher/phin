//! Rule parsing: turning rule-file text into ordered [`Rule`] values.
//!
//! Rule text arrives as a string — the technical layer reads files and hands
//! the contents to this pure function. Lines starting with `#` are comments and
//! empty/whitespace-only lines are ignored. Each rule line is split on the
//! ASCII `->` separator into a pattern side and a category side.
//!
//! Marker precedence on the pattern side: a leading `regex:` marks a
//! [`PatternKind::Regex`] pattern (compiled with `regex::Regex::new`, never
//! panicking), a leading `words:` marks a [`PatternKind::Words`] pattern whose
//! word list is separated by `,`, anything else is [`PatternKind::Literal`].
//!
//! Precedence between rules: a LATER rule beats an EARLIER one, so file order
//! is precedence order and later entries win. Rule files themselves are
//! ordered by a numeric filename prefix (`rules/10-built-in.rules`,
//! `rules/50-user.rules`, `rules/90-learned.txt`).
//!
//! The unicode arrow `→` is NOT accepted as a separator; a line using it (or
//! any other line without an ASCII `->`) is malformed.

use crate::domain::category;

/// A pattern's syntactic form, decided by the marker that prefixes it.
#[derive(Debug, Eq, PartialEq)]
pub enum PatternKind {
    /// Exact text to match as-is.
    Literal,
    /// A comma-separated list of words to match.
    Words,
    /// A regular expression to compile and match.
    Regex,
}

/// One parsed rule: where it was read, its kind, its raw pattern text (with
/// the marker stripped), its split word list, and its `parent:child` category
/// path.
#[derive(Debug, Eq, PartialEq)]
pub struct Rule {
    /// 1-based line number in the source text where the rule was read.
    pub line: usize,
    /// Syntactic form of the pattern.
    pub kind: PatternKind,
    /// Raw pattern text after the marker is stripped.
    pub pattern: String,
    /// Split word list; empty unless `kind == PatternKind::Words`.
    pub words: Vec::<String>,
    /// Category path, e.g. `food:supermarket`.
    pub category: String,
}

/// What can go wrong while parsing one rule line.
#[derive(Debug, Eq, PartialEq)]
pub enum RuleError {
    /// Line had no ASCII `->`, an empty pattern, or an empty category.
    MalformedRule(usize, String),
    /// A `regex:` pattern that `regex::Regex::new` rejected.
    BadRegex(usize, String),
    /// A rule whose category path names a segment the category tree does not
    /// contain; the payload is the 1-based rule line, the offending segment and
    /// the names that were available at that point in the path, taken from
    /// [`domain::category`]'s [`PathError.UnknownCategory`] payload.
    UnknownCategory(usize, String, Vec::<String>),
}

/// Parse rule-file text into rules, preserving file order (later wins).
pub fn parse_rules(text: &str) -> Result::<Vec::<Rule>, RuleError> {
    let mut rules = Vec::<Rule>::new();
    for (index, raw) in text.lines().enumerate() {
        let line = raw.trim();
        if line.len() == 0 || line.starts_with("#") { continue; }
        let parts: Vec::<&str> = line.split("->").collect();
        if parts.len() != 2 {
            return Err(RuleError::MalformedRule(index + 1, "line has no ASCII '->' separator".to_string()));
        }
        let left = parts[0].trim();
        let category = parts[1].trim();
        if left.len() == 0 {
            return Err(RuleError::MalformedRule(index + 1, "line has an empty pattern".to_string()));
        }
        if category.len() == 0 {
            return Err(RuleError::MalformedRule(index + 1, "line has an empty category".to_string()));
        }
        let kind: PatternKind;
        let pattern: &str;
        let mut words = Vec::<String>::new();
        if left.starts_with("regex:") {
            kind = PatternKind::Regex;
            pattern = left["regex:".len()..].trim();
            if pattern.len() == 0 {
                return Err(RuleError::MalformedRule(index + 1, "regex: pattern is empty".to_string()));
            }
            if regex::Regex::new(pattern).is_err() {
                return Err(RuleError::BadRegex(index + 1, "pattern is not a valid regular expression".to_string()));
            }
        } else if left.starts_with("words:") {
            kind = PatternKind::Words;
            pattern = left["words:".len()..].trim();
            if pattern.len() == 0 {
                return Err(RuleError::MalformedRule(index + 1, "words: pattern is empty".to_string()));
            }
            for word in pattern.split(",") {
                let word = word.trim();
                if word.len() != 0 { words.push(word.to_string()); }
            }
            if words.len() == 0 {
                return Err(RuleError::MalformedRule(index + 1, "words: list has no words".to_string()));
            }
        } else {
            kind = PatternKind::Literal;
            pattern = left;
        }
        rules.push(Rule {
            line: index + 1,
            kind: kind,
            pattern: pattern.to_string(),
            words: words,
            category: category.to_string(),
        });
    }
    return Ok(rules);
}

/// Check every rule's category path against the category tree.
///
/// Returns the validated `parent:child` category paths in rule order. Each
/// path is checked with [`domain::category::validate`], so an unknown segment
/// yields [`RuleError.UnknownCategory`] naming the offending rule line, the
/// segment and the candidate names the tree offered at that point. A path with
/// an empty segment (which has no segment to name) is reported as
/// [`RuleError.MalformedRule`] for that line instead.
pub fn validate_rule_categories(rules: Vec::<Rule>, tree: &category::Tree) -> Result::<Vec::<String>, RuleError> {
    let mut categories = Vec::<String>::new();
    for rule in rules.iter() {
        match category::validate(tree, &rule.category) {
            Ok(path) => { categories.push(path); }
            Err(category::PathError::UnknownCategory(segment, candidates)) => {
                return Err(RuleError::UnknownCategory(rule.line, segment, candidates));
            }
            Err(category::PathError::EmptySegment) => {
                return Err(RuleError::MalformedRule(rule.line, "category path has an empty segment".to_string()));
            }
            Err(_) => {
                return Err(RuleError::MalformedRule(rule.line, "category path is not valid in the tree".to_string()));
            }
        }
    }
    return Ok(categories);
}

#[cfg(test)]
fn fixture_categories() -> String {
    let text = "# Phin categories -- tree of parent:child names, order is preserved.\n# columns: parent;name\n;food\n;finance\n;leisure\nfood;supermarket\nfood;restaurant\nfinance;income\nfinance;expense\nexpense;transport\nexpense;utilities\nleisure;books\nleisure;games\n";
    return text.to_string();
}

#[cfg(test)]
fn fixture_rules() -> String {
    let text = "# Phin sample rules -- one rule per line: pattern -> category\n# '#' starts a comment, empty lines are ignored, file order = precedence.\n# The exact pattern syntax is finalised in plan item 1.5; this sample\n# shows literal, word-list and regex patterns.\n\nREWE MARKT -> food:supermarket\nALDI -> food:supermarket\nLIDL -> food:supermarket\nZUR LEITZ -> food:restaurant\nCAFETARIA -> food:restaurant\nTAXI -> finance:expense:transport\nBOOKS ONLINE -> leisure:books\nAMAZON MARKETPLACE -> leisure:games\nregex:^PAYMENT TO -> finance:income\nCLIENT PAYMENT -> finance:income\n";
    return text.to_string();
}

#[test]
fn comment_only_and_blank_file_yields_no_rules() {
    let text = "# only comments\n\n   \n# end\n";
    assert_eq!(parse_rules(text).unwrap().len(), 0);
}

#[test]
fn sample_rules_fixture_parses_in_order() {
    let text = r"# Phin sample rules -- one rule per line: pattern -> category
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
";
    let rules = parse_rules(text).unwrap();
    assert_eq!(rules.len(), 10);
    assert_eq!(rules[0], Rule { line: 6, kind: PatternKind::Literal, pattern: "REWE MARKT".to_string(), words: Vec::<String>::new(), category: "food:supermarket".to_string() });
    assert_eq!(rules[8], Rule { line: 14, kind: PatternKind::Regex, pattern: "^PAYMENT TO".to_string(), words: Vec::<String>::new(), category: "finance:income".to_string() });
    assert_eq!(rules[9], Rule { line: 15, kind: PatternKind::Literal, pattern: "CLIENT PAYMENT".to_string(), words: Vec::<String>::new(), category: "finance:income".to_string() });
}

#[test]
fn missing_arrow_is_malformed_with_line_number() {
    let text = "ALDI -> food:supermarket\nJUST NO ARROW\n";
    assert_eq!(parse_rules(text), Err(RuleError::MalformedRule(2, "line has no ASCII '->' separator".to_string())));
}

#[test]
fn empty_category_is_malformed_with_line_number() {
    let text = "\nALDI ->   \n";
    assert_eq!(parse_rules(text), Err(RuleError::MalformedRule(2, "line has an empty category".to_string())));
}

#[test]
fn unicode_arrow_is_not_accepted() {
    let text = "ALDI → food:supermarket\n";
    assert_eq!(parse_rules(text), Err(RuleError::MalformedRule(1, "line has no ASCII '->' separator".to_string())));
}

#[test]
fn words_marker_splits_on_commas() {
    let text = "words:REWE, MARKT -> food:supermarket\n";
    let rules = parse_rules(text).unwrap();
    assert_eq!(rules.len(), 1);
    let mut expected_words = Vec::<String>::new();
    expected_words.push("REWE".to_string());
    expected_words.push("MARKT".to_string());
    assert_eq!(rules[0], Rule { line: 1, kind: PatternKind::Words, pattern: "REWE, MARKT".to_string(), words: expected_words, category: "food:supermarket".to_string() });
}

#[test]
fn invalid_regex_reports_bad_regex() {
    let text = "regex:[ -> finance:income\n";
    assert_eq!(parse_rules(text), Err(RuleError::BadRegex(1, "pattern is not a valid regular expression".to_string())));
}

#[test]
fn all_fixture_rules_validate_against_fixture_tree() {
    let tree = category::from_csv(&fixture_categories()).unwrap();
    let rules = parse_rules(&fixture_rules()).unwrap();
    let mut expected = Vec::<String>::new();
    for rule in rules.iter() { expected.push(rule.category.clone()); }
    let categories = validate_rule_categories(rules, &tree).unwrap();
    assert_eq!(categories.len(), 10);
    assert_eq!(categories, expected);
}

#[test]
fn bad_rule_category_names_line_segment_and_candidates() {
    let text = "ALDI -> food:supermarket\nNEWCATEGORIES -> finance:travel\n";
    let tree = category::from_csv(&fixture_categories()).unwrap();
    let rules = parse_rules(&text).unwrap();
    let error = validate_rule_categories(rules, &tree).unwrap_err();
    match error {
        RuleError::UnknownCategory(line, segment, candidates) => {
            assert_eq!(line, 2);
            assert_eq!(segment, "travel".to_string());
            assert!(candidates.len() > 0);
            assert_eq!(candidates, category::children(&tree, "finance"));
        }
        _ => { assert!(false, "expected UnknownCategory"); }
    }
}

#[test]
fn empty_segment_in_rule_category_is_malformed_rule() {
    let text = "ALDI -> food:\n";
    let tree = category::from_csv(&fixture_categories()).unwrap();
    let rules = parse_rules(&text).unwrap();
    match validate_rule_categories(rules, &tree) {
        Err(RuleError::MalformedRule(line, reason)) => {
            assert_eq!(line, 1);
            assert_eq!(reason, "category path has an empty segment".to_string());
        }
        _ => { assert!(false, "expected MalformedRule for line 1"); }
    }
}

#[test]
fn no_input_above_panics() {
    let nasty = ["", "   ", "#x", "a->b", " -> x", "a -> ", "regex: -> x", "words:, -> x", "a → b", "a->b->c"];
    for text in nasty.iter() {
        match parse_rules(text) {
            Ok(rules) => { assert!(rules.len() <= 1); }
            Err(_) => {}
        };
    }
}
