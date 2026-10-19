//! Stable [`Feature`] tags of the `filter.` and `predictor.` areas, which
//! this crate owns (ADR 0020).
//!
//! The image codec tags name the codecs of Table 6 that `pagura-image`
//! decodes. This crate reports them only when a chain asks it to decode one
//! itself, which happens when another filter follows the codec
//! ([`FilterChain::split_image`](crate::FilterChain::split_image) only takes
//! a trailing one). `pagura-image` reports [`JPX`] for JPEG 2000 modes it
//! does not implement.
use pagura_core::Feature;

/// Predictor dimensions exceed the stable API representation.
pub const PREDICTOR_DIMENSIONS: Feature = Feature::new("predictor.dimensions");
/// A `/Filter` name outside 7.4.1, Table 6.
pub const UNKNOWN: Feature = Feature::new("filter.unknown");
/// `DCTDecode` (JPEG), decoded by `pagura-image`.
pub const DCT: Feature = Feature::new("filter.DCTDecode");
/// `JPXDecode` (JPEG 2000), decoded by `pagura-image`.
pub const JPX: Feature = Feature::new("filter.JPXDecode");
/// `JBIG2Decode`, decoded by `pagura-image`.
pub const JBIG2: Feature = Feature::new("filter.JBIG2Decode");
/// `CCITTFaxDecode`, decoded by `pagura-image`.
pub const CCITT_FAX: Feature = Feature::new("filter.CCITTFaxDecode");

/// Every feature tag of this crate.
pub const ALL: &[Feature] = &[PREDICTOR_DIMENSIONS, UNKNOWN, DCT, JPX, JBIG2, CCITT_FAX];

#[cfg(test)]
mod tests {
    use super::ALL;

    #[test]
    fn tags_are_unique_and_in_this_crates_areas() {
        for (index, tag) in ALL.iter().enumerate() {
            let (area, item) = tag.as_str().split_once('.').unwrap();
            assert!(["filter", "predictor"].contains(&area), "{tag}");
            assert!(!item.is_empty(), "{tag}");
            assert!(
                item.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-'),
                "{tag}"
            );
            assert!(!ALL[..index].contains(tag), "{tag}");
        }
    }
}
