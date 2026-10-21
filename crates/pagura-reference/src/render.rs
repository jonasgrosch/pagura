//! The reference rendering: the scene walked in source order over the whole
//! region in f64 buffers, each mark composited pixel by pixel with the
//! formulas of clause 11, and every output pixel quantized once.

mod group;
mod output;
mod sources;
mod tiling;
pub(crate) mod transfer;

use crate::buffer::{Buffer, Initial};
use crate::color::Colors;
use crate::overprint::Simulation;
use crate::pixel::{Blending, Elements, Source};
use crate::request::{Overprint, Rendering, Request};
use crate::shapes::{self, Coverage};
use pagura_color::{Process, Target};
use pagura_core::{Error, Limits, Result, ResultExt};
use pagura_image::DecodedImage;
use pagura_interpret::{
    Color, Composite, FillNode, GlyphsNode, GroupRole, ImageNode, MarkInfo, MaskId, Node, NodeId,
    Paint, Scene, ShadingNode, StrokeNode,
};
use pagura_raster::{DeviceRect, Transform};
use sources::{Sample, Sources};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;
use transfer::{Selection, Transfers};

/// Renders `request.region` of `scene` exactly.
///
/// # Errors
///
/// Returns the errors of the inputs the reference depends on (colour
/// conversion, image decoding, stroking), `Limit` errors for budgets, and
/// `InvalidArgument` for scenes the reference does not model: a partial
/// transfer boundary, a singular pen or shading matrix, pattern-coloured
/// stencil masks, and malformed scene ids.
pub fn render(scene: &Scene, request: &Request, limits: &Limits) -> Result<Rendering> {
    let mut renderer = Renderer::new(scene, request, limits)?;
    let root = scene
        .group(scene.root)
        .ok_or(Error::invalid_argument("scene without a page group"))?;
    let process = root
        .attributes
        .space
        .unwrap_or(if renderer.simulation.enabled {
            Process::Cmyk
        } else {
            Process::Rgb
        });
    let target = if renderer.simulation.enabled {
        Target::separated(process, Arc::clone(&renderer.simulation.spots))
    } else {
        Target::new(process)
    };
    let mut page = Buffer::new(
        request.region,
        target.channels(),
        &Initial::Transparent,
        limits.pixels,
    )?;
    let context = Context {
        device: request.transform,
        target: &target,
        elements: elements(root.attributes.knockout),
        initial: Initial::Transparent,
        selection: Selection::Opaque,
        stencil: None,
        depth: 0,
    };
    renderer.children(&root.children, &mut page, &context)?;
    renderer.output(&page, &target)
}

/// Where in the scene a node is painted.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Context<'b> {
    /// Default user space (pattern space in a cell) to device pixels.
    pub(crate) device: Transform,
    /// The blending space and spot channels of the buffer painted into.
    pub(crate) target: &'b Target,
    /// How elements of the enclosing group composite.
    pub(crate) elements: Elements,
    /// The enclosing group's initial backdrop.
    pub(crate) initial: Initial<'b>,
    /// Whether marks select transfers.
    pub(crate) selection: Selection,
    /// The colour of an uncoloured tiling pattern, which replaces its
    /// cell's paints.
    pub(crate) stencil: Option<&'b Color>,
    /// Nesting depth.
    pub(crate) depth: usize,
}

pub(crate) fn elements(knockout: bool) -> Elements {
    if knockout {
        Elements::Knockout
    } else {
        Elements::Accumulate
    }
}

/// The state of one rendering.
pub(crate) struct Renderer<'s> {
    pub(crate) scene: &'s Scene,
    pub(crate) limits: &'s Limits,
    pub(crate) request: &'s Request,
    pub(crate) window: DeviceRect,
    pub(crate) colors: Colors,
    pub(crate) simulation: Simulation,
    pub(crate) transfers: Option<Transfers>,
    masks: HashMap<MaskKey, Rc<Vec<f64>>>,
    images: HashMap<usize, Rc<DecodedImage>>,
}

impl std::fmt::Debug for Renderer<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Renderer")
            .field("window", &self.window)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct MaskKey {
    mask: MaskId,
    device: [u64; 6],
    process: Process,
}

impl<'s> Renderer<'s> {
    fn new(scene: &'s Scene, request: &'s Request, limits: &'s Limits) -> Result<Self> {
        if request.region.is_empty() {
            return Err(Error::invalid_argument("empty region"));
        }
        let simulation = Simulation::discover(
            scene,
            request.overprint == Overprint::Simulate,
            limits.depth,
        )?;
        Ok(Self {
            scene,
            limits,
            request,
            window: request.region,
            colors: Colors::new(),
            simulation,
            transfers: Transfers::new(scene, request.region),
            masks: HashMap::new(),
            images: HashMap::new(),
        })
    }

    /// Paints `children` into `buffer` in source order.
    pub(crate) fn children(
        &mut self,
        children: &[NodeId],
        buffer: &mut Buffer,
        context: &Context<'_>,
    ) -> Result<()> {
        if context.depth >= self.limits.depth {
            return Err(Error::limit("compositing depth exceeded"));
        }
        for &id in children {
            let node = self
                .scene
                .node(id)
                .ok_or(Error::invalid_argument("node id outside the scene"))?;
            match node {
                Node::Fill(fill) => self.fill(fill, buffer, context).at(fill.mark.position),
                Node::Stroke(stroke) => self
                    .stroke(stroke, buffer, context)
                    .at(stroke.mark.position),
                Node::Glyphs(glyphs) => self.glyphs(glyphs, buffer, context),
                Node::Image(image) => self.image(image, buffer, context).at(image.mark.position),
                Node::Shading(shading) => self
                    .shading(shading, buffer, context)
                    .at(shading.mark.position),
                Node::Group(id) => self.group(*id, buffer, context),
            }?;
        }
        Ok(())
    }

    pub(crate) fn state(&self, mark: &MarkInfo) -> Result<&'s Composite> {
        self.scene
            .states
            .get(mark.state.index())
            .map(AsRef::as_ref)
            .ok_or(Error::invalid_argument("state id outside the scene"))
    }

    fn paint(&self, id: pagura_interpret::PaintId) -> Result<&'s Paint> {
        self.scene
            .paints
            .get(id.index())
            .map(AsRef::as_ref)
            .ok_or(Error::invalid_argument("paint id outside the scene"))
    }

    fn fill(&mut self, fill: &FillNode, buffer: &mut Buffer, context: &Context<'_>) -> Result<()> {
        let paint = self.paint(fill.paint)?;
        let state = self.state(&fill.mark)?;
        let mut regions = vec![shapes::fill(
            &fill.path,
            fill.rule,
            context.device,
            self.limits,
        )?];
        regions.extend(shapes::clips(
            self.scene,
            fill.mark.clip,
            context.device,
            self.limits,
        )?);
        self.paint_regions(regions, paint, state, buffer, context)
    }

    fn stroke(
        &mut self,
        stroke: &StrokeNode,
        buffer: &mut Buffer,
        context: &Context<'_>,
    ) -> Result<()> {
        let paint = self.paint(stroke.paint)?;
        let state = self.state(&stroke.mark)?;
        let pen = self
            .scene
            .pens
            .get(stroke.pen.index())
            .ok_or(Error::invalid_argument("pen id outside the scene"))?;
        let mut regions = vec![shapes::stroke(
            &stroke.path,
            pen,
            context.device,
            self.limits,
        )?];
        regions.extend(shapes::clips(
            self.scene,
            stroke.mark.clip,
            context.device,
            self.limits,
        )?);
        self.paint_regions(regions, paint, state, buffer, context)
    }

    /// Each placement is a separate mark (9.3.8).
    fn glyphs(
        &mut self,
        glyphs: &GlyphsNode,
        buffer: &mut Buffer,
        context: &Context<'_>,
    ) -> Result<()> {
        let paint = self.paint(glyphs.paint)?;
        let state = self.state(&glyphs.mark)?;
        let clips = shapes::clips(self.scene, glyphs.mark.clip, context.device, self.limits)
            .at(glyphs.mark.position)?;
        for (i, placement) in glyphs.placements.iter().enumerate() {
            let result = (|| {
                let source = glyphs
                    .sources
                    .get(pagura_core::convert::u32_to_usize(placement.source))
                    .ok_or(Error::invalid_argument("glyph source outside the run"))?;
                let mut regions = vec![shapes::glyph(
                    source,
                    placement.origin,
                    context.device,
                    self.limits,
                )?];
                regions.extend(clips.iter().cloned());
                self.paint_regions(regions, paint, state, buffer, context)
            })();
            result.at(glyphs.position(i))?;
        }
        Ok(())
    }

    /// Paints `paint` over the intersection of `regions`.
    fn paint_regions(
        &mut self,
        mut regions: Vec<crate::area::Region>,
        paint: &Paint,
        state: &Composite,
        buffer: &mut Buffer,
        context: &Context<'_>,
    ) -> Result<()> {
        let paint = match (context.stencil, paint) {
            (Some(color), _) => Paint::Solid(color.clone()),
            (None, paint) => paint.clone(),
        };
        if !paints(&paint) {
            return Ok(());
        }
        self.validate_transfer(state, matches!(paint, Paint::Solid(_)), context)?;
        match &paint {
            Paint::Solid(color) => {
                let sample = self.solid(color, state, context.target)?;
                let coverage = Coverage::of(&regions, buffer.rect());
                self.composite(
                    &coverage,
                    &Sources::Solid(sample),
                    state,
                    buffer,
                    context,
                    Eligible::Yes,
                )
            }
            Paint::Shading(pattern) => {
                let to_device = pattern.matrix.then(context.device);
                if pattern.shading.background.is_none()
                    && let Some(bbox) = pattern.shading.bbox
                {
                    regions.push(shapes::rect(bbox, to_device, self.limits)?);
                }
                let coverage = Coverage::of(&regions, buffer.rect());
                let sources = self.shading_source(
                    &pattern.shading,
                    to_device,
                    state,
                    context,
                    sources::Use::Pattern,
                )?;
                self.composite(&coverage, &sources, state, buffer, context, Eligible::Yes)
            }
            Paint::Tiling(tiling) => {
                let coverage = Coverage::of(&regions, buffer.rect());
                self.tiling(tiling, &coverage, state, buffer, context)
            }
        }
    }

    fn image(
        &mut self,
        node: &ImageNode,
        buffer: &mut Buffer,
        context: &Context<'_>,
    ) -> Result<()> {
        let state = self.state(&node.mark)?;
        let stencil = node.stencil.map(|id| self.paint(id)).transpose()?;
        let stencil = match (context.stencil, stencil) {
            (Some(color), Some(_)) => Some(Paint::Solid(color.clone())),
            (_, paint) => paint.cloned(),
        };
        let paints_anything = match (&node.image.space, &stencil) {
            (Some(space), _) => space.paints(),
            (None, Some(paint)) => paints(paint),
            (None, None) => false,
        };
        if !paints_anything {
            return Ok(());
        }
        self.validate_transfer(state, true, context)?;
        let to_device = node.transform.then(context.device);
        if !to_device.is_finite() {
            return Err(Error::limit("image transform overflow"));
        }
        if to_device.inverse().is_none() {
            return Ok(());
        }
        let mut regions = vec![shapes::unit_square(to_device, self.limits)?];
        regions.extend(shapes::clips(
            self.scene,
            node.mark.clip,
            context.device,
            self.limits,
        )?);
        let coverage = Coverage::of(&regions, buffer.rect());
        let sources = self.image_source(node, stencil.as_ref(), to_device, state, context)?;
        let eligible = if matches!(
            node.image.mask,
            pagura_interpret::ImageMask::Soft { .. } | pagura_interpret::ImageMask::Embedded
        ) {
            Eligible::No
        } else {
            Eligible::Yes
        };
        self.composite(&coverage, &sources, state, buffer, context, eligible)
    }

    fn shading(
        &mut self,
        node: &ShadingNode,
        buffer: &mut Buffer,
        context: &Context<'_>,
    ) -> Result<()> {
        let state = self.state(&node.mark)?;
        if !node.shading.space.paints() {
            return Ok(());
        }
        self.validate_transfer(state, true, context)?;
        let to_device = node.transform.then(context.device);
        if to_device.inverse().is_none() {
            return Err(Error::invalid_argument(
                "the reference has no singular shading transform",
            ));
        }
        let mut regions = Vec::new();
        if let Some(bbox) = node.shading.bbox {
            regions.push(shapes::rect(bbox, to_device, self.limits)?);
        }
        regions.extend(shapes::clips(
            self.scene,
            node.mark.clip,
            context.device,
            self.limits,
        )?);
        let coverage = Coverage::of(&regions, buffer.rect());
        let sources =
            self.shading_source(&node.shading, to_device, state, context, sources::Use::Mark)?;
        self.composite(&coverage, &sources, state, buffer, context, Eligible::Yes)
    }

    /// Pages with transfer functions select them per pixel; pattern paints
    /// and simulated overprinting have no selection there (the renderer
    /// declines them too).
    fn validate_transfer(
        &self,
        state: &Composite,
        solid: bool,
        context: &Context<'_>,
    ) -> Result<()> {
        if self.transfers.is_none() || context.selection == Selection::Off {
            return Ok(());
        }
        if !solid && context.stencil.is_none() {
            return Err(Error::invalid_argument(
                "the reference has no transfer selection for pattern paint",
            ));
        }
        if state.overprint.enabled && self.simulation.enabled {
            return Err(Error::invalid_argument(
                "the reference has no transfer selection for simulated overprinting",
            ));
        }
        Ok(())
    }

    /// Composites one mark: its coverage, its colours and the state.
    pub(crate) fn composite(
        &mut self,
        coverage: &Coverage,
        sources: &Sources<'_>,
        state: &Composite,
        buffer: &mut Buffer,
        context: &Context<'_>,
        eligible: Eligible,
    ) -> Result<()> {
        let mask = state
            .soft_mask
            .map(|mask| self.soft_mask(mask, context.device, context.target.process, context.depth))
            .transpose()?;
        let blending = Blending {
            mode: state.blend,
            process: context.target.process,
            channels: context.target.channels(),
        };
        for (x, y, covered) in coverage.pixels() {
            let Some(sample) = sources.sample(x, y) else {
                continue;
            };
            let factor = f64::from(covered) / 255.0;
            let mut source = Source {
                color: sample.color,
                alpha: sample.alpha * factor,
                shape: sample.shape * factor,
                preserve: sample.preserve,
            };
            if let Some(transfers) = &mut self.transfers
                && context.selection != Selection::Off
            {
                let opaque = context.selection == Selection::Opaque && eligible == Eligible::Yes;
                transfers.record((x, y), source.shape, state, opaque)?;
            }
            let m = mask
                .as_ref()
                .map_or(1.0, |m| mask_value(m, self.window, x, y));
            source.apply(state, m);
            let initial = context.initial.pixel(x, y);
            let mut pixel = buffer.get(x, y);
            pixel.paint(&source, blending, &initial, context.elements);
            buffer.set(x, y, &pixel);
        }
        Ok(())
    }

    /// The constant sample of a solid colour.
    pub(crate) fn solid(
        &mut self,
        color: &Color,
        state: &Composite,
        target: &Target,
    ) -> Result<Sample> {
        let convert = self
            .colors
            .prepare(&color.space, target, &state.conversion)?;
        let mut sample = Sample::opaque();
        let painted = convert.convert(&color.components, &mut sample.color[..target.channels()]);
        sample.preserve = self.preserve(
            state,
            painted,
            target,
            Some((&color.space, &color.components)),
        );
        Ok(sample)
    }

    /// The channels a sample leaves to the backdrop under simulated
    /// overprinting (11.7.4.3): those it does not paint, and with `OPM 1`
    /// the zero components of a DeviceCMYK fill, stroke or text colour.
    pub(crate) fn preserve(
        &self,
        state: &Composite,
        painted: pagura_color::Painted,
        target: &Target,
        direct: Option<(&Arc<pagura_color::ColorSpace>, &[f64])>,
    ) -> u64 {
        if !(self.simulation.enabled && state.overprint.enabled) {
            return 0;
        }
        let channels = target.channels().min(64);
        let all = if channels == 64 {
            u64::MAX
        } else {
            (1_u64 << channels) - 1
        };
        let mut preserve = all & !painted.0;
        if state.overprint.nonzero
            && target.process == Process::Cmyk
            && let Some((space, components)) = direct
            && cmyk(space)
        {
            for (i, &c) in components.iter().enumerate().take(4) {
                if c.clamp(0.0, 1.0) == 0.0 {
                    preserve |= 1 << i;
                }
            }
        }
        preserve
    }

    /// The decoded samples of an image, decoded once per rendering.
    pub(crate) fn decoded(&mut self, image: &pagura_interpret::Image) -> Result<Rc<DecodedImage>> {
        let key = Arc::as_ptr(&image.source) as usize;
        if let Some(decoded) = self.images.get(&key) {
            return Ok(Rc::clone(decoded));
        }
        let decoded = image.source.decode(
            &pagura_image::DecodeRequest::default(),
            self.limits,
            pagura_core::Recovery::Repair,
        )?;
        let decoded = Rc::new(decoded.image);
        self.images.insert(key, Rc::clone(&decoded));
        Ok(decoded)
    }

    /// The values of soft mask `mask` over the window, computed once per
    /// device transform and parent blending space.
    pub(crate) fn soft_mask(
        &mut self,
        mask: MaskId,
        device: Transform,
        process: Process,
        depth: usize,
    ) -> Result<Rc<Vec<f64>>> {
        let key = MaskKey {
            mask,
            device: [device.a, device.b, device.c, device.d, device.e, device.f].map(f64::to_bits),
            process,
        };
        if let Some(values) = self.masks.get(&key) {
            return Ok(Rc::clone(values));
        }
        let values = Rc::new(self.mask_values(mask, device, process, depth)?);
        self.masks.insert(key, Rc::clone(&values));
        Ok(values)
    }
}

/// Whether an image mark may select its transfer: not with a soft mask
/// or embedded alpha.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Eligible {
    Yes,
    No,
}

fn mask_value(values: &[f64], window: DeviceRect, x: u32, y: u32) -> f64 {
    crate::area::index(window, x, y)
        .and_then(|i| values.get(i))
        .copied()
        .unwrap_or(1.0)
}

/// Whether a paint can change any colorant (8.6.6.4).
fn paints(paint: &Paint) -> bool {
    match paint {
        Paint::Solid(color) => color.space.paints(),
        Paint::Shading(pattern) => pattern.shading.space.paints(),
        Paint::Tiling(_) => true,
    }
}

/// Whether a space is DeviceCMYK, directly or as the alternate of an
/// ICCBased space used without its profile.
fn cmyk(space: &pagura_color::ColorSpace) -> bool {
    match space {
        pagura_color::ColorSpace::DeviceCmyk => true,
        pagura_color::ColorSpace::Icc(icc) if icc.profile.is_none() => cmyk(&icc.alternate),
        _ => false,
    }
}

/// Group roles that paint where they occur.
pub(crate) fn transparency(role: GroupRole) -> bool {
    role == GroupRole::Transparency
}

#[cfg(test)]
mod tests;
