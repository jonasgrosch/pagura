//! Conservative group allocation bounds: source geometry, hard clips and tile.
//! Cubic control hulls contain the curve; stroke padding includes miters/caps.
use super::{Group, Tile};
use crate::display::{Clip, Command, Mark, PathCommand};
use pagura_core::{Error, Result};
use pagura_raster::{Path, Point, Segment, Transform};
use std::{
    collections::HashMap,
    sync::{Arc, Weak},
};

// Weak identities prevent address reuse without retaining temporary path buffers.
// Optional bookkeeping is bounded independently of the group surfaces.
pub(super) struct Prepared {
    device: Transform,
    maximum: usize,
    hulls: HashMap<usize, (Weak<Path>, [Point; 2])>,
}
impl Prepared {
    pub(super) fn new(device: Transform, bytes: usize) -> Self {
        Self {
            device,
            maximum: bytes
                / 16
                / (size_of::<(usize, (Weak<Path>, [Point; 2]))>()
                    + size_of::<Path>()
                    + 2 * size_of::<usize>()
                    + 64),
            hulls: HashMap::new(),
        }
    }
}

pub(super) fn group_region(
    group: &Group,
    tile: Tile,
    device: Transform,
    work: &mut usize,
    maximum: usize,
    prepared: &mut Prepared,
) -> Result<Option<Tile>> {
    bounded_group(
        group,
        tile,
        device,
        &mut Work {
            used: work,
            maximum,
            prepared,
        },
    )
}
struct Work<'a> {
    used: &'a mut usize,
    maximum: usize,
    prepared: &'a mut Prepared,
}
impl Work<'_> {
    fn charge(&mut self, count: usize) -> Result<()> {
        *self.used = self
            .used
            .checked_add(count)
            .filter(|n| *n <= self.maximum)
            .ok_or(Error::limit("group bounds work exceeded"))?;
        Ok(())
    }
}
fn bounded_group(
    group: &Group,
    tile: Tile,
    device: Transform,
    work: &mut Work<'_>,
) -> Result<Option<Tile>> {
    let mut region = tile;
    if let Some(path) = &group.bounds {
        let Some(bounds) = retained_region(path, device, tile, (0.0, 0.0), work)? else {
            return Ok(None);
        };
        region = bounds;
    }
    commands_region(&group.commands, region, device, work)
}
fn commands_region(
    commands: &[Command],
    tile: Tile,
    device: Transform,
    work: &mut Work<'_>,
) -> Result<Option<Tile>> {
    work.charge(commands.len())?;
    let mut total = None;
    for command in commands {
        let (region, clip) = match command {
            Command::Path(p) => (paint_region(p, tile, device, work)?, p.clip.as_ref()),
            Command::Image(i) => {
                let mut path = pagura_raster::PathBuilder::new();
                path.rect(0.0, 0.0, 1.0, 1.0);
                (
                    path_region(
                        &path.finish(),
                        i.transform.then(device),
                        tile,
                        (0.0, 0.0),
                        work,
                    )?,
                    i.clip.as_ref(),
                )
            }
            Command::Shading(s) => (Some(tile), s.clip.as_ref()),
            Command::Group(g) if g.attributes.view.is_some() => (Some(tile), None),
            Command::Group(g) => (bounded_group(g, tile, device, work)?, None),
        };
        let Some(region) = region else {
            continue;
        };
        if let Some(region) = clip_region(clip, region, device, work)? {
            total = Some(total.map_or(region, |previous| union(previous, region)));
        }
    }
    Ok(total)
}
/// Cull from the control hull before outlining strokes or rebuilding clips.
/// Padding contains caps, miters, hairlines and device stroke adjustment.
pub(super) fn path_visible(
    path: &PathCommand,
    tile: Tile,
    device: Transform,
    work: &mut usize,
    maximum: usize,
    prepared: &mut Prepared,
) -> Result<bool> {
    Ok(paint_region(
        path,
        tile,
        device,
        &mut Work {
            used: work,
            maximum,
            prepared,
        },
    )?
    .is_some())
}
fn paint_region(
    p: &PathCommand,
    tile: Tile,
    device: Transform,
    work: &mut Work<'_>,
) -> Result<Option<Tile>> {
    let pad = match &p.mark {
        Mark::Fill(_) => (0.0, 0.0),
        Mark::Stroke(s, t, adjust) => {
            let t = t.then(device);
            let r = s.width * 0.5;
            let extra = if *adjust { 0.5 } else { 0.0 };
            let factor = s.miter_limit.max(2.0);
            (
                (r * t.a.hypot(t.c) + extra).max(0.5) * factor,
                (r * t.b.hypot(t.d) + extra).max(0.5) * factor,
            )
        }
    };
    retained_region(&p.path, device, tile, pad, work)
}
fn clip_region(
    mut clip: Option<&Arc<Clip>>,
    mut tile: Tile,
    device: Transform,
    work: &mut Work<'_>,
) -> Result<Option<Tile>> {
    while let Some(node) = clip {
        let Some(next) = retained_region(&node.path, device, tile, (0.0, 0.0), work)? else {
            return Ok(None);
        };
        tile = next;
        clip = node.parent.as_ref();
    }
    Ok(Some(tile))
}
fn retained_region(
    path: &Arc<Path>,
    device: Transform,
    tile: Tile,
    pad: (f64, f64),
    work: &mut Work<'_>,
) -> Result<Option<Tile>> {
    work.charge(1)?;
    let key = Arc::as_ptr(path) as usize;
    let hull = if device == work.prepared.device {
        if let Some((_, hull)) = work.prepared.hulls.get(&key) {
            *hull
        } else {
            let hull = control_hull(path, device, work)?;
            if work.prepared.hulls.len() < work.prepared.maximum
                && work.prepared.hulls.try_reserve(1).is_ok()
            {
                work.prepared
                    .hulls
                    .insert(key, (Arc::downgrade(path), hull));
            }
            hull
        }
    } else {
        control_hull(path, device, work)?
    };
    Ok(hull_region(hull, tile, pad))
}
fn path_region(
    path: &Path,
    device: Transform,
    tile: Tile,
    pad: (f64, f64),
    work: &mut Work<'_>,
) -> Result<Option<Tile>> {
    Ok(hull_region(control_hull(path, device, work)?, tile, pad))
}
fn control_hull(path: &Path, device: Transform, work: &mut Work<'_>) -> Result<[Point; 2]> {
    work.charge(path.segments().len())?;
    let mut lo = Point::new(f64::INFINITY, f64::INFINITY);
    let mut hi = Point::new(f64::NEG_INFINITY, f64::NEG_INFINITY);
    let mut record = |p: Point| -> Result<()> {
        let p = p * device;
        if !p.is_finite() {
            return Err(Error::limit("group bounds overflow"));
        }
        lo.x = lo.x.min(p.x);
        lo.y = lo.y.min(p.y);
        hi.x = hi.x.max(p.x);
        hi.y = hi.y.max(p.y);
        Ok(())
    };
    for segment in path.segments() {
        match *segment {
            Segment::MoveTo(p) | Segment::LineTo(p) => record(p)?,
            Segment::CubicTo(a, b, c) => {
                record(a)?;
                record(b)?;
                record(c)?;
            }
            Segment::Close => {}
        }
    }
    Ok([lo, hi])
}
fn hull_region([lo, hi]: [Point; 2], tile: Tile, pad: (f64, f64)) -> Option<Tile> {
    let x0 = super::coordinate((lo.x - pad.0).floor(), tile.x, tile.x + tile.width);
    let y0 = super::coordinate((lo.y - pad.1).floor(), tile.y, tile.y + tile.height);
    // Include the floor cell at an integral maximum for degenerate paths (8.5.3.3.1).
    let x1 = super::coordinate((hi.x + pad.0).floor() + 1.0, tile.x, tile.x + tile.width);
    let y1 = super::coordinate((hi.y + pad.1).floor() + 1.0, tile.y, tile.y + tile.height);
    (x0 < x1 && y0 < y1).then_some(Tile {
        x: x0,
        y: y0,
        width: x1 - x0,
        height: y1 - y0,
    })
}
fn union(a: Tile, b: Tile) -> Tile {
    let x = a.x.min(b.x);
    let y = a.y.min(b.y);
    Tile {
        x,
        y,
        width: (a.x + a.width).max(b.x + b.width) - x,
        height: (a.y + a.height).max(b.y + b.height) - y,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pagura_raster::PathBuilder;

    #[test]
    fn bounds_do_not_retain_temporary_path_buffers() -> Result<()> {
        let mut path = PathBuilder::new();
        path.rect(0.0, 0.0, 8.0, 8.0);
        let path = Arc::new(path.finish());
        let identity = Arc::downgrade(&path);
        let mut prepared = Prepared::new(Transform::IDENTITY, 4096);
        let mut used = 0;
        let mut work = Work {
            used: &mut used,
            maximum: 1000,
            prepared: &mut prepared,
        };
        retained_region(
            &path,
            Transform::IDENTITY,
            Tile {
                x: 0,
                y: 0,
                width: 8,
                height: 8,
            },
            (0.0, 0.0),
            &mut work,
        )?;
        drop(path);
        assert!(identity.upgrade().is_none());
        Ok(())
    }

    #[test]
    fn prepared_hulls_preserve_padding_tiles_and_other_transforms() -> Result<()> {
        let mut path = PathBuilder::new();
        path.rect(2.25, 3.5, 4.0, 5.0);
        let path = Arc::new(path.finish());
        let device = Transform::scale(2.0, 3.0);
        let mut prepared = Prepared::new(device, 4096);
        let mut used = 0;
        let mut work = Work {
            used: &mut used,
            maximum: 1000,
            prepared: &mut prepared,
        };
        let tile = Tile {
            x: 0,
            y: 0,
            width: 32,
            height: 32,
        };
        let full = Tile {
            x: 4,
            y: 10,
            width: 9,
            height: 16,
        };
        assert_eq!(
            retained_region(&path, device, tile, (0.0, 0.0), &mut work)?,
            Some(full)
        );
        let inset = Tile {
            x: 5,
            y: 12,
            width: 3,
            height: 4,
        };
        assert_eq!(
            retained_region(&path, device, inset, (0.5, 1.0), &mut work)?,
            Some(inset)
        );
        assert_eq!(
            retained_region(&path, Transform::IDENTITY, tile, (0.0, 0.0), &mut work)?,
            Some(Tile {
                x: 2,
                y: 3,
                width: 5,
                height: 6
            })
        );
        // An unavailable cache has the same geometry and error validation.
        let mut uncached = Prepared::new(device, 0);
        let mut work = Work {
            used: &mut used,
            maximum: 1000,
            prepared: &mut uncached,
        };
        assert_eq!(
            retained_region(&path, device, tile, (0.0, 0.0), &mut work)?,
            Some(full)
        );
        Ok(())
    }
}
