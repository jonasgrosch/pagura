//! Axial shadings whose axis is aligned with the device grid: the parameter
//! of a sample then depends on one device coordinate only, so a row shares
//! one evaluation (a vertical axis) or every row shares the evaluation of a
//! column (a horizontal axis).
//!
//! The sharing is exact. With the inverse transform's cross term and the
//! axis's other component zero, the cross products contribute signed zeros;
//! `u + (±0)` is `u` for any nonzero `u`, so a sample's parameter, and
//! everything computed from it, is bit-identical along the shared direction.
//! Samples whose projection `u` is zero are evaluated pixel by pixel.

use super::geometry::Axial;
use super::{Geometry, ShadingSource, Span, SpanColors};
use pagura_color::MAX_COMPONENTS;
use pagura_raster::Point;

/// The most columns a column table holds.
const MAX_COLUMNS: usize = 1 << 16;

#[derive(Debug)]
pub(super) struct Aligned {
    direction: Direction,
    /// Pixels whose every sample lies before the axis start, and after its
    /// end: their colour and alpha.
    before: Pixel,
    after: Pixel,
    /// For columns: the pixels of the columns where the shading varies.
    table: Option<Table>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Direction {
    /// The parameter depends on the row only.
    Rows,
    /// The parameter depends on the column only.
    Columns,
}

#[derive(Debug)]
struct Pixel {
    color: Box<[f64]>,
    alpha: f64,
}

#[derive(Debug)]
struct Table {
    first: u32,
    /// `channels` values per column.
    colors: Box<[f64]>,
    alpha: Box<[f64]>,
    /// Whether the column's samples all have nonzero projections.
    valid: Box<[bool]>,
}

impl Aligned {
    pub(super) fn new(source: &ShadingSource) -> Option<Self> {
        let Geometry::Axial(axial) = &*source.geometry else {
            return None;
        };
        // A pattern's `BBox` test and `Background` depend on the whole
        // sample point.
        let tested = source
            .pattern
            .as_ref()
            .is_some_and(|p| p.bbox.is_some() || p.background.is_some());
        if tested || !source.moderate(axial) {
            return None;
        }
        let i = source.inverse;
        let direction = if i.c == 0.0 && axial.delta.y == 0.0 {
            Direction::Columns
        } else if i.b == 0.0 && axial.delta.x == 0.0 {
            Direction::Rows
        } else {
            return None;
        };
        let pixel = |input: f64| {
            let mut color = vec![0.0; source.channels];
            let alpha = source.uniform(axial.input(input), &mut color);
            Pixel {
                color: color.into(),
                alpha,
            }
        };
        let mut aligned = Self {
            direction,
            before: pixel(-1.0),
            after: pixel(2.0),
            table: None,
        };
        if direction == Direction::Columns {
            aligned.table = Table::new(source, axial);
        }
        Some(aligned)
    }

    /// Pixels evaluated while preparing.
    pub(super) fn pixels(&self) -> usize {
        2 + self.table.as_ref().map_or(0, |t| t.valid.len())
    }

    pub(super) fn bytes(&self) -> usize {
        self.table
            .as_ref()
            .map_or(0, |t| (t.colors.len() + t.alpha.len()) * 8 + t.valid.len())
    }

    pub(super) fn fill(
        &self,
        source: &ShadingSource,
        axial: &Axial,
        span: Span<'_>,
        out: &mut SpanColors,
    ) {
        match self.direction {
            Direction::Rows => Self::fill_row(source, axial, span, out),
            Direction::Columns => self.fill_columns(source, axial, span, out),
        }
    }

    /// One evaluation for the whole span when the row's projections are
    /// nonzero.
    fn fill_row(source: &ShadingSource, axial: &Axial, span: Span<'_>, out: &mut SpanColors) {
        let channels = source.channels.max(1);
        let shared = (0..source.rate).all(|sy| {
            let p = source.sample_point((span.x, span.y), (0, sy));
            (p.y - axial.start.y) * axial.delta.y != 0.0
        });
        if !shared {
            source.fill_with(axial, span, out);
            return;
        }
        let SpanColors { colors, alpha, .. } = out;
        let split = channels.min(colors.len());
        let (first, rest) = colors.split_at_mut(split);
        let value = source.pixel(
            axial,
            (span.x, span.y),
            None,
            (first, &mut [0.0; MAX_COMPONENTS]),
        );
        for chunk in rest.chunks_exact_mut(channels) {
            chunk.copy_from_slice(first);
        }
        alpha.fill(value);
    }

    /// Column table lookups, with the extended ends as constants.
    fn fill_columns(
        &self,
        source: &ShadingSource,
        axial: &Axial,
        span: Span<'_>,
        out: &mut SpanColors,
    ) {
        let channels = source.channels.max(1);
        let SpanColors { colors, alpha, .. } = out;
        for (i, (color, alpha)) in colors
            .chunks_exact_mut(channels)
            .zip(alpha.iter_mut())
            .enumerate()
        {
            let x = span.x.saturating_add(u32::try_from(i).unwrap_or(u32::MAX));
            if let Some(table) = &self.table
                && let Some(index) = x.checked_sub(table.first).map(|i| i as usize)
                && table.valid.get(index) == Some(&true)
            {
                color.copy_from_slice(
                    table
                        .colors
                        .get(index * channels..(index + 1) * channels)
                        .unwrap_or_default(),
                );
                *alpha = table.alpha.get(index).copied().unwrap_or(0.0);
                continue;
            }
            let pixel = match classify(source, axial, (x, span.y)) {
                Class::Before => &self.before,
                Class::After => &self.after,
                Class::Other => {
                    *alpha = source.pixel(
                        axial,
                        (x, span.y),
                        None,
                        (color, &mut [0.0; MAX_COMPONENTS]),
                    );
                    continue;
                }
            };
            color.copy_from_slice(&pixel.color);
            *alpha = pixel.alpha;
        }
    }
}

/// Where a column's samples lie on the axis.
enum Class {
    Before,
    After,
    Other,
}

/// Whether every sample of pixel `(x, y)` lies strictly before the axis
/// start or after its end, with nonzero projections.
fn classify(source: &ShadingSource, axial: &Axial, (x, y): (u32, u32)) -> Class {
    let (mut before, mut after) = (true, true);
    for sx in 0..source.rate {
        let p = source.sample_point((x, y), (sx, 0));
        if (p.x - axial.start.x) * axial.delta.x == 0.0 {
            return Class::Other;
        }
        let t = axial.parameter(p);
        before &= t < 0.0;
        after &= t > 1.0;
    }
    match (before, after) {
        (true, _) => Class::Before,
        (_, true) => Class::After,
        _ => Class::Other,
    }
}

impl Table {
    /// The pixels of the columns between the axis ends, evaluated on row 0.
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "column bounds are clamped to the u32 device grid"
    )]
    fn new(source: &ShadingSource, axial: &Axial) -> Option<Self> {
        let t = source.transform();
        let end = Point::new(axial.start.x + axial.delta.x, axial.start.y + axial.delta.y);
        let (a, b) = (t.apply(axial.start).x, t.apply(end).x);
        let first = (a.min(b).floor() - 2.0).clamp(0.0, f64::from(u32::MAX));
        let last = (a.max(b).ceil() + 2.0).clamp(0.0, f64::from(u32::MAX));
        let count = (last - first) as usize + 1;
        if !(first.is_finite() && last.is_finite()) || count > MAX_COLUMNS {
            return None;
        }
        let first = first as u32;
        let channels = source.channels.max(1);
        let mut colors = vec![0.0; count * channels];
        let mut alpha = vec![0.0; count];
        let mut valid = vec![false; count];
        for (i, ((color, alpha), valid)) in colors
            .chunks_exact_mut(channels)
            .zip(alpha.iter_mut())
            .zip(valid.iter_mut())
            .enumerate()
        {
            let x = first.saturating_add(u32::try_from(i).unwrap_or(u32::MAX));
            *valid = (0..source.rate).all(|sx| {
                let p = source.sample_point((x, 0), (sx, 0));
                (p.x - axial.start.x) * axial.delta.x != 0.0
            });
            if *valid {
                *alpha = source.pixel(axial, (x, 0), None, (color, &mut [0.0; MAX_COMPONENTS]));
            }
        }
        Some(Self {
            first,
            colors: colors.into(),
            alpha: alpha.into(),
            valid: valid.into(),
        })
    }
}

impl ShadingSource {
    /// The shading-space point of sample `(sx, sy)` of pixel `(x, y)`.
    pub(super) fn sample_point(&self, (x, y): (u32, u32), (sx, sy): (u32, u32)) -> Point {
        let divisor = f64::from(self.rate);
        Point::new(
            f64::from(x) + (f64::from(sx) + 0.5) / divisor,
            f64::from(y) + (f64::from(sy) + 0.5) / divisor,
        ) * self.inverse
    }

    /// The pixel whose every sample has the function input `input` (or
    /// misses for `None`), as [`ShadingSource::pixel`] computes it.
    fn uniform(&self, input: Option<f64>, color: &mut [f64]) -> f64 {
        let Some(x) = input else {
            return 0.0;
        };
        self.pixel(&Fixed(x), (0, 0), None, (color, &mut [0.0; MAX_COMPONENTS]))
    }

    /// Whether device coordinates below 2³² map to parameters far from
    /// overflow, so that the shared arithmetic stays finite.
    pub(super) fn moderate(&self, axial: &Axial) -> bool {
        let i = self.inverse;
        let coordinate = (i.a.abs() + i.b.abs() + i.c.abs() + i.d.abs()) * 4_294_967_296.0
            + i.e.abs()
            + i.f.abs()
            + axial.start.x.abs()
            + axial.start.y.abs();
        let bound = coordinate * (axial.delta.x.abs() + axial.delta.y.abs()) / axial.denominator;
        axial.denominator > 0.0 && bound.is_finite() && bound < 1e300
    }
}

/// A geometry whose every sample has one function input.
struct Fixed(f64);

impl super::Sampler for Fixed {
    fn sample(&self, _: Point, input: &mut [f64; MAX_COMPONENTS]) -> bool {
        input[0] = self.0;
        true
    }
}
