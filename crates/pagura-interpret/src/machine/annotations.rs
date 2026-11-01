//! Annotations (12.5): visibility flags and optional content, normal
//! appearances and appearance states, appearances generated for
//! annotations without one and for filled forms (`pagura-appearance`), and
//! the fixed views of `NoZoom` and `NoRotate`.

use super::emit::GroupStart;
use super::form::FormBody;
use super::state::Graphics;
use super::{Machine, Scope, Text};
use crate::device::{Device, Marked, Needs};
use crate::model::{GroupAttributes, GroupCause, ViewFit};
use crate::repairs;
use pagura_appearance::{
    Appearance, FilledWidgets, FontSource, appearance_bounds as envelope,
    appearance_placement as placement, form_geometry as geometry,
};
use pagura_core::{Diagnostic, Dictionary, Error, ObjectId, Position, Rect, Result, Value};
use pagura_document::{Document, Page};
use pagura_raster::{Path, Point, Transform};
use pagura_text::{PdfFont, Provider};
use std::sync::Arc;

/// `NoZoom` (12.5.3, Table 167).
const NO_ZOOM: i64 = 8;
/// `NoRotate`.
const NO_ROTATE: i64 = 16;

impl<D: Device + ?Sized> Machine<'_, '_, D> {
    /// The annotations of `page`, in order.
    pub(super) fn annotations(&mut self, page: &Page) -> Result<()> {
        let document = self.document();
        let Some(values) = page.dictionary().get(b"Annots") else {
            return Ok(());
        };
        let values = document.resolve(values)?;
        // A null dictionary entry is equivalent to absence (7.3.9), including
        // an indirect null. Do not turn a page with no annotations into an error.
        if matches!(values, Value::Null) {
            return Ok(());
        }
        let values = values
            .as_array()
            .ok_or(Error::syntax("Annots must be an array"))?;
        if values.len() > self.session.limits.traversal_work {
            return Err(Error::limit("annotation count exceeded"));
        }
        let filled = match self.session.filler {
            Some(filler) if draws(self.needs()) => Some(FilledWidgets::new(filler)?),
            _ => None,
        };
        for value in values {
            let Some(value) = document.annotation_entry(value, page.id())? else {
                continue;
            };
            self.annotation(value, page, filled.as_ref())
                .map_err(|error| error.in_object_opt(value.as_reference()))?;
        }
        Ok(())
    }

    fn annotation(
        &mut self,
        value: &Value,
        page: &Page,
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
        if !draws(self.needs()) {
            return Ok(());
        }
        if skip_inert_popup(document, d, subtype.as_bytes(), value.as_reference())? {
            return Ok(());
        }
        let rect = rectangle(document, d, b"Rect")?;
        let (rect, flags) = if subtype == b"Watermark"
            && let Some(rect) = fixed_print(document, d, rect, page)?
        {
            // The existing fixed view cancels page rotation about the media
            // anchor. Display zoom and UserUnit retain their usual meaning.
            (rect, flags | NO_ROTATE)
        } else {
            (rect, flags)
        };
        let resources = page.resources();
        let text_icon = subtype == b"Text";
        let appearance = match self.filled(filled, d, rect)? {
            Some(appearance) => Some(appearance),
            None => pagura_appearance::stored_appearance(
                document,
                d,
                pagura_appearance::AppearanceMode::Normal,
                None,
            )?,
        };
        let drawn = if let Some(appearance) = appearance {
            appearance
        } else {
            let fonts = SessionFonts(&self.session.fonts);
            match pagura_appearance::annotation_appearance_with_fonts(document, d, rect, &fonts)? {
                Some(stream) => Appearance::Generated(stream),
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
            Appearance::Existing(id) => {
                let object = document.object(*id)?;
                machine
                    .appearance(FormBody::Stored(object), rect, resources, Text::Paint)
                    .map_err(|e| e.in_object(*id))
            }
            Appearance::Generated(stream) => machine.appearance(
                FormBody::Generated {
                    dictionary: stream.dictionary(),
                    bytes: stream.content(),
                },
                rect,
                resources,
                Text::Paint,
            ),
            Appearance::Hidden => Ok(()),
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
        if self.needs().structure {
            self.device
                .marked(&Marked::ObjectBegin { object, hidden })?;
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
            let position = body.id().map_or(self.position, |object| Position::Stream {
                object,
                offset: 0,
            });
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

/// Whether a device receives annotation appearances: they are drawn
/// content (12.5.5), not page text, so devices that need only text and
/// structure skip them; occurrences reach structure devices either way.
fn draws(needs: Needs) -> bool {
    needs.paint || needs.images || needs.outlines
}

/// The interpreter's fonts, for laying out generated field text.
struct SessionFonts<'s, 'a>(&'s crate::fonts::Fonts<'a>);

impl FontSource for SessionFonts<'_, '_> {
    fn font(
        &self,
        _: &Document,
        id: Option<ObjectId>,
        dictionary: &Dictionary,
    ) -> Result<Arc<PdfFont>> {
        self.0.load(id, dictionary, Position::Unknown)
    }

    fn provider(&self) -> Option<&dyn Provider> {
        self.0.provider()
    }
}

/// Missing geometry cannot hide supplied artwork. Only an inert Popup can
/// be skipped without inventing coordinates or paint, with an explicit repair.
fn skip_inert_popup(
    document: &Document,
    annotation: &Dictionary,
    subtype: &[u8],
    object: Option<ObjectId>,
) -> Result<bool> {
    if subtype != b"Popup"
        || document.optional(annotation, b"AP")?.is_some()
        || annotation
            .get(b"Rect")
            .map(|value| document.resolve(value))
            .transpose()?
            .is_some_and(|value| !value.is_null())
    {
        return Ok(false);
    }
    document.repair(
        Diagnostic::new(
            pagura_appearance::repairs::POPUP_MISSING_RECT,
            "popup without Rect or appearance skipped in flat-page viewing",
        )
        .in_object_opt(object),
    )?;
    Ok(true)
}

/// A rectangle entry.
fn rectangle(document: &Document, d: &Dictionary, key: &[u8]) -> Result<Rect> {
    let values = d
        .get(key)
        .map(|v| document.resolve(v))
        .transpose()?
        .and_then(Value::as_array)
        .ok_or(Error::syntax("invalid annotation rectangle"))?;
    let [x0, y0, x1, y1] = values else {
        return Err(Error::syntax("annotation rectangle requires four numbers"));
    };
    let coordinate = |value| {
        document.resolve(value)?.as_number().ok_or(Error::syntax(
            "annotation rectangle coordinate must be numeric",
        ))
    };
    Rect::from_corners(
        coordinate(x0)?,
        coordinate(y0)?,
        coordinate(x1)?,
        coordinate(y1)?,
    )
    .ok_or(Error::syntax("invalid annotation rectangle"))
}

/// Screen placement of a `FixedPrint` watermark (12.5.6.22, Tables 193–194).
/// `MediaBox` defines the media; `CropBox` clips the resulting display as usual.
/// The returned upright rectangle's upper-left corner is in default user
/// space. The existing `NoRotate` view keeps the fitted appearance upright at
/// that anchor. This does not supply physical print media, tiling or n-up.
fn fixed_print(
    document: &Document,
    annotation: &Dictionary,
    rect: Rect,
    page: &Page,
) -> Result<Option<Rect>> {
    let Some(value) = annotation.get(b"FixedPrint") else {
        return Ok(None);
    };
    let value = document.resolve(value)?;
    if matches!(value, Value::Null) {
        return Ok(None);
    }
    let dictionary = value
        .as_dictionary()
        .ok_or(Error::syntax("FixedPrint must be a dictionary"))?;
    let entry = |key: &[u8]| {
        dictionary
            .get(key)
            .map(|value| document.resolve(value))
            .transpose()
            .map(|value| value.filter(|value| !matches!(value, Value::Null)))
    };
    if entry(b"Type")?
        .and_then(Value::as_name)
        .is_none_or(|name| name != b"FixedPrint")
    {
        return Err(Error::syntax("FixedPrint Type must be FixedPrint"));
    }
    let mut matrix = Transform::IDENTITY;
    if let Some(value) = entry(b"Matrix")? {
        let Some(values @ [_, _, _, _, _, _]) = value.as_array() else {
            return Err(Error::syntax("FixedPrint Matrix requires six numbers"));
        };
        let mut components = [0.0; 6];
        for (target, value) in components.iter_mut().zip(values) {
            *target = document
                .resolve(value)?
                .as_number()
                .ok_or(Error::syntax("FixedPrint Matrix requires six numbers"))?;
        }
        matrix = super::matrix(components);
    }
    let fraction = |key: &[u8]| {
        entry(key)?
            .map(|value| {
                value
                    .as_number()
                    .ok_or(Error::syntax("FixedPrint offset must be numeric"))
            })
            .transpose()
            .map(|value| value.unwrap_or(0.0))
    };
    let origin = Rect::from_corners(0.0, 0.0, rect.width(), rect.height())
        .ok_or(Error::limit("FixedPrint rectangle exceeds numeric range"))?;
    let target = envelope(origin, matrix)?;
    let view = Transform::page_view(page.media_box(), page.rotation());
    let media = envelope(page.media_box(), view)?;
    let anchor = Point::new(
        target.x0() + fraction(b"H")? * media.width(),
        media.height() - (target.y1() + fraction(b"V")? * media.height()),
    );
    let anchor = view
        .inverse()
        .ok_or(Error::limit(
            "FixedPrint media transform exceeds numeric range",
        ))?
        .apply(anchor);
    Rect::from_corners(
        anchor.x,
        anchor.y - target.height(),
        anchor.x + target.width(),
        anchor.y,
    )
    .map(Some)
    .ok_or(Error::limit("FixedPrint placement exceeds numeric range"))
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
