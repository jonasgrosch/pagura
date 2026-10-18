//! Inherited field entries and shared primitive readers (12.7.4.1).
use super::{AdditionalAction, Justification};
use crate::navigation::{
    actions,
    context::{Context, Source},
    text,
};
use pagura_core::{Dictionary, Error, Result, Value};

#[derive(Clone, Copy)]
pub(super) struct Entry<'a> {
    pub value: &'a Value,
    pub source: Source,
}
#[derive(Clone, Copy, Default)]
pub(super) struct Inherited<'a>([Option<Entry<'a>>; 8]);
const KEYS: [&[u8]; 8] = [b"FT", b"Ff", b"V", b"DV", b"DA", b"Q", b"Opt", b"MaxLen"];
impl<'a> Inherited<'a> {
    pub fn get(&self, key: &[u8]) -> Option<Entry<'a>> {
        KEYS.iter().position(|k| *k == key).and_then(|i| self.0[i])
    }
    pub fn overlay(
        mut self,
        ctx: &mut Context<'a>,
        d: &'a Dictionary,
        source: Source,
    ) -> Result<Self> {
        for (key, entry) in KEYS.into_iter().zip(&mut self.0) {
            if let Some(value) = ctx.present(d, key, source)? {
                *entry = Some(Entry { value, source });
            }
        }
        Ok(self)
    }
    pub fn defaults(ctx: &mut Context<'a>, d: &'a Dictionary, source: Source) -> Result<Self> {
        let mut inherited = Self::default();
        for (i, key) in [(4, b"DA".as_slice()), (5, b"Q")] {
            inherited.0[i] = ctx
                .present(d, key, source)?
                .map(|value| Entry { value, source });
        }
        Ok(inherited)
    }
}
pub(super) fn unsigned<'a>(entry: Entry<'a>, ctx: &mut Context<'a>) -> Result<u32> {
    let (value, source) = ctx.resolve(entry.value, entry.source)?;
    value
        .as_integer()
        .and_then(|v| u32::try_from(v).ok())
        .ok_or_else(|| source.error(Error::syntax("form integer must be unsigned 32-bit")))
}
pub(super) fn appearance<'a>(
    ctx: &mut Context<'a>,
    entry: Option<Entry<'a>>,
) -> Result<Option<&'a [u8]>> {
    entry
        .map(|entry| {
            let (value, source) = ctx.resolve(entry.value, entry.source)?;
            value
                .as_string()
                .ok_or_else(|| source.error(Error::syntax("form DA must be a string")))
        })
        .transpose()
}
pub(super) fn quadding<'a>(
    ctx: &mut Context<'a>,
    entry: Option<Entry<'a>>,
) -> Result<Justification> {
    Ok(
        match entry.map(|e| unsigned(e, ctx)).transpose()?.unwrap_or(0) {
            0 => Justification::Left,
            1 => Justification::Centre,
            2 => Justification::Right,
            _ => {
                return Err(entry.map_or_else(
                    || Error::syntax("invalid form Q"),
                    |e| e.source.error(Error::syntax("invalid form Q")),
                ));
            }
        },
    )
}
pub(super) fn string<'a>(
    ctx: &mut Context<'a>,
    value: &'a Value,
    source: Source,
) -> Result<String> {
    let (value, source) = ctx.resolve(value, source)?;
    text::decode(
        ctx,
        value
            .as_string()
            .ok_or_else(|| source.error(Error::syntax("form text must be a string")))?,
        source,
    )
}
pub(super) fn optional_text<'a>(
    ctx: &mut Context<'a>,
    d: &'a Dictionary,
    key: &[u8],
    source: Source,
) -> Result<Option<String>> {
    ctx.present(d, key, source)?
        .map(|v| string(ctx, v, source))
        .transpose()
}
pub(super) fn additional<'a>(
    ctx: &mut Context<'a>,
    d: &'a Dictionary,
    source: Source,
) -> Result<Vec<AdditionalAction<'a>>> {
    let Some(value) = ctx.present(d, b"AA", source)? else {
        return Ok(Vec::new());
    };
    let (dictionary, source) = ctx.dictionary(value, source)?;
    ctx.allocate::<AdditionalAction<'_>>(dictionary.len(), source)?;
    let mut result = Vec::new();
    for (trigger, value) in dictionary.iter() {
        ctx.step(0, source)?;
        if !ctx.resolve(value, source)?.0.is_null() {
            result.push(AdditionalAction {
                trigger,
                actions: actions::read_with(ctx, value, source)?,
            });
        }
    }
    Ok(result)
}
pub(super) fn array<'a>(
    ctx: &mut Context<'a>,
    value: &'a Value,
    source: Source,
) -> Result<&'a [Value]> {
    let (value, source) = ctx.resolve(value, source)?;
    value
        .as_array()
        .ok_or_else(|| source.error(Error::syntax("form entry must be an array")))
}

/// Preserve malformed names as anonymous source nodes, never synthesize names.
pub(super) fn partial_name<'a>(
    ctx: &mut Context<'a>,
    d: &'a Dictionary,
    source: Source,
) -> Result<Option<String>> {
    let Some(value) = ctx.entry(d, b"T", source)? else {
        return Ok(None);
    };
    if let Some(bytes) = value.as_string() {
        return text::decode(ctx, bytes, source).map(Some);
    }
    ctx.repair(
        crate::tags::FORM_NAME,
        "wrong-typed field name kept as an anonymous node (12.7.4.2)",
        source,
    )?;
    Ok(None)
}
