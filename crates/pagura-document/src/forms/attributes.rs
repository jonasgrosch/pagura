//! Inherited field entries and shared primitive readers (12.7.4.1).
use super::{AdditionalAction, Justification};
use crate::navigation::actions;
use crate::source::ReadResult;
use crate::view::{Context, Source};
use pagura_core::{Dictionary, Error, Result, Value};

#[derive(Clone, Copy)]
pub(super) struct Entry<'a> {
    pub value: &'a Value,
    pub source: Source,
}

/// The inheritable field entries (Tables 226, 228, 230 and 232).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Key {
    Ft,
    Ff,
    V,
    Dv,
    Da,
    Q,
    Opt,
    MaxLen,
}

impl Key {
    const ALL: [Self; 8] = [
        Self::Ft,
        Self::Ff,
        Self::V,
        Self::Dv,
        Self::Da,
        Self::Q,
        Self::Opt,
        Self::MaxLen,
    ];

    const fn name(self) -> &'static [u8] {
        match self {
            Self::Ft => b"FT",
            Self::Ff => b"Ff",
            Self::V => b"V",
            Self::Dv => b"DV",
            Self::Da => b"DA",
            Self::Q => b"Q",
            Self::Opt => b"Opt",
            Self::MaxLen => b"MaxLen",
        }
    }
}

/// The nearest definition of each inheritable entry (12.7.4.1).
#[derive(Clone, Copy, Default)]
pub(super) struct Inherited<'a>([Option<Entry<'a>>; Key::ALL.len()]);

impl<'a> Inherited<'a> {
    pub(super) const fn get(&self, key: Key) -> Option<Entry<'a>> {
        self.0[key as usize]
    }

    pub(super) fn overlay(
        mut self,
        context: &mut Context<'a>,
        dictionary: &'a Dictionary,
        source: Source,
    ) -> ReadResult<Self> {
        for (key, entry) in Key::ALL.into_iter().zip(&mut self.0) {
            if let Some(value) = context.present(dictionary, key.name(), source)? {
                *entry = Some(Entry { value, source });
            }
        }
        Ok(self)
    }

    pub(super) fn defaults(
        context: &mut Context<'a>,
        dictionary: &'a Dictionary,
        source: Source,
    ) -> ReadResult<Self> {
        let mut inherited = Self::default();
        // The AcroForm dictionary supplies document-wide DA and Q (Table 224).
        for key in [Key::Da, Key::Q] {
            inherited.0[key as usize] = context
                .present(dictionary, key.name(), source)?
                .map(|value| Entry { value, source });
        }
        Ok(inherited)
    }
}

pub(super) fn unsigned<'a>(entry: Entry<'a>, context: &mut Context<'a>) -> ReadResult<u32> {
    let (value, source) = context.resolve(entry.value, entry.source)?;
    value
        .as_integer()
        .and_then(|v| u32::try_from(v).ok())
        .ok_or_else(|| source.error(Error::syntax("form integer must be unsigned 32-bit")))
}

pub(super) fn appearance<'a>(
    context: &mut Context<'a>,
    entry: Option<Entry<'a>>,
) -> ReadResult<Option<&'a [u8]>> {
    entry
        .map(|entry| {
            let (value, source) = context.resolve(entry.value, entry.source)?;
            value
                .as_string()
                .ok_or_else(|| source.error(Error::syntax("form DA must be a string")))
        })
        .transpose()
}

pub(super) fn quadding<'a>(
    context: &mut Context<'a>,
    entry: Option<Entry<'a>>,
) -> ReadResult<Justification> {
    Ok(
        match entry
            .map(|e| unsigned(e, context))
            .transpose()?
            .unwrap_or(0)
        {
            0 => Justification::Left,
            1 => Justification::Center,
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
    context: &mut Context<'a>,
    value: &'a Value,
    source: Source,
) -> ReadResult<String> {
    let (value, source) = context.resolve(value, source)?;
    context.text(
        value
            .as_string()
            .ok_or_else(|| source.error(Error::syntax("form text must be a string")))?,
        source,
    )
}

pub(super) fn optional_text<'a>(
    context: &mut Context<'a>,
    dictionary: &'a Dictionary,
    key: &[u8],
    source: Source,
) -> ReadResult<Option<String>> {
    context
        .present(dictionary, key, source)?
        .map(|v| string(context, v, source))
        .transpose()
}

pub(super) fn additional<'a>(
    context: &mut Context<'a>,
    dictionary: &'a Dictionary,
    source: Source,
) -> ReadResult<Vec<AdditionalAction<'a>>> {
    let Some(value) = context.present(dictionary, b"AA", source)? else {
        return Ok(Vec::new());
    };
    let (triggers, source) = context.dictionary(value, source)?;
    context.allocate::<AdditionalAction<'_>>(triggers.len(), source)?;
    let mut result = Vec::new();
    for (trigger, value) in triggers {
        context.step(0, source)?;
        if !context.resolve(value, source)?.0.is_null() {
            result.push(AdditionalAction {
                trigger,
                actions: actions::read_with(context, value, source)?,
            });
        }
    }
    Ok(result)
}

pub(super) fn array<'a>(
    context: &mut Context<'a>,
    value: &'a Value,
    source: Source,
) -> ReadResult<&'a [Value]> {
    let (value, source) = context.resolve(value, source)?;
    value
        .as_array()
        .ok_or_else(|| source.error(Error::syntax("form entry must be an array")))
}

/// Preserve malformed names as anonymous source nodes, never synthesize names.
pub(super) fn partial_name<'a>(
    context: &mut Context<'a>,
    dictionary: &'a Dictionary,
    source: Source,
) -> ReadResult<Option<String>> {
    let Some(value) = context.entry(dictionary, b"T", source)? else {
        return Ok(None);
    };
    if let Some(bytes) = value.as_string() {
        return context.text(bytes, source).map(Some);
    }
    context.repair(
        crate::repairs::FORM_NAME,
        "wrong-typed field name kept as an anonymous node (12.7.4.2)",
        source,
    )?;
    Ok(None)
}
