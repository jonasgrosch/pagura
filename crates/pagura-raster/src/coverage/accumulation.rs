//! Integer cell accumulation with conservative winding certificates.

mod certificate;
mod dda;
mod geometry;
mod resolver;
mod scatter;

use super::{Shape, capacity_bytes, sort_work};
use crate::IntRect;
use certificate::Certificates;
use geometry::{CELL, Geometry, reserve};
use pagura_core::convert::u32_to_usize;
use pagura_core::{Error, Limits, Meter, Result};
use resolver::Resolver;
use scatter::{DENSE, Piece, Scatter, SpanEvent, add};

#[derive(Debug, Default)]
pub(super) struct Accumulator {
    geometry: Geometry,
    scatter: Scatter,
    certificates: Certificates,
    resolver: Resolver,
    prefix: Vec<i64>,
    spans: Vec<SpanEvent>,
    cell: Vec<Piece>,
}

#[derive(Debug, Default)]
struct Cursor {
    piece: usize,
    risk: usize,
    span: usize,
    column: u32,
    originals: i64,
    ghosts: i64,
    prepared: bool,
}

impl Accumulator {
    pub(super) fn retained_bytes(&self) -> usize {
        [
            self.geometry.retained_bytes(),
            self.scatter.retained_bytes(),
            self.certificates.retained_bytes(),
            self.resolver.retained_bytes(),
            capacity_bytes(&self.prefix),
            capacity_bytes(&self.spans),
            capacity_bytes(&self.cell),
        ]
        .into_iter()
        .fold(0, usize::saturating_add)
    }

    /// Integrates `shapes` over `area` into `output`, continuing `work`.
    pub(super) fn render(
        &mut self,
        shapes: &[Shape],
        output: &mut [u8],
        area: IntRect,
        limits: &Limits,
        mut work: Meter,
    ) -> Result<()> {
        if shapes.is_empty() {
            output.fill(0);
            return Ok(());
        }
        let stride = u32_to_usize(area.width());
        let pixels = stride
            .checked_mul(u32_to_usize(area.height()))
            .ok_or(Error::limit("mask size overflow"))?;
        if output.len() != pixels {
            return Err(Error::invalid_argument("mask output size mismatch"));
        }
        self.geometry.build(shapes, limits, &mut work)?;
        self.scatter.prepare(&self.geometry, area, &mut work)?;
        self.prefix.clear();
        reserve(&mut self.prefix, self.geometry.rules.len())?;
        self.prefix.resize(self.geometry.rules.len(), 0);
        output.fill(0);
        for (row, out) in output.chunks_exact_mut(stride).enumerate() {
            let row = u32::try_from(row).map_err(|_| Error::limit("row exceeds u32"))?;
            self.row(out, area, row, limits, &mut work)?;
        }
        Ok(())
    }

    fn row(
        &mut self,
        out: &mut [u8],
        area: IntRect,
        row: u32,
        limits: &Limits,
        work: &mut Meter,
    ) -> Result<()> {
        let width = area.width();
        self.scatter.row(&self.geometry, area, row, work)?;
        self.prefix.copy_from_slice(&self.scatter.carry);
        self.spans.clear();
        let mut cursor = Cursor {
            originals: self.scatter.sparse_bound().unwrap_or(0),
            ..Cursor::default()
        };
        while cursor.column < width {
            self.events(&mut cursor)?;
            let mut end = cursor.piece;
            while self
                .scatter
                .pieces
                .get(end)
                .is_some_and(|p| p.column == cursor.column)
            {
                end += 1;
            }
            if end == cursor.piece {
                self.run(&mut cursor, out, area, row, limits, work)?;
                continue;
            }
            self.collect(cursor.piece..end, cursor.column)?;
            let cell = [
                i64::from(area.x()) + i64::from(cursor.column),
                i64::from(area.y()) + i64::from(row),
            ];
            out[u32_to_usize(cursor.column)] = self.value(&mut cursor, cell, limits, work)?;
            for piece in &self.scatter.pieces[cursor.piece..end] {
                add(
                    &mut self.prefix[self.geometry.group(u32_to_usize(piece.segment))],
                    i64::from(piece.b[1]) - i64::from(piece.a[1]),
                )?;
            }
            cursor.piece = end;
            cursor.column += 1;
        }
        Ok(())
    }

    fn events(&mut self, cursor: &mut Cursor) -> Result<()> {
        while let Some(event) = self
            .scatter
            .risks
            .get(cursor.risk)
            .filter(|e| e.column == cursor.column)
        {
            add(&mut cursor.originals, event.originals)?;
            add(&mut cursor.ghosts, event.ghosts)?;
            cursor.risk += 1;
        }
        if cursor.originals < 0 || cursor.ghosts < 0 {
            return Err(Error::invalid_argument("invalid source density interval"));
        }
        while let Some(event) = self
            .scatter
            .spans
            .get(cursor.span)
            .filter(|e| e.column == cursor.column)
        {
            if event.entering {
                reserve(&mut self.spans, 1)?;
                self.spans.push(*event);
            } else {
                let index = self
                    .spans
                    .iter()
                    .position(|span| span.segment == event.segment)
                    .ok_or(Error::invalid_argument("invalid horizontal span interval"))?;
                self.spans.swap_remove(index);
            }
            cursor.span += 1;
        }
        Ok(())
    }

    fn collect(&mut self, range: std::ops::Range<usize>, column: u32) -> Result<()> {
        self.cell.clear();
        reserve(&mut self.cell, range.len() + self.spans.len())?;
        self.cell.extend_from_slice(&self.scatter.pieces[range]);
        for span in &self.spans {
            let segment = u32_to_usize(span.segment);
            let (a, b) = ([0, span.height], [CELL, span.height]);
            let (a, b) = if self.geometry.points[self.geometry.next(segment)][0]
                > self.geometry.points[segment][0]
            {
                (a, b)
            } else {
                (b, a)
            };
            self.cell.push(Piece {
                column,
                segment: span.segment,
                a,
                b,
            });
        }
        Ok(())
    }

    fn value(
        &mut self,
        cursor: &mut Cursor,
        cell: [i64; 2],
        limits: &Limits,
        work: &mut Meter,
    ) -> Result<u8> {
        let unsnapped = cursor.ghosts > 0
            || cursor.originals > i64::from(DENSE)
            || self.cell.len() > u32_to_usize(DENSE);
        if !unsnapped {
            if let Some(value) =
                self.certificates
                    .cell(&self.geometry, &self.cell, &self.prefix, cell, work)?
            {
                #[cfg(debug_assertions)]
                if (cell[0] ^ cell[1]).trailing_zeros() >= 7
                    && let Some(reference) = self.resolver.snapped(
                        &self.geometry,
                        &self.cell,
                        &self.prefix,
                        limits,
                        work,
                    )?
                {
                    debug_assert!(
                        value.abs_diff(reference) <= 1,
                        "certified cell differs from its local sweep"
                    );
                }
                return Ok(value);
            }
            if let Some(value) =
                self.resolver
                    .snapped(&self.geometry, &self.cell, &self.prefix, limits, work)?
            {
                return Ok(value);
            }
        }
        self.prepare_resolver(&mut cursor.prepared, work)?;
        self.resolver.original(&self.geometry, cell, limits, work)
    }

    fn run(
        &mut self,
        cursor: &mut Cursor,
        out: &mut [u8],
        area: IntRect,
        row: u32,
        limits: &Limits,
        work: &mut Meter,
    ) -> Result<()> {
        let width = area.width();
        let next = [
            self.scatter.pieces.get(cursor.piece).map(|p| p.column),
            self.scatter.risks.get(cursor.risk).map(|r| r.column),
            self.scatter.spans.get(cursor.span).map(|s| s.column),
        ]
        .into_iter()
        .flatten()
        .min()
        .unwrap_or(width)
        .min(width);
        let value = if cursor.ghosts > 0 || cursor.originals > i64::from(DENSE) {
            None
        } else {
            self.certificates
                .run(&self.geometry, &self.spans, &self.prefix, work)?
        };
        if let Some(value) = value {
            out[u32_to_usize(cursor.column)..u32_to_usize(next)].fill(value);
        } else {
            self.prepare_resolver(&mut cursor.prepared, work)?;
            for column in cursor.column..next {
                out[u32_to_usize(column)] = self.resolver.original(
                    &self.geometry,
                    [
                        i64::from(area.x()) + i64::from(column),
                        i64::from(area.y()) + i64::from(row),
                    ],
                    limits,
                    work,
                )?;
            }
        }
        cursor.column = next;
        Ok(())
    }

    fn prepare_resolver(&mut self, prepared: &mut bool, work: &mut Meter) -> Result<()> {
        if !*prepared {
            work.charge(sort_work(self.scatter.active.len())?)?;
            self.resolver.prepare_row(&self.scatter.active)?;
            *prepared = true;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
