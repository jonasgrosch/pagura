//! Page boundaries, Table 31 and 14.11.2. Invalid geometry is never used to
//! identify an otherwise untyped leaf.

use crate::{Document, repairs};
use pagura_core::{Diagnostic, Diagnostics, ObjectId, Rect, Result, Value};

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
) -> Result<(Rect, Rect)> {
    let media = rectangle(document, media)?;
    let crop_rectangle = rectangle(document, crop)?;
    let media = if let Some(media) = media {
        media
    } else {
        diagnostics.repair(
                Diagnostic::new(repairs::PAGE_MEDIA_BOX,
                    "missing or invalid MediaBox replaced by valid CropBox or Letter viewing convention")
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

fn rectangle(document: &Document, value: Option<&Value>) -> Result<Option<Rect>> {
    let Some(value) = value else { return Ok(None) };
    let Some(values) = document
        .resolve(value)?
        .as_array()
        .filter(|values| values.len() == 4)
    else {
        return Ok(None);
    };
    // Four scalar resolutions, without cloning arbitrary subtrees.
    let mut elements = [Value::Null, Value::Null, Value::Null, Value::Null];
    for (target, value) in elements.iter_mut().zip(values) {
        let Some(number) = document.resolve(value)?.as_number() else {
            return Ok(None);
        };
        *target = Value::Real(number);
    }
    Ok(Rect::from_array(&elements).filter(|rect| !rect.is_empty()))
}
