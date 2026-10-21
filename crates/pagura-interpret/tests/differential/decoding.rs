//! A scene recorder that decodes every image it records, with its masks,
//! as the old compiler did while compiling: decoding failures then fail the
//! interpretation where the old one failed, so outcomes compare.

use pagura_core::Result;
use pagura_document::Document;
use pagura_image::DecodeRequest;
use pagura_interpret::{
    Device, FillMark, FontInfo, GlyphRun, GroupInfo, Image, ImageMark, ImageMask, Marked, Needs,
    PageInfo, Scene, SceneRecorder, ShadingMark, SoftMaskInfo, StrokeMark, TilingInfo,
};

pub struct Decoding<'d> {
    recorder: SceneRecorder,
    document: &'d Document,
}

impl<'d> Decoding<'d> {
    pub fn new(document: &'d Document) -> Self {
        Self {
            recorder: SceneRecorder::new(document.limits()),
            document,
        }
    }

    pub fn finish(self) -> Result<Scene> {
        self.recorder.finish()
    }

    fn decode(&self, image: &Image) -> Result<()> {
        let source = &image.source;
        let decoded = source.decode(&DecodeRequest::FULL, &self.document.limits(), source.recovery())?;
        self.document.report(decoded.diagnostics);
        match &image.mask {
            ImageMask::Soft { mask, .. } | ImageMask::Stencil(mask) => self.decode(mask),
            _ => Ok(()),
        }
    }
}

impl Device for Decoding<'_> {
    fn needs(&self) -> Needs {
        self.recorder.needs()
    }
    fn begin_page(&mut self, page: &PageInfo) -> Result<()> {
        self.recorder.begin_page(page)
    }
    fn end_page(&mut self) -> Result<()> {
        self.recorder.end_page()
    }
    fn fill(&mut self, mark: &FillMark<'_>) -> Result<()> {
        self.recorder.fill(mark)
    }
    fn stroke(&mut self, mark: &StrokeMark<'_>) -> Result<()> {
        self.recorder.stroke(mark)
    }
    fn glyphs(&mut self, run: &GlyphRun<'_>) -> Result<()> {
        self.recorder.glyphs(run)
    }
    fn image(&mut self, mark: &ImageMark<'_>) -> Result<()> {
        self.decode(mark.image)?;
        self.recorder.image(mark)
    }
    fn shading(&mut self, mark: &ShadingMark<'_>) -> Result<()> {
        self.recorder.shading(mark)
    }
    fn begin_group(&mut self, group: &GroupInfo<'_>) -> Result<()> {
        self.recorder.begin_group(group)
    }
    fn end_group(&mut self) -> Result<()> {
        self.recorder.end_group()
    }
    fn begin_soft_mask(&mut self, mask: &SoftMaskInfo<'_>) -> Result<()> {
        self.recorder.begin_soft_mask(mask)
    }
    fn end_soft_mask(&mut self) -> Result<()> {
        self.recorder.end_soft_mask()
    }
    fn begin_tiling(&mut self, tiling: &TilingInfo) -> Result<()> {
        self.recorder.begin_tiling(tiling)
    }
    fn end_tiling(&mut self) -> Result<()> {
        self.recorder.end_tiling()
    }
    fn select_font(&mut self, font: &FontInfo) -> Result<()> {
        self.recorder.select_font(font)
    }
    fn marked(&mut self, event: &Marked<'_>) -> Result<()> {
        self.recorder.marked(event)
    }
}
