//! Conservative device hulls of a node's stores, the footprints the
//! choice of formats compares, at page level so that they do not depend on
//! the region.

use crate::prepare::{bounds, index};
use pagura_core::{Error, Result};
use pagura_interpret::{Node, Scene};
use pagura_raster::{Path, Transform};

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
pub(super) fn extent(scene: &Scene, node: &Node, t: Transform) -> Result<Option<bounds::Hull>> {
    let Node::Glyphs(glyphs) = node else {
        return Ok(hulls(scene, node, t)?
            .iter()
            .fold(None, |extent, &hull| unite(extent, hull)));
    };
    // Each source's hull at the extreme origins of its placements.
    let mut origins: Vec<Option<bounds::Hull>> = vec![None; glyphs.sources.len()];
    for placement in &glyphs.placements {
        let origin = t.apply(placement.origin);
        if let Some(slot) = origins.get_mut(index(placement.source)) {
            *slot = unite(*slot, Some([origin.x, origin.y, origin.x, origin.y]));
        }
    }
    let device = Transform::new(t.a, t.b, t.c, t.d, 0.0, 0.0);
    let mut extent = None;
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
    Ok(extent)
}

/// The device hulls of a node's stores.
pub(super) fn hulls(scene: &Scene, node: &Node, t: Transform) -> Result<Vec<Option<bounds::Hull>>> {
    Ok(match node {
        Node::Fill(fill) => vec![fill_hull(fill, t)],
        Node::Stroke(stroke) => vec![stroke_hull(scene, stroke, t)?],
        Node::Glyphs(glyphs) => glyph_hulls(glyphs, t),
        Node::Image(image) => vec![bounds::path(
            &bounds::unit_square(),
            image.transform.then(t),
        )],
        Node::Shading(shading) => vec![shading.shading.bbox.map_or(Some(WHOLE), |b| {
            bounds::path(&Path::rect(b), shading.transform.then(t))
        })],
        Node::Group(_) => Vec::new(),
    })
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
) -> Vec<Option<bounds::Hull>> {
    let t = transform;
    let device = Transform::new(t.a, t.b, t.c, t.d, 0.0, 0.0);
    let relative: Vec<Option<bounds::Hull>> = glyphs
        .sources
        .iter()
        .map(|source| {
            let l = source.linear;
            bounds::path(
                &source.outline,
                Transform::new(l.a, l.b, l.c, l.d, 0.0, 0.0).then(device),
            )
        })
        .collect();
    glyphs
        .placements
        .iter()
        .map(|placement| {
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
        })
        .collect()
}
