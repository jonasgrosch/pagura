//! The sampler against an independent high-precision oracle on the images
//! of the generated fixtures and the benchmark pages (ADR 0004 section 3).
//! Run in release:
//!
//! ```text
//! cargo test --release -p pagura-paint --lib image_accuracy -- --ignored --nocapture
//! ```
//!
//! For every device pixel an image covers, the oracle is the exact-area
//! average of the decoded sample values under the pixel's footprint (or the
//! nearest sample, or bilinear interpolation of the samples around its
//! centre when the image is magnified), evaluated in double-double
//! arithmetic from the footprint's f64 bounds. Each colour channel is
//! quantized as the output quantizer does, for the old route's sampler (the
//! port in `oracle`), for the new sampler, and correctly rounded from the
//! oracle. The test reports the channels whose quantized value changed,
//! every changed one's oracle value, the largest distance of a new value
//! from its oracle in levels, and the exact-rounding match rates, and it
//! fails when a new value leaves one level of its oracle.
//!
//! `PAGURA_IMAGE_PAGES` (as for the timing) adds or replaces the benchmark
//! pages; `PAGURA_IMAGE_FIXTURES=0` leaves out the generated fixtures.

use super::oracle::Oracle;
use super::timing::{IMAGE_PAGES, Placed, decode_once, pages, placed, sampler, setting};
use super::{Case, case};
use crate::SpanColors;
use pagura_color::ColorSpace;
use pagura_document::{Document, OpenOptions};
use pagura_image::{DecodeRequest, Layout, Reduce, SourceMask};
use pagura_raster::{Point, Transform, quantize_unit};
use std::fmt::Write as _;
use std::sync::Arc;

/// A double-double number: `high + low` with `|low| ≤ ulp(high) / 2`.
#[derive(Clone, Copy, Debug, Default)]
struct Wide {
    high: f64,
    low: f64,
}

/// The exact sum of two doubles as a double-double.
fn two_sum(a: f64, b: f64) -> Wide {
    let high = a + b;
    let v = high - a;
    let low = (a - (high - v)) + (b - v);
    Wide { high, low }
}

/// Dekker's split of a double into two halves of 26 bits.
fn split(a: f64) -> (f64, f64) {
    let c = 134_217_729.0 * a;
    let high = c - (c - a);
    (high, a - high)
}

/// The exact product of two doubles as a double-double.
fn two_product(a: f64, b: f64) -> Wide {
    let high = a * b;
    let (ah, al) = split(a);
    let (bh, bl) = split(b);
    let low = ((ah * bh - high) + ah * bl + al * bh) + al * bl;
    Wide { high, low }
}

impl Wide {
    fn new(value: f64) -> Self {
        Self {
            high: value,
            low: 0.0,
        }
    }

    fn add(self, other: Self) -> Self {
        let s = two_sum(self.high, other.high);
        let low = s.low + self.low + other.low;
        let high = s.high + low;
        Self {
            high,
            low: low - (high - s.high),
        }
    }

    fn mul(self, other: Self) -> Self {
        let p = two_product(self.high, other.high);
        let low = p.low + (self.high * other.low + self.low * other.high);
        let high = p.high + low;
        Self {
            high,
            low: low - (high - p.high),
        }
    }

    fn div(self, other: Self) -> Self {
        let q = self.high / other.high;
        let r = self.add(other.mul(Self::new(-q)));
        let q2 = r.high / other.high;
        Self::new(q).add(Self::new(q2))
    }

    fn value(self) -> f64 {
        self.high + self.low
    }
}

/// The cells a device pixel's footprint covers along one axis, with their
/// exact weights (as the old route computes the bounds).
fn axis(pixel: u32, scale: f64, offset: f64, maximum: u32) -> Vec<(u32, f64)> {
    let a = f64::from(pixel) * scale + offset;
    let b = (f64::from(pixel) + 1.0) * scale + offset;
    let low = a.min(b).clamp(0.0, f64::from(maximum));
    let high = a.max(b).clamp(0.0, f64::from(maximum));
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "clamped to the image"
    )]
    let (first, end) = (low.floor() as u32, high.ceil() as u32);
    (first..end.max(first))
        .map(|cell| {
            let weight = high.min(f64::from(cell) + 1.0) - low.max(f64::from(cell));
            (cell.min(maximum.saturating_sub(1)), weight.max(0.0))
        })
        .filter(|&(_, weight)| weight > 0.0)
        .collect()
}

/// The two cells around a pixel centre along one axis with their bilinear
/// weights.
fn taps(center: f64, maximum: u32) -> [(u32, f64); 2] {
    let position = center - 0.5;
    let left = position.floor();
    let fraction = position - left;
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "clamped to the image"
    )]
    let clamp = |v: f64| v.clamp(0.0, f64::from(maximum.saturating_sub(1))) as u32;
    [(clamp(left), 1.0 - fraction), (clamp(left + 1.0), fraction)]
}

/// The oracle of one image: its samples, decode intervals and placement.
struct Exact<'a> {
    case: &'a Case,
    inverse: Transform,
    minifying: bool,
}

impl Exact<'_> {
    fn new(case: &Case, unit_to_device: Transform) -> Exact<'_> {
        let (width, height) = case.image.extent();
        let inverse = unit_to_device
            .inverse()
            .unwrap()
            .then(Transform::new(width, 0.0, 0.0, -height, 0.0, height));
        let t = inverse;
        Exact {
            case,
            inverse,
            minifying: t.a.hypot(t.b) > 1.0 + 1e-9 || t.c.hypot(t.d) > 1.0 + 1e-9,
        }
    }

    /// The texel of component `k` of sample (`x`, `y`): its decoded value
    /// `low + code / maximum × (high − low)` as an exact rational of its
    /// code, clipped to the colour range as device colour converts, and its
    /// alpha, 0 where a colour key masks the sample (8.9.6.4).
    fn value(&self, (x, y): (u32, u32), k: usize) -> (Wide, f64) {
        let image = &self.case.image;
        let n = usize::from(image.components);
        let pixel = usize::try_from(y).unwrap() * usize::try_from(image.width).unwrap()
            + usize::try_from(x).unwrap();
        let code = |k: usize| image.samples.get(pixel * n + k).unwrap();
        let maximum = f64::from(u16::MAX >> (16 - image.component_bits(k).unwrap()));
        let [low, high] = self.case.decode[k];
        let value = Wide::new(low).add(
            Wide::new(f64::from(code(k)))
                .div(Wide::new(maximum))
                .mul(Wide::new(high).add(Wide::new(-low))),
        );
        let value = if value.value() < 0.0 {
            Wide::default()
        } else if value.value() > 1.0 {
            Wide::new(1.0)
        } else {
            value
        };
        let keyed = self.case.key.as_ref().is_some_and(|ranges| {
            ranges.len() == n
                && ranges
                    .iter()
                    .enumerate()
                    .all(|(k, &[low, high])| (low..=high).contains(&code(k)))
        });
        (value, if keyed { 0.0 } else { 1.0 })
    }

    /// Component `k` of device pixel (`x`, `y`), or `None` where the image
    /// is not separable, upright or turned by a quarter.
    fn pixel(&self, (x, y): (u32, u32), k: usize) -> Option<Wide> {
        let t = self.inverse;
        let image = &self.case.image;
        let (width, height) = (image.width, image.height);
        let upright = t.b == 0.0 && t.c == 0.0;
        let swapped = t.a == 0.0 && t.d == 0.0;
        if !upright && !swapped {
            return None;
        }
        // Per device axis: the source axis it steps and the transform terms.
        let (columns, rows) = if self.minifying {
            let (along_x, along_y) = if swapped {
                (axis(x, t.b, t.f, height), axis(y, t.c, t.e, width))
            } else {
                (axis(x, t.a, t.e, width), axis(y, t.d, t.f, height))
            };
            (along_x, along_y)
        } else {
            let center = Point::new(f64::from(x) + 0.5, f64::from(y) + 0.5) * t;
            if image.interpolate {
                (
                    taps(center.x, width).to_vec(),
                    taps(center.y, height).to_vec(),
                )
            } else {
                #[expect(
                    clippy::cast_possible_truncation,
                    clippy::cast_sign_loss,
                    reason = "clamped to the image"
                )]
                let clamp = |v: f64, m: u32| v.floor().clamp(0.0, f64::from(m - 1)) as u32;
                let (value, alpha) =
                    self.value((clamp(center.x, width), clamp(center.y, height)), k);
                return Some(if alpha > 0.0 { value } else { Wide::default() });
            }
        };
        // In the swapped case the device x footprint steps source rows.
        let (source_columns, source_rows) = if swapped && self.minifying {
            (rows, columns)
        } else {
            (columns, rows)
        };
        // Colour associated with alpha averages, then the alpha divides it.
        let mut sum = Wide::default();
        let mut weight = Wide::default();
        for &(row, wr) in &source_rows {
            for &(column, wc) in &source_columns {
                let (value, alpha) = self.value((column, row), k);
                let w = two_product(wr, wc).mul(Wide::new(alpha));
                sum = sum.add(w.mul(value));
                weight = weight.add(w);
            }
        }
        (weight.high > 0.0).then(|| sum.div(weight))
    }
}

/// Counts for one image or a whole run.
#[derive(Default)]
struct Tally {
    channels: usize,
    changed: usize,
    changed_exact: usize,
    old_exact: usize,
    new_exact: usize,
    worst: f64,
    beyond: usize,
}

impl Tally {
    fn absorb(&mut self, other: &Self) {
        self.channels += other.channels;
        self.changed += other.changed;
        self.changed_exact += other.changed_exact;
        self.old_exact += other.old_exact;
        self.new_exact += other.new_exact;
        self.worst = self.worst.max(other.worst);
        self.beyond += other.beyond;
    }
}

/// The correctly rounded level of an exact unit value, half up. A level
/// within 1e-20 of an integer is that integer: the double-double carries
/// about 1e-30 of error, and an exact tie of these rationals lands there.
fn rounded(value: Wide) -> u8 {
    let level = value.mul(Wide::new(255.0)).add(Wide::new(0.5));
    let nearest = level.value().round();
    let floor = if level.add(Wide::new(-nearest)).value().abs() < 1e-20 {
        nearest
    } else {
        level.value().floor()
    };
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "clamped to the byte range"
    )]
    let byte = floor.clamp(0.0, 255.0) as u8;
    byte
}

/// Compares the new sampler of `placed` with the old route's and the oracle
/// on every pixel of its device bounds, logging changed channels.
fn compare(placed: &Placed, case: &Case, log: &mut String) -> Tally {
    let limits = pagura_core::Limits::default();
    let Some(new) =
        decode_once(placed, &limits, false).and_then(|decoded| sampler(placed, decoded))
    else {
        return Tally::default();
    };
    let rect = placed.rect;
    let mut old = Oracle::with_columns(case, placed.unit_to_device, 3, (rect.x0, rect.x1));
    let exact = Exact::new(case, placed.unit_to_device);
    let len = usize::try_from(rect.x1 - rect.x0).unwrap();
    let mut out = SpanColors::default();
    let mut tally = Tally::default();
    let components = usize::from(case.image.components);
    for y in rect.y0..rect.y1 {
        new.span(rect.x0, y, len, &mut out);
        for (i, x) in (rect.x0..rect.x1).enumerate() {
            let before = old.pixel(x, y);
            for channel in 0..out.channels {
                // Device grays and RGB convert channel by channel.
                let k = if components == 1 { 0 } else { channel };
                let Some(oracle) = exact.pixel((x, y), k) else {
                    continue;
                };
                let value = out.colors[i * out.channels + channel];
                let (q_old, q_new) = (quantize_unit(before.color[channel]), quantize_unit(value));
                let q_exact = rounded(oracle);
                let distance = (f64::from(q_new) - oracle.mul(Wide::new(255.0)).value()).abs();
                tally.channels += 1;
                tally.worst = tally.worst.max(distance);
                tally.beyond += usize::from(distance > 1.0);
                tally.old_exact += usize::from(q_old == q_exact);
                tally.new_exact += usize::from(q_new == q_exact);
                if q_old != q_new {
                    tally.changed += 1;
                    tally.changed_exact += usize::from(q_new == q_exact);
                    let _ = writeln!(
                        log,
                        "  ({x}, {y}) channel {channel}: old {q_old} new {q_new} oracle {:.12}",
                        oracle.mul(Wide::new(255.0)).value()
                    );
                }
            }
        }
    }
    tally
}

/// The images of one page as cases for the old route, decoded interleaved
/// on their own grid.
fn cases(placed: &[Placed]) -> Vec<Option<Case>> {
    let limits = pagura_core::Limits::default();
    placed
        .iter()
        .map(|placed| {
            let space = placed.space.clone()?;
            if !matches!(
                placed.source.mask(),
                SourceMask::None | SourceMask::ColorKey(_)
            ) || !matches!(space, ColorSpace::DeviceGray | ColorSpace::DeviceRgb)
            {
                return None;
            }
            let decoded = placed
                .source
                .decode(
                    &DecodeRequest::new(Reduce::Full, None, Layout::Interleaved),
                    &limits,
                    placed.source.recovery(),
                )
                .ok()?;
            let components = usize::from(decoded.image.components);
            let mut case = case(decoded.image);
            case.space = Some(space);
            case.decode = placed.source.decode_array().map_or_else(
                || vec![[0.0, 1.0]; components],
                |values| values.as_chunks::<2>().0.to_vec(),
            );
            if let SourceMask::ColorKey(ranges) = placed.source.mask() {
                case.key = Some(ranges.clone());
            }
            Some(case)
        })
        .collect()
}

/// Generated fixture documents with image XObjects.
fn fixtures() -> Vec<(String, Arc<Document>, usize)> {
    if setting("PAGURA_IMAGE_FIXTURES", "1") == "0" {
        return Vec::new();
    }
    let root = pagura_testkit::workspace_root().join("corpus/generated");
    let mut documents = Vec::new();
    for area in std::fs::read_dir(&root).into_iter().flatten().flatten() {
        for file in std::fs::read_dir(area.path())
            .into_iter()
            .flatten()
            .flatten()
        {
            let path = file.path();
            let Ok(bytes) = std::fs::read(&path) else {
                continue;
            };
            if path.extension().is_none_or(|e| e != "pdf")
                || !bytes.windows(5).any(|w| w == b"Image")
            {
                continue;
            }
            if let Ok(document) = Document::open(bytes, &OpenOptions::default()) {
                let name = path
                    .strip_prefix(&root)
                    .unwrap_or(&path)
                    .display()
                    .to_string();
                documents.push((name, Arc::new(document), 0));
            }
        }
    }
    documents.sort_by(|a, b| a.0.cmp(&b.0));
    documents
}

#[test]
#[ignore = "accuracy report; needs the benchmark corpus for its pages"]
fn image_accuracy() {
    let dpis: Vec<f64> = setting("PAGURA_IMAGE_DPI", "72,150,300")
        .split(',')
        .filter_map(|v| v.trim().parse().ok())
        .collect();
    let mut total = Tally::default();
    let mut log = String::new();
    let mut documents = fixtures();
    documents.extend(pages(IMAGE_PAGES));
    for (name, document, index) in documents {
        let Ok(page) = document.page(index) else {
            continue;
        };
        for &dpi in &dpis {
            let images = placed(&document, page, dpi / 72.0);
            for (placed, case) in images.iter().zip(cases(&images)) {
                let Some(case) = case else {
                    continue;
                };
                let mut changes = String::new();
                let tally = compare(placed, &case, &mut changes);
                if tally.changed > 0 || tally.beyond > 0 {
                    let _ = writeln!(
                        log,
                        "{name} {dpi}: {} of {} channels changed, worst {:.6} levels",
                        tally.changed, tally.channels, tally.worst
                    );
                    log.push_str(&changes);
                }
                total.absorb(&tally);
            }
        }
    }
    eprint!("{log}");
    #[expect(clippy::cast_precision_loss, reason = "report ratios")]
    let rate = |count: usize, of: usize| count as f64 / of.max(1) as f64;
    eprintln!(
        "channels {}: changed {} (exact {:.4}), old exact rounding {:.6}, new {:.6}, \
         worst {:.6} levels, beyond one level {}",
        total.channels,
        total.changed,
        rate(total.changed_exact, total.changed),
        rate(total.old_exact, total.channels),
        rate(total.new_exact, total.channels),
        total.worst,
        total.beyond
    );
    assert_eq!(total.beyond, 0, "values beyond one level of the oracle");
}

/// Differences of one comparison in levels.
#[derive(Default)]
struct Differences {
    channels: usize,
    differing: usize,
    largest: u8,
    total: u64,
}

impl Differences {
    fn add(&mut self, a: u8, b: u8) {
        let difference = a.abs_diff(b);
        self.channels += 1;
        self.differing += usize::from(difference != 0);
        self.largest = self.largest.max(difference);
        self.total += u64::from(difference);
    }

    #[expect(clippy::cast_precision_loss, reason = "report ratios")]
    fn report(&self, name: &str) -> String {
        format!(
            "{name}: {} of {} channels differ, largest {}, mean {:.5}",
            self.differing,
            self.channels,
            self.largest,
            self.total as f64 / self.channels.max(1) as f64
        )
    }
}

/// The device pixels wholly inside the unit square under `unit_to_device`
/// and within `rect`.
fn interior(unit_to_device: Transform, rect: pagura_raster::DeviceRect) -> (u32, u32, u32, u32) {
    let corners = [(0.0, 0.0), (1.0, 0.0), (0.0, 1.0), (1.0, 1.0)]
        .map(|(x, y)| Point::new(x, y) * unit_to_device);
    let low = |pick: fn(&Point) -> f64| corners.iter().map(pick).fold(f64::INFINITY, f64::min);
    let high = |pick: fn(&Point) -> f64| corners.iter().map(pick).fold(f64::NEG_INFINITY, f64::max);
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "clamped to the image's device bounds"
    )]
    let clamp = |v: f64, (a, b): (u32, u32)| v.clamp(f64::from(a), f64::from(b)) as u32;
    (
        clamp(low(|p| p.x).ceil(), (rect.x0, rect.x1)),
        clamp(low(|p| p.y).ceil(), (rect.y0, rect.y1)),
        clamp(high(|p| p.x).floor(), (rect.x0, rect.x1)),
        clamp(high(|p| p.y).floor(), (rect.y0, rect.y1)),
    )
}

/// Drawn-size decoding (`reduced_image_decode`) against the old
/// competitive route on the benchmark pages: the new sampler's quantized
/// colours over the reduced decode, the old route's page pixels, and both
/// against the exact oracle, on the device pixels wholly inside each image.
#[test]
#[ignore = "accuracy report; needs the benchmark corpus"]
fn reduced_against_competitive() {
    let dpis: Vec<f64> = setting("PAGURA_IMAGE_DPI", "72,150,300")
        .split(',')
        .filter_map(|v| v.trim().parse().ok())
        .collect();
    let limits = pagura_core::Limits::default();
    for (name, document, index) in pages(IMAGE_PAGES) {
        let Ok(page) = document.page(index) else {
            continue;
        };
        for &dpi in &dpis {
            let context = pagura_render::RenderContext::new(Arc::clone(&document));
            let options = pagura_render::RenderOptions {
                scale: dpi / 72.0,
                accuracy: pagura_render::Accuracy::Competitive,
                ..pagura_render::RenderOptions::default()
            };
            let Ok(old) = context.render_page(page, &options) else {
                continue;
            };
            let images = placed(&document, page, dpi / 72.0);
            let (mut against_old, mut new_oracle, mut old_oracle) = (
                Differences::default(),
                Differences::default(),
                Differences::default(),
            );
            for (placed, case) in images.iter().zip(cases(&images)) {
                let (Some(case), Some(new)) = (
                    case,
                    decode_once(placed, &limits, true).and_then(|d| sampler(placed, d)),
                ) else {
                    continue;
                };
                let exact = Exact::new(&case, placed.unit_to_device);
                let (x0, y0, x1, y1) = interior(placed.unit_to_device, placed.rect);
                let len = usize::try_from(x1.saturating_sub(x0)).unwrap();
                let mut out = SpanColors::default();
                let width = usize::try_from(old.width()).unwrap();
                for y in y0..y1 {
                    new.span(x0, y, len, &mut out);
                    for (i, x) in (x0..x1).enumerate() {
                        let at =
                            (usize::try_from(y).unwrap() * width + usize::try_from(x).unwrap()) * 4;
                        for channel in 0..out.channels.min(3) {
                            let k = if case.image.components == 1 {
                                0
                            } else {
                                channel
                            };
                            let fresh = quantize_unit(out.colors[i * out.channels + channel]);
                            let before = old.data()[at + channel];
                            against_old.add(fresh, before);
                            if let Some(oracle) = exact.pixel((x, y), k) {
                                let oracle = rounded(oracle);
                                new_oracle.add(fresh, oracle);
                                old_oracle.add(before, oracle);
                            }
                        }
                    }
                }
            }
            eprintln!(
                "{name} {dpi} | {} | {} | {}",
                against_old.report("new against old competitive"),
                new_oracle.report("new against oracle"),
                old_oracle.report("old competitive against oracle")
            );
        }
    }
}
