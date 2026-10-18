#![forbid(unsafe_code)]
//! Bounded PDF stream filters (ISO 32000-2 clause 7.4).
//!
//! This crate turns encoded stream bytes into decoded bytes. It knows the
//! filter vocabulary and parameters but nothing about documents: callers
//! resolve indirect references before building a [`FilterChain`], and the
//! security handler removes the `Crypt` filter before decoding.
//! [`flate::compress`] provides zlib output for stream writing; [`png::encode`]
//! encodes eight-bit Gray, RGB and straight RGBA images without host capabilities.
//!
//! Every decoder enforces a caller-supplied output cap *while* decoding, so
//! a small hostile input can never allocate more than the cap. Damaged data
//! that can be used anyway (for example a truncated Flate stream) is a
//! repair reported through [`Diagnostics`], never a silent success (ADR 0003).

mod ascii;
mod encoding;
#[cfg(test)]
mod encoding_tests;
pub mod flate;
mod lzw;
mod output;
pub mod png;

use pagura_core::syntax::is_whitespace;
use pagura_core::{
    Diagnostic, Diagnostics, Dictionary, Error, Feature, Name, Position, Repair, Result, Value,
};

use std::borrow::Cow;

/// Stable tags for filters this crate does not decode.
pub mod features {
    use pagura_core::Feature;

    /// Predictor dimensions exceed the stable API representation.
    pub const PREDICTOR_DIMENSIONS: Feature = Feature::new("predictor.dimensions");

    /// `DCTDecode` (JPEG); decoded by the image layer, not implemented yet.
    pub const DCT: Feature = Feature::new("filter.DCTDecode");
    /// `JPXDecode` (JPEG 2000); not implemented yet.
    pub const JPX: Feature = Feature::new("filter.JPXDecode");
    /// `JBIG2Decode`; not implemented yet.
    pub const JBIG2: Feature = Feature::new("filter.JBIG2Decode");
    /// `CCITTFaxDecode`; not implemented yet.
    pub const CCITT_FAX: Feature = Feature::new("filter.CCITTFaxDecode");
}

/// Stable, security-relevant repairs for damaged filter input (ADR 0003).
pub mod tags {
    use pagura_core::Repair;
    /// FlateDecode data lacks the zlib wrapper; decoded as raw DEFLATE.
    /// ISO 32000-2 clause 7.4.4 requires RFC 1950 framing; Strict rejects
    /// this repair. Explicit `decode_raw_deflate` does not apply it.
    pub const RAW_DEFLATE: Repair = Repair::new("filter.raw-deflate");
    /// Keep the decoded prefix when input ends before the required end marker.
    /// Incomplete predictor rows remain errors; no synthetic pixels are added.
    pub const TRUNCATED: Repair = Repair::new("filter.truncated");
    /// Keep a full 12-bit LZW dictionary through entry 4095 without further
    /// insertions until clear or EOD; report missing clear once per stream.
    /// This recovers an encoder fault under ISO 32000-2 clause 7.4.4.2.
    pub const LZW_FULL_TABLE: Repair = Repair::new("filter.lzw-full-table");
    /// Ignore nonzero unused bits after LZW EOD. ISO 32000-2 clause 7.4.4.2
    /// requires the encoder to zero them; Strict rejects this repair.
    pub const LZW_PADDING: Repair = Repair::new("filter.lzw-padding");
    /// Start with the initial literal dictionary when the leading clear code
    /// is absent and the first code is a literal or EOD (7.4.4.2).
    pub const LZW_INITIAL_CLEAR: Repair = Repair::new("filter.lzw-initial-clear");
    /// Keep decoded bytes despite a mismatched zlib Adler-32 checksum.
    pub const CHECKSUM_MISMATCH: Repair = Repair::new("filter.checksum-mismatch");
    /// Ignore bytes after the first end marker. ASCII filters permit trailing
    /// PDF whitespace; binary filters report any trailing byte, including zero.
    pub const TRAILING_DATA: Repair = Repair::new("filter.trailing-data");
}

fn syntax(message: &'static str, offset: usize) -> Error {
    Error::syntax(message).at(Position::Buffer(offset))
}

fn limit(offset: usize) -> Error {
    Error::limit("decoded stream budget exceeded").at(Position::Buffer(offset))
}

fn repair(
    diagnostics: &mut Diagnostics,
    tag: Repair,
    message: &'static str,
    offset: usize,
) -> Result<()> {
    diagnostics.repair(Diagnostic::new(tag, message).at(Position::Buffer(offset)))
}

fn trailing(input: &[u8], end: usize, ascii: bool, diagnostics: &mut Diagnostics) -> Result<()> {
    if let Some(bytes) = input.get(end..)
        && let Some(index) = bytes
            .iter()
            .position(|&byte| !ascii || !is_whitespace(byte))
    {
        repair(
            diagnostics,
            tags::TRAILING_DATA,
            "data after filter end marker",
            end + index,
        )?;
    }
    Ok(())
}

/// Decodes explicitly unwrapped DEFLATE (RFC 1951 section 3.2), applying
/// the optional PDF predictor after inflation.
///
/// [`Filter::Flate`] accepts zlib and, with a reported repair, automatically
/// recognized raw input, preferring a valid zlib header. An unwrapped stored block can coincidentally
/// start with such a header; use this function when the container identifies
/// raw DEFLATE, avoiding that inherent format ambiguity. PDF FlateDecode
/// requires the zlib wrapper (ISO 32000-2 clause 7.4.4). This explicit raw
/// entry point does not report [`tags::RAW_DEFLATE`].
///
/// # Errors
///
/// Returns the same bounded, positioned syntax/limit errors and reported
/// truncation/trailing-data repairs as [`Filter::decode`].
pub fn decode_raw_deflate(
    input: &[u8],
    max_output: usize,
    predictor: Predictor,
    diagnostics: &mut Diagnostics,
) -> Result<Vec<u8>> {
    flate::raw(input, max_output, predictor, diagnostics)
}

/// Predictor parameters shared by `FlateDecode` and `LZWDecode`
/// (clause 7.4.4.4, Table 8).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Predictor {
    /// The `/Predictor` algorithm.
    pub kind: PredictorKind,
    /// `/Colors`: interleaved colour components per sample, at least 1.
    pub colors: u8,
    /// `/BitsPerComponent`: 1, 2, 4, 8 or 16.
    pub bits_per_component: u8,
    /// `/Columns`: samples per row, at least 1.
    pub columns: u32,
}

impl Default for Predictor {
    fn default() -> Self {
        Self {
            kind: PredictorKind::None,
            colors: 1,
            bits_per_component: 8,
            columns: 1,
        }
    }
}

/// The `/Predictor` value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PredictorKind {
    /// 1: no prediction.
    None,
    /// 2: TIFF Predictor 2.
    Tiff,
    /// 10–15: PNG predictors, selected per row by a tag byte.
    Png,
}

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
            return Err(limit(max_output));
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

impl Predictor {
    /// Reads predictor parameters from a `/DecodeParms` dictionary.
    ///
    /// # Errors
    ///
    /// Returns a syntax error for values outside Table 8, or
    /// `Unsupported(predictor.dimensions)` for positive dimensions too large
    /// for the stable [`Predictor`] representation.
    pub fn from_params(params: Option<&Dictionary>) -> Result<Self> {
        let Some(params) = params else {
            return Ok(Self::default());
        };
        let integer = |key: &[u8], default: i64| match params.get(key) {
            None | Some(Value::Null) => Ok(default),
            Some(Value::Integer(value)) => Ok(*value),
            Some(_) => Err(Error::syntax("predictor parameter must be an integer")),
        };
        let kind = match integer(b"Predictor", 1)? {
            1 => PredictorKind::None,
            2 => PredictorKind::Tiff,
            10..=15 => PredictorKind::Png,
            _ => return Err(Error::syntax("invalid Predictor")),
        };
        let colors = integer(b"Colors", 1)?;
        if colors < 1 {
            return Err(Error::syntax("invalid Colors"));
        }
        let colors = u8::try_from(colors).map_err(|_| {
            Error::unsupported(
                features::PREDICTOR_DIMENSIONS,
                "Colors exceeds predictor API representation",
            )
        })?;
        let bits_per_component = u8::try_from(integer(b"BitsPerComponent", 8)?)
            .ok()
            .filter(|b| matches!(b, 1 | 2 | 4 | 8 | 16))
            .ok_or(Error::syntax("invalid BitsPerComponent"))?;
        let columns = integer(b"Columns", 1)?;
        if columns < 1 {
            return Err(Error::syntax("invalid Columns"));
        }
        let columns = u32::try_from(columns).map_err(|_| {
            Error::unsupported(
                features::PREDICTOR_DIMENSIONS,
                "Columns exceeds predictor API representation",
            )
        })?;
        Ok(Self {
            kind,
            colors,
            bits_per_component,
            columns,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{Filter, FilterChain, PredictorKind};
    use pagura_core::{Dictionary, Name, Value};

    #[test]
    fn chains_pair_filters_with_parameters_and_borrow_plain_data() {
        let params: Dictionary = [
            (Name::new(b"Predictor"), Value::Integer(12)),
            (Name::new(b"Columns"), Value::Integer(5)),
        ]
        .into_iter()
        .collect();
        let chain = FilterChain::from_entries(
            Some(&Value::Array(vec![Value::name(b"A85"), Value::name(b"Fl")])),
            Some(&Value::Array(vec![Value::Null, Value::Dictionary(params)])),
        )
        .unwrap();
        assert_eq!(chain.filters()[0], Filter::Ascii85);
        let Filter::Flate { predictor } = chain.filters()[1] else {
            panic!("expected Flate");
        };
        assert_eq!((predictor.kind, predictor.columns), (PredictorKind::Png, 5));
        let plain = FilterChain::default();
        let mut diagnostics = pagura_core::Diagnostics::default();
        let decoded = plain.decode(b"abc", 3, &mut diagnostics).unwrap();
        assert!(matches!(decoded, std::borrow::Cow::Borrowed(b"abc")));
    }

    #[test]
    fn validates_parameters_and_preserves_image_and_security_boundaries() {
        use super::{Predictor, features};
        use pagura_core::{Diagnostics, ErrorKind};
        for (key, value) in [
            (b"Colors".as_slice(), 0),
            (b"Columns", -1),
            (b"BitsPerComponent", 3),
            (b"Predictor", 9),
            (b"EarlyChange", 2),
        ] {
            let params = [(Name::new(key), Value::Integer(value))]
                .into_iter()
                .collect();
            assert_eq!(
                Filter::from_pdf(b"LZWDecode", Some(&params))
                    .unwrap_err()
                    .kind,
                ErrorKind::Syntax
            );
        }
        for (key, value) in [
            (b"Colors".as_slice(), 256),
            (b"Columns", i64::from(u32::MAX) + 1),
        ] {
            let params = [(Name::new(key), Value::Integer(value))]
                .into_iter()
                .collect();
            assert_eq!(
                Predictor::from_params(Some(&params)).unwrap_err().kind,
                ErrorKind::Unsupported(features::PREDICTOR_DIMENSIONS)
            );
        }
        for value in 10..=15 {
            let params = [(Name::new(b"Predictor"), Value::Integer(value))]
                .into_iter()
                .collect();
            assert_eq!(
                Predictor::from_params(Some(&params)).unwrap().kind,
                PredictorKind::Png
            );
        }
        for (name, tag) in [
            (b"DCTDecode".as_slice(), features::DCT),
            (b"JPXDecode", features::JPX),
            (b"JBIG2Decode", features::JBIG2),
            (b"CCITTFaxDecode", features::CCITT_FAX),
        ] {
            let filter = Filter::from_pdf(name, None).unwrap();
            assert_eq!(
                filter
                    .decode(b"", 0, &mut Diagnostics::default())
                    .unwrap_err()
                    .kind,
                ErrorKind::Unsupported(tag)
            );
        }
        let crypt = Filter::from_pdf(b"Crypt", None).unwrap();
        assert_eq!(
            crypt
                .decode(b"", 0, &mut Diagnostics::default())
                .unwrap_err()
                .kind,
            ErrorKind::InvalidArgument
        );
        let mut chain = FilterChain::from_entries(
            Some(&Value::Array(vec![
                Value::name(b"Crypt"),
                Value::name(b"AHx"),
            ])),
            None,
        )
        .unwrap();
        assert_eq!(chain.take_crypt(), Some(None));
        assert_eq!(chain.take_crypt(), None);
        assert_eq!(
            chain
                .decode(b"6162>", 2, &mut Diagnostics::default())
                .unwrap()
                .as_ref(),
            b"ab"
        );
    }

    #[test]
    fn chained_filters_use_ordered_steps_with_a_cap_at_each_step() {
        let chain = FilterChain::from_entries(
            Some(&Value::Array(vec![Value::name(b"AHx"), Value::name(b"RL")])),
            None,
        )
        .unwrap();
        let mut diagnostics = pagura_core::Diagnostics::default();
        assert_eq!(
            chain
                .decode(b"ff4180>", 3, &mut diagnostics)
                .unwrap()
                .as_ref(),
            b"AA"
        );
        assert!(diagnostics.is_empty());
        assert_eq!(
            chain
                .decode(b"ff4180>", 2, &mut diagnostics)
                .unwrap_err()
                .kind,
            pagura_core::ErrorKind::Limit
        );
    }
}

#[cfg(test)]
mod vectors;
