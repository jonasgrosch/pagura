//! Widget source data and appearance-state names (12.5.6.19, 12.7.5.2).
use super::{Widget, attributes};
use crate::{
    navigation::actions,
    repairs,
    view::{Context, Source, number},
};
use pagura_core::{Dictionary, Error, Name, Rect, Result};

/// Whether a widget has its own dictionary or shares its field's (12.7.4.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Representation {
    Separate,
    Merged,
}

pub(super) fn read<'a>(
    context: &mut Context<'a>,
    dictionary: &'a Dictionary,
    source: Source,
    field: usize,
    representation: Representation,
) -> Result<Widget<'a>> {
    let merged = representation == Representation::Merged;
    if !is_widget(context, dictionary, source)? {
        context.repair(
            repairs::FORM_STRUCTURE,
            "unnamed field child read as Widget (12.7.4.2)",
            source,
        )?;
    }
    context.allocate::<Widget<'_>>(1, source)?;
    Ok(Widget {
        field,
        object: source.object,
        position: source.position,
        merged,
        rectangle: rectangle(context, dictionary, source)?,
        on_states: on_states(context, dictionary, source)?,
        appearance_state: context
            .entry(dictionary, b"AS", source)?
            .map(|v| {
                v.as_name()
                    .ok_or_else(|| source.error(Error::syntax("widget AS must be a name")))
            })
            .transpose()?,
        action: context
            .present(dictionary, b"A", source)?
            .map(|v| actions::read_with(context, v, source))
            .transpose()?,
        additional_actions: if merged {
            Vec::new()
        } else {
            attributes::additional(context, dictionary, source)?
        },
        dictionary,
    })
}

fn rectangle<'a>(
    context: &mut Context<'a>,
    dictionary: &'a Dictionary,
    source: Source,
) -> Result<Rect> {
    let value = context.required(dictionary, b"Rect", source)?;
    let values = attributes::array(context, value, source)?;
    let [x0, y0, x1, y1] = values else {
        return Err(source.error(Error::syntax("widget Rect requires four numbers")));
    };
    let mut read = |v| context.resolve(v, source).and_then(|(v, s)| number(v, s));
    let (x0, y0, x1, y1) = (read(x0)?, read(y0)?, read(x1)?, read(y1)?);
    Rect::from_corners(x0, y0, x1, y1)
        .ok_or_else(|| source.error(Error::syntax("invalid widget Rect")))
}

fn on_states<'a>(
    context: &mut Context<'a>,
    dictionary: &'a Dictionary,
    source: Source,
) -> Result<Vec<&'a Name>> {
    let Some(ap) = context.present(dictionary, b"AP", source)? else {
        return Ok(Vec::new());
    };
    let (ap, source) = context.dictionary(ap, source)?;
    let Some(normal) = context.present(ap, b"N", source)? else {
        return Ok(Vec::new());
    };
    let (value, source) = context.resolve(normal, source)?;
    if source
        .object
        .map(|id| context.document.object(id))
        .transpose()?
        .is_some_and(|o| std::ptr::eq(o.value(), value) && o.stream_range().is_some())
    {
        return Ok(Vec::new());
    }
    let dictionary = value.as_dictionary().ok_or_else(|| {
        source.error(Error::syntax(
            "widget AP N must be a stream or state dictionary",
        ))
    })?;
    context.allocate::<&Name>(dictionary.len(), source)?;
    let mut states = Vec::new();
    for (key, _) in dictionary {
        context.step(0, source)?;
        if key.as_bytes() != b"Off" {
            states.push(key);
        }
    }
    Ok(states)
}

pub(super) fn is_widget<'a>(
    context: &mut Context<'a>,
    dictionary: &'a Dictionary,
    source: Source,
) -> Result<bool> {
    Ok(context
        .entry(dictionary, b"Subtype", source)?
        .and_then(|v| v.as_name())
        .is_some_and(|n| n.as_bytes() == b"Widget"))
}
