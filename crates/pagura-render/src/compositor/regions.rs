//! Conservative group allocation bounds: source geometry, hard clips and tile.
//! Cubic control hulls contain the curve; stroke padding includes miters/caps.
use super::{Group, Tile};
use crate::display::{Clip, Command, Mark, PathCommand};
use pagura_core::{Error, Result};
use pagura_raster::{Path, Point, Segment, Transform};
use std::sync::Arc;

pub(super) fn group_region(
    group: &Group,
    tile: Tile,
    device: Transform,
    work: &mut usize,
    maximum: usize,
) -> Result<Option<Tile>> {
    bounded_group(
        group,
        tile,
        device,
        &mut Work {
            used: work,
            maximum,
        },
    )
}
struct Work<'a> {
    used: &'a mut usize,
    maximum: usize,
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
        let Some(bounds) = path_region(path, device, tile, (0.0, 0.0), work)? else {
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
) -> Result<bool> {
    Ok(paint_region(
        path,
        tile,
        device,
        &mut Work {
            used: work,
            maximum,
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
    path_region(&p.path, device, tile, pad, work)
}
fn clip_region(
    mut clip: Option<&Arc<Clip>>,
    mut tile: Tile,
    device: Transform,
    work: &mut Work<'_>,
) -> Result<Option<Tile>> {
    while let Some(node) = clip {
        let Some(next) = path_region(&node.path, device, tile, (0.0, 0.0), work)? else {
            return Ok(None);
        };
        tile = next;
        clip = node.parent.as_ref();
    }
    Ok(Some(tile))
}
fn path_region(
    path: &Path,
    device: Transform,
    tile: Tile,
    pad: (f64, f64),
    work: &mut Work<'_>,
) -> Result<Option<Tile>> {
    work.charge(path.segments().len())?;
    let mut lo = Point::new(f64::INFINITY, f64::INFINITY);
    let mut hi = Point::new(f64::NEG_INFINITY, f64::NEG_INFINITY);
    let mut record = |p: Point| -> Result<()> {
        let p = p * device;
        if !p.is_finite() {
            return Err(Error::limit("group bounds overflow"));
        }
        lo.x = lo.x.min(p.x - pad.0);
        lo.y = lo.y.min(p.y - pad.1);
        hi.x = hi.x.max(p.x + pad.0);
        hi.y = hi.y.max(p.y + pad.1);
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
    let x0 = super::coordinate(lo.x.floor(), tile.x, tile.x + tile.width);
    let y0 = super::coordinate(lo.y.floor(), tile.y, tile.y + tile.height);
    // Include the floor cell at an integral maximum for degenerate paths (8.5.3.3.1).
    let x1 = super::coordinate(hi.x.floor() + 1.0, tile.x, tile.x + tile.width);
    let y1 = super::coordinate(hi.y.floor() + 1.0, tile.y, tile.y + tile.height);
    Ok((x0 < x1 && y0 < y1).then_some(Tile {
        x: x0,
        y: y0,
        width: x1 - x0,
        height: y1 - y0,
    }))
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
