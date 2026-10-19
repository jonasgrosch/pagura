//! Tile rendering, viewport generations and supervisor cancellation.

use super::Session;
use super::documents::HANDLE;
use crate::protocol::{Response, layout};
use pagura::{Error, Position, RenderOptions, Result, Tile};

impl Session {
    /// Renders a tile; with a generation, a cancelled tile is skipped
    /// instead (1.1). Geometry and budgets are checked before compilation.
    pub(super) fn render(
        &mut self,
        document: u32,
        page: u32,
        scale: f64,
        tile: Tile,
        generation: Option<u32>,
    ) -> Result<Response> {
        // A generated tile's fields follow its generation word.
        let at = if generation.is_some() {
            HANDLE + 4
        } else {
            HANDLE
        };
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
        let (_, found) = self.documents.page(document, page, at)?;
        if !(scale.is_finite() && scale > 0.0) {
            return Err(Error::invalid_argument("scale must be positive and finite")
                .at(Position::Buffer(at + 8)));
        }
        let options = RenderOptions {
            scale,
            ..RenderOptions::default()
        };
        let (width, height) = pagura::device_size(found, &options)?;
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
                Error::invalid_argument("tile lies outside the page").at(Position::Buffer(at + 16))
            );
        }
        let cancellation = &self.cancellation;
        let cancelled = || generation.is_some() && cancellation.is_cancelled();
        // A list compiled for a cancelled tile is not admitted to the cache.
        let list = self
            .documents
            .compiled(document, page, at, || !cancelled())?;
        if let Some(generation) = generation.filter(|_| cancelled()) {
            return Ok(Response::Skipped(generation));
        }
        let pixmap = list.render_tile(&options, tile)?;
        if let Some(generation) = generation.filter(|_| cancelled()) {
            return Ok(Response::Skipped(generation));
        }
        Ok(Response::Tile {
            width: pixmap.width(),
            height: pixmap.height(),
            rgba: pixmap.to_straight_rgba(),
        })
    }

    /// A tile of the selected generation, or its skip status when a newer
    /// generation was selected or the supervisor cancelled it, decided
    /// before page lookup, compilation or painting.
    pub(super) fn generation_tile(
        &mut self,
        generation: u32,
        document: u32,
        page: u32,
        scale: f64,
        tile: Tile,
    ) -> Result<Response> {
        let current = self.documents.generation(document, HANDLE + 4)?;
        if generation < current {
            return Ok(Response::Skipped(generation));
        }
        if generation == 0 || generation != current {
            return Err(
                Error::invalid_argument("viewport generation was not selected")
                    .at(Position::Buffer(HANDLE)),
            );
        }
        if self.cancellation.is_cancelled() {
            return Ok(Response::Skipped(generation));
        }
        Ok(
            match self.render(document, page, scale, tile, Some(generation))? {
                Response::Tile {
                    width,
                    height,
                    rgba,
                } => Response::GenerationTile {
                    generation,
                    width,
                    height,
                    rgba,
                },
                response => response,
            },
        )
    }
}
