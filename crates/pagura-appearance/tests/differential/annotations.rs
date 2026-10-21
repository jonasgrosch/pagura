//! Annotation appearances: the previous renderer drew annotations without
//! `AP` directly. Each page is drawn by that renderer twice, as stored and
//! with every such annotation given the appearance
//! [`pagura_appearance::annotation_appearance`] generates (or hidden when
//! it generates none), and the pixels must be identical. A generation
//! error must be the error the previous renderer reports for the page.

use crate::support::{corpus, open_with};
use pagura_appearance::{AppearanceStream, annotation_appearance};
use pagura_core::{Dictionary, Error, Name, ObjectId, Recovery, Rect, Result, Value};
use pagura_document::{Document, OpenOptions, Page, RevisionKind};
use pagura_legacy::{RenderOptions, compile_page};
use pagura_write::{NewObject, XrefFormat, write_incremental};
use std::collections::BTreeMap;

/// What generation decided for one annotation.
enum Generated {
    Untouched,
    Hidden,
    Drawn(AppearanceStream),
}

#[derive(Debug, Default)]
struct Coverage {
    pages: usize,
    drawn_pages: usize,
    drawn: BTreeMap<String, usize>,
    hidden: usize,
    errors: BTreeMap<String, usize>,
}

/// Whether the previous renderer reaches appearance generation for `d`:
/// no AP, visible, not hidden by optional content and a known subtype when
/// the Invisible flag is set (12.5.3).
fn generated(document: &Document, d: &Dictionary) -> Option<Rect> {
    if d.contains_key(b"AP") || d.contains_key(b"OC") {
        return None;
    }
    let flags = match d.get(b"F") {
        Some(value) => document.resolve(value).ok()?.as_integer()?,
        None => 0,
    };
    if !(0..1024).contains(&flags) || flags & (2 | 32) != 0 || flags & 1 != 0 {
        return None;
    }
    let rect = document.resolve(d.get(b"Rect")?).ok()?.as_array()?;
    Rect::from_array(rect)
}

fn annotations<'a>(document: &'a Document, page: &'a Page) -> Option<&'a [Value]> {
    let values = page.dictionary().get(b"Annots")?;
    document.resolve(values).ok()?.as_array()
}

/// Generates every eligible annotation of `page`, in order.
fn generate_page(document: &Document, page: &Page) -> Result<Vec<Generated>> {
    let mut result = Vec::new();
    for value in annotations(document, page).unwrap_or_default() {
        let Ok(d) = document.resolve_dictionary(value) else {
            result.push(Generated::Untouched);
            continue;
        };
        let Some(rect) = generated(document, d) else {
            result.push(Generated::Untouched);
            continue;
        };
        result.push(match annotation_appearance(document, d, rect)? {
            Some(stream) => Generated::Drawn(stream),
            None => Generated::Hidden,
        });
    }
    Ok(result)
}

/// The document with a revision in which every generated annotation of
/// `page` carries its appearance as `AP`, or the Hidden flag.
fn with_appearances(document: &Document, page: &Page, generated: &[Generated]) -> Option<Vec<u8>> {
    let values = annotations(document, page)?;
    let mut next = document.object_ids().map(|id| id.number).max()? + 1;
    for revision in document.revisions() {
        let size = revision.trailer().get(b"Size")?.as_integer()?;
        next = next.max(u32::try_from(size).ok()?);
    }
    let mut objects = BTreeMap::new();
    let mut array = Vec::new();
    for (value, generated) in values.iter().zip(generated) {
        let mut d = document.resolve_dictionary(value).ok()?.clone();
        match generated {
            Generated::Untouched => {
                array.push(value.clone());
                continue;
            }
            Generated::Hidden => {
                let flags = d.get(b"F").and_then(Value::as_integer).unwrap_or(0);
                d.insert(Name::new(b"F"), Value::Integer(flags | 2));
            }
            Generated::Drawn(stream) => {
                let id = ObjectId::new(next, 0);
                next += 1;
                objects.insert(
                    id,
                    NewObject::Stream {
                        dictionary: stream.dictionary().clone(),
                        data: stream.content().to_vec(),
                    },
                );
                let mut ap = Dictionary::new();
                ap.insert(Name::new(b"N"), Value::Reference(id));
                d.insert(Name::new(b"AP"), Value::Dictionary(ap));
            }
        }
        match value.as_reference() {
            Some(id) => {
                objects.insert(id, NewObject::Value(Value::Dictionary(d)));
                array.push(value.clone());
            }
            None => array.push(Value::Dictionary(d)),
        }
    }
    let mut page_dictionary = page.dictionary().clone();
    page_dictionary.insert(Name::new(b"Annots"), Value::Array(array));
    objects.insert(
        page.id(),
        NewObject::Value(Value::Dictionary(page_dictionary)),
    );
    let mut trailer = document.trailer().clone();
    trailer.insert(Name::new(b"Size"), Value::Integer(i64::from(next)));
    let format = if document
        .revisions()
        .iter()
        .all(|revision| revision.kind() == RevisionKind::Table)
    {
        XrefFormat::Table
    } else {
        XrefFormat::Stream
    };
    write_incremental(
        document.original_bytes(),
        document.revisions().first()?.offset(),
        &trailer,
        &objects,
        format,
        &document.limits(),
    )
    .ok()
}

/// The page drawn at two scales, or the error drawing it reports.
fn draw(document: &Document, index: usize) -> std::result::Result<Vec<Vec<u8>>, Error> {
    let page = document.page(index)?;
    let list = compile_page(document, page)?;
    [1.0, 2.5]
        .into_iter()
        .map(|scale| {
            list.render(&RenderOptions {
                scale,
                ..Default::default()
            })
            .map(|pixmap| pixmap.data().to_vec())
        })
        .collect()
}

#[expect(
    clippy::unwrap_used,
    clippy::panic,
    reason = "a difference fails the test"
)]
fn compare_page(
    name: &str,
    bytes: &[u8],
    options: &OpenOptions,
    index: usize,
    coverage: &mut Coverage,
) {
    let Some(document) = open_with(bytes, options) else {
        return;
    };
    let Ok(page) = document.page(index) else {
        return;
    };
    let generated = match generate_page(&document, page) {
        Ok(generated) => generated,
        Err(error) => {
            let expected = compile_page(&document, page).map(|_| ()).unwrap_err();
            assert_eq!(
                (error.kind, error.message),
                (expected.kind, expected.message),
                "{name} page {index}"
            );
            let tag = error.feature().map_or_else(
                || error.kind.name().to_string(),
                |feature| feature.as_str().to_string(),
            );
            *coverage.errors.entry(tag).or_default() += 1;
            return;
        }
    };
    if generated.iter().all(|g| matches!(g, Generated::Untouched)) {
        return;
    }
    let Some(revision) = with_appearances(&document, page, &generated) else {
        panic!("{name} page {index}: revision could not be written");
    };
    let revised = open_with(&revision, options).unwrap();
    let expected = draw(&document, index);
    let actual = draw(&revised, index);
    match (&actual, &expected) {
        (Ok(actual), Ok(expected)) => {
            assert!(actual == expected, "{name} page {index}: pixels differ");
            coverage.drawn_pages += 1;
        }
        _ => assert_eq!(
            actual.as_ref().map(|_| ()).map_err(|e| (e.kind, e.message)),
            expected
                .as_ref()
                .map(|_| ())
                .map_err(|e| (e.kind, e.message)),
            "{name} page {index}"
        ),
    }
    coverage.pages += 1;
    for (value, generated) in annotations(&document, page).unwrap().iter().zip(&generated) {
        match generated {
            Generated::Untouched => {}
            Generated::Hidden => coverage.hidden += 1,
            Generated::Drawn(_) => {
                let subtype = document
                    .resolve_dictionary(value)
                    .unwrap()
                    .get_name(b"Subtype")
                    .map(|n| String::from_utf8_lossy(n.as_bytes()).into_owned())
                    .unwrap_or_default();
                *coverage.drawn.entry(subtype).or_default() += 1;
            }
        }
    }
}

#[test]
fn generated_annotations_draw_as_the_previous_renderer_drew_them() {
    let mut coverage = Coverage::default();
    let options = [
        OpenOptions::default(),
        OpenOptions {
            recovery: Recovery::Strict,
            ..Default::default()
        },
    ];
    for path in corpus() {
        let bytes = std::fs::read(&path).unwrap();
        if !bytes.windows(6).any(|w| w == b"Annots") {
            continue;
        }
        let name = path.display().to_string();
        for options in &options {
            let pages = open_with(&bytes, options)
                .and_then(|document| document.pages().ok().map(<[Page]>::len))
                .unwrap_or(0);
            for index in 0..pages {
                compare_page(&name, &bytes, options, index, &mut coverage);
            }
        }
    }
    eprintln!("annotation differential coverage: {coverage:?}");
    assert!(coverage.drawn_pages > 40, "{coverage:?}");
    for subtype in ["Link", "Square", "Text", "Widget"] {
        assert!(coverage.drawn.contains_key(subtype), "{coverage:?}");
    }
}

/// A one-page document whose page paints a grey backdrop and has the
/// single annotation dictionary `annotation` (its entries, without `<<`).
#[expect(clippy::unwrap_used, reason = "the fixture text is fixed and valid")]
fn synthetic(annotation: &[u8]) -> Vec<u8> {
    let parse = |text: &[u8]| {
        pagura_core::syntax::Parser::new(
            text,
            pagura_core::syntax::Origin::Buffer,
            pagura_core::Limits::default(),
        )
        .parse()
        .unwrap()
    };
    let id = |number| ObjectId::new(number, 0);
    let mut builder = pagura_write::Builder::new(pagura_core::Limits::default());
    let mut annotation_text = b"<< ".to_vec();
    annotation_text.extend_from_slice(annotation);
    annotation_text.extend_from_slice(b" >>");
    let objects = [
        (1, parse(b"<< /Type /Catalog /Pages 2 0 R >>")),
        (2, parse(b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>")),
    ];
    for (number, value) in objects {
        builder.insert(id(number), NewObject::Value(value)).unwrap();
    }
    let mut page = parse(b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 12 10] /Contents 4 0 R >>")
        .as_dictionary()
        .unwrap()
        .clone();
    page.insert(
        Name::new(b"Annots"),
        Value::Array(vec![parse(&annotation_text)]),
    );
    builder
        .insert(id(3), NewObject::Value(Value::Dictionary(page)))
        .unwrap();
    builder
        .insert(
            id(4),
            NewObject::Stream {
                dictionary: Dictionary::new(),
                data: b"0.5 g 0 0 6 10 re f".to_vec(),
            },
        )
        .unwrap();
    builder.finish(id(1)).unwrap()
}

/// Entries that reach every branch of generation, with coordinates that
/// are not exact in binary.
const SYNTHETIC: &[&[u8]] = &[
    b"/Subtype /Link /Rect [0.3 0.7 7.1 7.9]",
    b"/Subtype /Link /Rect [7.1 7.9 0.3 0.7] /C [0 0 1] /BS << /W 1.3 >>",
    b"/Subtype /Link /Rect [0.3 0.7 11.1 9.9] /BS << /W 1.7 /S /D /D [2 0.5] >> /C [1 0 0]",
    b"/Subtype /Link /Rect [0.3 0.7 11.1 9.9] /BS << /S /D >> /C [0.2 0.4 0.6 0.1]",
    b"/Subtype /Link /Rect [0.3 0.7 11.1 9.9] /BS << /S /U /W 2.25 >> /C [0.25]",
    b"/Subtype /Link /Rect [0.3 0.7 11.1 9.9] /Border [0 0 2.5 [1 1.5]] /CA 0.35",
    b"/Subtype /Link /Rect [0.3 0.7 11.1 9.9] /Border [0 0 0]",
    b"/Subtype /Link /Rect [0.3 0.7 11.1 9.9] /C []",
    b"/Subtype /Link /Rect [0.3 0.7 11.1 9.9] /Border [1 1 2]",
    b"/Subtype /Link /Rect [0.3 0.7 11.1 9.9] /Border [0 0 2 3 4]",
    b"/Subtype /Link /Rect [0.3 0.7 11.1 9.9] /BS << /W /Thick >>",
    b"/Subtype /Link /Rect [0.3 0.7 11.1 9.9] /BS << /S /B >>",
    b"/Subtype /Link /Rect [0.3 0.7 11.1 9.9] /BS << /W -1 >>",
    b"/Subtype /Link /Rect [0.3 0.7 11.1 9.9] /C [1 0]",
    b"/Subtype /Link /Rect [0.3 0.7 11.1 9.9] /CA /Half",
    b"/Subtype /Link /Rect [0.3 0.7 11.1 9.9] /CA 7 /ca -2",
    b"/Subtype /Link /Rect [2 2 2 2]",
    b"/Subtype /Link /Rect [0 0 1000000.5 3.3]",
    b"/Subtype /Square /Rect [0.3 0.7 11.1 9.9] /IC [0 1 0] /C [0 0 0 1] /BS << /W 3.3 >> /CA 0.6 /ca 0.25",
    b"/Subtype /Square /Rect [0.3 0.7 11.1 9.9] /IC [0.5] /Border [0 0 0] /ca 0.4",
    b"/Subtype /Square /Rect [0.3 0.7 11.1 9.9] /IC [] /C [] /ca /Bad",
    b"/Subtype /Square /Rect [0.3 0.7 11.1 9.9] /IC [0 0 1] /CA 0.5",
    b"/Subtype /Square /Rect [0.3 0.7 11.1 9.9] /BE << /S /C >>",
    b"/Subtype /Square /Rect [0.3 0.7 11.1 9.9] /RD [1 1 1 1]",
    b"/Subtype /Square /Rect [0.3 0.7 3.1 9.9] /BS << /W 9 >> /IC [1 0 0]",
    b"/Subtype /Text /Rect [0.3 0.7 11.1 9.9]",
    b"/Subtype /Text /Rect [0.3 0.7 11.1 9.9] /Name /Key /C [0.1 0.2 0.3 0.4] /CA 0.5",
    b"/Subtype /Text /Rect [0.3 0.7 11.1 9.9] /Name (Help) /C []",
    b"/Subtype /Text /Rect [1.25 1.5 7.75 9.5] /Name /Paragraph /ca 0.3",
    b"/Subtype /Text /Rect [0.3 0.7 11.1 9.9] /Name /Unknown /C [2 -1 0.5]",
    b"/Subtype /Text /Rect [0.3 0.7 11.1 9.9] /C [0 0]",
    b"/Subtype /Widget /Rect [0.3 0.7 11.1 9.9] /FT /Tx /MK << /BG [0 0 1] /BC [1 0 0] >> /BS << /W 1.5 /S /D /D [1] >> /CA 0.8",
    b"/Subtype /Widget /Rect [0.3 0.7 11.1 9.9] /FT /Tx /MK << /BC [0.3] >> /BS << /S /U /W 2 >>",
    b"/Subtype /Widget /Rect [0.3 0.7 11.1 9.9] /FT /Sig /MK << /BG [0 0 0 0.5] /BC [0 1 0] /R 360 >> /Border [0 0 4 [2]]",
    b"/Subtype /Widget /Rect [0.3 0.7 11.1 9.9] /FT /Tx /V () /RV () /MK << /BC [0 0 1] /R 180 >>",
    b"/Subtype /Widget /Rect [0.3 0.7 11.1 9.9] /FT /Tx /MK << /BG [0 0 1] /R 180 >>",
    b"/Subtype /Widget /Rect [0.3 0.7 11.1 9.9] /FT /Tx /MK << /BC [0 0 1] /R 45 >>",
    b"/Subtype /Widget /Rect [0.3 0.7 11.1 9.9] /FT /Tx /MK << /BC [0 0 1] >> /BS << /W 0 >>",
    b"/Subtype /Widget /Rect [0.3 0.7 11.1 9.9] /FT /Tx /RV (rich)",
    b"/Subtype /Widget /Rect [0.3 0.7 11.1 9.9] /FT /Btn /MK << /BG [1 0 0] >>",
    b"/Subtype /Widget /Rect [0.3 0.7 11.1 9.9] /MK << /BG [1 0 0] >>",
    b"/Subtype /Widget /Rect [0.3 0.7 0.9 9.9] /FT /Tx /MK << /BC [0 0 1] >> /BS << /W 5 >>",
    b"/Subtype /Popup /Rect [0.3 0.7 11.1 9.9]",
    b"/Subtype /Projection /Rect [0.3 0.7 11.1 9.9]",
    b"/Subtype /Circle /Rect [0.3 0.7 11.1 9.9]",
    b"/Rect [0.3 0.7 11.1 9.9]",
];

#[test]
fn synthetic_annotations_draw_as_the_previous_renderer_drew_them() {
    let mut coverage = Coverage::default();
    for (index, entries) in SYNTHETIC.iter().enumerate() {
        let bytes = synthetic(entries);
        let name = format!("synthetic {index}: {}", String::from_utf8_lossy(entries));
        for recovery in [Recovery::Repair, Recovery::Strict] {
            let options = OpenOptions {
                recovery,
                ..Default::default()
            };
            compare_page(&name, &bytes, &options, 0, &mut coverage);
        }
    }
    eprintln!("synthetic annotation coverage: {coverage:?}");
    assert!(coverage.drawn_pages > 50, "{coverage:?}");
}

/// The one intended difference: a dash array that cannot be followed is
/// rejected when the appearance is generated, as `d` rejects it, where the
/// previous renderer drew the annotation directly and failed only when
/// stroking it, with an `InvalidArgument` error.
#[test]
fn unusable_dash_arrays_fail_at_generation() {
    for entries in [
        b"/Subtype /Link /Rect [0 0 8 8] /BS << /S /D /D [-1 2] >>".as_slice(),
        b"/Subtype /Link /Rect [0 0 8 8] /Border [0 0 1 [0 0]]",
        b"/Subtype /Widget /Rect [0 0 8 8] /FT /Tx /MK << /BC [0] >> /Border [0 0 1 [0]]",
    ] {
        let bytes = synthetic(entries);
        let document = open_with(&bytes, &OpenOptions::default()).unwrap();
        let page = document.page(0).unwrap();
        let error = generate_page(&document, page).map(|_| ()).unwrap_err();
        assert_eq!(error.message, "invalid annotation dash pattern");
        let previous = compile_page(&document, page)
            .unwrap()
            .render(&RenderOptions::default())
            .unwrap_err();
        assert_eq!(previous.kind, pagura_core::ErrorKind::InvalidArgument);
    }
}

/// The other intended difference: an annotation whose rectangle has no
/// area gets no appearance, as fitting a stored one into it would scale it
/// to nothing (12.5.5); its entries are still read, with the same errors.
/// The previous renderer stroked such a border directly, and its clip to
/// the area-less rectangle did not remove the stroke.
#[test]
fn rectangles_without_area_get_no_appearance_after_validation() {
    for entries in [
        b"/Subtype /Link /Rect [2 2 2 9]".as_slice(),
        b"/Subtype /Square /Rect [2 2 9 2] /IC [1 0 0]",
        b"/Subtype /Text /Rect [2 2 2 9]",
        b"/Subtype /Widget /Rect [2 2 2 9] /FT /Tx /MK << /BC [0] >>",
    ] {
        let bytes = synthetic(entries);
        let document = open_with(&bytes, &OpenOptions::default()).unwrap();
        let page = document.page(0).unwrap();
        let generated = generate_page(&document, page).unwrap();
        assert!(matches!(generated.as_slice(), [Generated::Hidden]));
    }
    for entries in [
        b"/Subtype /Link /Rect [2 2 2 9] /BS << /W /Thick >>".as_slice(),
        b"/Subtype /Text /Rect [2 2 2 9] /C [0 0]",
        b"/Subtype /Widget /Rect [2 2 2 9] /FT /Tx /V (value)",
    ] {
        let bytes = synthetic(entries);
        let document = open_with(&bytes, &OpenOptions::default()).unwrap();
        let page = document.page(0).unwrap();
        let error = generate_page(&document, page).map(|_| ()).unwrap_err();
        let previous = compile_page(&document, page).map(|_| ()).unwrap_err();
        assert_eq!(
            (error.kind, error.message),
            (previous.kind, previous.message)
        );
    }
}
