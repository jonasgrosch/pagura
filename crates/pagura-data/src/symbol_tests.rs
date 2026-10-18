//! Full AFM code/name coverage against independently parsed CFF charsets.
use super::tests::{dictionary, expected, integer, near, number, open, row, text};
use super::*;
use pagura_text::{GlyphShape, PdfFont};

#[test]
fn foxit_charsets_and_pdf_width_precedence() -> Result<()> {
    let provider = Bundled::default();
    for name in [
        "substitute-Symbol-charset",
        "substitute-ZapfDingbats-charset",
        "substitute-Symbol-differences",
        "substitute-Symbol-differences-widths",
        "substitute-ZapfDingbats-differences",
        "substitute-ZapfDingbats-differences-widths",
        "substitute-Symbol-truetype-symbolic",
        "substitute-Symbol-truetype-nonsymbolic",
    ] {
        let document = open(name)?;
        let font = PdfFont::load_with(&document, dictionary(&document)?, &provider)?;
        let input = pagura_testkit::fixture("text", &format!("{name}.input"));
        let glyphs = font.decode(&input)?;
        let rows = expected(name, "out")?;
        assert_eq!(glyphs.len(), rows.len());
        for (glyph, value) in glyphs.iter().zip(&rows) {
            let values = row(value)?;
            assert_eq!(glyph.code, integer(&values[0])?);
            near(glyph.advance, number(&values[2])? / 1000.0);
            let GlyphShape::Outline {
                program,
                glyph,
                matrix,
            } = &glyph.shape
            else {
                return Err(pagura_core::Error::syntax("missing symbol outline"));
            };
            assert_eq!(glyph.0, integer(&values[3])?, "{name}");
            assert_eq!(
                program.glyph_for_name(
                    text(&values[1])
                        .ok_or_else(|| pagura_core::Error::syntax("missing glyph name"))?
                        .as_bytes()
                ),
                Some(*glyph)
            );
            assert_eq!(*matrix, program.font_matrix());
            program.outline(*glyph)?;
        }
        assert_eq!(document.diagnostics().len(), 1);
        assert_eq!(
            document.diagnostics()[0].kind,
            pagura_core::DiagnosticKind::Information(pagura_text::SUBSTITUTED)
        );
        assert_eq!(
            document.diagnostics()[0].message,
            if name.contains("ZapfDingbats") {
                "substitute outlines for ZapfDingbats"
            } else {
                "substitute outlines for Symbol"
            }
        );
    }
    Ok(())
}
