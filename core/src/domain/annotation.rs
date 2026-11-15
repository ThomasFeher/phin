//! The annotation record: a note attached to a transaction, with provenance
//! and a confidence value.
//!
//! An annotation is what the mail side knows about a payment before the link
//! to a transaction is decided: the extracted text, the purchased items, the
//! mbox it came from, and how sure the automatic link is. Mail data never
//! creates a transaction (rule R002); it only ever produces annotations that
//! point at one.
//!
//! The CSV rendering (annotations.csv columns `id;ref;text;items;source;confidence`)
//! keeps `items` as one `|`-separated string, while the domain — and the JSON
//! plugin protocol — keep it as a list. [`domain::annotation::from_parts`] reads
//! the CSV form, [`domain::annotation::items_to_text`] writes it back.
//!
//! Confidence is stored as a plain `f64`. That is deliberate: confidence is a
//! bounded heuristic measure between 0 and 1, not money, so it never takes part
//! in an exact sum and never needs the `i64` minor-unit representation that
//! [`domain::money`] uses. The plugin protocol already carries it as a JSON
//! decimal. A fixed-point integer representation was the alternative and was
//! not chosen; [`domain::annotation::clamp_confidence`] keeps the value inside
//! `0…1` on construction, so out-of-range input cannot leak into the model.

/// An annotation record: its own id, the transaction it refers to (possibly
/// none), the extracted text, the purchased items, its provenance and its
/// confidence.
#[derive(Debug)]
pub struct Annotation {
    /// The annotation's own content hash (hex, optionally `#N`). Computed by a
    /// later plan item; here it is an opaque string.
    pub id: String,
    /// The transaction id this annotation refers to. EMPTY means the annotation
    /// is orphan/pending and is not linked. A NON-empty ref may equally name a
    /// transaction id that does not exist: the domain has no transaction table
    /// to check against, so both orphan forms are representable and linking is
    /// decided elsewhere.
    pub r#ref: String,
    /// The annotation text as extracted from the mail.
    pub text: String,
    /// Purchased items, one per entry; may be empty. The CSV form joins them
    /// with `|`.
    pub items: Vec::<String>,
    /// Provenance: the mbox file and folder the annotation came from.
    pub source: String,
    /// Link confidence, clamped to `0…1`. Low confidence means the link stays
    /// pending for the user to confirm or reject.
    pub confidence: f64,
}

/// Clamp a confidence value into `0…1`, total and never panicking.
///
/// `NaN` maps to `0.0` (a NaN confidence carries no information, so it is the
/// least confident link); anything below `0.0` maps to `0.0`, anything above
/// `1.0` maps to `1.0`, and every other value is returned unchanged.
pub fn clamp_confidence(value: f64) -> f64 {
    if value != value {
        return 0.0;
    }
    if value < 0.0 {
        return 0.0;
    }
    if value > 1.0 {
        return 1.0;
    }
    return value;
}

/// Build an annotation from the annotations.csv column values.
///
/// `items_joined` is the `|`-separated CSV text: an empty string yields an
/// EMPTY item list (not one empty item), a trailing `|` yields no phantom item,
/// and whitespace around each item is trimmed. `confidence` is passed through
/// [`domain::annotation::clamp_confidence`].
pub fn from_parts(id: String, ref_id: String, text: String, items_joined: String, source: String, confidence: f64) -> Annotation {
    let mut items = Vec::<String>::new();
    if items_joined.len() != 0 {
        for item in items_joined.split("|") {
            let item = item.trim();
            if item.len() != 0 { items.push(item.to_string()); }
        }
    }
    return Annotation {
        id: id,
        r#ref: ref_id,
        text: text,
        items: items,
        source: source,
        confidence: clamp_confidence(confidence),
    };
}

/// Render an item list as the `|`-separated CSV text.
///
/// An empty list renders as an empty string. For the lists this codec produces,
/// [`domain::annotation::from_parts`] splitting the result returns the original
/// list.
pub fn items_to_text(items: Vec::<String>) -> String {
    let mut text = "".to_string();
    for item in items.iter() {
        if text.len() != 0 { text += "|"; }
        text += item;
    }
    return text;
}

#[cfg(test)]
fn two_items() -> Vec::<String> {
    let mut items = Vec::<String>::new();
    items.push("1x book".to_string());
    items.push("1x ebook".to_string());
    return items;
}

#[cfg(test)]
fn two_trimmed_items() -> Vec::<String> {
    let mut items = Vec::<String>::new();
    items.push("a".to_string());
    items.push("b".to_string());
    return items;
}

#[test]
fn confidence_is_clamped_into_range() {
    assert_eq!(clamp_confidence(-0.5), 0.0);
    assert_eq!(clamp_confidence(1.5), 1.0);
    assert_eq!(clamp_confidence(2.0), 1.0);
    assert_eq!(clamp_confidence(0.0 / 0.0), 0.0);
    assert_eq!(clamp_confidence(-(0.0 / 0.0)), 0.0);
    assert_eq!(clamp_confidence(0.0), 0.0);
    assert_eq!(clamp_confidence(1.0), 1.0);
    assert_eq!(clamp_confidence(0.9), 0.9);
}

#[test]
fn confidence_clamping_never_panics() {
    let nasty = [0.0 / 0.0, -(0.0 / 0.0), 0.0, -0.0, 1.0, -1.0, 1.0e300, -1.0e300];
    for value in nasty.iter() {
        let clamped = clamp_confidence(*value);
        assert!(clamped >= 0.0);
        assert!(clamped <= 1.0);
    }
}

#[test]
fn annotation_referring_to_unknown_transaction_is_representable() {
    let annotation = from_parts("abc123".to_string(), "deadbeef0000".to_string(), "PayPal payment to Amazon Services".to_string(), "1x book|1x ebook".to_string(), "mail_paypal.mbox".to_string(), 0.9);
    assert_eq!(annotation.r#ref, "deadbeef0000".to_string());
    assert_eq!(annotation.items, two_items());
    assert_eq!(annotation.confidence, 0.9);
}

#[test]
fn annotation_with_empty_ref_is_representable() {
    let annotation = from_parts("abc123".to_string(), "".to_string(), "PayPal payment to Amazon Services".to_string(), "1x book|1x ebook".to_string(), "mail_paypal.mbox".to_string(), 0.9);
    assert_eq!(annotation.r#ref, "".to_string());
}

#[test]
fn empty_items_text_yields_empty_item_list() {
    let annotation = from_parts("abc123".to_string(), "deadbeef0000".to_string(), "text".to_string(), "".to_string(), "mail_paypal.mbox".to_string(), 0.5);
    assert_eq!(annotation.items, Vec::<String>::new());
}

#[test]
fn items_split_on_pipe_and_trimmed() {
    let annotation = from_parts("abc123".to_string(), "deadbeef0000".to_string(), "text".to_string(), "1x book|1x ebook".to_string(), "mail_paypal.mbox".to_string(), 0.5);
    assert_eq!(annotation.items, two_items());
    let trailing = from_parts("abc123".to_string(), "".to_string(), "text".to_string(), "a|".to_string(), "mail_paypal.mbox".to_string(), 0.5);
    assert_eq!(trailing.items.len(), 1);
    assert_eq!(trailing.items[0], "a".to_string());
    let padded = from_parts("abc123".to_string(), "".to_string(), "text".to_string(), " a | b ".to_string(), "mail_paypal.mbox".to_string(), 0.5);
    assert_eq!(padded.items, two_trimmed_items());
}

#[test]
fn items_round_trip_through_text() {
    let text = items_to_text(two_items());
    assert_eq!(text, "1x book|1x ebook".to_string());
    let annotation = from_parts("abc123".to_string(), "deadbeef0000".to_string(), "t".to_string(), text, "mail_paypal.mbox".to_string(), 0.5);
    assert_eq!(annotation.items, two_items());
    assert_eq!(items_to_text(Vec::<String>::new()), "".to_string());
    let empty = from_parts("abc123".to_string(), "".to_string(), "t".to_string(), items_to_text(Vec::<String>::new()), "mail_paypal.mbox".to_string(), 0.5);
    assert_eq!(empty.items, Vec::<String>::new());
}

#[test]
fn fixture_annotation_from_plugin_protocol_survives_construction() {
    let annotation = from_parts("abc123".to_string(), "deadbeef0000".to_string(), "PayPal payment to Amazon Services".to_string(), "1x book|1x ebook".to_string(), "mail_paypal.mbox".to_string(), 0.9);
    assert_eq!(annotation.text, "PayPal payment to Amazon Services".to_string());
    assert_eq!(annotation.r#ref, "deadbeef0000".to_string());
    assert_eq!(annotation.source, "mail_paypal.mbox".to_string());
    assert_eq!(annotation.items, two_items());
    assert_eq!(annotation.confidence, 0.9);
}

#[test]
fn out_of_range_confidence_clamps_on_construction() {
    let low = from_parts("abc123".to_string(), "".to_string(), "t".to_string(), "".to_string(), "mail_paypal.mbox".to_string(), -0.5);
    assert_eq!(low.confidence, 0.0);
    let high = from_parts("abc123".to_string(), "".to_string(), "t".to_string(), "".to_string(), "mail_paypal.mbox".to_string(), 1.5);
    assert_eq!(high.confidence, 1.0);
    let nan = from_parts("abc123".to_string(), "".to_string(), "t".to_string(), "".to_string(), "mail_paypal.mbox".to_string(), 0.0 / 0.0);
    assert_eq!(nan.confidence, 0.0);
}
