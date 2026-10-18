//! Filter names, parameters and chains (clause 7.4.1, Tables 5 and 6).
use crate::{Predictor, ascii, features, flate, lzw};
use pagura_core::{Diagnostics, Dictionary, Error, Feature, Name, Result, Value};
use std::borrow::Cow;

/// One decoding step.
#[derive(Debug, Clone, PartialEq)]
pub enum Filter {
    /// `ASCIIHexDecode` / `AHx`.
    AsciiHex,
    /// `ASCII85Decode` / `A85`.
    Ascii85,
    /// `LZWDecode` / `LZW`.
    Lzw {
        /// Predictor applied after decompression.
        predictor: Predictor,
        /// `/EarlyChange`, default true.
        early_change: bool,
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
    /// `Crypt`, applied by the security handler. Carries `/Name`, if any.
    Crypt(Option<Name>),
}

/// The image codecs of clause 7.4 (Table 6), decoded by `pagura-image`.
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
    /// The stable feature tag reported while the codec is unavailable.
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
                        .ok_or(Error::syntax("filter must be a name"))
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

    /// Removes a leading `Crypt` filter for the security handler, returning
    /// its `/Name` (`None` means the default, `Identity`).
    pub fn take_crypt(&mut self) -> Option<Option<Name>> {
        match self.filters.first() {
            Some(Filter::Crypt(_)) => match self.filters.remove(0) {
                Filter::Crypt(name) => Some(name),
                _ => None,
            },
            _ => None,
        }
    }

    /// Removes a trailing image codec, returning it with its parameters, so
    /// the image layer can decode it after the preceding filters ran. An
    /// image codec anywhere but last stays in the chain and fails decoding.
    pub fn split_image(&mut self) -> Option<(ImageCodec, Option<Dictionary>)> {
        if !matches!(self.filters.last(), Some(Filter::Image { .. })) {
            return None;
        }
        match self.filters.pop() {
            Some(Filter::Image { codec, params }) => Some((codec, params)),
            _ => None,
        }
    }

    /// Applies every filter in order. Unencoded input is borrowed.
    ///
    /// # Errors
    ///
    /// Returns a limit error as soon as the output would exceed
    /// `max_output` bytes, a syntax error for corrupt data, and
    /// `Unsupported` for image codecs. Under [`pagura_core::Recovery::Strict`],
    /// damage that would otherwise be repaired is a syntax error.
    pub fn decode<'a>(
        &self,
        input: &'a [u8],
        max_output: usize,
        diagnostics: &mut Diagnostics,
    ) -> Result<Cow<'a, [u8]>> {
        if self.filters.is_empty() && input.len() > max_output {
            return Err(crate::report::limit(max_output));
        }
        let mut data = Cow::Borrowed(input);
        for filter in &self.filters {
            data = Cow::Owned(filter.decode(&data, max_output, diagnostics)?);
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
                    None | Some(Value::Null | Value::Integer(1)) => true,
                    Some(Value::Integer(0)) => false,
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
                // ISO 32000-2 7.4.10, Table 14; null entries are absent (7.3.7).
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
                Self::Crypt(name)
            }
            _ => {
                return Err(Error::unsupported(
                    Feature::new("filter.unknown"),
                    "unknown stream filter",
                ));
            }
        })
    }

    /// Decodes `input` with this filter alone.
    ///
    /// # Errors
    ///
    /// See [`FilterChain::decode`].
    pub fn decode(
        &self,
        input: &[u8],
        max_output: usize,
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
            Self::RunLength => ascii::run_length(input, max_output, diagnostics),
            Self::Flate { predictor } => flate::decode(input, max_output, *predictor, diagnostics),
            Self::Lzw {
                predictor,
                early_change,
            } => lzw::decode(input, max_output, *predictor, *early_change, diagnostics),
        }
    }
}

impl Filter {
    fn image(codec: ImageCodec, params: Option<&Dictionary>) -> Self {
        Self::Image {
            codec,
            params: params.cloned(),
        }
    }
}
