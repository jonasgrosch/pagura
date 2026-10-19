//! Inline image abbreviations and data boundaries (8.9.7, Tables 91–92).
use crate::dispatch::{self, Codec};
use crate::filter_rules::{self, Source};
use crate::{DecodedImage, parameters as p};
use pagura_codec::{Filter, FilterChain, ImageCodec};
use pagura_core::convert::usize_to_u64;
use pagura_core::{
    Diagnostics, Dictionary, Error, ErrorKind, Limits, Name, Result, Value, syntax::is_whitespace,
};
use pagura_document::Document;

pub(crate) fn expanded(dictionary: &Dictionary) -> Result<Dictionary> {
    let mut expanded = Dictionary::new();
    for (key, value) in dictionary {
        let key: &[u8] = match key.as_bytes() {
            b"BPC" | b"BitsPerComponent" => b"BitsPerComponent",
            b"CS" | b"ColorSpace" => b"ColorSpace",
            b"D" | b"Decode" => b"Decode",
            b"DP" | b"DecodeParms" => b"DecodeParms",
            b"F" | b"Filter" => b"Filter",
            b"H" | b"Height" => b"Height",
            b"IM" | b"ImageMask" => b"ImageMask",
            b"I" | b"Interpolate" => b"Interpolate",
            b"L" | b"Length" => b"Length",
            b"W" | b"Width" => b"Width",
            b"Intent" => b"Intent",
            _ => continue,
        };
        let value = if key == b"ColorSpace" {
            expand_space(value)
        } else {
            value.clone()
        };
        if expanded.insert(Name::new(key), value).is_some() {
            return Err(Error::syntax("conflicting inline image abbreviations"));
        }
    }
    Ok(expanded)
}

fn expand_space(value: &Value) -> Value {
    match value {
        Value::Name(name) => Value::name(match name.as_bytes() {
            b"G" => b"DeviceGray",
            b"RGB" => b"DeviceRGB",
            b"CMYK" => b"DeviceCMYK",
            b"I" => b"Indexed",
            name => name,
        }),
        Value::Array(values) => Value::Array(values.iter().map(expand_space).collect()),
        _ => value.clone(),
    }
}

pub(crate) fn decode(
    document: &Document,
    resources: &Dictionary,
    dictionary: &Dictionary,
    data: &[u8],
) -> Result<DecodedImage> {
    let mut diagnostics = Diagnostics::new(document.recovery());
    let result = decode_data(document, resources, dictionary, data, &mut diagnostics);
    // Positions are offsets in the inline data, or unknown for faults of the
    // dictionary; the caller places them in its content stream.
    document.report(diagnostics.take());
    result
}

fn decode_data(
    document: &Document,
    resources: &Dictionary,
    dictionary: &Dictionary,
    data: &[u8],
    diagnostics: &mut Diagnostics,
) -> Result<DecodedImage> {
    let mut dict = expanded(dictionary)?;
    if p::stencil_space(document, &dict, diagnostics)? {
        dict.remove(b"ColorSpace");
    }
    filter_rules::inline_space(dict.get(b"ColorSpace"))?;
    filter_rules::validate(document, &dict, Source::Inline, diagnostics)?;
    if let Some(Value::Name(name)) = dict.get(b"ColorSpace")
        && !matches!(
            name.as_bytes(),
            b"DeviceGray" | b"DeviceRGB" | b"DeviceCMYK"
        )
    {
        let spaces = resources
            .get(b"ColorSpace")
            .ok_or(Error::syntax("missing inline colour resource"))?;
        let space = document
            .resolve_dictionary(spaces)?
            .get(name.as_bytes())
            .ok_or(Error::syntax("undefined inline colour space"))?;
        dict.insert(Name::new(b"ColorSpace"), document.resolve(space)?.clone());
    }
    let (w, h) = p::dimensions(&dict)?;
    p::budget(w, h, 1, 1, &document.limits())?;
    let mut chain = FilterChain::from_entries(dict.get(b"Filter"), dict.get(b"DecodeParms"))?;
    if chain.filters().iter().any(|f| {
        matches!(
            f,
            Filter::Crypt(_)
                | Filter::Image {
                    codec: ImageCodec::Jbig2 | ImageCodec::Jpx,
                    ..
                }
        )
    }) {
        return Err(Error::syntax("filter is forbidden in inline images"));
    }
    let codec = chain.split_image();
    let decoded = chain.decode(data, document.limits().decoded_bytes, diagnostics)?;
    let codec = codec.as_ref().map(|(codec, params)| Codec {
        codec: *codec,
        params: params.as_ref(),
    });
    dispatch::decode(document, &dict, &decoded, codec, diagnostics)
}

pub(crate) fn data_length(
    dictionary: &Dictionary,
    content: &[u8],
    limits: &Limits,
) -> Result<usize> {
    let dict = expanded(dictionary)?;
    if let Some(value) = dict.get(b"Length") {
        let last_filter = match dict.get(b"Filter") {
            Some(Value::Array(filters)) => filters.last(),
            value => value,
        };
        let skip = if last_filter.and_then(Value::as_name).is_some_and(|name| {
            matches!(
                name.as_bytes(),
                b"ASCIIHexDecode" | b"AHx" | b"ASCII85Decode" | b"A85"
            )
        }) {
            content
                .iter()
                .position(|&byte| !is_whitespace(byte))
                .unwrap_or(content.len())
        } else {
            0
        };
        let length = value
            .as_integer()
            .and_then(|v| usize::try_from(v).ok())
            .and_then(|length| length.checked_add(skip))
            .filter(|&length| length <= content.len())
            .ok_or(Error::syntax("invalid inline image Length"))?;
        return Ok(length);
    }
    // Table 5 permits an array of zero filters. Its inline equivalent (8.9.7)
    // is unencoded data, just like an absent or null Filter; no repair applies.
    if dict
        .get(b"Filter")
        .is_none_or(|value| value.is_null() || value.as_array().is_some_and(<[_]>::is_empty))
    {
        let (width, height) = p::dimensions(&dict)?;
        let stencil = p::boolean(&dict, b"ImageMask", false)?;
        if !stencil
            && dict
                .get(b"ColorSpace")
                .and_then(Value::as_name)
                .is_some_and(|n| {
                    !matches!(n.as_bytes(), b"DeviceGray" | b"DeviceRGB" | b"DeviceCMYK")
                })
        {
            return Err(Error::unsupported(
                crate::features::INLINE_COLOR_LENGTH,
                "inline image Length requires resource colour-space resolution",
            ));
        }
        let n = if stencil {
            1
        } else {
            p::components(
                dict.get(b"ColorSpace")
                    .ok_or(Error::syntax("missing inline ColorSpace"))?,
            )?
        };
        let bits = p::integer(&dict, b"BitsPerComponent", i64::from(stencil))?;
        if !matches!(bits, 1 | 2 | 4 | 8 | 16) {
            return Err(Error::syntax("invalid inline BitsPerComponent"));
        }
        let length = u64::from(width)
            .checked_mul(u64::from(n))
            .and_then(|v| v.checked_mul(u64::try_from(bits).ok()?))
            .map(|v| v.div_ceil(8))
            .and_then(|v| v.checked_mul(u64::from(height)))
            .and_then(|v| usize::try_from(v).ok())
            .filter(|&v| v <= content.len())
            .ok_or(Error::syntax("truncated inline image data"))?;
        return Ok(length);
    }
    if let Some(length) = filter_boundary(&dict, content)? {
        return Ok(length);
    }
    filtered_length(&dict, content, limits)
}

fn filtered_length(dict: &Dictionary, content: &[u8], limits: &Limits) -> Result<usize> {
    let mut chain = FilterChain::from_entries(dict.get(b"Filter"), dict.get(b"DecodeParms"))?;
    let codec = chain.split_image();
    if chain.take_crypt().is_some()
        || matches!(codec, Some((ImageCodec::Jbig2 | ImageCodec::Jpx, _)))
    {
        return Err(Error::syntax("filter is forbidden in inline images"));
    }
    // Legacy binary filters without consumed-byte APIs: verify candidate
    // boundaries using strict decoding. Sixteen attempts bound total work by
    // a constant multiple of the content size; never guess a damaged prefix.
    let mut attempts = 0;
    for index in 0..content.len().saturating_sub(2) {
        if is_whitespace(content[index])
            && &content[index + 1..index + 3] == b"EI"
            && content
                .get(index + 3)
                .is_none_or(|&b| is_whitespace(b) || pagura_core::syntax::is_delimiter(b))
        {
            attempts += 1;
            if attempts > 16 {
                return Err(Error::limit("inline image boundary search budget exceeded"));
            }
            if valid_prefix(&chain, codec.as_ref(), dict, &content[..index], limits)? {
                return Ok(index);
            }
        }
    }
    Err(Error::syntax("missing or invalid inline image terminator"))
}

/// Whether `data` decodes completely as the inline image: a strict decode
/// that fails means the candidate boundary is wrong, but exceeding a budget
/// is reported, not taken as an answer.
fn valid_prefix(
    chain: &FilterChain,
    codec: Option<&(ImageCodec, Option<Dictionary>)>,
    dict: &Dictionary,
    data: &[u8],
    limits: &Limits,
) -> Result<bool> {
    let candidate = |result: Result<bool>| match result {
        Err(error) if error.kind == ErrorKind::Limit => Err(error),
        result => Ok(result.unwrap_or(false)),
    };
    let mut diagnostics = Diagnostics::new(pagura_core::Recovery::Strict);
    let decoded = match chain.decode(data, limits.decoded_bytes, &mut diagnostics) {
        Ok(decoded) => decoded,
        Err(error) => return candidate(Err(error)),
    };
    match codec {
        None => Ok(true),
        Some((ImageCodec::Dct, params)) => {
            let transform = params
                .as_ref()
                .and_then(|p| p.get(b"ColorTransform"))
                .and_then(Value::as_integer);
            candidate(
                pagura_imagecodec::dct::decode(&decoded, transform, limits)
                    .map(|image| p::dimensions(dict) == Ok((image.width, image.height))),
            )
        }
        Some((ImageCodec::CcittFax, params)) => {
            let (Ok(params), Ok((_, height))) = (
                crate::adapter::fax::params(params.as_ref()),
                p::dimensions(dict),
            ) else {
                return Ok(false);
            };
            // Framing follows the encoded fax rows, not the eventual image
            // grid. decode_inline reports any Columns/Width repair later.
            let row = u64::from(params.columns.get()).div_ceil(8);
            candidate(
                pagura_imagecodec::ccitt::decode(&decoded, &params, limits)
                    .map(|bytes| usize_to_u64(bytes.len()) == row * u64::from(height)),
            )
        }
        _ => Ok(false),
    }
}

fn filter_boundary(dict: &Dictionary, data: &[u8]) -> Result<Option<usize>> {
    let value = dict.get(b"Filter");
    let first = match value {
        Some(Value::Array(values)) => values.first(),
        value => value,
    };
    let name = first
        .and_then(Value::as_name)
        .ok_or(Error::syntax("invalid inline Filter"))?;
    let length = match name.as_bytes() {
        b"ASCIIHexDecode" | b"AHx" => Some(
            data.iter()
                .position(|&b| b == b'>')
                .ok_or(Error::syntax("missing ASCIIHex terminator"))?
                + 1,
        ),
        b"ASCII85Decode" | b"A85" => Some(
            data.windows(2)
                .position(|b| b == b"~>")
                .ok_or(Error::syntax("missing ASCII85 terminator"))?
                + 2,
        ),
        b"DCTDecode" | b"DCT" => Some(pagura_imagecodec::dct::data_length(data)?),
        b"RunLengthDecode" | b"RL" => {
            let mut offset = 0;
            loop {
                let control = *data
                    .get(offset)
                    .ok_or(Error::syntax("truncated inline RunLength data"))?;
                offset += 1;
                if control == 128 {
                    break;
                }
                offset = offset
                    .checked_add(if control < 128 {
                        usize::from(control) + 1
                    } else {
                        1
                    })
                    .filter(|&v| v <= data.len())
                    .ok_or(Error::syntax("truncated inline RunLength data"))?;
            }
            Some(offset)
        }
        _ => None,
    };
    Ok(length)
}
