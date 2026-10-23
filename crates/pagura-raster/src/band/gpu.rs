//! Optional read-only GPU geometry seam. Ordinary coverage stays on the GPU;
//! exceptional rows resolved by CPU preparation are exported separately.
use crate::band::curve::Mono;
use crate::band::glyph::{Kind, PolygonLines};
use crate::band::shape::Body;
use crate::{FillRule, Point, Shape};
use pagura_core::Result;
mod corrections;
pub use corrections::CorrectionRow;

/// A prepared shape borrowed by an experimental coverage producer.
#[derive(Debug, Clone, Copy)]
pub struct Geometry<'a>(&'a Shape);
impl Shape {
    /// Borrows geometry without running the ordinary band rasterizer.
    pub fn gpu_geometry(&self) -> Geometry<'_> {
        Geometry(self)
    }
}
impl<'a> Geometry<'a> {
    /// Shared translation-free source and its exact placement origin.
    pub fn shared(self) -> Option<(&'a crate::GlyphEdges, Point)> {
        match &self.0.body {
            Body::Glyph(body) if !self.device_points() => Some((&body.edges, body.origin)),
            _ => None,
        }
    }
    /// Whether geometric glyph points depend on the device floor cells.
    pub fn device_points(self) -> bool {
        matches!(&self.0.body, Body::Glyph(body) if matches!(&body.edges.kind,Kind::Polygon{points,..} if !points.is_empty()))
    }
    /// Continuous rectangular clip, in device coordinates.
    pub fn rectangle_clip(self) -> Option<[f64; 4]> {
        self.0
            .clip
            .as_ref()
            .and_then(crate::ClipShape::rect)
            .map(|r| [r.left, r.top, r.right, r.bottom])
    }
    /// Its fill rule.
    pub fn rule(self) -> FillRule {
        self.0.rule
    }
    /// Visits lines and monotone quadratics, in accumulation order.
    /// A line repeats its endpoint in the last two positions.
    ///
    /// # Errors
    /// Propagates the visitor's work/allocation failure.
    pub fn edges(self, mut visit: impl FnMut([Point; 3], bool) -> Result<()>) -> Result<()> {
        match &self.0.body {
            Body::Edges(body) => pieces(&body.lines, &body.curves, Point::default(), visit),
            Body::Glyph(body) => match &body.edges.kind {
                Kind::Curve { relative, .. } => {
                    pieces(&relative.lines, &relative.curves, body.origin, visit)
                }
                Kind::Polygon {
                    polygon, points, ..
                } => {
                    for (_, line) in PolygonLines::new(polygon, points, body.origin).indexed() {
                        if !line.is_horizontal() {
                            visit([line.a, line.b, line.b], false)?;
                        }
                    }
                    Ok(())
                }
            },
        }
    }
    /// Whether an active nonrectangular clip restricts the shape.
    pub fn has_clip_rows(self) -> bool {
        self.0.masked
    }
    /// Visits kept clip runs. Joint boundary rows are supplied separately
    /// by `corrections`, so boundary values are never multiplied.
    ///
    /// # Errors
    /// Propagates the visitor's budget error.
    pub fn clips(self, y: u32, mut visit: impl FnMut(u32, u32, u8) -> Result<()>) -> Result<()> {
        if let Some(rows) = self
            .0
            .clip
            .as_ref()
            .filter(|_| self.0.masked)
            .and_then(crate::ClipShape::rows)
        {
            for segment in rows.row(y) {
                visit(segment.x0, segment.x1, segment.value)?;
            }
        }
        Ok(())
    }
}
fn pieces(
    lines: &[crate::band::edges::Line],
    curves: &[Mono],
    origin: Point,
    mut visit: impl FnMut([Point; 3], bool) -> Result<()>,
) -> Result<()> {
    let translate = |p: Point| Point::new(p.x + origin.x, p.y + origin.y);
    for line in lines {
        if !line.is_horizontal() {
            visit([line.a, line.b, line.b].map(translate), false)?;
        }
    }
    for q in curves {
        let a = Point::new(q.x0, q.y0);
        let c = Point::new(q.x1, q.y1);
        let points = if q.direction > 0.0 {
            [a, q.control(), c]
        } else {
            [c, q.control(), a]
        };
        visit(points.map(translate), true)?;
    }
    Ok(())
}
impl crate::GlyphEdges {
    /// Visits a shared source once, without placement translation.
    ///
    /// # Errors
    /// Propagates the visitor's budget error.
    pub fn gpu_edges(&self, mut visit: impl FnMut([Point; 3], bool) -> Result<()>) -> Result<()> {
        match &self.kind {
            Kind::Curve { relative, .. } => {
                pieces(&relative.lines, &relative.curves, Point::default(), visit)
            }
            Kind::Polygon {
                polygon, points, ..
            } => {
                for (_, line) in PolygonLines::new(polygon, points, Point::default()).indexed() {
                    if !line.is_horizontal() {
                        visit([line.a, line.b, line.b], false)?;
                    }
                }
                Ok(())
            }
        }
    }
}
