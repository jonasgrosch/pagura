//! Interpretation and rendering of one document's pages through one store:
//! the steps the free functions and [`crate::RenderContext`] share.

use crate::options::{RenderOptions, Tile};
use pagura_core::{Error, ObjectId, Rect, Result, Store};
use pagura_document::forms::FormFiller;
use pagura_document::{Document, Page};
use pagura_extract::{LayoutOptions, PositionedText, TextPage};
use pagura_interpret::{Interpreter, Scene};
use pagura_paint::{Completion, PreparedScene, RenderRequest};
use pagura_raster::{DeviceRect, Pixmap};
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

    /// Renders `tile` of `scene`, or the whole page, asking `stop` with the
    /// first device row of each band before rendering it.
    pub(crate) fn render(
        self,
        scene: &Scene,
        options: &RenderOptions,
        tile: Option<Tile>,
        stop: impl FnMut(u32) -> bool,
    ) -> Result<(Pixmap, Completion)> {
        let mut prepared = self.prepare(scene, options, tile)?;
        let region = prepared.region();
        let mut pixmap = Pixmap::new_with_limits(
            region.width(),
            region.height(),
            options.background,
            self.document.limits(),
        )?;
        let completion = prepared.render_until(&mut pixmap, stop);
        Ok((pixmap, completion))
    }

    /// Prepares `scene` for one region and reports the repairs preparation
    /// made to the document, where strict recovery refuses them.
    fn prepare(
        self,
        scene: &Scene,
        options: &RenderOptions,
        tile: Option<Tile>,
    ) -> Result<PreparedScene> {
        let size = scene.page.device_size(options.scale)?;
        let page = DeviceRect {
            x0: 0,
            y0: 0,
            x1: size.0,
            y1: size.1,
        };
        let region = match tile {
            Some(tile) => tile.region(size)?,
            None => page,
        };
        let mut request =
            RenderRequest::new(scene.page.device_transform(options.scale), page, region);
        request.background = options.background;
        request.overprint = options.overprint.resolve(scene.print_intent);
        request.fast = options.fast;
        let prepared = pagura_paint::prepare(scene, &request, self.store, &self.document.limits())?;
        for repair in prepared.repairs() {
            self.document.repair(repair.clone())?;
        }
        Ok(prepared)
    }
}
