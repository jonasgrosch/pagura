//! The rendering tags the facade re-exports are unique, well formed and in
//! the areas their crates own (ADR 0017).

use pagura::{JBIG2_EOF_PAGE_ASSOCIATION, JBIG2_FILE_WRAPPER, form_appearance, tags};

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
fn packaging_tags_have_unique_owned_public_identities() {
    assert_eq!(JBIG2_FILE_WRAPPER.as_str(), "jbig2.file-wrapper");
    assert_eq!(
        JBIG2_EOF_PAGE_ASSOCIATION.as_str(),
        "jbig2.eof-page-association"
    );
    let tags = [JBIG2_FILE_WRAPPER, JBIG2_EOF_PAGE_ASSOCIATION];
    for (index, tag) in tags.iter().enumerate() {
        let (area, item) = tag.as_str().split_once('.').unwrap();
        assert_eq!(area, "jbig2");
        assert!(
            !item.is_empty()
                && item
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte == b'-')
        );
        assert!(!tags[..index].contains(tag));
    }
}
