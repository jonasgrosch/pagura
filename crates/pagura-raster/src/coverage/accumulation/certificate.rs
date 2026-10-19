//! Sufficient winding certificates and exact integration of horizontal runs.

use super::super::{capacity_bytes, charge, inside, sort_work};
use super::geometry::{GRID, Geometry, reserve};
use super::scatter::{Piece, SpanEvent, add};
use pagura_core::{Limits, Result};

const AREA: i128 = 1_i128 << 41;

#[derive(Debug, Clone, Copy)]
struct GroupCell {
    group: usize,
    prefix: i64,
    position: [i64; 2],
}

#[derive(Debug, Default)]
pub(super) struct Certificates {
    hits: Vec<(i32, i64)>,
    levels: Vec<(i32, usize, i64)>,
    winding: Vec<i64>,
}

impl Certificates {
    pub(super) fn retained_bytes(&self) -> usize {
        capacity_bytes(&self.hits)
            .saturating_add(capacity_bytes(&self.levels))
            .saturating_add(capacity_bytes(&self.winding))
    }

    pub(super) fn cell(
        &mut self,
        g: &Geometry,
        pieces: &[Piece],
        prefix: &[i64],
        cell: [i64; 2],
        limits: &Limits,
        work: &mut usize,
    ) -> Result<Option<u8>> {
        let mut fractional = None;
        let mut unknown = false;
        for (group, &rule) in g.rules.iter().enumerate() {
            let mut pairs = 0;
            let mut value = 2 * i128::from(GRID) * i128::from(prefix[group]);
            let mut first = None;
            let mut one = true;
            let mut count = 0;
            for piece in pieces
                .iter()
                .filter(|p| g.group(p.segment as usize) == group)
            {
                charge(work, 1, limits.raster_work)?;
                count += 1;
                let chain = g.segments[piece.segment as usize].chain;
                if let Some(previous) = first {
                    one &= previous == chain;
                } else {
                    first = Some(chain);
                }
                let dy = i64::from(piece.b[1]) - i64::from(piece.a[1]);
                value += i128::from(2 * GRID - i64::from(piece.a[0]) - i64::from(piece.b[0]))
                    * i128::from(dy);
            }
            let interpolated = match rule {
                crate::FillRule::NonZero => value.abs().min(AREA),
                crate::FillRule::EvenOdd => {
                    let t = value.rem_euclid(2 * AREA);
                    t.min(2 * AREA - t)
                }
            };
            let value = if count == 0 {
                (prefix[group] % GRID == 0).then(|| {
                    if inside(prefix[group] / GRID, rule) {
                        AREA
                    } else {
                        0
                    }
                })
            } else if one {
                Some(interpolated)
            } else {
                self.walk(
                    g,
                    pieces,
                    GroupCell {
                        group,
                        prefix: prefix[group],
                        position: cell,
                    },
                    &mut pairs,
                    limits,
                    work,
                )?
                .and_then(|(lo, hi, parity)| {
                    if hi - lo <= 1 {
                        Some(interpolated)
                    } else if rule == crate::FillRule::NonZero && (lo > 0 || hi < 0) {
                        Some(AREA)
                    } else if rule == crate::FillRule::EvenOdd {
                        parity.map(|odd| if odd { AREA } else { 0 })
                    } else {
                        None
                    }
                })
            };
            match value {
                Some(0) => return Ok(Some(0)),
                Some(AREA) => {}
                Some(value) => {
                    if fractional.replace(value).is_some() {
                        unknown = true;
                    }
                }
                None => unknown = true,
            }
        }
        Ok((!unknown).then(|| round(fractional.unwrap_or(AREA), AREA)))
    }

    fn walk(
        &mut self,
        g: &Geometry,
        pieces: &[Piece],
        context: GroupCell,
        pairs: &mut usize,
        limits: &Limits,
        work: &mut usize,
    ) -> Result<Option<(i64, i64, Option<bool>)>> {
        let GroupCell {
            group,
            prefix,
            position: cell,
        } = context;
        self.hits.clear();
        let mut left = prefix;
        for (i, p) in pieces.iter().enumerate() {
            charge(work, 1, limits.raster_work)?;
            if g.group(p.segment as usize) != group {
                continue;
            }
            let contour = &g.contours[g.segments[p.segment as usize].contour as usize];
            if contour.enclosed == Some(cell) {
                return Ok(None);
            }
            for q in &pieces[..i] {
                charge(work, 1, limits.raster_work)?;
                if g.group(q.segment as usize) != group
                    || g.segments[p.segment as usize].chain == g.segments[q.segment as usize].chain
                {
                    continue;
                }
                *pairs += 1;
                if *pairs > 256 {
                    return Ok(None);
                }
                if intersects(p.a, p.b, q.a, q.b) && !shared_neighbour(g, *p, *q, cell) {
                    return Ok(None);
                }
            }
            if p.a[0] == 0 && p.b[0] == 0 {
                add(&mut left, i64::from(p.b[1]) - i64::from(p.a[1]))?;
                continue;
            }
            reserve(&mut self.hits, 2)?;
            let dx = i64::from(p.b[0]) - i64::from(p.a[0]);
            let dy = i64::from(p.b[1]) - i64::from(p.a[1]);
            for point in [p.a, p.b] {
                if let Some((position, delta)) = hit(point, dx, dy) {
                    self.hits.push((position, delta));
                }
            }
        }
        if self.hits.is_empty() {
            return Ok(None);
        }
        let n = self.hits.len();
        charge(work, sort_work(n)?, limits.raster_work)?;
        self.hits.sort_unstable_by_key(|h| h.0);
        let mut index = 0;
        let mut previous = 0;
        let mut sum = 0;
        let mut parities = 0_u8;
        let mut integral = 0;
        let mut lo = i64::MAX;
        let mut hi = i64::MIN;
        while index < self.hits.len() {
            let position = i64::from(self.hits[index].0);
            if position > previous {
                lo = lo.min(sum);
                hi = hi.max(sum);
                parities |= 1 << (sum & 1);
                integral += sum * (position.max(3 * GRID) - previous.max(3 * GRID));
            }
            while index < self.hits.len() && i64::from(self.hits[index].0) == position {
                add(&mut sum, self.hits[index].1)?;
                index += 1;
            }
            previous = position;
        }
        if previous < 4 * GRID {
            lo = lo.min(sum);
            hi = hi.max(sum);
            parities |= 1 << (sum & 1);
            integral += sum * (4 * GRID - previous.max(3 * GRID));
        }
        let difference = left - integral;
        if sum != 0 || difference % GRID != 0 {
            return Ok(None);
        }
        let base = difference / GRID;
        let parity = match parities {
            1 => Some(base & 1 != 0),
            2 => Some(base & 1 == 0),
            _ => None,
        };
        Ok(Some((base + lo, base + hi, parity)))
    }

    pub(super) fn run(
        &mut self,
        g: &Geometry,
        spans: &[SpanEvent],
        prefix: &[i64],
        limits: &Limits,
        work: &mut usize,
    ) -> Result<Option<u8>> {
        self.levels.clear();
        self.winding.clear();
        reserve(&mut self.winding, prefix.len())?;
        self.winding.extend_from_slice(prefix);
        reserve(&mut self.levels, spans.len())?;
        for span in spans {
            let segment = span.segment as usize;
            let next = g.next(segment);
            let group = g.group(segment);
            let delta = if g.points[next][0] > g.points[segment][0] {
                -1
            } else {
                1
            };
            add(
                &mut self.winding[group],
                -delta * (GRID - i64::from(span.height)),
            )?;
            self.levels.push((span.height, group, delta));
        }
        for value in &mut self.winding {
            if *value % GRID != 0 {
                return Ok(None);
            }
            *value /= GRID;
        }
        let n = spans.len();
        charge(
            work,
            sort_work(n)?
                .checked_add(g.rules.len())
                .ok_or_else(|| pagura_core::Error::limit("run work overflow"))?,
            limits.raster_work,
        )?;
        self.levels.sort_unstable_by_key(|p| p.0);
        let mut previous = 0;
        let mut area = 0;
        for &(height, group, delta) in &self.levels {
            if self
                .winding
                .iter()
                .zip(&g.rules)
                .all(|(&w, &r)| inside(w, r))
            {
                area += i64::from(height) - previous;
            }
            add(&mut self.winding[group], delta)?;
            previous = i64::from(height);
        }
        if self
            .winding
            .iter()
            .zip(&g.rules)
            .all(|(&w, &r)| inside(w, r))
        {
            area += GRID - previous;
        }
        Ok(Some(round(i128::from(area), i128::from(GRID))))
    }
}

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "clamped nonnegative area rounds to a byte"
)]
pub(super) fn round(numerator: i128, denominator: i128) -> u8 {
    ((255 * numerator.clamp(0, denominator) + denominator / 2) / denominator) as u8
}

fn hit(point: [i32; 2], dx: i64, dy: i64) -> Option<(i32, i64)> {
    let x = i64::from(point[0]);
    let y = i64::from(point[1]);
    let (position, mut delta) = if y == 0 {
        (x, dy)
    } else if x == GRID {
        (GRID + y, -dx)
    } else if y == GRID {
        (3 * GRID - x, -dy)
    } else if x == 0 {
        (4 * GRID - y, dx)
    } else {
        return None;
    };
    if delta == 0 {
        if x == 0 {
            delta = dx;
        } else if x == GRID {
            delta = -dx;
        }
        if delta == 0 {
            if y == 0 {
                delta = dy;
            } else if y == GRID {
                delta = -dy;
            }
        }
    }
    Some((i32::try_from(position % (4 * GRID)).ok()?, delta.signum()))
}

fn orient(a: [i32; 2], b: [i32; 2], c: [i32; 2]) -> i64 {
    (i64::from(b[0]) - i64::from(a[0])) * (i64::from(c[1]) - i64::from(a[1]))
        - (i64::from(b[1]) - i64::from(a[1])) * (i64::from(c[0]) - i64::from(a[0]))
}

fn on(a: [i32; 2], b: [i32; 2], p: [i32; 2]) -> bool {
    (0..2).all(|i| a[i].min(b[i]) <= p[i] && p[i] <= a[i].max(b[i]))
}

fn intersects(a: [i32; 2], b: [i32; 2], c: [i32; 2], d: [i32; 2]) -> bool {
    let (ac, ad, ca, cb) = (
        orient(a, b, c),
        orient(a, b, d),
        orient(c, d, a),
        orient(c, d, b),
    );
    (ac.signum() != ad.signum()
        && ca.signum() != cb.signum()
        && ac != 0
        && ad != 0
        && ca != 0
        && cb != 0)
        || (ac == 0 && on(a, b, c))
        || (ad == 0 && on(a, b, d))
        || (ca == 0 && on(c, d, a))
        || (cb == 0 && on(c, d, b))
}

fn shared_neighbour(g: &Geometry, p: Piece, q: Piece, cell: [i64; 2]) -> bool {
    let (a, b) = (p.segment as usize, q.segment as usize);
    if !g.neighbours(a, b) {
        return false;
    }
    let shared = g.points[if g.next(a) == b { b } else { a }];
    let shared = [shared[0] - cell[0] * GRID, shared[1] - cell[1] * GRID];
    let endpoint = |p: [i32; 2]| [i64::from(p[0]), i64::from(p[1])] == shared;
    if ![p.a, p.b].into_iter().any(endpoint) || ![q.a, q.b].into_iter().any(endpoint) {
        return false;
    }
    let dx = i64::from(p.b[0]) - i64::from(p.a[0]);
    let dy = i64::from(p.b[1]) - i64::from(p.a[1]);
    let ex = i64::from(q.b[0]) - i64::from(q.a[0]);
    let ey = i64::from(q.b[1]) - i64::from(q.a[1]);
    dx * ey - dy * ex != 0 || dx * ex + dy * ey > 0
}

#[cfg(test)]
mod tests;
