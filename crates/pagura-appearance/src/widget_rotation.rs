//! Counterclockwise widget orientation from MK R (Table 192).

use crate::stream::form_dictionary;
use crate::values::optional;
use pagura_core::{Dictionary, Error, Name, Rect, Result, Rotation, Value};
use pagura_document::Document;

pub(crate) struct Frame {
    pub(crate) bounds: Rect,
    pub(crate) matrix: Option<[f64; 6]>,
}

pub(crate) fn read(document: &Document, mk: Option<&Dictionary>) -> Result<Rotation> {
    let value = mk
        .map(|mk| optional(document, mk, b"R"))
        .transpose()?
        .flatten();
    let degrees = value
        .map(|value| {
            value
                .as_integer()
                .ok_or(Error::syntax("MK R must be an integer"))
        })
        .transpose()?
        .unwrap_or(0);
    // The shared Rotation type is clockwise; MK R is counterclockwise.
    // Normalize before negating so even i64::MIN stays representable.
    Rotation::try_from(-degrees.rem_euclid(360))
        .map_err(|_| Error::syntax("MK R must be a multiple of 90"))
}

pub(crate) fn frame(rotation: Rotation, rect: Rect) -> Result<Frame> {
    let (width, height) = (rect.width(), rect.height());
    if !width.is_finite() || !height.is_finite() {
        return Err(Error::limit("widget rotation exceeds numeric range"));
    }
    let (width, height) = if matches!(rotation, Rotation::Clockwise90 | Rotation::Clockwise270) {
        (height, width)
    } else {
        (width, height)
    };
    let bounds = if rotation == Rotation::None {
        rect
    } else {
        Rect::from_corners(0.0, 0.0, width, height)
            .ok_or(Error::limit("invalid rotated widget bounds"))?
    };
    let matrix = match rotation {
        Rotation::None => None,
        Rotation::Clockwise270 => Some([0.0, 1.0, -1.0, 0.0, rect.x1(), rect.y0()]),
        Rotation::Clockwise180 => Some([-1.0, 0.0, 0.0, -1.0, rect.x1(), rect.y1()]),
        Rotation::Clockwise90 => Some([0.0, -1.0, 1.0, 0.0, rect.x0(), rect.y1()]),
    };
    Ok(Frame { bounds, matrix })
}

impl Frame {
    pub(crate) fn dictionary(&self) -> Dictionary {
        let mut dictionary = form_dictionary(self.bounds);
        if let Some(matrix) = self.matrix {
            dictionary.insert(
                Name::new(b"Matrix"),
                Value::Array(matrix.map(Value::Real).to_vec()),
            );
        }
        dictionary
    }
}
