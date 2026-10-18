//! Opaque rectangle interiors erase earlier paint (ISO 32000-2, 11.3.3–11.3.5).
//! Subtract only whole device pixels: original fractional edges and hard clips
//! still pass through the rasterizer. Group boundaries end the search.

use crate::display::{Command, Mark, PathCommand};
use crate::{Tile, color::Color};
use pagura_core::{Limits, Rect};
use pagura_raster::{Path, PathBuilder, Point, Segment, Transform};
use std::collections::HashMap;

const MAX_OCCLUDERS: usize = 32;
const MAX_PIECES: usize = 16;
const MAX_ENTRIES: usize = 256;

#[derive(Default)]
pub(crate) struct Plan {
    regions: HashMap<*const PathCommand, Vec<Rect>>,
}

impl Plan {
    pub(crate) fn for_list(
        list: &crate::display::DisplayList,
        options: &crate::RenderOptions,
        tile: Tile,
    ) -> Option<Self> {
        if list.group.knockout
            || list
                .group
                .model
                .is_some_and(|m| m != crate::transparency::Model::Rgb)
        {
            return None;
        }
        let plan = Self::new(
            &list.commands,
            list.device_transform(options),
            tile,
            list.crop,
            list.limits,
        );
        (!plan.regions.is_empty()).then_some(plan)
    }

    pub(crate) fn new(
        commands: &[Command],
        device: Transform,
        tile: Tile,
        crop: Rect,
        limits: Limits,
    ) -> Self {
        if commands.len() < 2 || limits.path_segments < 5 {
            return Self::default();
        }
        let Some(crop) = rectangle(&rect_path(crop), device) else {
            return Self::default();
        };
        let viewport = Rect {
            x0: f64::from(tile.x),
            y0: f64::from(tile.y),
            x1: f64::from(tile.x) + f64::from(tile.width),
            y1: f64::from(tile.y) + f64::from(tile.height),
        };
        let Some(crop) = crop.intersection(&viewport) else {
            return Self::default();
        };
        let mut plan = Self::default();
        let mut occluders = Vec::new();
        if occluders.try_reserve_exact(MAX_OCCLUDERS).is_err() {
            return plan;
        }
        // Optional bookkeeping never consumes the renderer's error budget.
        // Falling back also bounds both traversal and rectangle fragmentation.
        let mut work = limits.raster_work / 16;
        let maximum =
            MAX_ENTRIES.min(limits.decoded_bytes / (MAX_PIECES * size_of::<Rect>() + 128) / 64);
        for command in commands.iter().rev() {
            if work == 0 || plan.regions.len() >= maximum {
                break;
            }
            work -= 1;
            let p = match command {
                Command::Path(p) => p,
                Command::Group(_) => {
                    occluders.clear();
                    continue;
                }
                Command::Image(_) | Command::Shading(_) => continue,
            };
            if !eligible(p) {
                continue;
            }
            let Some(source) = rectangle(&p.path, device).and_then(|r| r.intersection(&viewport))
            else {
                continue;
            };
            if !occluders.is_empty()
                && let Some(pieces) = subtract_all(source, &occluders, &mut work)
            {
                let remaining: f64 = pieces.iter().map(area).sum();
                let saved = area(&source) - remaining;
                // Small marks cannot repay interval bookkeeping.
                if saved >= 1024.0 && saved >= area(&source) * 0.25 {
                    if plan.regions.try_reserve(1).is_err() {
                        break;
                    }
                    plan.regions.insert(p, pieces);
                }
            }
            if occluders.len() < MAX_OCCLUDERS
                && exact_color(&p.color)
                && let Some(interior) = interior(p, source, crop, device, &mut work)
            {
                occluders.push(interior);
            }
        }
        plan
    }

    pub(crate) fn regions(&self, command: &PathCommand) -> Option<&[Rect]> {
        // Most marks are glyph outlines; avoid hashing them altogether.
        (!self.regions.is_empty() && command.path.segments().len() <= 6)
            .then(|| self.regions.get(&std::ptr::from_ref(command)))
            .flatten()
            .map(Vec::as_slice)
    }
}

/// Preserve each surviving pixel's original coverage, with no mask rewriting.
pub(crate) fn row<'a>(
    coverage: &'a [u8],
    origin: (u32, u32),
    regions: &'a [Rect],
) -> impl Iterator<Item = (usize, u8)> + 'a {
    regions
        .iter()
        .filter_map(move |r| {
            let y = f64::from(origin.1);
            if y < r.y0.floor() || y >= r.y1.ceil() {
                return None;
            }
            let start = column(r.x0.floor(), origin.0, coverage.len());
            let end = column(r.x1.ceil(), origin.0, coverage.len());
            coverage.get(start..end).map(|slice| (start, slice))
        })
        .flat_map(|(start, slice)| {
            slice
                .iter()
                .copied()
                .enumerate()
                .map(move |(column, value)| (start + column, value))
        })
        .filter(|(_, value)| *value != 0)
}

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "column is clamped to the validated coverage slice length"
)]
fn column(value: f64, origin: u32, maximum: usize) -> usize {
    (value - f64::from(origin)).clamp(0.0, maximum as f64) as usize
}

fn eligible(p: &PathCommand) -> bool {
    matches!(p.mark, Mark::Fill(_))
        && matches!(p.color, Color::Solid { .. })
        && p.color.paints()
        && p.composite.simple()
        && !p.composite.overprint
        && p.transfer.is_none()
}

fn exact_color(color: &Color) -> bool {
    // The floating compositor evaluates (1 - backdrop_alpha) * source +
    // backdrop_alpha * source even for an opaque Normal store. Endpoints
    // preserve its exact rounding when the earlier backdrop is omitted.
    matches!(color, Color::Solid { space: crate::color::Space::Gray | crate::color::Space::Rgb | crate::color::Space::Cmyk, components }
        if components.iter().all(|&v| v == 0.0 || v == 1.0))
}

fn interior(
    p: &PathCommand,
    source: Rect,
    crop: Rect,
    device: Transform,
    work: &mut usize,
) -> Option<Rect> {
    let mut region = source.intersection(&crop)?;
    let mut clip = p.clip.as_deref();
    while let Some(node) = clip {
        *work = work.checked_sub(1)?;
        region = region.intersection(&rectangle(&node.path, device)?)?;
        clip = node.parent.as_deref();
    }
    region.x0 = region.x0.ceil();
    region.y0 = region.y0.ceil();
    region.x1 = region.x1.floor();
    region.y1 = region.y1.floor();
    (!region.is_empty()).then_some(region)
}

fn rectangle(path: &Path, device: Transform) -> Option<Rect> {
    let (Segment::MoveTo(a), rest) = path.segments().split_first()? else {
        return None;
    };
    let [
        Segment::LineTo(b),
        Segment::LineTo(c),
        Segment::LineTo(d),
        tail @ ..,
    ] = rest
    else {
        return None;
    };
    match tail {
        [] | [Segment::Close] => {}
        [Segment::LineTo(end)] | [Segment::LineTo(end), Segment::Close] if end == a => {}
        _ => return None,
    }
    let [a, b, c, d] = [a, b, c, d].map(|&p| p * device);
    let axis_aligned = (a.x == b.x && b.y == c.y && c.x == d.x && d.y == a.y)
        || (a.y == b.y && b.x == c.x && c.y == d.y && d.x == a.x);
    let rect = Rect::from_corners(a.x, a.y, c.x, c.y)?;
    (axis_aligned && !rect.is_empty()).then_some(rect)
}

fn area(rect: &Rect) -> f64 {
    rect.width() * rect.height()
}

fn subtract_all(source: Rect, occluders: &[Rect], work: &mut usize) -> Option<Vec<Rect>> {
    let mut pieces = Vec::new();
    pieces.try_reserve_exact(MAX_PIECES).ok()?;
    pieces.push(source);
    for occluder in occluders {
        // Appended pieces are already outside this occluder.
        for index in (0..pieces.len()).rev() {
            *work = work.checked_sub(1)?;
            let Some(overlap) = pieces[index].intersection(occluder) else {
                continue;
            };
            let r = pieces.swap_remove(index);
            let candidates = [
                Rect {
                    y1: overlap.y0,
                    ..r
                },
                Rect {
                    y0: overlap.y1,
                    ..r
                },
                Rect {
                    x1: overlap.x0,
                    y0: overlap.y0,
                    y1: overlap.y1,
                    ..r
                },
                Rect {
                    x0: overlap.x1,
                    y0: overlap.y0,
                    y1: overlap.y1,
                    ..r
                },
            ];
            for piece in candidates.into_iter().filter(|r| !r.is_empty()) {
                if pieces.len() == MAX_PIECES {
                    return None;
                }
                pieces.push(piece);
            }
        }
    }
    Some(pieces)
}

fn rect_path(rect: Rect) -> Path {
    let mut path = PathBuilder::new();
    path.move_to(Point::new(rect.x0, rect.y0));
    let _ = path.line_to(Point::new(rect.x1, rect.y0));
    let _ = path.line_to(Point::new(rect.x1, rect.y1));
    let _ = path.line_to(Point::new(rect.x0, rect.y1));
    path.close();
    path.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        color::Space,
        display::Clip,
        transparency::{Attributes, Composite, Group},
    };
    use pagura_core::Position;
    use pagura_raster::FillRule;
    use std::sync::Arc;

    const TILE: Tile = Tile {
        x: 0,
        y: 0,
        width: 64,
        height: 64,
    };
    const PAGE: Rect = Rect {
        x0: 0.0,
        y0: 0.0,
        x1: 64.0,
        y1: 64.0,
    };

    fn mark(rect: Rect, components: Vec<f64>) -> Command {
        Command::Path(PathCommand {
            path: Arc::new(rect_path(rect)),
            mark: Mark::Fill(FillRule::NonZero),
            clip: None,
            color: Color::Solid {
                space: Space::Rgb,
                components,
            },
            position: Position::Unknown,
            transfer: None,
            composite: Composite::default(),
        })
    }

    fn path(command: &Command) -> &PathCommand {
        let Command::Path(p) = command else {
            unreachable!()
        };
        p
    }

    #[test]
    fn fractional_occluder_keeps_original_coverage_outside_whole_pixels() {
        let foreground = Rect {
            x0: 4.25,
            y0: 6.5,
            x1: 60.75,
            y1: 61.125,
        };
        let commands = [
            mark(PAGE, vec![1.0, 0.0, 0.0]),
            mark(foreground, vec![0.0, 0.0, 1.0]),
        ];
        let plan = Plan::new(
            &commands,
            Transform::IDENTITY,
            TILE,
            PAGE,
            Limits::default(),
        );
        let pieces = plan.regions(path(&commands[0])).unwrap();
        for y in 0..64 {
            let actual: Vec<_> = row(&[137; 64], (0, y), pieces).collect();
            let mut columns: Vec<_> = actual
                .iter()
                .map(|&(x, coverage)| {
                    assert_eq!(coverage, 137);
                    x
                })
                .collect();
            columns.sort_unstable();
            let expected: Vec<_> = (0..64)
                .filter(|x| !((5..60).contains(x) && (7..61).contains(&y)))
                .collect();
            assert_eq!(columns, expected);
        }
    }

    #[test]
    fn hard_clips_fractional_crops_and_group_boundaries_are_conservative() {
        let mut commands = [mark(PAGE, vec![1.0; 3]), mark(PAGE, vec![0.0; 3])];
        let Command::Path(p) = &mut commands[1] else {
            unreachable!()
        };
        p.clip = Some(Arc::new(Clip {
            parent: None,
            path: Arc::new(rect_path(Rect {
                x0: 8.25,
                y0: 8.25,
                x1: 56.75,
                y1: 56.75,
            })),
            rule: FillRule::EvenOdd,
        }));
        let plan = Plan::new(
            &commands,
            Transform::IDENTITY,
            TILE,
            PAGE,
            Limits::default(),
        );
        let pieces = plan.regions(path(&commands[0])).unwrap();
        assert_eq!(pieces.iter().map(area).sum::<f64>(), 4096.0 - 47.0 * 47.0);

        let group = Command::Group(Group {
            position: Position::Unknown,
            bounds: None,
            commands: vec![],
            attributes: Attributes::default(),
            composite: Composite::default(),
        });
        let commands = [mark(PAGE, vec![1.0; 3]), group, mark(PAGE, vec![0.0; 3])];
        let plan = Plan::new(
            &commands,
            Transform::IDENTITY,
            TILE,
            PAGE,
            Limits::default(),
        );
        assert!(plan.regions(path(&commands[0])).is_none());

        let commands = [mark(PAGE, vec![1.0; 3]), mark(PAGE, vec![0.0; 3])];
        let crop = Rect { x1: 63.5, ..PAGE };
        let plan = Plan::new(
            &commands,
            Transform::IDENTITY,
            TILE,
            crop,
            Limits::default(),
        );
        assert_eq!(
            plan.regions(path(&commands[0]))
                .unwrap()
                .iter()
                .map(area)
                .sum::<f64>(),
            64.0
        );
    }

    #[test]
    fn opacity_overprint_and_exhausted_budgets_disable_occlusion() {
        for (alpha, overprint) in [(0.5, false), (1.0, true)] {
            let mut commands = [mark(PAGE, vec![1.0; 3]), mark(PAGE, vec![0.0; 3])];
            let Command::Path(p) = &mut commands[1] else {
                unreachable!()
            };
            p.composite.alpha = alpha;
            p.composite.overprint = overprint;
            let plan = Plan::new(
                &commands,
                Transform::IDENTITY,
                TILE,
                PAGE,
                Limits::default(),
            );
            assert!(plan.regions(path(&commands[0])).is_none());
        }
        let commands = [mark(PAGE, vec![1.0; 3]), mark(PAGE, vec![0.0; 3])];
        let plan = Plan::new(
            &commands,
            Transform::IDENTITY,
            TILE,
            PAGE,
            Limits {
                raster_work: 0,
                ..Limits::default()
            },
        );
        assert!(plan.regions(path(&commands[0])).is_none());
        let plan = Plan::new(
            &commands,
            Transform::IDENTITY,
            TILE,
            PAGE,
            Limits::default(),
        );
        assert_eq!(plan.regions(path(&commands[0])), Some([].as_slice()));
        let commands = [mark(PAGE, vec![1.0; 3]), mark(PAGE, vec![0.5; 3])];
        let plan = Plan::new(
            &commands,
            Transform::IDENTITY,
            TILE,
            PAGE,
            Limits::default(),
        );
        assert!(plan.regions(path(&commands[0])).is_none());
    }
}
