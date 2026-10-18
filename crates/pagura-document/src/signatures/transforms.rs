//! Typed inert modification parameters (12.8.2; Tables 236, 256–259).
use super::{FieldLock, FieldSelection, MdpPermission, SignatureReference, Transform};
use crate::navigation::context::{Context, Source};
use pagura_core::{Dictionary, Error, Name, Result, Value};

pub(super) fn name<'a>(d: &'a Dictionary, key: &[u8], source: Source) -> Result<Option<&'a Name>> {
    present(d, key)
        .map(|v| {
            v.as_name()
                .ok_or_else(|| source.error(Error::syntax("signature entry must be a direct name")))
        })
        .transpose()
}
pub(super) fn string<'a>(
    d: &'a Dictionary,
    key: &[u8],
    source: Source,
) -> Result<Option<&'a [u8]>> {
    present(d, key)
        .map(|v| {
            v.as_string().ok_or_else(|| {
                source.error(Error::syntax("signature entry must be a direct string"))
            })
        })
        .transpose()
}
pub(super) fn present<'a>(d: &'a Dictionary, key: &[u8]) -> Option<&'a Value> {
    d.get(key).filter(|v| !v.is_null())
}
fn dictionary(value: &Value, source: Source) -> Result<&Dictionary> {
    value
        .as_dictionary()
        .ok_or_else(|| source.error(Error::syntax("signature entry must be a direct dictionary")))
}
fn type_name(d: &Dictionary, expected: &[u8], source: Source) -> Result<()> {
    if name(d, b"Type", source)?.is_some_and(|n| n.as_bytes() != expected) {
        return Err(source.error(Error::syntax("invalid signature dictionary Type")));
    }
    Ok(())
}
pub(super) fn permission(d: &Dictionary, source: Source) -> Result<Option<MdpPermission>> {
    present(d, b"P")
        .map(|v| match v.as_number() {
            Some(1.0) => Ok(MdpPermission::NoChanges),
            Some(2.0) => Ok(MdpPermission::FormsAndSignatures),
            Some(3.0) => Ok(MdpPermission::FormsSignaturesAndAnnotations),
            _ => Err(source.error(Error::syntax("signature permission must be 1, 2 or 3"))),
        })
        .transpose()
}
fn selection<'a>(
    ctx: &mut Context<'a>,
    d: &'a Dictionary,
    source: Source,
) -> Result<FieldSelection<'a>> {
    let action = name(d, b"Action", source)?
        .ok_or_else(|| source.error(Error::syntax("field lock requires Action")))?;
    let action = action.as_bytes();
    if action == b"All" {
        return Ok(FieldSelection::All);
    }
    if !matches!(action, b"Include" | b"Exclude") {
        return Err(source.error(Error::syntax("unknown field lock Action")));
    }
    let fields = present(d, b"Fields")
        .and_then(Value::as_array)
        .ok_or_else(|| source.error(Error::syntax("field lock requires direct Fields array")))?;
    ctx.allocate::<&[u8]>(fields.len(), source)?;
    let fields = fields
        .iter()
        .map(|v| {
            ctx.step(0, source)?;
            v.as_string().ok_or_else(|| {
                source.error(Error::syntax("locked field name must be a direct string"))
            })
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(if action == b"Include" {
        FieldSelection::Include(fields)
    } else {
        FieldSelection::Exclude(fields)
    })
}
pub(super) fn lock<'a>(
    ctx: &mut Context<'a>,
    d: &'a Dictionary,
    source: Source,
) -> Result<FieldLock<'a>> {
    type_name(d, b"SigFieldLock", source)?;
    Ok(FieldLock {
        dictionary: d,
        fields: selection(ctx, d, source)?,
        permission: permission(d, source)?,
    })
}
pub(super) fn references<'a>(
    ctx: &mut Context<'a>,
    d: &'a Dictionary,
    source: Source,
) -> Result<Vec<SignatureReference<'a>>> {
    let Some(value) = present(d, b"Reference") else {
        return Ok(Vec::new());
    };
    let values = value
        .as_array()
        .ok_or_else(|| source.error(Error::syntax("signature Reference must be a direct array")))?;
    ctx.allocate::<SignatureReference<'_>>(values.len(), source)?;
    values
        .iter()
        .map(|value| reference(ctx, dictionary(value, source)?, source))
        .collect()
}
fn reference<'a>(
    ctx: &mut Context<'a>,
    d: &'a Dictionary,
    source: Source,
) -> Result<SignatureReference<'a>> {
    ctx.step(0, source)?;
    type_name(d, b"SigRef", source)?;
    let method = name(d, b"TransformMethod", source)?.ok_or_else(|| {
        source.error(Error::syntax(
            "signature reference requires TransformMethod",
        ))
    })?;
    let parameters = present(d, b"TransformParams")
        .map(|v| dictionary(v, source))
        .transpose()?;
    if let Some(parameters) = parameters {
        type_name(parameters, b"TransformParams", source)?;
    }
    let data = present(d, b"Data");
    let transform = match method.as_bytes() {
        b"DocMDP" => {
            let p = if let Some(parameters) = parameters {
                version(parameters, b"1.2", source)?;
                permission(parameters, source)?
            } else {
                None
            };
            Transform::DocMdp(p.unwrap_or(MdpPermission::FormsAndSignatures))
        }
        b"FieldMDP" => {
            if data.and_then(Value::as_reference).is_none() {
                return Err(source.error(Error::syntax(
                    "FieldMDP requires an indirect Data reference",
                )));
            }
            let p = parameters
                .ok_or_else(|| source.error(Error::syntax("FieldMDP requires TransformParams")))?;
            version(p, b"1.2", source)?;
            Transform::FieldMdp(selection(ctx, p, source)?)
        }
        b"UR" => Transform::UsageRights,
        _ => Transform::Other(method),
    };
    Ok(SignatureReference {
        dictionary: d,
        transform,
        parameters,
        data,
    })
}
fn version(d: &Dictionary, expected: &[u8], source: Source) -> Result<()> {
    if name(d, b"V", source)?.is_some_and(|n| n.as_bytes() != expected) {
        return Err(source.error(Error::syntax("invalid modification transform version")));
    }
    Ok(())
}
