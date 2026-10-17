//! ICC source matrix/TRC processing for PDF colour spaces (8.6.5.5).
//!
//! Accepts ICC v2.0–2.4 and v4.0–4.3 Gray/RGB profiles with XYZ PCS. Tag
//! layouts and curve equations implemented here await primary ICC-text review;
//! their independent fixtures carry the same qualification. LUT processing is
//! explicit Unsupported. Later versions may request PDF Alternate fallback.
//! Output policy: relative PCS D50, Bradford adaptation to D65, sRGB encoding.

use crate::{cie, features};
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
        let profile = Profile::parse(&bytes, components, self.limits()).map(|p| p.map(Arc::new));
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

#[derive(Debug, Clone)]
pub(crate) struct Profile {
    matrix: [[f64; 3]; 3],
    curves: Vec<Curve>,
    adaptation: [[f64; 3]; 3],
}
#[derive(Debug, Clone)]
enum Curve {
    Identity,
    Gamma(f64),
    Samples(Vec<f64>),
    Parametric(u16, [f64; 7]),
}
impl Profile {
    fn retained_bytes(&self) -> usize {
        self.curves.iter().fold(
            size_of::<Self>()
                .saturating_add(self.curves.capacity().saturating_mul(size_of::<Curve>())),
            |total, curve| {
                total.saturating_add(if let Curve::Samples(values) = curve {
                    values.capacity().saturating_mul(size_of::<f64>())
                } else {
                    0
                })
            },
        )
    }
    pub(crate) fn parse(data: &[u8], components: usize, limits: Limits) -> Result<Option<Self>> {
        let tags = header(data, components, limits)?;
        let major = *data.get(8).ok_or(Error::syntax("truncated ICC version"))?;
        let minor = *data.get(9).ok_or(Error::syntax("truncated ICC version"))? >> 4;
        if major > 4 || (major == 4 && minor > 3) {
            return Ok(None);
        }
        if !((major == 2 && minor <= 4) || (major == 4 && minor <= 3)) {
            return Err(unsupported("unsupported ICC profile version"));
        }
        if slice(data, 20, 4)? != b"XYZ " || components == 4 || slice(data, 16, 4)? == b"Lab " {
            return Err(unsupported("ICC profile requires LUT processing"));
        }
        if tags
            .keys()
            .any(|v| matches!(v, b"A2B0" | b"A2B1" | b"A2B2"))
        {
            return Err(unsupported("ICC AToB LUT processing is not implemented"));
        }
        let tag = |name: &[u8; 4]| {
            tags.get(name)
                .copied()
                .ok_or(Error::syntax("missing ICC matrix/TRC tag"))
        };
        let mut work = 0;
        let (matrix, curves) = if components == 1 {
            let white = xyz(tag(b"wtpt")?)?;
            (
                [
                    [white[0], 0.0, 0.0],
                    [white[1], 0.0, 0.0],
                    [white[2], 0.0, 0.0],
                ],
                vec![Curve::parse(tag(b"kTRC")?, &mut work, limits)?],
            )
        } else {
            let [r, g, b] = [
                xyz(tag(b"rXYZ")?)?,
                xyz(tag(b"gXYZ")?)?,
                xyz(tag(b"bXYZ")?)?,
            ];
            let curves = [b"rTRC", b"gTRC", b"bTRC"]
                .iter()
                .map(|name| Curve::parse(tag(name)?, &mut work, limits))
                .collect::<Result<Vec<_>>>()?;
            (
                [[r[0], g[0], b[0]], [r[1], g[1], b[1]], [r[2], g[2], b[2]]],
                curves,
            )
        };
        Ok(Some(Self {
            matrix,
            curves,
            adaptation: cie::adaptation([0.9642, 1.0, 0.8249])?,
        }))
    }
    pub(crate) fn convert(&self, inputs: &[f64]) -> Result<[f64; 3]> {
        let mut linear = [0.0; 3];
        for (i, curve) in self.curves.iter().enumerate() {
            linear[i] = curve.evaluate(inputs.get(i).copied().unwrap_or(0.0).clamp(0.0, 1.0))?;
        }
        let xyz = cie::map(self.adaptation, cie::map(self.matrix, linear));
        Ok(cie::xyz_to_srgb(xyz))
    }
}

fn header(data: &[u8], components: usize, limits: Limits) -> Result<BTreeMap<[u8; 4], &[u8]>> {
    if usize::try_from(u32_at(data, 0)?).ok() != Some(data.len()) || slice(data, 36, 4)? != b"acsp"
    {
        return Err(Error::syntax("invalid ICC profile header or size"));
    }
    if !matches!(slice(data, 12, 4)?, b"scnr" | b"mntr" | b"prtr" | b"spac") {
        return Err(Error::syntax("invalid PDF ICC profile class"));
    }
    let actual = match slice(data, 16, 4)? {
        b"GRAY" => 1,
        b"RGB " | b"Lab " => 3,
        b"CMYK" => 4,
        _ => return Err(Error::syntax("invalid PDF ICC data colour space")),
    };
    if components != actual {
        return Err(Error::syntax("ICC profile N differs from header"));
    }
    let count =
        usize::try_from(u32_at(data, 128)?).map_err(|_| Error::limit("ICC tag count overflow"))?;
    if count > limits.value_nodes || count > limits.decoded_bytes / 64 {
        return Err(Error::limit("ICC tag budget exceeded"));
    }
    let end = count
        .checked_mul(12)
        .and_then(|n| n.checked_add(132))
        .ok_or(Error::limit("ICC table overflow"))?;
    slice(data, 0, end)?;
    let mut tags = BTreeMap::new();
    for i in 0..count {
        let entry = 132 + i * 12;
        let name = slice(data, entry, 4)?
            .try_into()
            .map_err(|_| Error::syntax("invalid ICC tag"))?;
        let offset = usize::try_from(u32_at(data, entry + 4)?)
            .map_err(|_| Error::limit("ICC offset overflow"))?;
        let length = usize::try_from(u32_at(data, entry + 8)?)
            .map_err(|_| Error::limit("ICC tag size overflow"))?;
        if offset < end || !offset.is_multiple_of(4) || length < 8 {
            return Err(Error::syntax("invalid ICC tag extent"));
        }
        let value = slice(data, offset, length)?;
        if tags.insert(name, value).is_some() {
            return Err(Error::syntax("duplicate ICC tag"));
        }
    }
    Ok(tags)
}
impl Curve {
    fn parse(data: &[u8], work: &mut usize, limits: Limits) -> Result<Self> {
        match slice(data, 0, 4)? {
            b"curv" => {
                let count = usize::try_from(u32_at(data, 8)?)
                    .map_err(|_| Error::limit("ICC curve size overflow"))?;
                *work = work
                    .checked_add(count)
                    .filter(|&n| n <= limits.value_nodes && n <= limits.decoded_bytes / 8)
                    .ok_or(Error::limit("ICC curve storage budget exceeded"))?;
                slice(
                    data,
                    12,
                    count
                        .checked_mul(2)
                        .ok_or(Error::limit("ICC curve size overflow"))?,
                )?;
                match count {
                    0 => Ok(Self::Identity),
                    1 => {
                        let gamma = f64::from(u16_at(data, 12)?) / 256.0;
                        if gamma <= 0.0 {
                            return Err(Error::syntax("invalid ICC curve gamma"));
                        }
                        Ok(Self::Gamma(gamma))
                    }
                    _ => {
                        let mut samples = Vec::new();
                        samples
                            .try_reserve_exact(count)
                            .map_err(|_| Error::limit("ICC curve allocation failed"))?;
                        for i in 0..count {
                            samples.push(f64::from(u16_at(data, 12 + 2 * i)?) / 65535.0);
                        }
                        if samples.windows(2).any(|p| p[0] > p[1]) {
                            return Err(Error::syntax("nonmonotonic ICC TRC"));
                        }
                        Ok(Self::Samples(samples))
                    }
                }
            }
            b"para" => {
                let kind = u16_at(data, 8)?;
                let count = match kind {
                    0 => 1,
                    1 => 3,
                    2 => 4,
                    3 => 5,
                    4 => 7,
                    _ => return Err(unsupported("unsupported ICC parametric curve")),
                };
                let mut params = [0.0; 7];
                for (i, p) in params.iter_mut().enumerate().take(count) {
                    *p = fixed(data, 12 + 4 * i)?;
                }
                if params[0] <= 0.0 || (kind > 0 && params[1] <= 0.0) {
                    return Err(Error::syntax("invalid ICC parametric curve"));
                }
                if kind >= 3 && params[4] <= 1.0 && params[1] * params[4].max(0.0) + params[2] < 0.0
                {
                    return Err(Error::syntax("ICC curve power includes negative inputs"));
                }
                Ok(Self::Parametric(kind, params))
            }
            _ => Err(unsupported("unsupported ICC curve tag type")),
        }
    }
    fn evaluate(&self, x: f64) -> Result<f64> {
        let value = match self {
            Self::Identity => x,
            Self::Gamma(g) => x.powf(*g),
            Self::Samples(values) => {
                let position = x * usize_f64(values.len() - 1);
                let low = sample_index(position).min(values.len() - 1);
                let high = (low + 1).min(values.len() - 1);
                values[low] + (position - usize_f64(low)) * (values[high] - values[low])
            }
            Self::Parametric(kind, [g, a, b, c, d, e, f]) => match kind {
                0 => x.powf(*g),
                1 => {
                    if x >= -b / a {
                        (a * x + b).powf(*g)
                    } else {
                        0.0
                    }
                }
                2 => {
                    if x >= -b / a {
                        (a * x + b).powf(*g) + c
                    } else {
                        *c
                    }
                }
                3 => {
                    if x >= *d {
                        (a * x + b).powf(*g)
                    } else {
                        c * x
                    }
                }
                _ => {
                    if x >= *d {
                        (a * x + b).powf(*g) + e
                    } else {
                        c * x + f
                    }
                }
            },
        };
        if !value.is_finite() {
            return Err(Error::syntax("undefined ICC curve arithmetic"));
        }
        Ok(value.clamp(0.0, 1.0))
    }
}
fn xyz(data: &[u8]) -> Result<[f64; 3]> {
    if slice(data, 0, 4)? != b"XYZ " {
        return Err(Error::syntax("invalid ICC XYZ tag type"));
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
