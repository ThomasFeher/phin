//! Learning a rule from a correction: plan item 1.8.
//!
//! When the user re-categorizes a transaction, the domain proposes a general
//! rule that would have produced that category, plus how many transactions in
//! the given set the rule would affect. The proposal is DATA, never a file:
//! appending `rules/90-learned.txt` is the technical layer's job in a later
//! item, and the transaction set arrives here as already-parsed values.
//!
//! GENERALIZATION CONTRACT (documented rule by rule):
//! - Tokenize the seed description on whitespace and on every character that
//!   is not an ASCII letter or digit, so `TAXI AIRPORT; NIGHT` yields the
//!   tokens `TAXI`, `AIRPORT`, `NIGHT`.
//! - `shared` = the longest token-prefix of the seed description that also
//!   starts the token list of at least ONE OTHER transaction's description
//!   (a "prefix-run" at position 0; "other" means a different `id`, since the
//!   seed itself is part of the set).
//! - The candidate is the longest such prefix that is a PROPER prefix of the
//!   description (at most `tokens.len() - 1` tokens) and has at least 2
//!   tokens; it renders as the tokens joined by single spaces.
//! - The candidate becomes a [`PatternKind::Literal`] rule, NOT a `words:`
//!   rule: merchant names are contiguous prefixes (`REWE MARKT`, not
//!   `REWE`+`MARKT` anywhere), and Literal's case-insensitive substring
//!   semantics keep that contiguity, while `words:` would also match
//!   `MARKT REWE` and other scattered word combinations.
//! - The candidate is VERIFIED against the seed with plan item 1.6's matcher
//!   before it is offered: a Literal rule is a case-insensitive SUBSTRING of
//!   `transaction.description`, so joining tokens with single spaces can build
//!   a pattern that is not a substring of any description (the seed
//!   `TAXI;AIRPORT REST` does not contain `TAXI AIRPORT`; the `;` sits inside
//!   the token run). When the longest candidate does not match the seed, it is
//!   SHORTENED one token at a time, down to the 2-token floor, and re-verified;
//!   a candidate that still does not match yields
//!   [`RefusalReason::PatternDoesNotMatchSeed`] instead of an `Accepted` whose
//!   affected count would silently exclude the seed.
//! - Refusals are TYPED results, not errors: [`RefusalReason::UniqueDescription`]
//!   when the description shares no leading token with any other transaction
//!   (nothing to generalize), [`RefusalReason::SingleTokenTooNarrow`] when the
//!   only shared prefix is a single token (the plan calls one token too
//!   narrow), and [`RefusalReason::FullDescriptionOnly`] when the only shared
//!   prefix would be the whole description (a duplicate row, no
//!   generalization possible), and [`RefusalReason::PatternDoesNotMatchSeed`]
//!   when no candidate of at least 2 tokens matches the seed under 1.6's
//!   substring semantics.
//! - The affected count is "transactions this rule would classify", computed
//!   by handing a one-rule list to [`classify::classify`] (reusing plan item
//!   1.6's matching semantics, never re-implementing them) and counting
//!   [`Classification::Category`] results; the seed transaction itself is
//!   included, because a candidate is only ever offered once 1.6 has matched
//!   it against the seed.
//! - The proposed rule must re-parse via plan item 1.5: [`rule_to_text`]
//!   renders the rule line the parser accepts. `line` is 0 because a learned
//!   rule was not read from a file; the core appends it later.

use crate::domain::classify;
use crate::domain::rule;
use crate::domain::transaction;

/// A typed refusal: why nothing could be learned. These are RESULT values the
/// caller shows the user, not [`BuildError`]-style failures.
#[derive(Debug, Eq, PartialEq)]
pub enum RefusalReason {
    /// The description shares no leading token with any other transaction.
    UniqueDescription,
    /// The only shared prefix is a single token, which is too narrow.
    SingleTokenTooNarrow,
    /// The only candidate would be the whole description (duplicate row).
    FullDescriptionOnly,
    /// No candidate of at least 2 tokens is a substring of the seed
    /// description, so the space-joined token run could never classify the
    /// seed itself (e.g. the seed `TAXI;AIRPORT REST`, whose tokens `TAXI`
    /// and `AIRPORT` are glued by a `;`).
    PatternDoesNotMatchSeed,
}

/// The outcome of [`propose_rule`]: a rule plus its affected-transaction
/// count, or a typed refusal.
#[derive(Debug, Eq, PartialEq)]
pub enum Proposal {
    /// The proposed rule and how many transactions of the set it classifies.
    Accepted(rule::Rule, usize),
    /// Nothing could be learned; the payload says why.
    Refused(RefusalReason),
}

/// Render a rule as the text [`crate::domain::rule::parse_rules`] accepts, so
/// a learned rule round-trips through plan item 1.5's parser.
///
/// A [`PatternKind::Literal`] rule renders as its plain pattern. A
/// [`PatternKind::Words`] rule renders with the `words:` marker and its word
/// list joined by `, ` — the canonical form of the pattern, rebuilt from the
/// parsed word list rather than the raw text, so re-parsing is stable AND
/// identical: [`crate::domain::rule::parse_rules`] splits on `,` and trims
/// each word, so `words:REWE, MARKT` reparses to the same `pattern` and the
/// same word list, making the round-trip a full-struct equality. A
/// [`PatternKind::Regex`] rule renders with the `regex:` marker. Every form
/// ends with ` -> category`.
pub fn rule_to_text(rule: &rule::Rule) -> String {
    let body: String = match rule.kind {
        rule::PatternKind::Literal => { rule.pattern.to_string() }
        rule::PatternKind::Words => { "words:".to_owned() + &rule.words.join(", ") }
        rule::PatternKind::Regex => { "regex:".to_owned() + &rule.pattern }
    };
    return body + " -> " + &rule.category;
}

/// Split text into runs of ASCII letters and digits; everything else (spaces,
/// `;`, `.`, `,`) is a separator.
fn tokenize(text: &str) -> Vec::<String> {
    let mut tokens = Vec::<String>::new();
    let mut current = "".to_string();
    for character in text.chars() {
        let is_token_char = (character >= 'a' && character <= 'z')
            || (character >= 'A' && character <= 'Z')
            || (character >= '0' && character <= '9');
        if is_token_char {
            current.push_str(character.to_string().as_str());
        } else {
            if current.len() != 0 { tokens.push(current.to_string()); }
            current = "".to_string();
        }
    }
    if current.len() != 0 { tokens.push(current.to_string()); }
    return tokens;
}

/// Length of the common token-prefix of two token lists (prefix-run at
/// position 0).
fn common_prefix_length(left: &Vec::<String>, right: &Vec::<String>) -> usize {
    let limit = if left.len() < right.len() { left.len() } else { right.len() };
    let mut length = 0;
    while length < limit {
        if left[length] != right[length] { break; }
        length += 1;
    }
    return length;
}

/// A one-rule list holding a single [`PatternKind::Literal`] rule: the shape
/// [`propose_rule`] hands to [`classify::classify`]. `line` is 0 because the
/// rule was learned, not read from a file, and `category` is only carried for
/// [`classify`]'s result; matching never reads it.
fn one_literal_rule(pattern: &str, category: &str) -> Vec::<rule::Rule> {
    let mut rules = Vec::<rule::Rule>::new();
    rules.push(rule::Rule { line: 0, kind: rule::PatternKind::Literal, pattern: pattern.to_string(), words: Vec::<String>::new(), category: category.to_string() });
    return rules;
}

/// Does the candidate pattern match the seed under plan item 1.6's semantics?
/// Reuses [`classify::classify`] with a one-rule list rather than repeating
/// 1.6's case-insensitive substring test here, so verification and counting
/// can never drift apart.
fn seed_matches(transaction: &transaction::Transaction, pattern: &str, category: &str) -> bool {
    let one_rule = one_literal_rule(pattern, category);
    match classify::classify(transaction, &one_rule, &Vec::<classify::Override>::new()) {
        classify::Classification::Category(_) => { return true; }
        classify::Classification::Unknown => { return false; }
    }
}

/// How many transactions of `transactions` a one-rule Literal list would
/// classify, counted with [`classify::classify`] so the number is exactly
/// plan item 1.6's semantics, never a re-implementation.
fn count_affected(transactions: &Vec::<transaction::Transaction>, pattern: &str, category: &str) -> usize {
    let one_rule = one_literal_rule(pattern, category);
    let mut affected = 0;
    for entry in transactions.iter() {
        match classify::classify(entry, &one_rule, &Vec::<classify::Override>::new()) {
            classify::Classification::Category(_) => { affected += 1; }
            classify::Classification::Unknown => { }
        }
    }
    return affected;
}

/// Propose a generalizing rule for `transaction` recategorized to
/// `category`, and count the transactions of `transactions` it would
/// classify.
///
/// The generalization contract is documented in the module doc: the
/// candidate is the longest token-prefix of the description that is a proper
/// prefix (at most one token fewer than the whole description), has at least
/// two tokens, and starts the token list of at least one OTHER transaction
/// (a different `id`). It is then verified against the seed with
/// [`classify::classify`]: a Literal rule matches as a case-insensitive
/// SUBSTRING, so a space-joined token run can fail to be a substring of the
/// seed; the candidate is then shortened one token at a time, down to the
/// 2-token floor, and re-verified, and a candidate that still fails yields
/// [`RefusalReason::PatternDoesNotMatchSeed`]. The rule that is offered
/// therefore always classifies the seed, so the affected count includes it.
/// It becomes a [`PatternKind::Literal`] rule with `line: 0` — learned rules
/// are not read from a rule file, so there is no source line to report; the
/// core appends the rendered text later and [`crate::domain::rule::parse_rules`]
/// stamps the line at that point.
///
/// The affected count reuses [`classify::classify`] with a one-rule list, so
/// it counts exactly the transactions plan item 1.6 would classify —
/// including the seed itself.
pub fn propose_rule(transaction: &transaction::Transaction, category: &str, transactions: &Vec::<transaction::Transaction>) -> Proposal {
    let tokens = tokenize(&transaction.description);
    let mut shared = 0;
    for other in transactions.iter() {
        if other.id == transaction.id { continue; }
        let length = common_prefix_length(&tokens, &tokenize(&other.description));
        if length > shared { shared = length; }
    }
    let max_proper = if tokens.len() == 0 { 0 } else { tokens.len() - 1 };
    let candidate_length = if shared < max_proper { shared } else { max_proper };
    if candidate_length < 2 {
        if shared >= tokens.len() && shared != 0 {
            return Proposal::Refused(RefusalReason::FullDescriptionOnly);
        }
        if shared == 1 {
            return Proposal::Refused(RefusalReason::SingleTokenTooNarrow);
        }
        return Proposal::Refused(RefusalReason::UniqueDescription);
    }
    let mut length = candidate_length;
    let mut pattern = "".to_string();
    let mut matches_seed = false;
    while length >= 2 {
        let mut parts = Vec::<String>::new();
        for token in tokens[..length].iter() { parts.push(token.to_string()); }
        pattern = parts.join(" ");
        if seed_matches(transaction, &pattern, category) { matches_seed = true; break; }
        length -= 1;
    }
    if !matches_seed {
        return Proposal::Refused(RefusalReason::PatternDoesNotMatchSeed);
    }
    let affected = count_affected(transactions, &pattern, category);
    return Proposal::Accepted(rule::Rule {
        line: 0,
        kind: rule::PatternKind::Literal,
        pattern: pattern,
        words: Vec::<String>::new(),
        category: category.to_string(),
    }, affected);
}

#[cfg(test)]
fn fixture_rows() -> Vec::<transaction::Transaction> {
    // Byte-identical stored field values of the rows of testdata/transactions.csv.
    let raw = [
        ("39945b2fed", "2024-01-03", -3999, "REWE MARKT 12", "REWE MARKET", "bank_a.pdf"),
        ("f596357483", "2024-01-09", -5990, "DINNER ZUR LEITZ", "ZUR LEITZ", "bank_a.pdf"),
        ("dc0db110c4", "2024-01-15", 42000, "PAYMENT TO TOBIAS", "TAXI BERLIN", "bank_a.pdf"),
        ("3d7478d7cc", "2024-01-21", -1847, "BOOKS ONLINE", "THONIX", "bank_a.pdf"),
        ("2b5a9d2799", "2024-01-24", -123456, "MEGA MART 04", "MEGA MART", "bank_a.pdf"),
        ("7a8051207a", "2024-01-28", -12990, "LIDL 0815", "LIDL", "bank_a.pdf"),
        ("a86d11da9a", "2024-02-03", -8990, "ALDI 4711", "ALDI", "bank_b1.pdf"),
        ("140ef279a4", "2024-02-11", -2500, "TAXI AIRPORT; NIGHT", "TAXI BERLIN", "bank_b1.pdf"),
        ("e86f3fc214", "2024-02-17", 85000, "CLIENT PAYMENT", "ALICE", "bank_b1.pdf"),
        ("fbd45cccf1", "2024-02-24", -3456, "REWE MARKT 12", "REWE MARKET", "bank_b1.pdf"),
        ("58ed99aa4c", "2024-03-02", -1590, "CAFETARIA", "CAFETARIA", "bank_b2.pdf"),
        ("80c63211a8", "2024-03-05", -1500, "REWE MARKT 12", "REWE MARKET", "bank_b2.pdf"),
        ("80c63211a8#2", "2024-03-05", -1500, "REWE MARKT 12", "REWE MARKET", "bank_b2.pdf"),
        ("5b35bef1fd", "2024-03-19", -6789, "AMAZON MARKETPLACE", "AMZN", "bank_b2.pdf"),
        ("79caf386d4", "2024-04-02", 2599, "LEGO STORE DE", "LEGO STORE", "mail_paypal.mbox"),
        ("4fd4783f1e", "2024-04-05", -1999, "NETFLIX.COM", "NETFLIX", "mail_paypal.mbox"),
        ("30fc834ea9", "2024-04-09", 899, "BAKERY BERLIN", "BAKERY BERLIN", "mail_paypal.mbox"),
        ("bad5886754", "2024-04-12", -4500, "STEAM PURCHASE", "VALVE", "mail_paypal.mbox"),
        ("0cfe0127e0", "2024-04-15", 12000, "CLIENT PAYMENT", "ALICE", "mail_paypal.mbox"),
        ("3d3a5287f1", "2024-04-18", -799, "BOOKS ONLINE", "THONIX", "mail_paypal.mbox"),
        ("06113c369c", "2024-04-21", 3450, "SUPERMARKET; DELICATES", "DELICATES", "mail_paypal.mbox"),
        ("f9aeec83dc", "2024-04-26", -2999, "TAXI AIRPORT; NIGHT", "TAXI BERLIN", "mail_paypal.mbox"),
    ];
    let mut rows = Vec::<transaction::Transaction>::new();
    for entry in raw.iter() {
        rows.push(transaction::from_stored(entry.0, entry.1, entry.2, "EUR", entry.3, entry.4, entry.5).unwrap());
    }
    return rows;
}

#[cfg(test)]
fn row(id: &str, description: &str) -> transaction::Transaction {
    return transaction::from_stored(id, "2024-01-03", -100, "EUR", description, "", "test").unwrap();
}

#[test]
fn rewe_seed_proposes_prefix_rule_not_full_description() {
    let rows = fixture_rows();
    let seed = row("39945b2fed", "REWE MARKT 12");
    match propose_rule(&seed, "food:supermarket", &rows) {
        Proposal::Accepted(proposed, affected) => {
            // Hand count of `REWE MARKT 12` rows in the fixture: ids
            // 39945b2fed, fbd45cccf1, 80c63211a8, 80c63211a8#2 = 4.
            assert_eq!(proposed.pattern, "REWE MARKT".to_string());
            assert_ne!(proposed.pattern, seed.description);
            assert_eq!(affected, 4);
            assert_eq!(proposed.kind, rule::PatternKind::Literal);
        }
        Proposal::Refused(reason) => { assert!(false, "expected Accepted, got {:?}", reason); }
    }
}

#[test]
fn unique_description_is_refused_with_a_typed_reason() {
    let mut rows = Vec::<transaction::Transaction>::new();
    rows.push(row("aaaaaaaaaa", "NETFLIX.COM"));
    rows.push(row("bbbbbbbbbb", "STEAM PURCHASE"));
    let seed = row("aaaaaaaaaa", "NETFLIX.COM");
    assert_eq!(propose_rule(&seed, "leisure:games", &rows), Proposal::Refused(RefusalReason::UniqueDescription));
}

#[test]
fn single_shared_token_is_too_narrow_and_refused() {
    let mut rows = Vec::<transaction::Transaction>::new();
    rows.push(row("aaaaaaaaaa", "CAFETARIA BERLIN"));
    rows.push(row("bbbbbbbbbb", "CAFETARIA CAFE"));
    let seed = row("aaaaaaaaaa", "CAFETARIA BERLIN");
    assert_eq!(propose_rule(&seed, "food:restaurant", &rows), Proposal::Refused(RefusalReason::SingleTokenTooNarrow));
}

#[test]
fn duplicate_row_offers_only_the_full_description_and_refuses() {
    let mut rows = Vec::<transaction::Transaction>::new();
    rows.push(row("aaaaaaaaaa", "BOOKS ONLINE"));
    rows.push(row("bbbbbbbbbb", "BOOKS ONLINE"));
    let seed = row("aaaaaaaaaa", "BOOKS ONLINE");
    assert_eq!(propose_rule(&seed, "leisure:books", &rows), Proposal::Refused(RefusalReason::FullDescriptionOnly));
}

#[test]
fn proposed_rule_round_trips_through_the_rule_parser() {
    let rows = fixture_rows();
    let seed = row("39945b2fed", "REWE MARKT 12");
    let proposal = propose_rule(&seed, "food:supermarket", &rows);
    match proposal {
        Proposal::Accepted(proposed, _) => {
            let text = rule_to_text(&proposed);
            let parsed = rule::parse_rules(&text).unwrap();
            assert_eq!(parsed.len(), 1);
            // FULL-struct equality, as rule.rs's own test does, with ONE
            // normalization: `line`. A learned rule carries `line: 0`
            // (documented on `propose_rule`), while `parse_rules` stamps the
            // 1-based line it read, which is 1 for the single rendered line.
            // Copying that stamped `line` into the proposed rule, every field
            // must round-trip exactly.
            let normalized = rule::Rule { line: parsed[0].line, kind: proposed.kind, pattern: proposed.pattern, words: proposed.words, category: proposed.category };
            assert_eq!(parsed[0], normalized);
            assert_eq!(parsed[0].line, 1);
        }
        Proposal::Refused(reason) => { assert!(false, "expected Accepted, got {:?}", reason); }
    }
}

#[test]
fn learned_rule_carries_requested_category_and_line_zero() {
    let rows = fixture_rows();
    let seed = row("39945b2fed", "REWE MARKT 12");
    match propose_rule(&seed, "food:supermarket", &rows) {
        Proposal::Accepted(proposed, _) => {
            assert_eq!(proposed.category, "food:supermarket".to_string());
            // line 0 means "not read from a rule file"; the core appends
            // learned rules to rules/90-learned.txt in a later item.
            assert_eq!(proposed.line, 0);
            assert!(proposed.words.len() == 0);
        }
        Proposal::Refused(reason) => { assert!(false, "expected Accepted, got {:?}", reason); }
    }
}

#[test]
fn proposing_twice_gives_equal_results() {
    let rows = fixture_rows();
    let seed = row("39945b2fed", "REWE MARKT 12");
    let first = propose_rule(&seed, "food:supermarket", &rows);
    let second = propose_rule(&seed, "food:supermarket", &rows);
    assert_eq!(first, second);
}

#[test]
fn words_and_regex_rules_render_back_to_the_parser() {
    let words_rule = rule::Rule { line: 1, kind: rule::PatternKind::Words, pattern: "REWE, MARKT".to_string(), words: ["REWE".to_string(), "MARKT".to_string()].to_vec(), category: "food:supermarket".to_string() };
    let regex_rule = rule::Rule { line: 1, kind: rule::PatternKind::Regex, pattern: "^PAYMENT TO".to_string(), words: Vec::<String>::new(), category: "finance:income".to_string() };
    assert_eq!(rule::parse_rules(&rule_to_text(&words_rule)).unwrap()[0], words_rule);
    assert_eq!(rule::parse_rules(&rule_to_text(&regex_rule)).unwrap()[0], regex_rule);
    // The Words round-trip is full-struct equality because `rule_to_text`
    // renders the canonical separator `, `: `parse_rules` splits on `,` and
    // trims, so `words:REWE, MARKT` reparses to the SAME `pattern` ("REWE,
    // MARKT") and the same word list, not a de-spaced "REWE,MARKT".
}

#[test]
fn candidate_that_is_not_a_substring_of_the_seed_is_refused_not_offered() {
    // 1.6 matches a Literal rule as a case-insensitive SUBSTRING of the
    // description. The seed `TAXI;AIRPORT REST` shares the token prefix
    // `TAXI`, `AIRPORT` with `TAXI;AIRPORT DAY`, but the space-joined
    // candidate `TAXI AIRPORT` is NOT a substring of either description, so
    // offering it would report an affected count that excludes the seed. The
    // only sound outcome is the typed refusal (shortening reaches the 2-token
    // floor and still fails).
    let mut rows = Vec::<transaction::Transaction>::new();
    rows.push(row("aaaaaaaaaa", "TAXI;AIRPORT REST"));
    rows.push(row("bbbbbbbbbb", "TAXI;AIRPORT DAY"));
    let seed = row("aaaaaaaaaa", "TAXI;AIRPORT REST");
    assert_eq!(propose_rule(&seed, "finance:expense:transport", &rows), Proposal::Refused(RefusalReason::PatternDoesNotMatchSeed));
}

#[test]
fn candidate_is_shortened_until_it_matches_the_seed_and_counts_it() {
    // `TAXI AIRPORT; NIGHT RUN` shares the 3-token prefix `TAXI`, `AIRPORT`,
    // `NIGHT` with `TAXI AIRPORT; NIGHT STOP`, but `TAXI AIRPORT NIGHT` is
    // not a substring of the seed (the `;` sits after `AIRPORT`). Shortening
    // one token gives `TAXI AIRPORT`, which DOES match the seed, so the
    // proposal is `Accepted` and its affected count includes the seed: both
    // rows contain `TAXI AIRPORT`, so 2.
    let mut rows = Vec::<transaction::Transaction>::new();
    rows.push(row("aaaaaaaaaa", "TAXI AIRPORT; NIGHT RUN"));
    rows.push(row("bbbbbbbbbb", "TAXI AIRPORT; NIGHT STOP"));
    let seed = row("aaaaaaaaaa", "TAXI AIRPORT; NIGHT RUN");
    match propose_rule(&seed, "finance:expense:transport", &rows) {
        Proposal::Accepted(proposed, affected) => {
            assert_eq!(proposed.pattern, "TAXI AIRPORT".to_string());
            assert_eq!(affected, 2);
            // The seed is counted, which is the invariant the refusal above
            // protects: no `Accepted` ever excludes the seed.
            let mut one_rule = Vec::<rule::Rule>::new();
            one_rule.push(proposed);
            assert_eq!(classify::classify(&seed, &one_rule, &Vec::<classify::Override>::new()), classify::Classification::Category("finance:expense:transport".to_string()));
        }
        Proposal::Refused(reason) => { assert!(false, "expected a shortened Accepted, got {:?}", reason); }
    }
}
