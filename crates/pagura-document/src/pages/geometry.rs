//! Page boundaries, Table 31 and 14.11.2. Invalid geometry is never used to
//! identify an otherwise untyped leaf.

use crate::source::ReadResult;
use crate::{Document, repairs};
use pagura_core::{Diagnostic, Diagnostics, ObjectId, Rect, Value};

/// US Letter, the viewing convention for a page without a usable MediaBox.
const LETTER: Rect = if let Some(rect) = Rect::from_corners(0.0, 0.0, 612.0, 792.0) {
    rect
} else {
    // Evaluated at compile time: finite corners always form a rectangle.
    unreachable!()
};

/// The Letter fallback is a viewing convention also used by PDF.js, not an
/// ISO default. Prefer a valid CropBox when the required MediaBox is unusable.
/// Resolution errors (including limits and damaged object streams) propagate.
pub(crate) fn boxes(
    document: &Document,
    id: ObjectId,
    media: Option<&Value>,
    crop: Option<&Value>,
    diagnostics: &mut Diagnostics,
) -> ReadResult<(Rect, Rect)> {
    let media = rectangle(document, media)?;
    let crop_rectangle = rectangle(document, crop)?;
    let media = if let Some(media) = media {
        media
    } else {
        diagnostics.repair(
            Diagnostic::new(
                repairs::PAGE_MEDIA_BOX,
                "missing or invalid MediaBox replaced by valid CropBox or Letter viewing \
                 convention",
            )
            .in_object(id),
        )?;
        crop_rectangle.unwrap_or(LETTER)
    };
    let crop = if let Some(crop) = crop_rectangle.and_then(|crop| crop.intersection(&media)) {
        crop
    } else {
        // An absent or null optional entry has the standard default.
        if crop.is_some_and(|value| !matches!(value, Value::Null)) {
            let resolved = crop.map(|value| document.resolve(value)).transpose()?;
            if !matches!(resolved, Some(Value::Null)) {
                diagnostics.repair(
                    Diagnostic::new(
                        repairs::PAGE_CROP_BOX,
                        "invalid or disjoint CropBox replaced by MediaBox",
                    )
                    .in_object(id),
                )?;
            }
        }
        media
    };
    Ok((media, crop))
}

/// A usable rectangle, or `None` when the entry is absent or malformed and
/// the caller repairs it; resolution errors propagate.
fn rectangle(document: &Document, value: Option<&Value>) -> ReadResult<Option<Rect>> {
    let Some(value) = value else { return Ok(None) };
    let Some(values @ [_, _, _, _]) = document.resolve(value)?.as_array() else {
        return Ok(None);
    };
    let mut corners = [0.0; 4];
    for (target, value) in corners.iter_mut().zip(values) {
        let Some(number) = document.resolve(value)?.as_number() else {
            return Ok(None);
        };
        *target = number;
    }
    let [x0, y0, x1, y1] = corners;
    Ok(Rect::from_corners(x0, y0, x1, y1).filter(|rect| !rect.is_empty()))
}
