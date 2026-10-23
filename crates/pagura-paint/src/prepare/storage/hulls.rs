//! Conservative device hulls of a node's stores, the footprints the
//! choice of formats compares, at page level so that they do not depend on
//! the region.

use crate::prepare::{bounds, index};
use pagura_core::{Error, Meter, Rect, Result};
use pagura_interpret::{Node, Scene};
use pagura_raster::{Point, Transform};

/// A device rectangle that covers every pixel.
const WHOLE: bounds::Hull = [f64::MIN, f64::MIN, f64::MAX, f64::MAX];

/// The smallest hull holding both.
pub(super) fn unite(a: Option<bounds::Hull>, b: Option<bounds::Hull>) -> Option<bounds::Hull> {
    match (a, b) {
        (Some(a), Some(b)) => Some([
            a[0].min(b[0]),
            a[1].min(b[1]),
            a[2].max(b[2]),
            a[3].max(b[3]),
        ]),
        (a, None) => a,
        (None, b) => b,
    }
}

/// The device hull of all of a node's stores.
pub(super) fn extent(
    scene: &Scene,
    node: &Node,
    t: Transform,
    bytes: &mut Meter,
) -> Result<Option<bounds::Hull>> {
    let Node::Glyphs(glyphs) = node else {
        let hulls = hulls(scene, node, t, bytes)?;
        let extent = hulls.iter().fold(None, |extent, &hull| unite(extent, hull));
        let released = hulls.capacity() * size_of::<Option<bounds::Hull>>();
        drop(hulls);
        bytes.refund(released);
        return Ok(extent);
    };
    // Each source's hull at the extreme origins of its placements.
    let mut origins = Vec::new();
    bytes.reserve_capacity(&mut origins, glyphs.sources.len())?;
    origins.resize(glyphs.sources.len(), None);
    for placement in &glyphs.placements {
        let origin = t.apply(placement.origin);
        if let Some(slot) = origins.get_mut(index(placement.source)) {
            *slot = unite(*slot, Some([origin.x, origin.y, origin.x, origin.y]));
        }
    }
    let device = Transform::new(t.a, t.b, t.c, t.d, 0.0, 0.0);
    let mut extent = None;
    let released = origins.capacity() * size_of::<Option<bounds::Hull>>();
    for (source, origins) in glyphs.sources.iter().zip(origins) {
        let Some([ox0, oy0, ox1, oy1]) = origins else {
            continue;
        };
        let l = source.linear;
        let linear = Transform::new(l.a, l.b, l.c, l.d, 0.0, 0.0).then(device);
        let hull = bounds::path(&source.outline, linear)
            .map(|[x0, y0, x1, y1]| bounds::expand([x0 + ox0, y0 + oy0, x1 + ox1, y1 + oy1], 1.0));
        extent = unite(extent, hull);
    }
    bytes.refund(released);
    Ok(extent)
}

/// The device hulls of a node's stores.
pub(super) fn hulls(
    scene: &Scene,
    node: &Node,
    t: Transform,
    bytes: &mut Meter,
) -> Result<Vec<Option<bounds::Hull>>> {
    let hull = match node {
        Node::Fill(fill) => fill_hull(fill, t),
        Node::Stroke(stroke) => stroke_hull(scene, stroke, t)?,
        Node::Glyphs(glyphs) => return glyph_hulls(glyphs, t, bytes),
        Node::Image(image) => rectangle(
            [
                Point::new(0.0, 0.0),
                Point::new(1.0, 0.0),
                Point::new(1.0, 1.0),
                Point::new(0.0, 1.0),
            ],
            image.transform.then(t),
        ),
        Node::Shading(shading) => shading.shading.bbox.map_or(Some(WHOLE), |b| {
            rectangle(corners(b), shading.transform.then(t))
        }),
        Node::Group(_) => return Ok(Vec::new()),
    };
    let mut hulls = Vec::new();
    bytes.reserve_capacity(&mut hulls, 1)?;
    hulls.push(hull);
    Ok(hulls)
}

fn corners(rect: Rect) -> [Point; 4] {
    [
        Point::new(rect.x0(), rect.y0()),
        Point::new(rect.x1(), rect.y0()),
        Point::new(rect.x1(), rect.y1()),
        Point::new(rect.x0(), rect.y1()),
    ]
}

fn rectangle(corners: [Point; 4], transform: Transform) -> Option<bounds::Hull> {
    bounds::points(corners.map(|point| transform.apply(point)))
}

/// A fill's device hull, a pixel wider for degenerate subpaths.
fn fill_hull(fill: &pagura_interpret::FillNode, transform: Transform) -> Option<bounds::Hull> {
    bounds::path(&fill.path, transform).map(|h| bounds::expand(h, 1.0))
}

/// A stroke's device hull: its path's, grown by the pen's reach.
fn stroke_hull(
    scene: &Scene,
    stroke: &pagura_interpret::StrokeNode,
    transform: Transform,
) -> Result<Option<bounds::Hull>> {
    let pen = scene
        .pens
        .get(stroke.pen.index())
        .ok_or(Error::invalid_argument("scene pen out of range"))?;
    Ok(bounds::path(&stroke.path, transform)
        .map(|h| bounds::expand(h, bounds::stroke_reach(pen, transform) + 2.0)))
}

/// The device hull of every placement of a glyph run.
fn glyph_hulls(
    glyphs: &pagura_interpret::GlyphsNode,
    transform: Transform,
    bytes: &mut Meter,
) -> Result<Vec<Option<bounds::Hull>>> {
    let t = transform;
    let device = Transform::new(t.a, t.b, t.c, t.d, 0.0, 0.0);
    let mut relative = Vec::new();
    bytes.reserve_capacity(&mut relative, glyphs.sources.len())?;
    relative.extend(glyphs.sources.iter().map(|source| {
        let l = source.linear;
        bounds::path(
            &source.outline,
            Transform::new(l.a, l.b, l.c, l.d, 0.0, 0.0).then(device),
        )
    }));
    let mut hulls = Vec::new();
    bytes.reserve_capacity(&mut hulls, glyphs.placements.len())?;
    hulls.extend(glyphs.placements.iter().map(|placement| {
        let origin = t.apply(placement.origin);
        relative
            .get(index(placement.source))
            .copied()
            .flatten()
            .map(|[x0, y0, x1, y1]| {
                bounds::expand(
                    [x0 + origin.x, y0 + origin.y, x1 + origin.x, y1 + origin.y],
                    1.0,
                )
            })
    }));
    let released = relative.capacity() * size_of::<Option<bounds::Hull>>();
    drop(relative);
    bytes.refund(released);
    Ok(hulls)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pagura_raster::Path;

    #[test]
    fn direct_corners_keep_path_hull_bits_and_order() {
        for rect in [
            Rect::from_corners(0.0, 0.0, 1.0, 1.0).unwrap(),
            Rect::from_corners(-3.25, -0.0, 2.75, 8.5).unwrap(),
            Rect::from_corners(-1e100, -2e100, 3e100, 4e100).unwrap(),
        ] {
            for transform in [
                Transform::IDENTITY,
                Transform::new(-0.0, 1.25, -2.0, 0.5, -0.0, 7.0),
                Transform::new(1e-90, -2e-90, 3e-90, 4e-90, 1e100, -1e100),
            ] {
                let bits = |hull: Option<bounds::Hull>| hull.map(|hull| hull.map(f64::to_bits));
                assert_eq!(
                    bits(rectangle(corners(rect), transform)),
                    bits(bounds::path(&Path::rect(rect), transform))
                );
            }
        }
    }
}
