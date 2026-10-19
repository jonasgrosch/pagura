//! Filter names, parameters and chains (7.4.1, Tables 5 and 6).
use crate::bounded::DECODE_WORK;
use crate::{Predictor, ascii, features, flate, lzw, run_length};
use pagura_core::{Diagnostics, Dictionary, Error, Feature, Limits, Meter, Name, Result, Value};
use std::borrow::Cow;

/// One decoding step: a filter of 7.4.1, Table 6.
///
/// Non-exhaustive, because a later edition or an extension can define
/// further filters.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum Filter {
    /// `ASCIIHexDecode` / `AHx`.
    AsciiHex,
    /// `ASCII85Decode` / `A85`.
    Ascii85,
    /// `LZWDecode` / `LZW`.
    Lzw {
        /// Predictor applied after decompression.
        predictor: Predictor,
        /// `/EarlyChange`.
        early_change: EarlyChange,
    },
    /// `FlateDecode` / `Fl` (zlib, RFC 1950/1951).
    Flate {
        /// Predictor applied after decompression.
        predictor: Predictor,
    },
    /// `RunLengthDecode` / `RL`.
    RunLength,
    /// An image codec, decoded by the image layer (`pagura-image`) rather
    /// than here, with its `/DecodeParms` entry.
    Image {
        /// Which codec.
        codec: ImageCodec,
        /// The codec's parameter dictionary, if any.
        params: Option<Dictionary>,
    },
    /// `Crypt`, applied by the security handler.
    Crypt(CryptFilter),
}

/// The parameters of a `Crypt` filter (7.4.10, Table 14).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CryptFilter {
    name: Option<Name>,
}

impl CryptFilter {
    /// The `/Name` entry: the crypt filter to apply. `None` means the
    /// default, `Identity`.
    pub const fn name(&self) -> Option<&Name> {
        self.name.as_ref()
    }

    /// The `/Name` entry, consuming the parameters.
    pub fn into_name(self) -> Option<Name> {
        self.name
    }
}

/// When LZW codes grow by one bit: the `/EarlyChange` parameter (Table 8),
/// which admits only these two values.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum EarlyChange {
    /// `1`, the default: one code early, as TIFF LZW does.
    #[default]
    Early,
    /// `0`: as late as possible.
    Postponed,
}

/// The image codecs of 7.4.1, Table 6, decoded by `pagura-image`. Table 6
/// closes the set, and `pagura-image` matches it exhaustively.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ImageCodec {
    /// `DCTDecode`: JPEG (ITU-T T.81).
    Dct,
    /// `JPXDecode`: JPEG 2000 (ISO/IEC 15444-1).
    Jpx,
    /// `JBIG2Decode`: JBIG2 (ITU-T T.88).
    Jbig2,
    /// `CCITTFaxDecode`: Group 3 and Group 4 fax (ITU-T T.4, T.6).
    CcittFax,
}

impl ImageCodec {
    /// The codec's feature tag, reported when a chain asks this crate to
    /// decode the codec itself (see [`features`]).
    pub const fn feature(self) -> Feature {
        match self {
            Self::Dct => features::DCT,
            Self::Jpx => features::JPX,
            Self::Jbig2 => features::JBIG2,
            Self::CcittFax => features::CCITT_FAX,
        }
    }
}

/// The ordered filters of one stream (`/Filter` plus `/DecodeParms`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FilterChain {
    filters: Vec<Filter>,
}

impl FilterChain {
    /// Builds a chain from a stream dictionary's `/Filter` and
    /// `/DecodeParms` entries, which the caller has already resolved to
    /// direct objects (including array elements).
    ///
    /// # Errors
    ///
    /// Returns a syntax error for malformed entries and `Unsupported` for
    /// unknown filter names.
    pub fn from_entries(filter: Option<&Value>, params: Option<&Value>) -> Result<Self> {
        let names: Vec<&Name> = match filter {
            None | Some(Value::Null) => Vec::new(),
            Some(Value::Name(name)) => vec![name],
            Some(Value::Array(values)) => values
                .iter()
                .map(|value| {
                    value
                        .as_name()
                        .ok_or(Error::syntax("Filter array entry must be a name"))
                })
                .collect::<Result<_>>()?,
            Some(_) => return Err(Error::syntax("Filter must be a name or array")),
        };
        let params: Vec<Option<&Dictionary>> = match params {
            None | Some(Value::Null) => vec![None; names.len()],
            Some(Value::Dictionary(dictionary)) if names.len() == 1 => vec![Some(dictionary)],
            Some(Value::Array(values)) if values.len() == names.len() => values
                .iter()
                .map(|value| match value {
                    Value::Null => Ok(None),
                    Value::Dictionary(dictionary) => Ok(Some(dictionary)),
                    _ => Err(Error::syntax(
                        "DecodeParms entry must be a dictionary or null",
                    )),
                })
                .collect::<Result<_>>()?,
            Some(_) => return Err(Error::syntax("DecodeParms does not match Filter")),
        };
        let filters = names
            .into_iter()
            .zip(params)
            .enumerate()
            .map(|(index, (name, params))| {
                let filter = Filter::from_pdf(name.as_bytes(), params)?;
                if index != 0 && matches!(filter, Filter::Crypt(_)) {
                    return Err(Error::syntax("Crypt must be the first stream filter"));
                }
                Ok(filter)
            })
            .collect::<Result<_>>()?;
        Ok(Self { filters })
    }

    /// The filters in application order.
    pub fn filters(&self) -> &[Filter] {
        &self.filters
    }

    /// Whether the stream is stored unencoded.
    pub fn is_empty(&self) -> bool {
        self.filters.is_empty()
    }

    /// Removes a leading `Crypt` filter for the security handler and
    /// returns its parameters. [`FilterChain::from_entries`] rejects a
    /// `Crypt` filter anywhere else.
    pub fn take_crypt(&mut self) -> Option<CryptFilter> {
        let Some(Filter::Crypt(crypt)) = self.filters.first_mut() else {
            return None;
        };
        let crypt = std::mem::take(crypt);
        self.filters.remove(0);
        Some(crypt)
    }

    /// Removes a trailing image codec, returning it with its parameters, so
    /// the image layer can decode it after the preceding filters ran. An
    /// image codec anywhere but last stays in the chain and fails decoding.
    pub fn split_image(&mut self) -> Option<(ImageCodec, Option<Dictionary>)> {
        let Some(Filter::Image { codec, params }) = self.filters.last_mut() else {
            return None;
        };
        let image = (*codec, params.take());
        self.filters.pop();
        Some(image)
    }

    /// Applies every filter in order. Unencoded input is borrowed.
    ///
    /// `limits.decoded_bytes` caps the output of every filter, and
    /// `limits.decode_work` the work of the whole chain: building Flate's
    /// Huffman tables costs one unit per table entry.
    ///
    /// # Errors
    ///
    /// Returns a limit error as soon as an output would exceed
    /// `limits.decoded_bytes` or the work `limits.decode_work`, a syntax
    /// error for corrupt data, and `Unsupported` for image codecs. Under [`pagura_core::Recovery::Strict`],
    /// damage that would otherwise be repaired is a syntax error.
    ///
    /// Errors and repairs are positioned at [`Position::Buffer`] offsets in
    /// the input of the filter that found them: the encoded data for the
    /// first filter, and the previous filter's output for later ones.
    ///
    /// [`Position::Buffer`]: pagura_core::Position::Buffer
    pub fn decode<'a>(
        &self,
        input: &'a [u8],
        limits: &Limits,
        diagnostics: &mut Diagnostics,
    ) -> Result<Cow<'a, [u8]>> {
        let max_output = limits.decoded_bytes;
        if self.filters.is_empty() && input.len() > max_output {
            return Err(crate::bounded::decode_limit(max_output));
        }
        let mut work = Meter::new(limits.decode_work, DECODE_WORK);
        let mut data = Cow::Borrowed(input);
        for filter in &self.filters {
            data = Cow::Owned(filter.decode_with(&data, max_output, &mut work, diagnostics)?);
        }
        Ok(data)
    }
}

impl Filter {
    /// Interprets one filter name and its optional parameter dictionary.
    ///
    /// # Errors
    ///
    /// Returns a syntax error for invalid parameters and `Unsupported` for
    /// unknown names.
    pub fn from_pdf(name: &[u8], params: Option<&Dictionary>) -> Result<Self> {
        Ok(match name {
            b"ASCIIHexDecode" | b"AHx" => Self::AsciiHex,
            b"ASCII85Decode" | b"A85" => Self::Ascii85,
            b"LZWDecode" | b"LZW" => Self::Lzw {
                predictor: Predictor::from_params(params)?,
                early_change: match params.and_then(|p| p.get(b"EarlyChange")) {
                    None | Some(Value::Null | Value::Integer(1)) => EarlyChange::Early,
                    Some(Value::Integer(0)) => EarlyChange::Postponed,
                    Some(_) => return Err(Error::syntax("EarlyChange must be 0 or 1")),
                },
            },
            b"FlateDecode" | b"Fl" => Self::Flate {
                predictor: Predictor::from_params(params)?,
            },
            b"RunLengthDecode" | b"RL" => Self::RunLength,
            b"DCTDecode" | b"DCT" => Self::image(ImageCodec::Dct, params),
            b"JPXDecode" => Self::image(ImageCodec::Jpx, params),
            b"JBIG2Decode" => Self::image(ImageCodec::Jbig2, params),
            b"CCITTFaxDecode" | b"CCF" => Self::image(ImageCodec::CcittFax, params),
            b"Crypt" => {
                // 7.4.10, Table 14; null entries are absent (7.3.7).
                match params.and_then(|p| p.get(b"Type")) {
                    None | Some(Value::Null) => {}
                    Some(Value::Name(name)) if name.as_bytes() == b"CryptFilterDecodeParms" => {}
                    Some(_) => return Err(Error::syntax("invalid Crypt parameter Type")),
                }
                let name = match params.and_then(|p| p.get(b"Name")) {
                    None | Some(Value::Null) => None,
                    Some(Value::Name(name)) => Some(name.clone()),
                    Some(_) => return Err(Error::syntax("Crypt parameter Name must be a name")),
                };
                Self::Crypt(CryptFilter { name })
            }
            _ => {
                return Err(Error::unsupported(
                    features::UNKNOWN,
                    "unknown stream filter",
                ));
            }
        })
    }

    /// Decodes `input` with this filter alone, within `limits` as
    /// [`FilterChain::decode`] describes.
    ///
    /// # Errors
    ///
    /// See [`FilterChain::decode`]; positions are offsets in `input`.
    pub fn decode(
        &self,
        input: &[u8],
        limits: &Limits,
        diagnostics: &mut Diagnostics,
    ) -> Result<Vec<u8>> {
        let mut work = Meter::new(limits.decode_work, DECODE_WORK);
        self.decode_with(input, limits.decoded_bytes, &mut work, diagnostics)
    }

    fn decode_with(
        &self,
        input: &[u8],
        max_output: usize,
        work: &mut Meter,
        diagnostics: &mut Diagnostics,
    ) -> Result<Vec<u8>> {
        match self {
            Self::Image { codec, .. } => Err(Error::unsupported(
                codec.feature(),
                "image codecs are decoded by the image layer",
            )),
            Self::Crypt(_) => Err(Error::invalid_argument(
                "Crypt filters are applied by the security handler",
            )),
            Self::AsciiHex => ascii::hex(input, max_output, diagnostics),
            Self::Ascii85 => ascii::ascii85(input, max_output, diagnostics),
            Self::RunLength => run_length::decode(input, max_output, diagnostics),
            Self::Flate { predictor } => {
                flate::decode(input, max_output, *predictor, work, diagnostics)
            }
            Self::Lzw {
                predictor,
                early_change,
            } => lzw::decode(input, max_output, *predictor, *early_change, diagnostics),
        }
    }

    fn image(codec: ImageCodec, params: Option<&Dictionary>) -> Self {
        Self::Image {
            codec,
            params: params.cloned(),
        }
    }
}
