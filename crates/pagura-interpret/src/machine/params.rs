//! Line, flatness and rendering-intent parameters (8.4.3, 8.6.5.8), set by
//! their operators and by graphics state parameter dictionaries.

use super::state::Graphics;
use pagura_color::Intent;
use pagura_content::Op;
use pagura_core::{Error, Result};
use pagura_raster::{LineCap, LineJoin};

/// Applies a parameter operation to `state`.
///
/// # Errors
///
/// `Syntax` for an operation that sets no parameter.
pub(super) fn apply(op: &Op, state: &mut Graphics) -> Result<()> {
    match op {
        Op::LineWidth(width) => state.stroke.width = width.max(0.0),
        Op::LineCap(cap) => {
            state.stroke.cap = match cap.clamp(&0, &2) {
                0 => LineCap::Butt,
                1 => LineCap::Round,
                _ => LineCap::ProjectingSquare,
            }
        }
        Op::LineJoin(join) => {
            state.stroke.join = match join.clamp(&0, &2) {
                0 => LineJoin::Miter,
                1 => LineJoin::Round,
                _ => LineJoin::Bevel,
            }
        }
        Op::MiterLimit(limit) => state.stroke.miter_limit = limit.max(1.0),
        Op::Flatness(flatness) => {
            state.flatness = flatness.clamp(0.0, 100.0);
            return Ok(());
        }
        Op::RenderingIntent(name) => {
            // Table 69 permits RelativeColorimetric for unknown intents.
            state.intent = Intent::from_name(name.as_bytes());
            state.composite_changed();
            return Ok(());
        }
        Op::Dash(dash) => {
            state.stroke.dash_array.clone_from(&dash.lengths);
            state.stroke.dash_phase = dash.phase;
        }
        _ => return Err(Error::syntax("invalid graphics parameter")),
    }
    state.pen_changed();
    Ok(())
}
