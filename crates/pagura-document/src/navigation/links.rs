//! Link annotations, their hit geometry and inert targets (12.5.6.5).

use super::destinations::{self, Target};
use crate::source::{ReadFault, ReadResult};
use crate::view::{Context, Source, number};
use crate::{Document, Page, repairs};
use pagura_core::{Dictionary, Error, ErrorKind, Name, Rect, Result, Value};

/// Link feedback while the user presses it (Table 176).
///
/// Closed by Table 176; `None` is the table's own name for no feedback.
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

impl TryFrom<&Name> for HighlightMode {
    type Error = Error;

    fn try_from(name: &Name) -> Result<Self> {
        match name.as_bytes() {
            b"N" => Ok(Self::None),
            b"I" => Ok(Self::Invert),
            b"O" => Ok(Self::Outline),
            b"P" => Ok(Self::Push),
            _ => Err(Error::syntax("invalid link highlight mode")),
        }
    }
}

/// Annotation border style (12.5.4, Table 168).
///
/// Closed by Table 168, which lets readers treat other names as an error.
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

impl TryFrom<&Name> for BorderStyle {
    type Error = Error;

    fn try_from(name: &Name) -> Result<Self> {
        match name.as_bytes() {
            b"S" => Ok(Self::Solid),
            b"D" => Ok(Self::Dashed),
            b"B" => Ok(Self::Beveled),
            b"I" => Ok(Self::Inset),
            b"U" => Ok(Self::Underline),
            _ => Err(Error::syntax("unknown annotation border style")),
        }
    }
}

/// Resolved border appearance, with BS taking precedence over Border.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
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
#[non_exhaustive]
pub struct Link<'a> {
    /// Normalized annotation rectangle.
    pub rectangle: Rect,
    /// Active quadrilaterals, in source vertex order. Empty means use Rect.
    /// As Table 176 requires, any coordinate outside Rect disables all quads.
    pub quad_points: Vec<[f64; 8]>,
    /// Border appearance.
    pub border: LinkBorder,
    /// Feedback while pressed.
    pub highlight: HighlightMode,
    /// A destination or inert action tree.
    pub target: Option<Target<'a>>,
    /// Unmodified annotation dictionary, including flags and extension data.
    pub dictionary: &'a Dictionary,
}

fn numbers<'a>(
    context: &mut Context<'a>,
    value: &'a Value,
    source: Source,
) -> ReadResult<Vec<f64>> {
    let (value, source) = context.resolve(value, source)?;
    let values = value
        .as_array()
        .ok_or_else(|| source.error(Error::syntax("annotation geometry must be an array")))?;
    context.allocate::<f64>(values.len(), source)?;
    values
        .iter()
        .map(|value| {
            let (value, source) = context.resolve(value, source)?;
            Ok(number(value, source)?)
        })
        .collect()
}

fn nonnegative(value: f64, source: Source) -> Result<f64> {
    if value >= 0.0 {
        Ok(value)
    } else {
        Err(source.error(Error::syntax("negative annotation border dimension")))
    }
}

fn dash<'a>(context: &mut Context<'a>, value: &'a Value, source: Source) -> ReadResult<Vec<f64>> {
    let dash = numbers(context, value, source)?;
    if dash.iter().any(|&length| length < 0.0)
        || (!dash.is_empty() && dash.iter().all(|&length| length == 0.0))
    {
        return Err(source
            .error(Error::syntax("invalid annotation dash pattern"))
            .into());
    }
    Ok(dash)
}

fn border<'a>(
    context: &mut Context<'a>,
    dictionary: &'a Dictionary,
    source: Source,
) -> ReadResult<LinkBorder> {
    let mut border = LinkBorder {
        horizontal_radius: 0.0,
        vertical_radius: 0.0,
        width: 1.0,
        style: BorderStyle::Solid,
        dash: Vec::new(),
    };
    if let Some(style) = context.entry(dictionary, b"BS", source)? {
        let (style, source) = context.dictionary(style, source)?;
        if let Some(width) = context.entry(style, b"W", source)? {
            border.width = nonnegative(number(width, source)?, source)?;
        }
        if let Some(name) = context.entry(style, b"S", source)? {
            border.style = name
                .as_name()
                .ok_or(Error::syntax("unknown annotation border style"))
                .and_then(BorderStyle::try_from)
                .map_err(|error| source.fault(error))?;
        }
        border.dash = if let Some(value) = style.get(b"D") {
            dash(context, value, source)?
        } else {
            vec![3.0]
        };
        if border.style != BorderStyle::Dashed {
            border.dash.clear();
        }
    } else if let Some(value) = context.entry(dictionary, b"Border", source)? {
        let values = value
            .as_array()
            .ok_or_else(|| source.error(Error::syntax("Border must be an array")))?;
        let ([horizontal, vertical, width] | [horizontal, vertical, width, _]) = values else {
            return Err(source
                .error(Error::syntax("Border requires three or four entries"))
                .into());
        };
        for (target, value) in [
            (&mut border.horizontal_radius, horizontal),
            (&mut border.vertical_radius, vertical),
            (&mut border.width, width),
        ] {
            let (value, source) = context.resolve(value, source)?;
            *target = nonnegative(number(value, source)?, source)?;
        }
        if let [_, _, _, dashes] = values {
            border.dash = dash(context, dashes, source)?;
            border.style = BorderStyle::Dashed;
        }
    }
    Ok(border)
}

fn quads<'a>(
    context: &mut Context<'a>,
    value: &'a Value,
    rectangle: Rect,
    source: Source,
) -> ReadResult<Vec<[f64; 8]>> {
    let values = match numbers(context, value, source) {
        Ok(values) if values.len() % 8 == 0 => values,
        Ok(_) => {
            ignore_quads(context, source)?;
            return Ok(Vec::new());
        }
        Err(ReadFault::Invalid(error)) if error.kind == ErrorKind::Syntax => {
            ignore_quads(context, source)?;
            return Ok(Vec::new());
        }
        Err(error) => return Err(error),
    };
    if values.as_chunks::<2>().0.iter().any(|&[x, y]| {
        x < rectangle.x0() || x > rectangle.x1() || y < rectangle.y0() || y > rectangle.y1()
    }) {
        return Ok(Vec::new());
    }
    context.allocate::<[f64; 8]>(values.len() / 8, source)?;
    Ok(values.as_chunks::<8>().0.to_vec())
}

fn ignore_quads(context: &mut Context<'_>, source: Source) -> Result<()> {
    context.repair(
        repairs::LINK_QUADS,
        "malformed link QuadPoints ignored; Rect used (12.5.6.5, Table 176)",
        source,
    )
}

pub(super) fn read<'a>(document: &'a Document, page: &'a Page) -> ReadResult<Vec<Link<'a>>> {
    document.check_page(page)?;
    let mut context = Context::new(document);
    let source = context.source(page.id())?;
    let Some(annotations) = context.entry(page.dictionary(), b"Annots", source)? else {
        return Ok(Vec::new());
    };
    let annotations = annotations
        .as_array()
        .ok_or_else(|| source.error(Error::syntax("page Annots must be an array")))?;
    let mut links = Vec::new();
    for value in annotations {
        context.step(0, source)?;
        let (dictionary, source) = context.dictionary(value, source)?;
        if context
            .entry(dictionary, b"Subtype", source)?
            .and_then(Value::as_name)
            .is_none_or(|name| name != b"Link")
        {
            continue;
        }
        context.type_name(dictionary, b"Annot", source)?;
        let values = numbers(
            &mut context,
            dictionary
                .get(b"Rect")
                .ok_or_else(|| source.error(Error::syntax("link requires Rect")))?,
            source,
        )?;
        let &[x0, y0, x1, y1] = values.as_slice() else {
            return Err(source
                .error(Error::syntax("link Rect requires four numbers"))
                .into());
        };
        let rectangle = Rect::from_corners(x0, y0, x1, y1)
            .ok_or_else(|| source.error(Error::syntax("invalid link rectangle")))?;
        let quad_points = context
            .entry(dictionary, b"QuadPoints", source)?
            .map(|value| quads(&mut context, value, rectangle, source))
            .transpose()?
            .unwrap_or_default();
        let highlight = match context.entry(dictionary, b"H", source)? {
            Some(value) => value
                .as_name()
                .ok_or(Error::syntax("invalid link highlight mode"))
                .and_then(HighlightMode::try_from)
                .map_err(|error| source.fault(error))?,
            None => HighlightMode::Invert,
        };
        context.allocate::<Link<'_>>(1, source)?;
        links.push(Link {
            rectangle,
            quad_points,
            border: border(&mut context, dictionary, source)?,
            highlight,
            target: destinations::target(&mut context, dictionary, source)?,
            dictionary,
        });
    }
    Ok(links)
}
