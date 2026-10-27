//! Payload estimates held by a recorded scene, apart from its table slots.
//!
//! Interned table entries pay once per entry. Other shared roots pay per
//! retained reference: this may overestimate aliases, but never depends on
//! the Store, reference counts, or earlier recordings. ICC profiles are the
//! exception: the interpretation hands every reference to one profile stream
//! the same profile, so a scene pays for each once ([`Profiles`]). Child
//! value types supply their own retained estimates; this is not allocator
//! telemetry.

use crate::model::{
    Composite, FillStroke, Image, ImageMask, Paint, Pen, Shading, ShadingFunction, ShadingKind,
    Transfer,
};
use pagura_color::{ColorSpace, Function, Profile};
use pagura_core::hash::FastMap;
use pagura_core::{Meter, Result};
use pagura_raster::Path;
use std::sync::Arc;

pub(super) const ARC: usize = 2 * size_of::<usize>();

/// The ICC profiles of a scene's colour spaces, each charged once.
#[derive(Debug, Default)]
pub(super) struct Profiles(FastMap<usize, Arc<Profile>>);

impl Profiles {
    /// Charges the profiles of `spaces` the scene has not met yet. The map
    /// holds each one, so that its address stays its own.
    pub(super) fn charge<'a>(
        &mut self,
        spaces: impl IntoIterator<Item = &'a ColorSpace>,
        bytes: &mut Meter,
    ) -> Result<()> {
        for profile in spaces.into_iter().flat_map(ColorSpace::profiles) {
            let key = Arc::as_ptr(profile).addr();
            if self.0.contains_key(&key) {
                continue;
            }
            bytes.reserve_map_capacity(&mut self.0, 1)?;
            bytes.charge(ARC.saturating_add(profile.retained_bytes()))?;
            self.0.insert(key, Arc::clone(profile));
        }
        Ok(())
    }

    /// How many profiles the scene holds.
    #[cfg(test)]
    pub(super) fn len(&self) -> usize {
        self.0.len()
    }

    /// The bytes of the index, which a finished scene does not keep.
    pub(super) fn index_bytes(&self, bytes: &Meter) -> Result<usize> {
        bytes.map_bytes(&self.0)
    }
}

/// The colour space a paint holds.
pub(super) fn paint_space(paint: &Paint) -> Option<&ColorSpace> {
    match paint {
        Paint::Solid(color) => Some(&color.space),
        Paint::Shading(value) => Some(&value.shading.space),
        Paint::Tiling(value) => value.color.as_ref().map(|color| color.space.as_ref()),
    }
}

/// The colour spaces of an image and its masks.
pub(super) fn image_spaces(image: &Image) -> impl Iterator<Item = &ColorSpace> {
    std::iter::successors(Some(image), |image| match &image.mask {
        ImageMask::Stencil(mask) | ImageMask::Soft { mask, .. } => Some(mask.as_ref()),
        ImageMask::None | ImageMask::ColorKey(_) | ImageMask::Embedded => None,
    })
    .filter_map(|image| image.space.as_deref())
}

pub(super) fn path(path: &Path) -> usize {
    ARC.saturating_add(size_of::<Path>())
        .saturating_add(path.retained_bytes())
}

pub(super) fn function(function: &Function) -> usize {
    ARC.saturating_add(function.retained_bytes())
}

pub(super) fn composite(state: &Composite) -> usize {
    let transfer = state.transfer.as_deref().map_or(0, |transfer| {
        let functions = match transfer {
            Transfer::All(f) => function(f),
            Transfer::PerComponent(fs) => fs
                .iter()
                .flatten()
                .map(|f| function(f))
                .fold(0, usize::saturating_add),
        };
        ARC.saturating_add(size_of::<Transfer>())
            .saturating_add(functions)
    });
    [
        ARC,
        size_of::<Composite>(),
        transfer,
        state
            .conversion
            .black_generation
            .as_deref()
            .map_or(0, function),
        state
            .conversion
            .undercolor_removal
            .as_deref()
            .map_or(0, function),
    ]
    .into_iter()
    .fold(0, usize::saturating_add)
}

pub(super) fn fill_stroke(states: &FillStroke) -> usize {
    [
        composite(&states.group),
        composite(&states.fill),
        composite(&states.stroke),
    ]
    .into_iter()
    .fold(0, usize::saturating_add)
}

pub(super) fn paint(paint: &Paint) -> usize {
    let color = |color: &crate::model::Color| {
        ARC.saturating_add(color.space.retained_bytes_without_profiles())
            .saturating_add(color.components.heap_bytes())
    };
    let payload = match paint {
        Paint::Solid(value) => color(value),
        Paint::Shading(value) => shading(&value.shading),
        Paint::Tiling(value) => value.color.as_ref().map_or(0, color),
    };
    ARC.saturating_add(size_of::<Paint>())
        .saturating_add(payload)
}

pub(super) fn pen(pen: &Pen) -> usize {
    ARC.saturating_add(size_of::<Pen>()).saturating_add(
        pen.stroke
            .dash_array
            .capacity()
            .saturating_mul(size_of::<f64>()),
    )
}

pub(super) fn shading(shading: &Shading) -> usize {
    let functions = match &shading.function {
        Some(ShadingFunction::Single(f)) => function(f),
        Some(ShadingFunction::PerComponent(fs)) => {
            ARC.saturating_add(size_of_val(fs.as_ref())).saturating_add(
                fs.iter()
                    .map(|f| function(f))
                    .fold(0, usize::saturating_add),
            )
        }
        None => 0,
    };
    let geometry = match &shading.kind {
        ShadingKind::Triangles(triangles) => ARC
            .saturating_add(size_of_val(triangles.as_ref()))
            .saturating_add(
                triangles
                    .iter()
                    .flatten()
                    .map(|vertex| vertex.color.heap_bytes())
                    .fold(0, usize::saturating_add),
            ),
        ShadingKind::Patches(patches) => ARC
            .saturating_add(size_of_val(patches.as_ref()))
            .saturating_add(
                patches
                    .iter()
                    .flat_map(|patch| &patch.colors)
                    .map(pagura_color::Components::heap_bytes)
                    .fold(0, usize::saturating_add),
            ),
        _ => 0,
    };
    [
        ARC,
        size_of::<Shading>(),
        ARC,
        shading.space.retained_bytes_without_profiles(),
        shading
            .background
            .as_ref()
            .map_or(0, pagura_color::Components::heap_bytes),
        functions,
        geometry,
    ]
    .into_iter()
    .fold(0, usize::saturating_add)
}

pub(super) fn image(mut image: &Image) -> usize {
    let mut total = 0_usize;
    loop {
        // ImageSource includes its root/header/data estimates. Keep the Arc
        // counters of those three owners beside the model's own root.
        total = [
            total,
            ARC,
            size_of::<Image>(),
            3 * ARC,
            image.source.retained_bytes(),
            image.space.as_ref().map_or(0, |space| {
                ARC.saturating_add(space.retained_bytes_without_profiles())
            }),
            ARC,
            size_of_val(image.decode.as_ref()),
        ]
        .into_iter()
        .fold(0, usize::saturating_add);
        match &image.mask {
            ImageMask::Stencil(mask) => image = mask,
            ImageMask::Soft { mask, matte } => {
                total = total.saturating_add(
                    matte
                        .as_ref()
                        .map_or(0, pagura_color::Components::heap_bytes),
                );
                image = mask;
            }
            ImageMask::ColorKey(ranges) => {
                return total
                    .saturating_add(ARC)
                    .saturating_add(size_of_val(ranges.as_ref()));
            }
            ImageMask::None | ImageMask::Embedded => return total,
        }
    }
}
