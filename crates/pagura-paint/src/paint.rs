//! The seams between preparation, the executor and prepared values: colours
//! of spans, unit curves (transfer, soft-mask and black-generation
//! functions) and colour conversions. Each is implemented for the prepared
//! values of `crate::source` and `pagura-color`, and is called once per
//! span or pixel group, never matched per pixel.

use crate::source::{ImageSampler, Source, SpanColors};
use pagura_color::{Converter, Function, MAX_COMPONENTS, Painted};
use std::fmt;

/// Prepared paint as the executor reads it.
pub(crate) trait PaintSpans: Send + Sync + fmt::Debug {
    /// The colour of every pixel, when it is one opaque colour.
    fn constant(&self) -> Option<&[f64]>;

    /// The channels a constant colour paints, for overprint.
    fn painted(&self) -> Painted;

    /// Fills `out` with the straight colours and alpha of device pixels
    /// `x .. x + len` of row `y`. A pixel's alpha is also its shape.
    fn span(&self, x: u32, y: u32, len: usize, out: &mut SpanColors);

    /// As [`Self::span`] for a shading pattern whose degenerate `BBox`
    /// outline covers `boundary` of each pixel
    /// ([`BBoxUse::Boundary`](crate::source::BBoxUse::Boundary)).
    fn span_with_boundary(&self, x: u32, y: u32, boundary: &[u8], out: &mut SpanColors) {
        self.span(x, y, boundary.len(), out);
    }

    /// Render work units one pixel costs besides compositing (mesh
    /// geometry tests).
    fn work_per_pixel(&self) -> usize {
        0
    }

    /// Bytes the paint holds besides its own value.
    fn bytes(&self) -> usize {
        0
    }
}

impl PaintSpans for Source {
    fn constant(&self) -> Option<&[f64]> {
        Source::constant(self)
    }

    fn painted(&self) -> Painted {
        match self {
            Self::Solid { painted, .. } => *painted,
            Self::Shading(_) | Self::Image(_) => Painted::default(),
        }
    }

    fn span(&self, x: u32, y: u32, len: usize, out: &mut SpanColors) {
        Source::span(self, x, y, len, out);
    }

    fn span_with_boundary(&self, x: u32, y: u32, boundary: &[u8], out: &mut SpanColors) {
        match self {
            Self::Shading(shading) => shading.span_with_boundary(x, y, boundary, out),
            Self::Solid { .. } | Self::Image(_) => self.span(x, y, boundary.len(), out),
        }
    }

    fn work_per_pixel(&self) -> usize {
        match self {
            Self::Shading(shading) => shading.geometry_work(),
            Self::Solid { .. } | Self::Image(_) => 0,
        }
    }

    fn bytes(&self) -> usize {
        match self {
            Self::Solid { color, .. } => size_of_val::<[f64]>(color),
            Self::Shading(shading) => shading.bytes(),
            Self::Image(_) => 0,
        }
    }
}

/// A stencil mask (8.9.6.2): alpha from the mask's samples, colour from the
/// fill paint.
#[derive(Debug)]
pub(crate) struct StencilPaint {
    /// The fill paint.
    pub(crate) color: Box<dyn PaintSpans>,
    /// The mask's samples.
    pub(crate) mask: ImageSampler,
}

impl PaintSpans for StencilPaint {
    fn constant(&self) -> Option<&[f64]> {
        None
    }

    fn painted(&self) -> Painted {
        self.color.painted()
    }

    fn span(&self, x: u32, y: u32, len: usize, out: &mut SpanColors) {
        self.mask.span(x, y, len, out);
        let alpha = std::mem::take(&mut out.alpha);
        self.color.span(x, y, len, out);
        out.alpha = alpha;
    }

    fn work_per_pixel(&self) -> usize {
        self.color.work_per_pixel()
    }

    fn bytes(&self) -> usize {
        self.color.bytes()
    }
}

/// A function of one input whose first output is used: a transfer,
/// soft-mask transfer, black-generation or undercolour-removal function.
pub(crate) trait Curve: Send + Sync + fmt::Debug {
    /// The first output at `x`.
    fn map(&self, x: f64) -> f64;
    /// The `function_work` units one evaluation costs.
    fn cost(&self) -> usize;
}

impl Curve for Function {
    fn map(&self, x: f64) -> f64 {
        let mut output = [0.0; MAX_COMPONENTS];
        let count = self.outputs().clamp(1, MAX_COMPONENTS);
        self.evaluate(&[x], &mut output[..count]);
        output[0]
    }

    fn cost(&self) -> usize {
        Function::cost(self)
    }
}

/// A prepared colour conversion into a process target, for the spot
/// equivalents of overprint preview.
pub(crate) trait Convert: Send + Sync + fmt::Debug {
    /// Converts `components` into `output` (additive coordinates).
    fn convert(&self, components: &[f64], output: &mut [f64]) -> Painted;
    /// The `function_work` units one conversion costs.
    fn cost(&self) -> usize;
}

impl Convert for Converter {
    fn convert(&self, components: &[f64], output: &mut [f64]) -> Painted {
        Converter::convert(self, components, output)
    }

    fn cost(&self) -> usize {
        Converter::cost(self)
    }
}
