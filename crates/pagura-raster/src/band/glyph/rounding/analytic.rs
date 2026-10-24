//! Temporary original-control byte proofs within a completed inspection's loan.
//!
//! Unsupported evidence or an exhausted child meter leaves the original
//! sequential certificate collector responsible for the pixel. No proof
//! owner survives this call, and no mandatory preparation meter is borrowed.

use super::{Pixel, Source};
use crate::band::curve::coverage::{self, Work};
use crate::band::{Prep, RASTER_WORK, RENDER_BYTES};
use crate::quantize_unit;
use pagura_core::{Meter, Result};

pub(super) fn hint(
    source: Source<'_>,
    options: &Prep<'_>,
    pending: &mut [Pixel],
    proven: &[Pixel],
    surplus: usize,
    remaining_bytes: usize,
) {
    if pending.is_empty() || surplus == 0 || remaining_bytes == 0 {
        return;
    }
    let mut work = Meter::new(surplus, RASTER_WORK);
    let mut bytes = Meter::new(remaining_bytes, RENDER_BYTES);
    // Only this optional prepass catches its own errors. All earlier valid
    // hints remain useful, and every temporary owner is dropped on return.
    let _ = populate(
        source,
        options,
        pending,
        proven,
        &mut bytes,
        &mut Work::new(&mut work),
    );
}

fn populate(
    source: Source<'_>,
    options: &Prep<'_>,
    pending: &mut [Pixel],
    proven: &[Pixel],
    bytes: &mut Meter,
    work: &mut Work<'_>,
) -> Result<()> {
    // Reserve the fixed entry, clip and temporary-owner teardown blocks
    // before any optional allocation or successful hint can be published.
    work.blocks(16)?;
    if options.clip.is_some_and(|clip| clip.paths().is_some())
        || !source.origin.x.is_finite()
        || !source.origin.y.is_finite()
    {
        return Ok(());
    }
    let mut first = None;
    for (index, pixel) in pending.iter().enumerate() {
        work.blocks(1)?;
        if !known(pixel, proven, work)? {
            first = Some(index);
            break;
        }
    }
    let Some(first) = first else {
        return Ok(());
    };
    let Some(metadata) = coverage::prepare(
        source.outline,
        source.linear,
        source.points,
        options.limits,
        bytes,
        work,
    )?
    else {
        return Ok(());
    };
    if let Some(rect) = options.clip.and_then(|clip| clip.rect())
        && !metadata.inside_rect(source.origin, rect, work)?
    {
        return Ok(());
    }
    for pixel in pending.iter_mut().skip(first) {
        work.blocks(1)?;
        if known(pixel, proven, work)? {
            continue;
        }
        let Some(area) = metadata.unit(source.origin, [pixel.x, pixel.y], work)? else {
            continue;
        };
        work.blocks(1)?;
        let byte = quantize_unit(area.lower());
        if byte == quantize_unit(area.upper()) {
            work.blocks(1)?;
            pixel.hint = Some(byte);
        }
    }
    Ok(())
}

/// The previous replacement passes keep this prefix sorted by row and column.
fn known(pixel: &Pixel, proven: &[Pixel], work: &mut Work<'_>) -> Result<bool> {
    let key = (pixel.y, pixel.x);
    let mut lo = 0;
    let mut hi = proven.len();
    while lo < hi {
        work.blocks(1)?;
        let mid = lo + (hi - lo) / 2;
        let Some(known) = proven.get(mid) else {
            return Ok(false);
        };
        match (known.y, known.x).cmp(&key) {
            std::cmp::Ordering::Less => lo = mid + 1,
            std::cmp::Ordering::Equal => return Ok(true),
            std::cmp::Ordering::Greater => hi = mid,
        }
    }
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Area, DeviceRect, PathBuilder, Point, Transform};
    use pagura_core::Limits;

    fn source(path: &crate::Path) -> Source<'_> {
        Source {
            outline: path,
            linear: Transform::IDENTITY,
            origin: Point::default(),
            points: &[],
            initial_area: super::super::AREA,
            proof: None,
        }
    }

    fn options(limits: &Limits) -> Prep<'_> {
        Prep {
            area: Area {
                region: DeviceRect {
                    x0: 0,
                    y0: 0,
                    x1: 1,
                    y1: 1,
                },
                band_height: 1,
            },
            limits,
            clip: None,
        }
    }

    #[test]
    fn all_known_pixels_decline_before_any_metadata_owner() {
        let path = PathBuilder::new().finish();
        let limits = Limits::default();
        let mut pending = [Pixel {
            x: 0,
            y: 0,
            byte: 0,
            hint: None,
        }];
        let proven = [Pixel {
            byte: 85,
            ..pending[0]
        }];
        let mut work = Meter::new(32, RASTER_WORK);
        let mut bytes = Meter::new(0, RENDER_BYTES);
        populate(
            source(&path),
            &options(&limits),
            &mut pending,
            &proven,
            &mut bytes,
            &mut Work::new(&mut work),
        )
        .unwrap();
        assert_eq!(pending[0].hint, None);
        assert_eq!(bytes.used(), 0);
        assert!(work.used() <= 32);
    }

    #[test]
    fn a_hint_preserves_geometry_byte_and_optional_refusals_leave_fallback() {
        let mut path = PathBuilder::new();
        path.move_to(Point::default());
        path.line_to(Point::new(1.0 / 3.0, 0.0)).unwrap();
        path.cubic_to(
            Point::new(1.0 / 3.0, 1.0 / 3.0),
            Point::new(1.0 / 3.0, 2.0 / 3.0),
            Point::new(1.0 / 3.0, 1.0),
        )
        .unwrap();
        path.line_to(Point::new(0.0, 1.0)).unwrap();
        path.close();
        let path = path.finish();
        let limits = Limits::default();
        let original = Pixel {
            x: 0,
            y: 0,
            byte: 3,
            hint: None,
        };
        let mut pending = [original];
        hint(
            source(&path),
            &options(&limits),
            &mut pending,
            &[],
            limits.raster_work,
            limits.render_bytes,
        );
        assert_eq!(pending[0].byte, 3);
        assert_eq!(pending[0].hint, Some(85));
        for (work, bytes) in [(0, limits.render_bytes), (limits.raster_work, 0), (16, 16)] {
            pending = [original];
            hint(
                source(&path),
                &options(&limits),
                &mut pending,
                &[],
                work,
                bytes,
            );
            assert_eq!(pending[0].hint, None);
            assert_eq!(pending[0].byte, 3);
        }
    }
}
