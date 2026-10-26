//! Fixture access and oracle parsing shared by the font tests.
use pagura_core::{
    Dictionary, Error, Limits, ObjectId, Recovery, Result, Value,
    syntax::{Origin, Parser},
};
use pagura_document::{Document, OpenOptions};
use pagura_testkit::fixture;
use pagura_text::{FontUnicode, GlyphShape, PdfFont, Provider};

/// Opens `text/<name>.pdf` under `recovery`.
pub(crate) fn open(name: &str, recovery: Recovery) -> Result<Document> {
    Document::open(
        fixture("text", &format!("{name}.pdf")),
        &OpenOptions {
            recovery,
            ..OpenOptions::default()
        },
    )
}

/// The font dictionary every vector stores as object 4.
pub(crate) fn dictionary(document: &Document) -> pagura_document::source::ReadResult<&Dictionary> {
    document
        .object(ObjectId::new(4, 0))?
        .dictionary()
        .ok_or_else(|| Error::syntax("missing fixture font").into())
}

/// Loads `font` with `provider`, and its Unicode, as text extraction does.
pub(crate) fn load_text(
    document: &Document,
    font: &Dictionary,
    provider: Option<&dyn Provider>,
) -> pagura_document::source::ReadResult<(PdfFont, FontUnicode)> {
    let loaded = PdfFont::load_with_provider(document, font, provider)?;
    let unicode = loaded.unicode(document, font, provider)?;
    Ok((loaded, unicode))
}

/// The array oracle `text/<name>.<suffix>`, in PDF syntax.
pub(crate) fn values(name: &str, suffix: &str) -> Result<Vec<Value>> {
    let bytes = fixture("text", &format!("{name}.{suffix}"));
    match Parser::new(&bytes, Origin::Buffer, Limits::default()).parse()? {
        Value::Array(values) => Ok(values),
        _ => Err(Error::syntax("expected oracle array")),
    }
}

pub(crate) fn row(value: &Value) -> Result<&[Value]> {
    value
        .as_array()
        .ok_or_else(|| Error::syntax("expected oracle row"))
}

pub(crate) fn number(value: &Value) -> Result<f64> {
    value
        .as_number()
        .ok_or_else(|| Error::syntax("expected oracle number"))
}

pub(crate) fn uint(value: &Value) -> Result<u32> {
    value
        .as_integer()
        .and_then(|n| u32::try_from(n).ok())
        .ok_or_else(|| Error::syntax("expected oracle integer"))
}

pub(crate) fn text(value: &Value) -> Option<&str> {
    value.as_string().and_then(|v| std::str::from_utf8(v).ok())
}

pub(crate) fn near(actual: f64, expected: f64) {
    assert!((actual - expected).abs() < 1e-12, "{actual} != {expected}");
}

/// The lines of the text oracle `text/<name>.<suffix>`.
pub(crate) fn lines(name: &str, suffix: &str) -> Result<Vec<String>> {
    let bytes = fixture("text", &format!("{name}.{suffix}"));
    Ok(std::str::from_utf8(&bytes)
        .map_err(|_| Error::syntax("invalid text oracle"))?
        .lines()
        .map(str::to_owned)
        .collect())
}

/// The error of a call that must fail.
pub(crate) fn rejected<T>(
    result: pagura_document::source::ReadResult<T>,
) -> pagura_document::source::ReadResult<Error> {
    match result {
        Err(pagura_document::source::ReadFault::Invalid(error)) => Ok(error),
        Err(fault) => Err(fault),
        Ok(_) => Err(Error::syntax("expected a rejection").into()),
    }
}

/// Decodes `text/<name>.input` and checks each glyph against the oracle
/// rows `[code length advance word-space unicode glyph]`, with outline
/// glyphs drawn through the matrix of `text/<name>.matrix`.
pub(crate) fn check_glyphs(
    (font, unicode): &(PdfFont, FontUnicode),
    name: &str,
    suffix: &str,
) -> Result<()> {
    let glyphs = font.decode(&fixture("text", &format!("{name}.input")), Some(unicode))?;
    let expected = values(name, suffix)?;
    assert_eq!(glyphs.len(), expected.len(), "{name}");
    for (glyph, expected) in glyphs.iter().zip(&expected) {
        let expected = row(expected)?;
        assert_eq!(glyph.code, uint(&expected[0])?, "{name}");
        assert_eq!(u32::from(glyph.byte_len), uint(&expected[1])?, "{name}");
        near(glyph.advance, number(&expected[2])?);
        assert_eq!(Some(glyph.word_space), expected[3].as_bool());
        assert_eq!(
            glyph.unicode.as_deref().map(str::as_bytes),
            expected[4].as_string(),
            "{name}: {}",
            glyph.code
        );
        assert!(glyph.vertical.is_none());
        if expected[5].is_null() {
            assert!(matches!(glyph.shape, GlyphShape::Blank));
            continue;
        }
        let GlyphShape::Outline {
            program,
            glyph: id,
            matrix,
        } = &glyph.shape
        else {
            return Err(Error::syntax("expected drawable outline"));
        };
        assert_eq!(id.0, uint(&expected[5])?, "{name}");
        let matrices = values(name, "matrix")?;
        for (actual, expected) in [matrix.a, matrix.b, matrix.c, matrix.d, matrix.e, matrix.f]
            .into_iter()
            .zip(row(&matrices[0])?)
        {
            near(actual, number(expected)?);
        }
        program.outline(*id)?;
    }
    Ok(())
}
