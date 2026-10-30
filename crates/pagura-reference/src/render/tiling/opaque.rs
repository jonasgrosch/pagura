//! Independent per-pixel witnesses for opaque cells. Unlike preparation's
//! global lattice certificate, this walks the requested pixels and tests
//! actual placed quadrilaterals against every corner of each device pixel.

use super::{Context, Contribution, Coverage, Renderer};
use pagura_color::ColorSpace;
use pagura_core::{Error, Meter, Result};
use pagura_interpret::{FillNode, GroupRole, Node, Paint, Tiling, TilingColors};
use pagura_raster::{Path, Point, Segment, Transform};

type Indices = ((i64, i64), (i64, i64));

impl<'s> Renderer<'s> {
    /// Each pixel must have one finite enumerated instance covering its
    /// entire square. This is sufficient for opaque Normal repeated paint;
    /// merely covering the plane with independently antialiased cells is not.
    pub(super) fn opaque_instances(
        &mut self,
        tiling: &Tiling,
        (coverage, geometric): (&Coverage, Option<&[crate::area::Region]>),
        device: Transform,
        indices: Indices,
        context: &Context<'_>,
    ) -> Result<Option<Vec<Contribution>>> {
        let Some(fill) = self.opaque_fill(tiling, geometric, context)? else {
            return Ok(None);
        };
        let state = self.state(&fill.mark)?;
        let Paint::Solid(color) = self.paint(fill.paint)? else {
            return Ok(None);
        };
        let mut memory = Meter::new(
            self.limits.render_bytes,
            "reference tiling memory budget exceeded",
        );
        let Some(quads) = self.opaque_quads(tiling, fill, &mut memory)? else {
            return Ok(None);
        };
        self.tiling_work.charge_product(quads.len(), 4)?;
        let Some(middle) = intersection_center(&quads) else {
            return Ok(None);
        };
        let sample = self.solid(color, state, context.target)?;
        let Some(inverse) = device.inverse() else {
            return Ok(None);
        };
        let rect = coverage.rect();
        let mut contributions = Vec::new();
        // Reserve once: repeated exact growth near the memory ceiling would
        // otherwise turn a bounded pixel traversal into quadratic copying.
        memory.reserve(&mut contributions, crate::area::pixels(rect))?;
        for y in rect.y0..rect.y1 {
            for x in rect.x0..rect.x1 {
                self.tiling_work.charge(1)?;
                let factor = coverage.get(x, y);
                if factor == 0.0 {
                    continue;
                }
                if !witness(
                    (x, y),
                    (&quads, middle),
                    tiling.step,
                    device,
                    inverse,
                    indices,
                    &mut self.tiling_work,
                )? {
                    return Ok(None);
                }
                contributions.push(Contribution {
                    x,
                    y,
                    color: sample.color,
                    alpha: factor,
                    shape: factor,
                });
            }
        }
        Ok(Some(contributions))
    }

    fn opaque_fill(
        &mut self,
        tiling: &Tiling,
        geometric: Option<&[crate::area::Region]>,
        context: &Context<'_>,
    ) -> Result<Option<&'s FillNode>> {
        if tiling.colors != TilingColors::Colored
            || geometric.is_some()
            || context.stencil.is_some()
            || !context.inherited.is_empty()
            || self.limits.display_commands == 0
        {
            return Ok(None);
        }
        if context.depth.saturating_add(1) >= self.limits.depth {
            return Err(Error::limit("compositing depth exceeded"));
        }
        self.tiling_work.charge(1)?;
        let cell = self
            .scene
            .group(tiling.cell)
            .ok_or(Error::invalid_argument("group id outside the scene"))?;
        if cell.role != GroupRole::Tiling {
            return Ok(None);
        }
        let [node] = cell.children.as_slice() else {
            return Ok(None);
        };
        let Some(Node::Fill(fill)) = self.scene.nodes.get(node.index()) else {
            return Ok(None);
        };
        let state = self.state(&fill.mark)?;
        if !state.is_plain()
            || state.alpha_is_shape
            || state.overprint.enabled
            || state.conversion.black_generation.is_some()
            || state.conversion.undercolor_removal.is_some()
        {
            return Ok(None);
        }
        let Paint::Solid(color) = self.paint(fill.paint)? else {
            return Ok(None);
        };
        if !matches!(color.space.as_ref(), ColorSpace::DeviceGray)
            || color.components.as_ref() != [0.0]
        {
            return Ok(None);
        }
        Ok(Some(fill))
    }

    fn opaque_quads(
        &mut self,
        tiling: &Tiling,
        fill: &FillNode,
        memory: &mut Meter,
    ) -> Result<Option<Vec<[Point; 4]>>> {
        let mut quads = Vec::new();
        let Some(points) = rectangle(&fill.path, &mut self.tiling_work)? else {
            return Ok(None);
        };
        memory.reserve_capacity(&mut quads, 1)?;
        quads.push(points);
        let b = tiling.bbox;
        let bbox = [
            Point::new(b.x0(), b.y0()),
            Point::new(b.x1(), b.y0()),
            Point::new(b.x1(), b.y1()),
            Point::new(b.x0(), b.y1()),
        ];
        if !bbox.iter().all(|p| p.within_working_range()) {
            return Ok(None);
        }
        memory.reserve_capacity(&mut quads, 1)?;
        quads.push(bbox);
        let mut clip = fill.mark.clip;
        let mut depth = 0;
        while let Some(id) = clip {
            self.tiling_work.charge(1)?;
            if depth >= self.limits.mask_sources {
                return Err(Error::limit("reference tiling clip budget exceeded"));
            }
            depth += 1;
            let entry = self
                .scene
                .clips
                .get(id.index())
                .ok_or(Error::invalid_argument("clip id outside the scene"))?;
            if entry
                .parent
                .is_some_and(|parent| parent.index() >= id.index())
            {
                return Ok(None);
            }
            let Some(points) = rectangle(&entry.path, &mut self.tiling_work)? else {
                return Ok(None);
            };
            memory.reserve_capacity(&mut quads, 1)?;
            quads.push(points);
            clip = entry.parent;
        }
        Ok(Some(quads))
    }
}

fn rectangle(path: &Path, work: &mut Meter) -> Result<Option<[Point; 4]>> {
    work.charge(1)?;
    if path.segments().len() != 5 {
        return Ok(None);
    }
    work.charge(9)?;
    let [
        Segment::MoveTo(a),
        Segment::LineTo(b),
        Segment::LineTo(c),
        Segment::LineTo(d),
        Segment::Close,
    ] = path.segments()
    else {
        return Ok(None);
    };
    let points = [*a, *b, *c, *d];
    if !points.iter().all(|p| p.within_working_range()) {
        return Ok(None);
    }
    let horizontal =
        a.y == b.y && b.x == c.x && c.y == d.y && d.x == a.x && a.x != b.x && b.y != c.y;
    let vertical = a.x == b.x && b.y == c.y && c.x == d.x && d.y == a.y && a.y != b.y && b.x != c.x;
    Ok((horizontal || vertical).then_some(points))
}

fn intersection_center(quads: &[[Point; 4]]) -> Option<Point> {
    let mut bounds = [
        f64::NEG_INFINITY,
        f64::NEG_INFINITY,
        f64::INFINITY,
        f64::INFINITY,
    ];
    for points in quads {
        bounds[0] = bounds[0].max(points[0].x.min(points[2].x));
        bounds[1] = bounds[1].max(points[0].y.min(points[2].y));
        bounds[2] = bounds[2].min(points[0].x.max(points[2].x));
        bounds[3] = bounds[3].min(points[0].y.max(points[2].y));
    }
    (bounds[0] < bounds[2] && bounds[1] < bounds[3])
        .then(|| Point::new(bounds[0].midpoint(bounds[2]), bounds[1].midpoint(bounds[3])))
}

fn witness(
    (x, y): (u32, u32),
    (quads, middle): (&[[Point; 4]], Point),
    step: [f64; 2],
    device: Transform,
    inverse: Transform,
    (columns, rows): Indices,
    work: &mut Meter,
) -> Result<bool> {
    let center = Point::new(f64::from(x) + 0.5, f64::from(y) + 0.5) * inverse;
    // The clipped ink's centre chooses a small candidate neighbourhood; it is not
    // itself evidence. Every accepted candidate is tested in device space.
    let raw = [
        (center.x - middle.x) / step[0],
        (center.y - middle.y) / step[1],
    ];
    if raw.iter().any(|v| !v.is_finite() || v.abs() > 4e15) {
        return Ok(false);
    }
    #[expect(
        clippy::cast_possible_truncation,
        reason = "rounded finite indices within ±4e15"
    )]
    let [column, row] = raw.map(|v| v.round() as i64);
    for j in -1..=1 {
        for i in -1..=1 {
            work.charge(1)?;
            let (column, row) = (column + i, row + j);
            if column < columns.0 || column > columns.1 || row < rows.0 || row > rows.1 {
                continue;
            }
            #[expect(
                clippy::cast_precision_loss,
                reason = "finite instance indices within ±4e15"
            )]
            let placed =
                Transform::translate(column as f64 * step[0], row as f64 * step[1]).then(device);
            let mut contained = true;
            for quad in quads {
                work.charge(128)?;
                let points = quad.map(|p| p * placed);
                if !points.iter().all(|p| p.within_working_range()) || !contains_pixel(points, x, y)
                {
                    contained = false;
                    break;
                }
            }
            if contained {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

/// A strict convex half-plane test, with a conservative error bound on the
/// subtraction/products in each determinant. Uncertain signs refuse the
/// witness. Reflection reverses every sign and is accepted symmetrically.
fn contains_pixel(points: [Point; 4], x: u32, y: u32) -> bool {
    let corners = [(x, y), (x + 1, y), (x + 1, y + 1), (x, y + 1)]
        .map(|(x, y)| Point::new(f64::from(x), f64::from(y)));
    let mut sign = 0.0;
    for (a, b) in points.iter().zip(points.iter().cycle().skip(1)).take(4) {
        for p in corners {
            let left = (b.x - a.x) * (p.y - a.y);
            let right = (b.y - a.y) * (p.x - a.x);
            let determinant = left - right;
            let error = 16.0 * f64::EPSILON * (left.abs() + right.abs()) + f64::MIN_POSITIVE;
            if !determinant.is_finite() || determinant.abs() <= error {
                return false;
            }
            if sign == 0.0 {
                sign = determinant;
            }
            if sign.is_sign_positive() != determinant.is_sign_positive() {
                return false;
            }
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strict_pixel_interior_accepts_reflection_and_refuses_edges() {
        let square = [
            Point::new(-1.0, -1.0),
            Point::new(2.0, -1.0),
            Point::new(2.0, 2.0),
            Point::new(-1.0, 2.0),
        ];
        assert!(contains_pixel(square, 0, 0));
        assert!(contains_pixel(
            [square[3], square[2], square[1], square[0]],
            0,
            0
        ));
        assert!(!contains_pixel(square, 1, 1));
        let diamond = [
            Point::new(-2.0, 0.5),
            Point::new(0.5, -2.0),
            Point::new(3.0, 0.5),
            Point::new(0.5, 3.0),
        ];
        assert!(contains_pixel(diamond, 0, 0));
    }

    #[test]
    fn candidate_must_belong_to_finite_enumerator_and_work_is_cumulative() {
        let quad = [
            Point::new(0.0, 0.0),
            Point::new(10.0, 0.0),
            Point::new(10.0, 10.0),
            Point::new(0.0, 10.0),
        ];
        let mut work = Meter::new(10000, "test work");
        assert!(
            witness(
                (0, 0),
                (&[quad], Point::new(5.0, 5.0)),
                [0.001; 2],
                Transform::IDENTITY,
                Transform::IDENTITY,
                ((-10000, 10000), (-10000, 10000)),
                &mut work
            )
            .unwrap()
        );
        assert!(
            !witness(
                (0, 0),
                (&[quad], Point::new(5.0, 5.0)),
                [0.001; 2],
                Transform::IDENTITY,
                Transform::IDENTITY,
                ((0, 1), (0, 1)),
                &mut work
            )
            .unwrap()
        );
        let used = work.used();
        assert!(used > 128);
        let mut tiny = Meter::new(1, "test work");
        assert!(
            witness(
                (0, 0),
                (&[quad], Point::new(5.0, 5.0)),
                [0.001; 2],
                Transform::IDENTITY,
                Transform::IDENTITY,
                ((-10000, 10000), (-10000, 10000)),
                &mut tiny
            )
            .is_err()
        );
    }

    #[test]
    fn offset_clip_selects_candidates_near_the_clipped_ink() {
        let ink = [
            Point::new(0.0, 0.0),
            Point::new(1200.0, 0.0),
            Point::new(1200.0, 1200.0),
            Point::new(0.0, 1200.0),
        ];
        let clip = [
            Point::new(1100.0, 1100.0),
            Point::new(1200.0, 1100.0),
            Point::new(1200.0, 1200.0),
            Point::new(1100.0, 1200.0),
        ];
        let middle = intersection_center(&[ink, clip]).unwrap();
        assert_eq!(middle, Point::new(1150.0, 1150.0));
        let device = Transform::translate(-1100.0, -1100.0);
        let mut work = Meter::new(10000, "test work");
        assert!(
            witness(
                (0, 0),
                (&[ink, clip], middle),
                [0.001; 2],
                device,
                device.inverse().unwrap(),
                ((-2_000_000, 2_000_000), (-2_000_000, 2_000_000)),
                &mut work
            )
            .unwrap()
        );
    }
}
