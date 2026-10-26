//! Note and file attachment icons (12.5.6.4, 12.5.6.15). Predefined icons
//! use original geometric symbols by reader policy; no font is required.

use super::marks::{Color, Marks, Pen, Subpath, color};
use crate::stream::AppearanceStream;
use pagura_core::{Dictionary, Error, Name, Rect, Result, Value};
use pagura_document::Document;

/// The icon's frame in its 16 × 16 design space.
const FRAME: Subpath<'static> = Subpath::Rect {
    x: 1.0,
    y: 1.0,
    width: 14.0,
    height: 14.0,
};

/// The icon `Name` (Note by default) scaled from 16 × 16 units to `rect`:
/// a frame filled with `C` (yellow by default) and black 1-unit lines.
pub(super) fn generate(
    document: &Document,
    annotation: &Dictionary,
    rect: Rect,
) -> Result<Option<AppearanceStream>> {
    let name = annotation
        .get(b"Name")
        .map(|value| document.resolve(value))
        .transpose()?
        .and_then(Value::as_name)
        .map_or(b"Note".as_slice(), Name::as_bytes);
    draw(document, annotation, rect, lines(name))
}

/// File attachment icons (12.5.6.15, Table 187). Unknown names use PushPin
/// by reader policy; null is absent (7.3.9). A non-name is malformed.
/// Geometry is original, font-free reader artwork. Payloads stay unopened.
pub(super) fn file_attachment(
    document: &Document,
    annotation: &Dictionary,
    rect: Rect,
) -> Result<Option<AppearanceStream>> {
    let name = match annotation
        .get(b"Name")
        .map(|value| document.resolve(value))
        .transpose()?
    {
        None | Some(Value::Null) => b"PushPin".as_slice(),
        Some(Value::Name(name)) => name.as_bytes(),
        Some(_) => return Err(Error::syntax("file attachment icon Name must be a name")),
    };
    draw(document, annotation, rect, file_lines(name))
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
