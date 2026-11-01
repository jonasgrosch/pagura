//! Source text, choice, button and unsigned signature widgets (12.5.6.19, 12.7.5).
//! AcroForm values use the document's requested-field view and shared form
//! generator. Empty text/signature widgets retain the lazy geometric fallback
//! after the shared AcroForm membership proof; inconsistent membership in a present AcroForm is an error.
//! Appearance characteristics belong to the widget itself.

use super::border::Border;
use super::marks::{Marks, Pen, Subpath, color};
use crate::features;
use crate::fonts::FontSource;
use crate::stream::AppearanceStream;
use crate::widget_rotation;
use pagura_core::{Dictionary, Error, Meter, Rect, Result, Value};
use pagura_document::Document;

/// Source Tx/Ch/Btn content, or empty-widget geometry after AcroForm membership checks.
pub(super) fn generate(
    document: &Document,
    annotation: &Dictionary,
    rect: Rect,
    fonts: &dyn FontSource,
) -> Result<Option<AppearanceStream>> {
    let unsupported = || {
        Error::unsupported(
            features::ANNOTATION_APPEARANCE,
            "widget value requires an explicit appearance or text layout",
        )
    };
    let field = source_dictionary(document, annotation)?;
    let kind = inherited(document, field, b"FT")?
        .and_then(Value::as_name)
        .ok_or(Error::syntax("widget has no field type"))?;
    let value = inherited(document, field, b"V")?;
    let empty = |value: &Value| {
        value.is_null()
            || value.as_string().is_some_and(|bytes| {
                // A text string may encode zero characters after its UTF-16BE
                // or PDF 2.0 UTF-8 BOM (7.9.2.2). Recognize only those exact
                // empty encodings; malformed or nonempty data stays selected.
                // Sig V is a dictionary, so keep its legacy raw-empty boundary.
                bytes.is_empty()
                    || kind == b"Tx" && matches!(bytes, [0xfe, 0xff] | [0xef, 0xbb, 0xbf])
            })
    };
    let geometry = matches!(kind.as_bytes(), b"Tx" | b"Sig")
        && value.is_none_or(empty)
        && inherited(document, field, b"RV")?.is_none_or(empty);
    if !geometry {
        if let Some(field) = document.widget_field(annotation)? {
            if rect.width() == 0.0 || rect.height() == 0.0 {
                return Ok(None);
            }
            return crate::form::source_widget(document, annotation, rect, &field, fonts).map(Some);
        }
        return Err(unsupported());
    }
    // Text is not selected for an empty widget. Keep the old MK-only display
    // independent of XFA, DA/font parsing and edit-fit rules, but retain the
    // same bounded ancestry/membership proof as nonempty source widgets.
    document.widget_field_membership(annotation)?;
    let mut marks = Marks::new(document, annotation);
    if let Some(mk) = annotation.get(b"MK") {
        let mk = document.resolve_dictionary(mk)?;
        characteristics(&mut marks, document, annotation, mk, rect)?;
    }
    marks.finish(rect)
}

/// An unnamed separate widget does not shadow its owning field's V or FT.
fn source_dictionary<'a>(
    document: &'a Document,
    annotation: &'a Dictionary,
) -> Result<&'a Dictionary> {
    if document
        .optional(document.catalog()?, b"AcroForm")?
        .is_some()
        && document.optional(annotation, b"T")?.is_none()
        && let Some(parent) = document.optional(annotation, b"Parent")?
    {
        return document.resolve_dictionary(parent);
    }
    Ok(annotation)
}

/// Table 192: absent BG and BC mean no background and no border. DA's
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
    let limits = document.limits();
    let mut storage = Meter::new(
        limits.document_bytes,
        "widget ancestry storage budget exceeded",
    );
    let mut work = Meter::new(
        limits.traversal_work,
        "widget ancestry work budget exceeded",
    );
    let mut visited = Vec::new();
    for _ in 0..limits.depth {
        work.charge(1)?;
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
            work.charge(visited.len())?;
            if visited.contains(&id) {
                return Err(Error::cycle("cyclic widget field ancestry").in_object(id));
            }
            storage.reserve_capacity(&mut visited, 1)?;
            visited.push(id);
        }
        dictionary = document.resolve_dictionary(parent)?;
    }
    Err(Error::limit("widget field ancestry exceeded"))
}
