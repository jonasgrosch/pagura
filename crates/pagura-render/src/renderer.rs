//! Interpretation and rendering of one document's pages through one store:
//! the steps the free functions and [`crate::RenderContext`] share.

use crate::options::{RenderOptions, Tile};
use pagura_core::{Error, Limits, Meter, ObjectId, Rect, Result, Store};
use pagura_document::forms::FormFiller;
use pagura_document::{Document, Page};
use pagura_extract::{LayoutOptions, PositionedText, TextPage};
use pagura_interpret::{Interpreter, Scene};
use pagura_paint::{Completion, PreparedScene, RenderRequest, Scratch};
use pagura_raster::Pixmap;
use pagura_text::Provider;
use std::ops::Range;

/// A document, its font provider and the store that holds what rendering
/// its pages reuses.
#[derive(Clone, Copy)]
pub(crate) struct Renderer<'a> {
    pub(crate) document: &'a Document,
    pub(crate) provider: Option<&'a dyn Provider>,
    pub(crate) store: &'a Store,
}

impl<'a> Renderer<'a> {
    fn interpreter(self) -> Interpreter<'a> {
        Interpreter::new(self.document)
            .with_provider(self.provider)
            .with_store(self.store)
    }

    /// Records `page` with stored appearances.
    pub(crate) fn record(self, page: &Page) -> Result<Scene> {
        self.interpreter().scene(page)
    }

    /// Records `page` with the in-memory form values of `filler`.
    pub(crate) fn record_filled(self, filler: &FormFiller<'_>, page: &Page) -> Result<Scene> {
        if !std::ptr::eq(filler.document(), self.document) {
            return Err(Error::invalid_argument(
                "form does not belong to the rendering document",
            ));
        }
        self.interpreter().with_filler(filler).scene(page)
    }

    /// The positioned text of `page` through the text device, laid out in
    /// reading order with its logical structure (ADR 0014).
    pub(crate) fn text_page(self, page: &Page) -> Result<TextPage> {
        let text = self.interpreter().text(page)?;
        TextPage::from_positioned(text, LayoutOptions::default(), self.document.limits())?
            .with_structure(self.document, page)
    }

    /// The positioned text of one stored annotation appearance of `page`,
    /// fitted into `rect`.
    pub(crate) fn appearance_text(
        self,
        page: &Page,
        appearance: ObjectId,
        rect: Rect,
    ) -> Result<PositionedText> {
        self.interpreter().appearance_text(page, appearance, rect)
    }

    /// Renders `tile` of `scene`, or the whole page, in `scratch`, asking
    /// `stop` with the first device row of each band before rendering it.
    pub(crate) fn render(
        self,
        scene: &Scene,
        (options, tile): (&RenderOptions, Option<Tile>),
        scratch: &mut Scratch,
        stop: impl FnMut(u32) -> bool + Send,
    ) -> Result<(Pixmap, Completion)> {
        let prepared = self.prepare(scene, (options, tile))?;
        self.draw(&prepared, scratch, stop)
    }

    /// Prepares the whole region before lending any completed output band
    /// to the consumer. Output storage is bounded by one band, rather than
    /// an owned full surface, and belongs to this call alone.
    pub(crate) fn render_bands<E>(
        self,
        scene: &Scene,
        (options, tile): (&RenderOptions, Option<Tile>),
        scratch: &mut Scratch,
        stop: impl FnMut(u32) -> bool,
        consume: impl FnMut(Range<u32>, &[u8]) -> std::result::Result<(), E>,
    ) -> Result<std::result::Result<Completion, E>> {
        let prepared = self.prepare(scene, (options, tile))?;
        let mut rendering = prepared.with_scratch(scratch)?;
        let mut band = Vec::new();
        let limits = self.document.limits();
        let mut output = Meter::new(limits.decoded_bytes, "output band byte budget exceeded");
        output.reserve_capacity(&mut band, rendering.band_bytes()?)?;
        if band.capacity() > limits.decoded_bytes {
            return Err(Error::limit("output band allocation exceeds byte budget"));
        }
        Ok(rendering.render_bands(&mut band, stop, consume))
    }

    /// Renders `prepared` in `scratch`.
    fn draw(
        self,
        prepared: &PreparedScene,
        scratch: &mut Scratch,
        stop: impl FnMut(u32) -> bool + Send,
    ) -> Result<(Pixmap, Completion)> {
        let region = prepared.region();
        let (width, height) = (region.width(), region.height());
        let limits = self.document.limits();
        let bytes = surface_bytes(width, height, limits)?;
        let mut rendering = prepared.with_scratch(scratch)?;
        // Rendered rows are written whole, so the surface is collected band
        // by band rather than filled with the background first; rows a stop
        // leaves get the background.
        let mut data = Vec::new();
        data.try_reserve_exact(bytes)
            .map_err(|_| Error::limit("surface allocation failed"))?;
        let completion = rendering.render_until(&mut data, stop);
        let rendered = data.len().min(bytes);
        data.resize(bytes, 0);
        if let Some(rest) = data.get_mut(rendered..) {
            rest.as_chunks_mut::<4>().0.fill(prepared.background());
        }
        let pixmap = Pixmap::from_premultiplied(width, height, data, limits)?;
        Ok((pixmap, completion))
    }

    /// Prepares `scene` for one region and reports its repairs to the
    /// document, where strict recovery refuses them.
    fn prepare(
        self,
        scene: &Scene,
        (options, tile): (&RenderOptions, Option<Tile>),
    ) -> Result<PreparedScene> {
        let mut request = RenderRequest::page(&scene.page, options.scale)?;
        if let Some(tile) = tile {
            request.region = tile.region((request.page.width(), request.page.height()))?;
        }
        request.background = options.background;
        request.overprint = options.overprint.resolve(scene.print_intent);
        let limits = self.document.limits();
        let prepared = pagura_paint::prepare(scene, &request, self.store, &limits)?;
        for repair in prepared.repairs() {
            self.document.repair(repair.clone())?;
        }
        Ok(prepared)
    }
}

/// The bytes of a `width × height` surface, which must be within both the
/// pixel and the byte budget, as [`Pixmap::from_premultiplied`] requires.
fn surface_bytes(width: u32, height: u32, limits: Limits) -> Result<usize> {
    let pixels = usize::try_from(u64::from(width) * u64::from(height))
        .ok()
        .filter(|&pixels| pixels <= limits.pixels)
        .ok_or(Error::limit("surface pixel budget exceeded"))?;
    pixels
        .checked_mul(4)
        .filter(|&bytes| bytes <= limits.decoded_bytes)
        .ok_or(Error::limit("surface byte budget exceeded"))
}
