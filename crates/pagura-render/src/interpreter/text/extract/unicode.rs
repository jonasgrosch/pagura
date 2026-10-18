//! Extraction-only fallback after ToUnicode, encoding names and collections.
//! ISO 32000-2 9.10.2 permits reader-selected text when those methods fail.
use pagura_core::{Error, Limits, Result};
use pagura_text::{Glyph, GlyphShape};
use std::borrow::Cow;

pub(super) fn recover<'a>(
    glyph: &'a Glyph,
    base_font: &[u8],
    work: &mut usize,
    limits: Limits,
) -> Result<Option<Cow<'a, str>>> {
    if let Some(text) = &glyph.unicode {
        return Ok(Some(Cow::Borrowed(text)));
    }
    let GlyphShape::Outline { program, glyph, .. } = &glyph.shape else {
        return Ok(None);
    };
    let Some(name) = program.glyph_name(*glyph) else {
        return Ok(None);
    };
    *work = work
        .checked_add(name.len())
        .filter(|n| *n <= limits.operations)
        .ok_or_else(|| Error::limit("glyph-name Unicode work budget exceeded"))?;
    Ok(pagura_text::glyph_name_unicode_for_font(name.as_bytes(), base_font).map(Cow::Owned))
}

#[cfg(test)]
mod tests {
    use super::*;
    use pagura_font::{FontFormat, FontProgram, GlyphId};
    use std::sync::Arc;

    fn raw_glyph() -> Glyph {
        let program = Arc::new(
            FontProgram::parse(
                pagura_testkit::fixture("font", "truetype-cmap4.font"),
                FontFormat::TrueType,
                &Limits::default(),
            )
            .unwrap(),
        );
        Glyph::new(
            65,
            1,
            0.6,
            GlyphShape::Outline {
                matrix: program.font_matrix(),
                program,
                glyph: GlyphId(1),
            },
        )
    }

    #[test]
    fn name_fallback_preserves_authoritative_text_and_mapping_order() {
        let mut glyph = raw_glyph();
        let limits = Limits::default();
        assert_eq!(
            recover(&glyph, b"", &mut 0, limits).unwrap().as_deref(),
            Some("A")
        );
        let no_work = Limits {
            operations: 0,
            ..limits
        };
        assert!(recover(&glyph, b"", &mut 0, no_work).is_err());
        for known in ["", "Z", "AB😀"] {
            glyph.unicode = Some(known.into());
            assert_eq!(
                recover(&glyph, b"", &mut 0, no_work).unwrap().as_deref(),
                Some(known)
            );
        }
    }

    #[test]
    fn aggregate_name_work_is_bounded_without_mutating_the_glyph() {
        let glyph = raw_glyph();
        let limits = Limits {
            operations: 2,
            ..Limits::default()
        };
        let mut work = 0;
        for _ in 0..2 {
            assert!(recover(&glyph, b"", &mut work, limits).unwrap().is_some());
        }
        assert!(recover(&glyph, b"", &mut work, limits).is_err());
        let mut exhausted = usize::MAX;
        assert!(recover(&glyph, b"", &mut exhausted, limits).is_err());
        assert_eq!(glyph.unicode, None);
        assert_eq!(glyph.advance, 0.6);
    }
}
