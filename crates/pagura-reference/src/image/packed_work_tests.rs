//! Independent literal packed-row storage-visit and cumulative work controls.
use super::*;
use pagura_core::{ErrorKind, Limits, Meter, ObjectId, Recovery};
use pagura_document::{Document, OpenOptions};
use pagura_image::{ImageSource, Orientation};

fn complete_literal_tiles(width: u32, height: u32, orientation: Orientation) -> Rc<DecodedTiles> {
    let stride = (width as usize).div_ceil(8);
    let mut raw = vec![0xff_u8; stride * height as usize];
    for y in 0..height as usize {
        for x in 0..width as usize {
            if (x * 7 + y * 11) % 13 < 6 {
                raw[y * stride + x / 8] &= !(1 << (7 - x % 8));
            }
        }
    }
    let mut image = format!("<< /Type /XObject /Subtype /Image /Width {width} /Height {height} /ColorSpace /DeviceGray /BitsPerComponent 1 /Length {} >>\nstream\n", raw.len()).into_bytes();
    image.extend_from_slice(&raw);
    image.extend_from_slice(b"\nendstream");
    let objects: [&[u8]; 5] = [
        b"<< /Type /Catalog /Pages 2 0 R >>",
        b"<< /Type /Pages /Count 1 /Kids [3 0 R] >>",
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 1 1] /Contents 4 0 R >>",
        b"<< /Length 0 >>\nstream\n\nendstream",
        &image,
    ];
    let mut pdf = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, object) in objects.into_iter().enumerate() {
        offsets.push(pdf.len());
        pdf.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
        pdf.extend_from_slice(object);
        pdf.extend_from_slice(b"\nendobj\n");
    }
    let xref = pdf.len();
    pdf.extend_from_slice(b"xref\n0 6\n0000000000 65535 f \n");
    for offset in offsets {
        pdf.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    pdf.extend_from_slice(
        format!("trailer\n<< /Size 6 /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n").as_bytes(),
    );
    let limits = Limits {
        pixels: width as usize,
        ..Limits::default()
    };
    let doc = Document::open(
        pdf,
        &OpenOptions {
            limits,
            recovery: Recovery::Strict,
            ..OpenOptions::default()
        },
    )
    .unwrap();
    let source =
        ImageSource::from_xobject_with_tiling(&doc, doc.object(ObjectId::new(5, 0)).unwrap())
            .unwrap();
    Rc::new(source.decode_tiles(orientation, &limits).unwrap())
}

#[test]
fn packed_storage_visits_obey_both_independent_row_bounds() {
    for width in [1_u32, 63, 64, 65, 127, 128, 129] {
        for orientation in [Orientation::Rows, Orientation::Transposed] {
            if width == 1 && orientation == Orientation::Transposed {
                continue;
            } // No oversized tiled grid can admit one complete transposed row here.
            let tiles = complete_literal_tiles(width, 7, orientation);
            let (w, h) = (tiles.width(), tiles.height());
            for decode in [[0.0, 1.0], [1.0, 0.0]] {
                let values = Values::packed(Rc::clone(&tiles), decode).unwrap();
                for (low, high) in [
                    (Point::new(0.0, 0.0), Point::new(f64::from(w), f64::from(h))),
                    (Point::new(-0.5, -0.25), Point::new(0.75, 2.5)),
                    (
                        Point::new(0.125, 0.25),
                        Point::new(f64::from(w) - 0.125, f64::from(h) - 0.25),
                    ),
                    (Point::new(-2.0, -2.0), Point::new(-1.0, -1.0)),
                ] {
                    PACKED_VISITS.set(0);
                    let actual = values.rectangle_mean(low, high).unwrap();
                    let visits = PACKED_VISITS.get();
                    let (x0, y0) = (low.x.max(0.0), low.y.max(0.0));
                    let (x1, y1) = (high.x.min(f64::from(w)), high.y.min(f64::from(h)));
                    if x1 <= x0 || y1 <= y0 {
                        assert_eq!((actual, visits), (0.0, 0));
                        continue;
                    }
                    let c = clamp_index(x1.ceil() - 1.0, w) - clamp_index(x0.floor(), w) + 1;
                    let r = clamp_index(y1.ceil() - 1.0, h) - clamp_index(y0.floor(), h) + 1;
                    assert!(visits <= r * c, "logical visits {visits} vs {}", r * c);
                    assert!(visits <= r * (c.div_ceil(8) + 16), "packed visits {visits}");
                    let mut numerator = 0.0;
                    for y in 0..h {
                        for x in 0..w {
                            let (sx, sy) = if orientation == Orientation::Rows {
                                (x, y)
                            } else {
                                (y, x)
                            };
                            let bit = u8::from((sx as usize * 7 + sy as usize * 11) % 13 >= 6);
                            let alpha = if decode[0] == 0.0 {
                                f64::from(bit)
                            } else {
                                f64::from(1 - bit)
                            };
                            let area = (x1.min(f64::from(x) + 1.0) - x0.max(f64::from(x))).max(0.0)
                                * (y1.min(f64::from(y) + 1.0) - y0.max(f64::from(y))).max(0.0);
                            numerator += alpha * area;
                        }
                    }
                    let expected = numerator / ((x1 - x0) * (y1 - y0));
                    assert!(
                        (actual - expected).abs() < 1e-12,
                        "literal bits {width} {orientation:?} {decode:?}: {actual} vs {expected}"
                    );
                }
            }
        }
    }
}

#[test]
fn packed_work_is_cumulative_and_other_modes_keep_old_reservations() {
    let sampler = |t, magnify| Sampler {
        width: 129,
        height: 7,
        interpolate: magnify == Magnify::Bilinear,
        inverse: t,
    };
    let upright = sampler(
        Transform::new(65.0, 0.0, 0.0, 7.0, 0.0, 0.0),
        Magnify::Nearest,
    );
    let mut work = Meter::new(350, "independent packed work");
    upright
        .charge_packed_work([(0, 0, 1.0), (1, 0, 1.0)].into_iter(), &mut work)
        .unwrap();
    assert_eq!(work.used(), 350);
    let error = upright
        .charge_packed_work([(2, 0, 1.0)].into_iter(), &mut work)
        .unwrap_err();
    assert_eq!(error.kind, ErrorKind::Limit);
    assert_eq!(work.used(), 350);
    let quarter = sampler(
        Transform::new(0.0, 7.0, 65.0, 0.0, 0.0, 0.0),
        Magnify::Nearest,
    );
    let mut work = Meter::new(175, "quarter packed work");
    quarter
        .charge_packed_work([(0, 0, 1.0)].into_iter(), &mut work)
        .unwrap();
    assert_eq!(work.used(), 175);
    let affine = sampler(
        Transform::new(65.0, 0.25, 0.5, 7.0, 0.0, 0.0),
        Magnify::Nearest,
    );
    let mut work = Meter::new(476, "affine old cell work");
    affine
        .charge_packed_work([(0, 0, 1.0)].into_iter(), &mut work)
        .unwrap();
    assert_eq!(work.used(), 476);
    for (magnify, expected) in [(Magnify::Nearest, 1), (Magnify::Bilinear, 4)] {
        let nearest = sampler(Transform::scale(0.25, 0.25), magnify);
        let mut work = Meter::new(expected, "point work");
        nearest
            .charge_packed_work([(0, 0, 1.0)].into_iter(), &mut work)
            .unwrap();
        assert_eq!(work.used(), expected);
    }
    let overflow = sampler(
        Transform::new(f64::MAX / 4.0, 0.0, 0.0, 1.0, 0.0, 0.0),
        Magnify::Nearest,
    );
    let mut work = Meter::new(usize::MAX, "overflow");
    assert_eq!(
        overflow
            .charge_packed_work([(u32::MAX, 0, 1.0)].into_iter(), &mut work)
            .unwrap_err()
            .kind,
        ErrorKind::Limit
    );
}
