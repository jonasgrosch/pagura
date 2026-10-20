//! Reuse the integer image sampler for opaque interiors of a byte page.
//! Boundary pixels retain the general geometric-coverage path. Indexed device
//! palettes expand once into codes instead of unpacking a Texel per footprint,
//! and a matte is removed from each sample once, as the image sampler does.
use super::{Context, Model, Surface, narrow};
use crate::{color::Space, display::ImageCommand, interpreter::images::Image};
use pagura_core::{Error, Result};
use pagura_image::{DecodedImage, ImageKind, MaskKind, Samples};
use pagura_raster::{Mask, Pixmap, Rgba8};
use std::sync::{Arc, Weak};

/// One expanded image, a palette's colours or samples without their matte,
/// shared by every band of its immutable source image.
pub(super) struct Palette {
    source: Weak<Image>,
    image: Arc<Image>,
    bytes: usize,
}
impl Palette {
    pub(super) fn matches(&self, image: &Arc<Image>) -> bool {
        self.source.as_ptr() == Arc::as_ptr(image)
    }
    fn new(source: &Arc<Image>, image: Arc<Image>) -> Self {
        let bytes = match &image.decoded.samples {
            Samples::Bytes(v) => v.capacity(),
            _ => 0,
        }
        .saturating_add(image.decode.capacity() * size_of::<(f64, f64)>())
        .saturating_add(2 * size_of::<Image>() + 4 * size_of::<usize>());
        Self {
            source: Arc::downgrade(source),
            image,
            bytes,
        }
    }
}

impl Context {
    /// Keeps an expanded image only while its source is painted.
    pub(super) fn keep_palette_for(&mut self, image: &Arc<Image>) {
        if (matches!(image.space, Some(Space::Indexed { .. })) || image.matte.is_some())
            && self.palette.as_ref().is_some_and(|p| !p.matches(image))
        {
            self.clear_palette();
        }
    }
    pub(super) fn clear_palette(&mut self) {
        // An image operation may hold a live clone. It must retain its byte
        // charge until that operation releases the last non-cache owner.
        if self
            .palette
            .as_ref()
            .is_some_and(|p| Arc::strong_count(&p.image) > 1)
        {
            return;
        }
        if let Some(palette) = self.palette.take() {
            self.allocated -= palette.bytes;
        }
    }
    fn interior_image(
        &mut self,
        command: &ImageCommand,
        available: usize,
        output_bytes: usize,
    ) -> Result<Option<(Arc<Image>, usize)>> {
        let expanded = matches!(command.image.space, Some(Space::Indexed { .. }))
            || command.image.matte.is_some();
        let pair = if expanded {
            self.charge(command.image.decoded.samples.len().saturating_add(256))?;
            if let Some(palette) = &self.palette
                && palette.matches(&command.image)
            {
                (Arc::clone(&palette.image), 0)
            } else {
                let maximum = available - output_bytes;
                let image = if command.image.matte.is_some() {
                    unmatted(&command.image, maximum)
                } else {
                    expand(&command.image, maximum)?
                };
                let Some(image) = image else {
                    return Ok(None);
                };
                let palette = Palette::new(&command.image, Arc::new(image));
                let image = Arc::clone(&palette.image);
                let bytes = palette.bytes;
                if bytes <= (self.limits.decoded_bytes / 16).min(16 * 1024 * 1024)
                    && bytes.saturating_add(output_bytes) <= available
                {
                    self.clear_palette();
                    if self.palette.is_none() {
                        self.allocated += bytes;
                        self.palette = Some(palette);
                        (image, 0)
                    } else {
                        (image, bytes)
                    }
                } else {
                    (image, bytes)
                }
            }
        } else {
            (Arc::clone(&command.image), 0)
        };
        Ok(Some(pair))
    }
    pub(super) fn image_interiors(
        &mut self,
        command: &ImageCommand,
        target: &mut Surface,
        model: Model,
        clip: &Mask,
    ) -> Result<bool> {
        if !matches!(target.narrow(), Some(narrow::Buffer::U8(_)))
            || model != Model::Rgb
            || self.separations.enabled
            || !command.composite.simple()
            || command.composite.overprint
            || command.transfer.is_some()
            || !matches!(
                command.image.space,
                Some(Space::Rgb | Space::Gray | Space::Indexed { .. })
            )
        {
            return Ok(false);
        }
        // A soft mask's levels are sampled as a gray image of their own.
        let soft = match (&command.image.mask, &command.image.decoded.mask) {
            (None, MaskKind::None) => None,
            (Some(mask), MaskKind::Soft(_)) if mask.mask.is_none() => Some(Arc::clone(mask)),
            _ => return Ok(false),
        };
        if command.image.matte.is_some() && soft.is_none() {
            return Ok(false);
        }
        let count = usize::try_from(u64::from(clip.width()) * u64::from(clip.height()))
            .map_err(|_| Error::limit("image interior extent overflow"))?;
        let output_bytes = count
            .checked_mul(if soft.is_some() { 8 } else { 4 })
            .ok_or(Error::limit("image interior size overflow"))?;
        let available = self.limits.decoded_bytes.saturating_sub(self.allocated);
        if output_bytes > available / 4 {
            return Ok(false);
        }
        let Some((image, bytes)) = self.interior_image(command, available, output_bytes)? else {
            return Ok(false);
        };
        let temporary = bytes
            .checked_add(output_bytes)
            .ok_or(Error::limit("image interior size overflow"))?;
        if temporary > available {
            return Ok(false);
        }
        let mut output =
            Pixmap::new_with_limits(clip.width(), clip.height(), Rgba8::WHITE, self.limits)?;
        let region = crate::Tile {
            x: clip.origin().0,
            y: clip.origin().1,
            width: clip.width(),
            height: clip.height(),
        };
        let source = |image| ImageCommand {
            image,
            transform: command.transform,
            clip: None,
            stencil: command.stencil.clone(),
            composite: command.composite.clone(),
            transfer: None,
            position: command.position,
        };
        let mut opacity = soft
            .as_ref()
            .map(|_| {
                Pixmap::new_with_limits(clip.width(), clip.height(), Rgba8::WHITE, self.limits)
            })
            .transpose()?;
        let cached = std::mem::take(&mut self.clip);
        let viewport = self.viewport.take();
        self.allocated += temporary;
        let mut result = self.paint_image_colour(&source(image), model, &mut output, region);
        if let (Ok(true), Some(mask), Some(opacity)) = (&result, soft, opacity.as_mut()) {
            result =
                self.paint_opaque_image(&source(mask), model, opacity, (region, region), [255; 4]);
        }
        self.allocated -= temporary;
        self.clip = cached;
        self.viewport = viewport;
        if !result? {
            return Ok(false);
        }
        compose(target, clip, region, (&output, opacity.as_ref()))
    }
}

/// Stores the fully covered spans of `clip` from the integer sampler's
/// `output` over `region`, composed at `opacity`'s levels when given.
fn compose(
    target: &mut Surface,
    clip: &Mask,
    region: crate::Tile,
    (output, opacity): (&Pixmap, Option<&Pixmap>),
) -> Result<bool> {
    let tile = target.tile;
    let Some(narrow::Buffer::U8(bytes)) = target.narrow_mut() else {
        return Ok(false);
    };
    let stride = tile.width as usize;
    for (row, column, coverage) in clip.spans() {
        if coverage.first() != Some(&255) {
            continue;
        }
        let x = (region.x - tile.x) as usize + column;
        let y = (region.y - tile.y) as usize + row;
        let source_start = (row * region.width as usize + column) * 4;
        let source = output
            .data()
            .get(source_start..source_start + coverage.len() * 4)
            .ok_or(Error::invalid_argument("image interior outside sampler"))?;
        if let Some(opacity) = &opacity {
            let opacity = opacity
                .data()
                .get(source_start..source_start + coverage.len() * 4)
                .ok_or(Error::invalid_argument("image interior outside sampler"))?;
            bytes.over(
                y * stride + x,
                source.as_chunks::<4>().0,
                opacity.as_chunks::<4>().0,
            )?;
        } else {
            bytes.copy(y * stride + x, source.as_chunks::<4>().0)?;
        }
    }
    Ok(true)
}

/// An eight-bit DeviceGray or DeviceRGB image whose soft mask has its
/// sample grid, with the matte removed from every sample (11.6.5.3) as the
/// image sampler does: decoded, unclipped components c at opacity a become
/// matte + (c - matte) / a, clipped, and zero where a is zero.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss,
    reason = "clipped levels scale to bytes"
)]
fn unmatted(image: &Image, maximum: usize) -> Option<Image> {
    let matte = image.matte.as_deref()?;
    let mask = image.mask.as_deref()?;
    let components = usize::from(image.decoded.components);
    let space = matches!(
        (components, &image.space, matte.len()),
        (1, Some(Space::Gray), 1) | (3, Some(Space::Rgb), 3)
    );
    let (Samples::Bytes(samples), Samples::Bytes(codes)) =
        (&image.decoded.samples, &mask.decoded.samples)
    else {
        return None;
    };
    let [(low, high)] = mask.decode.as_slice() else {
        return None;
    };
    if !space
        || image.decoded.bits_per_component != 8
        || mask.decoded.bits_per_component != 8
        || mask.decoded.components != 1
        || (mask.decoded.width, mask.decoded.height) != (image.decoded.width, image.decoded.height)
        || image.decode.len() != components
        || samples.len() != codes.len().checked_mul(components)?
        || samples
            .len()
            .saturating_add(2 * size_of::<Image>() + 4 * size_of::<usize>())
            > maximum
    {
        return None;
    }
    let opacity: [f64; 256] =
        std::array::from_fn(|code| (low + code as f64 / 255.0 * (high - low)).clamp(0.0, 1.0));
    let channels: Vec<[f64; 256]> = image
        .decode
        .iter()
        .map(|&(low, high)| std::array::from_fn(|code| low + code as f64 / 255.0 * (high - low)))
        .collect();
    let mut pixels = Vec::new();
    pixels.try_reserve_exact(samples.len()).ok()?;
    for (pixel, &code) in samples.chunks_exact(components).zip(codes) {
        let alpha = opacity[usize::from(code)];
        for ((&sample, channel), &matte) in pixel.iter().zip(&channels).zip(matte) {
            let value = if alpha > 0.0 {
                (matte + (channel[usize::from(sample)] - matte) / alpha).clamp(0.0, 1.0)
            } else {
                0.0
            };
            pixels.push((value * 255.0).round() as u8);
        }
    }
    Some(Image {
        decoded: DecodedImage {
            width: image.decoded.width,
            height: image.decoded.height,
            bits_per_component: 8,
            components: image.decoded.components,
            samples: Samples::Bytes(pixels),
            kind: ImageKind::Color { color_space: None },
            decode: None,
            mask: MaskKind::None,
            interpolate: image.decoded.interpolate,
            reduction: image.decoded.reduction,
        },
        space: image.space.clone(),
        mask: None,
        matte: None,
        decode: vec![(0.0, 1.0); components],
        intent: image.intent,
    })
}

fn expand(image: &Image, maximum: usize) -> Result<Option<Image>> {
    let Some(Space::Indexed { base, high, lookup }) = &image.space else {
        return Ok(None);
    };
    let components = match base.as_ref() {
        Space::Rgb => 3,
        Space::Gray => 1,
        _ => return Ok(None),
    };
    let bits = image.decoded.bits_per_component;
    if !(1..=8).contains(&bits)
        || image.decoded.components != 1
        || image.decode.as_slice() != [(0.0, f64::from(u16::MAX >> (16 - bits)))]
        || lookup.len() < (usize::from(*high) + 1) * components
    {
        return Ok(None);
    }
    let Samples::Bytes(samples) = &image.decoded.samples else {
        return Ok(None);
    };
    let bytes = samples
        .len()
        .checked_mul(3)
        .ok_or(Error::limit("palette expansion size overflow"))?;
    if bytes.saturating_add(
        2 * size_of::<Image>() + 4 * size_of::<usize>() + 3 * size_of::<(f64, f64)>(),
    ) > maximum
    {
        return Ok(None);
    }
    let palette: [[u8; 3]; 256] = std::array::from_fn(|code| {
        let offset = code.min(usize::from(*high)) * components;
        if components == 1 {
            [lookup[offset]; 3]
        } else {
            [lookup[offset], lookup[offset + 1], lookup[offset + 2]]
        }
    });
    let mut pixels = Vec::new();
    pixels
        .try_reserve_exact(bytes)
        .map_err(|_| Error::limit("palette expansion allocation failed"))?;
    for &sample in samples {
        pixels.extend_from_slice(&palette[usize::from(sample)]);
    }
    Ok(Some(Image {
        decoded: DecodedImage {
            width: image.decoded.width,
            height: image.decoded.height,
            bits_per_component: 8,
            components: 3,
            samples: Samples::Bytes(pixels),
            kind: ImageKind::Color { color_space: None },
            decode: None,
            mask: MaskKind::None,
            interpolate: image.decoded.interpolate,
            reduction: image.decoded.reduction,
        },
        space: Some(Space::Rgb),
        mask: None,
        matte: None,
        decode: vec![(0.0, 1.0); 3],
        intent: image.intent,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matte_removal_divides_each_sample_by_its_own_opacity() {
        let mask = Image {
            decoded: DecodedImage {
                width: 2,
                height: 1,
                bits_per_component: 8,
                components: 1,
                samples: Samples::Bytes(vec![255, 51]),
                kind: ImageKind::Color { color_space: None },
                decode: None,
                mask: MaskKind::None,
                interpolate: false,
                reduction: None,
            },
            space: Some(Space::Gray),
            mask: None,
            matte: None,
            decode: vec![(0.0, 1.0)],
            intent: None,
        };
        let mut image = Image {
            decoded: DecodedImage {
                width: 2,
                height: 1,
                bits_per_component: 8,
                components: 3,
                samples: Samples::Bytes(vec![10, 20, 30, 40, 51, 0]),
                kind: ImageKind::Color { color_space: None },
                decode: None,
                mask: MaskKind::Soft(pagura_core::ObjectId::new(1, 0)),
                interpolate: false,
                reduction: None,
            },
            space: Some(Space::Rgb),
            mask: Some(Arc::new(mask)),
            matte: Some(vec![0.0, 0.2, 1.0]),
            decode: vec![(0.0, 1.0); 3],
            intent: None,
        };
        let straight = unmatted(&image, 4096).unwrap();
        // Opaque samples keep their levels; at opacity 1/5, matte + (c - matte) * 5
        // gives 200, 51 + (51 - 51) * 5 = 51 and 255 + (0 - 255) * 5, clipped to 0.
        assert_eq!(
            straight.decoded.samples,
            Samples::Bytes(vec![10, 20, 30, 200, 51, 0])
        );
        assert!(straight.mask.is_none() && straight.matte.is_none());
        image.matte = Some(vec![0.0]);
        assert!(unmatted(&image, 4096).is_none());
    }
    use crate::compositor::Policy;
    use crate::display::{Clip, Command, DisplayList};
    use crate::{RenderOptions, Tile};
    use pagura_core::{Limits, Position, Rect};
    use pagura_document::Rotation;
    use pagura_raster::{FillRule, PathBuilder, Transform};
    use std::sync::OnceLock;
    fn image() -> Image {
        Image {
            decoded: DecodedImage {
                width: 2,
                height: 2,
                bits_per_component: 8,
                components: 1,
                samples: Samples::Bytes(vec![0, 1, 2, 255]),
                kind: ImageKind::Color { color_space: None },
                decode: None,
                mask: MaskKind::None,
                interpolate: false,
                reduction: None,
            },
            space: Some(Space::Indexed {
                base: Arc::new(Space::Rgb),
                high: 2,
                lookup: Arc::from([255, 0, 0, 0, 255, 0, 0, 0, 255]),
            }),
            mask: None,
            matte: None,
            decode: vec![(0.0, 255.0)],
            intent: None,
        }
    }
    #[test]
    fn palette_cache_uses_source_identity_without_retaining_source_pixels() {
        let source = Arc::new(image());
        let converted = Arc::new(expand(&source, 4096).unwrap().unwrap());
        let cached = Palette::new(&source, Arc::clone(&converted));
        assert!(cached.matches(&source));
        assert!(!cached.matches(&Arc::new(image())));
        let Samples::Bytes(pixels) = &converted.decoded.samples else {
            unreachable!()
        };
        assert!(cached.bytes >= pixels.capacity() + 2 * size_of::<Image>());
        drop(source);
        assert!(cached.source.upgrade().is_none());
        assert_eq!(
            converted.decoded.samples,
            Samples::Bytes(vec![255, 0, 0, 0, 255, 0, 0, 0, 255, 0, 0, 255])
        );
    }
    #[test]
    fn palette_codes_clamp_to_high_and_decline_noncanonical_decode() {
        let mut source = image();
        let expanded = expand(&source, 4096).unwrap().unwrap();
        assert_eq!(
            expanded.decoded.samples,
            Samples::Bytes(vec![255, 0, 0, 0, 255, 0, 0, 0, 255, 0, 0, 255])
        );
        assert!(expand(&source, 1).unwrap().is_none());
        source.decode[0] = (1.0, 0.0);
        assert!(expand(&source, 4096).unwrap().is_none());
    }
    #[test]
    fn indexed_interiors_preserve_page_and_hard_clip_coordinates() {
        for clipped in [false, true] {
            let mut path = PathBuilder::new();
            path.rect(1.0, 1.0, 2.0, 2.0);
            let clip = clipped.then(|| {
                Arc::new(Clip {
                    parent: None,
                    path: Arc::new(path.finish()),
                    rule: FillRule::NonZero,
                })
            });
            let list = DisplayList {
                commands: vec![Command::Image(ImageCommand {
                    image: Arc::new(image()),
                    transform: Transform::scale(4.0, 4.0),
                    clip,
                    stencil: crate::color::Color::default(),
                    composite: crate::transparency::Composite::opaque(),
                    transfer: None,
                    position: Position::Unknown,
                })],
                crop: Rect {
                    x0: 0.0,
                    y0: 0.0,
                    x1: 4.0,
                    y1: 4.0,
                },
                rotation: Rotation::None,
                user_unit: 1.0,
                limits: Limits::default(),
                group: crate::transparency::Attributes::default(),
                print_intent: false,
                separations: OnceLock::new(),
            };
            let options = RenderOptions::default();
            let page = Tile {
                x: 0,
                y: 0,
                width: 4,
                height: 4,
            };
            let actual = super::super::render_policy(
                &list,
                &options,
                page,
                Some((
                    Policy {
                        rows: 2,
                        curve_tolerance: Some(1.0 / 64.0),
                        byte_composition: true,
                        clip_product: false,
                        geometric_compounds: false,
                    },
                    &mut None,
                    Vec::new(),
                )),
            )
            .unwrap();
            let palette = [[255, 0, 0, 255], [0, 255, 0, 255], [0, 0, 255, 255]];
            for y in 0..4usize {
                for x in 0..4usize {
                    let expected = if clipped && (!(1..3).contains(&x) || !(1..3).contains(&y)) {
                        [255; 4]
                    } else {
                        palette[[0, 1, 2, 2][(y / 2) * 2 + x / 2]]
                    };
                    assert_eq!(
                        &actual.data()[(y * 4 + x) * 4..(y * 4 + x + 1) * 4],
                        expected
                    );
                }
            }
            let exact = super::super::render(
                &list,
                &RenderOptions::default(),
                Tile {
                    x: 0,
                    y: 0,
                    width: 4,
                    height: 4,
                },
            )
            .unwrap();
            assert_eq!(actual, exact);
        }
    }
}

#[cfg(test)]
mod stencil_tests {
    use super::*;
    use crate::compositor::Policy;
    use crate::display::{Command, DisplayList};
    use crate::{RenderOptions, Tile};
    use pagura_core::{Limits, Position, Rect};
    use pagura_document::Rotation;
    use pagura_raster::Transform;
    use std::sync::OnceLock;
    #[test]
    fn constant_stencils_fill_spans_with_exact_byte_coverage_and_polarity() {
        for inverted in [false, true] {
            let image = Image {
                decoded: DecodedImage {
                    width: 1,
                    height: 1,
                    bits_per_component: 1,
                    components: 1,
                    samples: Samples::Bytes(vec![0]),
                    kind: ImageKind::Stencil { inverted },
                    decode: None,
                    mask: MaskKind::None,
                    interpolate: true,
                    reduction: None,
                },
                space: None,
                mask: None,
                matte: None,
                decode: vec![(0.0, 1.0)],
                intent: None,
            };
            let list = DisplayList {
                commands: vec![Command::Image(ImageCommand {
                    image: Arc::new(image),
                    transform: Transform::new(3.0, 0.0, 0.0, 4.0, 0.5, 0.0),
                    clip: None,
                    stencil: crate::color::Color::Solid {
                        space: Space::Rgb,
                        components: vec![0.0, 0.0, 1.0],
                    },
                    composite: crate::transparency::Composite::opaque(),
                    transfer: None,
                    position: Position::Unknown,
                })],
                crop: Rect {
                    x0: 0.0,
                    y0: 0.0,
                    x1: 4.0,
                    y1: 4.0,
                },
                rotation: Rotation::None,
                user_unit: 1.0,
                limits: Limits::default(),
                group: crate::transparency::Attributes::default(),
                print_intent: false,
                separations: OnceLock::new(),
            };
            for bands in [
                None,
                Some(Policy {
                    rows: 2,
                    curve_tolerance: Some(1.0 / 64.0),
                    byte_composition: true,
                    clip_product: false,
                    geometric_compounds: false,
                }),
            ] {
                let options = RenderOptions::default();
                let pixmap = if let Some(bands) = bands {
                    super::super::render_policy(
                        &list,
                        &options,
                        Tile {
                            x: 0,
                            y: 0,
                            width: 4,
                            height: 4,
                        },
                        Some((bands, &mut None, Vec::new())),
                    )
                } else {
                    list.render(&options)
                }
                .unwrap();
                for y in 0..4usize {
                    for x in 0..4usize {
                        let expected = if inverted {
                            [255; 4]
                        } else if x == 0 || x == 3 {
                            [127, 127, 255, 255]
                        } else {
                            [0, 0, 255, 255]
                        };
                        assert_eq!(
                            &pixmap.data()[(y * 4 + x) * 4..(y * 4 + x + 1) * 4],
                            expected
                        );
                    }
                }
            }
        }
    }
}
