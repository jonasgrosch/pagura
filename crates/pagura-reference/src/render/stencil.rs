//! Patterned stencils (8.9.6.2, 8.7.3, 11.6.7), independently as sample
//! quadrilaterals in the reference's slab-integrated regions. This uses no
//! product stencil path, row-run compression or prepared raster geometry.

use super::{Context, Renderer};
use crate::area::Region;
use crate::buffer::Buffer;
use crate::shapes::Coverage;
use pagura_core::{Error, Result};
use pagura_image::ImageKind;
use pagura_interpret::{Composite, ImageNode, Paint};
use pagura_raster::{FillRule, Point, Transform};

impl Renderer<'_> {
    /// Paints the pattern over the exact union of marked source sample
    /// cells. Tiling children inherit these device-space regions; shading
    /// uses the ordinary reference colour-sampling contract over them.
    pub(super) fn pattern_stencil(
        &mut self,
        node: &ImageNode,
        paint: &Paint,
        state: &Composite,
        buffer: &mut Buffer,
        context: &Context<'_>,
    ) -> Result<()> {
        if node.image.interpolate {
            return Err(Error::invalid_argument(
                "the reference has no interpolated patterned stencil",
            ));
        }
        let image = self.decoded(&node.image)?;
        let ImageKind::Stencil { inverted } = image.kind else {
            return Err(Error::invalid_argument(
                "pattern stencil has colour samples",
            ));
        };
        if image.components != 1 || image.bits_per_component != 1 {
            return Err(Error::invalid_argument("pattern stencil is not one bit"));
        }
        let samples = usize::try_from(u64::from(image.width) * u64::from(image.height))
            .map_err(|_| Error::limit("reference stencil sample count overflow"))?;
        if samples > self.limits.path_segments / 4 {
            return Err(Error::limit("reference stencil geometry budget exceeded"));
        }
        if samples != image.samples.len() {
            return Err(Error::syntax("pattern stencil sample count mismatch"));
        }
        let transform = node.transform.then(context.device);
        let mut region = Region::new(FillRule::NonZero);
        for index in 0..samples {
            if image.samples.get(index) == Some(u16::from(inverted)) {
                let width = usize::try_from(image.width)
                    .map_err(|_| Error::limit("reference stencil width overflow"))?;
                let column = u32::try_from(index % width)
                    .map_err(|_| Error::limit("reference stencil column overflow"))?;
                let row = u32::try_from(index / width)
                    .map_err(|_| Error::limit("reference stencil row overflow"))?;
                let corners = sample_cell(column, row, image.width, image.height, transform);
                region.push_contour(&corners);
            }
        }
        let mut regions = self.clips(node.mark.clip, context)?;
        regions.push(region);
        match paint {
            Paint::Tiling(pattern) => {
                let coverage = Coverage::of(&regions, buffer.rect(), self.request.quantization)?;
                self.tiling(pattern, (&coverage, Some(&regions)), state, buffer, context)
            }
            Paint::Shading(_) => self.paint_regions(regions, paint, state, buffer, context),
            Paint::Solid(_) => Err(Error::invalid_argument("pattern stencil without a pattern")),
        }
    }
}

/// The image grid has its first sample at the upper-left corner of the
/// unit square (8.9.3); the four corners keep their geometry under the CTM.
fn sample_cell(x: u32, y: u32, width: u32, height: u32, transform: Transform) -> [Point; 4] {
    let left = f64::from(x) / f64::from(width);
    let right = f64::from(x + 1) / f64::from(width);
    let upper = 1.0 - f64::from(y) / f64::from(height);
    let lower = 1.0 - f64::from(y + 1) / f64::from(height);
    [
        Point::new(left, upper),
        Point::new(left, lower),
        Point::new(right, lower),
        Point::new(right, upper),
    ]
    .map(|point| point * transform)
}
