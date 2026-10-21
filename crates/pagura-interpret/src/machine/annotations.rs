//! Annotations (12.5): visibility flags and optional content, normal
//! appearances and appearance states, appearances generated for
//! annotations without one and for filled forms (`pagura-appearance`), and
//! the fixed views of `NoZoom` and `NoRotate`.

use super::emit::GroupStart;
use super::form::{FormBody, geometry};
use super::state::Graphics;
use super::{Machine, Scope, Text};
use crate::device::{Device, Marked};
use crate::model::{GroupAttributes, GroupCause, ViewFit};
use crate::repairs;
use pagura_appearance::{Appearance, AppearanceStream, FilledWidgets, FontSource};
use pagura_core::{Diagnostic, Dictionary, Error, ObjectId, Position, Rect, Result, Value};
use pagura_document::{Document, Page};
use pagura_raster::{Path, Point, Transform};
use pagura_text::{PdfFont, Provider};
use std::sync::Arc;

/// `NoZoom` (12.5.3, Table 167).
const NO_ZOOM: i64 = 8;
/// `NoRotate`.
const NO_ROTATE: i64 = 16;

/// What an annotation draws.
enum Drawn {
    Stored(ObjectId),
    Generated(AppearanceStream),
    Nothing,
}

impl<D: Device + ?Sized> Machine<'_, '_, D> {
    /// The annotations of `page`, in order.
    pub(super) fn annotations(&mut self, page: &Page) -> Result<()> {
        let document = self.document();
        let Some(values) = page.dictionary().get(b"Annots") else {
            return Ok(());
        };
        let values = document
            .resolve(values)?
            .as_array()
            .ok_or(Error::syntax("Annots must be an array"))?;
        if values.len() > self.session.limits.traversal_work {
            return Err(Error::limit("annotation count exceeded"));
        }
        let filled = match self.session.filler {
            Some(filler) if self.needs().paint => Some(FilledWidgets::new(filler)?),
            _ => None,
        };
        for value in values {
            self.annotation(value, page.resources(), filled.as_ref())
                .map_err(|error| error.in_object_opt(value.as_reference()))?;
        }
        Ok(())
    }

    fn annotation(
        &mut self,
        value: &Value,
        resources: &Dictionary,
        filled: Option<&FilledWidgets<'_, '_>>,
    ) -> Result<()> {
        let document = self.document();
        let d = document.resolve_dictionary(value)?;
        let flags = d
            .get(b"F")
            .map(|v| {
                document
                    .resolve(v)?
                    .as_integer()
                    .ok_or(Error::syntax("annotation F must be integer"))
            })
            .transpose()?
            .unwrap_or(0);
        if flags < 0 || flags & !1023 != 0 {
            return Err(Error::syntax("invalid annotation flags"));
        }
        // Hidden and NoView (Table 167).
        if flags & (2 | 32) != 0 {
            return self.occurrence(value, true);
        }
        if let Some(oc) = d.get(b"OC")
            && !self.session.visibility.visible(document, oc)?
        {
            return self.occurrence(value, true);
        }
        let subtype = d
            .get(b"Subtype")
            .map(|v| document.resolve(v))
            .transpose()?
            .and_then(Value::as_name)
            .ok_or(Error::syntax("annotation has no Subtype"))?;
        // Invisible: unknown subtypes are not shown.
        if flags & 1 != 0 && !standard_type(subtype.as_bytes()) {
            return self.occurrence(value, true);
        }
        self.occurrence(value, false)?;
        if !self.needs().paint {
            return Ok(());
        }
        let rect = rectangle(document, d, b"Rect")?;
        let text_icon = subtype == b"Text";
        let drawn = if let Some(appearance) = self.filled(filled, d, rect)? {
            match appearance {
                Appearance::Existing(id) => Drawn::Stored(id),
                Appearance::Generated(stream) => Drawn::Generated(stream),
                Appearance::Hidden => Drawn::Nothing,
            }
        } else if let Some(ap) = d.get(b"AP") {
            let Some(id) = normal_appearance(document, d, ap)? else {
                return Ok(());
            };
            Drawn::Stored(id)
        } else {
            match pagura_appearance::annotation_appearance(document, d, rect)? {
                Some(stream) => Drawn::Generated(stream),
                None => return Ok(()),
            }
        };
        // Text annotations keep their icon's size and orientation.
        let flags = if text_icon {
            flags | NO_ZOOM | NO_ROTATE
        } else {
            flags
        };
        self.annotation_view(rect, flags, |machine| match &drawn {
            Drawn::Stored(id) => {
                let object = document.object(*id)?;
                machine
                    .appearance(FormBody::Stored(object), rect, resources, Text::Paint)
                    .map_err(|e| e.in_object(*id))
            }
            Drawn::Generated(stream) => machine.appearance(
                FormBody::Generated {
                    dictionary: stream.dictionary(),
                    bytes: stream.content(),
                },
                rect,
                resources,
                Text::Paint,
            ),
            Drawn::Nothing => Ok(()),
        })
    }

    /// The appearance a form session draws for a widget, if it differs
    /// from the stored one.
    fn filled(
        &mut self,
        filled: Option<&FilledWidgets<'_, '_>>,
        d: &Dictionary,
        rect: Rect,
    ) -> Result<Option<Appearance>> {
        let Some(filled) = filled else {
            return Ok(None);
        };
        if !d.is_name(b"Subtype", b"Widget") {
            return Ok(None);
        }
        let document = self.document();
        let visibility = &self.session.visibility;
        let fonts = SessionFonts(&self.session.fonts);
        filled.appearance(d, rect, &fonts, |stream| {
            let hidden = stream
                .get(b"OC")
                .map(|oc| visibility.visible(document, oc))
                .transpose()?
                .is_some_and(|visible| !visible);
            let (bbox, matrix) = geometry(document, stream)?;
            Ok(hidden || placement(bbox, matrix, rect)?.is_none())
        })
    }

    /// Records an annotation occurrence for structure (`OBJR`).
    fn occurrence(&mut self, value: &Value, hidden: bool) -> Result<()> {
        let Some(object) = value.as_reference() else {
            return Ok(());
        };
        if self.needs().structure && self.needs().text {
            self.device.marked(&Marked::ObjectBegin { object, hidden })?;
            self.device.marked(&Marked::ObjectEnd)?;
        }
        Ok(())
    }

    /// Draws what `draw` emits inside a fixed view when `flags` asks for
    /// one (12.5.3): the group keeps its size or orientation around the
    /// rectangle's upper-left corner.
    fn annotation_view(
        &mut self,
        rect: Rect,
        flags: i64,
        draw: impl FnOnce(&mut Self) -> Result<()>,
    ) -> Result<()> {
        if flags & (NO_ZOOM | NO_ROTATE) == 0 {
            return draw(self);
        }
        let bounds = Arc::new(Path::rect(rect));
        let opaque = Arc::clone(&self.session.opaque);
        let scope = Scope::new(Graphics::default(), Text::Paint);
        self.begin_group(
            &GroupStart {
                cause: GroupCause::Annotation,
                attributes: GroupAttributes::default(),
                bbox: Some(&bounds),
                view: Some(ViewFit {
                    anchor: Point::new(rect.x0(), rect.y1()),
                    no_zoom: flags & NO_ZOOM != 0,
                    no_rotate: flags & NO_ROTATE != 0,
                }),
                composite: &opaque,
            },
            &scope,
        )?;
        draw(self)?;
        self.device.end_group()
    }

    /// Draws an appearance stream fitted into `rect` (12.5.5), starting from
    /// the initial graphics state, inside a group unless the stream has a
    /// group of its own. `role` says whether its text is page text.
    pub(crate) fn appearance(
        &mut self,
        body: FormBody<'_>,
        rect: Rect,
        resources: &Dictionary,
        role: Text,
    ) -> Result<()> {
        let document = self.document();
        let form = body.dictionary()?;
        let (bbox, matrix) = geometry(document, form)?;
        let Some(placement) = placement(bbox, matrix, rect)? else {
            // An indirect appearance names its own stream; an inline one
            // only the annotation's position.
            let position = body
                .id()
                .map_or(self.position, |object| Position::Stream { object, offset: 0 });
            return document.repair(
                Diagnostic::new(
                    repairs::APPEARANCE_EMPTY,
                    "zero-sized appearance or annotation rectangle; appearance skipped",
                )
                .in_object_opt(body.id())
                .at(position),
            );
        };
        let mut state = Graphics::default();
        state.set_ctm(placement);
        if self.needs().paint {
            let gray = self.session.colors.device_gray();
            state.set_both_inks(&gray);
        }
        let mut scope = Scope::new(state, role);
        let grouped = !form.contains_key(b"Group") && self.needs().paint;
        if grouped {
            let bounds = Arc::new(Path::rect(rect));
            let opaque = Arc::clone(&self.session.opaque);
            self.begin_group(
                &GroupStart {
                    cause: GroupCause::Annotation,
                    attributes: GroupAttributes::default(),
                    bbox: Some(&bounds),
                    view: None,
                    composite: &opaque,
                },
                &scope,
            )?;
        }
        self.form(body, resources, &mut scope)?;
        if grouped {
            self.device.end_group()?;
        }
        Ok(())
    }
}

/// The interpreter's fonts, for laying out generated field text.
struct SessionFonts<'s, 'a>(&'s crate::fonts::Fonts<'a>);

impl FontSource for SessionFonts<'_, '_> {
    fn font(&self, _: &Document, id: Option<ObjectId>, dictionary: &Dictionary) -> Result<Arc<PdfFont>> {
        self.0.load(id, dictionary, Position::Unknown)
    }

    fn provider(&self) -> Option<&dyn Provider> {
        self.0.provider()
    }
}

/// The normal appearance stream of an annotation (12.5.5): `N` itself, or
/// its entry for the appearance state `AS`; `None` for an undefined state.
fn normal_appearance(document: &Document, d: &Dictionary, ap: &Value) -> Result<Option<ObjectId>> {
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

/// A rectangle entry.
fn rectangle(document: &Document, d: &Dictionary, key: &[u8]) -> Result<Rect> {
    d.get(key)
        .map(|v| document.resolve(v))
        .transpose()?
        .and_then(Value::as_array)
        .and_then(Rect::from_array)
        .ok_or(Error::syntax("invalid annotation or appearance rectangle"))
}

/// The transform that fits the upright envelope of the transformed `BBox`
/// into the annotation rectangle (12.5.5, Algorithm 8.1); `None` when
/// either has no area.
///
/// # Errors
///
/// `Syntax` for a matrix that collapses a box with area, `Limit` for a fit
/// beyond the numeric range.
pub(crate) fn placement(b: Rect, m: Transform, r: Rect) -> Result<Option<Transform>> {
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
