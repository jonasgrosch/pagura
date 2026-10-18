//! Empty variable-text and unsigned signature widgets (12.5.6.19, 12.7.5).
//! Values and field types inherit through Parent; appearance characteristics do
//! not. Nonempty values require text layout or a supplied appearance stream.

use super::annotation_shapes::{annotation_color, border, rect_path};
use super::{Compiler, Dictionary, Error, Mark, Rect, Result, Transform, Value};
use pagura_raster::{FillRule, PathBuilder, Point, Stroke};

impl Compiler<'_> {
    pub(super) fn annotation_widget(&mut self, d: &Dictionary, rect: Rect) -> Result<()> {
        let unsupported = || {
            Error::unsupported(
                crate::features::ANNOTATION_APPEARANCE,
                "widget value requires an explicit appearance or text layout",
            )
        };
        let kind = inherited(self.document, d, b"FT")?
            .and_then(Value::as_name)
            .ok_or(Error::syntax("widget has no field type"))?;
        let value = inherited(self.document, d, b"V")?;
        let empty =
            value.is_none_or(|v| v.is_null() || v.as_string().is_some_and(<[u8]>::is_empty));
        if !matches!(kind.as_bytes(), b"Tx" | b"Sig") || !empty {
            return Err(unsupported());
        }
        if let Some(rich) = inherited(self.document, d, b"RV")?
            && !rich.is_null()
            && rich.as_string() != Some(b"")
        {
            return Err(unsupported());
        }
        let Some(mk) = d.get(b"MK") else {
            return Ok(());
        };
        let mk = self.document.resolve_dictionary(mk)?;
        let rotation = mk
            .get(b"R")
            .map(|v| {
                self.document
                    .resolve(v)?
                    .as_integer()
                    .filter(|n| n % 90 == 0)
                    .ok_or(Error::syntax("widget rotation must be a multiple of 90"))
            })
            .transpose()?
            .unwrap_or(0)
            .rem_euclid(360);
        // Table 189: absent BG/BC means no background/border. DA's colours
        // apply to variable text, not to a widget rectangle.
        if let Some(color) = annotation_color(self.document, mk, b"BG", None)? {
            self.annotation_mark(
                d,
                rect_path(rect),
                Mark::Fill(FillRule::NonZero),
                color,
                rect,
            )?;
        }
        let Some(color) = annotation_color(self.document, mk, b"BC", None)? else {
            return Ok(());
        };
        let border = border(self.document, d)?;
        if border.width == 0.0 {
            return Ok(());
        }
        if rotation != 0 {
            return Err(Error::unsupported(
                crate::features::ANNOTATION_APPEARANCE,
                "rotated widget borders require an explicit appearance",
            ));
        }
        let inset = (border.width * 0.5)
            .min(rect.width() * 0.5)
            .min(rect.height() * 0.5);
        let mut path = PathBuilder::new();
        if border.underline {
            path.move_to(Point::new(rect.x0() + inset, rect.y0() + inset));
            path.line_to(Point::new(rect.x1() - inset, rect.y0() + inset));
        } else {
            path.rect(
                rect.x0() + inset,
                rect.y0() + inset,
                (rect.width() - 2.0 * inset).max(0.0),
                (rect.height() - 2.0 * inset).max(0.0),
            );
        }
        self.annotation_mark(
            d,
            path.finish(),
            Mark::Stroke(
                Stroke {
                    width: border.width,
                    dash_array: border.dash,
                    ..Stroke::default()
                },
                Transform::IDENTITY,
                false,
            ),
            color,
            rect,
        )
    }
}

fn inherited<'a>(
    document: &'a pagura_document::Document,
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
