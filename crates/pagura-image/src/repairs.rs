//! Stable tags for the reported repairs of malformed image dictionaries and
//! data (ADR 0003).
use pagura_core::Repair;

/// Missing packed bytes become zero, including a missing low byte of a
/// 16-bit sample; dimensions and row padding remain authoritative (8.9.3,
/// 8.9.5.1). The fill value is a reader policy, not specified by ISO.
pub const DATA_SHORT: Repair = Repair::new("image.data-short");
/// Bytes beyond the declared sample rows are ignored (8.9.3, 8.9.5.1).
pub const DATA_EXCESS: Repair = Repair::new("image.data-excess");
/// Ignore a stencil's extraneous color space (8.9.5.1, Table 87).
pub const STENCIL_COLOR_SPACE: Repair = Repair::new("image.stencil-color-space");
/// Recognize Table 92 filter abbreviations on an XObject (8.9.7).
pub const FILTER_ABBREVIATION: Repair = Repair::new("image.filter-abbreviation");
/// Treat a serialized `(null)` parameter as absent, or ignore a wrong-typed
/// parameter for parameterless JPXDecode (7.3.9, 7.3.8.1 Table 5, 7.4.1,
/// 7.4.9). Actual null values require no repair.
pub const DECODE_PARAMS: Repair = Repair::new("image.decode-params");
/// Decode fax rows using Columns, then crop or white-pad to the image Width
/// (7.4.6 Table 11; 8.9.5.1 Table 87). Row padding is not a sample.
pub const CCITT_COLUMNS: Repair = Repair::new("image.ccitt-columns");
/// Decode at the image's sample precision after a syntax failure with a
/// conflicting valid predictor precision, only if the retry succeeds
/// (7.4.4.3 Table 10).
pub const PREDICTOR_PRECISION: Repair = Repair::new("image.predictor-precision");
/// Subtype Image identifies an image XObject despite a malformed optional
/// Type entry (8.9.5 Table 87). A missing or invalid Subtype remains an error.
pub const XOBJECT_TYPE: Repair = Repair::new("image.xobject-type");
/// DCTDecode data with predictor parameters is decoded as Flate only after
/// strict framing, checksum and predictor validation and an exact match to
/// the declared eight-bit image sample count (7.4.4, 7.4.8).
pub const FILTER_MISMATCH: Repair = Repair::new("image.filter-mismatch");

/// Every repair tag of this crate.
pub const ALL: &[Repair] = &[
    DATA_SHORT,
    DATA_EXCESS,
    STENCIL_COLOR_SPACE,
    FILTER_ABBREVIATION,
    DECODE_PARAMS,
    CCITT_COLUMNS,
    PREDICTOR_PRECISION,
    XOBJECT_TYPE,
    FILTER_MISMATCH,
];
