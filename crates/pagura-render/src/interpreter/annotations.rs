//! Normal annotation appearances and screen visibility (12.5.2–12.5.5).

use super::{
    Compiler, Dictionary, Error, Graphics, Page, Point, Rect, Result, Transform, Value, matrix,
};

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
        for value in values {
            self.annotation(value, page.resources()).map_err(|error| {
                value
                    .as_reference()
                    .map_or(error.clone(), |id| error.in_object(id))
            })?;
        }
        Ok(())
    }

    fn annotation(&mut self, value: &Value, resources: &Dictionary) -> Result<()> {
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
            return Ok(());
        }
        if let Some(oc) = d.get(b"OC")
            && !self.visibility.visible(self.document, oc)?
        {
            return Ok(());
        }
        let subtype = d
            .get(b"Subtype")
            .map(|v| self.document.resolve(v))
            .transpose()?
            .and_then(Value::as_name)
            .ok_or(Error::syntax("annotation has no Subtype"))?;
        if flags & 1 != 0 && !standard_type(subtype.as_bytes()) {
            return Ok(());
        }
        let rect = rectangle(self.document, d, b"Rect")?;
        if rect.width() == 0.0 && rect.height() == 0.0 {
            return Ok(());
        }
        let Some(ap) = d.get(b"AP") else {
            if matches!(subtype.as_bytes(), b"Popup" | b"Projection")
                || (subtype == b"Link" && no_border(self.document, d)?)
            {
                return Ok(());
            }
            return Err(Error::unsupported(
                crate::features::ANNOTATION_APPEARANCE,
                "annotation appearance generation is not implemented",
            ));
        };
        let Some(id) = normal_appearance(self.document, d, ap)? else {
            return Ok(());
        };
        if flags & (8 | 16) != 0 {
            return Err(Error::unsupported(
                crate::features::ANNOTATION_FLAGS,
                "NoZoom and NoRotate annotation fitting is not implemented",
            ));
        }
        self.paint_appearance(id, rect, resources)
            .map_err(|error| error.in_object(id))
    }

    fn paint_appearance(
        &mut self,
        id: pagura_core::ObjectId,
        rect: Rect,
        resources: &Dictionary,
    ) -> Result<()> {
        let object = self.document.object(id)?;
        let form = object
            .dictionary()
            .filter(|_| object.is_stream())
            .ok_or(Error::syntax("appearance must be a stream"))?;
        let bbox = rectangle(self.document, form, b"BBox")?;
        let transform = form
            .get(b"Matrix")
            .map(|v| {
                let v = self
                    .document
                    .resolve(v)?
                    .as_array()
                    .ok_or(Error::syntax("appearance Matrix must be an array"))?;
                matrix(v)
            })
            .transpose()?
            .unwrap_or(Transform::IDENTITY);
        let placement = placement(bbox, transform, rect)?;
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
        let result = self.form_object(id, resources);
        if result.is_ok() && !form.contains_key(b"Group") {
            let mut bounds = pagura_raster::PathBuilder::new();
            bounds.rect(rect.x0, rect.y0, rect.width(), rect.height());
            let commands = self.list.commands.split_off(start);
            self.list
                .commands
                .push(crate::display::Command::Group(crate::transparency::Group {
                    position: pagura_core::Position::Unknown,
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

fn rectangle(document: &pagura_document::Document, d: &Dictionary, key: &[u8]) -> Result<Rect> {
    d.get(key)
        .map(|v| document.resolve(v))
        .transpose()?
        .and_then(Value::as_array)
        .and_then(Rect::from_array)
        .ok_or(Error::syntax("invalid annotation or appearance rectangle"))
}

/// Fit the upright envelope of all four transformed corners (12.5.5).
fn placement(b: Rect, m: Transform, r: Rect) -> Result<Transform> {
    let points = [
        Point::new(b.x0, b.y0),
        Point::new(b.x1, b.y0),
        Point::new(b.x0, b.y1),
        Point::new(b.x1, b.y1),
    ]
    .map(|p| p * m);
    let x0 = points.iter().map(|p| p.x).fold(f64::INFINITY, f64::min);
    let x1 = points.iter().map(|p| p.x).fold(f64::NEG_INFINITY, f64::max);
    let y0 = points.iter().map(|p| p.y).fold(f64::INFINITY, f64::min);
    let y1 = points.iter().map(|p| p.y).fold(f64::NEG_INFINITY, f64::max);
    if x1 <= x0 || y1 <= y0 {
        return Err(Error::syntax("degenerate appearance bounds"));
    }
    let sx = r.width() / (x1 - x0);
    let sy = r.height() / (y1 - y0);
    let a = Transform::new(sx, 0.0, 0.0, sy, r.x0 - sx * x0, r.y0 - sy * y0);
    if !a.is_finite() {
        return Err(Error::limit("appearance fitting exceeds numeric range"));
    }
    Ok(a)
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
