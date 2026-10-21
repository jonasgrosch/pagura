//! Mesh shadings (8.7.4.5.5–8.7.4.5.8): Gouraud triangles and tensor
//! patches. Where primitives overlap, the last in stream order wins.
//!
//! Primitives are binned by their bounds into a uniform grid over the mesh,
//! each cell listing its primitives last first, so that a sample tests only
//! the primitives whose bounds contain it, stopping at the first hit.

mod patch;

use super::{Input, Sampler};
use pagura_core::{Error, Result};
use pagura_interpret::{MeshVertex, Patch};
use pagura_raster::Point;

/// The most cells the grid has along one axis.
const GRID: usize = 256;

/// A prepared mesh.
#[derive(Debug, Clone)]
pub(super) struct Mesh {
    /// Values per vertex: 1 with a function, the colour components without.
    values: usize,
    triangles: Vec<Triangle>,
    patches: Vec<patch::Patch>,
    /// Primitive `i` is triangle `i` or patch `i`: a mesh has one kind.
    kind: Kind,
    grid: Grid,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Triangles,
    Patches,
}

#[derive(Debug, Clone)]
struct Triangle {
    points: [Point; 3],
    /// Three vertices of `values` values each.
    colors: Box<[f64]>,
    bounds: Bounds,
}

/// Inclusive bounds of control points.
#[derive(Debug, Clone, Copy)]
pub(super) struct Bounds {
    lo: Point,
    hi: Point,
}

#[derive(Debug, Clone)]
struct Grid {
    origin: Point,
    /// Cells per unit along each axis.
    scale: Point,
    columns: usize,
    rows: usize,
    /// `cells[i]..cells[i + 1]` indexes `entries` for cell `i`.
    cells: Box<[u32]>,
    /// Primitive indices, last first within each cell.
    entries: Box<[u32]>,
    /// The longest cell list.
    occupancy: usize,
}

impl Mesh {
    pub(super) fn triangles(triangles: &[[MeshVertex; 3]], values: usize) -> Result<Self> {
        let triangles: Vec<Triangle> = triangles
            .iter()
            .map(|t| {
                let points = [t[0].point, t[1].point, t[2].point];
                let mut colors = vec![0.0; 3 * values];
                for (vertex, out) in t.iter().zip(colors.chunks_exact_mut(values.max(1))) {
                    for (out, &v) in out.iter_mut().zip(vertex.color.iter()) {
                        *out = v;
                    }
                }
                Triangle {
                    points,
                    colors: colors.into(),
                    bounds: Bounds::points(points),
                }
            })
            .collect();
        let bounds: Vec<Bounds> = triangles.iter().map(|t| t.bounds).collect();
        Ok(Self {
            values,
            grid: Grid::new(&bounds)?,
            triangles,
            patches: Vec::new(),
            kind: Kind::Triangles,
        })
    }

    pub(super) fn patches(patches: &[Patch], values: usize) -> Result<Self> {
        let patches: Vec<patch::Patch> = patches
            .iter()
            .map(|p| patch::Patch::new(p, values))
            .collect();
        let bounds: Vec<Bounds> = patches.iter().map(patch::Patch::bounds).collect();
        Ok(Self {
            values,
            grid: Grid::new(&bounds)?,
            triangles: Vec::new(),
            patches,
            kind: Kind::Patches,
        })
    }

    /// The most geometry work units one sample costs: one per primitive the
    /// densest cell lists, times the most a primitive's test costs.
    pub(super) fn work(&self) -> usize {
        let per_primitive = match self.kind {
            Kind::Triangles => 1,
            Kind::Patches => patch::MAX_WORK,
        };
        self.grid.occupancy.saturating_mul(per_primitive).max(1)
    }

    /// Bytes the prepared mesh holds.
    pub(super) fn bytes(&self) -> usize {
        let triangles = self.triangles.len() * (size_of::<Triangle>() + 3 * self.values * 8);
        let patches = self.patches.len() * patch::Patch::bytes(self.values);
        triangles + patches + 4 * (self.grid.cells.len() + self.grid.entries.len())
    }
}

impl Sampler for Mesh {
    fn sample(&self, point: Point, input: &mut Input) -> bool {
        for &index in self.grid.cell(point) {
            let index = index as usize;
            let hit = match self.kind {
                Kind::Triangles => self
                    .triangles
                    .get(index)
                    .is_some_and(|t| t.sample(point, self.values, input)),
                Kind::Patches => self
                    .patches
                    .get(index)
                    .is_some_and(|p| p.sample(point, self.values, input)),
            };
            if hit {
                return true;
            }
        }
        false
    }
}

impl Triangle {
    /// Gouraud interpolation by barycentric weights, accepting points on
    /// the edges within a relative tolerance.
    fn sample(&self, p: Point, values: usize, out: &mut Input) -> bool {
        if !self.bounds.contains(p) {
            return false;
        }
        let v = &self.points;
        let cross =
            |a: Point, b: Point, c: Point| (b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x);
        let area = cross(v[0], v[1], v[2]);
        if area == 0.0 {
            return false;
        }
        let w1 = cross(v[0], p, v[2]) / area;
        let w2 = cross(v[0], v[1], p) / area;
        let w0 = 1.0 - w1 - w2;
        if w0.min(w1).min(w2) < -1e-12 {
            return false;
        }
        let (c0, rest) = self.colors.split_at(values.min(self.colors.len()));
        let (c1, c2) = rest.split_at(values.min(rest.len()));
        for (i, out) in out.iter_mut().enumerate().take(values) {
            let c = |colors: &[f64]| colors.get(i).copied().unwrap_or(0.0);
            *out = w0 * c(c0) + w1 * c(c1) + w2 * c(c2);
        }
        true
    }
}

impl Bounds {
    pub(super) fn points(points: impl IntoIterator<Item = Point>) -> Self {
        let mut b = Self {
            lo: Point::new(f64::INFINITY, f64::INFINITY),
            hi: Point::new(f64::NEG_INFINITY, f64::NEG_INFINITY),
        };
        for p in points {
            b.lo.x = b.lo.x.min(p.x);
            b.lo.y = b.lo.y.min(p.y);
            b.hi.x = b.hi.x.max(p.x);
            b.hi.y = b.hi.y.max(p.y);
        }
        b
    }

    pub(super) fn contains(self, p: Point) -> bool {
        p.x >= self.lo.x && p.x <= self.hi.x && p.y >= self.lo.y && p.y <= self.hi.y
    }

    fn union(self, other: Self) -> Self {
        Self::points([self.lo, self.hi, other.lo, other.hi])
    }
}

impl Grid {
    fn new(bounds: &[Bounds]) -> Result<Self> {
        let all = bounds
            .iter()
            .copied()
            .reduce(Bounds::union)
            .unwrap_or_else(|| Bounds::points([Point::default()]));
        let side = |count: usize| count.isqrt().clamp(1, GRID);
        let (columns, rows) = (side(bounds.len()), side(bounds.len()));
        let extent = |lo: f64, hi: f64, cells: usize| {
            let size = hi - lo;
            if size > 0.0 && size.is_finite() {
                cells as f64 / size
            } else {
                0.0
            }
        };
        let mut grid = Self {
            origin: all.lo,
            scale: Point::new(
                extent(all.lo.x, all.hi.x, columns),
                extent(all.lo.y, all.hi.y, rows),
            ),
            columns,
            rows,
            cells: Box::new([]),
            entries: Box::new([]),
            occupancy: 0,
        };
        // Count, then fill the cells in reverse primitive order.
        let cells = columns * rows;
        let mut counts = vec![0_u32; cells + 1];
        for b in bounds {
            grid.each_cell(*b, |cell| counts[cell] += 1);
        }
        let total: usize = counts.iter().map(|&c| c as usize).sum();
        if u32::try_from(total).is_err() {
            return Err(Error::limit("mesh shading grid exceeds its index range"));
        }
        grid.occupancy = counts.iter().map(|&c| c as usize).max().unwrap_or(0);
        let mut starts = vec![0_u32; cells + 1];
        let mut running = 0_u32;
        for (start, &count) in starts.iter_mut().zip(&counts) {
            *start = running;
            running += count;
        }
        let mut next = starts.clone();
        let mut entries = vec![0_u32; total];
        for (index, b) in bounds.iter().enumerate().rev() {
            let index = u32::try_from(index).unwrap_or(u32::MAX);
            grid.each_cell(*b, |cell| {
                let slot = next[cell] as usize;
                entries[slot] = index;
                next[cell] += 1;
            });
        }
        grid.cells = starts.into();
        grid.entries = entries.into();
        Ok(grid)
    }

    /// The cell coordinate of `value` along an axis.
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "clamped to the grid before conversion"
    )]
    fn coordinate(value: f64, origin: f64, scale: f64, cells: usize) -> usize {
        let cell = ((value - origin) * scale).floor();
        if cell.is_nan() || cell < 0.0 {
            0
        } else {
            (cell as usize).min(cells - 1)
        }
    }

    fn each_cell(&self, b: Bounds, mut visit: impl FnMut(usize)) {
        let x0 = Self::coordinate(b.lo.x, self.origin.x, self.scale.x, self.columns);
        let x1 = Self::coordinate(b.hi.x, self.origin.x, self.scale.x, self.columns);
        let y0 = Self::coordinate(b.lo.y, self.origin.y, self.scale.y, self.rows);
        let y1 = Self::coordinate(b.hi.y, self.origin.y, self.scale.y, self.rows);
        for y in y0..=y1 {
            for x in x0..=x1 {
                visit(y * self.columns + x);
            }
        }
    }

    /// The primitives whose bounds may contain `p`, last first. Points
    /// outside the grid read the nearest cell; containment is tested exactly.
    fn cell(&self, p: Point) -> &[u32] {
        let x = Self::coordinate(p.x, self.origin.x, self.scale.x, self.columns);
        let y = Self::coordinate(p.y, self.origin.y, self.scale.y, self.rows);
        let cell = y * self.columns + x;
        let start = self.cells.get(cell).copied().unwrap_or(0) as usize;
        let end = self.cells.get(cell + 1).copied().unwrap_or(0) as usize;
        self.entries.get(start..end).unwrap_or(&[])
    }
}
