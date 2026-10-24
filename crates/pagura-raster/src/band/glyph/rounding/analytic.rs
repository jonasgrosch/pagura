//! Temporary original-control byte proofs within a completed inspection's loan.
//!
//! Unsupported evidence or an exhausted child meter leaves the original
//! sequential certificate collector responsible for the pixel. No proof
//! owner survives this call, and no mandatory preparation meter is borrowed.

use super::{Pixel, Source};
use crate::band::curve::coverage::{self, Work};
use crate::band::{ClipShape, Prep, RASTER_WORK, RENDER_BYTES};
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
    if let Some(rect) = options.clip.and_then(ClipShape::rect)
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

    fn third_outline() -> crate::Path {
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
        path.finish()
    }

    #[test]
    fn partial_setup_byte_refusal_keeps_parent_headroom() {
        let path = third_outline();
        let limits = Limits::default();
        // Exactly the first Builder owner fits. Its starts reservation must
        // fail after the pieces allocation has actually succeeded.
        let first_owner = (3 * path.segments().len() + 1) * size_of::<crate::band::curve::Piece>();
        let mut parent = Meter::new(first_owner + 7, RENDER_BYTES);
        let mut retained = Vec::<u8>::new();
        parent.reserve_capacity(&mut retained, 7).unwrap();
        retained.resize(7, 1);
        let before = parent.clone();
        let original = Pixel {
            x: 0,
            y: 0,
            byte: 3,
            hint: None,
        };
        let mut pending = [original];
        let mut work = Meter::new(limits.raster_work, RASTER_WORK);
        let mut child_bytes = Meter::new(parent.remaining(), RENDER_BYTES);
        let error = populate(
            source(&path),
            &options(&limits),
            &mut pending,
            &[],
            &mut child_bytes,
            &mut Work::new(&mut work),
        )
        .unwrap_err();
        assert_eq!(error.kind, pagura_core::ErrorKind::Limit);
        assert_eq!(error.message, RENDER_BYTES);
        assert_eq!(child_bytes.used(), first_owner);
        assert_eq!(pending[0].hint, None);
        assert_eq!(pending[0].byte, original.byte);

        // The public prepass catches that same optional refusal and cannot
        // retain an owner or poison the mandatory parent's next reservation.
        hint(
            source(&path),
            &options(&limits),
            &mut pending,
            &[],
            limits.raster_work,
            parent.remaining(),
        );
        assert_eq!(parent, before);
        assert_eq!(pending[0].hint, None);
        assert_eq!(retained, [1; 7]);
        let mut mandatory = Vec::<u8>::new();
        parent
            .reserve_capacity(&mut mandatory, first_owner)
            .unwrap();
        assert_eq!(parent.remaining(), 0);
    }

    fn third_hint_tariff(path: &crate::Path, options: &Prep<'_>, first: Pixel) -> usize {
        // Measure the literal tariff through one completed proof. This
        // calibrates a resource boundary without duplicating its arithmetic.
        let mut calibration = [first];
        let mut work = Meter::new(options.limits.raster_work, RASTER_WORK);
        let mut bytes = Meter::new(options.limits.render_bytes, RENDER_BYTES);
        populate(
            source(path),
            options,
            &mut calibration,
            &[],
            &mut bytes,
            &mut Work::new(&mut work),
        )
        .unwrap();
        assert_eq!(calibration[0].hint, Some(85));
        work.used()
    }

    #[test]
    fn exhausted_work_keeps_a_valid_hint_prefix_and_original_collector_error() {
        let path = third_outline();
        let limits = Limits::default();
        let mut options = options(&limits);
        options.area.region.x1 = 2;
        let first = Pixel {
            x: 0,
            y: 0,
            byte: 3,
            hint: None,
        };
        let second = Pixel { x: 1, ..first };
        let first_cost = third_hint_tariff(&path, &options, first);

        let mut pair = [first, second];
        let mut work = Meter::new(first_cost, RASTER_WORK);
        let mut bytes = Meter::new(limits.render_bytes, RENDER_BYTES);
        let error = populate(
            source(&path),
            &options,
            &mut pair,
            &[],
            &mut bytes,
            &mut Work::new(&mut work),
        )
        .unwrap_err();
        assert_eq!(error.kind, pagura_core::ErrorKind::Limit);
        assert_eq!(work.used(), first_cost);
        assert_eq!(pair[0].hint, Some(85));
        assert_eq!(pair[1].hint, None);
        assert_eq!([pair[0].byte, pair[1].byte], [3, 3]);

        let paid = first_cost + 16;
        let mut parent_work = Meter::new(paid + 2, RASTER_WORK);
        parent_work.charge(paid).unwrap();
        let mut parent_bytes = Meter::new(limits.render_bytes, RENDER_BYTES);
        let mut pending = Vec::new();
        parent_bytes.reserve_capacity(&mut pending, 2).unwrap();
        pending.extend([first, second]);
        let before_work = parent_work.clone();
        let before_bytes = parent_bytes.clone();
        hint(
            source(&path),
            &options,
            &mut pending,
            &[],
            first_cost,
            parent_bytes.remaining(),
        );
        assert_eq!(parent_work, before_work);
        assert_eq!(parent_bytes, before_bytes);
        assert_eq!(pending[0].hint, Some(85));
        assert_eq!(pending[1].hint, None);

        // The valid first hint still enters the unchanged collector before
        // its next mandatory lookup reports the original work-budget error.
        let (mut targets, mut proven) = (Vec::new(), Vec::new());
        let error = super::super::collect_targets(
            source(&path),
            pending,
            (&mut targets, &mut proven),
            &options,
            crate::band::union::Meters {
                work: &mut parent_work,
                bytes: &mut parent_bytes,
            },
        )
        .unwrap_err();
        assert_eq!(error.kind, pagura_core::ErrorKind::Limit);
        assert_eq!(error.message, RASTER_WORK);
        assert_eq!(parent_work.used(), paid + 2);
        assert_eq!(proven.len(), 1);
        assert_eq!(proven[0].byte, 85);
        assert_eq!(targets, [[0.0, 0.0, 1.0, 1.0]]);
    }
}
