//! Dictionary/filter consistency (8.9.5.1 Table 87 and 8.9.7 Table 92).
use pagura_core::{Dictionary, Error, Result, Value};
use pagura_document::Document;

pub(crate) fn validate(document: &Document, dict: &Dictionary, inline: bool) -> Result<()> {
    let filter = dict
        .get(b"Filter")
        .map(|v| document.resolve(v))
        .transpose()?;
    let filters = match filter {
        None | Some(Value::Null) => return Ok(()),
        Some(Value::Array(values)) => values.as_slice(),
        Some(value) => std::slice::from_ref(value),
    };
    for value in filters {
        let name = document
            .resolve(value)?
            .as_name()
            .ok_or(Error::syntax("invalid image Filter"))?;
        if !inline
            && matches!(
                name.as_bytes(),
                b"AHx" | b"A85" | b"LZW" | b"Fl" | b"RL" | b"CCF" | b"DCT"
            )
        {
            return Err(Error::syntax(
                "filter abbreviations are restricted to inline images",
            ));
        }
    }
    if filters
        .last()
        .map(|v| document.resolve(v))
        .transpose()?
        .and_then(Value::as_name)
        .is_some_and(|n| matches!(n.as_bytes(), b"RunLengthDecode" | b"RL"))
        && super::parameters::integer(dict, b"BitsPerComponent", 0)? != 8
    {
        return Err(Error::syntax(
            "RunLength images require eight-bit components",
        ));
    }
    Ok(())
}

pub(crate) fn inline_space(value: Option<&Value>) -> Result<()> {
    let Some(value) = value.filter(|v| !v.is_null()) else {
        return Ok(());
    };
    // Named resources are resolved by the caller; direct arrays have only
    // the limited Indexed form admitted by 8.9.7.
    if value.as_name().is_some() {
        return Ok(());
    }
    let array = value
        .as_array()
        .ok_or(Error::syntax("invalid inline ColorSpace"))?;
    let device = |v: &Value| {
        v.as_name()
            .is_some_and(|n| matches!(n.as_bytes(), b"DeviceGray" | b"DeviceRGB" | b"DeviceCMYK"))
    };
    if array.len() != 4
        || array[0].as_name().is_none_or(|n| n != b"Indexed")
        || !device(&array[1])
        || !array[2]
            .as_integer()
            .is_some_and(|v| (0..=255).contains(&v))
        || array[3].as_string().is_none()
    {
        return Err(Error::syntax(
            "direct inline colour space must be limited Indexed",
        ));
    }
    Ok(())
}
