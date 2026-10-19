//! The row-fragment approximate path, selected by [`crate::Accuracy::RowFragments`].
//!
//! It renders opaque RGB pages of solid-colour marks, axis-aligned images and
//! simple groups straight into premultiplied RGBA8 with byte source-over:
//! - glyphs from prepared row fragments at 1/128-pixel origins
//!   ([`glyphs`]), and other paths by signed area after flattening curves
//!   to chords ([`coverage`]);
//! - axis-aligned images from four midpoint samples per pixel ([`images`]);
//! - groups of Normal composition at full opacity flattened onto the page,
//!   so knockout among their marks is not applied;
//! - marks are not clipped by fractional crop-box edges.
//!
//! Shadings and other images are painted by the exact compositor and placed
//! over the page. Pages with anything else take the exact path whole, as does
//! a page that exceeds this path's working budgets. None of this meets ADR
//! 0004's one-level contract; its accuracy is measured against the
//! competitive accuracy gate. Every value depends only on page device
//! coordinates, so any tile partition assembles into the page.

mod clip;
mod coverage;
mod glyphs;
mod images;
mod paint;

use crate::color::Color;
use crate::display::{Command, Mark, PathCommand};
use crate::transparency::Model;
use crate::{DisplayList, RenderOptions, Tile};
use clip::{ClipMask, clip_mask, rectangle};
pub(crate) use glyphs::GlyphCache;
use pagura_core::{Error, Limits, Result};
use pagura_raster::{
    FillRule, Pixmap, Point, Rgba8, Transform, adjusted_stroke_outline, stroke_outline,
};
use paint::Target;
use std::collections::HashMap;

/// Device-space chord tolerance for filled and stroked paths.
const TOLERANCE: f64 = 0.0625;

/// The glyph cache retained by a rendering context, in bytes.
pub(crate) const CACHE_BYTES: usize = 16 * 1024 * 1024;

/// Whether this path renders `list`; otherwise the exact path renders it.
///
/// # Errors
/// As [`DisplayList::preview_separations`].
pub(crate) fn admits(list: &DisplayList, options: &RenderOptions) -> Result<bool> {
    Ok(options.background.a == 255
        && !list.group.knockout
        && list.group.model.is_none_or(|m| m == Model::Rgb)
        && admit(&list.commands, 0, list.limits.depth)
        && !list.preview_separations(options)?.enabled)
}

fn admit(commands: &[Command], depth: usize, maximum: usize) -> bool {
    if depth >= maximum.min(64) {
        return false;
    }
    commands.iter().all(|c| match c {
        Command::Path(p) => solid(p),
        Command::Glyphs(run) => solid(&run.paint),
        Command::Image(i) => i.composite.simple() && !i.composite.overprint,
        Command::Shading(s) => s.composite.simple() && !s.composite.overprint,
        Command::Group(g) => {
            g.composite.simple()
                && g.attributes.view.is_none()
                && g.attributes.model.is_none_or(|m| m == Model::Rgb)
                && admit(&g.commands, depth + 1, maximum)
        }
    })
}

fn solid(p: &PathCommand) -> bool {
    p.composite.simple() && !p.composite.overprint && matches!(p.color, Color::Solid { .. })
}

/// Renders `tile` of an admitted `list`.
///
/// # Errors
/// Returns geometry, rendering and resource-limit errors.
pub(crate) fn render(
    list: &DisplayList,
    options: &RenderOptions,
    tile: Tile,
    glyphs: &mut GlyphCache,
) -> Result<Pixmap> {
    let page = list.device_size(options)?;
    let device = list.device_transform(options);
    let mut target = Target::new(tile, options.background, list.limits)?;
    let geometry = (device, page, list.limits);
    let mut clips = HashMap::new();
    let mut stack = vec![list.commands.iter()];
    while let Some(iter) = stack.last_mut() {
        let Some(command) = iter.next() else {
            stack.pop();
            continue;
        };
        match command {
            Command::Group(g) => stack.push(g.commands.iter()),
            Command::Path(p) => {
                let color = p
                    .color
                    .to_rgba(p.transfer.as_ref(), p.composite.conversion.intent)?;
                let clip = clip_mask(p.clip.as_ref(), geometry, &mut clips)?;
                path(p, geometry, &mut target, color, clip)?;
            }
            Command::Glyphs(run) => {
                let p = &run.paint;
                let color = p
                    .color
                    .to_rgba(p.transfer.as_ref(), p.composite.conversion.intent)?;
                let clip = clip_mask(p.clip.as_ref(), geometry, &mut clips)?;
                for &placement in &run.glyphs {
                    let source = run.source(placement)?;
                    let transform = Transform {
                        e: placement.origin.x,
                        f: placement.origin.y,
                        ..source.linear
                    }
                    .then(device);
                    let (coverage, offset) =
                        glyphs.coverage(&source.outline, transform, list.limits)?;
                    paint::blit(&mut target, &coverage, offset, color, clip);
                }
            }
            Command::Image(image) if images::eligible(image, device) => {
                let bounds = intersect(delegate_bounds(command, device, page), tile);
                if bounds.width != 0 && bounds.height != 0 {
                    let clip = clip_mask(image.clip.as_ref(), geometry, &mut clips)?;
                    images::paint(image, device, &mut target, bounds, clip, list.limits)?;
                }
            }
            Command::Image(_) | Command::Shading(_) => {
                delegate(list, command, options, &mut target, geometry)?;
            }
        }
    }
    Ok(target.finish())
}

/// Paints one mark with the exact compositor and places it over the page.
fn delegate(
    list: &DisplayList,
    command: &Command,
    options: &RenderOptions,
    target: &mut Target,
    (device, page, _): (Transform, (u32, u32), Limits),
) -> Result<()> {
    let bounds = intersect(delegate_bounds(command, device, page), target.tile());
    if bounds.width == 0 || bounds.height == 0 {
        return Ok(());
    }
    let single = match command {
        Command::Image(image) => Command::Image(image.clone()),
        Command::Shading(shading) => Command::Shading(shading.clone()),
        _ => {
            return Err(Error::invalid_argument(
                "only marks without coverage delegate",
            ));
        }
    };
    let options = RenderOptions {
        background: Rgba8::TRANSPARENT,
        ..*options
    };
    let pixels = crate::compositor::render(&list.single(single), &options, bounds)?;
    paint::over(target, &pixels, bounds);
    Ok(())
}

fn path(
    p: &PathCommand,
    (device, page, limits): (Transform, (u32, u32), Limits),
    target: &mut Target,
    color: Rgba8,
    clip: Option<&ClipMask>,
) -> Result<()> {
    if matches!(p.mark, Mark::Fill(_))
        && let Some(mut bounds) = rectangle(&p.path, device)
        && clip.is_none_or(|c| matches!(c, ClipMask::Rectangle(_)))
    {
        if let Some(ClipMask::Rectangle(c)) = clip {
            bounds = [
                bounds[0].max(c[0]),
                bounds[1].max(c[1]),
                bounds[2].min(c[2]),
                bounds[3].min(c[3]),
            ];
        }
        paint::fill_rectangle(target, bounds, page, color);
        return Ok(());
    }
    let coverage = match &p.mark {
        Mark::Fill(rule) => coverage::rasterize(&p.path, device, *rule, limits, TOLERANCE, page)?,
        Mark::Stroke(stroke, ctm, adjust) => {
            let inverse = ctm.inverse().ok_or(Error::unsupported(
                crate::features::SINGULAR_STROKE,
                "singular stroke transform is not implemented",
            ))?;
            let path = p.path.transformed(inverse);
            let outline = if *adjust {
                adjusted_stroke_outline
            } else {
                stroke_outline
            };
            let (path, transform) = outline(&path, ctm.then(device), stroke, &limits)?;
            coverage::rasterize(&path, transform, FillRule::NonZero, limits, TOLERANCE, page)?
        }
    };
    paint::blit(target, &coverage, (0, 0), color, clip);
    Ok(())
}

fn intersect(a: Tile, b: Tile) -> Tile {
    let x0 = a.x.max(b.x);
    let y0 = a.y.max(b.y);
    let x1 = (a.x + a.width).min(b.x + b.width);
    let y1 = (a.y + a.height).min(b.y + b.height);
    Tile {
        x: x0,
        y: y0,
        width: x1.saturating_sub(x0),
        height: y1.saturating_sub(y0),
    }
}

/// Device pixels an image may touch, or the page for any other mark.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "coordinates are clamped to page dimensions"
)]
fn delegate_bounds(command: &Command, device: Transform, (width, height): (u32, u32)) -> Tile {
    let Command::Image(i) = command else {
        return Tile {
            x: 0,
            y: 0,
            width,
            height,
        };
    };
    let m = i.transform.then(device);
    let corners = [
        Point::new(0.0, 0.0),
        Point::new(1.0, 0.0),
        Point::new(0.0, 1.0),
        Point::new(1.0, 1.0),
    ]
    .map(|p| p * m);
    let low = |f: fn(&Point) -> f64| corners.iter().map(f).fold(f64::INFINITY, f64::min);
    let high = |f: fn(&Point) -> f64| corners.iter().map(f).fold(0.0, f64::max);
    let x0 = low(|p| p.x).floor().clamp(0.0, f64::from(width)) as u32;
    let y0 = low(|p| p.y).floor().clamp(0.0, f64::from(height)) as u32;
    let x1 = high(|p| p.x).ceil().clamp(0.0, f64::from(width)) as u32;
    let y1 = high(|p| p.y).ceil().clamp(0.0, f64::from(height)) as u32;
    Tile {
        x: x0,
        y: y0,
        width: x1.saturating_sub(x0),
        height: y1.saturating_sub(y0),
    }
}

#[cfg(test)]
mod tests;
