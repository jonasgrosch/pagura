//! Glyph placements of the band rasterizer against the old exact glyph
//! backends (`GlyphGeometry` placed by `Rasterizer::clipped_glyph`), for the
//! outlines of the font fixtures and the bundled fonts.

use pagura_core::{Limits, Meter};
use pagura_font::{FontFormat, FontProgram, GlyphId};
use pagura_raster::{
    Area, BandCoverage, BandRasterizer, ClipRegion, ClipShape, DeviceRect, FillRule,
    GlyphEdges, GlyphGeometry, GlyphOrigins, MarkCoverage, Path, PathBuilder, Point, Precision,
    Rasterizer, Shape, ShapeOptions, Span, Transform,
};
use std::sync::Arc;

fn root() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// The fixture and bundled font programs that parse, with their names.
fn fonts() -> Vec<(String, FontProgram)> {
    let mut out = Vec::new();
    let limits = Limits::default();
    let fixtures = root().join("corpus/generated/font");
    let mut entries: Vec<_> = std::fs::read_dir(&fixtures)
        .map(|d| d.filter_map(|e| e.ok()).map(|e| e.path()).collect())
        .unwrap_or_default();
    entries.sort();
    for path in entries {
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        let Some(stem) = name.strip_suffix(".font") else {
            continue;
        };
        let format = if stem.starts_with("truetype") {
            FontFormat::TrueType
        } else if stem.starts_with("opentype") {
            FontFormat::OpenType
        } else if stem.starts_with("cff") {
            FontFormat::Cff
        } else if stem.starts_with("type1") {
            FontFormat::Type1
        } else {
            continue;
        };
        let data = std::fs::read(&path).unwrap();
        if let Ok(font) = FontProgram::parse(data, format, &limits) {
            out.push((stem.to_string(), font));
        }
    }
    for name in ["Tinos-Regular.ttf", "Arimo-BoldItalic.ttf", "Cousine-Regular.ttf"] {
        let data = std::fs::read(root().join("data/fonts").join(name)).unwrap();
        out.push((name.to_string(), FontProgram::parse(data, FontFormat::TrueType, &limits).unwrap()));
    }
    let data = std::fs::read(root().join("data/fonts/FoxitSymbol.cff")).unwrap();
    out.push(("FoxitSymbol.cff".to_string(), FontProgram::parse(data, FontFormat::Cff, &limits).unwrap()));
    out
}

const AREA: Area = Area {
    region: DeviceRect {
        x0: 0,
        y0: 0,
        x1: 72,
        y1: 64,
    },
    band_height: 16,
};

fn raster(area: &Area, shape: Option<&Shape>, clip: Option<&ClipShape>) -> (Vec<u8>, usize) {
    let mut bytes = Meter::new(usize::MAX, "bytes");
    let mut raster = BandRasterizer::new(*area, &mut bytes).unwrap();
    let mut band =
        BandCoverage::with_capacity(pagura_raster::Capacity::band(area), &mut bytes).unwrap();
    let width = area.region.width() as usize;
    let mut out = vec![0; width * area.region.height() as usize];
    for b in area.bands() {
        raster.rasterize(MarkCoverage { shape, clip }, b, &mut band);
        for row in band.rows() {
            for span in row.spans() {
                let (x, values) = match span {
                    Span::Full { x, len } => (x, vec![255; len as usize]),
                    Span::Partial { x, coverage } => (x, coverage.to_vec()),
                };
                let start = (row.y - area.region.y0) as usize * width + (x - area.region.x0) as usize;
                out[start..start + values.len()].copy_from_slice(&values);
            }
        }
    }
    (out, raster.work_done())
}

fn old(area: &Area, geometry: &Arc<GlyphGeometry>, origin: Point, clip: Option<&Path>) -> Vec<u8> {
    let r = area.region;
    let (w, h) = (r.width(), r.height());
    let limits = Limits::default();
    let mut whole = PathBuilder::new();
    whole.rect(-1.0, -1.0, f64::from(w) + 2.0, f64::from(h) + 2.0);
    let whole = whole.finish();
    let region = ClipRegion::from_path(
        w,
        h,
        clip.unwrap_or(&whole),
        Transform::IDENTITY,
        FillRule::NonZero,
        limits,
        (r.x0, r.y0),
    )
    .unwrap();
    let mut out = vec![0; (w * h) as usize];
    if let Some(mask) = Rasterizer::default()
        .clipped_glyph(&region, Arc::clone(geometry), origin, limits)
        .unwrap()
    {
        let (x0, y0) = mask.origin();
        for (y, row) in mask.coverage().chunks(mask.width() as usize).enumerate() {
            let start = ((y0 - r.y0) as usize + y) * w as usize + (x0 - r.x0) as usize;
            out[start..start + row.len()].copy_from_slice(row);
        }
    }
    out
}

fn options<'a>(limits: &'a Limits, clip: Option<&'a ClipShape>) -> ShapeOptions<'a> {
    ShapeOptions {
        area: AREA,
        precision: Precision::Exact,
        limits,
        clip,
    }
}

#[derive(Debug, Default)]
struct Tally {
    glyphs: usize,
    pixels: usize,
    differing: usize,
    largest: u8,
    /// Differing pixels where the new or the old value is the correctly
    /// rounded area of the finely subdivided outline, or neither.
    new_rounds: usize,
    old_rounds: usize,
    neither: usize,
}

/// The outline under `linear` and placed at `origin`, each curve cut into
/// 4096 chords: within 1e-5 pixel of the true curve at these sizes.
fn fine(outline: &Path, linear: [f64; 4], origin: Point) -> Vec<Vec<(f64, f64)>> {
    let map = |p: Point| {
        (
            linear[0] * p.x + linear[2] * p.y + origin.x,
            linear[1] * p.x + linear[3] * p.y + origin.y,
        )
    };
    let mut contours = Vec::new();
    let mut current: Vec<(f64, f64)> = Vec::new();
    let mut last = Point::new(0.0, 0.0);
    for segment in outline.segments() {
        match *segment {
            pagura_raster::Segment::MoveTo(p) => {
                if current.len() > 1 {
                    contours.push(std::mem::take(&mut current));
                }
                current.clear();
                current.push(map(p));
                last = p;
            }
            pagura_raster::Segment::LineTo(p) => {
                current.push(map(p));
                last = p;
            }
            pagura_raster::Segment::CubicTo { c1, c2, end } => {
                for i in 1..=4096 {
                    let t = f64::from(i) / 4096.0;
                    let u = 1.0 - t;
                    let x = u * u * u * last.x + 3.0 * u * u * t * c1.x + 3.0 * u * t * t * c2.x + t * t * t * end.x;
                    let y = u * u * u * last.y + 3.0 * u * u * t * c1.y + 3.0 * u * t * t * c2.y + t * t * t * end.y;
                    current.push(map(Point::new(x, y)));
                }
                last = end;
            }
            pagura_raster::Segment::Close => {}
        }
    }
    if current.len() > 1 {
        contours.push(current);
    }
    contours
}

/// The signed area of `contours` within pixel `(x, y)`, clipped exactly in
/// pixel-relative coordinates (the winding integral; the covered area for
/// outlines whose contours do not overlap).
fn pixel_area(contours: &[Vec<(f64, f64)>], x: f64, y: f64) -> f64 {
    let mut total = 0.0;
    for contour in contours {
        let mut poly: Vec<(f64, f64)> = contour
            .iter()
            .map(|p| (p.0 - x, p.1 - y))
            .filter(|_| true)
            .collect();
        for (axis, low) in [(0, true), (0, false), (1, true), (1, false)] {
            let value = |p: (f64, f64)| if axis == 0 { p.0 } else { p.1 };
            let inside = |p: (f64, f64)| if low { value(p) >= 0.0 } else { value(p) <= 1.0 };
            let bound = if low { 0.0 } else { 1.0 };
            let mut out = Vec::with_capacity(poly.len());
            for i in 0..poly.len() {
                let a = poly[i];
                let b = poly[(i + 1) % poly.len()];
                if inside(a) {
                    out.push(a);
                }
                if inside(a) != inside(b) {
                    let t = (bound - value(a)) / (value(b) - value(a));
                    out.push((a.0 + t * (b.0 - a.0), a.1 + t * (b.1 - a.1)));
                }
            }
            poly = out;
            if poly.is_empty() {
                break;
            }
        }
        for i in 0..poly.len() {
            let a = poly[i];
            let b = poly[(i + 1) % poly.len()];
            total += (a.0 * b.1 - b.0 * a.1) * 0.5;
        }
    }
    total
}

#[test]
fn glyphs_of_fixture_and_bundled_fonts_match_the_old_backends() {
    let limits = Limits::default();
    let fonts = fonts();
    assert!(fonts.len() > 40, "{}", fonts.len());
    let mut triangle = PathBuilder::new();
    triangle.move_to(Point::new(2.3, 1.7));
    triangle.line_to(Point::new(70.1, 9.4)).unwrap();
    triangle.line_to(Point::new(21.6, 62.2)).unwrap();
    triangle.close();
    let triangle = triangle.finish();
    let mut rect = PathBuilder::new();
    rect.rect(9.3, 7.6, 40.2, 31.7);
    let rect = rect.finish();
    let clips: [Option<&Path>; 3] = [None, Some(&rect), Some(&triangle)];
    let mut tallies: [Tally; 3] = Default::default();
    for (name, font) in &fonts {
        let count = font.glyph_count().min(if name.ends_with(".ttf") { 160 } else { 40 });
        let matrix = font.font_matrix();
        for g in 0..count {
            let Ok(outline) = font.outline(GlyphId(g)) else {
                continue;
            };
            if outline.is_empty() {
                continue;
            }
            for (k, (size, skew)) in [(11.0, 0.0), (37.0, 0.0), (23.0, 0.3)].into_iter().enumerate() {
                let linear = matrix.then(Transform::new(size, 0.0, skew * size, -size, 0.0, 0.0));
                let linear = [linear.a, linear.b, linear.c, linear.d];
                let origin = Point::new(
                    7.0 + f64::from(g % 7) * 0.37 + k as f64 * 3.1,
                    48.0 + f64::from(g % 5) * 0.29,
                );
                let geometry = Arc::new(GlyphGeometry::new(&outline, linear, &limits).unwrap());
                let edges = Arc::new(GlyphEdges::new(&outline, linear, &limits).unwrap());
                for (c, clip_path) in clips.iter().enumerate() {
                    let clip = clip_path.map(|p| {
                        ClipShape::new(&[(p, FillRule::NonZero)], Transform::IDENTITY, &options(&limits, None))
                            .unwrap()
                    });
                    let opts = options(&limits, clip.as_ref());
                    let shape = Shape::glyph(&edges, origin, GlyphOrigins::Exact, &opts).unwrap();
                    let new = match &shape {
                        Some(shape) => {
                            let (coverage, work) = raster(&AREA, Some(shape), clip.as_ref());
                            assert!(work <= shape.work(), "{name} {g}: work {work} > {}", shape.work());
                            coverage
                        }
                        None => vec![0; 72 * 64],
                    };
                    let reference = old(&AREA, &geometry, origin, *clip_path);
                    let tally = &mut tallies[c];
                    tally.glyphs += 1;
                    tally.pixels += new.len();
                    let mut contours = None;
                    for (i, (a, b)) in new.iter().zip(&reference).enumerate() {
                        if a != b {
                            tally.differing += 1;
                            tally.largest = tally.largest.max(a.abs_diff(*b));
                            if c == 0 {
                                let contours = contours.get_or_insert_with(|| fine(&outline, linear, origin));
                                let area = pixel_area(contours, (i % 72) as f64, (i / 72) as f64).abs();
                                let level = (area.min(1.0) * 255.0 + 0.5) as u8;
                                if *a == level {
                                    tally.new_rounds += 1;
                                } else if *b == level {
                                    tally.old_rounds += 1;
                                } else {
                                    tally.neither += 1;
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    for (clip, tally) in ["none", "rectangle", "triangle"].iter().zip(&tallies) {
        eprintln!("glyphs, clip {clip}: {tally:?}");
        assert!(tally.largest <= 1, "{clip}: {tally:?}");
    }
}

