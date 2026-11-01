//! ICC.1:2010 source transforms for PDF ICCBased spaces (ISO 32000-2 8.6.5.5).
//!
//! Forward Gray/RGB matrix/TRC and mft1/mft2/mAB pipelines use D50 PCS.
//! Bradford adaptation to D65, sRGB encoding, gamut clipping and final byte
//! rounding are output-device policy. Inverse BToA transforms are never used.

mod clut;
mod curves;
mod format;
mod lut;
mod mab;

use crate::affine::{Affine, Undecided};
use crate::cie::{self, D50};
use crate::features;
use curves::Curve;
use pagura_core::bytes::Cursor;
use pagura_core::{Error, Limits, Meter, Result};
use std::sync::Arc;

/// A rendering intent (8.6.5.8, Table 69). The standard closes the set; an
/// unknown name selects relative colorimetric.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Intent {
    /// Perceptual.
    Perceptual,
    /// Relative colorimetric, the default.
    #[default]
    Relative,
    /// Saturation.
    Saturation,
    /// Absolute colorimetric.
    Absolute,
}
impl Intent {
    /// The intent a PDF name selects.
    pub fn from_name(name: &[u8]) -> Self {
        match name {
            b"Perceptual" => Self::Perceptual,
            b"Saturation" => Self::Saturation,
            b"AbsoluteColorimetric" => Self::Absolute,
            _ => Self::Relative,
        }
    }
    fn index(self) -> usize {
        match self {
            Self::Perceptual => 0,
            Self::Saturation => 2,
            Self::Relative | Self::Absolute => 1,
        }
    }
}

/// Storage and work admitted while parsing one profile (ADR 0016): decoded
/// table values cost one `decode_work` unit and one `f64` of `decode_bytes`.
struct Budget {
    work: Meter,
    bytes: Meter,
}
impl Budget {
    fn new(limits: &Limits) -> Self {
        Self {
            work: Meter::new(limits.decode_work, "ICC profile work budget exceeded"),
            bytes: Meter::new(limits.decode_bytes, "ICC transform storage budget exceeded"),
        }
    }
    fn values(&mut self, count: usize) -> Result<()> {
        self.work.charge(count)?;
        self.bytes.charge_items::<f64>(count)
    }
}
/// A parsed ICC profile's forward (device to PCS) transforms.
#[derive(Debug, Clone)]
pub struct Profile {
    transforms: [Option<Arc<Forward>>; 3],
    model: Option<Forward>,
    lab_input: bool,
    white: [f64; 3],
}
#[derive(Debug, Clone)]
enum Forward {
    Gray {
        curve: Curve,
        pcs: Pcs,
    },
    Matrix {
        matrix: cie::Matrix,
        curves: [Curve; 3],
    },
    Lut(lut::Lut, Encoding),
    Mab(mab::Mab, Encoding),
    Unsupported(Error),
}
/// The profile connection space of a TRC-only model.
#[derive(Debug, Clone, Copy)]
enum Pcs {
    Xyz,
    Lab,
}

impl Pcs {
    /// The PCS XYZ of a gray model's curve value.
    fn xyz(self, value: f64) -> [f64; 3] {
        match self {
            Self::Lab => cie::lab_to_xyz([value * 100.0, 0.0, 0.0], D50),
            Self::Xyz => D50.map(|w| w * value),
        }
    }
}
#[derive(Debug, Clone, Copy)]
enum Encoding {
    Xyz,
    Lab,
    LegacyLab,
}
impl Profile {
    /// Estimated bytes this profile retains.
    pub fn retained_bytes(&self) -> usize {
        let mut total = size_of::<Self>();
        for (i, transform) in self.transforms.iter().enumerate() {
            if let Some(transform) = transform
                && !self.transforms[..i]
                    .iter()
                    .flatten()
                    .any(|v| Arc::ptr_eq(v, transform))
            {
                total = total
                    .saturating_add(size_of::<Forward>())
                    .saturating_add(transform.retained_bytes());
            }
        }
        total.saturating_add(self.model.as_ref().map_or(0, Forward::retained_bytes))
    }
    /// Parses a profile for a source space of `components` channels.
    /// `Ok(None)` means a later profile version that callers replace with
    /// the alternate space. Error positions are offsets in `data`.
    ///
    /// # Errors
    /// Returns `Syntax` for malformed profiles, `Limit` when parsing exceeds
    /// `decode_work` or `decode_bytes`, and `Unsupported` for profile
    /// versions outside the supported subset.
    pub fn parse(data: &[u8], components: usize, limits: &Limits) -> Result<Option<Self>> {
        let profile = Cursor::new(data).with_message("truncated ICC profile");
        let mut budget = Budget::new(limits);
        let tags = format::header(&profile, components, &mut budget)?;
        let version = profile.bytes(8, 2)?;
        let (major, minor) = (version[0], version[1] >> 4);
        if major > 4 || (major == 4 && minor > 3) {
            return Ok(None);
        }
        if !((major == 2 && minor <= 4) || (major == 4 && minor <= 3)) {
            return Err(unsupported("unsupported ICC profile version"));
        }
        let white = format::model_tags(&profile, &tags, &mut budget)?;
        let lab = profile.bytes(20, 4)? == b"Lab ";
        let model = matrix_model(&profile, &tags, &mut budget)?;
        let mut transforms: [Option<Arc<Forward>>; 3] = [None, None, None];
        let names = [b"A2B0", b"A2B1", b"A2B2"];
        for (i, name) in names.iter().enumerate() {
            if let Some(tag) = tags.get(*name) {
                if let Some(j) =
                    (0..i).find(|&j| tags.get(names[j]).map(Cursor::data) == Some(tag.data()))
                {
                    let (previous, next) = transforms.split_at_mut(i);
                    next[0].clone_from(&previous[j]);
                } else {
                    transforms[i] =
                        Some(Arc::new(Forward::parse(tag, components, lab, &mut budget)?));
                }
            }
        }
        if model.is_none() && transforms[0].is_none() {
            return Err(Error::syntax("ICC profile has no forward model"));
        }
        Ok(Some(Self {
            transforms,
            model,
            lab_input: profile.bytes(16, 4)? == b"Lab ",
            white,
        }))
    }
    /// Whether the profile's data colour space is CIELAB.
    pub fn lab_input(&self) -> bool {
        self.lab_input
    }
    /// Converts source components to encoded sRGB.
    ///
    /// # Errors
    /// Returns `Unsupported` when the selected transform is outside the
    /// supported subset, and `Syntax` for undefined curve arithmetic.
    pub fn convert(&self, inputs: &[f64], intent: Intent) -> Result<[f64; 3]> {
        let transform = self.forward(intent)?;
        let mut arithmetic = Arithmetic::default();
        let rgb = self.evaluate(transform, inputs, intent, &mut arithmetic);
        if arithmetic.undefined {
            return Err(Error::syntax("undefined ICC curve arithmetic"));
        }
        Ok(rgb)
    }

    /// Checks that `intent` selects a supported forward transform, so that
    /// [`Self::convert_total`] converts as [`Self::convert`] does.
    pub(crate) fn check(&self, intent: Intent) -> Result<()> {
        self.forward(intent).map(|_| ())
    }

    /// Converts as [`Self::convert`] does, with undefined curve arithmetic
    /// giving 0 instead of an error; black when the intent's transform is
    /// unsupported, which [`Self::check`] rules out.
    pub(crate) fn convert_total(&self, inputs: &[f64], intent: Intent) -> [f64; 3] {
        self.linear_total(inputs, intent).map(cie::encode)
    }

    /// The gamut-clipped linear sRGB that [`Self::convert_total`] encodes.
    pub(crate) fn linear_total(&self, inputs: &[f64], intent: Intent) -> [f64; 3] {
        self.forward(intent).map_or([0.0; 3], |transform| {
            cie::linear_srgb(self.xyz(transform, inputs, intent, &mut Arithmetic::default()))
        })
    }

    /// The inputs the transform for `intent` reads where it is a gray (1) or
    /// matrix/TRC (3) model of device inputs, whose first stage takes each
    /// input on its own ([`Self::curve`]); `None` for other transforms.
    pub(crate) fn curved_inputs(&self, intent: Intent) -> Option<usize> {
        match self.forward(intent).ok()? {
            _ if self.lab_input => None,
            Forward::Gray { .. } => Some(1),
            Forward::Matrix { .. } => Some(3),
            _ => None,
        }
    }

    /// The stage of [`Self::linear_total`] that takes each input on its
    /// own, where the transform is a gray or matrix/TRC model of device
    /// inputs: input `i` clamped to `0..=1` and through its curve, with
    /// undefined curve arithmetic giving 0 as there. `None` for other
    /// transforms, and for an input the transform does not read.
    pub(crate) fn curve(&self, i: usize, input: f64, intent: Intent) -> Option<f64> {
        if self.lab_input {
            return None;
        }
        let curve = match self.forward(intent).ok()? {
            Forward::Gray { curve, .. } if i == 0 => curve,
            Forward::Matrix { curves, .. } => curves.get(i)?,
            _ => return None,
        };
        Some(curve.evaluate(input.clamp(0.0, 1.0), &mut Arithmetic::default()))
    }

    /// [`Self::linear_total`] of inputs already through [`Self::curve`],
    /// `curved` holding one value for each input the transform reads.
    pub(crate) fn linear_curved(&self, curved: &[f64], intent: Intent) -> [f64; 3] {
        let value = |i: usize| curved.get(i).copied().unwrap_or(0.0);
        let xyz = match self.forward(intent) {
            Ok(Forward::Gray { pcs, .. }) => pcs.xyz(value(0)),
            Ok(Forward::Matrix { matrix, .. }) => cie::map(*matrix, [value(0), value(1), value(2)]),
            _ => return [0.0; 3],
        };
        cie::linear_srgb(self.relative(xyz, intent))
    }

    /// [`Self::linear_curved`] of a matrix/TRC model resolved once: its
    /// matrix and, under the absolute intent, the factor each XYZ value is
    /// scaled by ([`Self::relative`]). `None` for other transforms.
    pub(crate) fn curved_matrix(&self, intent: Intent) -> Option<(cie::Matrix, Option<[f64; 3]>)> {
        let Ok(Forward::Matrix { matrix, .. }) = self.forward(intent) else {
            return None;
        };
        let scale =
            (intent == Intent::Absolute).then(|| std::array::from_fn(|i| self.white[i] / D50[i]));
        Some((*matrix, scale))
    }

    /// [`Self::linear_total`] of several colours, `count` components each,
    /// into `out`, three values each. A matrix/TRC model is applied a stage
    /// at a time: each curve over every colour, then the matrices colour by
    /// colour, with the same arithmetic as one colour at a time.
    pub(crate) fn linear_line(
        &self,
        inputs: &[f64],
        count: usize,
        intent: Intent,
        out: &mut [f64],
    ) {
        const CHUNK: usize = 32;
        let count = count.max(1);
        let Ok(transform) = self.forward(intent) else {
            out.as_chunks_mut::<3>().0.fill([0.0; 3]);
            return;
        };
        let batch = crate::batch::Batch::new((inputs, count), (out, 3));
        let arithmetic = &mut Arithmetic::default();
        let staged = !self.lab_input && intent != Intent::Absolute;
        let (Forward::Matrix { matrix, curves }, true) = (transform, staged) else {
            for (inputs, out) in batch.points() {
                out.copy_from_slice(&cie::linear_srgb(
                    self.xyz(transform, inputs, intent, arithmetic),
                ));
            }
            return;
        };
        let mut values = [[0.0; CHUNK]; 3];
        for chunk in batch.chunks(CHUNK) {
            let (inputs, out) = chunk.parts();
            let out = out.as_chunks_mut::<3>().0;
            let len = out.len();
            for (i, inputs) in inputs.chunks_exact(count).enumerate() {
                for (channel, values) in values.iter_mut().enumerate() {
                    values[i] = inputs.get(channel).map_or(0.0, |&v| v.clamp(0.0, 1.0));
                }
            }
            for (curve, values) in curves.iter().zip(&mut values) {
                curve.evaluate_line(&mut values[..len], arithmetic);
            }
            for (i, out) in out.iter_mut().enumerate() {
                let xyz = cie::map(*matrix, [values[0][i], values[1][i], values[2][i]]);
                *out = cie::linear_srgb(cie::map(cie::D50_TO_D65, xyz));
            }
        }
    }

    /// Encloses what [`Self::linear_total`] computes for every value of
    /// `inputs` (see [`crate::affine`]), for a matrix/TRC model with
    /// identity or sampled curves and a source that is not CIELAB.
    pub(crate) fn enclose_linear(
        &self,
        inputs: &[Affine],
        intent: Intent,
    ) -> std::result::Result<[Affine; 3], Undecided> {
        let (Ok(Forward::Matrix { matrix, curves }), false) =
            (self.forward(intent), self.lab_input)
        else {
            return Err(Undecided::Unsupported);
        };
        let mut linear = [Affine::point(0.0); 3];
        for (i, (out, curve)) in linear.iter_mut().zip(curves).enumerate() {
            let value = match inputs.get(i) {
                Some(input) => input.clamp(0.0, 1.0)?,
                None => Affine::point(0.0),
            };
            *out = curve.enclose(value)?;
        }
        let mut xyz = cie::enclose_map(*matrix, linear);
        if intent == Intent::Absolute {
            for (i, value) in xyz.iter_mut().enumerate() {
                *value = value.scale(self.white[i] / D50[i]);
            }
        }
        cie::enclose_linear_srgb(cie::enclose_map(cie::D50_TO_D65, xyz))
    }

    /// The forward transform `intent` selects (ICC 8.10.2): the designated
    /// AToB, then AToB0, then the matrix/TRC model. Unsupported DToB
    /// elements may be bypassed at step (a); BToA and the header's preferred
    /// intent are ignored for PDF source spaces.
    fn forward(&self, intent: Intent) -> Result<&Forward> {
        let transform = self.transforms[intent.index()]
            .as_deref()
            .or(self.transforms[0].as_deref())
            .or(self.model.as_ref())
            .ok_or(Error::syntax(
                "ICC profile has no applicable forward transform",
            ))?;
        if let Forward::Unsupported(error) = transform {
            return Err(error.clone());
        }
        Ok(transform)
    }

    fn evaluate(
        &self,
        transform: &Forward,
        inputs: &[f64],
        intent: Intent,
        arithmetic: &mut Arithmetic,
    ) -> [f64; 3] {
        cie::xyz_to_srgb(self.xyz(transform, inputs, intent, arithmetic))
    }

    /// The D65-relative XYZ of source components.
    fn xyz(
        &self,
        transform: &Forward,
        inputs: &[f64],
        intent: Intent,
        arithmetic: &mut Arithmetic,
    ) -> [f64; 3] {
        let values = std::array::from_fn(|i| {
            inputs.get(i).map_or(0.0, |&input| {
                if self.lab_input {
                    if i == 0 {
                        input.clamp(0.0, 100.0) / 100.0
                    } else {
                        (input.clamp(-128.0, 127.0) + 128.0) / 255.0
                    }
                } else {
                    input.clamp(0.0, 1.0)
                }
            })
        });
        self.relative(transform.evaluate(values, arithmetic), intent)
    }

    /// D65-relative XYZ from the transform's PCS XYZ.
    fn relative(&self, mut xyz: [f64; 3], intent: Intent) -> [f64; 3] {
        if intent == Intent::Absolute {
            // ICC 6.3.2: recover media-absolute colorimetry from relative PCS.
            for (i, value) in xyz.iter_mut().enumerate() {
                *value *= self.white[i] / D50[i];
            }
        }
        cie::map(cie::D50_TO_D65, xyz)
    }
}

/// Undefined arithmetic seen while evaluating one transform.
#[derive(Debug, Default)]
struct Arithmetic {
    undefined: bool,
}
impl Forward {
    fn parse(tag: &Cursor<'_>, inputs: usize, lab: bool, budget: &mut Budget) -> Result<Self> {
        let signature = tag.bytes(0, 4)?;
        let encoding = if lab {
            if signature == b"mft2" {
                Encoding::LegacyLab
            } else {
                Encoding::Lab
            }
        } else {
            Encoding::Xyz
        };
        Ok(match signature {
            b"mft1" if !lab => {
                Self::Unsupported(unsupported("ICC lut8 XYZ PCS encoding is unspecified"))
            }
            b"mft1" | b"mft2" => Self::Lut(lut::Lut::parse(tag, inputs, budget)?, encoding),
            b"mAB " => Self::Mab(mab::Mab::parse(tag, inputs, budget)?, encoding),
            b"mBA " => Self::Unsupported(unsupported("ICC inverse mBA is not a forward transform")),
            _ => Self::Unsupported(unsupported("unsupported ICC forward transform type")),
        })
    }
    fn evaluate(&self, values: [f64; 4], arithmetic: &mut Arithmetic) -> [f64; 3] {
        match self {
            Self::Gray { curve, pcs } => pcs.xyz(curve.evaluate(values[0], arithmetic)),
            Self::Matrix { matrix, curves } => {
                let linear = std::array::from_fn(|i| curves[i].evaluate(values[i], arithmetic));
                cie::map(*matrix, linear)
            }
            Self::Lut(lut, encoding) => encoding.decode(lut.evaluate(values)),
            Self::Mab(mab, encoding) => encoding.decode(mab.evaluate(values, arithmetic)),
            Self::Unsupported(_) => [0.0; 3],
        }
    }
    fn retained_bytes(&self) -> usize {
        match self {
            Self::Gray { curve, .. } => curve.retained_bytes(),
            Self::Matrix { curves, .. } => curves.iter().map(Curve::retained_bytes).sum(),
            Self::Lut(lut, _) => lut.retained_bytes(),
            Self::Mab(mab, _) => mab.retained_bytes(),
            Self::Unsupported(_) => 0,
        }
    }
}
impl Encoding {
    fn decode(self, values: [f64; 3]) -> [f64; 3] {
        match self {
            Self::Xyz => values.map(|v| v * (65535.0 / 32768.0)), // 6.3.4.2, Table 11.
            Self::Lab => cie::lab_to_xyz(
                [
                    values[0] * 100.0,
                    values[1] * 255.0 - 128.0,
                    values[2] * 255.0 - 128.0,
                ],
                D50,
            ),
            Self::LegacyLab => {
                // 10.8, Tables 39–40: mft2 keeps FF00 white in v2 AND v4.
                let v = values.map(|v| (v * (65535.0 / 65280.0)).clamp(0.0, 1.0));
                cie::lab_to_xyz(
                    [v[0] * 100.0, v[1] * 255.0 - 128.0, v[2] * 255.0 - 128.0],
                    D50,
                )
            }
        }
    }
}
fn matrix_model(
    profile: &Cursor<'_>,
    tags: &format::Tags<'_>,
    budget: &mut Budget,
) -> Result<Option<Forward>> {
    if profile.bytes(16, 4)? == b"GRAY" {
        let pcs = if profile.bytes(20, 4)? == b"Lab " {
            Pcs::Lab
        } else {
            Pcs::Xyz
        };
        return tags
            .get(b"kTRC")
            .map(|tag| Curve::parse(tag, budget).map(|curve| Forward::Gray { curve, pcs }))
            .transpose();
    }
    if profile.bytes(16, 4)? != b"RGB " || profile.bytes(20, 4)? != b"XYZ " {
        return Ok(None);
    }
    if ![b"rXYZ", b"gXYZ", b"bXYZ", b"rTRC", b"gTRC", b"bTRC"]
        .iter()
        .all(|name| tags.contains_key(*name))
    {
        return Ok(None);
    }
    let [r, g, b] = [
        xyz(format::required(tags, *b"rXYZ")?)?,
        xyz(format::required(tags, *b"gXYZ")?)?,
        xyz(format::required(tags, *b"bXYZ")?)?,
    ];
    let curves = [
        Curve::parse(format::required(tags, *b"rTRC")?, budget)?,
        Curve::parse(format::required(tags, *b"gTRC")?, budget)?,
        Curve::parse(format::required(tags, *b"bTRC")?, budget)?,
    ];
    Ok(Some(Forward::Matrix {
        matrix: [[r[0], g[0], b[0]], [r[1], g[1], b[1]], [r[2], g[2], b[2]]],
        curves,
    }))
}
fn xyz(tag: &Cursor<'_>) -> Result<[f64; 3]> {
    let length = tag.data().len();
    if tag.bytes(0, 4)? != b"XYZ " || length < 20 || !(length - 8).is_multiple_of(12) {
        return Err(tag.error("invalid ICC XYZ tag type or extent"));
    }
    format::zeros(tag, 4, 4)?;
    let mut values = tag.cursor_at(8)?;
    let first = [values.fixed()?, values.fixed()?, values.fixed()?];
    let mut rest = tag.cursor_at(8)?;
    while !rest.at_end() {
        if rest.fixed()? < 0.0 {
            return Err(Error::syntax("negative ICC XYZ tag value").at(rest.position()));
        }
    }
    Ok(first)
}

/// Random access into a profile or one of its tags. Offsets are relative
/// to the cursor's data; error positions stay offsets in the profile.
/// Truncation is a `Syntax` error and offset overflow a `Limit` error.
trait At<'a> {
    fn bytes(&self, offset: usize, length: usize) -> Result<&'a [u8]>;
    fn sub(&self, offset: usize, length: usize) -> Result<Cursor<'a>>;
    fn u8_at(&self, offset: usize) -> Result<u8>;
    fn u16_at(&self, offset: usize) -> Result<u16>;
    fn u32_at(&self, offset: usize) -> Result<u32>;
}
impl<'a> At<'a> for Cursor<'a> {
    fn bytes(&self, offset: usize, length: usize) -> Result<&'a [u8]> {
        self.cursor_at(offset)?.take(length)
    }
    fn sub(&self, offset: usize, length: usize) -> Result<Cursor<'a>> {
        self.cursor_at(offset)?.take_cursor(length)
    }
    fn u8_at(&self, offset: usize) -> Result<u8> {
        self.cursor_at(offset)?.u8()
    }
    fn u16_at(&self, offset: usize) -> Result<u16> {
        self.cursor_at(offset)?.u16()
    }
    fn u32_at(&self, offset: usize) -> Result<u32> {
        self.cursor_at(offset)?.u32()
    }
}
fn unsupported(message: &'static str) -> Error {
    Error::unsupported(features::ICC_BASED, message)
}
#[expect(
    clippy::cast_precision_loss,
    reason = "sample count bounded by the decode_bytes budget"
)]
fn usize_f64(value: usize) -> f64 {
    value as f64
}
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "sample position is finite and bounded by validated curve length"
)]
fn sample_index(value: f64) -> usize {
    value as usize
}

#[cfg(test)]
mod tests {
    use super::*;
    use pagura_core::ErrorKind;
    use pagura_testkit::{Manifest, fixture};

    struct Expected {
        data: Vec<u8>,
        components: usize,
        work: usize,
        bytes: usize,
    }

    /// A color fixture and the work and storage its parse must admit: one
    /// work unit per tag entry and table value, `TAG_BYTES` per tag entry
    /// and one `f64` per table value.
    fn expected(name: &str) -> Expected {
        let manifest = Manifest::load("color");
        let case = manifest.case(name).unwrap();
        let number = |key| usize::try_from(case.get(key).unwrap().as_u64().unwrap()).unwrap();
        let (tags, values) = (number("tags"), number("table_values"));
        Expected {
            data: fixture("color", &format!("{name}.icc")),
            components: number("components"),
            work: tags + values,
            bytes: tags * format::TAG_BYTES + values * size_of::<f64>(),
        }
    }

    #[test]
    fn table_storage_and_work_are_admitted_before_evaluation() {
        for name in ["lut16-cmyk-xyz", "sampled-rgb"] {
            let case = expected(name);
            for (limits, succeeds) in [
                (
                    Limits {
                        decode_work: case.work - 1,
                        ..Limits::default()
                    },
                    false,
                ),
                (
                    Limits {
                        decode_bytes: case.bytes - 1,
                        ..Limits::default()
                    },
                    false,
                ),
                (
                    Limits {
                        decode_work: case.work,
                        decode_bytes: case.bytes,
                        ..Limits::default()
                    },
                    true,
                ),
            ] {
                let result = Profile::parse(&case.data, case.components, &limits);
                if succeeds {
                    let profile = result.unwrap().unwrap();
                    assert!(
                        profile.convert(&[0.5; 4], Intent::Relative).is_ok(),
                        "{name}"
                    );
                } else {
                    assert_eq!(result.unwrap_err().kind, ErrorKind::Limit, "{name}");
                }
            }
        }
    }

    #[test]
    fn truncated_and_mismatched_profiles_are_syntax_errors_at_their_offset() {
        let case = expected("sampled-rgb");
        let limits = Limits::default();
        assert!(Profile::parse(&case.data, 3, &limits).unwrap().is_some());
        for end in 0..case.data.len() {
            let error = Profile::parse(&case.data[..end], 3, &limits).unwrap_err();
            assert_eq!(error.kind, ErrorKind::Syntax, "{end}");
        }
        assert_eq!(
            Profile::parse(&case.data, 1, &limits).unwrap_err().kind,
            ErrorKind::Syntax
        );
        // A profile that declares more bytes than it has is truncated where
        // the first tag lies beyond the data.
        let mut short = case.data.clone();
        let length = u32::try_from(short.len() - 4).unwrap();
        short.truncate(short.len() - 4);
        short[..4].copy_from_slice(&length.to_be_bytes());
        let error = Profile::parse(&short, 3, &limits).unwrap_err();
        assert_eq!(error.kind, ErrorKind::Syntax);
        assert!(matches!(error.position, pagura_core::Position::Buffer(n) if n <= short.len()));
    }
}
