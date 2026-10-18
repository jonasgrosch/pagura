//! Widget source data and appearance-state names (12.5.6.19, 12.7.5.2).
use super::{Widget, attributes};
use crate::{
    navigation::actions,
    repairs,
    view::{Context, Source, number},
};
use pagura_core::{Dictionary, Error, Name, Rect, Result};

pub(super) fn read<'a>(
    ctx: &mut Context<'a>,
    d: &'a Dictionary,
    source: Source,
    field: usize,
    merged: bool,
) -> Result<Widget<'a>> {
    if !is_widget(ctx, d, source)? {
        ctx.repair(
            repairs::FORM_STRUCTURE,
            "unnamed field child read as Widget (12.7.4.2)",
            source,
        )?;
    }
    ctx.allocate::<Widget<'_>>(1, source)?;
    Ok(Widget {
        field,
        object: source.object,
        position: source.position,
        merged,
        rectangle: rectangle(ctx, d, source)?,
        on_states: on_states(ctx, d, source)?,
        appearance_state: ctx
            .entry(d, b"AS", source)?
            .map(|v| {
                v.as_name()
                    .ok_or_else(|| source.error(Error::syntax("widget AS must be a name")))
            })
            .transpose()?,
        action: ctx
            .present(d, b"A", source)?
            .map(|v| actions::read_with(ctx, v, source))
            .transpose()?,
        additional_actions: if merged {
            Vec::new()
        } else {
            attributes::additional(ctx, d, source)?
        },
        dictionary: d,
    })
}
fn rectangle<'a>(ctx: &mut Context<'a>, d: &'a Dictionary, source: Source) -> Result<Rect> {
    let value = ctx.required(d, b"Rect", source)?;
    let values = attributes::array(ctx, value, source)?;
    let [x0, y0, x1, y1] = values else {
        return Err(source.error(Error::syntax("widget Rect requires four numbers")));
    };
    let mut read = |v| ctx.resolve(v, source).and_then(|(v, s)| number(v, s));
    let (x0, y0, x1, y1) = (read(x0)?, read(y0)?, read(x1)?, read(y1)?);
    Rect::from_corners(x0, y0, x1, y1)
        .ok_or_else(|| source.error(Error::syntax("invalid widget Rect")))
}
fn on_states<'a>(
    ctx: &mut Context<'a>,
    d: &'a Dictionary,
    source: Source,
) -> Result<Vec<&'a Name>> {
    let Some(ap) = ctx.present(d, b"AP", source)? else {
        return Ok(Vec::new());
    };
    let (ap, source) = ctx.dictionary(ap, source)?;
    let Some(normal) = ctx.present(ap, b"N", source)? else {
        return Ok(Vec::new());
    };
    let (value, source) = ctx.resolve(normal, source)?;
    if source
        .object
        .map(|id| ctx.doc.object(id))
        .transpose()?
        .is_some_and(|o| std::ptr::eq(o.value(), value) && o.stream_range().is_some())
    {
        return Ok(Vec::new());
    }
    let d = value.as_dictionary().ok_or_else(|| {
        source.error(Error::syntax(
            "widget AP N must be a stream or state dictionary",
        ))
    })?;
    ctx.allocate::<&Name>(d.len(), source)?;
    let mut states = Vec::new();
    for (key, _) in d {
        ctx.step(0, source)?;
        if key.as_bytes() != b"Off" {
            states.push(key);
        }
    }
    Ok(states)
}

pub(super) fn is_widget<'a>(
    ctx: &mut Context<'a>,
    d: &'a Dictionary,
    source: Source,
) -> Result<bool> {
    Ok(ctx
        .entry(d, b"Subtype", source)?
        .and_then(|v| v.as_name())
        .is_some_and(|n| n.as_bytes() == b"Widget"))
}
