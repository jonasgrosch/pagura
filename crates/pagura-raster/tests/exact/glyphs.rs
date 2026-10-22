//! Glyph placements against the exact area of their outlines (quadratic
//! outlines against their true curves), for the outlines of the font
//! fixtures and the bundled fonts, alone and within clips.

use crate::support::{area, compare, options, pixels, raster, rectangle, reference};
use pagura_core::Limits;
use pagura_font::{FontFormat, FontProgram, GlyphId};
use pagura_raster::{ClipShape, FillRule, GlyphEdges, Path, PathBuilder, Point, Shape, Transform};
use pagura_reference::exact;
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
        .map(|d| d.filter_map(Result::ok).map(|e| e.path()).collect())
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
    for name in [
        "Tinos-Regular.ttf",
        "Arimo-BoldItalic.ttf",
        "Cousine-Regular.ttf",
    ] {
        let data = std::fs::read(root().join("data/fonts").join(name)).unwrap();
        out.push((
            name.to_string(),
            FontProgram::parse(data, FontFormat::TrueType, &limits).unwrap(),
        ));
    }
    let data = std::fs::read(root().join("data/fonts/FoxitSymbol.cff")).unwrap();
    out.push((
        "FoxitSymbol.cff".to_string(),
        FontProgram::parse(data, FontFormat::Cff, &limits).unwrap(),
    ));
    out
}

/// A glyph clip's differences from the exact coverage.
#[derive(Debug, Default)]
struct Tally {
    glyphs: usize,
    pixels: usize,
    differing: usize,
    largest: u8,
}

#[test]
fn glyphs_of_fixture_and_bundled_fonts_are_within_one_level() {
    let limits = Limits::default();
    let area = area(0, 0, 72, 64, 16);
    let fonts = fonts();
    assert!(fonts.len() > 40, "{}", fonts.len());
    let mut triangle = PathBuilder::new();
    triangle.move_to(Point::new(2.3, 1.7));
    triangle.line_to(Point::new(70.1, 9.4)).unwrap();
    triangle.line_to(Point::new(21.6, 62.2)).unwrap();
    triangle.close();
    let triangle = triangle.finish();
    let rect = rectangle(9.3, 7.6, 40.2, 31.7);
    let clips: [&[(&Path, FillRule)]; 3] = [
        &[],
        &[(&rect, FillRule::NonZero)],
        &[(&triangle, FillRule::NonZero)],
    ];
    let mut tallies: [Tally; 3] = Default::default();
    for (name, font) in &fonts {
        let ttf = std::path::Path::new(name)
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("ttf"));
        let count = font.glyph_count().min(if ttf { 160 } else { 40 });
        let matrix = font.font_matrix();
        for g in 0..count {
            let Ok(outline) = font.outline(GlyphId(g)) else {
                continue;
            };
            if outline.is_empty() {
                continue;
            }
            for (k, (size, skew)) in [(11.0, 0.0), (37.0, 0.0), (23.0, 0.3)]
                .into_iter()
                .enumerate()
            {
                let linear = matrix.then(Transform::new(size, 0.0, skew * size, -size, 0.0, 0.0));
                let linear = [linear.a, linear.b, linear.c, linear.d];
                let origin = Point::new(
                    7.0 + f64::from(g % 7) * 0.37 + f64::from(u8::try_from(k).unwrap()) * 3.1,
                    48.0 + f64::from(g % 5) * 0.29,
                );
                let edges = Arc::new(GlyphEdges::new(&outline, linear, &limits).unwrap());
                let mark = exact::glyph(&outline, linear, origin, &limits).unwrap();
                for (clips, tally) in clips.iter().zip(&mut tallies) {
                    let options = options(area, &limits);
                    let clip = (!clips.is_empty())
                        .then(|| ClipShape::new(clips, Transform::IDENTITY, &options).unwrap());
                    let shape = Shape::glyph_with_clip(&edges, origin, clip.as_ref(), &options)
                        .unwrap_or_else(|error| {
                            panic!(
                                "{name} glyph{g} size{size} skew{skew} clips{}: {error}",
                                clips.len()
                            )
                        });
                    let new = match &shape {
                        Some(shape) => {
                            let (coverage, work) = raster(&area, Some(shape), clip.as_ref());
                            assert!(
                                work <= shape.work(),
                                "{name} {g}: work {work} > {}",
                                shape.work()
                            );
                            coverage
                        }
                        None => vec![0; pixels(area.region)],
                    };
                    let (count, largest) =
                        compare(&new, &reference(area.region, mark.clone(), clips));
                    tally.glyphs += 1;
                    tally.pixels += new.len();
                    tally.differing += count;
                    tally.largest = tally.largest.max(largest);
                }
            }
        }
    }
    for (clip, tally) in ["none", "rectangle", "triangle"].iter().zip(&tallies) {
        eprintln!("glyphs, clip {clip}: {tally:?}");
        assert!(tally.largest <= 1, "{clip}: {tally:?}");
    }
}
