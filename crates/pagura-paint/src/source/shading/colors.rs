//! A shading's colour stage: its function (or the mesh's interpolated
//! colours) and the prepared conversion into the target.

use super::{Input, PaintContext, Parts};
use pagura_color::{Converter, Function, MAX_CHANNELS, MAX_COMPONENTS, Painted};
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
}

impl Sample {
    pub(super) fn new() -> Self {
        Self {
            input: [0.0; MAX_COMPONENTS],
            last: None,
            key: [0.0; MAX_COMPONENTS],
            color: [0.0; MAX_CHANNELS],
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
        })
    }

    /// Owned conversion storage and function handles; function data stays shared.
    pub(super) fn bytes(&self) -> usize {
        self.converter.owned_bytes()
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

    /// The colour of one sample's function input.
    pub(super) fn convert(&self, input: &Input, out: &mut [f64]) {
        let mut values = [0.0; MAX_COMPONENTS];
        let values = values.get_mut(..self.components).unwrap_or_default();
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
        self.converter.convert(values, out);
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
            out.copy_from_slice(color);
            return;
        }
        self.convert(&sample.input, out);
        key.copy_from_slice(input);
        if let Some(color) = sample.color.get_mut(..out.len()) {
            color.copy_from_slice(out);
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

/// The inputs a function reads from a sample.
fn arguments<'a>(input: &'a Input, function: &Function) -> &'a [f64] {
    input.get(..function.inputs()).unwrap_or(input)
}
