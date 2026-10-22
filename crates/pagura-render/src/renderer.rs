//! Interpretation and rendering of one document's pages through one store:
//! the steps the free functions and [`crate::RenderContext`] share.

use crate::options::{RenderOptions, Tile};
use pagura_core::{Error, Limits, ObjectId, Rect, Result, Store};
use pagura_document::forms::FormFiller;
use pagura_document::{Document, Page};
use pagura_extract::{LayoutOptions, PositionedText, TextPage};
use pagura_interpret::{Interpreter, Scene};
use pagura_paint::{Completion, PreparedScene, RenderRequest, Scratch};
use pagura_raster::{Pixmap, Rgba8};
use pagura_text::Provider;

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
        stop: impl FnMut(u32) -> bool,
    ) -> Result<(Pixmap, Completion)> {
        let prepared = self.prepare(scene, (options, tile), scratch)?;
        let rendered = self.draw(&prepared, options.background, scratch, stop);
        scratch.recycle(prepared);
        rendered
    }

    /// Renders `prepared` over `background` in `scratch`.
    fn draw(
        self,
        prepared: &PreparedScene,
        background: Rgba8,
        scratch: &mut Scratch,
        stop: impl FnMut(u32) -> bool,
    ) -> Result<(Pixmap, Completion)> {
        let mut rendering = prepared.with_scratch(scratch)?;
        let region = prepared.region();
        let (width, height) = (region.width(), region.height());
        let limits = self.document.limits();
        let Some(bytes) = surface_bytes(width, height, limits) else {
            // An empty surface or one over the pixel budget is refused as
            // allocation refuses it; one over only the byte budget, which an
            // allocation taken over must keep to, is allocated as before.
            let mut pixmap = Pixmap::new(width, height, background, limits.pixels)?;
            let completion = rendering.render_until(&mut pixmap, stop);
            return Ok((pixmap, completion));
        };
        // Rendered rows are written whole, so the surface is collected band
        // by band rather than filled with the background first; rows a stop
        // leaves get the background.
        let mut data = Vec::new();
        data.try_reserve_exact(bytes)
            .map_err(|_| Error::limit("surface allocation failed"))?;
        let completion = rendering.render_rows_until(&mut data, stop);
        let rendered = data.len().min(bytes);
        data.resize(bytes, 0);
        if let Some(rest) = data.get_mut(rendered..) {
            rest.as_chunks_mut::<4>().0.fill(prepared.background());
        }
        let pixmap = Pixmap::from_premultiplied(width, height, data, limits)?;
        Ok((pixmap, completion))
    }

    /// Prepares `scene` for one region in the storage of `scratch` and
    /// reports the repairs preparation made to the document, where strict
    /// recovery refuses them.
    fn prepare(
        self,
        scene: &Scene,
        (options, tile): (&RenderOptions, Option<Tile>),
        scratch: &mut Scratch,
    ) -> Result<PreparedScene> {
        let mut request = RenderRequest::page(&scene.page, options.scale)?;
        if let Some(tile) = tile {
            request.region = tile.region((request.page.width(), request.page.height()))?;
        }
        request.background = options.background;
        request.overprint = options.overprint.resolve(scene.print_intent);
        request.fast = options.fast;
        let limits = self.document.limits();
        let prepared = scratch.prepare(scene, &request, (self.store, &limits))?;
        for repair in prepared.repairs() {
            self.document.repair(repair.clone())?;
        }
        Ok(prepared)
    }
}

/// The bytes of a `width × height` surface when it is within both the pixel
/// and the byte budget.
fn surface_bytes(width: u32, height: u32, limits: Limits) -> Option<usize> {
    let pixels = usize::try_from(u64::from(width) * u64::from(height)).ok()?;
    let bytes = pixels.checked_mul(4)?;
    (pixels > 0 && pixels <= limits.pixels && bytes <= limits.decoded_bytes).then_some(bytes)
}
