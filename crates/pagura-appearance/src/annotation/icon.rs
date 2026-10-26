//! Note icons (12.5.6.4). The seven predefined icons are simple geometric
//! symbols by reader policy; no document font is required.

use super::marks::{Color, Marks, Pen, Subpath, color};
use crate::stream::AppearanceStream;
use pagura_core::{Dictionary, Name, Rect, Value};
use pagura_document::Document;
use pagura_document::source::ReadResult;

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
) -> ReadResult<Option<AppearanceStream>> {
    let name = annotation
        .get(b"Name")
        .map(|value| document.resolve(value))
        .transpose()?
        .and_then(Value::as_name)
        .map_or(b"Note".as_slice(), Name::as_bytes);
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
    let lines = lines(name);
    let path: Vec<Subpath<'_>> = std::iter::once(FRAME)
        .chain(lines.iter().map(|line| Subpath::Polyline(line)))
        .collect();
    let pen = Pen {
        width: 1.0,
        dash: &[],
    };
    marks.stroke(&path, &Color::black(), pen)?;
    Ok(marks.finish(rect)?)
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
