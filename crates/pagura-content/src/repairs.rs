//! Reported repairs of inline content framing (8.9.7; ADR 0003).
use pagura_core::Repair;

/// A caller-framed image ends immediately before a complete `EI` token,
/// without the required intervening white-space byte. The image bytes and
/// following operator are unchanged; no boundary is inferred by this repair.
pub const EI_SEPARATOR: Repair = Repair::new("inline.ei-separator");

/// Every repair owned by this crate, in the `inline.` area.
pub const ALL: &[Repair] = &[EI_SEPARATOR];
