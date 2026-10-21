//! Image sources decode exactly as the decode entry points do: for every
//! image XObject and page-content inline image of the generated corpus and
//! of the benchmark pages (when the corpus is present), reading a source and
//! decoding it fully gives the same image, the same error and the same
//! repairs, in both recovery modes.

use pagura_core::{Diagnostic, Name, ObjectId, Recovery, Result, Value, syntax};
use pagura_document::{Document, OpenOptions};
use pagura_image::deferred::{SampleLayout, storage_bytes};
use pagura_image::{
    DecodeRequest, DecodedImage, ImageId, ImageSource, Layout, MaskKind, Reduce, SourceMask,
    decode_inline, decode_xobject, decode_xobject_with, inline_data_length,
};
use std::path::{Path, PathBuf};

/// The outcome of one decode and the repairs it reported.
type Outcome = (Result<DecodedImage>, Vec<Diagnostic>);

#[derive(Default)]
struct Tally {
    images: usize,
    failures: usize,
    inline: usize,
    requests: usize,
    mismatches: Vec<String>,
}

fn open(bytes: &[u8], recovery: Recovery) -> Option<Document> {
    Document::open(
        bytes.to_vec(),
        &OpenOptions {
            recovery,
            ..OpenOptions::default()
        },
    )
    .ok()
}

fn reported(document: &Document, before: usize) -> Vec<Diagnostic> {
    document.diagnostics().split_off(before)
}

fn old_xobject(document: &Document, id: ObjectId) -> Option<Outcome> {
    let object = document.object(id).ok()?;
    let before = document.diagnostics().len();
    let result = decode_xobject(document, object);
    Some((result, reported(document, before)))
}

/// A source and the repairs reported while reading it.
type Read = (ImageSource, Vec<Diagnostic>);

fn new_xobject(document: &Document, id: ObjectId) -> Option<(Outcome, Option<Read>)> {
    let object = document.object(id).ok()?;
    let before = document.diagnostics().len();
    let source = ImageSource::from_xobject(document, object);
    Some(decode_source(document, source, before))
}

fn decode_source(
    document: &Document,
    source: Result<ImageSource>,
    before: usize,
) -> (Outcome, Option<Read>) {
    let source = match source {
        Ok(source) => source,
        Err(error) => return ((Err(error), reported(document, before)), None),
    };
    let read = reported(document, before);
    let mut diagnostics = read.clone();
    let result = source
        .decode(
            &DecodeRequest::FULL,
            &document.limits(),
            document.recovery(),
        )
        .map(|decoded| {
            diagnostics.extend(decoded.diagnostics);
            decoded.image
        });
    ((result, diagnostics), Some((source, read)))
}

/// The source describes the decoded image without decoding it.
fn describes(source: &ImageSource, image: &DecodedImage) -> Option<&'static str> {
    if (source.width(), source.height()) != (image.width, image.height) {
        return Some("dimensions");
    }
    if source.components() != image.components {
        return Some("components");
    }
    if (0..usize::from(image.components))
        .any(|i| source.component_bits(i) != image.component_bits(i))
        || source.bits_per_component() != image.bits_per_component
    {
        return Some("precision");
    }
    if source.kind() != &image.kind {
        return Some("kind");
    }
    if source.decode_array() != image.decode.as_deref() || source.interpolate() != image.interpolate
    {
        return Some("decode or interpolate");
    }
    let mask = match (source.mask(), &image.mask) {
        (SourceMask::None, MaskKind::None) => true,
        (SourceMask::ColorKey(ranges), MaskKind::ColorKey(keys)) => ranges
            .iter()
            .map(|&[low, high]| (low, high))
            .eq(keys.iter().copied()),
        (SourceMask::Stencil(a), MaskKind::Stencil(b)) => a == b,
        (SourceMask::Soft { object, .. }, MaskKind::Soft(id)) => object == id,
        (SourceMask::Embedded { premultiplied }, MaskKind::Embedded(alpha)) => {
            *premultiplied == alpha.premultiplied
        }
        _ => false,
    };
    (!mask).then_some("mask")
}

fn compare(
    tally: &mut Tally,
    label: &str,
    old: &Outcome,
    new: &Outcome,
    source: Option<&ImageSource>,
) {
    tally.images += 1;
    if old.0.is_err() {
        tally.failures += 1;
    }
    let difference = if old.0 != new.0 {
        Some(match (&old.0, &new.0) {
            (Err(a), Err(b)) => format!("error {a:?} vs {b:?}"),
            (Err(a), Ok(_)) => format!("old error {a:?}, new success"),
            (Ok(_), Err(b)) => format!("old success, new error {b:?}"),
            (Ok(_), Ok(_)) => "decoded image".to_owned(),
        })
    } else if old.1 != new.1 {
        Some(format!("repairs {:?} vs {:?}", old.1, new.1))
    } else if let (Ok(image), Some(source)) = (&new.0, source) {
        describes(source, image).map(|what| format!("source {what}"))
    } else {
        None
    };
    if let Some(difference) = difference {
        tally.mismatches.push(format!("{label}: {difference}"));
    }
}

fn is_image(document: &Document, id: ObjectId) -> bool {
    let Ok(object) = document.object(id) else {
        return false;
    };
    let Some(dictionary) = object.dictionary().filter(|_| object.is_stream()) else {
        return false;
    };
    let subtype = dictionary
        .get(b"Subtype")
        .and_then(|v| document.resolve(v).ok())
        .and_then(Value::as_name)
        .is_some_and(|name| name == b"Image");
    subtype || dictionary.contains_key(b"ImageMask")
}

/// Compares the image XObjects `ids` (every image when `None`) and the
/// inline images of page `page` (every page when `None`).
fn check_xobjects(
    tally: &mut Tally,
    label: &str,
    bytes: &[u8],
    (ids, page): (Option<&[ObjectId]>, Option<usize>),
) {
    for recovery in [Recovery::Repair, Recovery::Strict] {
        let (Some(old), Some(new)) = (open(bytes, recovery), open(bytes, recovery)) else {
            continue;
        };
        let all: Vec<ObjectId>;
        let ids = if let Some(ids) = ids {
            ids
        } else {
            all = old.object_ids().filter(|&id| is_image(&old, id)).collect();
            &all
        };
        for &id in ids {
            let (Some(a), Some((b, source))) = (old_xobject(&old, id), new_xobject(&new, id))
            else {
                continue;
            };
            let label = format!("{label} {id} {recovery:?}");
            compare(tally, &label, &a, &b, source.as_ref().map(|(s, _)| s));
            if let (Ok(_), Some(source)) = (&a.0, &source) {
                check_requests(tally, &label, (bytes, recovery), id, source);
            }
        }
        check_inline(tally, label, (&old, &new), page);
    }
}

/// Native layouts and drawn-size cells decode as the entry point's sample
/// layouts do, and the decode cost bounds the decoded storage. Each old
/// decode opens the document afresh, which reports repeated repairs again.
fn check_requests(
    tally: &mut Tally,
    label: &str,
    (bytes, recovery): (&[u8], Recovery),
    id: ObjectId,
    (source, read): &Read,
) {
    let mut requests = vec![(
        SampleLayout::Deferred,
        DecodeRequest::new(Reduce::Full, None, Layout::Native),
    )];
    if source.drawn(20.0) != Reduce::Full {
        for cells in [1, 2, 3, 4, 6] {
            assert_eq!(
                source.drawn(20.0 / f64::from(cells)),
                Reduce::Drawn { cells }
            );
            requests.push((
                SampleLayout::Cells(cells),
                DecodeRequest::new(Reduce::Drawn { cells }, None, Layout::Interleaved),
            ));
        }
        assert_eq!(source.drawn(20.0 / 6.0 - 1e-9), Reduce::Full);
    }
    for (layout, request) in requests {
        let Some(old) = open(bytes, recovery) else {
            return;
        };
        let Ok(object) = old.object(id) else {
            return;
        };
        let before = old.diagnostics().len();
        let a = (
            decode_xobject_with(&old, object, layout, None),
            reported(&old, before),
        );
        let mut diagnostics = read.clone();
        let image = source
            .decode(&request, &old.limits(), recovery)
            .map(|decoded| {
                diagnostics.extend(decoded.diagnostics);
                decoded.image
            });
        let b = (image, diagnostics);
        tally.requests += 1;
        if a != b {
            tally
                .mismatches
                .push(format!("{label} {layout:?}: differs from the entry point"));
        }
        if let Ok(image) = &b.0 {
            let (bytes, work) = source.decode_cost(&request);
            if bytes < storage_bytes(&image.samples) || work == 0 {
                tally
                    .mismatches
                    .push(format!("{label} {layout:?}: decode cost below its storage"));
            }
        }
    }
}

/// Inline images of page content, framed as content interpretation frames
/// them.
fn check_inline(
    tally: &mut Tally,
    label: &str,
    (old, new): (&Document, &Document),
    only: Option<usize>,
) {
    let Ok(pages) = old.pages() else {
        return;
    };
    for (index, page) in pages.iter().enumerate() {
        if only.is_some_and(|only| only != index) {
            continue;
        }
        let Ok(streams) = old.content_streams(page) else {
            continue;
        };
        for stream in streams {
            let Ok(data) = old.stream_data(stream) else {
                continue;
            };
            for (offset, dictionary, image) in inline_images(old, page.resources(), &data) {
                let id = ImageId::Inline {
                    stream: Some(stream.id()),
                    offset,
                };
                let before = old.diagnostics().len();
                let a = (
                    decode_inline(old, page.resources(), &dictionary, image),
                    reported(old, before),
                );
                let before = new.diagnostics().len();
                let source =
                    ImageSource::from_inline(new, page.resources(), &dictionary, image, id);
                let (b, source) = decode_source(new, source, before);
                tally.inline += 1;
                compare(
                    tally,
                    &format!("{label} page {index} inline {offset}"),
                    &a,
                    &b,
                    source.as_ref().map(|(s, _)| s),
                );
            }
        }
    }
}

/// The inline images of `content`: the offset of each `BI`, its dictionary
/// and its data.
fn inline_images<'a>(
    document: &Document,
    resources: &pagura_core::Dictionary,
    content: &'a [u8],
) -> Vec<(usize, pagura_core::Dictionary, &'a [u8])> {
    let mut images = Vec::new();
    let mut parser = syntax::Parser::new(content, syntax::Origin::Buffer, document.limits());
    loop {
        parser.skip_whitespace();
        let Some(byte) = parser.peek() else {
            break;
        };
        let start = parser.offset();
        if matches!(
            byte,
            b'/' | b'(' | b'[' | b'<' | b'+' | b'-' | b'.' | b'0'..=b'9'
        ) {
            if parser.parse().is_err() {
                break;
            }
            continue;
        }
        match parser.word() {
            Ok(b"BI") => match inline_image(document, resources, &mut parser) {
                Some((dictionary, data)) => images.push((start, dictionary, data)),
                None => break,
            },
            Ok(_) => {}
            Err(_) => break,
        }
    }
    images
}

fn inline_image<'a>(
    document: &Document,
    resources: &pagura_core::Dictionary,
    parser: &mut syntax::Parser<'a>,
) -> Option<(pagura_core::Dictionary, &'a [u8])> {
    let mut dictionary = pagura_core::Dictionary::new();
    loop {
        parser.skip_whitespace();
        if parser.peek() == Some(b'I') {
            if parser.word().ok()? != b"ID" {
                return None;
            }
            break;
        }
        let key = parser.parse().ok()?.as_name().cloned()?;
        let value = parser.parse().ok()?;
        dictionary.insert(key, value);
    }
    let offset = parser.offset();
    let byte = *parser.data().get(offset)?;
    let offset = offset
        + if byte == b'\r' && parser.data().get(offset + 1) == Some(&b'\n') {
            2
        } else {
            1
        };
    let data = parser.data().get(offset..)?;
    let length = inline_data_length(&framing(document, resources, &dictionary), data).ok()?;
    parser.seek(offset + length).ok()?;
    parser.skip_whitespace();
    if parser.word().ok()? != b"EI" {
        return None;
    }
    Some((dictionary, data.get(..length)?))
}

/// The dictionary content interpretation frames an inline image with: a
/// named colour space replaced by its definition, an ICC-based one by the
/// device space of its component count.
fn framing(
    document: &Document,
    resources: &pagura_core::Dictionary,
    dictionary: &pagura_core::Dictionary,
) -> pagura_core::Dictionary {
    let mut framing = dictionary.clone();
    let named = dictionary
        .get(b"CS")
        .or_else(|| dictionary.get(b"ColorSpace"))
        .and_then(Value::as_name)
        .filter(|n| {
            !matches!(
                n.as_bytes(),
                b"G" | b"RGB" | b"CMYK" | b"DeviceGray" | b"DeviceRGB" | b"DeviceCMYK"
            )
        });
    let Some(name) = named else {
        return framing;
    };
    let space = resources
        .get(b"ColorSpace")
        .and_then(|v| document.resolve_dictionary(v).ok())
        .and_then(|spaces| spaces.get(name.as_bytes()))
        .and_then(|v| document.resolve(v).ok());
    let Some(space) = space else {
        return framing;
    };
    let icc = space.as_array().and_then(|array| {
        (array.first()?.as_name()? == b"ICCBased")
            .then(|| document.resolve_dictionary(array.get(1)?).ok())
            .flatten()
            .and_then(|profile| profile.get(b"N")?.as_integer())
    });
    let space = match icc {
        Some(1) => Value::name(b"DeviceGray"),
        Some(3) => Value::name(b"DeviceRGB"),
        Some(4) => Value::name(b"DeviceCMYK"),
        _ => space.clone(),
    };
    framing.insert(Name::new(b"ColorSpace"), space);
    framing.remove(b"CS");
    framing
}

fn pdfs(directory: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    let mut entries: Vec<_> = entries.filter_map(|e| e.ok().map(|e| e.path())).collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            pdfs(&path, out);
        } else if path.extension().is_some_and(|e| e == "pdf") {
            out.push(path);
        }
    }
}

fn report(tally: &Tally, what: &str) {
    eprintln!(
        "{what}: {} images ({} inline), {} other requests, {} decode errors, {} mismatches",
        tally.images,
        tally.inline,
        tally.requests,
        tally.failures,
        tally.mismatches.len()
    );
    for mismatch in tally.mismatches.iter().take(40) {
        eprintln!("  {mismatch}");
    }
    assert!(
        tally.mismatches.is_empty(),
        "{what}: sources differ from the decode entry points"
    );
}

#[test]
fn sources_decode_every_generated_image_as_the_entry_points_do() {
    let mut files = Vec::new();
    pdfs(
        &pagura_testkit::workspace_root().join("corpus/generated"),
        &mut files,
    );
    assert!(files.len() > 100, "generated corpus missing");
    let mut tally = Tally::default();
    for path in &files {
        let Ok(bytes) = std::fs::read(path) else {
            continue;
        };
        let label = path
            .strip_prefix(pagura_testkit::workspace_root())
            .unwrap_or(path)
            .display()
            .to_string();
        check_xobjects(&mut tally, &label, &bytes, (None, None));
    }
    report(&tally, "generated corpus");
    assert!(tally.images > 100);
}

/// The image XObjects a page's resources reach, through forms.
fn page_images(document: &Document, page: usize) -> Vec<ObjectId> {
    let mut images = Vec::new();
    let Ok(page) = document.page(page) else {
        return images;
    };
    let mut pending = vec![page.resources().clone()];
    let mut seen = std::collections::BTreeSet::new();
    while let Some(resources) = pending.pop() {
        let Some(xobjects) = resources
            .get(b"XObject")
            .and_then(|v| document.resolve_dictionary(v).ok())
        else {
            continue;
        };
        for (_, value) in xobjects {
            let Some(id) = value.as_reference() else {
                continue;
            };
            if !seen.insert(id) {
                continue;
            }
            if is_image(document, id) {
                images.push(id);
            } else if let Some(resources) = document
                .object(id)
                .ok()
                .and_then(|o| o.dictionary())
                .and_then(|d| d.get(b"Resources"))
                .and_then(|v| document.resolve_dictionary(v).ok())
            {
                pending.push(resources.clone());
            }
        }
    }
    images
}

#[test]
fn sources_decode_the_benchmark_pages_images_as_the_entry_points_do() {
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
        return;
    };
    let list = home.join(".cache/pagura-coordination/fleet/bench-pages.json");
    let corpora = home.join(".cache/pagura-corpora");
    let Ok(text) = std::fs::read_to_string(&list) else {
        eprintln!("benchmark page list absent; skipped");
        return;
    };
    let list = pagura_testkit::Json::parse(&text).expect("benchmark page list is JSON");
    let pages = list.as_array().expect("benchmark page list is an array");
    let mut tally = Tally::default();
    for entry in pages {
        let (Some(file), Some(page)) = (
            entry.get("file").and_then(pagura_testkit::Json::as_str),
            entry.get("page").and_then(pagura_testkit::Json::as_u64),
        ) else {
            continue;
        };
        let Ok(bytes) = std::fs::read(corpora.join(file)) else {
            eprintln!("{file} absent; skipped");
            continue;
        };
        let page = usize::try_from(page).unwrap_or(usize::MAX);
        let Some(document) = open(&bytes, Recovery::Repair) else {
            continue;
        };
        let ids = page_images(&document, page);
        check_xobjects(
            &mut tally,
            &format!("{file} p{page}"),
            &bytes,
            (Some(&ids), Some(page)),
        );
    }
    report(&tally, "benchmark pages");
}
