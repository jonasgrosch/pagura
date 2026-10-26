//! Empty variable-text and unsigned signature widgets (12.5.6.19, 12.7.5).
//! Values and field types inherit through Parent; appearance characteristics
//! do not. Nonempty values require text layout or a supplied appearance
//! stream.

use super::border::Border;
use super::marks::{Marks, Pen, Subpath, color};
use crate::features;
use crate::stream::AppearanceStream;
use crate::widget_rotation;
use pagura_core::{Dictionary, Error, Rect, Result, Value};
use pagura_document::Document;

/// The MK background and border of an empty text or signature widget.
pub(super) fn generate(
    document: &Document,
    annotation: &Dictionary,
    rect: Rect,
) -> Result<Option<AppearanceStream>> {
    let unsupported = || {
        Error::unsupported(
            features::ANNOTATION_APPEARANCE,
            "widget value requires an explicit appearance or text layout",
        )
    };
    let kind = inherited(document, annotation, b"FT")?
        .and_then(Value::as_name)
        .ok_or(Error::syntax("widget has no field type"))?;
    let value = inherited(document, annotation, b"V")?;
    let empty = value.is_none_or(|v| v.is_null() || v.as_string().is_some_and(<[u8]>::is_empty));
    if !matches!(kind.as_bytes(), b"Tx" | b"Sig") || !empty {
        return Err(unsupported());
    }
    if let Some(rich) = inherited(document, annotation, b"RV")?
        && !rich.is_null()
        && rich.as_string() != Some(b"")
    {
        return Err(unsupported());
    }
    let mut marks = Marks::new(document, annotation);
    if let Some(mk) = annotation.get(b"MK") {
        let mk = document.resolve_dictionary(mk)?;
        characteristics(&mut marks, document, annotation, mk, rect)?;
    }
    marks.finish(rect)
}

/// Table 189: absent BG and BC mean no background and no border. DA's
/// colours apply to variable text, not to the widget rectangle.
fn characteristics(
    marks: &mut Marks<'_>,
    document: &Document,
    annotation: &Dictionary,
    mk: &Dictionary,
    rect: Rect,
) -> Result<()> {
    let rotation = widget_rotation::read(document, Some(mk))?;
    let frame = widget_rotation::frame(rotation, rect)?;
    if let Some(matrix) = frame.matrix {
        marks.transform(matrix)?;
    }
    let rect = frame.bounds;
    if let Some(background) = color(document, mk, b"BG", None)? {
        let area = Subpath::Rect {
            x: rect.x0(),
            y: rect.y0(),
            width: rect.width(),
            height: rect.height(),
        };
        marks.fill(&[area], &background)?;
    }
    let Some(color) = color(document, mk, b"BC", None)? else {
        return Ok(());
    };
    let border = Border::read_rectangular(document, annotation)?;
    if border.width == 0.0 {
        return Ok(());
    }
    if border.paint_relief(marks, rect, &color)? {
        return Ok(());
    }
    let inset = (border.width * 0.5)
        .min(rect.width() * 0.5)
        .min(rect.height() * 0.5);
    let underline = [
        (rect.x0() + inset, rect.y0() + inset),
        (rect.x1() - inset, rect.y0() + inset),
    ];
    let path = if border.underline {
        Subpath::Polyline(&underline)
    } else {
        let bounds = Rect::from_corners(
            rect.x0() + inset,
            rect.y0() + inset,
            rect.x1() - inset,
            rect.y1() - inset,
        )
        .ok_or(Error::limit("widget border bounds exceed numeric range"))?;
        border.outline(bounds)
    };
    let pen = Pen {
        width: border.width,
        dash: &border.dash,
    };
    marks.stroke(&[path], &color, pen)
}

/// The value of `key` in the field dictionary or its nearest ancestor that
/// has a non-null one (12.7.4.1).
fn inherited<'a>(
    document: &'a Document,
    mut dictionary: &'a Dictionary,
    key: &[u8],
) -> Result<Option<&'a Value>> {
    let mut visited = Vec::new();
    for _ in 0..document.limits().depth {
        if let Some(value) = dictionary.get(key) {
            // A null entry is equivalent to absence (7.3.9), so inheritance
            // continues rather than concealing a parent's field value.
            let value = document.resolve(value)?;
            if !value.is_null() {
                return Ok(Some(value));
            }
        }
        let Some(parent) = dictionary.get(b"Parent") else {
            return Ok(None);
        };
        if document.resolve(parent)?.is_null() {
            return Ok(None);
        }
        if let Some(id) = parent.as_reference() {
            if visited.contains(&id) {
                return Err(Error::cycle("cyclic widget field ancestry").in_object(id));
            }
            visited.push(id);
        }
        dictionary = document.resolve_dictionary(parent)?;
    }
    Err(Error::limit("widget field ancestry exceeded"))
}
