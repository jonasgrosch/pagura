//! Normal button artwork (12.7.5.2, Tables 192 and 250).
//!
//! Caption/icon partitions are reader policy: equal halves for side-by-side
//! content, centered single-line text with the ordinary two-unit padding.
//! Missing toggle captions retain the existing ZaDb check-mark policy.

use super::{Generation, bounds, inset, layout, style::Style, update};
use crate::fonts::Encoding;
use crate::form::da::DefaultAppearance;
use crate::program::Program;
use crate::stream::{AppearanceStream, appearance_bounds, appearance_placement, form_geometry};
use crate::values::{number, optional, optional_dictionary};
use pagura_core::{Diagnostics, Dictionary, Error, Meter, Name, Rect, Result, Value, text_string};
use pagura_document::{
    Document,
    forms::{ButtonKind, FieldValue, Justification},
};

pub(super) fn generate(
    input: &Generation<'_, '_>,
    kind: ButtonKind,
    state: Option<&Name>,
) -> Result<AppearanceStream> {
    let document = input.field.document;
    let limits = document.limits();
    let style = Style::read(document, input.widget)?;
    let frame = crate::widget_rotation::frame(style.rotation, bounds(input.bounds)?)?;
    let bbox = frame.bounds;
    let mut content = Program::new(&limits);
    style.paint(&mut content, bbox)?;
    let state = state.or(match input.field.value {
        Some(FieldValue::Name(name)) => Some(*name),
        _ => None,
    });
    let mut resources = Dictionary::new();
    if kind == ButtonKind::Push || state.is_some_and(|name| name.as_bytes() != b"Off") {
        let mk = optional_dictionary(document, input.widget, b"MK")?;
        let position = if kind == ButtonKind::Push {
            Position::read(document, mk)?
        } else {
            Position::Caption
        };
        if position != Position::Caption {
            icon(
                &mut content,
                &mut resources,
                input,
                mk,
                position,
                bbox,
                style.inset - 2.0,
            )?;
        }
        if position != Position::Icon
            && bbox.width() > 2.0 * style.inset
            && bbox.height() > 2.0 * style.inset
        {
            let area = inset(bbox, style.inset)?;
            let value = mk
                .map(|mk| optional(document, mk, b"CA"))
                .transpose()?
                .flatten();
            if let Some(value) = value {
                caption(
                    &mut content,
                    &mut resources,
                    input,
                    value,
                    position.caption(area)?,
                )?;
            } else if kind != ButtonKind::Push {
                check_mark(&mut content, &mut resources, area)?;
            }
        }
    }
    let mut dictionary = frame.dictionary();
    dictionary.insert(Name::new(b"Resources"), Value::Dictionary(resources));
    AppearanceStream::new(document, dictionary, content.finish())
}

/// Decode a normal caption once, then use the ordinary bounded font/layout path.
fn caption(
    out: &mut Program,
    resources: &mut Dictionary,
    input: &Generation<'_, '_>,
    value: &Value,
    area: Rect,
) -> Result<()> {
    let document = input.field.document;
    let limits = document.limits();
    let bytes = value
        .as_string()
        .ok_or(Error::syntax("button CA must be a text string"))?;
    let mut storage = Meter::new(
        limits.document_bytes,
        "button caption storage budget exceeded",
    );
    storage.charge(bytes.len().saturating_mul(3).min(limits.string_bytes))?;
    let mut diagnostics = Diagnostics::new(document.recovery());
    let text = text_string::decode(bytes, &limits, &mut diagnostics)?;
    document.report(diagnostics.take());
    if text.is_empty() {
        return Ok(());
    }
    *resources = update::resources(document, Some(resources), input.default_resources)?;
    let da = DefaultAppearance::parse(
        input
            .field
            .default_appearance
            .ok_or(Error::syntax("button caption has no DA"))?,
        &limits,
    )?;
    let font = Encoding::load(document, resources, da.font.as_bytes(), input.fonts)?;
    clip(out, area)?;
    out.text("BT\n")?;
    layout::write(
        out,
        &font,
        &da,
        area,
        &layout::TextLayout {
            text: &text,
            multiline: false,
            comb: None,
            password: false,
            quadding: Justification::Center,
            no_scroll: false,
        },
        &limits,
    )?;
    out.text("ET\nQ\n")
}

/// Table 192's closed set of normal caption/icon positions.
#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
enum Position {
    Caption,
    Icon,
    Below,
    Above,
    Right,
    Left,
    Overlay,
}

impl Position {
    fn read(document: &Document, mk: Option<&Dictionary>) -> Result<Self> {
        let value = mk
            .map(|mk| optional(document, mk, b"TP"))
            .transpose()?
            .flatten();
        let code = value
            .map(|value| {
                value
                    .as_integer()
                    .ok_or(Error::syntax("button TP must be an integer"))
            })
            .transpose()?
            .unwrap_or(0);
        Self::try_from(code)
    }

    fn caption(self, area: Rect) -> Result<Rect> {
        let (x0, y0, x1, y1) = (area.x0(), area.y0(), area.x1(), area.y1());
        let (mx, my) = (f64::midpoint(x0, x1), f64::midpoint(y0, y1));
        let rect = match self {
            Self::Below => Rect::from_corners(x0, y0, x1, my),
            Self::Above => Rect::from_corners(x0, my, x1, y1),
            Self::Right => Rect::from_corners(mx, y0, x1, y1),
            Self::Left => Rect::from_corners(x0, y0, mx, y1),
            _ => Some(area),
        };
        rect.ok_or(Error::limit("button partition exceeds numeric range"))
    }

    fn icon(self, area: Rect) -> Result<Rect> {
        match self {
            Self::Below => Self::Above.caption(area),
            Self::Above => Self::Below.caption(area),
            Self::Right => Self::Left.caption(area),
            Self::Left => Self::Right.caption(area),
            _ => Ok(area),
        }
    }
}

impl TryFrom<i64> for Position {
    type Error = Error;

    fn try_from(code: i64) -> Result<Self> {
        match code {
            0 => Ok(Self::Caption),
            1 => Ok(Self::Icon),
            2 => Ok(Self::Below),
            3 => Ok(Self::Above),
            4 => Ok(Self::Right),
            5 => Ok(Self::Left),
            6 => Ok(Self::Overlay),
            _ => Err(Error::syntax("button TP must be from 0 through 6")),
        }
    }
}

/// A Form stays a source reference: the ordinary interpreter owns its resources,
/// clipping, transparency and execution, exactly as for a page's Form XObject.
fn icon(
    out: &mut Program,
    resources: &mut Dictionary,
    input: &Generation<'_, '_>,
    mk: Option<&Dictionary>,
    position: Position,
    bbox: Rect,
    border: f64,
) -> Result<()> {
    let document = input.field.document;
    let Some(mk) = mk else { return Ok(()) };
    let Some(icon) = mk.get(b"I") else {
        return Ok(());
    };
    if document.resolve(icon)?.is_null() {
        return Ok(());
    }
    let id = icon
        .as_reference()
        .ok_or(Error::syntax("button I must be an indirect Form stream"))?;
    let object = document.object(id)?;
    let dict = object
        .dictionary()
        .filter(|_| object.is_stream())
        .ok_or(Error::syntax("button I must be a Form stream"))?;
    if optional(document, dict, b"Subtype")?
        .and_then(Value::as_name)
        .map(Name::as_bytes)
        != Some(b"Form")
    {
        return Err(Error::syntax("button icon must have Form subtype").in_object(id));
    }
    let (source, matrix) = form_geometry(document, dict).map_err(|e| e.in_object(id))?;
    let envelope = appearance_bounds(source, matrix)?;
    let fit = optional_dictionary(document, mk, b"IF")?;
    let full = fit
        .map(|fit| optional(document, fit, b"FB"))
        .transpose()?
        .flatten();
    let border = match full {
        None | Some(Value::Boolean(false)) => border,
        Some(Value::Boolean(true)) => 0.0,
        _ => return Err(Error::syntax("icon fit FB must be a boolean")),
    };
    if bbox.width() <= 2.0 * border || bbox.height() <= 2.0 * border {
        return Ok(());
    }
    let area = position.icon(inset(bbox, border)?)?;
    let target = fitted(document, fit, envelope, area)?;
    let Some(transform) = appearance_placement(source, matrix, target)? else {
        return Ok(());
    };
    let defaults = if position == Position::Icon
        || optional(document, mk, b"CA")?
            .is_none_or(|value| value.as_string().is_some_and(<[u8]>::is_empty))
    {
        None
    } else {
        input
            .default_resources
            .map(|defaults| optional_dictionary(document, defaults, b"XObject"))
            .transpose()?
            .flatten()
    };
    let mut name = Name::new(b"Icon");
    if let Some(defaults) = defaults {
        let mut work = Meter::new(
            document.limits().operations,
            "button icon resource-name budget exceeded",
        );
        for index in 0..=defaults.len() {
            work.charge(1)?;
            if defaults.get(name.as_bytes()).is_none() {
                break;
            }
            name = Name::new(format!("Icon{index}").as_bytes());
        }
    }
    let mut objects = Dictionary::new();
    objects.insert(name.clone(), Value::Reference(id));
    resources.insert(Name::new(b"XObject"), Value::Dictionary(objects));
    clip(out, area)?;
    out.text(&format!(
        "{} {} {} {} {} {} cm\n{name} Do\nQ\n",
        transform.a, transform.b, transform.c, transform.d, transform.e, transform.f
    ))
}

fn fitted(document: &Document, fit: Option<&Dictionary>, source: Rect, area: Rect) -> Result<Rect> {
    // Keep zero-area/singular handling in the shared appearance fitting owner.
    if source.is_empty() {
        return Ok(area);
    }
    let entry = |key| {
        fit.map(|fit| optional(document, fit, key))
            .transpose()
            .map(Option::flatten)
    };
    let name = |key, default: &'static [u8]| -> Result<&[u8]> {
        entry(key)?
            .map(|value| {
                value
                    .as_name()
                    .map(Name::as_bytes)
                    .ok_or(Error::syntax("icon fit mode must be a name"))
            })
            .transpose()
            .map(|value| value.unwrap_or(default))
    };
    let (mut sx, mut sy) = (
        area.width() / source.width(),
        area.height() / source.height(),
    );
    let proportional = match name(b"S", b"P")? {
        b"P" => true,
        b"A" => false,
        _ => return Err(Error::syntax("invalid icon fit S")),
    };
    let scale = match name(b"SW", b"A")? {
        b"A" => true,
        b"B" => sx.min(sy) < 1.0,
        b"S" => sx.min(sy) >= 1.0 && sx.max(sy) > 1.0,
        b"N" => false,
        _ => return Err(Error::syntax("invalid icon fit SW")),
    };
    if scale && proportional {
        sx = sx.min(sy);
        sy = sx;
    } else if !scale {
        sx = 1.0;
        sy = 1.0;
    }
    let mut align = [0.5; 2];
    if proportional && let Some(value) = entry(b"A")? {
        let values = value
            .as_array()
            .filter(|values| values.len() == 2)
            .ok_or(Error::syntax("icon fit A must contain two numbers"))?;
        for (out, value) in align.iter_mut().zip(values) {
            *out = number(document, value)?;
            if !(0.0..=1.0).contains(out) {
                return Err(Error::syntax("icon alignment outside zero through one"));
            }
        }
    }
    let (width, height) = (source.width() * sx, source.height() * sy);
    let x = area.x0() + (area.width() - width) * align[0];
    let y = area.y0() + (area.height() - height) * align[1];
    Rect::from_corners(x, y, x + width, y + height)
        .ok_or(Error::limit("icon fitting exceeds numeric range"))
}

fn clip(out: &mut Program, area: Rect) -> Result<()> {
    out.text(&format!(
        "q\n{} {} {} {} re W n\n",
        area.x0(),
        area.y0(),
        area.width(),
        area.height()
    ))
}

/// Preserve the existing default toggle mark, including its baseline policy.
fn check_mark(out: &mut Program, resources: &mut Dictionary, area: Rect) -> Result<()> {
    let size = area.height().min(area.width());
    resources.insert(Name::new(b"Font"), Value::Dictionary(zapf_dingbats()));
    out.text(&format!(
        "q\n{} {} {} {} re W n\n0 g\nBT\n/ZaDb {size} Tf\n1 0 0 1 {} {} Tm\n<34> Tj\nET\nQ\n",
        area.x0(),
        area.y0(),
        area.width(),
        area.height(),
        area.x0(),
        area.y0()
    ))
}

/// A font dictionary with the standard ZapfDingbats font as `/ZaDb`.
fn zapf_dingbats() -> Dictionary {
    let mut font = Dictionary::new();
    font.insert(Name::new(b"Type"), Value::name(b"Font"));
    font.insert(Name::new(b"Subtype"), Value::name(b"Type1"));
    font.insert(Name::new(b"BaseFont"), Value::name(b"ZapfDingbats"));
    let mut fonts = Dictionary::new();
    fonts.insert(Name::new(b"ZaDb"), Value::Dictionary(font));
    fonts
}
