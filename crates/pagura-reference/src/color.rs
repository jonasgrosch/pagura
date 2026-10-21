//! Colour through `pagura-color`'s definitions (ADR 0028, ADR 0034).
//!
//! Every conversion goes through a prepared [`Converter`], except a device
//! colour into a blending space of its own family, which 10.4 defines as
//! the identity. Compositing stores additive channels as intensity and
//! subtractive ones as `1 − tint`, as the converters produce them.

use pagura_color::{
    ColorSpace, Conversion, Converter, Intent, MAX_CHANNELS, Painted, Process, Target,
};
use pagura_core::{Error, Result};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

/// The most channels a pixel has: process plus spot channels.
pub(crate) const MAX: usize = MAX_CHANNELS;

/// The channel values of one pixel; a target's first `channels()` are used.
pub(crate) type Channels = [f64; MAX];

/// A conversion prepared for one colour space, target and set of
/// conversion parameters.
#[derive(Debug)]
pub(crate) enum Prepared {
    /// A device colour into a blending space of its own family.
    Native {
        /// The family.
        process: Process,
        /// The target's channels, spot channels included.
        channels: usize,
    },
    /// Any other conversion.
    Converter(Box<Converter>),
}

impl Prepared {
    /// Converts `components` into `output` (the target's channels) and
    /// returns the channels the colour paints.
    pub(crate) fn convert(&self, components: &[f64], output: &mut [f64]) -> Painted {
        match self {
            Self::Native { process, channels } => {
                let count = process.components();
                for (i, out) in output.iter_mut().enumerate().take(*channels) {
                    *out = if i < count {
                        let v = components.get(i).copied().unwrap_or(0.0).clamp(0.0, 1.0);
                        if *process == Process::Cmyk {
                            1.0 - v
                        } else {
                            v
                        }
                    } else {
                        1.0
                    };
                }
                Painted((1_u64 << count) - 1)
            }
            Self::Converter(converter) => converter.convert(components, output),
        }
    }
}

/// Prepared conversions, cached for one rendering by colour-space and
/// parameter identity. The scene owns every keyed `Arc` for the rendering's
/// whole duration, so no address is reused while the cache lives.
#[derive(Debug)]
pub(crate) struct Colors {
    cache: HashMap<Key, Rc<Prepared>>,
    devices: [Arc<ColorSpace>; 3],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct Key {
    space: usize,
    process: Process,
    spots: usize,
    spot_count: usize,
    intent: u8,
    black: usize,
    removal: usize,
}

impl Colors {
    pub(crate) fn new() -> Self {
        Self {
            cache: HashMap::new(),
            devices: [
                Arc::new(ColorSpace::DeviceGray),
                Arc::new(ColorSpace::DeviceRgb),
                Arc::new(ColorSpace::DeviceCmyk),
            ],
        }
    }

    /// The device colour space of a blending space's family.
    pub(crate) fn device(&self, process: Process) -> &Arc<ColorSpace> {
        match process {
            Process::Gray => &self.devices[0],
            Process::Rgb => &self.devices[1],
            Process::Cmyk => &self.devices[2],
        }
    }

    /// The conversion of `space` into `target` under `conversion`.
    ///
    /// # Errors
    ///
    /// Returns the converter's errors: `Unsupported` for a conversion
    /// colour management cannot perform.
    pub(crate) fn prepare(
        &mut self,
        space: &Arc<ColorSpace>,
        target: &Target,
        conversion: &Conversion,
    ) -> Result<Rc<Prepared>> {
        if target.channels() > MAX {
            return Err(Error::limit("too many colour channels"));
        }
        let key = Key {
            space: Arc::as_ptr(space) as usize,
            process: target.process,
            spots: Arc::as_ptr(&target.spots).cast::<u8>() as usize,
            spot_count: target.spots.len(),
            intent: match conversion.intent {
                Intent::Perceptual => 0,
                Intent::Relative => 1,
                Intent::Saturation => 2,
                Intent::Absolute => 3,
            },
            black: conversion
                .black_generation
                .as_ref()
                .map_or(0, |f| Arc::as_ptr(f) as usize),
            removal: conversion
                .undercolor_removal
                .as_ref()
                .map_or(0, |f| Arc::as_ptr(f) as usize),
        };
        if let Some(prepared) = self.cache.get(&key) {
            return Ok(Rc::clone(prepared));
        }
        let prepared = Rc::new(match (space.as_ref(), target.process) {
            (ColorSpace::DeviceGray, Process::Gray)
            | (ColorSpace::DeviceRgb, Process::Rgb)
            | (ColorSpace::DeviceCmyk, Process::Cmyk) => Prepared::Native {
                process: target.process,
                channels: target.channels(),
            },
            _ => Prepared::Converter(Box::new(Converter::new(space, target, conversion)?)),
        });
        self.cache.insert(key, Rc::clone(&prepared));
        Ok(prepared)
    }

    /// The conversion of colours stored in blending space `from` into
    /// `target`: [`components`] turns stored values into the components
    /// it takes.
    ///
    /// # Errors
    ///
    /// As [`Self::prepare`].
    pub(crate) fn prepare_process(
        &mut self,
        from: Process,
        target: &Target,
        conversion: &Conversion,
    ) -> Result<Rc<Prepared>> {
        let space = Arc::clone(self.device(from));
        self.prepare(&space, target, conversion)
    }
}

/// The device components of process values stored in compositing form.
pub(crate) fn components(process: Process, stored: &[f64]) -> [f64; 4] {
    let mut out = [0.0; 4];
    for (out, &v) in out.iter_mut().zip(stored).take(process.components()) {
        *out = if process == Process::Cmyk { 1.0 - v } else { v };
    }
    out
}

/// The luminosity of a colour stored in compositing form (11.5.3): the
/// gray level, the RGB luminance, or for CMYK the luminance of the inks
/// with black subtracted before clipping.
pub(crate) fn luminosity(process: Process, c: &[f64]) -> f64 {
    let at = |i: usize| c.get(i).copied().unwrap_or(0.0);
    match process {
        Process::Gray => at(0),
        Process::Rgb => lum([at(0), at(1), at(2)]),
        Process::Cmyk => (lum([at(0), at(1), at(2)]) + at(3) - 1.0).max(0.0),
    }
}

/// The luminance of an RGB triple (11.3.5.3).
pub(crate) fn lum(c: [f64; 3]) -> f64 {
    0.3 * c[0] + 0.59 * c[1] + 0.11 * c[2]
}
