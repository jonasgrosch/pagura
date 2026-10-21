//! Required source checks for scenes migrated from the opaque byte traversal.
//!
//! This source-order pass never consults visibility. ICC and tint functions can
//! parse successfully but fail when evaluated; moving them behind region culling
//! would silently turn an unsupported source into a successful blank render.
//! Conversion shares the render's function-work budget. Execution still converts
//! visible sources; a later bounded prepared-scene record can remove that cost.

use super::{Command, Context, Mark, Model};
use pagura_core::{Error, Result};
use pagura_raster::Segment;

impl Context {
    pub(super) fn validate_plain_sources(
        &mut self,
        commands: &[Command],
        depth: usize,
    ) -> Result<()> {
        if depth >= self.limits.depth {
            return Err(Error::limit("compositing depth exceeded"));
        }
        self.charge(commands.len())?;
        for command in commands {
            let result = (|| {
                let paint = match command {
                    Command::Path(paint) => paint,
                    Command::Glyphs(run) => &run.paint,
                    Command::Group(group) => {
                        return self.validate_plain_sources(&group.commands, depth + 1);
                    }
                    Command::Image(_) | Command::Shading(_) => {
                        return Err(Error::invalid_argument("non-opaque source validation"));
                    }
                };
                if paint.color.paints() {
                    self.color(&paint.color, Model::Rgb, &paint.composite, true)?;
                }
                // Glyph rasterization uses its outlines, not paint.mark's pen.
                // A non-painting path still validates its stroke transform.
                if matches!(command, Command::Path(_))
                    && let Mark::Stroke(_, ctm, _) = &paint.mark
                {
                    let inverse = ctm.inverse().ok_or(Error::unsupported(
                        crate::features::SINGULAR_STROKE,
                        "singular stroke transform is not implemented",
                    ))?;
                    if !ctm.then(self.device).is_finite() {
                        return Err(Error::invalid_argument("path transform is not finite"));
                    }
                    // The old traversal mapped into pen space before culling.
                    // Check without allocating an outline: finite source points
                    // can overflow through an otherwise invertible pen matrix.
                    self.charge(paint.path.segments().len())?;
                    for segment in paint.path.segments() {
                        let points = match segment {
                            Segment::MoveTo(p) | Segment::LineTo(p) => [Some(p), None, None],
                            Segment::CubicTo(a, b, p) => [Some(a), Some(b), Some(p)],
                            Segment::Close => [None; 3],
                        };
                        for point in points.into_iter().flatten() {
                            let mapped = inverse.apply(*point);
                            if !mapped.x.is_finite() || !mapped.y.is_finite() {
                                return Err(Error::invalid_argument("path geometry is not finite"));
                            }
                        }
                    }
                }
                Ok(())
            })();
            result.map_err(|error| crate::display::contextual(error, command.position()))?;
        }
        Ok(())
    }
}
