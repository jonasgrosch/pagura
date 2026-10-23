//! Source samples become reusable PNG resources, never page-sized raster
//! substitutes. Browser image resampling is deliberately measured separately
//! from Pagura's exact-area CPU sampler.
use super::{Failure, Result, Writer, matrix};
use pagura_color::{Converter, MAX_COMPONENTS, Process, Target};
use pagura_core::Error;
use pagura_image::{DecodeRequest, DecodedImage, ImageKind, MaskKind};
use pagura_interpret::{Image, ImageMask, ImageNode, Paint};
use pagura_raster::{Transform, quantize_unit};

impl Writer<'_> {
    pub(super) fn image(&mut self, node: &ImageNode) -> Result<()> {
        let key = (node.image.source.id(), node.mark.state, node.stencil);
        let id = if let Some(&id) = self.image_ids.get(&key) {
            id
        } else {
            let png = self.image_png(node)?;
            let id = self.image_ids.len();
            self.defs.add(format_args!("<image id=\"i{id}\" width=\"1\" height=\"1\" preserveAspectRatio=\"none\" image-rendering=\"{}\" href=\"data:image/png;base64,", if node.image.interpolate {"auto"} else {"pixelated"}))?;
            for chunk in png.chunks(3) {
                const ALPHABET: &[u8; 64] =
                    b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
                let a = chunk[0];
                let b = chunk.get(1).copied().unwrap_or(0);
                let c = chunk.get(2).copied().unwrap_or(0);
                self.defs.add(format_args!(
                    "{}{}{}{}",
                    char::from(ALPHABET[usize::from(a >> 2)]),
                    char::from(ALPHABET[usize::from((a & 3) << 4 | b >> 4)]),
                    if chunk.len() > 1 {
                        char::from(ALPHABET[usize::from((b & 15) << 2 | c >> 6)])
                    } else {
                        '='
                    },
                    if chunk.len() > 2 {
                        char::from(ALPHABET[usize::from(c & 63)])
                    } else {
                        '='
                    }
                ))?;
            }
            self.defs.add(format_args!("\"/>"))?;
            self.image_ids.insert(key, id);
            id
        };
        self.begin(node.mark)?;
        // PDF image samples start at the top, while the image unit square
        // has y up. SVG images have y down.
        let transform = Transform::new(1.0, 0.0, 0.0, -1.0, 0.0, 1.0).then(node.transform);
        self.body.add(format_args!(
            "<use href=\"#i{id}\" transform=\"{}\"/>",
            matrix(transform)?
        ))?;
        self.end(node.mark)
    }

    fn image_png(&mut self, node: &ImageNode) -> Result<Vec<u8>> {
        let image = &node.image;
        let count = (u64::from(image.source.width()) * u64::from(image.source.height()))
            .try_into()
            .map_err(|_| Error::limit("SVG image dimensions"))?;
        self.image_bytes.charge_product(count, 4)?;
        let decoded =
            image
                .source
                .decode(&DecodeRequest::FULL, &self.limits, image.source.recovery())?;
        self.document.report(decoded.diagnostics);
        let decoded = decoded.image;
        if matches!(decoded.mask, MaskKind::Embedded(_) | MaskKind::InData) {
            return Err(Failure::Unsupported(
                "SVG embedded image alpha is not implemented",
            ));
        }
        let mask = match &image.mask {
            ImageMask::Stencil(mask) | ImageMask::Soft { mask, matte: None } => Some(mask),
            ImageMask::Soft { matte: Some(_), .. } | ImageMask::Embedded => {
                return Err(Failure::Unsupported(
                    "SVG image matte and embedded alpha are not implemented",
                ));
            }
            ImageMask::None | ImageMask::ColorKey(_) => None,
        };
        let mask = if let Some(mask) = mask {
            if mask.source.width() != decoded.width || mask.source.height() != decoded.height {
                return Err(Failure::Unsupported(
                    "SVG image mask resampling is not implemented",
                ));
            }
            let output =
                mask.source
                    .decode(&DecodeRequest::FULL, &self.limits, mask.source.recovery())?;
            self.document.report(output.diagnostics);
            Some((mask, output.image))
        } else {
            None
        };
        let state = &self.scene.states[node.mark.state.index()];
        let mut conversion = state.conversion.clone();
        if let Some(intent) = image.intent {
            conversion.intent = intent;
        }
        let converter = image
            .space
            .as_ref()
            .map(|space| Converter::new(space, &Target::new(Process::Rgb), &conversion))
            .transpose()?;
        let stencil = if let Some(paint) = node.stencil {
            let Paint::Solid(color) = self.scene.paints[paint.index()].as_ref() else {
                return Err(Failure::Unsupported(
                    "SVG patterned image stencil is not implemented",
                ));
            };
            let converter = Converter::new(&color.space, &Target::new(Process::Rgb), &conversion)?;
            self.work.charge(converter.cost())?;
            self.functions.charge(converter.cost())?;
            let mut rgb = [0.0; 3];
            if converter.convert(&color.components, &mut rgb).0 == 0 {
                return Err(Failure::Unsupported("SVG non-painting image stencil"));
            }
            Some(rgb)
        } else {
            None
        };
        self.work
            .charge_product(count, converter.as_ref().map_or(1, |c| c.cost().max(1)))?;
        self.functions
            .charge_product(count, converter.as_ref().map_or(0, |c| c.cost()))?;
        let mut rgba = Vec::new();
        rgba.try_reserve_exact(count * 4)
            .map_err(|_| Error::limit("SVG image allocation"))?;
        let channels = usize::from(decoded.components);
        if channels > MAX_COMPONENTS {
            return Err(Error::limit("SVG image components").into());
        }
        let mut values = [0.0; MAX_COMPONENTS];
        for pixel in 0..count {
            let mut alpha = 1.0;
            let rgb = if let Some(rgb) = stencil {
                alpha = opacity(image, &decoded, pixel)?;
                rgb
            } else {
                for (channel, value) in values[..channels].iter_mut().enumerate() {
                    let bits = decoded
                        .component_bits(channel)
                        .ok_or(Error::invalid_argument("SVG image precision"))?;
                    let sample = decoded
                        .samples
                        .get(pixel * channels + channel)
                        .ok_or(Error::invalid_argument("SVG image samples"))?;
                    let [lo, hi] = image.decode.get(channel).copied().unwrap_or([0.0, 1.0]);
                    *value = lo + f64::from(sample) / f64::from((1_u32 << bits) - 1) * (hi - lo);
                }
                if let ImageMask::ColorKey(key) = &image.mask
                    && key.iter().enumerate().all(|(c, [lo, hi])| {
                        decoded
                            .samples
                            .get(pixel * channels + c)
                            .is_some_and(|s| (*lo..=*hi).contains(&s))
                    })
                {
                    alpha = 0.0;
                }
                let mut rgb = [0.0; 3];
                let converter = converter.as_ref().ok_or(Failure::Unsupported(
                    "SVG image colour space is unavailable",
                ))?;
                if converter.convert(&values[..channels], &mut rgb).0 == 0 {
                    alpha = 0.0;
                }
                rgb
            };
            if let Some((mask, decoded)) = &mask {
                alpha *= opacity(mask, decoded, pixel)?;
            }
            rgba.extend(rgb.map(quantize_unit));
            rgba.push(quantize_unit(alpha));
        }
        Ok(pagura_codec::png::encode(
            &rgba,
            decoded.width,
            decoded.height,
            &pagura_codec::png::Options {
                level: pagura_codec::flate::Level::Fast,
                limits: self.limits,
                ..Default::default()
            },
        )?)
    }
}

fn opacity(image: &Image, decoded: &DecodedImage, pixel: usize) -> Result<f64> {
    let sample = decoded
        .samples
        .get(pixel)
        .ok_or(Error::invalid_argument("SVG mask sample"))?;
    if let ImageKind::Stencil { inverted } = decoded.kind {
        return Ok(f64::from((sample != 0) == inverted));
    }
    let [lo, hi] = image.decode.first().copied().unwrap_or([0.0, 1.0]);
    Ok(
        (lo + f64::from(sample) / f64::from((1_u32 << decoded.bits_per_component) - 1) * (hi - lo))
            .clamp(0.0, 1.0),
    )
}
