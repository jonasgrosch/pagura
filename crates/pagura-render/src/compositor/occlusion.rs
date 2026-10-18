//! Paint surviving intervals of an otherwise opaque solid rectangle.

use super::{Context, Model, Pixel, Source, Surface};
use crate::display::PathCommand;
mod plan;
use pagura_core::{Error, Rect, Result};
use pagura_raster::Mask;
pub(super) use plan::Plan;

impl Context {
    pub(super) fn occluded_rectangle(
        &mut self,
        path: &PathCommand,
        regions: &[Rect],
        target: &mut Surface,
    ) -> Result<()> {
        if !super::path_active(path, Model::Rgb)? {
            return Ok(());
        }
        let Some(mask) = self.path_mask(path, target.tile)? else {
            return Ok(());
        };
        let result = self.occluded_path(path, &mask, target, regions);
        self.raster.recycle(mask);
        result
    }

    fn occluded_path(
        &mut self,
        path: &PathCommand,
        mask: &Mask,
        target: &mut Surface,
        regions: &[Rect],
    ) -> Result<()> {
        self.charge(mask.coverage().len())?;
        let color = Model::Rgb.color(
            &path.color,
            &path.composite.conversion,
            &mut self.function_work,
        )?;
        let (ox, oy) = mask.origin();
        let width =
            usize::try_from(mask.width()).map_err(|_| Error::limit("mask width exceeds usize"))?;
        for (row, coverage) in mask.coverage().chunks_exact(width).enumerate() {
            let y = oy + u32::try_from(row).map_err(|_| Error::limit("mask row exceeds u32"))?;
            for (column, coverage) in plan::row(coverage, (ox, y), regions) {
                let x = ox
                    + u32::try_from(column).map_err(|_| Error::limit("mask column exceeds u32"))?;
                let index = target
                    .index(x, y)
                    .ok_or(Error::invalid_argument("mask outside group"))?;
                let shape = f64::from(coverage) / 255.0;
                let source = Source {
                    color,
                    alpha: shape,
                    shape,
                };
                target
                    .pixels
                    .get_mut(index)
                    .ok_or(Error::invalid_argument("invalid group index"))?
                    .paint(source, &path.composite, Pixel::default(), false, Model::Rgb);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        RenderOptions, Tile,
        color::{Color, Space},
        display::{Command, DisplayList, Mark},
        transparency::{Attributes, Composite, Group},
    };
    use pagura_core::{Limits, Position};
    use pagura_document::Rotation;
    use pagura_raster::{FillRule, PathBuilder, Rgba8};
    use std::sync::Arc;

    fn mark(rect: Rect, gray: f64) -> Command {
        let mut path = PathBuilder::new();
        path.rect(rect.x0, rect.y0, rect.width(), rect.height());
        Command::Path(PathCommand {
            path: Arc::new(path.finish()),
            mark: Mark::Fill(FillRule::NonZero),
            clip: None,
            color: Color::Solid {
                space: Space::Gray,
                components: vec![gray],
            },
            position: Position::Unknown,
            transfer: None,
            composite: Composite::default(),
        })
    }

    #[test]
    fn opaque_interiors_match_exact_compositing_and_tile_partitions() {
        let crop = Rect {
            x0: 0.0,
            y0: 0.0,
            x1: 64.0,
            y1: 64.0,
        };
        let rect = Rect {
            x0: 4.25,
            y0: 6.5,
            x1: 60.75,
            y1: 61.125,
        };
        // The empty translucent group exercises the floating compositor without
        // adding any paint or crossing either rectangle's occlusion boundary.
        let list = DisplayList {
            commands: vec![
                mark(crop, 0.3),
                mark(rect, 1.0),
                Command::Group(Group {
                    position: Position::Unknown,
                    bounds: None,
                    commands: vec![],
                    attributes: Attributes::default(),
                    composite: Composite {
                        alpha: 0.5,
                        ..Composite::default()
                    },
                }),
            ],
            crop,
            rotation: Rotation::None,
            user_unit: 1.0,
            limits: Limits::default(),
            group: Attributes::default(),
        };
        let options = RenderOptions {
            background: Rgba8::TRANSPARENT,
            ..RenderOptions::default()
        };
        let full = list.render(&options).unwrap();
        for (y, row) in full.data().as_chunks::<256>().0.iter().enumerate() {
            for (x, pixel) in row.as_chunks::<4>().0.iter().enumerate() {
                let x = f64::from(u32::try_from(x).unwrap());
                let y = 63.0 - f64::from(u32::try_from(y).unwrap());
                let width = ((x + 1.0).min(rect.x1) - x.max(rect.x0)).clamp(0.0, 1.0);
                let height = ((y + 1.0).min(rect.y1) - y.max(rect.y0)).clamp(0.0, 1.0);
                let coverage = (width * height * 255.0).round() / 255.0;
                let expected = ((1.0 - coverage) * 0.3 * 255.0 + coverage * 255.0).round();
                assert_eq!(
                    pixel[..3]
                        .iter()
                        .copied()
                        .map(f64::from)
                        .collect::<Vec<_>>(),
                    [expected; 3]
                );
                assert_eq!(pixel[3], 255);
            }
        }
        for y in [0, 32] {
            for x in [0, 32] {
                let tile = list
                    .render_tile(
                        &options,
                        Tile {
                            x,
                            y,
                            width: 32,
                            height: 32,
                        },
                    )
                    .unwrap();
                for row in 0..32 {
                    let expected = &full.row(y + row).unwrap()
                        [usize::try_from(x).unwrap() * 4..usize::try_from(x + 32).unwrap() * 4];
                    assert_eq!(tile.row(row).unwrap(), expected);
                }
            }
        }
    }
}
