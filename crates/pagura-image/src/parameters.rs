//! Image dictionary validation (ISO 32000-2, 8.9.5, Tables 87–88).
use crate::Samples;
use pagura_core::convert::u32_to_usize;
use pagura_core::{Diagnostic, Diagnostics, Dictionary, Error, Limits, Position, Result, Value};
use pagura_document::Document;

pub(crate) fn resolved_dictionary(
    document: &Document,
    dict: &Dictionary,
    diagnostics: &mut Diagnostics,
) -> Result<Dictionary> {
    let stencil = stencil_space(document, dict, diagnostics)?;
    let filter = dict
        .get(b"Filter")
        .map(|v| document.resolve(v))
        .transpose()?;
    let last = match filter {
        Some(Value::Array(values)) => values.last(),
        value => value,
    };
    let jpx = last
        .map(|v| document.resolve(v))
        .transpose()?
        .and_then(Value::as_name)
        .is_some_and(|name| name == b"JPXDecode");
    dict.iter()
        .filter(|(key, _)| !(stencil && key.as_bytes() == b"ColorSpace"))
        .map(|(key, value)| {
            let resolved = if matches!(
                key.as_bytes(),
                b"Width"
                    | b"Height"
                    | b"BitsPerComponent"
                    | b"ColorSpace"
                    | b"ImageMask"
                    | b"Interpolate"
                    | b"SMaskInData"
            ) && !(jpx && key.as_bytes() == b"BitsPerComponent")
                && (jpx || key.as_bytes() != b"SMaskInData")
            {
                document.resolve(value)?
            } else {
                value
            };
            Ok((key.clone(), resolved.clone()))
        })
        .collect()
}

/// Ignore only the colour entry; finish() still enforces every stencil constraint.
pub(crate) fn stencil_space(
    document: &Document,
    dict: &Dictionary,
    diagnostics: &mut Diagnostics,
) -> Result<bool> {
    let stencil = dict
        .get(b"ImageMask")
        .map(|v| document.resolve(v))
        .transpose()?
        .and_then(Value::as_bool)
        == Some(true);
    if stencil
        && dict
            .get(b"ColorSpace")
            .is_some_and(|v| !document.resolve(v).is_ok_and(Value::is_null))
    {
        diagnostics.repair(Diagnostic::new(
            crate::repairs::STENCIL_COLOR_SPACE,
            "extraneous stencil ColorSpace ignored (8.9.5.1, Table 87)",
        ))?;
    }
    Ok(stencil)
}

pub(crate) fn integer(dict: &Dictionary, key: &[u8], default: i64) -> Result<i64> {
    match dict.get(key) {
        None | Some(Value::Null) => Ok(default),
        Some(Value::Integer(value)) => Ok(*value),
        _ => Err(Error::syntax("image parameter must be an integer")),
    }
}

pub(crate) fn boolean(dict: &Dictionary, key: &[u8], default: bool) -> Result<bool> {
    match dict.get(key) {
        None | Some(Value::Null) => Ok(default),
        Some(Value::Boolean(value)) => Ok(*value),
        _ => Err(Error::syntax("image parameter must be a boolean")),
    }
}

pub(crate) fn dimensions(dict: &Dictionary) -> Result<(u32, u32)> {
    let dimension = |key| {
        let value = integer(dict, key, 0)?;
        if value <= 0 {
            return Err(Error::syntax("invalid image dimensions"));
        }
        u32::try_from(value)
            .map_err(|_| Error::limit("image dimensions exceed addressable samples"))
    };
    Ok((dimension(b"Width")?, dimension(b"Height")?))
}

pub(crate) fn budget(
    width: u32,
    height: u32,
    components: u8,
    bits: u8,
    limits: &Limits,
) -> Result<usize> {
    pagura_imagecodec::sample_count(width, height, components, bits, limits)
}

pub(crate) fn components(space: &Value) -> Result<u8> {
    if let Some(name) = space.as_name() {
        return match name.as_bytes() {
            b"DeviceGray" | b"CalGray" => Ok(1),
            b"DeviceRGB" | b"CalRGB" | b"Lab" => Ok(3),
            b"DeviceCMYK" => Ok(4),
            _ => Err(Error::syntax("unknown image colour space")),
        };
    }
    let array = space
        .as_array()
        .ok_or(Error::syntax("invalid image colour space"))?;
    match array
        .first()
        .and_then(Value::as_name)
        .map(pagura_core::Name::as_bytes)
    {
        // 8.6.3 permits device spaces as names or one-name arrays.
        Some(b"DeviceGray" | b"DeviceRGB" | b"DeviceCMYK") if array.len() == 1 => {
            components(&array[0])
        }
        Some(b"Indexed" | b"Separation" | b"CalGray") => Ok(1),
        Some(b"CalRGB" | b"Lab") => Ok(3),
        Some(b"DeviceN") => array
            .get(1)
            .and_then(Value::as_array)
            .and_then(|names| u8::try_from(names.len()).ok())
            .filter(|&n| n > 0)
            .ok_or(Error::syntax("invalid DeviceN components")),
        Some(b"ICCBased") => Err(Error::syntax("unresolved ICC profile")),
        Some(b"Pattern") => Err(Error::syntax("Pattern is not an image colour space")),
        _ => Err(Error::syntax("invalid image colour space family")),
    }
}

pub(crate) fn resolve_space(document: &Document, space: &Value) -> Result<(Value, u8)> {
    let space = document.resolve(space)?;
    if let Some(array) = space.as_array() {
        let family = array
            .first()
            .and_then(Value::as_name)
            .ok_or(Error::syntax("invalid colour space"))?;
        if family == b"ICCBased" {
            let profile = array.get(1).ok_or(Error::syntax("missing ICC profile"))?;
            let dict = document.resolve_dictionary(profile)?;
            let n = u8::try_from(integer(dict, b"N", 0)?)
                .ok()
                .filter(|n| matches!(n, 1 | 3 | 4))
                .ok_or(Error::syntax("invalid ICC component count"))?;
            return Ok((space.clone(), n));
        }
    }
    Ok((space.clone(), components(space)?))
}

pub(crate) fn unpack_with_diagnostics(
    data: &[u8],
    width: u32,
    height: u32,
    components: u8,
    bits: u8,
    limits: &Limits,
    diagnostics: &mut Diagnostics,
) -> Result<Samples> {
    let count = budget(width, height, components, bits, limits)?;
    let row_values = usize::try_from(u64::from(width) * u64::from(components))
        .map_err(|_| Error::limit("image row overflow"))?;
    let row_bytes = row_values
        .checked_mul(usize::from(bits))
        .and_then(|v| v.checked_add(7))
        .map(|v| v / 8)
        .ok_or(Error::limit("image row overflow"))?;
    let length = row_bytes
        .checked_mul(u32_to_usize(height))
        .ok_or(Error::limit("image size overflow"))?;
    let data = packed_data(data, length, diagnostics)?;
    if bits == 16 {
        let mut values = Vec::with_capacity(count);
        values.extend(
            data.as_chunks::<2>()
                .0
                .iter()
                .map(|&bytes| u16::from_be_bytes(bytes)),
        );
        if data.len() % 2 != 0 {
            values.push(u16::from(data[data.len() - 1]) << 8);
        }
        values.resize(count, 0);
        return Ok(Samples::Words(values));
    }
    let mut values = vec![0; count];
    if bits == 8 {
        values[..data.len()].copy_from_slice(data);
    } else {
        let per_byte = 8 / usize::from(bits);
        let mask = 255 >> (8 - bits);
        for (row, output) in data
            .chunks(row_bytes)
            .zip(values.chunks_exact_mut(row_values))
        {
            for (&byte, group) in row.iter().zip(output.chunks_mut(per_byte)) {
                for (value, shift) in group.iter_mut().zip((0..per_byte).rev()) {
                    *value = (byte >> (shift * usize::from(bits))) & mask;
                }
            }
        }
    }
    Ok(Samples::Bytes(values))
}

/// Check packed row length without allocating missing bytes. Callers supply
/// zero for a missing byte and must ignore unused bits at each row's end.
pub(crate) fn packed_data<'a>(
    data: &'a [u8],
    length: usize,
    diagnostics: &mut Diagnostics,
) -> Result<&'a [u8]> {
    if data.len() != length {
        let (tag, message) = if data.len() < length {
            (
                crate::repairs::DATA_SHORT,
                "short image data padded with zero bytes (8.9.3, 8.9.5.1)",
            )
        } else {
            (
                crate::repairs::DATA_EXCESS,
                "excess image data ignored (8.9.3, 8.9.5.1)",
            )
        };
        diagnostics
            .repair(Diagnostic::new(tag, message).at(Position::Buffer(data.len().min(length))))?;
    }
    Ok(&data[..data.len().min(length)])
}

/// The `/Decode` array of an image with `components` components: two
/// finite numbers per component (8.9.5.2, Table 87).
pub(crate) fn decode_array(
    document: &Document,
    dict: &Dictionary,
    components: u8,
) -> Result<Option<Vec<f64>>> {
    let Some(value) = dict.get(b"Decode").filter(|v| !v.is_null()) else {
        return Ok(None);
    };
    let decode = document
        .resolve(value)?
        .as_array()
        .ok_or(Error::syntax("Decode must be an array"))?;
    if decode.len() != usize::from(components) * 2 {
        return Err(Error::syntax("invalid Decode length"));
    }
    decode
        .iter()
        .map(|v| {
            document
                .resolve(v)?
                .as_number()
                .filter(|v| v.is_finite())
                .ok_or(Error::syntax("invalid Decode value"))
        })
        .collect::<Result<_>>()
        .map(Some)
}

#[cfg(test)]
mod tests {
    use crate::Samples;
    use pagura_core::{Limits, Recovery};

    #[test]
    fn zero_byte_fill_preserves_the_high_byte_of_an_incomplete_word() {
        let mut diagnostics = pagura_core::Diagnostics::new(Recovery::Repair);
        let samples = super::unpack_with_diagnostics(
            &[0x12, 0x34, 0xab],
            2,
            1,
            1,
            16,
            &Limits::default(),
            &mut diagnostics,
        )
        .unwrap();
        assert_eq!(samples, Samples::Words(vec![0x1234, 0xab00]));
        assert_eq!(
            diagnostics.as_slice()[0].repair(),
            Some(crate::repairs::DATA_SHORT)
        );
    }
}
