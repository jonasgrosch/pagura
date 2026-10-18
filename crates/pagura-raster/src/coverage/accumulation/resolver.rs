//! One-cell strip sweep, using source geometry for ghosts and dense cells.

use super::super::{Edge, Exact, RASTER_WORK, Rectangle, bytes, clipped, normalize_boundaries};
use super::geometry::{GRID, Geometry, reserve};
use super::scatter::{Active, Piece};
use crate::Point;
use pagura_core::{Error, Limits, Meter, Result};

#[derive(Debug, Default)]
pub(super) struct Resolver {
    scratch: Exact,
    sources: Vec<u32>,
    pieces: Vec<Piece>,
}

impl Resolver {
    pub(super) fn retained_bytes(&self) -> usize {
        self.scratch
            .retained_bytes()
            .saturating_add(bytes(&self.sources))
            .saturating_add(bytes(&self.pieces))
    }

    pub(super) fn prepare_row(&mut self, active: &[Active]) -> Result<()> {
        self.sources.clear();
        reserve(&mut self.sources, active.len())?;
        self.sources.extend(active.iter().map(|s| s.segment));
        self.sources.sort_unstable();
        Ok(())
    }

    pub(super) fn original(
        &mut self,
        g: &Geometry,
        cell: [i64; 2],
        limits: &Limits,
        work: &mut usize,
    ) -> Result<u8> {
        self.begin(limits, *work)?;
        let result = (|| {
            let x = coordinate(cell[0]);
            let y = coordinate(cell[1]);
            for &segment in &self.sources {
                self.scratch.row.charge(3)?;
                let segment = segment as usize;
                let group = g.group(segment);
                let (a, b) = (g.exact[segment], g.exact[g.next(segment)]);
                reserve(&mut self.scratch.raw, 3)?;
                clipped(
                    &mut self.scratch.raw,
                    Point::new(a.x - x, a.y - y),
                    Point::new(b.x - x, b.y - y),
                    group,
                    g.rules[group],
                    Rectangle {
                        left: 0.0,
                        top: 0.0,
                        right: 1.0,
                        bottom: 1.0,
                    },
                    limits,
                )?;
            }
            self.finish(g.rules.len(), limits)
        })();
        *work = self.scratch.row.work.used();
        result
    }

    pub(super) fn snapped(
        &mut self,
        g: &Geometry,
        pieces: &[Piece],
        prefix: &[i64],
        limits: &Limits,
        work: &mut usize,
    ) -> Result<Option<u8>> {
        self.begin(limits, *work)?;
        self.pieces.clear();
        reserve(&mut self.pieces, pieces.len())?;
        self.pieces.extend_from_slice(pieces);
        self.pieces.sort_unstable_by_key(|p| p.segment);
        let result = (|| {
            for p in &self.pieces {
                if p.a[1] == p.b[1] || (p.a[0] == 0 && p.b[0] == 0) {
                    continue;
                }
                let group = g.group(p.segment as usize);
                let (a, b, winding) = if p.a[1] < p.b[1] {
                    (p.a, p.b, 1)
                } else {
                    (p.b, p.a, -1)
                };
                reserve(&mut self.scratch.raw, 1)?;
                self.scratch.raw.push(Edge {
                    a: local(a),
                    b: local(b),
                    winding,
                    group,
                    rule: g.rules[group],
                    id: 0,
                });
            }
            for (group, &rule) in g.rules.iter().enumerate() {
                let mut left = prefix[group];
                let mut right = prefix[group];
                let mut moments = [0_i64; 2];
                for p in self
                    .pieces
                    .iter()
                    .filter(|p| g.group(p.segment as usize) == group)
                {
                    self.scratch.row.charge(1)?;
                    let dy = i64::from(p.b[1]) - i64::from(p.a[1]);
                    right += dy;
                    if p.a[0] == 0 && p.b[0] == 0 {
                        left += dy;
                        continue;
                    }
                    let delta = -(i64::from(p.b[0]) - i64::from(p.a[0])).signum();
                    for endpoint in [p.a, p.b] {
                        for (side, boundary) in [0_i32, 1 << 20].into_iter().enumerate() {
                            if endpoint[0] != boundary || delta == 0 {
                                continue;
                            }
                            moments[side] += delta * (GRID - i64::from(endpoint[1]));
                            if i64::from(endpoint[1]) < GRID {
                                reserve(&mut self.scratch.raw, 1)?;
                                self.scratch.raw.push(Edge {
                                    a: Point::new(
                                        f64::from(boundary) / 1_048_576.0,
                                        f64::from(endpoint[1]) / 1_048_576.0,
                                    ),
                                    b: Point::new(f64::from(boundary) / 1_048_576.0, 1.0),
                                    winding: if side == 0 { delta } else { -delta },
                                    group,
                                    rule,
                                    id: 0,
                                });
                            }
                        }
                    }
                }
                for (side, integral) in [left, right].into_iter().enumerate() {
                    let difference = integral - moments[side];
                    if difference % GRID != 0 {
                        return Ok(None);
                    }
                    let winding = difference / GRID * if side == 0 { 1 } else { -1 };
                    if winding != 0 {
                        reserve(&mut self.scratch.raw, 1)?;
                        let x = if side == 0 { 0.0 } else { 1.0 };
                        self.scratch.raw.push(Edge {
                            a: Point::new(x, 0.0),
                            b: Point::new(x, 1.0),
                            winding,
                            group,
                            rule,
                            id: 0,
                        });
                    }
                }
            }
            self.finish(g.rules.len(), limits).map(Some)
        })();
        *work = self.scratch.row.work.used();
        result
    }

    fn begin(&mut self, limits: &Limits, work: usize) -> Result<()> {
        self.scratch.row.work = Meter::new(limits.raster_work, RASTER_WORK);
        self.scratch.row.work.charge(work)?;
        self.scratch.row.maximum_events = limits.path_segments;
        self.scratch.row.reset(1, 0)?;
        self.scratch.raw.clear();
        self.scratch.edges.clear();
        self.scratch.boundary.clear();
        Ok(())
    }

    fn finish(&mut self, groups: usize, limits: &Limits) -> Result<u8> {
        if self.scratch.raw.len() > limits.path_segments {
            return Err(Error::limit("cell edge budget exceeded"));
        }
        reserve(
            &mut self.scratch.boundary,
            self.scratch
                .raw
                .len()
                .checked_mul(2)
                .ok_or(Error::limit("cell boundary count overflow"))?,
        )?;
        reserve(
            &mut self.scratch.edges,
            self.scratch
                .raw
                .len()
                .checked_mul(2)
                .ok_or(Error::limit("cell edge count overflow"))?,
        )?;
        normalize_boundaries(
            &mut self.scratch.raw,
            &mut self.scratch.edges,
            &mut self.scratch.boundary,
            0.0,
            1.0,
            limits,
            &mut self.scratch.row,
        )?;
        let mut output = [0];
        self.scratch
            .sweep_edges(1, &mut output, (0, 0), groups, 0.0)?;
        Ok(output[0])
    }
}

fn local(p: [i32; 2]) -> Point {
    Point::new(f64::from(p[0]) / 1_048_576.0, f64::from(p[1]) / 1_048_576.0)
}

#[expect(
    clippy::cast_precision_loss,
    reason = "cell indices come from u32 origins and sizes, so are exactly representable"
)]
fn coordinate(value: i64) -> f64 {
    value as f64
}
