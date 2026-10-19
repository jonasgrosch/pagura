//! Conservative default-space bounds for implicit text/glyph groups.
use super::{Arc, Path, PathBuilder, Segment};
use crate::display::{Command, Mark};
use pagura_raster::Point;

pub(super) fn bounds(commands: &[Command]) -> Option<Arc<Path>> {
    let mut low = Point::new(f64::INFINITY, f64::INFINITY);
    let mut high = Point::new(f64::NEG_INFINITY, f64::NEG_INFINITY);
    let mut stack = vec![commands.iter()];
    while let Some(iter) = stack.last_mut() {
        let Some(command) = iter.next() else {
            stack.pop();
            continue;
        };
        match command {
            Command::Path(p) => {
                let pad = match &p.mark {
                    Mark::Fill(_) => (0.0, 0.0),
                    Mark::Stroke { pen: s, ctm: t, .. } if s.width > 0.0 => {
                        let r = s.width * s.miter_limit.max(2.0);
                        (r * t.a.hypot(t.c), r * t.b.hypot(t.d))
                    }
                    Mark::Stroke { .. } => return None,
                };
                for segment in p.path.segments() {
                    match *segment {
                        Segment::MoveTo(p) | Segment::LineTo(p) => {
                            include(p, pad, &mut low, &mut high);
                        }
                        Segment::CubicTo(a, b, c) => {
                            for p in [a, b, c] {
                                include(p, pad, &mut low, &mut high);
                            }
                        }
                        Segment::Close => {}
                    }
                }
            }
            Command::Image(i) => {
                for p in [
                    Point::new(0.0, 0.0),
                    Point::new(1.0, 0.0),
                    Point::new(1.0, 1.0),
                    Point::new(0.0, 1.0),
                ] {
                    include(p * i.transform, (0.0, 0.0), &mut low, &mut high);
                }
            }
            Command::Group(g) => stack.push(g.commands.iter()),
            // Canonical glyph extents must be combined with device origins
            // after scaling; a user-space envelope can lose those extents.
            Command::Glyphs(_) | Command::Shading(_) => return None,
        }
    }
    if !low.is_finite() || !high.is_finite() {
        return None;
    }
    let mut path = PathBuilder::new();
    path.rect(low.x, low.y, high.x - low.x, high.y - low.y);
    Some(Arc::new(path.finish()))
}
fn include(p: Point, pad: (f64, f64), low: &mut Point, high: &mut Point) {
    low.x = low.x.min(p.x - pad.0);
    low.y = low.y.min(p.y - pad.1);
    high.x = high.x.max(p.x + pad.0);
    high.y = high.y.max(p.y + pad.1);
}
