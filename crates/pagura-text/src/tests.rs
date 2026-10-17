//! Independently generated vectors (tools/fixturegen/text.py).
use super::*;
use pagura_core::{
    ErrorKind, Value,
    syntax::{Origin, Parser},
};
use pagura_document::OpenOptions;
use pagura_testkit::fixture;

fn document(name: &str) -> Result<Document> {
    Document::open(
        fixture("text", &format!("{name}.pdf")),
        &OpenOptions::default(),
    )
}

fn expected(name: &str) -> Result<Vec<Value>> {
    let data = fixture("text", &format!("{name}.out"));
    match Parser::new(&data, Origin::Buffer, Limits::default()).parse()? {
        Value::Array(values) => Ok(values),
        _ => Err(Error::syntax("expected array")),
    }
}

fn dictionary(document: &Document) -> Result<&Dictionary> {
    document
        .object(ObjectId::new(4, 0))?
        .dictionary()
        .ok_or_else(|| Error::syntax("expected dictionary"))
}

fn rows(value: &Value) -> Result<&[Value]> {
    value
        .as_array()
        .ok_or_else(|| Error::syntax("expected row"))
}
fn float(value: &Value) -> Result<f64> {
    value
        .as_number()
        .ok_or_else(|| Error::syntax("expected number"))
}
fn integer(value: &Value) -> Result<u32> {
    value
        .as_integer()
        .and_then(|v| u32::try_from(v).ok())
        .ok_or_else(|| Error::syntax("expected integer"))
}
fn text(value: &Value) -> Option<&str> {
    value.as_string().and_then(|v| std::str::from_utf8(v).ok())
}
fn near(actual: f64, expected: f64) {
    assert!((actual - expected).abs() < 1e-12, "{actual} != {expected}");
}

#[test]
fn type3_decoding_vectors() -> Result<()> {
    for name in [
        "type3-differences",
        "type3-undefined",
        "type3-matrix",
        "encoding-winansi",
        "encoding-macroman",
        "encoding-standard",
        "encoding-macexpert",
        "encoding-symbol",
        "encoding-dingbats",
        "encoding-pdfdoc",
        "tounicode-precedence",
        "tounicode-empty",
    ] {
        let document = document(name)?;
        let font = PdfFont::load(&document, dictionary(&document)?)?;
        assert_eq!(font.kind(), FontKind::Type3);
        assert_eq!(font.writing_mode(), WritingMode::Horizontal);
        assert!(
            font.type3_resources()
                .is_some_and(|d| d.contains_key(b"ProcSet"))
        );
        let actual = font.decode(&fixture("text", &format!("{name}.input")))?;
        let expected = expected(name)?;
        assert_eq!(actual.len(), expected.len(), "{name}");
        for (glyph, expected) in actual.iter().zip(&expected) {
            let row = rows(expected)?;
            assert_eq!(glyph.code, integer(&row[0])?, "{name}");
            assert_eq!(u32::from(glyph.byte_len), integer(&row[1])?);
            near(glyph.advance, float(&row[2])?);
            assert_eq!(Some(glyph.word_space), row[3].as_bool());
            assert_eq!(
                glyph.unicode.as_deref(),
                text(&row[4]),
                "{name}: {}",
                glyph.code
            );
            assert_eq!(glyph.vertical, None);
            if row[5].is_null() {
                assert!(matches!(glyph.shape, GlyphShape::Blank));
                continue;
            }
            let GlyphShape::Type3 { procedure, matrix } = glyph.shape else {
                return Err(Error::syntax("expected Type3 shape"));
            };
            assert_eq!(procedure.number, integer(&row[5])?);
            if name == "type3-matrix" {
                assert_eq!(
                    matrix,
                    Transform::new(0.002, 0.003, -0.004, 0.005, 2.0, 3.0)
                );
            }
        }
        assert!(font.decode(&[])?.is_empty());
    }
    Ok(())
}

#[test]
fn cmap_vectors() -> Result<()> {
    for name in [
        "cmap-mixed",
        "cmap-bytewise",
        "cmap-usecmap",
        "cmap-parent-stream",
        "cmap-indirect-name",
        "unicode-parent",
    ] {
        let document = document(name)?;
        let map = cmap::load(
            &document,
            &Value::Reference(ObjectId::new(4, 0)),
            name == "unicode-parent",
        )?;
        let input = fixture("text", &format!("{name}.input"));
        let mut offset = 0;
        let mut work = 0;
        for value in expected(name)? {
            let row = rows(&value)?;
            let (code, valid) = map.next(&input[offset..], &mut work, document.limits())?;
            assert_eq!(code.value, integer(&row[0])?, "{name}");
            assert_eq!(u32::from(code.len), integer(&row[1])?, "{name}");
            let cid = if valid {
                map.cid(code, &mut work, document.limits())?
            } else {
                None
            };
            assert_eq!(
                cid,
                if row[2].is_null() {
                    None
                } else {
                    Some(integer(&row[2])?)
                },
                "{name}"
            );
            assert_eq!(
                map.notdef(code, &mut work, document.limits())?,
                integer(&row[3])?
            );
            assert_eq!(map.text(code), text(&row[4]));
            offset += usize::from(code.len);
        }
        assert_eq!(offset, input.len());
        assert_eq!(
            map.mode,
            if matches!(name, "cmap-mixed" | "cmap-usecmap" | "cmap-indirect-name") {
                WritingMode::Vertical
            } else {
                WritingMode::Horizontal
            }
        );
    }
    Ok(())
}

#[test]
fn cid_metric_vectors() -> Result<()> {
    for name in ["cid-metrics", "cid-default-metrics"] {
        let document = document(name)?;
        let metrics = metrics::Metrics::composite(&document, dictionary(&document)?)?;
        for value in expected(name)? {
            let row = rows(&value)?;
            let code = integer(&row[0])?;
            near(metrics.width(code), float(&row[1])?);
            let vertical = metrics.vertical(code);
            near(vertical.advance, float(&row[2])?);
            near(vertical.origin.0, float(&row[3])?);
            near(vertical.origin.1, float(&row[4])?);
        }
    }
    Ok(())
}

#[test]
fn font_error_vectors() -> Result<()> {
    for name in [
        "nonembedded-type1",
        "nonembedded-mmtype1",
        "nonembedded-truetype",
        "predefined-cmap",
        "differences-overflow",
        "width-count",
        "unicode-surrogate",
        "unicode-cycle",
        "unicode-range-overflow",
        "unicode-odd-destination",
        "unicode-two-byte",
        "unicode-prefix-overlap",
        "unicode-range-array-length",
        "unicode-block-count",
        "unicode-usecmap-mismatch",
    ] {
        let document = document(name)?;
        let error = PdfFont::load(&document, dictionary(&document)?)
            .err()
            .ok_or_else(|| Error::syntax("expected load failure"))?;
        let expected = fixture("text", &format!("{name}.error"));
        let expected = std::str::from_utf8(&expected)
            .map_err(|_| Error::syntax("invalid error oracle"))?
            .trim();
        assert_eq!(
            error.feature().map_or(error.kind.name(), |f| f.as_str()),
            expected,
            "{name}"
        );
        if name.starts_with("unicode-") {
            assert!(error.object.is_some());
        }
    }
    Ok(())
}

#[test]
fn decode_limits_and_positions() -> Result<()> {
    let document = document("type3-differences")?;
    let mut font = PdfFont::load(&document, dictionary(&document)?)?;
    font.limits.string_bytes = 1;
    assert_eq!(
        font.decode(b"AA").err().map(|e| e.kind),
        Some(ErrorKind::Limit)
    );
    font.limits.string_bytes = 100;
    font.limits.value_nodes = 1;
    assert_eq!(
        font.decode(b"AA").err().map(|e| e.position),
        Some(Position::Buffer(1))
    );
    font.limits.value_nodes = 100;
    font.limits.decoded_bytes = 1;
    assert_eq!(
        font.decode(b"C").err().map(|e| e.kind),
        Some(ErrorKind::Limit)
    );
    font.limits.operations = 0;
    assert_eq!(
        font.decode(b"A").err().map(|e| e.kind),
        Some(ErrorKind::Limit)
    );
    Ok(())
}

#[test]
fn base_encoding_vectors() -> Result<()> {
    for name in [
        "winansi",
        "macroman",
        "standard",
        "macexpert",
        "symbol",
        "dingbats",
        "pdfdoc",
        "table-winansi",
        "table-macroman",
    ] {
        let name = format!("encoding-{name}");
        let base = fixture("text", &format!("{name}.base"));
        let encoding = encoding::base(&base)?;
        for value in expected(&name)? {
            let row = rows(&value)?;
            let code =
                usize::try_from(integer(&row[0])?).map_err(|_| Error::syntax("invalid code"))?;
            let unicode = encoding[code]
                .as_ref()
                .and_then(|name| encoding::unicode(name.as_bytes()));
            assert_eq!(unicode.as_deref(), text(&row[4]), "{name}: {code}");
        }
    }
    Ok(())
}

#[test]
fn invalid_metric_vectors() -> Result<()> {
    for name in [
        "cid-width-reversed",
        "cid-width-overflow",
        "cid-w2-truncated",
        "cid-range-truncated",
        "cid-dw2-length",
    ] {
        let document = document(name)?;
        let result = metrics::Metrics::composite(&document, dictionary(&document)?);
        assert_eq!(
            result.err().map(|e| e.kind),
            Some(ErrorKind::Syntax),
            "{name}"
        );
    }
    Ok(())
}

#[test]
fn missing_width_vector() -> Result<()> {
    let document = document("simple-missing-width")?;
    let dictionary = dictionary(&document)?;
    let descriptor = dictionary
        .get(b"FontDescriptor")
        .and_then(Value::as_dictionary);
    let metrics = metrics::Metrics::simple(&document, dictionary, descriptor, 0.001)?;
    for value in expected("simple-missing-width")? {
        let row = rows(&value)?;
        near(metrics.width(integer(&row[0])?), float(&row[1])?);
    }
    Ok(())
}
