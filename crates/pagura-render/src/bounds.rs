//! Conservative painted-content bounds of a scene in default user space,
//! clamped to the crop box (12.3.2.2), for viewers that fit a page to its
//! content (`FitB`, 12.3.2.2 Table 151).
//!
//! The bounds are control hulls: curves, stroke envelopes, clips and group
//! bounding boxes can make them larger than the painted area, never
//! smaller. Marks whose paint changes no colorant (a Separation `None`)
//! contribute nothing. Annotation view flags introduce no zoom or rotation.

use pagura_core::{Error, Rect, Result};
use pagura_interpret::{
    ClipId, GlyphsNode, Group, ImageNode, MarkInfo, Node, Paint, PaintId, Pen, Scene, ShadingNode,
};
use pagura_raster::{Adjustment, Path, Point, Segment, Transform};

/// The bounds of everything `scene` paints, or `None` for a page that
/// paints nothing visible. Groups nest at most `depth` deep.
///
/// # Errors
///
/// Returns `Limit` when groups nest deeper than `depth`.
pub(crate) fn content_bounds(scene: &Scene, depth: usize) -> Result<Option<Rect>> {
    let Some(root) = scene.group(scene.root) else {
        return Ok(None);
    };
    group(scene, root, scene.page.crop, depth)
}

/// The union of the bounds of `group`'s children within `limit`.
fn group(scene: &Scene, group: &Group, limit: Rect, depth: usize) -> Result<Option<Rect>> {
    let depth = depth
        .checked_sub(1)
        .ok_or(Error::limit("content bounds nesting exceeded"))?;
    let mut total: Option<Rect> = None;
    for node in group.children.iter().filter_map(|&id| scene.node(id)) {
        let Some((bounds, mark)) = node_bounds(scene, node, limit, depth)? else {
            continue;
        };
        let bounds = bounds
            .and_then(|bounds| bounds.intersection(&limit))
            .and_then(|bounds| clipped(scene, mark.clip, bounds));
        if let Some(bounds) = bounds {
            total = Some(total.map_or(bounds, |total| total.union(&bounds)));
        }
    }
    Ok(total)
}

/// The bounds of one node before its clip, with the mark it is clipped and
/// composited by; `None` for a node that paints nothing.
fn node_bounds(
    scene: &Scene,
    node: &Node,
    limit: Rect,
    depth: usize,
) -> Result<Option<(Option<Rect>, MarkInfo)>> {
    Ok(match node {
        Node::Fill(fill) => paints(scene, fill.paint).then(|| (outline(&fill.path), fill.mark)),
        Node::Stroke(stroke) => paints(scene, stroke.paint).then(|| {
            let pen = scene.pens.get(stroke.pen.index());
            let bounds = outline(&stroke.path).and_then(|r| pen.and_then(|pen| widen(r, pen)));
            (bounds, stroke.mark)
        }),
        Node::Glyphs(glyphs) => paints(scene, glyphs.paint).then(|| (placed(glyphs), glyphs.mark)),
        Node::Image(image) => image_paints(scene, image).then(|| {
            let corners = [(0.0, 0.0), (1.0, 0.0), (0.0, 1.0), (1.0, 1.0)];
            let corners = corners.map(|(x, y)| image.transform.apply(Point::new(x, y)));
            (points_hull(corners), image.mark)
        }),
        Node::Shading(shading) => shading
            .shading
            .space
            .paints()
            .then(|| (shading_bounds(shading, scene.page.crop), shading.mark)),
        Node::Group(id) => {
            let Some(inner) = scene.group(*id) else {
                return Ok(None);
            };
            let limit = match &inner.bbox {
                Some(bbox) => hull(bbox, Transform::IDENTITY).and_then(|r| r.intersection(&limit)),
                None => Some(limit),
            };
            let bounds = match limit {
                Some(limit) => group(scene, inner, limit, depth)?,
                None => None,
            };
            Some((bounds, inner.mark))
        }
    })
}

/// `bounds` within every path of the clip chain from `clip` up.
fn clipped(scene: &Scene, mut clip: Option<ClipId>, mut bounds: Rect) -> Option<Rect> {
    // A chain visits each clip at most once; the count also stops a cycle.
    for _ in 0..scene.clips.len() {
        let Some(node) = clip.and_then(|id| scene.clips.get(id.index())) else {
            return Some(bounds);
        };
        bounds = hull(&node.path, Transform::IDENTITY)?.intersection(&bounds)?;
        clip = node.parent;
    }
    clip.is_none().then_some(bounds)
}

/// Whether the paint changes any colorant.
fn paints(scene: &Scene, paint: PaintId) -> bool {
    scene
        .paints
        .get(paint.index())
        .is_none_or(|paint| match paint.as_ref() {
            Paint::Solid(color) => color.space.paints(),
            Paint::Shading(shading) => shading.shading.space.paints(),
            Paint::Tiling(_) => true,
        })
}

fn image_paints(scene: &Scene, image: &ImageNode) -> bool {
    match (&image.image.space, image.stencil) {
        (Some(space), _) => space.paints(),
        (None, Some(paint)) => paints(scene, paint),
        (None, None) => true,
    }
}

/// The hull of a path, grown by one unit when it has a degenerate contour
/// of one point, which a round cap or a reference-device cell can paint
/// (8.5.3.3).
fn outline(path: &Path) -> Option<Rect> {
    let rect = hull(path, Transform::IDENTITY)?;
    if point_contour(path) {
        grow(rect, 1.0, 1.0)
    } else {
        Some(rect)
    }
}

/// Whether some contour of `path` is a single point.
fn point_contour(path: &Path) -> bool {
    let mut start = None;
    let mut point = false;
    for segment in path.segments() {
        match *segment {
            Segment::MoveTo(p) => {
                if point {
                    return true;
                }
                start = Some(p);
                point = true;
            }
            Segment::LineTo(p) => point &= start == Some(p),
            Segment::CubicTo { c1, c2, end } => {
                point &= [c1, c2, end].into_iter().all(|p| start == Some(p));
            }
            Segment::QuadTo { control, end } => {
                point &= [control, end].into_iter().all(|p| start == Some(p));
            }
            Segment::Close => {}
        }
    }
    point
}

/// A stroke's hull grown by its pen: half the line width in pen space, as
/// far as the pen transform stretches it along each axis, times the miter
/// limit (at least 2, which covers square caps), plus half a unit under
/// stroke adjustment.
fn widen(rect: Rect, pen: &Pen) -> Option<Rect> {
    let t = pen.transform;
    let half = pen.stroke.width * 0.5;
    let factor = pen.stroke.miter_limit.max(2.0);
    let adjust = match pen.adjustment {
        Adjustment::Adjusted => 0.5,
        Adjustment::None => 0.0,
    };
    let pad = |stretch: f64| (half * stretch + adjust).max(0.5) * factor;
    grow(rect, pad(t.a.hypot(t.c)), pad(t.b.hypot(t.d)))
}

/// The placements of a glyph run, grown by one unit.
fn placed(glyphs: &GlyphsNode) -> Option<Rect> {
    let sources: Vec<Option<Rect>> = glyphs
        .sources
        .iter()
        .map(|source| hull(&source.outline, source.linear))
        .collect();
    let mut total: Option<Rect> = None;
    for placement in &glyphs.placements {
        let source = usize::try_from(placement.source).ok();
        let Some(Some(glyph)) = source.and_then(|index| sources.get(index)) else {
            continue;
        };
        let Point { x, y } = placement.origin;
        let Some(moved) = Rect::from_corners(
            glyph.x0() + x,
            glyph.y0() + y,
            glyph.x1() + x,
            glyph.y1() + y,
        ) else {
            continue;
        };
        total = Some(total.map_or(moved, |total| total.union(&moved)));
    }
    total.and_then(|total| grow(total, 1.0, 1.0))
}

/// A shading's `BBox` under its transform (grown by a unit when it is
/// empty), or the crop box for a shading without one.
fn shading_bounds(shading: &ShadingNode, crop: Rect) -> Option<Rect> {
    let Some(bbox) = shading.shading.bbox else {
        return Some(crop);
    };
    let rect = hull(&Path::rect(bbox), shading.transform)?;
    if bbox.is_empty() {
        grow(rect, 1.0, 1.0)
    } else {
        Some(rect)
    }
}

/// The hull of the control points of `path` under `transform`; `None` for a
/// path without points or with a coordinate that is not finite.
fn hull(path: &Path, transform: Transform) -> Option<Rect> {
    points_hull(path.segments().iter().flat_map(|segment| {
        let points = match *segment {
            Segment::MoveTo(p) | Segment::LineTo(p) => [Some(p), None, None],
            Segment::CubicTo { c1, c2, end } => [Some(c1), Some(c2), Some(end)],
            Segment::QuadTo { control, end } => [Some(control), Some(end), None],
            Segment::Close => [None; 3],
        };
        points
            .into_iter()
            .flatten()
            .map(move |p| transform.apply(p))
    }))
}

/// The smallest rectangle holding `points`; `None` for no points or a
/// coordinate that is not finite.
fn points_hull(points: impl IntoIterator<Item = Point>) -> Option<Rect> {
    let mut points = points.into_iter();
    let first = points.next()?;
    let [x0, y0, x1, y1] = points.fold([first.x, first.y, first.x, first.y], |r, p| {
        [r[0].min(p.x), r[1].min(p.y), r[2].max(p.x), r[3].max(p.y)]
    });
    Rect::from_corners(x0, y0, x1, y1)
}

fn grow(rect: Rect, dx: f64, dy: f64) -> Option<Rect> {
    Rect::from_corners(
        rect.x0() - dx,
        rect.y0() - dy,
        rect.x1() + dx,
        rect.y1() + dy,
    )
}

#[cfg(test)]
mod tests;
