//! Geometric line/column policy; marked occurrences stay in content order.
use super::{Atom, LayoutOptions, Line, Point, PositionedGlyph, PositionedText, Replacement};
use std::collections::BTreeMap;
struct Block {
    lines: Vec<Line>,
    geometry: Option<usize>,
}
pub(super) fn direction(glyph: &PositionedGlyph) -> Point {
    let length = glyph.axis.x.hypot(glyph.axis.y);
    if length > 0.0 {
        Point::new(glyph.axis.x / length, glyph.axis.y / length)
    } else {
        Point::new(1.0, 0.0)
    }
}
pub(super) fn coordinates(origin: Point, axis: Point) -> (f64, f64) {
    (
        origin.x * axis.x + origin.y * axis.y,
        -origin.x * axis.y + origin.y * axis.x,
    )
}
pub(super) fn same_line(a: &PositionedGlyph, b: &PositionedGlyph, options: LayoutOptions) -> bool {
    let axis = direction(a);
    let other = direction(b);
    let (_, av) = coordinates(a.origin, axis);
    let (_, bv) = coordinates(b.origin, axis);
    axis.x * other.x + axis.y * other.y > 0.98
        && (av - bv).abs() <= options.baseline_tolerance * a.em.max(b.em).max(1e-6)
}
pub(super) fn gap(a: &PositionedGlyph, b: &PositionedGlyph) -> f64 {
    let axis = direction(a);
    let delta = Point::new(b.origin.x - a.advance_end.x, b.origin.y - a.advance_end.y);
    delta.x * axis.x + delta.y * axis.y
}
pub(super) fn content_lines(
    source: &PositionedText,
    atoms: &[Atom],
    options: LayoutOptions,
) -> Vec<Line> {
    let mut lines: Vec<Line> = Vec::new();
    for (index, atom) in atoms.iter().enumerate() {
        let continuation = atom.replacement == Some(Replacement::Actual)
            && lines
                .last()
                .and_then(|l| l.atoms.last())
                .is_some_and(|i| atoms[*i].replacement == Some(Replacement::Actual));
        let marked = if continuation {
            lines.last().and_then(|l| l.marked)
        } else {
            atom.marked
        };
        let same = lines.last().is_some_and(|line| {
            match (
                line.atoms.last().and_then(|i| atoms[*i].geometry),
                atom.geometry,
            ) {
                (Some(a), Some(b)) => {
                    same_line(&source.glyphs[a], &source.glyphs[b], options)
                        && gap(&source.glyphs[a], &source.glyphs[b]).abs()
                            <= 2.5 * source.glyphs[a].em.max(source.glyphs[b].em).max(1e-6)
                        && line.marked == marked
                }
                _ => line.marked == marked,
            }
        });
        if same {
            if let Some(line) = lines.last_mut() {
                line.atoms.push(index);
            }
        } else {
            lines.push(Line {
                atoms: vec![index],
                geometry: atom.geometry,
                marked,
            });
        }
    }
    lines
}

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "normalized finite angle is rounded to a degree in 0..=360"
)]
fn orientation(glyph: &PositionedGlyph) -> u16 {
    glyph
        .axis
        .y
        .atan2(glyph.axis.x)
        .to_degrees()
        .rem_euclid(360.0)
        .round() as u16
        % 360
}
pub(super) fn order(source: &PositionedText, lines: Vec<Line>) -> Vec<Line> {
    // Keep an entire marked occurrence together, even across orientations.
    let mut blocks: Vec<Block> = Vec::new();
    let mut marked = BTreeMap::new();
    for line in lines {
        if let Some(id) = line.marked {
            let index = *marked.entry(id).or_insert_with(|| {
                blocks.push(Block {
                    geometry: line.geometry,
                    lines: Vec::new(),
                });
                blocks.len() - 1
            });
            blocks[index].geometry = blocks[index].geometry.or(line.geometry);
            blocks[index].lines.push(line);
        } else {
            blocks.push(Block {
                geometry: line.geometry,
                lines: vec![line],
            });
        }
    }
    // A replacement without glyph geometry anchors content, rather than
    // being assigned a fabricated position for geometric sorting.
    let mut result = Vec::new();
    let mut positioned = Vec::new();
    for block in blocks {
        if block.geometry.is_none() {
            result.extend(order_positioned(source, std::mem::take(&mut positioned)));
            result.extend(block.lines);
        } else {
            positioned.push(block);
        }
    }
    result.extend(order_positioned(source, positioned));
    result
}
fn order_positioned(source: &PositionedText, blocks: Vec<Block>) -> Vec<Line> {
    let mut groups: Vec<Vec<Block>> = Vec::new();
    let mut indices = BTreeMap::new();
    for block in blocks {
        let key = block.geometry.map_or(0, |i| orientation(&source.glyphs[i]));
        let group = *indices.entry(key).or_insert_with(|| {
            groups.push(Vec::new());
            groups.len() - 1
        });
        groups[group].push(block);
    }
    let mut result = Vec::new();
    for mut blocks in groups {
        let axis = blocks
            .iter()
            .find_map(|b| b.geometry)
            .map_or(Point::new(1.0, 0.0), |i| direction(&source.glyphs[i]));
        let position = |b: &Block| {
            b.geometry
                .map_or((0.0, 0.0), |i| coordinates(source.glyphs[i].origin, axis))
        };
        blocks.sort_by(|a, b| position(a).0.total_cmp(&position(b).0));
        let mut start = 0;
        while start < blocks.len() {
            let mut end = start + 1;
            while end < blocks.len() {
                let a = &blocks[end - 1];
                let b = &blocks[end];
                let em = a
                    .geometry
                    .into_iter()
                    .chain(b.geometry)
                    .map(|i| source.glyphs[i].em)
                    .fold(1e-6, f64::max);
                if position(b).0 - position(a).0 > 1.5 * em {
                    break;
                }
                end += 1;
            }
            blocks[start..end].sort_by(|a, b| position(b).1.total_cmp(&position(a).1));
            start = end;
        }
        for block in blocks {
            result.extend(block.lines);
        }
    }
    result
}
