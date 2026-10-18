//! The page: chunked rendering, spot passes and conversion to output pixels.

use super::{Backdrop, Context, Frame, Separations, Surface, plan, transfer};
use crate::display::DisplayList;
use crate::transparency::Model;
use crate::{RenderOptions, Tile};
use pagura_core::Result;
use pagura_raster::Pixmap;
use std::sync::Arc;

impl Context {
    /// The compositor state for one render of `list` under its storage
    /// `plan`, and its page blending model.
    pub(super) fn page(
        list: &DisplayList,
        options: &RenderOptions,
        plan: &plan::Plan,
        separations: Arc<Separations>,
    ) -> Result<(Self, Model)> {
        let transfers = transfer::Regions::new(&list.commands, list.limits)?;
        let allocated = transfers.as_ref().map_or(0, transfer::Regions::bytes);
        let elide_opaque_groups =
            plan.format == plan::Format::F64 && !separations.enabled && list.scene().normal;
        let model = list.group.model.unwrap_or(if separations.enabled {
            Model::Cmyk
        } else {
            Model::Rgb
        });
        let context = Self {
            overprint: options.overprint.resolve(list.print_intent),
            separations,
            elide_opaque_groups,
            view_unit: list.user_unit,
            allocated,
            work: plan.work,
            transfers,
            ..Self::new(
                list.limits,
                list.device_transform(options),
                crate::device::rectangle(list.crop),
            )
        };
        Ok((context, model))
    }
}

// Keep the large general compositor out of the opaque page dispatch.
#[inline(never)]
pub(crate) fn render(list: &DisplayList, options: &RenderOptions, tile: Tile) -> Result<Pixmap> {
    let separations = list.preview_separations(options)?;
    let plan = plan::page(list, &separations)?;
    let (mut context, model) = Context::page(list, options, &plan, separations)?;
    let mut output =
        Pixmap::new_with_limits(tile.width, tile.height, options.background, list.limits)?;
    // Keep the high-precision page backdrop bounded too. Global device
    // coordinates preserve exact tile parity; one work budget spans all chunks.
    let mut y = tile.y;
    while y < tile.y + tile.height {
        let height = (tile.y + tile.height - y).min(plan.side);
        let mut x = tile.x;
        while x < tile.x + tile.width {
            let width = (tile.x + tile.width - x).min(plan.side);
            let region = Tile {
                x,
                y,
                width,
                height,
            };
            context.render_chunk(
                list,
                model,
                &mut output,
                tile,
                region,
                (
                    plan.format,
                    [
                        options.background.r,
                        options.background.g,
                        options.background.b,
                        options.background.a,
                    ],
                ),
            )?;
            x += width;
        }
        y += height;
    }
    Ok(output)
}

impl Context {
    fn render_chunk(
        &mut self,
        list: &DisplayList,
        model: Model,
        output: &mut Pixmap,
        tile: Tile,
        region: Tile,
        (format, background): (plan::Format, [u8; 4]),
    ) -> Result<()> {
        if let [crate::display::Command::Image(image)] = list.commands.as_slice()
            && !list.group.knockout
            && self.copy_opaque_image(image, model, output, tile, region)?
        {
            return Ok(());
        }
        self.clip = None;
        self.discard_mask();
        self.begin_transfers(region)?;
        let mut page = self.page_surface(region, format, background)?;
        let frame = Frame {
            initial: Backdrop::Transparent,
            model,
            knockout: list.group.knockout,
            depth: 0,
        };
        self.commands(&list.commands, &mut page, frame)?;
        // The page pass has selected every pixel's transfer; spot passes replay
        // the same marks for colourants only.
        if let Some(regions) = &mut self.transfers {
            regions.suspended = true;
        }
        let mut spots = Vec::with_capacity(self.separations.batches());
        for batch in 0..self.separations.batches() {
            self.spot_batch = Some(batch);
            self.clip = None;
            let mut surface = self.surface(region, Backdrop::Transparent)?;
            self.commands(&list.commands, &mut surface, frame)?;
            spots.push(surface);
        }
        self.spot_batch = None;
        if let Some(regions) = &mut self.transfers {
            regions.suspended = false;
        }
        self.write_output(&page, &spots, model, output, tile)?;
        self.allocated -= page.bytes() + spots.iter().map(Surface::bytes).sum::<usize>();
        Ok(())
    }
}
