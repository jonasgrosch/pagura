//! The rendering tags the facade re-exports are unique, well formed and in
//! the areas their crates own (ADR 0017).

use pagura::{form_appearance, repairs, tags};

#[test]
fn rendering_tags_are_unique_and_well_formed() {
    let features = [tags::appearance::ALL, tags::color::ALL, tags::content::ALL];
    let mut seen: Vec<&str> = form_appearance::information::ALL.to_vec();
    for tag in features.iter().flat_map(|all| all.iter()) {
        let tag = tag.as_str();
        let (area, item) = tag.split_once('.').unwrap_or_default();
        assert!(
            !area.is_empty()
                && area.bytes().all(|b| b.is_ascii_lowercase())
                && !item.is_empty()
                && item.bytes().all(|b| b.is_ascii_graphic()),
            "malformed tag {tag}"
        );
        assert!(!seen.contains(&tag), "duplicate tag {tag}");
        seen.push(tag);
    }
    assert!(
        tags::content::ALL
            .iter()
            .any(|tag| tag.as_str() == "shading.transform")
    );
    assert!(
        tags::color::ALL
            .iter()
            .any(|tag| tag.as_str() == "function.calculator")
    );
    assert_eq!(tags::image::JPEG_PROCESS.as_str(), "jpeg.process");
}

#[test]
fn inline_repairs_are_public_and_owned_by_the_inline_area() {
    let mut seen = Vec::new();
    for tag in repairs::inline::ALL {
        let tag = tag.as_str();
        assert_eq!(tag.split_once('.').map(|(area, _)| area), Some("inline"));
        assert!(tag.bytes().all(|byte| byte.is_ascii_graphic()));
        assert!(!seen.contains(&tag));
        seen.push(tag);
    }
    assert!(repairs::inline::ALL.contains(&repairs::inline::EI_SEPARATOR));
}

#[test]
fn image_repairs_are_exposed_through_the_facade() {
    let raw = pagura::repairs::image::JPX_CODESTREAM;
    assert_eq!(raw.as_str(), "jpx.raw-codestream");
    assert!(pagura::repairs::image::ALL.contains(&raw));
    let repair = pagura::repairs::image::JPX_PALETTE_PADDING;
    assert_eq!(repair.as_str(), "jpx.palette-padding");
    assert!(pagura::repairs::image::ALL.contains(&repair));
    let mut seen = Vec::new();
    for repair in pagura::repairs::image::ALL {
        assert!(!seen.contains(repair), "duplicate repair {repair:?}");
        seen.push(*repair);
    }
}
