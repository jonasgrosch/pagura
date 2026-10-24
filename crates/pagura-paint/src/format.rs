//! Storage formats of compositing buffers (ADR 0004, ADR 0034).

/// How a buffer stores premultiplied colour and alpha. Each page, group or
/// soft-mask buffer gets its format before its first store, from its
/// group's whole content.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) enum Format {
    /// 16-bit normalized with one rounding per store: Normal source-over
    /// chains within ADR 0004's bound.
    N16,
    /// f32, under ADR 0004's count bound.
    F32,
    /// f64: buffers that feed an amplifying operation, and buffers with spot
    /// channels.
    F64,
}
