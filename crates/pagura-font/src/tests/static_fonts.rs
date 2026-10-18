//! Explicit static procedures and unhinted blue-zone oracles.
use super::*;

#[test]
fn static_type1_aliases_are_not_postscript_execution() {
    for name in [
        "symbols",
        "canonical",
        "custom",
        "clear",
        "short",
        "comments",
    ] {
        vector(&format!("type1-static-{name}"), FontFormat::Type1);
    }
    for name in ["reader", "put", "nested", "custom"] {
        vector(&format!("type1-computed-{name}"), FontFormat::Type1);
    }
}

#[test]
fn blue_zones_can_have_valid_pairs_in_nonglobal_order() {
    vector("cff-blue-zone-order", FontFormat::Cff);
}

#[test]
fn retained_memory_vectors_keep_their_independent_outlines() {
    for (name, format) in [
        ("truetype", FontFormat::TrueType),
        ("cff", FontFormat::Cff),
        ("cff-cid", FontFormat::Cff),
        ("cff2", FontFormat::OpenType),
        ("type1-small", FontFormat::Type1),
        ("type1-large", FontFormat::Type1),
    ] {
        vector(&format!("memory-{name}"), format);
    }
}
