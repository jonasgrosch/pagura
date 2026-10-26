//! Stable repairs for nonconforming annotation appearance input.

use pagura_core::Repair;

/// Text-markup QuadPoints in top-left/top-right/bottom-left/bottom-right
/// order are reordered to counterclockwise vertices (12.5.6.10, Table 182).
pub const QUAD_ORDER: Repair = Repair::new("annotation.quad-order");

/// Every repair defined by this crate.
pub const ALL: &[Repair] = &[QUAD_ORDER];
