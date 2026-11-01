//! Stable reported repairs owned by the image codecs (ADR 0003).

use pagura_core::Repair;

/// 7.4.9 requires a full JPX file. Viewing may decode an unboxed
/// T.800 codestream only with an explicit PDF colour space and no
/// embedded-opacity request; metadata is never inferred from channel count.
pub const JPX_CODESTREAM: Repair = Repair::new("jpx.raw-codestream");

/// Clear only unused high bits above each declared palette precision.
/// The low-order sample bits and signed interpretation stay authoritative
/// (T.800 I.5.3.4, Tables I.12–I.13). This is a reader repair policy.
pub const JPX_PALETTE_PADDING: Repair = Repair::new("jpx.palette-padding");

/// Every repair in this module.
pub const ALL: &[Repair] = &[JPX_CODESTREAM, JPX_PALETTE_PADDING];
