//! Preparation, the only fallible step of rendering, and execution.
//!
//! [`prepare`] walks the scene in source order: it culls marks by
//! conservative device bounds against the region, validates them, reserves
//! `render_bytes`, `render_work` and `function_work`, builds coverage
//! sources and prepared paint, and bins items to bands. Storage formats come
//! from a survey of the whole content beforehand, so they never depend on
//! the region. The first error in source order among the marks that meet
//! the region is the result; it does not depend on band height.

pub(crate) mod bounds;
mod finish;
mod overprint;
pub(crate) mod planner;
mod shapes;
pub(crate) mod stacks;
mod storage;

pub(crate) use shapes::{Prepared, ShapeBuilder, Shapes};

use crate::execute::{Rendering, Scratch};
use crate::plan::{MAX_BAND_HEIGHT, Plan};
use crate::request::RenderRequest;
use pagura_core::{Diagnostic, Error, Limits, Meter, Result, Store};
use pagura_interpret::Scene;
use pagura_raster::{Area, DeviceRect};

/// A scene prepared for one request: every mark that meets the region
/// culled, validated, reserved and binned to bands, with its coverage
/// source and prepared paint, and every buffer's storage format chosen.
///
/// It holds no storage of rendering: a render takes that from a caller's
/// [`Scratch`] ([`Self::with_scratch`]), so the scene is shared, immutable,
/// and rendered any number of times.
#[derive(Debug)]
pub struct PreparedScene {
    plan: Plan,
    shapes: Shapes,
    repairs: Vec<Diagnostic>,
    charges: Charges,
}

/// How far a render got.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Completion {
    /// Every band was rendered.
    Complete,
    /// Rendering stopped between bands; rows from `region.y0` up to this
    /// row are rendered.
    Stopped {
        /// The first row not rendered.
        row: u32,
    },
}

/// The budgets a preparation reserved (ADR 0016): upper bounds of what
/// rendering it uses. For one region they do not depend on band height or
/// on the state of the `Store`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[non_exhaustive]
pub struct Charges {
    render_bytes: usize,
    render_work: usize,
    function_work: usize,
}

impl Charges {
    /// Bytes reserved against `Limits::render_bytes`.
    pub fn render_bytes(&self) -> usize {
        self.render_bytes
    }

    /// Work units reserved against `Limits::render_work`.
    pub fn render_work(&self) -> usize {
        self.render_work
    }

    /// Function evaluation units reserved against `Limits::function_work`.
    pub fn function_work(&self) -> usize {
        self.function_work
    }
}

/// The meters of one preparation.
#[derive(Debug)]
pub(crate) struct Budget {
    /// `render_bytes`.
    pub(crate) bytes: Meter,
    /// `render_work`.
    pub(crate) work: Meter,
    /// `function_work`.
    pub(crate) functions: Meter,
}

impl Budget {
    fn new(limits: &Limits) -> Self {
        Self {
            bytes: Meter::new(limits.render_bytes, "render memory budget exceeded"),
            work: Meter::new(limits.render_work, "render work budget exceeded"),
            functions: Meter::new(limits.function_work, "function work budget exceeded"),
        }
    }

    fn charges(&self) -> Charges {
        Charges {
            render_bytes: self.bytes.used(),
            render_work: self.work.used(),
            function_work: self.functions.used(),
        }
    }
}

/// Prepares `scene` for `request`, using `store` for glyph edge setups,
/// decoded images, prepared functions and colour transforms.
///
/// # Errors
///
/// Returns `InvalidArgument` for an empty region, a region outside the page
/// or a zero band height, and otherwise the first error in source order
/// among the marks that meet the region: `Syntax` and `Unsupported` for
/// content that cannot be painted, and `Limit` errors for `render_bytes`,
/// `render_work`, `function_work` and the per-fill budgets. Errors do not
/// depend on band height.
pub fn prepare(
    scene: &Scene,
    request: &RenderRequest,
    store: &Store,
    limits: &Limits,
) -> Result<PreparedScene> {
    let area = area(request, limits)?;
    let mut budget = Budget::new(limits);
    let shapes = ShapeBuilder::new(store, limits);
    let planner =
        planner::Planner::new(scene, request, (store, limits), area, shapes, &mut budget)?;
    let finished = finish::finish(planner, &mut budget)?;
    Ok(PreparedScene {
        plan: finished.plan,
        shapes: finished.shapes,
        repairs: finished.repairs,
        charges: budget.charges(),
    })
}

/// The region and band grid of `request`: bands of the requested height
/// (at most [`MAX_BAND_HEIGHT`] rows) on the global grid, or one band over
/// the whole region when it is no taller than a band (immediate mode).
fn area(request: &RenderRequest, limits: &Limits) -> Result<Area> {
    let region = request.region;
    let page = request.page;
    if request.band_height == 0 || region.is_empty() {
        return Err(Error::invalid_argument("empty region or band"));
    }
    if region.x0 < page.x0 || region.y0 < page.y0 || region.x1 > page.x1 || region.y1 > page.y1 {
        return Err(Error::invalid_argument("region outside the page"));
    }
    let pixels = u64::from(region.width()) * u64::from(region.height());
    if usize::try_from(pixels).map_or(true, |pixels| pixels > limits.pixels) {
        return Err(Error::limit("render pixel budget exceeded"));
    }
    let height = request.band_height.min(MAX_BAND_HEIGHT);
    let band_height = if region.height() <= height {
        region.y1
    } else {
        height
    };
    Ok(Area {
        region,
        band_height,
    })
}

#[cfg(test)]
mod tests;

/// A `u32` table index as `usize`.
pub(crate) fn index(value: u32) -> usize {
    pagura_core::convert::u32_to_usize(value)
}

impl PreparedScene {
    /// The rendered region.
    pub fn region(&self) -> DeviceRect {
        self.plan.area.region
    }

    /// The budgets preparation reserved.
    pub fn charges(&self) -> Charges {
        self.charges
    }

    /// The repairs preparation made (image decoding), in source order, for
    /// the caller to report through its `Diagnostics`.
    pub fn repairs(&self) -> &[Diagnostic] {
        &self.repairs
    }

    /// The background as the render stores it, premultiplied.
    pub fn background(&self) -> [u8; 4] {
        self.plan.output.background
    }

    /// Renders of this scene in `scratch`, sized for it: the one step of
    /// rendering that allocates.
    ///
    /// # Errors
    ///
    /// A `Limit` error when the storage cannot be allocated.
    pub fn with_scratch<'a>(&'a self, scratch: &'a mut Scratch) -> Result<Rendering<'a>> {
        let (workspace, rasterizer) = scratch.fit(&self.plan)?;
        Ok(Rendering {
            plan: &self.plan,
            shapes: &self.shapes,
            workspace,
            rasterizer,
        })
    }
}
