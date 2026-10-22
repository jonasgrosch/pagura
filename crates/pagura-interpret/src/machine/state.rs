//! The graphics state (8.4, Tables 51–52) and the shared values devices
//! receive: paints, composite states and pens, built once per change.

use crate::fonts::LoadedFont;
use crate::hash::FastMap;
use crate::model::{BlendMode, Clip, Color, Composite, MaskId, Overprint, Paint, Pen, Transfer};
use pagura_color::{ColorSpace, Components, Conversion, Function, Intent};
use pagura_raster::{Adjustment, Stroke, Transform};
use std::sync::Arc;

/// The colour of fills or of strokes: its space and the paint it makes.
/// Devices without paint keep none.
#[derive(Debug, Clone)]
pub(crate) struct Ink {
    pub(crate) space: Arc<ColorSpace>,
    pub(crate) paint: Arc<Paint>,
}

impl Ink {
    /// The initial colour of `space` (8.6.5–8.6.6), or no pattern for a
    /// Pattern space.
    pub(crate) fn initial(space: &Arc<ColorSpace>) -> Self {
        let components = if matches!(space.as_ref(), ColorSpace::Pattern(_)) {
            Components::from_slice(&[])
        } else {
            space.initial()
        };
        Self::solid(space, components)
    }

    /// A colour in `space`. In a Pattern space without a selected pattern
    /// the colour has no components and paints nothing.
    pub(crate) fn solid(space: &Arc<ColorSpace>, components: Components) -> Self {
        Self {
            space: Arc::clone(space),
            paint: Arc::new(Paint::Solid(Color {
                space: Arc::clone(space),
                components,
            })),
        }
    }
}

/// The most solid paints one interpretation keeps for reuse.
const MAX_SOLID_PAINTS: usize = 4096;

/// Solid paints by colour space and components (up to four), so content
/// that sets the same colours again shares one paint per colour instead of
/// building one per operator.
#[derive(Debug, Default)]
pub(crate) struct SolidPaints(FastMap<(usize, usize, [u64; 4]), Arc<Paint>>);

impl SolidPaints {
    /// The colour `values` in `space`, as [`Ink::solid`] builds it.
    pub(crate) fn ink(&mut self, space: &Arc<ColorSpace>, values: &[f64]) -> Ink {
        if values.len() > 4 {
            return Ink::solid(space, Components::from_slice(values));
        }
        let mut bits = [0; 4];
        for (bits, value) in bits.iter_mut().zip(values) {
            *bits = value.to_bits();
        }
        // A cached paint holds its space, so the address names one space.
        let key = (Arc::as_ptr(space).addr(), values.len(), bits);
        if let Some(paint) = self.0.get(&key) {
            return Ink {
                space: Arc::clone(space),
                paint: Arc::clone(paint),
            };
        }
        let ink = Ink::solid(space, Components::from_slice(values));
        if self.0.len() < MAX_SOLID_PAINTS {
            self.0.insert(key, Arc::clone(&ink.paint));
        }
        ink
    }
}

/// Text state parameters (9.3, Table 102).
#[derive(Debug, Clone)]
pub(crate) struct TextParams {
    pub(crate) font: Option<Arc<LoadedFont>>,
    pub(crate) size: f64,
    pub(crate) character: f64,
    pub(crate) word: f64,
    /// `Tz` / 100.
    pub(crate) horizontal: f64,
    pub(crate) leading: f64,
    pub(crate) rise: f64,
    pub(crate) mode: u8,
    pub(crate) knockout: bool,
}

impl Default for TextParams {
    fn default() -> Self {
        Self {
            font: None,
            size: 0.0,
            character: 0.0,
            word: 0.0,
            horizontal: 1.0,
            leading: 0.0,
            rise: 0.0,
            mode: 0,
            knockout: true,
        }
    }
}

/// The values built from the state, dropped when a parameter they depend on
/// changes.
#[derive(Debug, Clone, Default)]
struct Built {
    fill: Option<Arc<Composite>>,
    stroke: Option<Arc<Composite>>,
    pen: Option<Arc<Pen>>,
}

/// The graphics state that `q` saves and `Q` restores.
#[derive(Debug, Clone)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "independent flags of Tables 51 and 52"
)]
pub(crate) struct Graphics {
    pub(crate) ctm: Transform,
    pub(crate) fill: Option<Ink>,
    pub(crate) stroke_ink: Option<Ink>,
    pub(crate) stroke: Stroke,
    pub(crate) adjustment: bool,
    pub(crate) clip: Option<Arc<Clip>>,
    pub(crate) clip_depth: usize,
    pub(crate) intent: Intent,
    pub(crate) flatness: f64,
    pub(crate) transfer: Option<Arc<Transfer>>,
    pub(crate) blend: BlendMode,
    pub(crate) fill_alpha: f64,
    pub(crate) stroke_alpha: f64,
    pub(crate) soft_mask: Option<MaskId>,
    pub(crate) alpha_is_shape: bool,
    pub(crate) fill_overprint: bool,
    pub(crate) stroke_overprint: bool,
    pub(crate) nonzero_overprint: bool,
    pub(crate) black_generation: Option<Arc<Function>>,
    pub(crate) undercolor_removal: Option<Arc<Function>>,
    pub(crate) text: TextParams,
    built: Built,
}

impl Default for Graphics {
    fn default() -> Self {
        Self {
            ctm: Transform::IDENTITY,
            fill: None,
            stroke_ink: None,
            stroke: Stroke::default(),
            adjustment: false,
            clip: None,
            clip_depth: 0,
            intent: Intent::Relative,
            flatness: 1.0,
            transfer: None,
            blend: BlendMode::Normal,
            fill_alpha: 1.0,
            stroke_alpha: 1.0,
            soft_mask: None,
            alpha_is_shape: false,
            fill_overprint: false,
            stroke_overprint: false,
            nonzero_overprint: false,
            black_generation: None,
            undercolor_removal: None,
            text: TextParams::default(),
            built: Built::default(),
        }
    }
}

impl Graphics {
    /// Drops the composite states built from the state; call after changing
    /// any compositing parameter.
    pub(crate) fn composite_changed(&mut self) {
        self.built.fill = None;
        self.built.stroke = None;
    }

    /// Drops the pen; call after changing a line parameter or the CTM.
    pub(crate) fn pen_changed(&mut self) {
        self.built.pen = None;
    }

    pub(crate) fn set_ctm(&mut self, ctm: Transform) {
        self.ctm = ctm;
        self.pen_changed();
    }

    /// Sets both inks to the initial colour of `space`.
    pub(crate) fn set_both_inks(&mut self, space: &Arc<ColorSpace>) {
        let ink = Ink::initial(space);
        self.fill = Some(ink.clone());
        self.stroke_ink = Some(ink);
    }

    /// The composite state of fills, glyphs, images and shadings.
    pub(crate) fn fill_composite(&mut self) -> Arc<Composite> {
        if let Some(built) = &self.built.fill {
            return Arc::clone(built);
        }
        let built = Arc::new(self.composite(self.fill_alpha, self.fill_overprint));
        self.built.fill = Some(Arc::clone(&built));
        built
    }

    /// The composite state of strokes.
    pub(crate) fn stroke_composite(&mut self) -> Arc<Composite> {
        if let Some(built) = &self.built.stroke {
            return Arc::clone(built);
        }
        let built = Arc::new(self.composite(self.stroke_alpha, self.stroke_overprint));
        self.built.stroke = Some(Arc::clone(&built));
        built
    }

    fn composite(&self, alpha: f64, overprint: bool) -> Composite {
        let mut conversion = Conversion::default();
        conversion.intent = self.intent;
        conversion
            .black_generation
            .clone_from(&self.black_generation);
        conversion
            .undercolor_removal
            .clone_from(&self.undercolor_removal);
        Composite {
            alpha,
            blend: self.blend,
            soft_mask: self.soft_mask,
            alpha_is_shape: self.alpha_is_shape,
            overprint: Overprint {
                enabled: overprint,
                nonzero: self.nonzero_overprint,
            },
            transfer: self.transfer.clone(),
            conversion,
        }
    }

    /// The pen of strokes: the line parameters under the CTM.
    pub(crate) fn pen(&mut self) -> Arc<Pen> {
        if let Some(pen) = &self.built.pen {
            return Arc::clone(pen);
        }
        let pen = Arc::new(Pen {
            stroke: self.stroke.clone(),
            transform: self.ctm,
            adjustment: if self.adjustment {
                Adjustment::Adjusted
            } else {
                Adjustment::None
            },
        });
        self.built.pen = Some(Arc::clone(&pen));
        pen
    }

    /// Resets every compositing parameter to its initial value, as tiling
    /// cells start (8.7.3.1); the transfer function is kept.
    pub(crate) fn reset_composite(&mut self) {
        self.start_group();
        self.alpha_is_shape = false;
        self.fill_overprint = false;
        self.stroke_overprint = false;
        self.nonzero_overprint = false;
        self.black_generation = None;
        self.undercolor_removal = None;
    }

    /// Resets the compositing parameters a transparency group's content
    /// starts from (11.6.6): opaque, Normal and unmasked.
    pub(crate) fn start_group(&mut self) {
        self.fill_alpha = 1.0;
        self.stroke_alpha = 1.0;
        self.blend = BlendMode::Normal;
        self.soft_mask = None;
        self.composite_changed();
    }
}
