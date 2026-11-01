//! Stable repairs for nonconforming annotation appearance input.

use pagura_core::Repair;

/// Text-markup QuadPoints in top-left/top-right/bottom-left/bottom-right
/// order are reordered to counterclockwise vertices (12.5.6.10, Table 182).
pub const QUAD_ORDER: Repair = Repair::new("annotation.quad-order");

/// A Popup without Rect or a stored appearance has no flat-page artwork;
/// viewing skips that inert popup without inventing a rectangle (12.5.6.14).
pub const POPUP_MISSING_RECT: Repair = Repair::new("annotation.popup-missing-rect");

/// Every repair defined by this crate.
pub const ALL: &[Repair] = &[QUAD_ORDER, POPUP_MISSING_RECT];
