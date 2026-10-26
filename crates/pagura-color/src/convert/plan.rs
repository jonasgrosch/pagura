//! The prepared form of a conversion: the source chain resolved into a flat
//! list of steps and one terminal conversion, applied to a fixed component
//! buffer without allocation.

use super::{Colorants, Conversion, Painted, Process, Target};
use crate::cie::{self, Calibrated};
use crate::function::Function;
use crate::icc::{Intent, Profile};
use crate::space::{ColorSpace, Colorant, Indexed, MAX_COMPONENTS};
use pagura_core::{Error, Meter, Name, Result};
use std::sync::Arc;

/// The most links a colour space chain may have.
const MAX_CHAIN: usize = 64;

/// The process colorant names, in CMYK channel order (8.6.6.4).
const PROCESS_NAMES: [&[u8]; 4] = [b"Cyan", b"Magenta", b"Yellow", b"Black"];

#[derive(Debug, Clone)]
pub(super) struct Plan {
    process: usize,
    separated: bool,
    /// The lookup of a top-level Indexed space.
    lookup: Option<Lookup>,
    kind: Kind,
    painted: Painted,
    /// Whether `OPM 1` zero selection applies: a DeviceCMYK chain of only
    /// range clipping into a separated CMYK target.
    zero_selection: bool,
    cost: usize,
}

/// How the (possibly looked-up) colour reaches the channels.
#[derive(Debug, Clone)]
enum Kind {
    /// Separation or DeviceN tints painted straight into their channels:
    /// `(component, channel)` pairs.
    Tints(Vec<(usize, usize)>),
    /// Separation `All`: every channel at the tint.
    All,
    /// A space that paints nothing.
    Nothing,
    /// The general conversion of the process channels.
    Chain(Chain),
}

#[derive(Debug, Clone)]
struct Chain {
    steps: Vec<Step>,
    terminal: Terminal,
}

#[derive(Debug, Clone)]
enum Step {
    /// An Indexed lookup into decoded base components.
    Lookup(Lookup),
    /// The `Range` clipping of an ICCBased space used through its
    /// alternate: four clipped components, of which `count` continue.
    Clip { range: [[f64; 2]; 4], count: usize },
    /// A tint transform; inputs are clipped to `0..=1`.
    Tint(Arc<Function>),
}

#[derive(Debug, Clone)]
struct Lookup {
    high: f64,
    count: usize,
    /// `(high + 1) × count` decoded components.
    table: Vec<f64>,
}

#[derive(Debug, Clone)]
enum Terminal {
    /// The target's own device family: clipped, complemented for CMYK.
    Native { complement: bool },
    /// Separation `All`.
    All,
    /// Encoded sRGB, encoded into the target family.
    Rgb { source: Source, encode: Encode },
}

#[derive(Debug, Clone)]
enum Source {
    Gray,
    Rgb,
    Cmyk,
    Sycc,
    /// Separation `None` and all-`None` DeviceN: `1 − tint`.
    Inverse,
    Calibrated(Arc<Calibrated>),
    Icc {
        profile: Arc<Profile>,
        intent: Intent,
        range: [[f64; 2]; 4],
        count: usize,
    },
}

/// The encoding of sRGB into a process space's channels.
#[derive(Debug, Clone)]
pub(super) enum Encode {
    Gray,
    Rgb,
    Cmyk {
        black: Option<Arc<Function>>,
        removal: Option<Arc<Function>>,
    },
}

/// Colour components in flight.
#[derive(Clone, Copy)]
struct Values {
    values: [f64; MAX_COMPONENTS],
    len: usize,
}

impl Plan {
    pub(super) fn new(
        space: &ColorSpace,
        target: &Target,
        conversion: &Conversion,
        bytes: &mut Meter,
    ) -> Result<Self> {
        let (lookup, base) = match space {
            ColorSpace::Indexed(indexed) => (Some(Lookup::new(indexed, bytes)?), &*indexed.base),
            other => (None, other),
        };
        let separated = target.colorants == Colorants::Separated;
        let process = target.process.components();
        let map = if separated && base.paints() {
            tints(base, target, bytes)?
        } else {
            None
        };
        let kind = if !base.paints() {
            Kind::Nothing
        } else if let Some(map) = map {
            Kind::Tints(map)
        } else if matches!(base, ColorSpace::Separation(s) if s.colorant == Colorant::All) {
            Kind::All
        } else {
            Kind::Chain(Chain::new(base, target.process, conversion, bytes)?)
        };
        let channels = target.channels();
        let painted = match &kind {
            Kind::Nothing => Painted(0),
            _ if !separated => Painted::all(channels),
            Kind::Tints(map) => Painted(map.iter().fold(0, |bits, &(_, channel)| {
                bits | Painted::all(channel + 1).0 & !Painted::all(channel).0
            })),
            Kind::All => Painted::all(channels),
            Kind::Chain(_) => Painted::all(process),
        };
        let zero_selection = (separated || target.colorants == Colorants::ProcessOverprint)
            && target.process == Process::Cmyk
            && matches!(&kind, Kind::Chain(chain)
                if matches!(chain.terminal, Terminal::Native { .. })
                    && chain.steps.iter().all(|s| matches!(s, Step::Clip { .. })));
        let cost = match &kind {
            Kind::Chain(chain) => chain.cost(),
            _ => 0,
        };
        Ok(Self {
            process,
            separated,
            lookup,
            kind,
            painted,
            zero_selection,
            cost,
        })
    }

    pub(super) fn cost(&self) -> usize {
        self.cost
    }

    pub(super) fn owned_bytes(&self) -> usize {
        self.lookup.as_ref().map_or(0, Lookup::owned_bytes)
            + match &self.kind {
                Kind::Tints(map) => map.capacity() * size_of::<(usize, usize)>(),
                Kind::Chain(chain) => {
                    chain.steps.capacity() * size_of::<Step>()
                        + chain
                            .steps
                            .iter()
                            .map(|step| match step {
                                Step::Lookup(lookup) => lookup.owned_bytes(),
                                Step::Clip { .. } | Step::Tint(_) => 0,
                            })
                            .sum::<usize>()
                }
                Kind::All | Kind::Nothing => 0,
            }
    }

    pub(super) fn painted(&self) -> Painted {
        self.painted
    }

    pub(super) fn arithmetic(&self) -> bool {
        match &self.kind {
            Kind::Tints(_) | Kind::All | Kind::Nothing => true,
            Kind::Chain(chain) => chain.arithmetic(),
        }
    }

    pub(super) fn convert(&self, components: &[f64], output: &mut [f64]) {
        let split = self.process.min(output.len());
        if self.lookup.is_none()
            && let Kind::Chain(chain) = &self.kind
            && chain.steps.is_empty()
        {
            // The common case: no buffer of intermediate components.
            let (process, spots) = output.split_at_mut(split);
            chain.finish(
                components.get(..MAX_COMPONENTS).unwrap_or(components),
                process,
            );
            spots.fill(1.0);
            return;
        }
        let mut values = Values::new(components);
        if let Some(lookup) = &self.lookup {
            lookup.apply(&mut values);
        }
        let (process, spots) = output.split_at_mut(split);
        match &self.kind {
            Kind::Tints(map) => {
                output.fill(1.0);
                for &(component, channel) in map {
                    if let (Some(out), Some(&tint)) =
                        (output.get_mut(channel), values.slice().get(component))
                    {
                        *out = 1.0 - tint.clamp(0.0, 1.0);
                    }
                }
            }
            Kind::All => {
                let value = all(values.slice());
                process.fill(value);
                spots.fill(if self.separated { value } else { 1.0 });
            }
            Kind::Nothing => output.fill(1.0),
            Kind::Chain(chain) => {
                chain.apply(values, process);
                spots.fill(1.0);
            }
        }
    }

    /// As [`Self::convert`], for a caller that keeps only `round` of each
    /// value: a conversion that ends in encoded sRGB, written as RGB without
    /// any step before it, takes the transfer function's power only where
    /// [`cie::encode_bounds`] does not decide the rounding, and writes the
    /// lower bound otherwise.
    pub(super) fn convert_rounded(
        &self,
        components: &[f64],
        output: &mut [f64],
        round: impl Fn(f64) -> u32,
    ) {
        let linear = match &self.kind {
            Kind::Chain(chain) if self.lookup.is_none() && chain.steps.is_empty() => {
                chain.linear(components.get(..MAX_COMPONENTS).unwrap_or(components))
            }
            _ => None,
        };
        let Some(linear) = linear else {
            self.convert(components, output);
            return;
        };
        let (process, spots) = output.split_at_mut(self.process.min(output.len()));
        for (out, v) in process.iter_mut().zip(linear) {
            *out = encode_rounded(v, &round);
        }
        spots.fill(1.0);
    }

    /// Converts colours of `count` components each into `output`,
    /// `channels` values each, as [`Self::convert`] does colour by colour,
    /// or with `round` as [`Self::convert_rounded`] does. An ICC conversion
    /// into RGB without a step before it is applied a stage at a time over
    /// the whole line.
    pub(super) fn convert_line<R: Fn(f64) -> u32>(
        &self,
        (components, count): (&[f64], usize),
        (output, channels): (&mut [f64], usize),
        round: Option<&R>,
    ) {
        const CHUNK: usize = 32;
        let (count, channels) = (count.max(1), channels.max(1));
        let icc = match &self.kind {
            Kind::Chain(chain) if self.lookup.is_none() && chain.steps.is_empty() => {
                match &chain.terminal {
                    Terminal::Rgb {
                        source:
                            Source::Icc {
                                profile,
                                intent,
                                range,
                                count: used,
                            },
                        encode: Encode::Rgb,
                    } => Some((profile, *intent, range, *used)),
                    _ => None,
                }
            }
            _ => None,
        };
        let Some((profile, intent, range, used)) = icc else {
            for (components, output) in components
                .chunks_exact(count)
                .zip(output.chunks_exact_mut(channels))
            {
                match round {
                    Some(round) => self.convert_rounded(components, output, round),
                    None => self.convert(components, output),
                }
            }
            return;
        };
        let mut clipped = [0.0; CHUNK * 4];
        let mut linear = [[0.0; 3]; CHUNK];
        for (components, output) in components
            .chunks(CHUNK.saturating_mul(count))
            .zip(output.chunks_mut(CHUNK * channels))
        {
            let mut len = 0;
            for (components, clipped) in components
                .chunks_exact(count)
                .zip(clipped.chunks_exact_mut(used.max(1)))
            {
                for (clipped, value) in clipped.iter_mut().zip(clip_icc(components, range)) {
                    *clipped = value;
                }
                len += 1;
            }
            let linear = &mut linear[..len];
            profile.linear_line(
                &clipped[..len * used],
                used,
                intent,
                linear.as_flattened_mut(),
            );
            for (linear, output) in linear.iter().zip(output.chunks_exact_mut(channels)) {
                let (process, spots) = output.split_at_mut(self.process.min(output.len()));
                for (out, &v) in process.iter_mut().zip(linear) {
                    *out = match round {
                        Some(round) => encode_rounded(v, round),
                        None => cie::encode(v),
                    };
                }
                spots.fill(1.0);
            }
        }
    }

    pub(super) fn nonzero(&self, components: &[f64]) -> Painted {
        let Kind::Chain(chain) = &self.kind else {
            return self.painted;
        };
        if !self.zero_selection {
            return self.painted;
        }
        let mut values = Values::new(components);
        if let Some(lookup) = &self.lookup {
            lookup.apply(&mut values);
        }
        for step in &chain.steps {
            step.apply(&mut values);
        }
        let mut painted = self.painted.0;
        for (i, &v) in values.slice().iter().enumerate().take(self.process) {
            if v.clamp(0.0, 1.0) == 0.0 {
                painted &= !(1 << i);
            }
        }
        Painted(painted)
    }
}

/// The encoding of linear value `v`, or where `round` of it is all that
/// matters, a value that rounds alike, found without the transfer function's
/// power where its bounds decide the rounding.
fn encode_rounded(v: f64, round: &impl Fn(f64) -> u32) -> f64 {
    match cie::encode_bounds(v) {
        Some([low, high]) if round(low) == round(high) => low,
        _ => cie::encode(v),
    }
}

/// The channel map of a Separation or DeviceN space whose colorants a
/// separated target paints directly, or `None` when it converts through its
/// alternate: a process colorant name in a Gray or RGB target does.
fn tints(
    space: &ColorSpace,
    target: &Target,
    bytes: &mut Meter,
) -> Result<Option<Vec<(usize, usize)>>> {
    let colorants: &[Colorant] = match space {
        ColorSpace::Separation(s) if matches!(s.colorant, Colorant::Named(_)) => {
            std::slice::from_ref(&s.colorant)
        }
        ColorSpace::DeviceN(d) => &d.colorants,
        _ => return Ok(None),
    };
    let cmyk = target.process == Process::Cmyk;
    let process_name = |name: &Name| PROCESS_NAMES.iter().position(|&n| name == n);
    if !cmyk
        && colorants
            .iter()
            .any(|c| matches!(c, Colorant::Named(name) if process_name(name).is_some()))
    {
        return Ok(None);
    }
    let process = target.process.components();
    let channel = |name: &Name| {
        process_name(name).filter(|_| cmyk).or_else(|| {
            target
                .spots
                .iter()
                .position(|spot| spot == name)
                .map(|j| process + j)
        })
    };
    let mapped = || {
        colorants.iter().enumerate().filter_map(|(i, c)| match c {
            Colorant::Named(name) => channel(name).map(|channel| (i, channel)),
            Colorant::All | Colorant::None => None,
        })
    };
    let mut map = Vec::new();
    bytes.reserve(&mut map, mapped().count())?;
    map.extend(mapped());
    Ok(Some(map))
}

/// Appends one owned chain step, charging the overlap of its old and new
/// buffers before a possible reallocation.
fn push<T>(values: &mut Vec<T>, value: T, bytes: &mut Meter) -> Result<()> {
    if values.len() == values.capacity() {
        let old = values.capacity().saturating_mul(size_of::<T>());
        bytes.charge(old)?;
        let reserved = bytes.reserve(values, 1);
        bytes.refund(old);
        reserved?;
    }
    values.push(value);
    Ok(())
}

impl Chain {
    /// Follows `space` to the terminal conversion into `process`.
    fn new(
        mut space: &ColorSpace,
        process: Process,
        conversion: &Conversion,
        bytes: &mut Meter,
    ) -> Result<Self> {
        let mut steps = Vec::new();
        let encode = || Encode::new(process, conversion);
        let rgb = |source| Terminal::Rgb {
            source,
            encode: encode(),
        };
        for _ in 0..MAX_CHAIN {
            let device = match space {
                ColorSpace::DeviceGray => Some((Process::Gray, Source::Gray)),
                ColorSpace::DeviceRgb => Some((Process::Rgb, Source::Rgb)),
                ColorSpace::DeviceCmyk => Some((Process::Cmyk, Source::Cmyk)),
                _ => None,
            };
            if let Some((family, source)) = device {
                let terminal = if family == process {
                    Terminal::Native {
                        complement: process == Process::Cmyk,
                    }
                } else {
                    rgb(source)
                };
                return Ok(Self { steps, terminal });
            }
            let terminal = match space {
                ColorSpace::Sycc => rgb(Source::Sycc),
                ColorSpace::Calibrated(space) => rgb(Source::Calibrated(Arc::clone(space))),
                ColorSpace::Icc(icc) => {
                    let range = icc_range(icc.range)?;
                    let count = usize::from(icc.components).min(4);
                    let Some(profile) = &icc.profile else {
                        push(&mut steps, Step::Clip { range, count }, bytes)?;
                        space = &icc.alternate;
                        continue;
                    };
                    profile
                        .check(conversion.intent)
                        .map_err(|error| error.in_object_opt(icc.object))?;
                    rgb(Source::Icc {
                        profile: Arc::clone(profile),
                        intent: conversion.intent,
                        range,
                        count,
                    })
                }
                ColorSpace::Indexed(indexed) => {
                    let lookup = Lookup::new(indexed, bytes)?;
                    push(&mut steps, Step::Lookup(lookup), bytes)?;
                    space = &indexed.base;
                    continue;
                }
                ColorSpace::Separation(s) => match s.colorant {
                    Colorant::All => Terminal::All,
                    Colorant::None => rgb(Source::Inverse),
                    Colorant::Named(_) => {
                        push(&mut steps, Step::Tint(Arc::clone(&s.tint)), bytes)?;
                        space = &s.alternate;
                        continue;
                    }
                },
                ColorSpace::DeviceN(d) => {
                    if d.colorants.iter().all(|c| *c == Colorant::None) {
                        rgb(Source::Inverse)
                    } else {
                        push(&mut steps, Step::Tint(Arc::clone(&d.tint)), bytes)?;
                        space = &d.alternate;
                        continue;
                    }
                }
                ColorSpace::Pattern(_) => {
                    return Err(Error::invalid_argument(
                        "pattern is not a component colour space",
                    ));
                }
                ColorSpace::DeviceGray | ColorSpace::DeviceRgb | ColorSpace::DeviceCmyk => {
                    return Err(Error::invalid_argument("device space without a family"));
                }
            };
            return Ok(Self { steps, terminal });
        }
        Err(Error::limit("colour space nesting exceeded"))
    }

    fn cost(&self) -> usize {
        let tints = self.steps.iter().fold(0_usize, |cost, step| match step {
            Step::Tint(function) => cost.saturating_add(function.cost()),
            Step::Lookup(_) | Step::Clip { .. } => cost,
        });
        let encode = match &self.terminal {
            Terminal::Rgb { encode, .. } => encode.cost(),
            Terminal::Native { .. } | Terminal::All => 0,
        };
        tints.saturating_add(encode)
    }

    /// Whether no step and no terminal conversion evaluates a function or
    /// a CIE or ICC transform.
    fn arithmetic(&self) -> bool {
        let terminal = match &self.terminal {
            Terminal::Native { .. } | Terminal::All => true,
            Terminal::Rgb { source, encode } => {
                !matches!(source, Source::Calibrated(_) | Source::Icc { .. })
                    && !matches!(encode, Encode::Cmyk { black, removal }
                        if black.is_some() || removal.is_some())
            }
        };
        terminal && !self.steps.iter().any(|step| matches!(step, Step::Tint(_)))
    }

    /// The gamut-clipped linear sRGB that [`Self::finish`] encodes and
    /// writes as RGB, or `None` when it ends otherwise.
    fn linear(&self, values: &[f64]) -> Option<[f64; 3]> {
        match &self.terminal {
            Terminal::Rgb {
                source,
                encode: Encode::Rgb,
            } => source.linear(values),
            _ => None,
        }
    }

    /// Converts `values` into the process channels `output`.
    fn apply(&self, mut values: Values, output: &mut [f64]) {
        for step in &self.steps {
            step.apply(&mut values);
        }
        self.finish(values.slice(), output);
    }

    /// The terminal conversion of the chain's last components.
    fn finish(&self, values: &[f64], output: &mut [f64]) {
        match &self.terminal {
            Terminal::Native { complement } => {
                for (i, out) in output.iter_mut().enumerate() {
                    *out = values.get(i).map_or(0.0, |&v| {
                        let v = v.clamp(0.0, 1.0);
                        if *complement { 1.0 - v } else { v }
                    });
                }
            }
            Terminal::All => output.fill(all(values)),
            Terminal::Rgb { source, encode } => encode.apply(source.rgb(values), output),
        }
    }
}

impl Step {
    fn apply(&self, values: &mut Values) {
        match self {
            Self::Lookup(lookup) => lookup.apply(values),
            Self::Clip { range, count } => {
                let clipped = clip_icc(values.slice(), range);
                *values = Values::new(clipped.get(..*count).unwrap_or(&[]));
            }
            Self::Tint(function) => {
                let mut input = [0.0; MAX_COMPONENTS];
                for (input, &v) in input.iter_mut().zip(values.slice()) {
                    *input = v.clamp(0.0, 1.0);
                }
                let input = input.get(..values.len).unwrap_or(&[]);
                let mut next = Values {
                    values: [0.0; MAX_COMPONENTS],
                    len: function.outputs().min(MAX_COMPONENTS),
                };
                let len = next.len;
                function.evaluate(input, next.values.get_mut(..len).unwrap_or_default());
                *values = next;
            }
        }
    }
}

impl Lookup {
    fn new(indexed: &Indexed, budget: &mut Meter) -> Result<Self> {
        let count = indexed.base.components();
        if count > MAX_COMPONENTS {
            return Err(Error::limit("Indexed base has too many components"));
        }
        let entries = usize::from(indexed.high) + 1;
        let mut table = Vec::new();
        budget.reserve(&mut table, entries * count)?;
        let mut bytes = [0_u8; MAX_COMPONENTS];
        for entry in 0..entries {
            for (i, byte) in bytes.iter_mut().enumerate().take(count) {
                *byte = indexed.lookup.get(entry * count + i).copied().unwrap_or(0);
            }
            decode_lookup(&indexed.base, bytes.get(..count).unwrap_or(&[]), &mut table);
        }
        Ok(Self {
            high: f64::from(indexed.high),
            count,
            table,
        })
    }

    fn owned_bytes(&self) -> usize {
        self.table.capacity() * size_of::<f64>()
    }

    /// Replaces the index with its entry's components (8.6.6.3).
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "the index is rounded and clipped to 0..=255"
    )]
    fn apply(&self, values: &mut Values) {
        let index = values.slice().first().copied().unwrap_or(0.0);
        let index = index.round().clamp(0.0, self.high) as usize;
        let start = index * self.count;
        *values = Values::new(self.table.get(start..start + self.count).unwrap_or(&[]));
    }
}

/// Maps lookup bytes to the base space's component range (8.6.6.3).
fn decode_lookup(base: &ColorSpace, bytes: &[u8], out: &mut Vec<f64>) {
    let n = |i: usize| f64::from(bytes.get(i).copied().unwrap_or(0)) / 255.0;
    match base {
        ColorSpace::Calibrated(space) => match (space.components(), space.lab_range()) {
            (_, Some(r)) => out.extend([
                100.0 * n(0),
                r[0] + n(1) * (r[1] - r[0]),
                r[2] + n(2) * (r[3] - r[2]),
            ]),
            (1, None) => out.push(n(0)),
            _ => out.extend([n(0), n(1), n(2)]),
        },
        ColorSpace::Icc(icc) => out.extend(bytes.iter().zip(icc.range).map(|(&v, [low, high])| {
            low * (1.0 - f64::from(v) / 255.0) + high * (f64::from(v) / 255.0)
        })),
        _ => out.extend(bytes.iter().map(|&v| f64::from(v) / 255.0)),
    }
}

/// An ICCBased range, which must be ordered.
fn icc_range(range: [[f64; 2]; 4]) -> Result<[[f64; 2]; 4]> {
    if range.iter().all(|&[low, high]| low <= high) {
        Ok(range)
    } else {
        Err(Error::syntax("reversed ICC Range"))
    }
}

/// PDF Table 65 constrains source values before any alternate substitution.
fn clip_icc(components: &[f64], range: &[[f64; 2]; 4]) -> [f64; 4] {
    std::array::from_fn(|i| {
        let [low, high] = range[i];
        components.get(i).copied().unwrap_or(0.0).clamp(low, high)
    })
}

impl Source {
    /// The gamut-clipped linear sRGB of `values` for the colorimetric
    /// sources, which [`Self::rgb`] encodes.
    fn linear(&self, slice: &[f64]) -> Option<[f64; 3]> {
        match self {
            Self::Calibrated(space) => Some(space.linear(slice)),
            Self::Icc {
                profile,
                intent,
                range,
                count,
            } => {
                let clipped = clip_icc(slice, range);
                Some(profile.linear_total(clipped.get(..*count).unwrap_or(&[]), *intent))
            }
            Self::Gray | Self::Rgb | Self::Cmyk | Self::Sycc | Self::Inverse => None,
        }
    }

    /// Encoded sRGB of `values`.
    fn rgb(&self, slice: &[f64]) -> [f64; 3] {
        let n = |i: usize| slice.get(i).copied().unwrap_or(0.0).clamp(0.0, 1.0);
        match self {
            Self::Gray => [n(0); 3],
            Self::Rgb => [n(0), n(1), n(2)],
            Self::Sycc => {
                // T.800 I.5.3.3 / Table I.10 maps each unsigned channel to
                // 0..255 before IEC Annex F.15/F.16. `n` is already divided
                // by that channel's maximum code, even for mixed precision;
                // the chroma offset therefore remains 128/255, not a
                // precision-dependent half-code. Gamut clipping is the same
                // output policy used by the other source colour models;
                // rounding waits until the final renderer quantization.
                let y = n(0);
                let cb = n(1) - 128.0 / 255.0;
                let cr = n(2) - 128.0 / 255.0;
                [
                    y + 1.4020 * cr,
                    y - 0.3441 * cb - 0.7141 * cr,
                    y + 1.7720 * cb,
                ]
                .map(|value| value.clamp(0.0, 1.0))
            }
            Self::Cmyk => {
                let k = n(3);
                [
                    1.0 - (n(0) + k).min(1.0),
                    1.0 - (n(1) + k).min(1.0),
                    1.0 - (n(2) + k).min(1.0),
                ]
            }
            Self::Inverse => [1.0 - n(0); 3],
            Self::Calibrated(space) => space.convert(slice),
            Self::Icc {
                profile,
                intent,
                range,
                count,
            } => {
                let clipped = clip_icc(slice, range);
                profile.convert_total(clipped.get(..*count).unwrap_or(&[]), *intent)
            }
        }
    }
}

impl Encode {
    /// The encoding into `process`, with the black generation and
    /// undercolour removal of `conversion` for CMYK.
    pub(super) fn new(process: Process, conversion: &Conversion) -> Self {
        match process {
            Process::Gray => Self::Gray,
            Process::Rgb => Self::Rgb,
            Process::Cmyk => Self::Cmyk {
                black: conversion.black_generation.clone(),
                removal: conversion.undercolor_removal.clone(),
            },
        }
    }

    /// The `function_work` units one encoding costs: black generation and
    /// undercolour removal.
    pub(super) fn cost(&self) -> usize {
        match self {
            Self::Cmyk { black, removal } => [black, removal]
                .into_iter()
                .flatten()
                .fold(0_usize, |cost, f| cost.saturating_add(f.cost())),
            Self::Gray | Self::Rgb => 0,
        }
    }

    /// Writes encoded sRGB into the process channels (10.3).
    pub(super) fn apply(&self, rgb: [f64; 3], output: &mut [f64]) {
        let encoded = match self {
            Self::Rgb => [rgb[0], rgb[1], rgb[2], 0.0],
            Self::Gray => [luminosity(rgb), 0.0, 0.0, 0.0],
            Self::Cmyk { black, removal } => {
                // Default black generation and undercolour removal both use
                // the common ink amount (10.3.4).
                let inks = rgb.map(|v| 1.0 - v);
                let amount = inks.into_iter().fold(1.0, f64::min);
                let black = scalar(black.as_deref(), amount).clamp(0.0, 1.0);
                let removal = scalar(removal.as_deref(), amount).clamp(-1.0, 1.0);
                [
                    1.0 - (inks[0] - removal).clamp(0.0, 1.0),
                    1.0 - (inks[1] - removal).clamp(0.0, 1.0),
                    1.0 - (inks[2] - removal).clamp(0.0, 1.0),
                    1.0 - black,
                ]
            }
        };
        for (out, value) in output.iter_mut().zip(encoded) {
            *out = value;
        }
    }
}

/// The luminosity of an RGB colour (11.3.5.3).
pub fn luminosity([r, g, b]: [f64; 3]) -> f64 {
    0.3 * r + 0.59 * g + 0.11 * b
}

/// `function(value)`, or `value` without a function.
fn scalar(function: Option<&Function>, value: f64) -> f64 {
    let mut result = [value];
    if let Some(function) = function {
        function.evaluate(&[value], &mut result);
    }
    result[0]
}

impl Values {
    fn new(components: &[f64]) -> Self {
        let mut values = [0.0; MAX_COMPONENTS];
        let len = components.len().min(MAX_COMPONENTS);
        for (out, &v) in values.iter_mut().zip(components) {
            *out = v;
        }
        Self { values, len }
    }

    fn slice(&self) -> &[f64] {
        self.values.get(..self.len).unwrap_or(&[])
    }
}

/// `1 − tint` of Separation `All`; a missing tint is full.
fn all(values: &[f64]) -> f64 {
    1.0 - values.first().copied().unwrap_or(1.0).clamp(0.0, 1.0)
}
