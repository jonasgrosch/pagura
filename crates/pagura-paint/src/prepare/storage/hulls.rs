//! Conservative device hulls of a node's stores, the footprints the
//! choice of formats compares, at page level so that they do not depend on
//! the region.

use crate::prepare::{bounds, index};
use pagura_core::hash::FastMap;
use pagura_core::{Error, Meter, Rect, Result};
use pagura_interpret::{GlyphSource, Node, OutlineKey, Scene};
use pagura_raster::{DeviceRect, Point, Transform};

/// Glyph outline hulls by outline and device linear map, relative to the
/// placement origin: text shows the same glyphs node after node.
pub(super) type Outlines = FastMap<(OutlineKey, [u64; 4]), Option<bounds::Hull>>;

/// The hull of `source`'s outline under its linear map and `device`.
fn outline(
    source: &GlyphSource,
    device: Transform,
    outlines: &mut Outlines,
    bytes: &mut Meter,
) -> Result<Option<bounds::Hull>> {
    let l = source.linear;
    let linear = Transform::new(l.a, l.b, l.c, l.d, 0.0, 0.0).then(device);
    let key = (
        source.key,
        [linear.a, linear.b, linear.c, linear.d].map(f64::to_bits),
    );
    if let Some(&hull) = outlines.get(&key) {
        return Ok(hull);
    }
    bytes.reserve_map_capacity(outlines, 1)?;
    let hull = bounds::path(&source.outline, linear);
    outlines.insert(key, hull);
    Ok(hull)
}

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
    (outlines, bytes): (&mut Outlines, &mut Meter),
) -> Result<Option<bounds::Hull>> {
    let Node::Glyphs(glyphs) = node else {
        let hulls = hulls(scene, node, t, (outlines, bytes))?;
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
        // Survey failures refund every charge, so `?` leaves nothing held.
        let hull = outline(source, device, outlines, bytes)?
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
    (outlines, bytes): (&mut Outlines, &mut Meter),
) -> Result<Vec<Option<bounds::Hull>>> {
    let hull = match node {
        Node::Fill(fill) => fill_hull(fill, t),
        Node::Stroke(stroke) => stroke_hull(scene, stroke, t)?,
        Node::Glyphs(glyphs) => return glyph_hulls(glyphs, t, (outlines, bytes)),
        Node::Image(image) => bounds::rect(
            Rect::from_corners(0.0, 0.0, 1.0, 1.0).unwrap_or_else(|| unreachable!()),
            image.transform.then(t),
        ),
        Node::Shading(shading) => shading
            .shading
            .bbox
            .map_or(Some(WHOLE), |b| bounds::rect(b, shading.transform.then(t))),
        Node::Group(_) => return Ok(Vec::new()),
    };
    let mut hulls = Vec::new();
    bytes.reserve_capacity(&mut hulls, 1)?;
    hulls.push(hull);
    Ok(hulls)
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

/// The pixels each placement of a glyph run can cover, as the disjoint-member
/// identity compares them (ADR 0004 §6): its outline's control hull under
/// the device map, at its origin. The rasterizer's geometry for a placement
/// lies in that hull up to a few thousand rounding units of the coordinates'
/// magnitude, and its far edges keep a geometric point's floor cell, so the
/// hull grown by 2^-32 of that magnitude and a margin of 2^-32 pixels loses
/// no covered pixel.
pub(super) fn glyph_footprints(
    glyphs: &pagura_interpret::GlyphsNode,
    transform: Transform,
    pixels: DeviceRect,
    (outlines, bytes): (&mut Outlines, &mut Meter),
) -> Result<Vec<DeviceRect>> {
    let t = transform;
    let device = Transform::new(t.a, t.b, t.c, t.d, 0.0, 0.0);
    let mut relative = Vec::new();
    bytes.reserve_capacity(&mut relative, glyphs.sources.len())?;
    for source in &glyphs.sources {
        relative.push(outline(source, device, outlines, bytes)?);
    }
    let mut footprints = Vec::new();
    bytes.reserve_capacity(&mut footprints, glyphs.placements.len())?;
    footprints.extend(glyphs.placements.iter().map(|placement| {
        let hull = relative.get(index(placement.source)).copied().flatten();
        hull.map_or(DeviceRect::default(), |hull| {
            footprint(hull, t.apply(placement.origin), pixels)
        })
    }));
    let released = relative.capacity() * size_of::<Option<bounds::Hull>>();
    drop(relative);
    bytes.refund(released);
    Ok(footprints)
}

/// The pixels of relative `hull` at `origin`. A positive margin makes the
/// rounded-up far edge include the floor cell of a point on it.
fn footprint([x0, y0, x1, y1]: bounds::Hull, origin: Point, pixels: DeviceRect) -> DeviceRect {
    let magnitude = |values: [f64; 2]| values[0].abs().max(values[1].abs());
    let scale =
        magnitude([x0, x1]).max(magnitude([y0, y1])) + magnitude([origin.x, origin.y]) + 1.0;
    let hull = [x0 + origin.x, y0 + origin.y, x1 + origin.x, y1 + origin.y];
    bounds::device(Some(bounds::expand(hull, scale / 4_294_967_296.0)), pixels)
}

/// The device hull of every placement of a glyph run.
fn glyph_hulls(
    glyphs: &pagura_interpret::GlyphsNode,
    transform: Transform,
    (outlines, bytes): (&mut Outlines, &mut Meter),
) -> Result<Vec<Option<bounds::Hull>>> {
    let t = transform;
    let device = Transform::new(t.a, t.b, t.c, t.d, 0.0, 0.0);
    let mut relative = Vec::new();
    bytes.reserve_capacity(&mut relative, glyphs.sources.len())?;
    for source in &glyphs.sources {
        relative.push(outline(source, device, outlines, bytes)?);
    }
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
    use crate::plan::ItemKind;
    use crate::testing::{Build, mark};
    use pagura_interpret::{FontKey, GlyphsNode, GroupId, Placement, StateId};
    use pagura_raster::{Path, PathBuilder};

    #[test]
    fn glyph_footprints_hold_the_pixels_placements_cover() {
        let point = |x: f64, y: f64| Point::new(x, y);
        let mut quadratic = PathBuilder::new();
        quadratic.move_to(point(0.0, 0.0));
        let _ = quadratic.quad_to(point(1.0, 3.0), point(2.0, 0.0));
        let _ = quadratic.quad_to(point(1.0, -0.5), point(0.0, 0.0));
        let mut cubic = PathBuilder::new();
        cubic.move_to(point(0.25, 0.0));
        let _ = cubic.cubic_to(point(-1.0, 2.0), point(3.0, 2.5), point(1.75, 0.0));
        cubic.close();
        let mut lone = PathBuilder::new();
        lone.move_to(point(0.5, 0.5));
        lone.close();
        let outlines = [quadratic.finish(), cubic.finish(), lone.finish()].map(std::sync::Arc::new);
        // Upright, vertical, sheared, small and singular maps.
        let maps = [
            Transform::new(1.0, 0.0, 0.0, 1.0, 0.0, 0.0),
            Transform::new(0.0, 1.5, -1.5, 0.0, 0.0, 0.0),
            Transform::new(1.25, 0.25, 0.75, -1.0, 0.0, 0.0),
            Transform::new(0.125, 0.0, 0.0, 0.125, 0.0, 0.0),
            Transform::new(0.0, 0.0, 0.0, 0.0, 0.0, 0.0),
        ];
        // On, beside and between pixel boundaries.
        let offsets = [0.0, 1e-13, -1e-13, 0.5, 0.999_999_999_999, 0.25];
        let mut b = Build::new(240.0, 240.0);
        let red = b.rgb([0.9, 0.2, 0.1]);
        let mut nodes = Vec::new();
        for (o, outline) in outlines.iter().enumerate() {
            for (m, &linear) in maps.iter().enumerate() {
                let source = GlyphSource {
                    outline: outline.clone(),
                    key: OutlineKey {
                        font: FontKey(1),
                        glyph: u32::try_from(o * maps.len() + m).unwrap_or(0),
                    },
                    linear,
                };
                let row = f64::from(u32::try_from(o * maps.len() + m).unwrap_or(0));
                let placements = offsets
                    .iter()
                    .enumerate()
                    .flat_map(|(i, &dx)| {
                        offsets.iter().map(move |&dy| Placement {
                            source: 0,
                            origin: point(
                                8.0 + 12.0 * f64::from(u32::try_from(i).unwrap_or(0)) + dx,
                                8.0 + 15.0 * row + dy,
                            ),
                        })
                    })
                    .collect();
                nodes.push(GlyphsNode {
                    sources: vec![source],
                    placements,
                    positions: Vec::new(),
                    paint: red,
                    mark: mark(StateId(0)),
                });
            }
        }
        let mut footprints = Vec::new();
        let mut cache = Outlines::default();
        let mut bytes = Meter::new(usize::MAX, "test");
        for glyphs in &nodes {
            let found = glyph_footprints(glyphs, b.transform(), b.page(), (&mut cache, &mut bytes));
            footprints.extend(found.unwrap_or_else(|e| panic!("{e}")));
        }
        for glyphs in nodes {
            b.node(GroupId(0), Node::Glyphs(glyphs));
        }
        let prepared = b
            .prepare(&b.request(b.page(), 32), &pagura_core::Limits::default())
            .unwrap_or_else(|e| panic!("{e}"));
        let mut covered = Vec::new();
        for item in &prepared.plan.items {
            match &item.kind {
                ItemKind::Run(run) => covered.extend(run.glyphs.iter().map(|g| g.bounds)),
                ItemKind::Mark(_) => covered.push(item.bounds),
                ItemKind::Group(_) => {}
            }
        }
        // Every placement meets the page; those of nothing paint nothing.
        assert!(
            covered.len() * 2 >= footprints.len(),
            "{} of {} placements cover pixels",
            covered.len(),
            footprints.len()
        );
        // Placements lie well apart, so the footprint holding a placement's
        // pixels is its own.
        for bounds in covered {
            assert!(
                footprints.iter().any(|f| f.x0 <= bounds.x0
                    && f.y0 <= bounds.y0
                    && bounds.x1 <= f.x1
                    && bounds.y1 <= f.y1),
                "{bounds:?} in no footprint"
            );
        }
    }

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
                    bits(bounds::rect(rect, transform)),
                    bits(bounds::path(&Path::rect(rect), transform))
                );
            }
        }
    }
}
