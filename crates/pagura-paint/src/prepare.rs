//! Preparation, the only fallible step of rendering, and execution.
//!
//! [`prepare`] walks the scene in source order: it culls marks by
//! conservative device bounds against the region, validates them, reserves
//! `render_bytes`, `render_work` and `function_work`, builds coverage
//! sources and prepared paint, and bins items to bands. Storage formats come
//! from a survey of the whole content beforehand, so they never depend on
//! the region. The first error in source order among the marks that meet
//! the region is the result; it does not depend on band height.

mod backend;
pub(crate) mod bounds;
mod finish;
mod overprint;
pub(crate) mod planner;
mod storage;

pub(crate) use backend::{Backend, PaintRequest, Prepared, Producer, Raster, Storage};

use crate::execute::{Destination, Executing, Executor, Scratch, Workspace};
use crate::plan::{MAX_BAND_HEIGHT, Plan};
use crate::request::RenderRequest;
use pagura_core::{Diagnostic, Error, Limits, Meter, Result, Store};
use pagura_interpret::Scene;
use pagura_raster::{Area, BandRasterizer, DeviceRect, Pixmap};

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
    producer: Box<dyn Producer>,
    repairs: Vec<Diagnostic>,
    charges: Charges,
}

/// A prepared scene with scratch storage sized for it. Rendering cannot
/// fail.
#[derive(Debug)]
pub struct Rendering<'a> {
    plan: &'a Plan,
    producer: &'a dyn Producer,
    workspace: &'a mut Workspace,
    rasterizer: &'a mut BandRasterizer,
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
    prepare_in(scene, request, (store, limits), Storage::default())
}

/// [`prepare`] in the storage of an earlier prepared scene.
pub(crate) fn prepare_in(
    scene: &Scene,
    request: &RenderRequest,
    (store, limits): (&Store, &Limits),
    storage: Storage,
) -> Result<PreparedScene> {
    prepare_with(
        scene,
        request,
        (store, limits),
        Raster::new(store, limits).with_storage(storage),
    )
}

/// [`prepare`] with the geometry and paint of `backend`.
pub(crate) fn prepare_with<B: Backend>(
    scene: &Scene,
    request: &RenderRequest,
    (store, limits): (&Store, &Limits),
    backend: B,
) -> Result<PreparedScene> {
    let area = area(request, limits)?;
    let mut budget = Budget::new(limits);
    let planner = planner::Planner::new(scene, request, (store, limits), area, backend)?;
    let finished = finish::finish(planner, &mut budget)?;
    Ok(PreparedScene {
        plan: finished.plan,
        producer: finished.producer,
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
            producer: self.producer.as_ref(),
            workspace,
            rasterizer,
        })
    }

    /// The storage of its shapes and glyph masks, for the next preparation.
    pub(crate) fn into_storage(self) -> Storage {
        self.producer.into_storage()
    }

    /// The plan, for tests.
    #[cfg(test)]
    pub(crate) fn plan(&self) -> &Plan {
        &self.plan
    }
}

impl Rendering<'_> {
    /// Renders every band into `target`, whose pixel `(0, 0)` is the
    /// region's first pixel and which covers the region.
    pub fn render(&mut self, target: &mut Pixmap) {
        let _ = self.render_until(target, |_| false);
    }

    /// Renders bands into `target` top to bottom, asking `stop` with the
    /// next band's first row before each band.
    pub fn render_until(
        &mut self,
        target: &mut Pixmap,
        stop: impl FnMut(u32) -> bool,
    ) -> Completion {
        self.bands(stop, |executor, band| {
            Executor::band(executor, band, Destination::Pixmap(target));
        })
    }

    /// Renders bands top to bottom as [`Self::render_until`] does, appending
    /// each rendered row of the region to `rows` as premultiplied RGBA8,
    /// `4 × width` bytes, so that a surface need not be filled first. Room
    /// for the rows should be reserved: appending beyond it allocates.
    pub fn render_rows_until(
        &mut self,
        rows: &mut Vec<u8>,
        stop: impl FnMut(u32) -> bool,
    ) -> Completion {
        self.bands(stop, |executor, band| {
            Executor::band(executor, band, Destination::Rows(rows));
        })
    }

    fn bands(
        &mut self,
        mut stop: impl FnMut(u32) -> bool,
        mut band: impl FnMut(Executing<'_>, u32),
    ) -> Completion {
        let plan = self.plan;
        self.workspace.restart();
        for index in plan.area.bands() {
            let row = plan.area.band_rows(index).start;
            if stop(row) {
                return Completion::Stopped { row };
            }
            band(
                (
                    plan,
                    &mut *self.workspace,
                    (self.producer, &mut *self.rasterizer),
                ),
                index,
            );
        }
        Completion::Complete
    }

    /// The work and executor-side function evaluations of the last render,
    /// and the bytes retained: what [`PreparedScene::charges`] bounds.
    #[cfg(test)]
    pub(crate) fn usage(&self) -> Charges {
        let usage = self.workspace.usage;
        Charges {
            render_bytes: self.workspace.bytes()
                + self.rasterizer.retained_bytes()
                + finish::plan_bytes(self.plan),
            render_work: usage.work,
            function_work: usage.functions,
        }
    }
}
