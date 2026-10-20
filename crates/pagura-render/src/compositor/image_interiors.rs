//! Reuse the integer image sampler for opaque interiors of a byte page.
//! Boundary pixels retain the general geometric-coverage path. Indexed device
//! palettes expand once into codes instead of unpacking a Texel per footprint.
use super::{Context, Model, Surface, narrow};
use crate::{color::Space, display::ImageCommand, interpreter::images::Image};
use pagura_core::{Error, Result};
use pagura_image::{DecodedImage, ImageKind, MaskKind, Samples};
use pagura_raster::{Mask, Pixmap, Rgba8};
use std::sync::{Arc, Weak};

/// One expanded palette, shared by every band of its immutable image.
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
        let pair = if matches!(command.image.space, Some(Space::Indexed { .. })) {
            self.charge(command.image.decoded.samples.len().saturating_add(256))?;
            if let Some(palette) = &self.palette
                && palette.matches(&command.image)
            {
                (Arc::clone(&palette.image), 0)
            } else {
                let Some(image) = expand(&command.image, available - output_bytes)? else {
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
        // A matte of a DeviceGray or DeviceRGB image is removed once composed.
        let matte = match (command.image.matte.as_deref(), &command.image.space) {
            (None, _) => None,
            (Some(&[gray]), Some(Space::Gray)) if soft.is_some() => Some([gray; 3]),
            (Some(&[r, g, b]), Some(Space::Rgb)) if soft.is_some() => Some([r, g, b]),
            _ => return Ok(false),
        };
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
        let cached = self.clip.take();
        let viewport = self.viewport.take();
        self.allocated += temporary;
        let mut result = self.paint_image_colour(&source(image), model, &mut output, region);
        if let (Ok(true), Some(mask), Some(opacity)) = (&result, soft, opacity.as_mut()) {
            result = self.paint_opaque_image(&source(mask), model, opacity, (region, region), [255; 4]);
        }
        self.allocated -= temporary;
        self.clip = cached;
        self.viewport = viewport;
        if !result? {
            return Ok(false);
        }
        if let (Some(matte), Some(opacity)) = (matte, &opacity) {
            remove_matte(output.data_mut(), opacity.data(), matte);
        }
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
                bytes.over(y * stride + x, source.as_chunks::<4>().0, opacity.as_chunks::<4>().0)?;
            } else {
                bytes.copy(y * stride + x, source.as_chunks::<4>().0)?;
            }
        }
        Ok(true)
    }
}

/// Replaces premultiplied-with-matte colour levels by the colour they hold
/// at each opacity (11.6.5.3): composed at that opacity, the colour is
/// c - matte (1 - opacity), which removing the matte from area means keeps.
/// Zero opacity gives black, as the image sampler does.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "levels are clamped to the byte range"
)]
fn remove_matte(colors: &mut [u8], opacities: &[u8], matte: [f64; 3]) {
    let matte = matte.map(|m| m.clamp(0.0, 1.0) * 255.0);
    for (color, opacity) in colors
        .as_chunks_mut::<4>()
        .0
        .iter_mut()
        .zip(opacities.as_chunks::<4>().0)
    {
        let alpha = f64::from(opacity[0]) / 255.0;
        for (channel, matte) in color[..3].iter_mut().zip(matte) {
            *channel = if alpha > 0.0 {
                (matte + (f64::from(*channel) - matte) / alpha)
                    .round()
                    .clamp(0.0, 255.0) as u8
            } else {
                0
            };
        }
    }
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
                        fragments: false,
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
