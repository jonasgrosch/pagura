//! Appearances of annotations that have no appearance stream (12.5.5):
//! borders, ink/polygon paths, note/file icons and empty widgets (12.5.6).

mod border;
mod ending;
mod icon;
mod ink;
mod line;
mod marks;
mod path;
mod polygon;
mod widget;

use crate::features;
use crate::stream::AppearanceStream;
use pagura_core::{Dictionary, Error, Rect, Result, Value};
use pagura_document::Document;

/// Generates the normal appearance of `annotation`, which has no `AP`, in
/// its rectangle `rect` (the normalized `Rect` entry).
///
/// The appearance is a Form XObject whose BBox is `rect` and which has no
/// Matrix, so fitting it into the rectangle (12.5.5) is the identity and its
/// content is in default user space; the BBox clips every mark to the
/// rectangle. Strokes use the annotation's `CA` and fills its `ca` (else
/// `CA`), clamped to [0, 1], through the appearance's own ExtGState
/// resources; its Resources hold nothing else, so page resources such as
/// default colour spaces do not apply. Reader policy draws Link, Square and Circle
/// borders, Line/Ink/Polygon/PolyLine paths, seven Text icons, four FileAttachment
/// icons, and the MK background and border of empty text and signature widgets.
///
/// Nothing is drawn, and `None` returned, for a rectangle that is a single
/// point, for Popup and Projection annotations, for links without a border,
/// for annotations whose entries select no mark, and for a rectangle
/// without area; the last is decided after reading the annotation, so its
/// errors do not depend on the rectangle's area.
///
/// # Errors
/// `Syntax` errors for malformed entries, `Limit` errors for content and
/// storage budgets, and [`features::ANNOTATION_APPEARANCE`] for subtypes,
/// border styles, rotations and widget values that generation does not
/// cover.
pub fn annotation_appearance(
    document: &Document,
    annotation: &Dictionary,
    rect: Rect,
) -> Result<Option<AppearanceStream>> {
    if rect.width() == 0.0 && rect.height() == 0.0 {
        return Ok(None);
    }
    let subtype = annotation
        .get(b"Subtype")
        .map(|value| document.resolve(value))
        .transpose()?
        .and_then(Value::as_name)
        .ok_or(Error::syntax("annotation has no Subtype"))?;
    let appearance = match subtype.as_bytes() {
        b"Popup" | b"Projection" => return Ok(None),
        b"Link" if no_border(document, annotation)? => return Ok(None),
        b"Link" => border::generate(document, annotation, rect, border::Shape::Link)?,
        b"Square" => border::generate(document, annotation, rect, border::Shape::Square)?,
        b"Circle" => border::generate(document, annotation, rect, border::Shape::Circle)?,
        b"Ink" => ink::generate(document, annotation, rect)?,
        b"Line" => line::generate(document, annotation, rect)?,
        b"Polygon" => polygon::generate(document, annotation, rect, true)?,
        b"PolyLine" => polygon::generate(document, annotation, rect, false)?,
        b"Text" => icon::generate(document, annotation, rect)?,
        b"FileAttachment" => icon::file_attachment(document, annotation, rect)?,
        b"Widget" => widget::generate(document, annotation, rect)?,
        _ => {
            return Err(Error::unsupported(
                features::ANNOTATION_APPEARANCE,
                "annotation appearance generation is not implemented",
            ));
        }
    };
    Ok(appearance.filter(|_| rect.width() > 0.0 && rect.height() > 0.0))
}

/// Whether a link draws no border: an empty colour `C`, or a zero width in
/// BS, else in Border (Tables 166, 176).
fn no_border(document: &Document, annotation: &Dictionary) -> Result<bool> {
    if let Some(color) = annotation.get(b"C")
        && document
            .resolve(color)?
            .as_array()
            .is_some_and(<[Value]>::is_empty)
    {
        return Ok(true);
    }
    if let Some(style) = annotation.get(b"BS") {
        let style = document.resolve_dictionary(style)?;
        return style
            .get(b"W")
            .map(|value| {
                document
                    .resolve(value)?
                    .as_number()
                    .ok_or(Error::syntax("border width must be numeric"))
            })
            .transpose()
            .map(|width| width == Some(0.0));
    }
    if let Some(border) = annotation.get(b"Border") {
        let values = document
            .resolve(border)?
            .as_array()
            .filter(|values| matches!(values.len(), 3 | 4))
            .ok_or(Error::syntax("invalid link Border"))?;
        let width = values.get(2).ok_or(Error::syntax("missing border width"))?;
        return Ok(document.resolve(width)?.as_number() == Some(0.0));
    }
    Ok(false)
}

#[cfg(test)]
mod tests;
