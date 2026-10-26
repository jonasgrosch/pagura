//! Plain FreeText (12.5.6.6, Table 177) through the shared variable-text layout.
//!
//! Reader policy uses C as the optional background, a black border, two units
//! of interior padding, whitespace wrapping and the form auto-size/leading
//! policy. Only the DA font is copied from AcroForm DR. An absent resource
//! may use an exact standard-14 name through the explicit font provider.

mod callout;

use super::border::{Border, inset_bounds, plain_effect};
use super::marks::{Color, Marks, Pen, Subpath, color};
use crate::features;
use crate::fonts::{Encoding, FontSource};
use crate::form::{da::DefaultAppearance, layout, layout::TextLayout, supply_font};
use crate::program::Program;
use crate::stream::AppearanceStream;
use crate::values::{optional, optional_dictionary};
use pagura_core::{Diagnostics, Dictionary, Error, Meter, Name, Rect, Result, Value, text_string};
use pagura_document::{Document, forms::Justification};

pub(super) fn generate(
    document: &Document,
    annotation: &Dictionary,
    rect: Rect,
    fonts: &dyn FontSource,
) -> Result<Option<AppearanceStream>> {
    plain_text(document, annotation)?;
    let callout = callout::Callout::read(document, annotation)?;
    plain_effect(document, annotation)?;
    let bounds = inset_bounds(document, annotation, rect)?;
    let border = Border::read(document, annotation)?;
    let background = color(document, annotation, b"C", None)?;
    let quadding = quadding(document, annotation)?;
    let limits = document.limits();
    let mut storage = Meter::new(limits.document_bytes, "free text storage budget exceeded");
    let mut work = Meter::new(limits.operations, "free text work budget exceeded");
    let da_bytes = optional(document, annotation, b"DA")?
        .and_then(Value::as_string)
        .ok_or(Error::syntax("free text DA must be a byte string"))?;
    storage.charge_product(da_bytes.len(), 2)?;
    work.charge(da_bytes.len())?;
    let da = DefaultAppearance::parse(da_bytes, &limits)?;
    let text = contents(document, annotation, &mut storage)?;
    let mut geometry = Meter::new(limits.path_segments, "free text geometry budget exceeded");
    geometry.charge(12)?; // Background, border and text clip: four edges each.
    if callout.is_some() {
        geometry.charge(48)?; // At most two body edges and one fixed ending.
        work.charge(48)?;
    }
    let mut marks = Marks::new(document, annotation);
    if bounds.width() == 0.0 || bounds.height() == 0.0 {
        return Ok(None);
    }
    if let Some(callout) = callout {
        callout.draw(&mut marks, &border, background.as_ref())?;
    }
    paint_box(&mut marks, bounds, &border, background.as_ref())?;
    let area = inset(bounds, border.width + 2.0)?;
    if !text.is_empty() && area.width() > 0.0 && area.height() > 0.0 {
        work.charge(256)?;
        work.charge(text.len())?;
        // Include the fixed reverse font map and the shared layout's worst-case
        // character/line vectors, including their geometric spare capacity.
        storage.charge(256 * 64)?;
        storage.charge_product(text.len().saturating_add(1), 128)?;
        let resources = resources(document, da.font.as_bytes(), &mut storage)?;
        let font = Encoding::load(document, &resources, da.font.as_bytes(), fonts)?;
        let mut out = Program::new(&limits);
        out.text(&format!(
            "q\n{} {} {} {} re W n\nBT\n",
            area.x0(),
            area.y0(),
            area.width(),
            area.height()
        ))?;
        let text = TextLayout {
            text: &text,
            multiline: true,
            comb: None,
            password: false,
            quadding,
            no_scroll: false,
        };
        let layout_limits = pagura_core::Limits {
            operations: work.remaining(),
            ..limits
        };
        layout::write(&mut out, &font, &da, area, &text, &layout_limits)?;
        out.text("ET\nQ\n")?;
        let entries = document.resolve_dictionary(
            resources
                .get(b"Font")
                .ok_or(Error::syntax("missing free text font resources"))?,
        )?;
        storage.charge(entries.retained_bytes())?;
        marks.text(&out.finish(), entries.clone(), &Color::black())?;
    }
    marks.finish(rect)
}

fn contents(document: &Document, annotation: &Dictionary, storage: &mut Meter) -> Result<String> {
    let Some(value) = optional(document, annotation, b"Contents")? else {
        return Ok(String::new());
    };
    let bytes = value
        .as_string()
        .ok_or(Error::syntax("free text Contents must be a text string"))?;
    let limits = document.limits();
    storage.charge(bytes.len().saturating_mul(3).min(limits.string_bytes))?;
    let mut diagnostics = Diagnostics::new(document.recovery());
    let text = text_string::decode(bytes, &limits, &mut diagnostics)?;
    document.report(diagnostics.take());
    Ok(text)
}

fn resources(document: &Document, name: &[u8], storage: &mut Meter) -> Result<Dictionary> {
    let mut resources = Dictionary::new();
    if let Some(form) = optional_dictionary(document, document.catalog()?, b"AcroForm")?
        && let Some(defaults) = optional_dictionary(document, form, b"DR")?
        && let Some(fonts) = optional_dictionary(document, defaults, b"Font")?
        && let Some(font) = fonts.get(name)
    {
        storage.charge(font.retained_bytes())?;
        let mut selected = Dictionary::new();
        selected.insert(Name::new(name), font.clone());
        resources.insert(Name::new(b"Font"), Value::Dictionary(selected));
    }
    supply_font(document, &mut resources, name)?;
    storage.charge(resources.retained_bytes())?;
    Ok(resources)
}

fn plain_text(document: &Document, annotation: &Dictionary) -> Result<()> {
    if optional(document, annotation, b"RC")?.is_some()
        || optional(document, annotation, b"DS")?.is_some()
    {
        return Err(Error::unsupported(
            features::ANNOTATION_APPEARANCE,
            "rich free text appearance generation is unavailable",
        ));
    }
    Ok(())
}

fn quadding(document: &Document, annotation: &Dictionary) -> Result<Justification> {
    match optional(document, annotation, b"Q")?.map(Value::as_integer) {
        None | Some(Some(0)) => Ok(Justification::Left),
        Some(Some(1)) => Ok(Justification::Center),
        Some(Some(2)) => Ok(Justification::Right),
        _ => Err(Error::syntax(
            "free text Q must be an integer from zero to two",
        )),
    }
}

fn inset(rect: Rect, amount: f64) -> Result<Rect> {
    if !amount.is_finite() {
        return Err(Error::limit("free text padding exceeds numeric range"));
    }
    let amount = amount.min(rect.width() / 2.0).min(rect.height() / 2.0);
    Rect::from_corners(
        rect.x0() + amount,
        rect.y0() + amount,
        rect.x1() - amount,
        rect.y1() - amount,
    )
    .ok_or(Error::limit("free text bounds exceed numeric range"))
}

fn rectangle(bounds: Rect) -> Subpath<'static> {
    Subpath::Rect {
        x: bounds.x0(),
        y: bounds.y0(),
        width: bounds.width(),
        height: bounds.height(),
    }
}

fn paint_box(
    marks: &mut Marks<'_>,
    bounds: Rect,
    border: &Border,
    background: Option<&Color>,
) -> Result<()> {
    if let Some(background) = background {
        marks.fill(&[rectangle(bounds)], background)?;
    }
    if border.width > 0.0 {
        let inner = inset(bounds, border.width / 2.0)?;
        let line = [(inner.x0(), inner.y0()), (inner.x1(), inner.y0())];
        let path = if border.underline {
            Subpath::Polyline(&line)
        } else {
            rectangle(inner)
        };
        marks.stroke(
            &[path],
            &Color::black(),
            Pen {
                width: border.width,
                dash: &border.dash,
            },
        )?;
    }
    Ok(())
}
