//! Link annotations, their hit geometry and inert targets (12.5.6.5).
use super::{
    context::{Context, Source, number},
    destinations::{self, Target},
};
use crate::{Document, Page, tags};
use pagura_core::{Dictionary, Error, Rect, Result, Value};

/// Link feedback while the user presses it (Table 176).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HighlightMode {
    /// No feedback.
    None,
    /// Invert the rectangle contents (default).
    #[default]
    Invert,
    /// Invert the border.
    Outline,
    /// A pressed appearance.
    Push,
}
/// Annotation border style (12.5.4, Table 168).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BorderStyle {
    /// Continuous stroke.
    #[default]
    Solid,
    /// Dashed stroke.
    Dashed,
    /// Raised border.
    Beveled,
    /// Recessed border.
    Inset,
    /// Underline along the annotation bottom.
    Underline,
}
/// Resolved border appearance, with BS taking precedence over Border.
#[derive(Debug, Clone, PartialEq)]
pub struct LinkBorder {
    /// Horizontal corner radius from Border; zero for BS.
    pub horizontal_radius: f64,
    /// Vertical corner radius from Border; zero for BS.
    pub vertical_radius: f64,
    /// Line width; zero means no border.
    pub width: f64,
    /// Border style.
    pub style: BorderStyle,
    /// Dash lengths; empty for a solid border.
    pub dash: Vec<f64>,
}
/// A Link annotation. Geometry is in default user space, before page rotation.
#[derive(Debug, Clone, PartialEq)]
pub struct Link<'a> {
    /// Normalized annotation rectangle.
    pub rectangle: Rect,
    /// Active quadrilaterals, in source vertex order. Empty means use Rect.
    /// As Table 176 requires, any coordinate outside Rect disables all quads.
    pub quad_points: Vec<[f64; 8]>,
    /// Border appearance.
    pub border: LinkBorder,
    /// Highlight feedback.
    pub highlight: HighlightMode,
    /// A destination or inert action tree.
    pub target: Option<Target<'a>>,
    /// Unmodified annotation dictionary, including flags and extension data.
    pub dictionary: &'a Dictionary,
}
fn numbers(ctx: &mut Context<'_>, value: &Value, source: Source) -> Result<Vec<f64>> {
    let values = ctx
        .doc
        .resolve(value)?
        .as_array()
        .ok_or_else(|| source.error(Error::syntax("annotation geometry must be an array")))?;
    ctx.allocate::<f64>(values.len(), source)?;
    values
        .iter()
        .map(|v| number(ctx.doc.resolve(v)?, source))
        .collect()
}
fn nonnegative(n: f64, source: Source) -> Result<f64> {
    if n >= 0.0 {
        Ok(n)
    } else {
        Err(source.error(Error::syntax("negative annotation border dimension")))
    }
}
fn dash(ctx: &mut Context<'_>, value: &Value, source: Source) -> Result<Vec<f64>> {
    let dash = numbers(ctx, value, source)?;
    if dash.iter().any(|n| *n < 0.0) || (!dash.is_empty() && dash.iter().all(|n| *n == 0.0)) {
        return Err(source.error(Error::syntax("invalid annotation dash pattern")));
    }
    Ok(dash)
}
fn border<'a>(ctx: &mut Context<'a>, d: &'a Dictionary, source: Source) -> Result<LinkBorder> {
    let mut border = LinkBorder {
        horizontal_radius: 0.0,
        vertical_radius: 0.0,
        width: 1.0,
        style: BorderStyle::Solid,
        dash: Vec::new(),
    };
    if let Some(bs) = ctx.entry(d, b"BS", source)? {
        let (bs, source) = ctx.dictionary(bs, source)?;
        if let Some(w) = ctx.entry(bs, b"W", source)? {
            border.width = nonnegative(number(w, source)?, source)?;
        }
        if let Some(style) = ctx.entry(bs, b"S", source)? {
            border.style = match style.as_name().map(pagura_core::Name::as_bytes) {
                Some(b"S") => BorderStyle::Solid,
                Some(b"D") => BorderStyle::Dashed,
                Some(b"B") => BorderStyle::Beveled,
                Some(b"I") => BorderStyle::Inset,
                Some(b"U") => BorderStyle::Underline,
                _ => return Err(source.error(Error::syntax("unknown annotation border style"))),
            };
        }
        border.dash = if let Some(value) = bs.get(b"D") {
            dash(ctx, value, source)?
        } else {
            vec![3.0]
        };
        if border.style != BorderStyle::Dashed {
            border.dash.clear();
        }
    } else if let Some(value) = ctx.entry(d, b"Border", source)? {
        let values = value
            .as_array()
            .ok_or_else(|| source.error(Error::syntax("Border must be an array")))?;
        if !(3..=4).contains(&values.len()) {
            return Err(source.error(Error::syntax("Border requires three or four entries")));
        }
        border.horizontal_radius =
            nonnegative(number(ctx.doc.resolve(&values[0])?, source)?, source)?;
        border.vertical_radius =
            nonnegative(number(ctx.doc.resolve(&values[1])?, source)?, source)?;
        border.width = nonnegative(number(ctx.doc.resolve(&values[2])?, source)?, source)?;
        if let Some(value) = values.get(3) {
            border.dash = dash(ctx, value, source)?;
            border.style = BorderStyle::Dashed;
        }
    }
    Ok(border)
}
fn quads(
    ctx: &mut Context<'_>,
    value: &Value,
    rect: Rect,
    source: Source,
) -> Result<Vec<[f64; 8]>> {
    let values = match numbers(ctx, value, source) {
        Ok(values) if values.len() % 8 == 0 => values,
        Ok(_) => {
            ctx.repair(
                tags::LINK_QUADS,
                "malformed link QuadPoints ignored; Rect used (12.5.6.5, Table 176)",
                source,
            )?;
            return Ok(Vec::new());
        }
        Err(error) if error.kind == pagura_core::ErrorKind::Syntax => {
            ctx.repair(
                tags::LINK_QUADS,
                "malformed link QuadPoints ignored; Rect used (12.5.6.5, Table 176)",
                source,
            )?;
            return Ok(Vec::new());
        }
        Err(error) => return Err(error),
    };
    if values
        .as_chunks::<2>()
        .0
        .iter()
        .any(|xy| xy[0] < rect.x0() || xy[0] > rect.x1() || xy[1] < rect.y0() || xy[1] > rect.y1())
    {
        return Ok(Vec::new());
    }
    ctx.allocate::<[f64; 8]>(values.len() / 8, source)?;
    Ok(values.as_chunks::<8>().0.to_vec())
}
pub(super) fn read<'a>(doc: &'a Document, page: &'a Page) -> Result<Vec<Link<'a>>> {
    let mut ctx = Context::new(doc);
    let source = ctx.source(page.id())?;
    let Some(annots) = ctx.entry(page.dictionary(), b"Annots", source)? else {
        return Ok(Vec::new());
    };
    let annots = annots
        .as_array()
        .ok_or_else(|| source.error(Error::syntax("page Annots must be an array")))?;
    let mut links = Vec::new();
    for value in annots {
        ctx.step(0, source)?;
        let (d, source) = ctx.dictionary(value, source)?;
        if ctx
            .entry(d, b"Subtype", source)?
            .and_then(Value::as_name)
            .is_none_or(|n| n != b"Link")
        {
            continue;
        }
        ctx.type_name(d, b"Annot", source)?;
        let values = numbers(
            &mut ctx,
            d.get(b"Rect")
                .ok_or_else(|| source.error(Error::syntax("link requires Rect")))?,
            source,
        )?;
        let [x0, y0, x1, y1] = values.as_slice() else {
            return Err(source.error(Error::syntax("link Rect requires four numbers")));
        };
        let rectangle = Rect::from_corners(*x0, *y0, *x1, *y1)
            .ok_or_else(|| source.error(Error::syntax("invalid link rectangle")))?;
        let quad_points = ctx
            .entry(d, b"QuadPoints", source)?
            .map(|v| quads(&mut ctx, v, rectangle, source))
            .transpose()?
            .unwrap_or_default();
        let highlight = if let Some(value) = ctx.entry(d, b"H", source)? {
            match value.as_name().map(pagura_core::Name::as_bytes) {
                Some(b"I") => HighlightMode::Invert,
                Some(b"N") => HighlightMode::None,
                Some(b"O") => HighlightMode::Outline,
                Some(b"P") => HighlightMode::Push,
                _ => return Err(source.error(Error::syntax("invalid link highlight mode"))),
            }
        } else {
            HighlightMode::Invert
        };
        ctx.allocate::<Link<'_>>(1, source)?;
        links.push(Link {
            rectangle,
            quad_points,
            border: border(&mut ctx, d, source)?,
            highlight,
            target: destinations::target(&mut ctx, d, source)?,
            dictionary: d,
        });
    }
    Ok(links)
}
