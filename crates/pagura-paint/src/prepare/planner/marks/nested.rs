//! The geometry inherited by ordinary nested tiling marks beneath an
//! image stencil (8.7.3, 8.9.6.2): keep each fill, stroke or linear glyph
//! and clip as geometry, so leaf marks integrate the intersection once.

use super::{Geometry, Spec};
use crate::prepare::Budget;
use crate::prepare::planner::{Planner, Scope};
use pagura_core::{Error, Limits, Result};
use pagura_interpret::{MarkInfo, Pen};
use pagura_raster::{FillRule, Path, Segment, Transform};

impl Planner<'_> {
    /// Propagates the outer shape instead of applying its area average to
    /// a group whose children already include the inherited stencil.
    pub(super) fn pattern_geometry(
        &mut self,
        scope: &Scope,
        spec: &Spec<'_>,
        budget: &mut Budget,
    ) -> Result<u32> {
        match &spec.geometry {
            Geometry::Fill(path, _) if spec.stencil => {
                self.stencil_clip(scope, spec.info.clip, path, budget)
            }
            Geometry::Fill(path, rule) => self.pattern_clip(
                scope,
                spec.info.clip,
                path,
                (*rule, scope.transform),
                budget,
            ),
            Geometry::Stroke(path, pen) => self.pattern_stroke(scope, spec.info, path, pen, budget),
            Geometry::Glyph(source, linear, origin, _)
                if source.outline.segments().iter().all(|segment| {
                    matches!(
                        segment,
                        Segment::MoveTo(_) | Segment::LineTo(_) | Segment::Close
                    )
                }) =>
            {
                let [a, b, c, d] = *linear;
                self.pattern_clip(
                    scope,
                    spec.info.clip,
                    &source.outline,
                    (
                        FillRule::NonZero,
                        Transform::new(a, b, c, d, origin.x, origin.y),
                    ),
                    budget,
                )
            }
            // Generic clipping flattens curves. A patterned glyph's true
            // quadratic coverage cannot be replaced with that geometry.
            Geometry::Glyph(..) => Err(Error::unsupported(
                pagura_interpret::features::PATTERN_IMAGE_MASK,
                "curved patterned glyphs under an inherited stencil are not implemented",
            )),
            Geometry::Unit(_) | Geometry::Area(_) => Err(Error::invalid_argument(
                "a tiling paint requires path geometry",
            )),
        }
    }

    /// A stroked pattern mark contributes the existing stroke outline in
    /// device space. Source, outline and inherited copies coexist under
    /// the same render-byte allowance.
    fn pattern_stroke(
        &mut self,
        scope: &Scope,
        info: &MarkInfo,
        path: &Path,
        pen: &Pen,
        budget: &mut Budget,
    ) -> Result<u32> {
        let inverse = pen.transform.inverse().ok_or(Error::unsupported(
            pagura_interpret::features::SINGULAR_STROKE,
            "singular stroke transform",
        ))?;
        let local = path.transformed_with_bytes(inverse, &mut budget.bytes)?;
        let transform = pen.transform.then(scope.transform);
        let prepared = (|| {
            let limits = Limits {
                render_bytes: budget.bytes.remaining(),
                ..*self.limits
            };
            let (outline, transform) = match pen.adjustment {
                pagura_raster::Adjustment::None => {
                    pagura_raster::stroke_outline(&local, transform, &pen.stroke, &limits)?
                }
                pagura_raster::Adjustment::Adjusted => {
                    pagura_raster::adjusted_stroke_outline(&local, transform, &pen.stroke, &limits)?
                }
            };
            budget.bytes.charge(outline.retained_bytes())?;
            let prepared = self.pattern_clip(
                scope,
                info.clip,
                &outline,
                (FillRule::NonZero, transform),
                budget,
            );
            let released = outline.retained_bytes();
            drop(outline);
            budget.bytes.refund(released);
            prepared
        })();
        let released = local.retained_bytes();
        drop(local);
        budget.bytes.refund(released);
        prepared
    }
}
