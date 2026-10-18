//! Normal annotation appearances and screen visibility (12.5.2–12.5.5).

mod shapes;
mod widgets;

use super::{
    Compiler, Dictionary, Error, Graphics, Page, Point, Rect, Result, Transform, Value,
    forms::FormBody,
};
use crate::form_appearance::Appearance;

impl Compiler<'_> {
    pub(super) fn annotations(&mut self, page: &Page) -> Result<()> {
        let Some(values) = page.dictionary().get(b"Annots") else {
            return Ok(());
        };
        let values = self
            .document
            .resolve(values)?
            .as_array()
            .ok_or(Error::syntax("Annots must be an array"))?;
        if values.len() > self.document.limits().value_nodes {
            return Err(Error::limit("annotation count exceeded"));
        }
        let compilation = std::rc::Rc::clone(&self.compilation);
        let filled = compilation
            .filler
            .map(super::annotations_filled::FilledAnnotations::new)
            .transpose()?;
        for value in values {
            self.annotation(value, page.resources(), filled.as_ref())
                .map_err(|error| {
                    if let Some(id) = value.as_reference()
                        && error.object.is_none()
                    {
                        error.in_object(id)
                    } else {
                        error
                    }
                })?;
        }
        Ok(())
    }

    fn annotation(
        &mut self,
        value: &Value,
        resources: &Dictionary,
        filled: Option<&super::annotations_filled::FilledAnnotations<'_, '_>>,
    ) -> Result<()> {
        let d = self.document.resolve_dictionary(value)?;
        let flags = d
            .get(b"F")
            .map(|v| {
                self.document
                    .resolve(v)?
                    .as_integer()
                    .ok_or(Error::syntax("annotation F must be integer"))
            })
            .transpose()?
            .unwrap_or(0);
        if flags < 0 || flags & !1023 != 0 {
            return Err(Error::syntax("invalid annotation flags"));
        }
        if flags & (2 | 32) != 0 {
            self.extract_annotation(value, true)?;
            return Ok(());
        }
        if let Some(oc) = d.get(b"OC")
            && !self.visibility.visible(self.document, oc)?
        {
            self.extract_annotation(value, true)?;
            return Ok(());
        }
        let subtype = d
            .get(b"Subtype")
            .map(|v| self.document.resolve(v))
            .transpose()?
            .and_then(Value::as_name)
            .ok_or(Error::syntax("annotation has no Subtype"))?;
        if flags & 1 != 0 && !standard_type(subtype.as_bytes()) {
            self.extract_annotation(value, true)?;
            return Ok(());
        }
        self.extract_annotation(value, false)?;
        let rect = rectangle(self.document, d, b"Rect")?;
        if subtype == b"Widget"
            && let Some(appearance) = filled
                .map(|f| f.prepare(d, self, rect))
                .transpose()?
                .flatten()
        {
            let start = self.list.commands.len();
            self.paint_filled_appearance(&appearance, rect, resources)?;
            self.annotation_view(start, rect, flags);
            return Ok(());
        }
        let Some(ap) = d.get(b"AP") else {
            if rect.width() == 0.0 && rect.height() == 0.0 {
                return Ok(());
            }
            if matches!(subtype.as_bytes(), b"Popup" | b"Projection")
                || (subtype == b"Link" && no_border(self.document, d)?)
            {
                return Ok(());
            }
            return self.generated_annotation(d, subtype.as_bytes(), rect, flags);
        };
        let Some(id) = normal_appearance(self.document, d, ap)? else {
            return Ok(());
        };
        let start = self.list.commands.len();
        self.paint_filled_appearance(&Appearance::Existing(id), rect, resources)?;
        let flags = if subtype == b"Text" {
            flags | 8 | 16
        } else {
            flags
        };
        self.annotation_view(start, rect, flags);
        Ok(())
    }

    pub(super) fn annotation_view(&mut self, start: usize, rect: Rect, flags: i64) {
        if flags & (8 | 16) == 0 {
            return;
        }
        let commands = self.list.commands.split_off(start);
        let mut bounds = pagura_raster::PathBuilder::new();
        bounds.rect(rect.x0(), rect.y0(), rect.width(), rect.height());
        self.list
            .commands
            .push(crate::display::Command::Group(crate::transparency::Group {
                position: self.position,
                bounds: Some(std::sync::Arc::new(bounds.finish())),
                commands,
                attributes: crate::transparency::Attributes {
                    view: Some(crate::transparency::ViewFit {
                        anchor: Point::new(rect.x0(), rect.y1()),
                        no_zoom: flags & 8 != 0,
                        no_rotate: flags & 16 != 0,
                    }),
                    ..Default::default()
                },
                composite: crate::transparency::Composite::opaque(),
            }));
    }

    fn paint_filled_appearance(
        &mut self,
        appearance: &Appearance,
        rect: Rect,
        resources: &Dictionary,
    ) -> Result<()> {
        let body = match appearance {
            Appearance::Existing(id) => FormBody::Stored(self.document.object(*id)?),
            Appearance::Generated(stream) => FormBody::Generated {
                dictionary: stream.dictionary(),
                bytes: stream.content(),
            },
            Appearance::Hidden => return Ok(()),
        };
        self.paint_appearance(body, rect, resources)
            .map_err(|e| match body.id() {
                Some(id) => e.in_object(id),
                None => e,
            })
    }

    fn paint_appearance(
        &mut self,
        body: FormBody<'_>,
        rect: Rect,
        resources: &Dictionary,
    ) -> Result<()> {
        let form = body.dictionary()?;
        let (bbox, transform) = self.form_geometry(form)?;
        let Some(placement) = placement(bbox, transform, rect)? else {
            // An indirect appearance names its own stream; an inline one only
            // the annotation's position.
            let position = body
                .id()
                .map_or(self.position, |id| pagura_core::Position::Stream {
                    object: id,
                    offset: 0,
                });
            let diagnostic = pagura_core::Diagnostic::new(
                crate::repairs::APPEARANCE_EMPTY,
                "zero-sized appearance or annotation rectangle; appearance skipped",
            )
            .in_object_opt(body.id())
            .at(position);
            return super::repair(self.document, diagnostic);
        };
        // Annotation appearances start independently of the page's final state.
        // Table 166 requires ignoring annotation styling keys for an explicit AP.
        let saved = std::mem::replace(
            &mut self.state,
            Graphics {
                ctm: placement,
                ..Graphics::default()
            },
        );
        let start = self.list.commands.len();
        let extraction = self.extraction.take();
        let result = self.form_body(body, resources);
        self.extraction = extraction;
        if result.is_ok() && !form.contains_key(b"Group") {
            let mut bounds = pagura_raster::PathBuilder::new();
            bounds.rect(rect.x0(), rect.y0(), rect.width(), rect.height());
            let commands = self.list.commands.split_off(start);
            self.list
                .commands
                .push(crate::display::Command::Group(crate::transparency::Group {
                    position: self.position,
                    commands,
                    bounds: Some(std::sync::Arc::new(bounds.finish())),
                    attributes: crate::transparency::Attributes::default(),
                    composite: crate::transparency::Composite::opaque(),
                }));
        }
        self.state = saved;
        result
    }
}

fn normal_appearance(
    document: &pagura_document::Document,
    d: &Dictionary,
    ap: &Value,
) -> Result<Option<pagura_core::ObjectId>> {
    let ap = document.resolve_dictionary(ap)?;
    let normal = ap
        .get(b"N")
        .ok_or(Error::syntax("appearance dictionary has no N"))?;
    let stream = if let Some(id) = normal.as_reference() {
        document.object(id)?.is_stream()
    } else {
        false
    };
    let normal = if stream {
        normal
    } else {
        let states = document.resolve_dictionary(normal)?;
        let state = d
            .get(b"AS")
            .map(|v| document.resolve(v))
            .transpose()?
            .and_then(Value::as_name)
            .ok_or(Error::syntax("annotation has no appearance state"))?;
        // 12.5.5 permits displaying nothing for an undefined state.
        let Some(normal) = states.get(state.as_bytes()) else {
            return Ok(None);
        };
        normal
    };
    normal
        .as_reference()
        .map(Some)
        .ok_or(Error::syntax("appearance must be a stream reference"))
}

fn no_border(document: &pagura_document::Document, d: &Dictionary) -> Result<bool> {
    if let Some(color) = d.get(b"C")
        && document
            .resolve(color)?
            .as_array()
            .is_some_and(<[Value]>::is_empty)
    {
        return Ok(true);
    }
    if let Some(style) = d.get(b"BS") {
        let style = document.resolve_dictionary(style)?;
        return style
            .get(b"W")
            .map(|v| {
                document
                    .resolve(v)?
                    .as_number()
                    .ok_or(Error::syntax("border width must be numeric"))
            })
            .transpose()
            .map(|v| v == Some(0.0));
    }
    if let Some(border) = d.get(b"Border") {
        let values = document
            .resolve(border)?
            .as_array()
            .filter(|v| matches!(v.len(), 3 | 4))
            .ok_or(Error::syntax("invalid link Border"))?;
        let width = values.get(2).ok_or(Error::syntax("missing border width"))?;
        return Ok(document.resolve(width)?.as_number() == Some(0.0));
    }
    Ok(false)
}

pub(super) fn rectangle(
    document: &pagura_document::Document,
    d: &Dictionary,
    key: &[u8],
) -> Result<Rect> {
    d.get(key)
        .map(|v| document.resolve(v))
        .transpose()?
        .and_then(Value::as_array)
        .and_then(Rect::from_array)
        .ok_or(Error::syntax("invalid annotation or appearance rectangle"))
}

/// Fit the upright envelope of all four transformed corners (12.5.5).
pub(super) fn placement(b: Rect, m: Transform, r: Rect) -> Result<Option<Transform>> {
    let points = [
        Point::new(b.x0(), b.y0()),
        Point::new(b.x1(), b.y0()),
        Point::new(b.x0(), b.y1()),
        Point::new(b.x1(), b.y1()),
    ]
    .map(|p| p * m);
    let x0 = points.iter().map(|p| p.x).fold(f64::INFINITY, f64::min);
    let x1 = points.iter().map(|p| p.x).fold(f64::NEG_INFINITY, f64::max);
    let y0 = points.iter().map(|p| p.y).fold(f64::INFINITY, f64::min);
    let y1 = points.iter().map(|p| p.y).fold(f64::NEG_INFINITY, f64::max);
    if x1 <= x0 || y1 <= y0 {
        if b.width() == 0.0 || b.height() == 0.0 {
            return Ok(None);
        }
        return Err(Error::syntax("degenerate appearance bounds"));
    }
    if r.width() == 0.0 || r.height() == 0.0 {
        return Ok(None);
    }
    let sx = r.width() / (x1 - x0);
    let sy = r.height() / (y1 - y0);
    let a = Transform::new(sx, 0.0, 0.0, sy, r.x0() - sx * x0, r.y0() - sy * y0);
    if !a.is_finite() {
        return Err(Error::limit("appearance fitting exceeds numeric range"));
    }
    Ok(Some(a))
}

fn standard_type(name: &[u8]) -> bool {
    matches!(
        name,
        b"Text"
            | b"Link"
            | b"FreeText"
            | b"Line"
            | b"Square"
            | b"Circle"
            | b"Polygon"
            | b"PolyLine"
            | b"Highlight"
            | b"Underline"
            | b"Squiggly"
            | b"StrikeOut"
            | b"Caret"
            | b"Stamp"
            | b"Ink"
            | b"Popup"
            | b"FileAttachment"
            | b"Sound"
            | b"Movie"
            | b"Widget"
            | b"Screen"
            | b"PrinterMark"
            | b"TrapNet"
            | b"Watermark"
            | b"3D"
            | b"Redact"
            | b"Projection"
            | b"RichMedia"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{RenderOptions, compile_page};
    use pagura_core::{ErrorKind, ObjectId, Position, Recovery};
    use pagura_document::{Document, OpenOptions};
    use pagura_testkit::fixture;

    #[test]
    fn empty_appearance_fitting_reports_repairs_without_entering_the_stream() {
        for suffix in [
            "bbox-width",
            "bbox-height",
            "bbox-point",
            "bbox-rotated",
            "indirect",
            "rect-width",
            "rect-height",
            "rect-point",
        ] {
            let name = format!("annotation-empty-{suffix}");
            let object = ObjectId::new(5, 0);
            let position = Position::Stream { object, offset: 0 };
            let strict = Document::open(
                fixture("paint", &format!("{name}.pdf")),
                &OpenOptions {
                    recovery: Recovery::Strict,
                    ..Default::default()
                },
            )
            .unwrap();
            let error = compile_page(&strict, strict.page(0).unwrap()).unwrap_err();
            assert_eq!(error.kind, ErrorKind::Syntax);
            assert_eq!(error.repair, Some(crate::repairs::APPEARANCE_EMPTY));
            assert_eq!(error.object, Some(object));
            assert_eq!(error.position, position);
            assert_eq!(strict.diagnostics(), []);
            let repaired = Document::open(
                fixture("paint", &format!("{name}.pdf")),
                &OpenOptions::default(),
            )
            .unwrap();
            let page = repaired.page(0).unwrap();
            let image = compile_page(&repaired, page)
                .unwrap()
                .render(&RenderOptions::default())
                .unwrap();
            assert_eq!(
                pagura_testkit::ppm(
                    image.width(),
                    image.height(),
                    &image.to_rgb_over(pagura_raster::Rgba8::WHITE)
                ),
                fixture("paint", &format!("{name}.rgb")),
                "{name}"
            );
            let diagnostics = repaired.diagnostics();
            assert_eq!(diagnostics.len(), 1);
            assert_eq!(
                diagnostics[0].repair(),
                Some(crate::repairs::APPEARANCE_EMPTY)
            );
            assert_eq!(diagnostics[0].object, Some(object));
            assert_eq!(diagnostics[0].position, position);
            compile_page(&repaired, page).unwrap();
            assert_eq!(repaired.diagnostics(), diagnostics);
        }
    }

    #[test]
    fn fitting_uses_the_transformed_envelope_and_preserves_other_errors() {
        let zero_width = Rect::from_corners(0.0, 0.0, 0.0, 4.0).unwrap();
        let target = Rect::from_corners(2.0, 2.0, 10.0, 14.0).unwrap();
        assert!(
            placement(zero_width, Transform::IDENTITY, target)
                .unwrap()
                .is_none()
        );
        // An oblique line has a nonzero upright envelope and can still be fit.
        assert!(
            placement(
                zero_width,
                Transform::new(1.0, 1.0, -1.0, 1.0, 0.0, 0.0),
                target
            )
            .unwrap()
            .is_some()
        );
        let bbox = Rect::from_corners(0.0, 0.0, 2.0, 4.0).unwrap();
        let error =
            placement(bbox, Transform::new(0.0, 0.0, 0.0, 1.0, 0.0, 0.0), target).unwrap_err();
        assert_eq!(error.kind, ErrorKind::Syntax);
        let error = placement(
            Rect::from_corners(0.0, 0.0, 0.5, 0.5).unwrap(),
            Transform::IDENTITY,
            Rect::from_corners(0.0, 0.0, f64::MAX, f64::MAX).unwrap(),
        )
        .unwrap_err();
        assert_eq!(error.kind, ErrorKind::Limit);
    }
}
