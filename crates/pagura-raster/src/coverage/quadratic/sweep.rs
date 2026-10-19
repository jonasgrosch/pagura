//! The curve sweep: the line sweep's row and vertex structure over pieces.
//!
//! Line pieces keep the line sweep's clipping, boundary normalization, winding
//! and cell integration. Only ordering and curved cells are specific here.

use super::{Piece, build, intersections};
use crate::Point;
use crate::coverage::{
    BoundaryEvent, Edge, Rectangle, Row, Shape, Windings, bytes, normalize_boundaries,
};
use pagura_core::{Error, Limits, Result};

/// Reused curve-sweep storage; line-only fills never allocate it.
#[derive(Debug, Default)]
pub(in crate::coverage) struct Sweep {
    row: Row,
    lines: Vec<Edge>,
    normalized: Vec<Edge>,
    boundary: Vec<BoundaryEvent>,
    pieces: Vec<Piece>,
    vertices: Vec<f64>,
    active: Vec<Piece>,
    winding: Windings,
    strip: intersections::Strip,
}

impl Sweep {
    pub(in crate::coverage) fn retained_bytes(&self) -> usize {
        [
            bytes(&self.row.areas),
            bytes(&self.row.covers),
            bytes(&self.row.touched),
            bytes(&self.lines),
            bytes(&self.normalized),
            bytes(&self.boundary),
            bytes(&self.pieces),
            bytes(&self.vertices),
            bytes(&self.active),
            bytes(&self.winding.values),
            self.strip.retained_bytes(),
        ]
        .into_iter()
        .fold(0, usize::saturating_add)
    }

    pub(in crate::coverage) fn render(
        &mut self,
        (width, height): (u32, u32),
        shapes: &[Shape],
        out: &mut [u8],
        origin: (u32, u32),
        limits: &Limits,
        anchor: Point,
    ) -> Result<()> {
        let row = &mut self.row;
        row.work = 0;
        row.maximum = limits.raster_work;
        row.maximum_events = limits.path_segments;
        let (bounds, groups) =
            Rectangle::intersection(shapes, origin, (width, height), row, limits)?;
        let stride =
            usize::try_from(width).map_err(|_| Error::limit("mask width exceeds usize"))?;
        let bounds = Rectangle {
            left: bounds.left - anchor.x,
            right: bounds.right - anchor.x,
            top: bounds.top - anchor.y,
            bottom: bounds.bottom - anchor.y,
        };
        row.reset(stride, origin.0)?;
        row.origin -= anchor.x;
        self.lines.clear();
        self.boundary.clear();
        self.pieces.clear();
        build::glyph_edges(
            shapes,
            (bounds, anchor),
            limits,
            row,
            (&mut self.lines, &mut self.pieces),
        )?;
        self.normalized.clear();
        normalize_boundaries(
            &mut self.lines,
            &mut self.normalized,
            &mut self.boundary,
            bounds.left,
            bounds.right,
            limits,
            row,
        )?;
        // Normalization numbered the lines; curves continue that sequence.
        let lines = self.normalized.len();
        for (id, piece) in (lines..).zip(&mut self.pieces) {
            piece.edge.id = id;
        }
        self.pieces
            .try_reserve(lines)
            .map_err(|_| Error::limit("curve sweep allocation failed"))?;
        self.pieces
            .extend(self.normalized.drain(..).map(Piece::line));
        self.sweep(stride, out, origin, groups, anchor.y)
    }

    fn sweep(
        &mut self,
        stride: usize,
        out: &mut [u8],
        origin: (u32, u32),
        groups: usize,
        anchor_y: f64,
    ) -> Result<()> {
        let Self {
            row,
            pieces,
            vertices,
            active,
            winding,
            strip,
            ..
        } = self;
        row.charge(pieces.len())?;
        row.sorted(pieces.len())?;
        pieces.sort_unstable_by(|a, b| a.edge.a.y.total_cmp(&b.edge.a.y));
        vertices.clear();
        vertices.extend(pieces.iter().flat_map(|p| [p.edge.a.y, p.edge.b.y]));
        row.sorted(vertices.len())?;
        vertices.sort_unstable_by(f64::total_cmp);
        vertices.dedup();
        let mut vertices = vertices.iter().copied().peekable();
        active.clear();
        let mut admitted = 0;
        winding.values.resize(groups, 0);
        out.fill(0);
        for (y, output) in out.chunks_exact_mut(stride).enumerate() {
            let lo = f64::from(u32::try_from(y).map_err(|_| Error::limit("row exceeds u32"))?)
                + f64::from(origin.1)
                - anchor_y;
            let hi = lo + 1.0;
            while vertices.next_if(|&y| y <= lo).is_some() {}
            if active.is_empty() && pieces.get(admitted).is_none_or(|p| p.edge.a.y >= hi) {
                continue;
            }
            let mut start = lo;
            loop {
                let end = vertices.peek().copied().unwrap_or(hi).min(hi);
                let mid = start.midpoint(end);
                row.charge(active.len())?;
                active.retain(|p| p.edge.b.y > mid);
                while let Some(&piece) = pieces.get(admitted)
                    && piece.edge.a.y < mid
                {
                    row.charge(1)?;
                    if piece.edge.b.y > mid {
                        active.push(piece);
                    }
                    admitted += 1;
                }
                if !active.is_empty() {
                    intersections::strip(active, start, end, winding, row, strip)?;
                }
                if end == hi {
                    break;
                }
                vertices.next();
                start = end;
            }
            row.finish(output)?;
        }
        Ok(())
    }
}
