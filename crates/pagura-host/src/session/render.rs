//! Tile rendering, viewport generations and supervisor cancellation.

use super::Session;
use crate::protocol::{Field, Pixels, Response, TileRequest, layout};
use pagura::{Error, RenderOptions, Result};

impl Session {
    /// Renders a tile; with a generation, a cancelled tile is skipped
    /// instead (1.1). Geometry and budgets are checked before compilation.
    pub(super) fn render(
        &mut self,
        request: TileRequest,
        generation: Option<u32>,
    ) -> Result<Response> {
        let tile = request.tile.get();
        // Bound response storage before rendering or allocating straight pixels.
        let overhead = if generation.is_some() {
            layout::GENERATION_TILE_OVERHEAD
        } else {
            layout::TILE_OVERHEAD
        };
        let pixels = u64::from(tile.width) * u64::from(tile.height);
        let frame = pixels
            .checked_mul(4)
            .and_then(|bytes| bytes.checked_add(u64::try_from(overhead).unwrap_or(u64::MAX)));
        let max_frame = u64::try_from(self.max_frame_bytes()).unwrap_or(u64::MAX);
        if pixels > u64::try_from(self.config.engine.pixels).unwrap_or(u64::MAX)
            || frame.is_none_or(|bytes| bytes > max_frame)
        {
            return Err(Error::limit("tile response budget exceeded"));
        }
        let (_, page) = self.documents.page(request.page)?;
        let scale = request.scale.get();
        if !(scale.is_finite() && scale > 0.0) {
            return Err(Error::invalid_argument("scale must be positive and finite")
                .at(request.scale.position()));
        }
        let options = RenderOptions {
            scale,
            ..RenderOptions::default()
        };
        let (width, height) = pagura::device_size(page, &options)?;
        if tile.width == 0
            || tile.height == 0
            || tile
                .x
                .checked_add(tile.width)
                .is_none_or(|right| right > width)
            || tile
                .y
                .checked_add(tile.height)
                .is_none_or(|bottom| bottom > height)
        {
            return Err(
                Error::invalid_argument("tile lies outside the page").at(request.tile.position())
            );
        }
        let cancellation = &self.cancellation;
        let cancelled = || generation.is_some() && cancellation.is_cancelled();
        // A list compiled for a cancelled tile is not admitted to the cache.
        let list = self.documents.compiled(request.page, || !cancelled())?;
        if let Some(generation) = generation.filter(|_| cancelled()) {
            return Ok(Response::Skipped(generation));
        }
        let pixmap = list.render_tile(&options, tile)?;
        let pixels = Pixels::new(pixmap.width(), pixmap.height(), pixmap.to_straight_rgba());
        Ok(match generation {
            Some(generation) if cancelled() => Response::Skipped(generation),
            Some(generation) => Response::GenerationTile { generation, pixels },
            None => Response::Tile(pixels),
        })
    }

    /// A tile of the selected generation, or its skip status when a newer
    /// generation was selected or the supervisor cancelled it, decided
    /// before page lookup, compilation or painting.
    pub(super) fn generation_tile(
        &mut self,
        generation: Field<u32>,
        request: TileRequest,
    ) -> Result<Response> {
        let current = self.documents.generation(request.page.document)?;
        let requested = generation.get();
        if requested < current {
            return Ok(Response::Skipped(requested));
        }
        if requested == 0 || requested != current {
            return Err(
                Error::invalid_argument("viewport generation was not selected")
                    .at(generation.position()),
            );
        }
        if self.cancellation.is_cancelled() {
            return Ok(Response::Skipped(requested));
        }
        self.render(request, Some(requested))
    }
}
