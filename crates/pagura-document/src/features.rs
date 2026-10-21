//! Stable tags of the unsupported features this crate reports (ADR 0003,
//! ADR 0017). A tag never changes once published.

use pagura_core::Feature;

/// Stream data in external files (`/F`, clause 7.3.8.2).
pub const EXTERNAL_STREAM: Feature = Feature::new("stream.external");
/// Unknown non-Standard security handler; the filter name is not a static tag.
pub const UNKNOWN_SECURITY: Feature = Feature::new("security.unknown");
/// Adobe public-key security handler.
pub const PUBSEC: Feature = Feature::new("security.Adobe-PubSec");
/// An encryption algorithm version `V` other than 1, 2, 4 or 5 (Table 20).
pub const SECURITY_VERSION: Feature = Feature::new("security.version");
/// A crypt filter method the Standard handler cannot run: `None` (delegated)
/// or a method that does not match the algorithm version (Table 25).
pub const CRYPT_METHOD: Feature = Feature::new("security.CFM");
/// A crypt filter key length that differs from the file key (Table 25).
pub const CRYPT_KEY_LENGTH: Feature = Feature::new("security.crypt-length");
/// XFA interpretation and filling (12.7.8).
pub const XFA: Feature = Feature::new("form.xfa");
/// Rich text generation and filling (12.7.4.3).
pub const RICH_TEXT: Feature = Feature::new("form.rich-text");
/// Filling unsupported or nonterminal field types.
pub const FIELD_TYPE: Feature = Feature::new("form.field-type");
/// Reading an earlier revision of a document repaired at opening (7.5.6).
pub const HISTORICAL_SOURCE: Feature = Feature::new("document.historical-source");
/// Reading an earlier revision of an encrypted document (7.5.6, 7.6).
pub const HISTORICAL_ENCRYPTION: Feature = Feature::new("document.historical-encryption");

/// Every feature tag above.
pub const ALL: &[Feature] = &[
    EXTERNAL_STREAM,
    UNKNOWN_SECURITY,
    PUBSEC,
    SECURITY_VERSION,
    CRYPT_METHOD,
    CRYPT_KEY_LENGTH,
    XFA,
    RICH_TEXT,
    FIELD_TYPE,
    HISTORICAL_SOURCE,
    HISTORICAL_ENCRYPTION,
];

#[cfg(test)]
mod tests {
    use super::ALL;

    #[test]
    fn tags_are_unique_and_well_formed() {
        for (index, tag) in ALL.iter().enumerate() {
            let (area, item) = tag.as_str().split_once('.').unwrap();
            assert!(
                ["stream", "security", "form", "document"].contains(&area),
                "{tag}"
            );
            assert!(
                !item.is_empty() && item.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
            );
            assert!(!ALL[..index].contains(tag), "{tag}");
        }
    }
}
