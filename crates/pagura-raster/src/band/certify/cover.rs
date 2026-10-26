//! The covered set of a spline whose winding certificate fails, as parts of
//! its own pieces.
//!
//! Overlapping contours wind 2 where they overlap, where a cell's signed
//! area counts the overlap twice. The certificate's sweep is repeated:
//! between consecutive chain end heights every active chain spans the slab.
//! Chains whose x ranges over a slab overlap no other's keep their place
//! across it, and so do the chains of an overlapping group whose neighbours
//! the certificate finds apart (no two of them cross: Shamos and Hoey). The
//! winding left of each such chain is then the same at every height of the
//! slab, and the covered set's boundary there is the chains across which it
//! changes between zero and not zero. Each is kept over the slab, running
//! downward on the covered set's left and upward on its right, so that the
//! covered set winds one; the others are dropped.
//! Chains tracing the same path, by their pieces or as vertical sides at one
//! x, are kept or dropped together. Any other group is halved in height
//! until the width its chains stay within, times its height, is at most
//! [`LEAST`]; there its middle order is kept, and that area, once per chain
//! and once more, is added to an area allowance. Crossing chains stop near
//! 2^-20 pixel high, straight sides that coincide up to rounding after a
//! few halvings.
//!
//! A kept curve covers exact heights: the kernel takes a restricted piece's
//! ends from the same roots and measured residuals as a row boundary. A
//! straight piece is cut where a kept height ends inside it, and moving a
//! cut end by its interpolation error sweeps at most that error times the
//! piece's height, which is added to the allowance.

use super::{Down, Run, Work, chains_apart, spline_runs};
use crate::Point;
use crate::band::curve::{Mono, Spline};
use crate::band::edges::Line;
use crate::geometry::{greater, lesser};
use pagura_core::{Meter, Result};

/// The area, in square pixels, at or below which an overlapping group
/// keeps its middle order.
const LEAST: f64 = 1.0 / 1_099_511_627_776.0;

/// How often a neighbour test halves curved pieces before the slab itself
/// is halved: deeper tests repeat the slab's halving at a far higher cost.
const BISECTIONS: u32 = 4;

/// The covered set's boundary: straight pieces, then curves.
#[derive(Debug)]
pub(crate) struct Covered {
    pub(crate) lines: Vec<Line>,
    pub(crate) curves: Vec<Mono>,
    /// An area that bounds how far any pixel's covered area under these
    /// pieces can be from the spline's, beyond the kernel's measured
    /// arithmetic.
    pub(crate) allowance: f64,
}

/// The covered set of `spline`, whose monotone curves are `curves`, with
/// only its pieces' storage left charged to `bytes`. `None` when the
/// sweep's work bound is exceeded or a slab's winding does not return to
/// zero.
pub(crate) fn covered(
    spline: &Spline,
    curves: &[Mono],
    bytes: &mut Meter,
) -> Result<Option<Covered>> {
    let before = bytes.used();
    let result = sweep(spline, curves, bytes);
    let retained = result
        .as_ref()
        .ok()
        .and_then(Option::as_ref)
        .map_or(0, |c| {
            c.lines.capacity() * size_of::<Line>() + c.curves.capacity() * size_of::<Mono>()
        });
    bytes.refund(bytes.used().saturating_sub(before.saturating_add(retained)));
    result
}

/// A chain over one slab or part of it: its run, its piece containing the
/// top, and its x at the middle, at both ends and over the whole height.
#[derive(Debug, Clone, Copy)]
struct Strand {
    run: usize,
    piece: usize,
    mid: f64,
    ends: [f64; 2],
    range: [f64; 2],
}

/// A run kept over heights `lo..hi`, from its piece `piece` containing
/// `lo`, running downward or upward.
#[derive(Debug, Clone, Copy)]
struct Kept {
    run: usize,
    piece: usize,
    lo: f64,
    hi: f64,
    down: bool,
}

struct Chains<'a> {
    downs: &'a [Down],
    runs: &'a [Run],
    curves: &'a [Mono],
    margin: f64,
    work: Work,
    kept: Vec<Kept>,
    allowance: f64,
}

fn sweep(spline: &Spline, curves: &[Mono], bytes: &mut Meter) -> Result<Option<Covered>> {
    let (downs, runs, magnitude) = spline_runs(spline, bytes)?;
    let top = |r: &Run| downs[r.start].a.y;
    let bottom = |r: &Run| downs[r.end - 1].b.y;
    let mut heights: Vec<f64> = Vec::new();
    bytes.reserve(&mut heights, runs.len().saturating_mul(2))?;
    heights.extend(runs.iter().flat_map(|r| [top(r), bottom(r)]));
    heights.sort_unstable_by(f64::total_cmp);
    heights.dedup();
    let mut order = Vec::new();
    bytes.reserve(&mut order, runs.len())?;
    order.extend(0..runs.len());
    order.sort_unstable_by(|&a, &b| top(&runs[a]).total_cmp(&top(&runs[b])));
    let mut chains = Chains {
        downs: &downs,
        runs: &runs,
        curves,
        // As the certificate's: root solving errs by a few units of
        // roundoff of the largest coordinate.
        margin: magnitude * 2f64.powi(-40),
        work: Work {
            done: 0,
            cap: spline.pieces.len().saturating_mul(64).saturating_add(4096),
        },
        kept: Vec::new(),
        allowance: 0.0,
    };
    // Active runs and their pieces at the slab's top.
    let mut active: Vec<(usize, usize)> = Vec::new();
    bytes.reserve(&mut active, runs.len())?;
    let mut strands: Vec<Strand> = Vec::new();
    bytes.reserve(&mut strands, runs.len())?;
    let mut next = 0;
    for pair in heights.windows(2) {
        let (lo, hi) = (pair[0], pair[1]);
        active.retain(|a| bottom(&runs[a.0]) > lo);
        while let Some(&r) = order.get(next).filter(|&&r| top(&runs[r]) <= lo) {
            active.push((r, runs[r].start));
            next += 1;
        }
        if active.is_empty() {
            continue;
        }
        strands.clear();
        for a in &mut active {
            while a.1 + 1 < runs[a.0].end && downs[a.1].b.y <= lo {
                a.1 += 1;
            }
            strands.push(Strand {
                run: a.0,
                piece: a.1,
                mid: 0.0,
                ends: [0.0; 2],
                range: [0.0; 2],
            });
        }
        match chains.settle(&mut strands, (lo, hi), 0, bytes)? {
            Some(0) => {}
            _ => return Ok(None),
        }
    }
    let Chains {
        mut kept,
        allowance,
        mut work,
        ..
    } = chains;
    emit(
        &mut kept,
        (&downs, &runs, curves),
        (allowance, &mut work),
        bytes,
    )
}

impl Chains<'_> {
    /// The winding a chain adds from its left to its right, in units of
    /// a downward chain's.
    fn turn(&self, s: &Strand) -> i32 {
        if self.runs[s.run].down { 1 } else { -1 }
    }

    /// Advances `s` to its piece containing `lo` and measures it over
    /// `lo..hi`.
    fn measure(&self, s: &mut Strand, (lo, mid, hi): (f64, f64, f64)) {
        let end = self.runs[s.run].end;
        while s.piece + 1 < end && self.downs[s.piece].b.y <= lo {
            s.piece += 1;
        }
        let mut k = s.piece;
        let start = self.downs[k].x(self.curves, lo).0;
        let mut range = [start, start];
        loop {
            let piece = &self.downs[k];
            if piece.a.y <= mid && mid <= piece.b.y {
                s.mid = piece.x(self.curves, mid).0;
            }
            if piece.b.y >= hi || k + 1 >= end {
                let x = piece.x(self.curves, hi).0;
                range = [lesser(range[0], x), greater(range[1], x)];
                s.ends = [start, x];
                break;
            }
            range = [lesser(range[0], piece.b.x), greater(range[1], piece.b.x)];
            k += 1;
        }
        s.range = range;
    }

    /// Settles `strands` over `lo..hi`, given the winding left of them, and
    /// returns the winding right of them; `None` past the work bound.
    fn settle(
        &mut self,
        strands: &mut [Strand],
        (lo, hi): (f64, f64),
        mut winding: i32,
        bytes: &mut Meter,
    ) -> Result<Option<i32>> {
        if self.work.add(strands.len()).is_none() {
            return Ok(None);
        }
        let mid = lo.midpoint(hi);
        for s in strands.iter_mut() {
            self.measure(s, (lo, mid, hi));
        }
        // Groups of chains whose ranges overlap, left to right: no chain of
        // one group meets a chain of another anywhere in the slab.
        strands.sort_unstable_by(|a, b| a.range[0].total_cmp(&b.range[0]));
        let mut i = 0;
        while i < strands.len() {
            let mut right = strands[i].range[1];
            let mut j = i + 1;
            while j < strands.len() && strands[j].range[0] <= right + self.margin {
                right = greater(right, strands[j].range[1]);
                j += 1;
            }
            let left = strands[i].range[0];
            let group = &mut strands[i..j];
            group.sort_unstable_by(|a, b| a.mid.total_cmp(&b.mid));
            let turn: i32 = group.iter().map(|s| self.turn(s)).sum();
            let Some(apart) = self.apart(group, (lo, hi)) else {
                return Ok(None);
            };
            if apart {
                for s in group.iter() {
                    winding = self.keep(s, winding, (lo, hi), bytes)?;
                }
            } else if self.identical(group, (lo, hi)) {
                if (winding == 0) != (winding + turn == 0) {
                    self.keep_as(&group[0], winding == 0, (lo, hi), bytes)?;
                }
                winding += turn;
            } else if (hi - lo) * self.spread(group, (left, right), hi) <= LEAST
                || !(lo < mid && mid < hi)
            {
                for s in group.iter() {
                    winding = self.keep(s, winding, (lo, hi), bytes)?;
                }
                let chains = f64::from(u32::try_from(group.len() + 1).unwrap_or(u32::MAX));
                let area = (hi - lo) * self.spread(group, (left, right), hi);
                self.allowance = outward(self.allowance + area * chains);
            } else {
                for half in [(lo, mid), (mid, hi)] {
                    match self.settle(group, half, winding, bytes)? {
                        Some(w) if w == winding + turn => {}
                        _ => return Ok(None),
                    }
                }
                winding += turn;
            }
            i = j;
        }
        Ok(Some(winding))
    }

    /// A width the chains of `group` stay within at every height of the
    /// slab, which ends at `hi`, beyond which their order changes no
    /// winding: the most their ends spread where each is one straight piece
    /// over the slab, whose differences are linear, and otherwise their x
    /// ranges' extent `(left, right)`; both widened by the evaluation margin.
    fn spread(&self, group: &[Strand], (left, right): (f64, f64), hi: f64) -> f64 {
        let straight = group.iter().all(|s| {
            let piece = &self.downs[s.piece];
            piece.curve.is_none() && piece.b.y >= hi
        });
        let width = if straight {
            let at = |end: usize| {
                let (lo, hi) = group
                    .iter()
                    .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), s| {
                        (lesser(lo, s.ends[end]), greater(hi, s.ends[end]))
                    });
                hi - lo
            };
            greater(at(0), at(1))
        } else {
            right - left
        };
        width + 2.0 * self.margin
    }

    /// Whether no two chains of `group`, in middle order, cross over
    /// `lo..hi`; `None` past the work bound.
    fn apart(&mut self, group: &[Strand], (lo, hi): (f64, f64)) -> Option<bool> {
        for w in group.windows(2) {
            let (l, r) = (&w[0], &w[1]);
            // Ends in the other order cross for certain.
            if r.ends[0] - l.ends[0] < -self.margin || r.ends[1] - l.ends[1] < -self.margin {
                return Some(false);
            }
            let left = (l.piece, self.runs[l.run].end);
            let right = (r.piece, self.runs[r.run].end);
            if !chains_apart(
                (self.downs, self.curves),
                (left, right),
                (lo, hi),
                (self.margin, BISECTIONS),
                &mut self.work,
            )? {
                return Some(false);
            }
        }
        Some(true)
    }

    /// Whether every chain of `group` traces the same path over `lo..hi`:
    /// the same pieces, or vertical sides at one x.
    fn identical(&self, group: &[Strand], (lo, hi): (f64, f64)) -> bool {
        let same = |a: &Down, b: &Down| {
            a.a == b.a
                && a.b == b.b
                && match (a.curve, b.curve) {
                    (None, None) => true,
                    (Some(p), Some(q)) => self.curves[p].same_curve(&self.curves[q]),
                    _ => false,
                }
        };
        let pieces = |s: &Strand| {
            let end = self.runs[s.run].end;
            (s.piece..end)
                .map(|k| &self.downs[k])
                .take_while(move |p| p.a.y < hi)
                .filter(move |p| p.b.y > lo)
        };
        let vertical = |s: &Strand| {
            let mut x = None;
            for p in pieces(s) {
                if p.curve.is_some() || p.a.x != p.b.x || x.is_some_and(|x| x != p.a.x) {
                    return None;
                }
                x = Some(p.a.x);
            }
            x
        };
        let first = &group[0];
        if let Some(x) = vertical(first)
            && group[1..].iter().all(|s| vertical(s) == Some(x))
        {
            return true;
        }
        group[1..].iter().all(|s| {
            let (mut a, mut b) = (pieces(first), pieces(s));
            loop {
                match (a.next(), b.next()) {
                    (None, None) => break true,
                    (Some(p), Some(q)) if same(p, q) => {}
                    _ => break false,
                }
            }
        })
    }

    /// Keeps `s` over `lo..hi` where the winding changes between zero and
    /// not zero across it, and returns the winding right of it.
    fn keep(
        &mut self,
        s: &Strand,
        winding: i32,
        heights: (f64, f64),
        bytes: &mut Meter,
    ) -> Result<i32> {
        let after = winding + self.turn(s);
        if (winding == 0) != (after == 0) {
            self.keep_as(s, winding == 0, heights, bytes)?;
        }
        Ok(after)
    }

    /// Keeps `s` over `lo..hi` as the covered set's left side, running
    /// downward, or its right side, running upward: the covered set winds
    /// as a downward chain's right.
    fn keep_as(
        &mut self,
        s: &Strand,
        left: bool,
        (lo, hi): (f64, f64),
        bytes: &mut Meter,
    ) -> Result<()> {
        bytes.reserve_capacity(&mut self.kept, 1)?;
        self.kept.push(Kept {
            run: s.run,
            piece: s.piece,
            lo,
            hi,
            down: left,
        });
        Ok(())
    }
}

/// The pieces of the kept runs, each kept height range merged with the next
/// one of its run where they meet in the same direction; `None` past the
/// work bound. Each range visits only the pieces from the one containing
/// its top to the one containing its bottom.
fn emit(
    kept: &mut [Kept],
    (downs, runs, curves): (&[Down], &[Run], &[Mono]),
    (mut allowance, work): (f64, &mut Work),
    bytes: &mut Meter,
) -> Result<Option<Covered>> {
    kept.sort_unstable_by(|a, b| a.run.cmp(&b.run).then(a.lo.total_cmp(&b.lo)));
    let mut merged: Vec<Kept> = Vec::new();
    bytes.reserve(&mut merged, kept.len())?;
    for &k in kept.iter() {
        match merged.last_mut() {
            Some(last) if last.run == k.run && last.down == k.down && last.hi == k.lo => {
                last.hi = k.hi;
            }
            _ => merged.push(k),
        }
    }
    let mut lines = Vec::new();
    let mut monos = Vec::new();
    for k in &merged {
        let pieces = downs[k.piece..runs[k.run].end]
            .iter()
            .take_while(|piece| piece.a.y < k.hi);
        for piece in pieces {
            if work.add(1).is_none() {
                return Ok(None);
            }
            let (top, bottom) = (piece.a.y.max(k.lo), piece.b.y.min(k.hi));
            if top >= bottom {
                continue;
            }
            if let Some(c) = piece.curve {
                let direction = if k.down { 1.0 } else { -1.0 };
                bytes.reserve_capacity(&mut monos, 1)?;
                monos.push(curves[c].restricted(top, bottom, direction));
                continue;
            }
            let line = Line {
                a: piece.a,
                b: piece.b,
            };
            let at = |y: f64, end: Point| {
                if y == end.y {
                    end
                } else {
                    Point::new(line.x(y), y)
                }
            };
            let (a, b) = (at(top, piece.a), at(bottom, piece.b));
            if (a, b) != (piece.a, piece.b) {
                let error = line.interpolation_error().x;
                allowance = outward(allowance + error * (bottom - top));
            }
            bytes.reserve_capacity(&mut lines, 1)?;
            lines.push(if k.down {
                Line { a, b }
            } else {
                Line { a: b, b: a }
            });
        }
    }
    Ok(Some(Covered {
        lines,
        curves: monos,
        allowance,
    }))
}

fn outward(value: f64) -> f64 {
    value * (1.0 + 16.0 * f64::EPSILON)
}
