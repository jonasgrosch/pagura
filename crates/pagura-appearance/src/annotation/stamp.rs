//! Rubber stamps (12.5.6.12, Table 184).
//!
//! Reader artwork uses a red rectangular frame and centred uppercase
//! Helvetica-Bold/WinAnsi text. The smallest rectangle dimension determines
//! the frame width (1/32) and text padding (1/8). Text fits both dimensions.
//! Unknown custom names use Draft. Image/snapshot intents require stored AP.

use super::marks::{Color, Marks, Pen, Subpath, color};
use crate::features;
use crate::fonts::{Encoding, FontSource};
use crate::program::Program;
use crate::stream::AppearanceStream;
use crate::values::optional;
use pagura_core::{Dictionary, Error, Meter, Name, Rect, Result, Value};
use pagura_document::Document;

pub(super) fn generate(
    document: &Document,
    annotation: &Dictionary,
    rect: Rect,
    fonts: &dyn FontSource,
) -> Result<Option<AppearanceStream>> {
    let label = label(document, annotation)?;
    let (width, height) = (rect.width(), rect.height());
    if !width.is_finite() || !height.is_finite() {
        return Err(Error::limit("stamp extent exceeds numeric range"));
    }
    let Some(color) = color(document, annotation, b"C", Some(Color::red()))? else {
        return Ok(None);
    };
    if width == 0.0 || height == 0.0 {
        return Ok(None);
    }
    let stroke = width.min(height) / 32.0;
    if stroke == 0.0 {
        return Err(Error::limit("stamp extent is below numeric precision"));
    }
    let (text, resources) = text(document, label, rect, fonts)?;
    let mut marks = Marks::new(document, annotation);
    marks.stroke(
        &[Subpath::Rect {
            x: rect.x0() + stroke / 2.0,
            y: rect.y0() + stroke / 2.0,
            width: width - stroke,
            height: height - stroke,
        }],
        &color,
        Pen {
            width: stroke,
            dash: &[],
        },
    )?;
    marks.text(&text, resources, &color)?;
    marks.finish(rect)
}

fn text(
    document: &Document,
    label: &str,
    rect: Rect,
    fonts: &dyn FontSource,
) -> Result<(Vec<u8>, Dictionary)> {
    let limits = document.limits();
    let mut work = Meter::new(limits.operations, "stamp layout work exceeded");
    work.charge(256 + label.len())?;
    if limits.path_segments < 4 {
        return Err(Error::limit("stamp frame segment budget exceeded"));
    }
    let mut font = Dictionary::new();
    font.insert(Name::new(b"Type"), Value::name(b"Font"));
    font.insert(Name::new(b"Subtype"), Value::name(b"Type1"));
    font.insert(Name::new(b"BaseFont"), Value::name(b"Helvetica-Bold"));
    font.insert(Name::new(b"Encoding"), Value::name(b"WinAnsiEncoding"));
    let mut entries = Dictionary::new();
    entries.insert(Name::new(b"StampText"), Value::Dictionary(font));
    let mut resources = Dictionary::new();
    resources.insert(Name::new(b"Font"), Value::Dictionary(entries.clone()));
    let mut storage = Meter::new(limits.document_bytes, "stamp font storage exceeded");
    storage.charge(resources.retained_bytes())?;
    storage.charge(entries.retained_bytes())?;
    storage.charge(256 * 64)?;
    let encoding = Encoding::load(document, &resources, b"StampText", fonts)?;
    let mut codes = [0; 32];
    let mut advance = 0.0;
    for (index, character) in label.chars().enumerate() {
        let code = encoding.encode(character)?;
        *codes
            .get_mut(index)
            .ok_or(Error::limit("stamp label exceeds fixed storage"))? = code.code;
        advance += code.advance;
    }
    if !advance.is_finite() {
        return Err(Error::limit("stamp text width exceeds numeric range"));
    }
    if advance <= 0.0 {
        return Err(Error::syntax("stamp text requires positive font advances"));
    }
    let padding = rect.width().min(rect.height()) / 8.0;
    let size = ((rect.width() - 2.0 * padding) / advance)
        .min((rect.height() - 2.0 * padding) / (encoding.ascent - encoding.descent));
    let x = rect.x0() / 2.0 + rect.x1() / 2.0 - advance * size / 2.0;
    let y = rect.y0() / 2.0 + rect.y1() / 2.0 - (encoding.ascent + encoding.descent) * size / 2.0;
    if ![size, x, y].iter().all(|value| value.is_finite()) {
        return Err(Error::limit("stamp text position exceeds numeric range"));
    }
    let mut out = Program::new(&limits);
    out.text(&format!(
        "BT\n0 Tc\n0 Tw\n100 Tz\n0 TL\n0 Tr\n0 Ts\n/StampText {size} Tf\n1 0 0 1 {x} {y} Tm\n<"
    ))?;
    for code in codes
        .get(..label.len())
        .ok_or(Error::limit("invalid stamp label length"))?
    {
        out.text(&format!("{code:02X}"))?;
    }
    out.text("> Tj\nET\n")?;
    Ok((out.finish(), entries))
}

fn label(document: &Document, annotation: &Dictionary) -> Result<&'static str> {
    if let Some(intent) = optional(document, annotation, b"IT")? {
        let intent = intent
            .as_name()
            .ok_or(Error::syntax("stamp IT must be a name"))?;
        if intent != b"Stamp" {
            return Err(Error::unsupported(
                features::ANNOTATION_APPEARANCE,
                "image, snapshot or unknown stamp intent requires a stored appearance",
            ));
        }
    }
    let name = optional(document, annotation, b"Name")?
        .map(|value| {
            value
                .as_name()
                .ok_or(Error::syntax("stamp Name must be a name"))
        })
        .transpose()?;
    Ok(match name.map_or(b"Draft".as_slice(), Name::as_bytes) {
        b"Approved" => "APPROVED",
        b"Experimental" => "EXPERIMENTAL",
        b"NotApproved" => "NOT APPROVED",
        b"AsIs" => "AS IS",
        b"Expired" => "EXPIRED",
        b"NotForPublicRelease" => "NOT FOR PUBLIC RELEASE",
        b"Confidential" => "CONFIDENTIAL",
        b"Final" => "FINAL",
        b"Sold" => "SOLD",
        b"Departmental" => "DEPARTMENTAL",
        b"ForComment" => "FOR COMMENT",
        b"TopSecret" => "TOP SECRET",
        b"ForPublicRelease" => "FOR PUBLIC RELEASE",
        _ => "DRAFT",
    })
}
