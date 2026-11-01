//! Note, file attachment and Sound icons, and static Movie posters
//! (12.5.6.4, 12.5.6.15–17). Predefined icons use original geometric symbols by
//! reader policy; posters retain source image references for the interpreter.

use super::marks::{Color, Marks, Pen, Subpath, color};
use crate::program::Program;
use crate::stream::AppearanceStream;
use crate::values::{optional, optional_dictionary};
use pagura_core::{Dictionary, Error, Name, Rect, Result, Rotation, Value};
use pagura_document::Document;

/// The icon's frame in its 16 × 16 design space.
const FRAME: Subpath<'static> = Subpath::Rect {
    x: 1.0,
    y: 1.0,
    width: 14.0,
    height: 14.0,
};

/// The predefined reader-artwork family. Media and file payloads are not read.
#[derive(Debug, Clone, Copy)]
pub(super) enum Kind {
    Text,
    FileAttachment,
    Sound,
}

/// The icon `Name`, scaled from 16 × 16 units to `rect`: a frame filled
/// with `C` (yellow by default) and black 1-unit lines. Unknown names use
/// the family's default (Note, `PushPin` or Speaker) by reader policy.
pub(super) fn generate(
    document: &Document,
    annotation: &Dictionary,
    rect: Rect,
    kind: Kind,
) -> Result<Option<AppearanceStream>> {
    let default: &[u8] = match kind {
        Kind::Text => b"Note",
        Kind::FileAttachment => b"PushPin",
        Kind::Sound => b"Speaker",
    };
    let name = match annotation
        .get(b"Name")
        .map(|value| document.resolve(value))
        .transpose()?
    {
        None | Some(Value::Null) => default,
        Some(Value::Name(name)) => name.as_bytes(),
        // Preserve the existing Text-name fallback. Other icon families
        // reject malformed visual entries independently of rectangle area.
        Some(_) if matches!(kind, Kind::Text) => default,
        Some(_) => return Err(Error::syntax("annotation icon Name must be a name")),
    };
    let lines = match kind {
        Kind::Text => lines(name),
        Kind::FileAttachment => file_lines(name),
        Kind::Sound => sound_lines(name),
    };
    draw(document, annotation, rect, lines)
}

/// Table 306: only an explicit image stream is a self-contained poster.
/// Reader policy fits its unit square to Rect, applying the movie's clockwise
/// quarter-turn first. The ordinary image interpreter owns samples, masks and
/// colour spaces; movie files, Aspect and activation data are never resolved.
pub(super) fn movie_poster(
    document: &Document,
    annotation: &Dictionary,
    rect: Rect,
) -> Result<Option<AppearanceStream>> {
    let movie = optional_dictionary(document, annotation, b"Movie")?
        .ok_or(Error::syntax("movie annotation requires Movie dictionary"))?;
    match optional(document, movie, b"Poster")? {
        None | Some(Value::Boolean(false)) => return Ok(None),
        Some(Value::Boolean(true)) => {
            return Err(Error::unsupported(
                crate::features::ANNOTATION_APPEARANCE,
                "movie-file poster retrieval is unavailable",
            ));
        }
        Some(_) => {}
    }
    let id = movie
        .get(b"Poster")
        .and_then(Value::as_reference)
        .ok_or(Error::syntax("Movie Poster must reference an image stream"))?;
    let object = document.object(id)?;
    let dictionary = object
        .dictionary()
        .filter(|_| object.is_stream())
        .ok_or(Error::syntax("Movie Poster must be an image stream"))?;
    if optional(document, dictionary, b"Subtype")?
        .and_then(Value::as_name)
        .map(Name::as_bytes)
        != Some(b"Image")
    {
        return Err(Error::syntax("Movie Poster must have Image subtype").in_object(id));
    }
    let rotation = optional(document, movie, b"Rotate")?
        .map(|value| {
            value
                .as_integer()
                .ok_or(Error::syntax("Movie Rotate must be an integer"))
                .and_then(Rotation::try_from)
        })
        .transpose()?
        .unwrap_or_default();
    let frame = crate::widget_rotation::frame(rotation, rect)?;
    let bounds = frame.bounds;
    let mut out = Program::new(&document.limits());
    out.text(&format!(
        "q\n{} 0 0 {} {} {} cm\n/Poster Do\nQ\n",
        bounds.width(),
        bounds.height(),
        bounds.x0(),
        bounds.y0()
    ))?;
    let mut objects = Dictionary::new();
    objects.insert(Name::new(b"Poster"), Value::Reference(id));
    let mut resources = Dictionary::new();
    resources.insert(Name::new(b"XObject"), Value::Dictionary(objects));
    let mut dictionary = frame.dictionary();
    dictionary.insert(Name::new(b"Resources"), Value::Dictionary(resources));
    AppearanceStream::new(document, dictionary, out.finish()).map(Some)
}

/// Original rectilinear Speaker/Mic symbols, unrelated to measured reader
/// pixels. Closed contours start inside an edge so their butt caps meet.
fn sound_lines(name: &[u8]) -> &'static [&'static [(f64, f64)]] {
    match name {
        b"Mic" => &[
            &[
                (8., 6.),
                (6., 6.),
                (6., 13.),
                (10., 13.),
                (10., 6.),
                (8., 6.),
            ],
            &[(4., 10.), (4., 5.), (12., 5.), (12., 10.)],
            &[(8., 5.), (8., 2.)],
            &[(5., 2.), (11., 2.)],
        ],
        _ => &[
            &[
                (4., 6.),
                (6., 6.),
                (6., 4.),
                (10., 4.),
                (10., 12.),
                (6., 12.),
                (6., 10.),
                (3., 10.),
                (3., 6.),
                (4., 6.),
            ],
            &[(12., 6.), (12., 10.)],
            &[(14., 4.), (14., 12.)],
        ],
    }
}

fn draw(
    document: &Document,
    annotation: &Dictionary,
    rect: Rect,
    lines: &'static [&'static [(f64, f64)]],
) -> Result<Option<AppearanceStream>> {
    let mut marks = Marks::new(document, annotation);
    marks.transform([
        rect.width() / 16.0,
        0.0,
        0.0,
        rect.height() / 16.0,
        rect.x0(),
        rect.y0(),
    ])?;
    if let Some(background) = color(document, annotation, b"C", Some(Color::yellow()))? {
        marks.fill(&[FRAME], &background)?;
    }
    let path: Vec<Subpath<'_>> = std::iter::once(FRAME)
        .chain(lines.iter().map(|line| Subpath::Polyline(line)))
        .collect();
    let pen = Pen {
        width: 1.0,
        dash: &[],
    };
    marks.stroke(&path, &Color::black(), pen)?;
    marks.finish(rect)
}

/// The lines drawn inside the frame of icon `name`.
fn lines(name: &[u8]) -> &'static [&'static [(f64, f64)]] {
    match name {
        b"Comment" => &[
            &[(4., 11.), (12., 11.)],
            &[(4., 8.), (12., 8.)],
            &[(4., 5.), (9., 5.)],
            &[(4., 1.), (2., 0.)],
        ],
        b"Key" => &[
            &[(3., 9.), (3., 13.), (7., 13.), (7., 9.), (3., 9.)],
            &[(7., 9.), (12., 4.), (12., 2.), (14., 2.)],
        ],
        b"Help" => &[
            &[
                (4., 11.),
                (5., 13.),
                (10., 13.),
                (12., 11.),
                (11., 9.),
                (8., 7.),
                (8., 5.),
            ],
            &[(8., 3.), (8., 2.)],
        ],
        b"Paragraph" => &[
            &[
                (10., 2.),
                (10., 13.),
                (5., 13.),
                (3., 11.),
                (3., 9.),
                (5., 7.),
                (8., 7.),
            ],
            &[(8., 2.), (8., 13.)],
        ],
        b"NewParagraph" => &[
            &[
                (10., 4.),
                (10., 13.),
                (5., 13.),
                (3., 11.),
                (3., 9.),
                (5., 7.),
                (8., 7.),
            ],
            &[(8., 4.), (8., 13.)],
            &[(3., 2.), (13., 2.)],
        ],
        b"Insert" => &[&[(3., 4.), (8., 12.), (13., 4.)]],
        _ => &[
            &[(4., 12.), (12., 12.)],
            &[(4., 9.), (12., 9.)],
            &[(4., 6.), (10., 6.)],
            &[(10., 1.), (10., 4.), (15., 4.)],
        ],
    }
}

/// Four distinct framed symbols, in the same 16-unit design space as notes.
/// Closed contours start inside an edge, so coincident butt caps meet without
/// requiring an artificial join at a corner.
fn file_lines(name: &[u8]) -> &'static [&'static [(f64, f64)]] {
    match name {
        b"Graph" => &[
            &[(3., 13.), (3., 3.), (13., 3.)],
            &[(5., 3.), (5., 6.)],
            &[(8., 3.), (8., 9.)],
            &[(11., 3.), (11., 12.)],
        ],
        b"Paperclip" => &[&[
            (8., 5.),
            (8., 11.),
            (10., 11.),
            (10., 4.),
            (5., 4.),
            (5., 13.),
            (12., 13.),
            (12., 3.),
        ]],
        b"Tag" => &[
            &[
                (3., 8.),
                (3., 4.),
                (11., 4.),
                (11., 6.),
                (13., 6.),
                (13., 10.),
                (11., 10.),
                (11., 12.),
                (3., 12.),
                (3., 8.),
            ],
            &[
                (10., 7.),
                (11., 7.),
                (11., 9.),
                (9., 9.),
                (9., 7.),
                (10., 7.),
            ],
        ],
        _ => &[
            &[
                (8., 11.),
                (5., 11.),
                (5., 13.),
                (11., 13.),
                (11., 11.),
                (8., 11.),
            ],
            &[
                (6., 11.),
                (6., 7.),
                (4., 7.),
                (4., 5.),
                (12., 5.),
                (12., 7.),
                (10., 7.),
                (10., 11.),
            ],
            &[(8., 5.), (8., 2.)],
        ],
    }
}
