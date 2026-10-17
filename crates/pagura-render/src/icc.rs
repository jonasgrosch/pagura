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

use crate::{cie, features};
use curves::Curve;
use pagura_core::{Error, Limits, Result};
use std::collections::BTreeMap;

use pagura_core::ObjectId;
use pagura_document::Document;
use std::{
    cell::{Cell, RefCell},
    sync::Arc,
};

#[derive(Debug, Default)]
pub(crate) struct Cache {
    profiles: RefCell<BTreeMap<ObjectId, CachedProfile>>,
    bytes: Cell<usize>,
}
#[derive(Debug)]
struct CachedProfile {
    components: usize,
    profile: Option<Arc<Profile>>,
}
/// Per-compilation colour context, shared by forms and masks. Intrinsic profile
/// transforms are cached by immutable object identity; resource-dependent
/// Alternate spaces are resolved separately at each use.
pub(crate) struct ColorContext<'a> {
    document: &'a Document,
    cache: &'a Cache,
}
impl std::ops::Deref for ColorContext<'_> {
    type Target = Document;
    fn deref(&self) -> &Document {
        self.document
    }
}
impl<'a> ColorContext<'a> {
    pub(crate) fn new(document: &'a Document, cache: &'a Cache) -> Self {
        Self { document, cache }
    }
    pub(crate) fn profile(&self, id: ObjectId, components: usize) -> Result<Option<Arc<Profile>>> {
        if let Some(cached) = self.cache.profiles.borrow().get(&id) {
            if cached.components != components {
                return Err(
                    Error::syntax("ICC component count differs from cached profile").in_object(id),
                );
            }
            return Ok(cached.profile.clone());
        }
        let object = self.document.object(id)?;
        let bytes = self.document.stream_data(object)?;
        let profile = Profile::parse(&bytes, components, self.limits()).map(|p| {
            p.map(|mut p| {
                p.object = Some(id);
                Arc::new(p)
            })
        });
        let profile = profile.map_err(|e| e.in_object(id))?;
        let charge = profile
            .as_ref()
            .map_or(0, |p| p.retained_bytes())
            .checked_add(64 + size_of::<CachedProfile>())
            .ok_or(Error::limit("ICC cache byte size overflow"))?;
        let total = self
            .cache
            .bytes
            .get()
            .checked_add(charge)
            .filter(|&n| n <= self.limits().decoded_bytes)
            .ok_or(Error::limit("ICC cache byte budget exceeded"))?;
        if self.cache.profiles.borrow().len() >= self.limits().value_nodes {
            return Err(Error::limit("ICC cache entry budget exceeded"));
        }
        self.cache.profiles.borrow_mut().insert(
            id,
            CachedProfile {
                components,
                profile: profile.clone(),
            },
        );
        self.cache.bytes.set(total);
        Ok(profile)
    }
}

const D50: [f64; 3] = [0.9642, 1.0, 0.8249];

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum Intent {
    Perceptual,
    #[default]
    Relative,
    Saturation,
    Absolute,
}
impl Intent {
    pub(crate) fn from_name(name: &[u8]) -> Self {
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

struct Budget {
    values: usize,
    limits: Limits,
}
impl Budget {
    fn charge(&mut self, values: usize) -> Result<()> {
        self.values = self
            .values
            .checked_add(values)
            .filter(|&n| n <= self.limits.value_nodes && n <= self.limits.decoded_bytes / 8)
            .ok_or(Error::limit("ICC transform storage budget exceeded"))?;
        Ok(())
    }
}
#[derive(Debug, Clone)]
pub(crate) struct Profile {
    object: Option<ObjectId>,
    transforms: [Option<Arc<Forward>>; 3],
    model: Option<Forward>,
    lab_input: bool,
    white: [f64; 3],
    adaptation: [[f64; 3]; 3],
}
#[derive(Debug, Clone)]
enum Forward {
    Gray(Curve, bool),
    Matrix([[f64; 3]; 3], [Curve; 3]),
    Lut(lut::Lut, Encoding),
    Mab(mab::Mab, Encoding),
    Unsupported(Error),
}
#[derive(Debug, Clone, Copy)]
enum Encoding {
    Xyz,
    Lab,
    LegacyLab,
}
impl Profile {
    fn retained_bytes(&self) -> usize {
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
    pub(crate) fn parse(data: &[u8], components: usize, limits: Limits) -> Result<Option<Self>> {
        let tags = format::header(data, components, limits)?;
        let major = data[8];
        let minor = data[9] >> 4;
        if major > 4 || (major == 4 && minor > 3) {
            return Ok(None);
        }
        if !((major == 2 && minor <= 4) || (major == 4 && minor <= 3)) {
            return Err(unsupported("unsupported ICC profile version"));
        }
        let mut budget = Budget { values: 0, limits };
        let white = format::model_tags(data, &tags, &mut budget)?;
        let lab = slice(data, 20, 4)? == b"Lab ";
        let model = matrix_model(data, &tags, &mut budget)?;
        let mut transforms: [Option<Arc<Forward>>; 3] = [None, None, None];
        let names = [b"A2B0", b"A2B1", b"A2B2"];
        for (i, name) in names.iter().enumerate() {
            if let Some(bytes) = tags.get(*name) {
                if let Some(j) = (0..i).find(|&j| tags.get(names[j]) == Some(bytes)) {
                    let (previous, next) = transforms.split_at_mut(i);
                    next[0].clone_from(&previous[j]);
                } else {
                    transforms[i] = Some(Arc::new(Forward::parse(
                        bytes,
                        components,
                        lab,
                        &mut budget,
                    )?));
                }
            }
        }
        if model.is_none() && transforms[0].is_none() {
            return Err(Error::syntax("ICC profile has no forward model"));
        }
        Ok(Some(Self {
            object: None,
            transforms,
            model,
            lab_input: slice(data, 16, 4)? == b"Lab ",
            white,
            adaptation: cie::adaptation(D50)?,
        }))
    }
    pub(crate) fn lab_input(&self) -> bool {
        self.lab_input
    }
    pub(crate) fn convert(&self, inputs: &[f64], intent: Intent) -> Result<[f64; 3]> {
        let mut values = [0.0; 4];
        for (i, (value, &input)) in values.iter_mut().zip(inputs).enumerate() {
            *value = if self.lab_input {
                if i == 0 {
                    input.clamp(0.0, 100.0) / 100.0
                } else {
                    (input.clamp(-128.0, 127.0) + 128.0) / 255.0
                }
            } else {
                input.clamp(0.0, 1.0)
            };
        }
        // ICC 8.10.2: designated AToB, then AToB0, then matrix/TRCs.
        // Unsupported DToB elements may be bypassed at step (a); BToA and
        // the header's preferred intent are ignored for PDF source spaces.
        let transform = self.transforms[intent.index()]
            .as_deref()
            .or(self.transforms[0].as_deref())
            .or(self.model.as_ref())
            .ok_or(Error::syntax(
                "ICC profile has no applicable forward transform",
            ))?;
        let mut xyz = transform
            .evaluate(values)
            .map_err(|error| match self.object {
                Some(id) => error.in_object(id),
                None => error,
            })?;
        if intent == Intent::Absolute {
            // ICC 6.3.2: recover media-absolute colorimetry from relative PCS.
            for (i, value) in xyz.iter_mut().enumerate() {
                *value *= self.white[i] / D50[i];
            }
        }
        Ok(cie::xyz_to_srgb(cie::map(self.adaptation, xyz)))
    }
}
impl Forward {
    fn parse(data: &[u8], inputs: usize, lab: bool, budget: &mut Budget) -> Result<Self> {
        let signature = slice(data, 0, 4)?;
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
            b"mft1" | b"mft2" => Self::Lut(lut::Lut::parse(data, inputs, budget)?, encoding),
            b"mAB " => Self::Mab(mab::Mab::parse(data, inputs, budget)?, encoding),
            b"mBA " => Self::Unsupported(unsupported("ICC inverse mBA is not a forward transform")),
            _ => Self::Unsupported(unsupported("unsupported ICC forward transform type")),
        })
    }
    fn evaluate(&self, values: [f64; 4]) -> Result<[f64; 3]> {
        match self {
            Self::Gray(curve, lab) => {
                let value = curve.evaluate(values[0])?;
                Ok(if *lab {
                    lab_xyz([value * 100.0, 0.0, 0.0])
                } else {
                    D50.map(|w| w * value)
                })
            }
            Self::Matrix(matrix, curves) => {
                let mut linear = [0.0; 3];
                for (i, curve) in curves.iter().enumerate() {
                    linear[i] = curve.evaluate(values[i])?;
                }
                Ok(cie::map(*matrix, linear))
            }
            Self::Lut(lut, encoding) => Ok(encoding.decode(lut.evaluate(values))),
            Self::Mab(mab, encoding) => Ok(encoding.decode(mab.evaluate(values)?)),
            Self::Unsupported(error) => Err(error.clone()),
        }
    }
    fn retained_bytes(&self) -> usize {
        match self {
            Self::Gray(curve, _) => curve.retained_bytes(),
            Self::Matrix(_, curves) => curves.iter().map(Curve::retained_bytes).sum(),
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
            Self::Lab => lab_xyz([
                values[0] * 100.0,
                values[1] * 255.0 - 128.0,
                values[2] * 255.0 - 128.0,
            ]),
            Self::LegacyLab => {
                // 10.8, Tables 39–40: mft2 keeps FF00 white in v2 AND v4.
                let v = values.map(|v| (v * (65535.0 / 65280.0)).clamp(0.0, 1.0));
                lab_xyz([v[0] * 100.0, v[1] * 255.0 - 128.0, v[2] * 255.0 - 128.0])
            }
        }
    }
}
fn lab_xyz([l, a, b]: [f64; 3]) -> [f64; 3] {
    let y = (l + 16.0) / 116.0;
    let inverse = |v: f64| {
        if v > 6.0 / 29.0 {
            v * v * v
        } else {
            (v - 4.0 / 29.0) * (108.0 / 841.0)
        }
    };
    [
        D50[0] * inverse(y + a / 500.0),
        inverse(y),
        D50[2] * inverse(y - b / 200.0),
    ]
}
fn matrix_model(
    data: &[u8],
    tags: &format::Tags<'_>,
    budget: &mut Budget,
) -> Result<Option<Forward>> {
    if slice(data, 16, 4)? == b"GRAY" {
        return tags
            .get(b"kTRC")
            .map(|v| {
                Curve::parse(v, budget).map(|curve| {
                    Forward::Gray(curve, slice(data, 20, 4).is_ok_and(|v| v == b"Lab "))
                })
            })
            .transpose();
    }
    if slice(data, 16, 4)? != b"RGB " || slice(data, 20, 4)? != b"XYZ " {
        return Ok(None);
    }
    if ![b"rXYZ", b"gXYZ", b"bXYZ", b"rTRC", b"gTRC", b"bTRC"]
        .iter()
        .all(|name| tags.contains_key(*name))
    {
        return Ok(None);
    }
    let [r, g, b] = [
        xyz(format::required(tags, b"rXYZ")?)?,
        xyz(format::required(tags, b"gXYZ")?)?,
        xyz(format::required(tags, b"bXYZ")?)?,
    ];
    let curves = [
        Curve::parse(format::required(tags, b"rTRC")?, budget)?,
        Curve::parse(format::required(tags, b"gTRC")?, budget)?,
        Curve::parse(format::required(tags, b"bTRC")?, budget)?,
    ];
    Ok(Some(Forward::Matrix(
        [[r[0], g[0], b[0]], [r[1], g[1], b[1]], [r[2], g[2], b[2]]],
        curves,
    )))
}
fn xyz(data: &[u8]) -> Result<[f64; 3]> {
    if slice(data, 0, 4)? != b"XYZ " || data.len() < 20 || !(data.len() - 8).is_multiple_of(12) {
        return Err(Error::syntax("invalid ICC XYZ tag type or extent"));
    }
    format::zeros(slice(data, 4, 4)?)?;
    for at in (8..data.len()).step_by(4) {
        if fixed(data, at)? < 0.0 {
            return Err(Error::syntax("negative ICC XYZ tag value"));
        }
    }
    Ok([fixed(data, 8)?, fixed(data, 12)?, fixed(data, 16)?])
}
fn slice(data: &[u8], offset: usize, length: usize) -> Result<&[u8]> {
    let end = offset
        .checked_add(length)
        .ok_or(Error::limit("ICC extent overflow"))?;
    data.get(offset..end)
        .ok_or(Error::syntax("truncated ICC profile"))
}
fn u32_at(data: &[u8], offset: usize) -> Result<u32> {
    Ok(u32::from_be_bytes(
        slice(data, offset, 4)?
            .try_into()
            .map_err(|_| Error::syntax("invalid ICC integer"))?,
    ))
}
fn u16_at(data: &[u8], offset: usize) -> Result<u16> {
    Ok(u16::from_be_bytes(
        slice(data, offset, 2)?
            .try_into()
            .map_err(|_| Error::syntax("invalid ICC integer"))?,
    ))
}
fn fixed(data: &[u8], offset: usize) -> Result<f64> {
    let raw = i32::from_be_bytes(
        slice(data, offset, 4)?
            .try_into()
            .map_err(|_| Error::syntax("invalid ICC fixed value"))?,
    );
    Ok(f64::from(raw) / 65536.0)
}
fn unsupported(message: &'static str) -> Error {
    Error::unsupported(features::COLORSPACE_ICC, message)
}
#[expect(
    clippy::cast_precision_loss,
    reason = "sample count bounded by decoded bytes and value_nodes"
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
    use pagura_core::{ErrorKind, ObjectId, Value};
    use pagura_document::{Document, OpenOptions};

    #[test]
    fn lut_storage_is_admitted_before_evaluation() {
        let document = Document::open(
            pagura_testkit::fixture("content", "icc-mft2-4-XYZ-v4.pdf"),
            &OpenOptions::default(),
        )
        .unwrap();
        let data = document
            .stream_data(document.object(ObjectId::new(5, 0)).unwrap())
            .unwrap();
        // Four two-sample input curves, 2^4 three-channel nodes and three
        // two-sample output curves require 8 + 48 + 6 scalar values.
        for (nodes, succeeds) in [(61, false), (62, true)] {
            let result = Profile::parse(
                &data,
                4,
                Limits {
                    value_nodes: nodes,
                    ..Limits::default()
                },
            );
            if succeeds {
                assert!(
                    result
                        .unwrap()
                        .unwrap()
                        .convert(&[0.5; 4], Intent::Relative)
                        .is_ok()
                );
            } else {
                assert_eq!(result.unwrap_err().kind, ErrorKind::Limit);
            }
        }
        let error = Profile::parse(
            &data,
            4,
            Limits {
                decoded_bytes: 495,
                ..Limits::default()
            },
        )
        .unwrap_err();
        assert_eq!(error.kind, ErrorKind::Limit);
    }

    #[test]
    fn profile_extents_and_curve_storage_are_bounded() {
        let document = Document::open(
            pagura_testkit::fixture("content", "icc-3-sampled.pdf"),
            &OpenOptions::default(),
        )
        .unwrap();
        let object = document.object(ObjectId::new(5, 0)).unwrap();
        let data = document.stream_data(object).unwrap();
        assert!(
            Profile::parse(&data, 3, Limits::default())
                .unwrap()
                .is_some()
        );
        let cache = Cache::default();
        let context = ColorContext::new(&document, &cache);
        let first = context.profile(ObjectId::new(5, 0), 3).unwrap().unwrap();
        let retained = cache.bytes.get();
        let second = context.profile(ObjectId::new(5, 0), 3).unwrap().unwrap();
        assert!(Arc::ptr_eq(&first, &second));
        assert_eq!(cache.bytes.get(), retained);

        for end in 0..data.len() {
            assert_eq!(
                Profile::parse(&data[..end], 3, Limits::default())
                    .unwrap_err()
                    .kind,
                ErrorKind::Syntax
            );
        }
        assert_eq!(
            Profile::parse(&data, 1, Limits::default())
                .unwrap_err()
                .kind,
            ErrorKind::Syntax
        );
        assert_eq!(
            Profile::parse(
                &data,
                3,
                Limits {
                    value_nodes: 10,
                    ..Limits::default()
                }
            )
            .unwrap_err()
            .kind,
            ErrorKind::Limit
        );
        // The stream dictionary is immutable; profile admission never rewrites it.
        assert_eq!(
            object.dictionary().unwrap().get(b"N"),
            Some(&Value::Integer(3))
        );
    }
}
