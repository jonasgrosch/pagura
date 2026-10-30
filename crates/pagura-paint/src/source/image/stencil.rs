//! A non-interpolated stencil as exact sample-cell geometry (8.9.6.2).
//! Adjacent painted samples of one row share a rectangle. Keeping this
//! geometry through pattern preparation preserves mask/pattern correlation
//! at fractional device pixels; multiplying two area averages would not.

use super::{ImageBudgets, ImageSampler, Orientation, decode, decoded_bytes};
use crate::source::PaintContext;
use pagura_core::convert::u32_to_usize;
use pagura_core::{Diagnostic, Error, Result};
use pagura_image::ImageKind;
use pagura_interpret::Image;
use pagura_raster::{Path, PathBuilder, Point, Transform};

impl ImageSampler {
    /// Builds a stencil's painted sample cells in the image's user space.
    /// Interpolated masks retain their explicit unsupported result: their
    /// continuously varying shape cannot be represented by these cells.
    pub(crate) fn stencil_path(
        image: &Image,
        transform: Transform,
        context: &PaintContext<'_>,
        repairs: &mut Vec<Diagnostic>,
        budgets: ImageBudgets<'_>,
    ) -> Result<Path> {
        if image.interpolate {
            return Err(Error::unsupported(
                pagura_interpret::features::PATTERN_IMAGE_MASK,
                "interpolated pattern-coloured stencils are not implemented",
            ));
        }
        if transform.then(context.transform).inverse().is_none() {
            return Err(Error::unsupported(
                pagura_interpret::features::PATTERN_IMAGE_MASK,
                "singular pattern stencil transform",
            ));
        }
        let ImageBudgets {
            bytes,
            work,
            functions: _,
        } = budgets;
        let decoded = decode(&image.source, (false, Orientation::Rows), context)?;
        let retained = decoded_bytes(&decoded.image);
        bytes.charge(retained)?;
        let mut path = PathBuilder::new();
        let result = (|| {
            bytes.reserve_capacity(repairs, decoded.diagnostics.len())?;
            repairs.extend_from_slice(&decoded.diagnostics);
            let mask = &decoded.image;
            let ImageKind::Stencil { inverted } = &mask.kind else {
                return Err(Error::invalid_argument("pattern stencil is not a mask"));
            };
            let width = u32_to_usize(mask.width);
            let height = u32_to_usize(mask.height);
            work.charge_product(width, height)?;
            work.charge_product(width, height)?;
            for row in 0..mask.height {
                let start = u32_to_usize(row)
                    .checked_mul(width)
                    .ok_or(Error::limit("stencil sample offset overflow"))?;
                let paints = |column: u32| -> Result<bool> {
                    let index = start
                        .checked_add(u32_to_usize(column))
                        .ok_or(Error::limit("stencil sample offset overflow"))?;
                    mask.samples
                        .get(index)
                        .map(|code| code == u16::from(*inverted))
                        .ok_or(Error::syntax("stencil sample count mismatch"))
                };
                let mut column = 0;
                while column < mask.width {
                    if !paints(column)? {
                        column += 1;
                        continue;
                    }
                    let first = column;
                    column += 1;
                    while column < mask.width && paints(column)? {
                        column += 1;
                    }
                    let x0 = f64::from(first) / f64::from(mask.width);
                    let x1 = f64::from(column) / f64::from(mask.width);
                    let y0 = 1.0 - f64::from(row + 1) / f64::from(mask.height);
                    let y1 = 1.0 - f64::from(row) / f64::from(mask.height);
                    if path.len().saturating_add(5) > context.limits.path_segments {
                        return Err(Error::limit("stencil path segment budget exceeded"));
                    }
                    path.reserve(5, bytes)?;
                    path.polygon(
                        [
                            Point::new(x0, y0),
                            Point::new(x1, y0),
                            Point::new(x1, y1),
                            Point::new(x0, y1),
                        ]
                        .map(|point| transform.apply(point)),
                    );
                }
            }
            Ok(())
        })();
        drop(decoded);
        bytes.refund(retained);
        let path = path.finish();
        if let Err(error) = result {
            let released = path.retained_bytes();
            drop(path);
            bytes.refund(released);
            return Err(error);
        }
        Ok(path)
    }
}
