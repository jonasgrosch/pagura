//! A shading's colour stage: its function (or the mesh's interpolated
//! colours) and the prepared conversion into the target.
//!
//! The colours of a one-input stage kept to [`Precision::Level`] may be
//! certified ahead ([`Ramp`]): a sample whose certified colour rounds to one
//! level skips the evaluation, and the rest convert exactly, so levels are
//! those of exact conversion by construction.

use super::{Input, PaintContext, Parts};
use crate::kernel;
use crate::source::{Precision, copy_values};
use pagura_color::{Converter, Function, MAX_CHANNELS, MAX_COMPONENTS, Painted, Ramp, RampStage};
use pagura_core::{Error, Meter, Result};
use pagura_interpret::ShadingFunction;
use std::sync::Arc;

#[derive(Debug)]
pub(super) struct Colors {
    functions: Functions,
    /// Colour components of the shading's space.
    components: usize,
    converter: Converter,
    channels: usize,
    cost: usize,
    /// The input values a conversion reads.
    inputs: usize,
    /// Certified colours of a one-input stage; empty without.
    ramp: Ramp,
}

/// One sample's function input, and the last input converted with its
/// colour: inputs equal bit for bit convert to equal colours, so a run of
/// them (an extended axis, a flat stretch of a function) converts once.
pub(super) struct Sample {
    pub(super) input: Input,
    /// The input values the last conversion read; `None` before one.
    last: Option<usize>,
    key: Input,
    color: [f64; MAX_CHANNELS],
    /// How precisely converted colours are kept.
    precision: Precision,
}

impl Sample {
    /// A sample whose colours are kept to `precision`.
    pub(super) fn new(precision: Precision) -> Self {
        Self {
            input: [0.0; MAX_COMPONENTS],
            last: None,
            key: [0.0; MAX_COMPONENTS],
            color: [0.0; MAX_CHANNELS],
            precision,
        }
    }
}

#[derive(Debug)]
enum Functions {
    /// Mesh colours given directly.
    Identity,
    Single(Arc<Function>),
    PerComponent(Box<[Arc<Function>]>),
}

impl Colors {
    pub(super) fn new(
        shading: &Parts<'_>,
        context: &PaintContext<'_>,
        bytes: &mut Meter,
    ) -> Result<Self> {
        let components = shading.space.components();
        if components > MAX_COMPONENTS {
            return Err(Error::limit(
                "shading component count exceeds implementation limit (Annex C)",
            ));
        }
        let converter = Converter::new_metered(
            shading.space,
            context.target,
            &context.composite.conversion,
            bytes,
        )?;
        let functions = match shading.function {
            None => Functions::Identity,
            Some(ShadingFunction::Single(f)) => Functions::Single(Arc::clone(f)),
            Some(ShadingFunction::PerComponent(fs)) => {
                let mut held = Vec::new();
                bytes.reserve(&mut held, fs.len())?;
                held.extend(fs.iter().cloned());
                Functions::PerComponent(held.into())
            }
        };
        let function_cost = match &functions {
            Functions::Identity => 0,
            Functions::Single(f) => f.cost(),
            Functions::PerComponent(fs) => {
                fs.iter().fold(0_usize, |c, f| c.saturating_add(f.cost()))
            }
        };
        let inputs = match &functions {
            Functions::Identity => components,
            Functions::Single(f) => f.inputs(),
            Functions::PerComponent(fs) => fs.iter().map(|f| f.inputs()).max().unwrap_or(0),
        };
        Ok(Self {
            functions,
            components,
            channels: context.target.channels(),
            cost: function_cost.saturating_add(converter.cost()),
            converter,
            inputs: inputs.min(MAX_COMPONENTS),
            ramp: Ramp::default(),
        })
    }

    /// Certifies the colours of function inputs within `inputs` (bounds in
    /// IEEE total order) ahead, in as many pieces as `samples` samples
    /// repay; skipped where the stage has no enclosure, the functions meter
    /// lacks room for the most enclosures it may take, or `bytes` lacks room
    /// for its storage at any point of its construction.
    pub(super) fn certify(
        &mut self,
        inputs: [f64; 2],
        samples: usize,
        (bytes, functions): (&mut Meter, &mut Meter),
    ) {
        let stage = match &self.functions {
            Functions::Identity => return,
            Functions::Single(f) => RampStage::Single(f),
            Functions::PerComponent(fs) => RampStage::PerComponent(fs),
        };
        let limit = (samples / SAMPLES_PER_PIECE).min(MAX_PIECES);
        let enclosure = ENCLOSURE_COST.saturating_mul(self.cost);
        if limit < MIN_PIECES
            || limit.saturating_mul(4).saturating_mul(enclosure) > functions.remaining()
        {
            return;
        }
        // The ramp charges its storage to `bytes` as it allocates it.
        let ramp = Ramp::new(
            stage,
            self.components,
            &self.converter,
            (inputs, limit),
            bytes,
        );
        // Within the room checked above.
        if functions
            .charge_product(ramp.enclosures(), enclosure)
            .is_ok()
        {
            self.ramp = ramp;
        } else {
            bytes.refund(ramp.bytes());
        }
    }

    /// Whether colours are certified ahead.
    #[cfg(test)]
    pub(super) fn certified(&self) -> bool {
        !self.ramp.is_empty()
    }

    /// The ramp of a one-input stage, where it holds pieces.
    pub(super) fn ramp(&self) -> Option<&Ramp> {
        (!self.ramp.is_empty()).then_some(&self.ramp)
    }

    /// Owned conversion storage and function handles; function data stays shared.
    pub(super) fn bytes(&self) -> usize {
        self.converter.owned_bytes()
            + self.ramp.bytes()
            + match &self.functions {
                Functions::PerComponent(fs) => fs.len() * size_of::<Arc<Function>>(),
                Functions::Identity | Functions::Single(_) => 0,
            }
    }

    /// Values per mesh vertex: one parametric value with a function, the
    /// colour components without.
    pub(super) fn values(&self) -> usize {
        if matches!(self.functions, Functions::Identity) {
            self.components
        } else {
            1
        }
    }

    pub(super) fn channels(&self) -> usize {
        self.channels
    }

    /// `function_work` units of one sample: the functions and the conversion.
    pub(super) fn cost(&self) -> usize {
        self.cost
    }

    pub(super) fn painted(&self) -> Painted {
        self.converter.painted()
    }

    /// The input values a conversion reads.
    pub(super) fn inputs(&self) -> usize {
        self.inputs
    }

    /// Whether [`Self::convert_line`] takes this stage's samples: at most
    /// [`BATCH_VALUES`] inputs and components.
    pub(super) fn batched(&self) -> bool {
        self.inputs <= BATCH_VALUES && (1..=BATCH_VALUES).contains(&self.components)
    }

    /// The colours of several samples' function inputs, `inputs` values
    /// each, into `out`, `channels` values each, kept to `precision`: as
    /// [`Self::convert`] converts them one by one, for at most
    /// [`super::BATCH`] samples and a [`Self::batched`] stage.
    pub(super) fn convert_line(&self, inputs: &[f64], out: &mut [f64], precision: Precision) {
        if precision == Precision::Level && !self.ramp.is_empty() {
            self.convert_line_certified(inputs, out);
        } else {
            self.convert_line_exact(inputs, out, precision);
        }
    }

    /// [`Self::convert_line`] at [`Precision::Level`] through the ramp,
    /// whose stage takes one input: the samples it does not certify convert
    /// exactly, together.
    fn convert_line_certified(&self, inputs: &[f64], out: &mut [f64]) {
        let channels = self.channels.max(1);
        let mut missed = [0.0; super::BATCH];
        let mut slots = [0_usize; super::BATCH];
        let mut count = 0;
        for (i, (&x, color)) in inputs
            .iter()
            .zip(out.chunks_exact_mut(channels))
            .enumerate()
        {
            if !self.ramp.convert(x, color, level_bits)
                && let (Some(input), Some(slot)) = (missed.get_mut(count), slots.get_mut(count))
            {
                (*input, *slot) = (x, i);
                count += 1;
            }
        }
        if count == 0 {
            return;
        }
        // A stage with a ramp converts into at most this many channels.
        let mut colors = [0.0; super::BATCH * pagura_color::MAX_RAMP_CHANNELS];
        let Some(colors) = colors.get_mut(..count * channels) else {
            return;
        };
        self.convert_line_exact(&missed[..count], colors, Precision::Level);
        for (&slot, color) in slots[..count].iter().zip(colors.chunks_exact(channels)) {
            if let Some(out) = out.get_mut(slot * channels..(slot + 1) * channels) {
                copy_values(out, color);
            }
        }
    }

    /// [`Self::convert_line`] by evaluation.
    fn convert_line_exact(&self, inputs: &[f64], out: &mut [f64], precision: Precision) {
        let channels = self.channels.max(1);
        let components = self.components;
        let points = out.len() / channels;
        let mut values = [0.0; super::BATCH * BATCH_VALUES];
        let Some(values) = values.get_mut(..points * components) else {
            return;
        };
        match &self.functions {
            Functions::Single(f) if f.inputs() == 1 => {
                f.evaluate_line(inputs, values, components);
            }
            _ => {
                let n = self.inputs.max(1);
                for (input, values) in inputs
                    .chunks_exact(n)
                    .zip(values.chunks_exact_mut(components))
                {
                    self.evaluate(input, values);
                }
            }
        }
        match precision {
            Precision::Exact => self.converter.convert_line(values, components, out),
            Precision::Level => {
                self.converter
                    .convert_line_rounded(values, components, out, level_bits);
            }
        }
    }

    /// The colour of one sample's function input, kept to `precision`.
    pub(super) fn convert(&self, input: &Input, out: &mut [f64], precision: Precision) {
        let mut values = [0.0; MAX_COMPONENTS];
        let values = values.get_mut(..self.components).unwrap_or_default();
        self.evaluate(input, values);
        self.finish(values, out, precision);
    }

    /// The colour components of one sample's function input.
    fn evaluate(&self, input: &[f64], values: &mut [f64]) {
        match &self.functions {
            Functions::Identity => {
                values.copy_from_slice(input.get(..values.len()).unwrap_or_default());
            }
            Functions::Single(f) => f.evaluate(arguments(input, f), values),
            Functions::PerComponent(fs) => {
                for (f, out) in fs.iter().zip(values.iter_mut()) {
                    f.evaluate(arguments(input, f), std::slice::from_mut(out));
                }
            }
        }
    }

    /// The colour of colour components, kept to `precision`.
    fn finish(&self, values: &[f64], out: &mut [f64], precision: Precision) {
        match precision {
            Precision::Exact => self.converter.convert(values, out),
            Precision::Level => self.converter.convert_rounded(values, out, level_bits),
        };
    }

    /// [`Self::convert`] of `sample.input`: the last colour again when the
    /// input values it reads are those of the last conversion.
    pub(super) fn convert_sample(&self, sample: &mut Sample, out: &mut [f64]) {
        let n = self.inputs;
        let (input, key) = (&sample.input[..n], &mut sample.key[..n]);
        let same = sample.last == Some(n)
            && input
                .iter()
                .zip(&*key)
                .all(|(a, b)| a.to_bits() == b.to_bits());
        if same && let Some(color) = sample.color.get(..out.len()) {
            copy_values(out, color);
            return;
        }
        self.convert(&sample.input, out, sample.precision);
        copy_values(key, input);
        if let Some(color) = sample.color.get_mut(..out.len()) {
            copy_values(color, out);
            sample.last = Some(n);
        }
    }

    /// The colour of explicit components, such as a `Background`.
    pub(super) fn convert_components(&self, components: &[f64]) -> Box<[f64]> {
        let mut out = vec![0.0; self.channels];
        self.converter.convert(
            components.get(..self.components).unwrap_or(components),
            &mut out,
        );
        out.into()
    }

    /// The converted colour when the functions are constant.
    pub(super) fn constant(&self, work: &mut Meter) -> Result<Option<Box<[f64]>>> {
        let mut values = [0.0; MAX_COMPONENTS];
        let Some(values) = values.get_mut(..self.components) else {
            return Ok(None);
        };
        let constant = match &self.functions {
            Functions::Identity => false,
            Functions::Single(f) => f.constant_metered(values, work)?,
            Functions::PerComponent(fs) => {
                for (f, out) in fs.iter().zip(values.iter_mut()) {
                    if !f.constant_metered(std::slice::from_mut(out), work)? {
                        return Ok(None);
                    }
                }
                true
            }
        };
        if constant {
            work.charge(self.converter.cost())?;
            Ok(Some(self.convert_components(values)))
        } else {
            Ok(None)
        }
    }
}

/// The most inputs and colour components of a sample that
/// [`Colors::convert_line`] takes.
pub(super) const BATCH_VALUES: usize = 4;

/// Samples a ramp piece must serve: building one takes about two
/// enclosures, each about [`ENCLOSURE_COST`] evaluations, which this many
/// certified samples repay several times over.
const SAMPLES_PER_PIECE: usize = 32;

/// The fewest and most pieces a ramp is cut into.
const MIN_PIECES: usize = 16;
const MAX_PIECES: usize = 4096;

/// `function_work` of one enclosure in evaluations of the stage.
const ENCLOSURE_COST: usize = 4;

/// The F32 level of a colour value, as bits: what a value kept to
/// [`Precision::Level`] must preserve.
fn level_bits(value: f64) -> u32 {
    kernel::level(value).to_bits()
}

/// The inputs a function reads from a sample.
fn arguments<'a>(input: &'a [f64], function: &Function) -> &'a [f64] {
    input.get(..function.inputs()).unwrap_or(input)
}
