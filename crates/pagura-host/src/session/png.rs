//! Whole-page PNG output through the shared renderer and facade encoder.

use super::Session;
use crate::protocol::{PngBackground, PngRequest, layout};
use pagura::{Error, OverprintPreview, RenderOptions, Result, Rgba8, png};

impl Session {
    pub(super) fn page_png(&mut self, request: PngRequest) -> Result<Vec<u8>> {
        let scale = request.scale.get();
        if !(scale.is_finite() && scale > 0.0) {
            return Err(Error::invalid_argument("scale must be positive and finite")
                .at(request.scale.position()));
        }
        let options = RenderOptions {
            scale,
            background: match request.background.get() {
                PngBackground::White => Rgba8::WHITE,
                PngBackground::Transparent => Rgba8::TRANSPARENT,
            },
            overprint: OverprintPreview::Auto,
        };
        let (context, page) = self.documents.page(request.page)?;
        let mut limits = context.document().limits();
        let (width, height) = pagura::device_size(page, &options)?;
        let pixels = u64::from(width) * u64::from(height);
        // Reject oversized output surfaces before recording the page. The
        // renderer and encoder retain their ordinary per-operation limits.
        if pixels > u64::try_from(limits.pixels).unwrap_or(u64::MAX)
            || pixels > u64::try_from(limits.decoded_bytes / 4).unwrap_or(u64::MAX)
        {
            return Err(Error::limit("PNG page surface budget exceeded"));
        }
        // PNG encoding has a stream-byte budget; framing has its own cap.
        // Lowering only the output limit preserves each budget's meaning.
        limits.stream_bytes = limits
            .stream_bytes
            .min(self.max_frame_bytes().saturating_sub(layout::PNG_OVERHEAD));
        let scene = self.documents.scene(request.page)?;
        let (context, _) = self.documents.page(request.page)?;
        let surface = context.render_scene(&scene, &options, None)?;
        png::encode_pixmap(surface, png::Level::Default, &limits)
    }
}
