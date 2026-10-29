//! Hand-built scenes through preparation and execution, against the
//! reference renderer (an independent f64 evaluation of clause 11 with
//! exact coverage) and closed forms.

mod composition;
mod formats;
mod functions;
mod images;
mod invariance;
mod mixed;
mod patterns;
mod runs;
mod streaming;

use crate::execute::Rendering;
use crate::plan::{Conversion, Curve, Glyph, GroupPlan, Item, ItemKind, MaskPlan, Plan, SpotPlan};
use crate::request::RenderRequest;
use crate::source::{ScratchLayout, Source};
use crate::testing::Build;
use crate::{Charges, Scratch};
use pagura_core::{Limits, Result};

impl Build {
    /// Renders `request` into region-sized premultiplied RGBA8, checking
    /// that preparation reserved at least what rendering used.
    pub(crate) fn render(&self, request: &RenderRequest) -> Result<Vec<u8>> {
        Ok(self.render_charged(request)?.0)
    }

    /// [`Self::render`] with the reserved charges.
    pub(crate) fn render_charged(&self, request: &RenderRequest) -> Result<(Vec<u8>, Charges)> {
        let prepared = self.prepare(request, &Limits::default())?;
        let mut scratch = Scratch::default();
        let mut rendering = prepared.with_scratch(&mut scratch)?;
        let mut rows = Vec::new();
        rendering.render(&mut rows);
        assert_within_charges(&rendering, prepared.charges());
        Ok((rows, prepared.charges()))
    }

    /// Renders the whole page.
    pub(crate) fn render_page(&self) -> Vec<u8> {
        self.render(&self.request(self.page(), RenderRequest::BAND_HEIGHT))
            .unwrap_or_else(|e| panic!("render failed: {e}"))
    }
}

/// Asserts that the last render of `rendering` used no more work and
/// function evaluations, and retains no more bytes, than `reserved`.
pub(crate) fn assert_within_charges(rendering: &Rendering<'_>, reserved: Charges) {
    let usage = rendering.workspace.usage;
    let used = Charges {
        render_bytes: rendering.shapes.retained_bytes()
            + rendering.workspace.bytes()
            + rendering.rasterizer.retained_bytes()
            + plan_bytes(rendering.plan),
        render_work: usage.work,
        function_work: usage.functions,
    };
    assert!(
        used.render_bytes <= reserved.render_bytes
            && used.render_work <= reserved.render_work
            && used.function_work <= reserved.function_work,
        "used {used:?} beyond reserved {reserved:?}"
    );
}

/// Bytes the plan's own structures retain.
fn plan_bytes(plan: &Plan) -> usize {
    let groups: usize = plan
        .groups
        .iter()
        .map(|g| (g.items.capacity() + g.order.capacity()) * size_of::<u32>())
        .sum();
    let glyphs: usize = plan
        .items
        .iter()
        .map(|item| match &item.kind {
            ItemKind::Run(run) => run.glyphs.capacity() * size_of::<Glyph>(),
            _ => 0,
        })
        .sum();
    let Plan { _names: names, .. } = plan;
    let shared_names = 2 * size_of::<usize>()
        + size_of_val(&*names.spots)
        + names
            .spots
            .iter()
            .map(pagura_core::Name::heap_bytes)
            .sum::<usize>()
        + if std::sync::Arc::ptr_eq(&names.spots, &names.empty) {
            0
        } else {
            2 * size_of::<usize>()
        };
    let spots = plan.output.spots.capacity() * size_of::<SpotPlan>()
        + plan
            .output
            .spots
            .iter()
            .map(|spot| {
                spot.convert.owned_bytes() + spot.inputs.capacity() * size_of::<(usize, usize)>()
            })
            .sum::<usize>();
    shared_names
        + spots
        + plan.output.transfers.capacity() * size_of::<[Option<Curve>; 3]>()
        + plan.paints.iter().map(Source::bytes).sum::<usize>()
        + plan.items.capacity() * size_of::<Item>()
        + plan.groups.capacity() * size_of::<GroupPlan>()
        + groups
        + glyphs
        + plan.masks.capacity() * size_of::<MaskPlan>()
        + plan.paints.capacity() * size_of::<Source>()
        + plan.slots.of.capacity() * size_of::<u32>()
        + plan.slots.layouts.capacity() * size_of::<Option<ScratchLayout>>()
        + plan.conversions.capacity() * size_of::<Conversion>()
}
