//! Read-only geometry seam for the optional GPU coverage experiment.
//!
//! No coverage is rasterized here. The existing preparation's exceptional
//! winding/joint-clip cells and clip classification remain explicit inputs.
use crate::band::clip::INSIDE;
use crate::band::shape::Body;
use crate::{FillRule, Point, Shape};
use pagura_core::Result;

/// A prepared shape borrowed by an experimental coverage producer.
#[derive(Debug, Clone, Copy)]
pub struct Geometry<'a>(&'a Shape);

impl Shape {
    /// Borrows geometry without running the band rasterizer.
    pub fn gpu_geometry(&self) -> Geometry<'_> {
        Geometry(self)
    }
}

impl<'a> Geometry<'a> {
    /// A certified placement's shared translation-free source and origin.
    /// Identity is borrowed only for one export; it never enters the packet.
    pub fn shared(self) -> Option<(&'a crate::GlyphEdges, Point)> {
        match &self.0.body {
            Body::Glyph(body) => Some((&body.edges, body.origin)),
            _ => None,
        }
    }

    /// Its fill rule.
    pub fn rule(self) -> FillRule {
        self.0.rule
    }

    /// Visits straight edges and true monotone quadratics, in accumulation
    /// order. A line repeats its endpoint in the last two positions.
    ///
    /// # Errors
    /// Propagates the visitor's allocation or work budget error.
    pub fn edges(self, mut visit: impl FnMut([Point; 3], bool) -> Result<()>) -> Result<()> {
        let mut pieces = |lines: &[crate::band::edges::Line],
                          curves: &[crate::band::curve::Quadratic],
                          origin: Point| {
            for line in lines {
                visit(
                    [line.a, line.b, line.b].map(|p| Point::new(p.x + origin.x, p.y + origin.y)),
                    false,
                )?;
            }
            for curve in curves {
                visit(
                    curve
                        .points
                        .map(|p| Point::new(p.x + origin.x, p.y + origin.y)),
                    true,
                )?;
            }
            Ok(())
        };
        match &self.0.body {
            Body::Rect(r) => {
                // Horizontal edges integrate to zero.
                visit(
                    [
                        Point::new(r.left, r.top),
                        Point::new(r.left, r.bottom),
                        Point::new(r.left, r.bottom),
                    ],
                    false,
                )?;
                visit(
                    [
                        Point::new(r.right, r.bottom),
                        Point::new(r.right, r.top),
                        Point::new(r.right, r.top),
                    ],
                    false,
                )
            }
            Body::Edges(body) => pieces(&body.lines, &body.curves, Point::default()),
            Body::Glyph(body) => {
                if let Some(relative) = body.edges.relative() {
                    pieces(&relative.lines, &relative.curves, body.origin)
                } else {
                    // A Glyph body is constructed only with certified relative edges.
                    Err(pagura_core::Error::invalid_argument(
                        "missing GPU glyph edges",
                    ))
                }
            }
        }
    }

    /// Exact exceptional cells already resolved during CPU preparation.
    pub fn corrections(self, y: u32) -> &'a [(u32, u8)] {
        self.0.stored.row(y)
    }

    /// Whether a non-rectangular clip overlays the shape.
    pub fn has_clip_rows(self) -> bool {
        self.0.overlay
    }

    /// Visits the clip's kept runs in one row. Interior runs have value 255;
    /// boundary cells keep their exact prepared value. Joint boundary cells
    /// are supplied separately by `corrections`, never multiplied.
    ///
    /// # Errors
    /// Propagates the visitor's allocation or work budget error.
    pub fn clips(self, y: u32, mut visit: impl FnMut(u32, u32, u8) -> Result<()>) -> Result<()> {
        if let Some(rows) = self
            .0
            .clip
            .as_ref()
            .filter(|_| self.0.overlay)
            .and_then(crate::ClipShape::rows)
        {
            for s in rows.row(y) {
                let value = if s.cell == INSIDE {
                    255
                } else {
                    rows.cell(s.cell).map_or(0, |(c, _, _)| c.value)
                };
                visit(s.x0, s.x1, value)?;
            }
        }
        Ok(())
    }
}

impl crate::GlyphEdges {
    /// Visits a certified source once, without placement translation.
    ///
    /// # Errors
    /// Missing relative geometry or the visitor's allocation/work error.
    pub fn gpu_edges(&self, mut visit: impl FnMut([Point; 3], bool) -> Result<()>) -> Result<()> {
        let relative = self.relative().ok_or(pagura_core::Error::invalid_argument(
            "missing shared glyph geometry",
        ))?;
        for line in &relative.lines {
            visit([line.a, line.b, line.b], false)?;
        }
        for curve in &relative.curves {
            visit(curve.points, true)?;
        }
        Ok(())
    }
}
