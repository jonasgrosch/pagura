//! Stable repair tags of this crate (ADR 0003): every [`Repair`] that
//! `pagura-core` reports, in one place. The `syntax` area covers object
//! syntax (clauses 7.2–7.3) and the `string` area covers text strings
//! (clause 7.9.2.2).

use crate::Repair;

/// Repair applied when a dictionary repeats a key (clause 7.3.7 forbids it):
/// the last value wins. Readers disagree here, so the repair is always reported.
pub const DUPLICATE_KEY: Repair = Repair::new("syntax.duplicate-key");
/// A dictionary key with a missing solidus (7.3.7), accepted only when the
/// entire regular token begins with an ASCII letter. Reserved values are excluded.
pub const KEY_SOLIDUS: Repair = Repair::new("syntax.key-solidus");
/// A hash not followed by two hex digits is retained literally (7.3.5).
pub const NAME_ESCAPE: Repair = Repair::new("syntax.name-escape");
/// An undefined object-zero reference is read as null (7.3.10).
pub const ZERO_REFERENCE: Repair = Repair::new("syntax.reference-zero");
/// A lone closing parenthesis immediately before a dictionary closer (7.3.7).
pub const DICTIONARY_CLOSER: Repair = Repair::new("syntax.dictionary-closer");
/// A duplicated opening dictionary delimiter before its first key (7.3.7).
pub const DICTIONARY_OPENER: Repair = Repair::new("syntax.dictionary-opener");
/// A stream illegally used as a direct dictionary/array value (7.3.8.1).
pub const DIRECT_STREAM: Repair = Repair::new("syntax.direct-stream");
/// Leading zeros in reference identifiers (7.3.10, EC3 issue 379).
pub const LEADING_ZERO_REFERENCE: Repair = Repair::new("syntax.reference-leading-zero");
/// An invalid UTF-16BE or UTF-8 sequence, or a byte PDFDocEncoding does not
/// define, in a text string, replaced by U+FFFD (7.9.2.2, Annex D.3).
pub const INVALID_TEXT: Repair = Repair::new("string.invalid-text");
/// An escape character in a Unicode text string that starts no valid
/// language escape sequence, removed (7.9.2.2.2).
pub const LANGUAGE_ESCAPE: Repair = Repair::new("string.language-escape");

/// Every repair tag defined by this crate.
pub const ALL: &[Repair] = &[
    DUPLICATE_KEY,
    KEY_SOLIDUS,
    NAME_ESCAPE,
    ZERO_REFERENCE,
    DICTIONARY_CLOSER,
    DICTIONARY_OPENER,
    DIRECT_STREAM,
    LEADING_ZERO_REFERENCE,
    INVALID_TEXT,
    LANGUAGE_ESCAPE,
];

#[cfg(test)]
mod tests {
    use super::ALL;

    #[test]
    fn tags_are_unique_and_well_formed() {
        for (index, tag) in ALL.iter().enumerate() {
            let (area, item) = tag.as_str().split_once('.').unwrap();
            assert!(["syntax", "string"].contains(&area), "{tag}");
            assert!(!item.is_empty() && item.bytes().all(|b| b.is_ascii_lowercase() || b == b'-'));
            assert!(!ALL[..index].contains(tag), "{tag}");
        }
    }
}
