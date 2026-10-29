//! Mesh shadings (8.7.4.5.5–8.7.4.5.8): Gouraud triangles and tensor
//! patches. Where primitives overlap, the last in stream order wins.
//!
//! Primitives are binned by their bounds into a uniform grid over the mesh,
//! each cell listing its primitives last first, so that a sample tests only
//! the primitives whose bounds contain it, stopping at the first hit.

mod patch;

use super::{Input, Sampler, allocate};
use pagura_core::{Error, Meter, Result};
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
    pub(super) fn triangles(
        source: &[[MeshVertex; 3]],
        values: usize,
        bytes: &mut Meter,
    ) -> Result<Self> {
        let mut triangles = Vec::new();
        bytes.reserve(&mut triangles, source.len())?;
        for t in source {
            let points = [t[0].point, t[1].point, t[2].point];
            let mut colors = allocate(3 * values, bytes)?;
            for (vertex, out) in t.iter().zip(colors.chunks_exact_mut(values.max(1))) {
                for (out, &v) in out.iter_mut().zip(vertex.color.iter()) {
                    *out = v;
                }
            }
            triangles.push(Triangle {
                points,
                colors: colors.into(),
                bounds: Bounds::points(points),
            });
        }
        let mut bounds = Vec::new();
        bytes.reserve(&mut bounds, triangles.len())?;
        bounds.extend(triangles.iter().map(|t| t.bounds));
        let grid = Grid::new(&bounds, bytes)?;
        let released = bounds.capacity() * size_of::<Bounds>();
        drop(bounds);
        bytes.refund(released);
        Ok(Self {
            values,
            grid,
            triangles,
            patches: Vec::new(),
            kind: Kind::Triangles,
        })
    }

    pub(super) fn patches(source: &[Patch], values: usize, bytes: &mut Meter) -> Result<Self> {
        let mut patches = Vec::new();
        bytes.reserve(&mut patches, source.len())?;
        for patch in source {
            patches.push(patch::Patch::new(patch, values, bytes)?);
        }
        let mut bounds = Vec::new();
        bytes.reserve(&mut bounds, patches.len())?;
        bounds.extend(patches.iter().map(patch::Patch::bounds));
        let grid = Grid::new(&bounds, bytes)?;
        let released = bounds.capacity() * size_of::<Bounds>();
        drop(bounds);
        bytes.refund(released);
        Ok(Self {
            values,
            grid,
            triangles: Vec::new(),
            patches,
            kind: Kind::Patches,
        })
    }

    /// The most geometry work units a sample at `point` costs: one per
    /// primitive its cell lists, except the most a search costs for each
    /// patch whose bounds hold the sample (only those search), and one for
    /// an empty cell.
    pub(super) fn work_at(&self, point: Point) -> usize {
        let cell = self.grid.cell(point);
        let searched = match self.kind {
            Kind::Triangles => 0,
            Kind::Patches => cell
                .iter()
                .filter(|&&i| self.patches.get(i as usize).is_some_and(|p| p.holds(point)))
                .count(),
        };
        (cell.len() - searched)
            .saturating_add(searched.saturating_mul(patch::MAX_WORK))
            .max(1)
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
    fn new(bounds: &[Bounds], bytes: &mut Meter) -> Result<Self> {
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
        let mut counts = allocate::<u32>(cells + 1, bytes)?;
        let mut total = 0_usize;
        for b in bounds {
            let [x0, x1, y0, y1] = grid.cell_bounds(*b);
            let count = (x1 - x0 + 1) * (y1 - y0 + 1);
            total = total
                .checked_add(count)
                .filter(|&n| u32::try_from(n).is_ok())
                .ok_or(Error::limit("mesh shading grid exceeds its index range"))?;
            // Entries are reserved before visiting their cells; dense meshes
            // cannot do an unbounded counting pass before memory refusal.
            bytes.charge_items::<u32>(count)?;
            grid.each_cell(*b, |cell| counts[cell] += 1);
        }
        grid.occupancy = counts.iter().map(|&c| c as usize).max().unwrap_or(0);
        let mut starts = allocate(cells + 1, bytes)?;
        let mut running = 0_u32;
        for (start, &count) in starts.iter_mut().zip(&counts) {
            *start = running;
            running += count;
        }
        let mut next = allocate(cells + 1, bytes)?;
        next.copy_from_slice(&starts);
        let mut entries = Vec::new();
        entries
            .try_reserve_exact(total)
            .map_err(|_| Error::limit("mesh shading allocation failed"))?;
        entries.resize(total, 0_u32);
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
        let released = (counts.capacity() + next.capacity()) * size_of::<u32>();
        drop(counts);
        drop(next);
        bytes.refund(released);
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
        let [x0, x1, y0, y1] = self.cell_bounds(b);
        for y in y0..=y1 {
            for x in x0..=x1 {
                visit(y * self.columns + x);
            }
        }
    }

    fn cell_bounds(&self, b: Bounds) -> [usize; 4] {
        let x0 = Self::coordinate(b.lo.x, self.origin.x, self.scale.x, self.columns);
        let x1 = Self::coordinate(b.hi.x, self.origin.x, self.scale.x, self.columns);
        let y0 = Self::coordinate(b.lo.y, self.origin.y, self.scale.y, self.rows);
        let y1 = Self::coordinate(b.hi.y, self.origin.y, self.scale.y, self.rows);
        [x0, x1, y0, y1]
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

#[cfg(test)]
mod tests {
    use super::*;
    use pagura_color::Components;
    use pagura_core::ErrorKind;

    fn triangle() -> [MeshVertex; 3] {
        [
            Point::new(0.0, 0.0),
            Point::new(8.0, 0.0),
            Point::new(0.0, 8.0),
        ]
        .map(|point| MeshVertex {
            point,
            color: Components::from_slice(&[0.2, 0.3, 0.4]),
        })
    }

    #[test]
    fn mesh_storage_refunds_only_released_preparation_scratch() {
        let input = vec![triangle(); 17];
        let mut bytes = Meter::new(1 << 20, "mesh test");
        bytes.charge(13).unwrap();
        let mesh = Mesh::triangles(&input, 3, &mut bytes).unwrap();
        assert_eq!(bytes.used(), 13 + mesh.bytes());
        assert!(mesh.grid.occupancy > 1);
        let mut tiny = Meter::new(1, "mesh test");
        assert_eq!(
            Mesh::triangles(&input, 3, &mut tiny).unwrap_err().kind,
            ErrorKind::Limit
        );
        assert_eq!(tiny.used(), 0);
    }

    #[test]
    fn a_sample_costs_the_primitives_of_its_own_cell() {
        // One triangle across the mesh and many small ones in its corner:
        // the corner's cells list them all, the others only the large one.
        let mut input = vec![triangle()];
        for i in 0..48 {
            let offset = f64::from(i % 4) * 0.25;
            input.push(triangle().map(|vertex| MeshVertex {
                point: Point::new(vertex.point.x / 8.0 + offset, vertex.point.y / 8.0 + offset),
                ..vertex
            }));
        }
        let mut bytes = Meter::new(1 << 20, "mesh test");
        let mesh = Mesh::triangles(&input, 3, &mut bytes).unwrap();
        let points: Vec<Point> = (0..32)
            .flat_map(|y| (0..32).map(move |x| Point::new(f64::from(x) / 4.0, f64::from(y) / 4.0)))
            .collect();
        for &point in &points {
            assert_eq!(mesh.work_at(point), mesh.grid.cell(point).len().max(1));
        }
        assert!(
            points
                .iter()
                .any(|&p| mesh.work_at(p) == mesh.grid.occupancy)
        );
        let total: usize = points.iter().map(|&p| mesh.work_at(p)).sum();
        assert!(4 * total < points.len() * mesh.grid.occupancy, "{total}");
    }

    #[test]
    fn a_sample_reserves_searches_only_for_patches_whose_bounds_hold_it() {
        // A large patch, and a small one in its corner that the corner's
        // cells list too.
        let square = |size: f64| Patch {
            points: std::array::from_fn(|k| {
                let (i, j) = (u8::try_from(k % 4).unwrap(), u8::try_from(k / 4).unwrap());
                Point::new(size * f64::from(i) / 3.0, size * f64::from(j) / 3.0)
            }),
            colors: std::array::from_fn(|_| Components::from_slice(&[0.5])),
        };
        let mut bytes = Meter::new(1 << 20, "mesh test");
        let mesh = Mesh::patches(&[square(8.0), square(1.0)], 1, &mut bytes).unwrap();
        let mut input = Input::default();
        let (mut missed, mut total) = (0, 0);
        for y in 0..32 {
            for x in 0..32 {
                let point = Point::new(f64::from(x) / 4.0 + 0.125, f64::from(y) / 4.0 + 0.125);
                let listed = mesh.grid.cell(point);
                let held = listed
                    .iter()
                    .filter(|&&i| mesh.patches[i as usize].holds(point))
                    .count();
                for &i in listed {
                    let patch = &mesh.patches[i as usize];
                    // A patch whose bounds miss the sample cannot cover it.
                    if !patch.holds(point) {
                        assert!(!patch.sample(point, 1, &mut input));
                    }
                }
                let work = mesh.work_at(point);
                assert_eq!(work, (listed.len() - held + held * patch::MAX_WORK).max(1));
                missed += usize::from(held < listed.len());
                total += work;
            }
        }
        assert!(missed > 0);
        assert!(total < 32 * 32 * 2 * patch::MAX_WORK);
    }

    #[test]
    fn overlapping_meshes_refuse_their_grid_before_unbounded_growth() {
        let input = vec![triangle(); 1024];
        let mut bytes = Meter::new(256 * 1024, "mesh test");
        let error = Mesh::triangles(&input, 3, &mut bytes).unwrap_err();
        assert_eq!((error.kind, error.message), (ErrorKind::Limit, "mesh test"));
        assert!(bytes.used() <= bytes.max());
    }
}
