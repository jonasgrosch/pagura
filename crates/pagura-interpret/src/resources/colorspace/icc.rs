//! ICCBased colour spaces (8.6.5.5): the stream's `N`, `Range` and
//! `Alternate`, and profiles parsed once per object.

use super::super::numbers::entry;
use pagura_color::Profile;
use pagura_core::{Built, Dictionary, Error, ObjectId, Result, Store, StoreKey, Value};
use pagura_document::Document;
use std::collections::BTreeMap;
use std::sync::Arc;

/// The store key of a parsed profile: its stream and component count.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct ProfileKey(ObjectId, usize);

/// A parsed profile, or `None` for a profile the colour management does not
/// support, whose alternate space is used instead.
#[derive(Debug)]
struct StoredProfile(Option<Arc<Profile>>);

impl StoreKey for ProfileKey {
    type Value = StoredProfile;
}

/// Profiles met in one interpretation, by stream, and the bytes they retain.
#[derive(Debug)]
pub(crate) struct Profiles<'a> {
    store: Option<&'a Store>,
    entries: BTreeMap<ObjectId, (usize, Option<Arc<Profile>>)>,
    bytes: usize,
    document: &'a Document,
}

impl<'a> Profiles<'a> {
    pub(crate) fn new(document: &'a Document, store: Option<&'a Store>) -> Self {
        Self {
            store,
            entries: BTreeMap::new(),
            bytes: 0,
            document,
        }
    }

    /// The profile of stream `id` read for `components` components.
    fn get(&mut self, id: ObjectId, components: usize) -> Result<Option<Arc<Profile>>> {
        if let Some((count, profile)) = self.entries.get(&id) {
            if *count != components {
                return Err(
                    Error::syntax("ICC component count differs from cached profile").in_object(id),
                );
            }
            return Ok(profile.clone());
        }
        let document = self.document;
        let limits = document.limits();
        let parse = || -> Result<Built<StoredProfile>> {
            let object = document.object(id)?;
            let bytes = document.stream_data(object)?;
            let profile = Profile::parse(&bytes, components, &limits)
                .map_err(|e| e.in_object(id))?
                .map(Arc::new);
            let bytes = profile.as_ref().map_or(0, |p| p.retained_bytes());
            Ok(Built {
                value: StoredProfile(profile),
                bytes,
            })
        };
        let profile = match self.store {
            Some(store) => store
                .get_or_build(ProfileKey(id, components), parse)?
                .0
                .clone(),
            None => parse()?.value.0,
        };
        let charge = profile
            .as_ref()
            .map_or(0, |p| p.retained_bytes())
            .checked_add(64 + size_of::<(usize, Option<Arc<Profile>>)>())
            .ok_or(Error::limit("ICC cache byte size overflow"))?;
        let total = self
            .bytes
            .checked_add(charge)
            .filter(|&n| n <= limits.render_bytes)
            .ok_or(Error::limit("ICC cache byte budget exceeded"))?;
        if self.entries.len() >= limits.value_nodes {
            return Err(Error::limit("ICC cache entry budget exceeded"));
        }
        self.entries.insert(id, (components, profile.clone()));
        self.bytes = total;
        Ok(profile)
    }
}

/// An ICCBased space's stream as read, before its alternate.
pub(super) struct Read<'a> {
    pub(super) dictionary: &'a Dictionary,
    pub(super) components: u8,
    pub(super) range: [[f64; 2]; 4],
    pub(super) profile: Option<Arc<Profile>>,
}

/// Reads the profile stream `value` names.
pub(super) fn read<'a>(
    document: &'a Document,
    profiles: &mut Profiles<'_>,
    value: &Value,
) -> Result<Read<'a>> {
    let id = value
        .as_reference()
        .ok_or(Error::syntax("ICC profile must be a stream reference"))?;
    let object = document.object(id)?;
    if !object.is_stream() {
        return Err(Error::syntax("ICC profile must be a stream"));
    }
    let dictionary = object
        .dictionary()
        .ok_or(Error::syntax("ICC stream has no dictionary"))?;
    let components = entry(document, dictionary, b"N")?
        .and_then(Value::as_integer)
        .ok_or(Error::syntax("ICC profile has no N"))?;
    let components = match components {
        1 => 1,
        3 => 3,
        4 => 4,
        _ => return Err(Error::syntax("invalid ICC component count")),
    };
    let range = range(document, dictionary, usize::from(components))?;
    let profile = profiles.get(id, usize::from(components))?;
    Ok(Read {
        dictionary,
        components,
        range,
        profile,
    })
}

fn range(document: &Document, dictionary: &Dictionary, count: usize) -> Result<[[f64; 2]; 4]> {
    let mut ranges = [[0.0, 1.0]; 4];
    let Some(value) = dictionary.get(b"Range") else {
        return Ok(ranges);
    };
    let values = document
        .resolve(value)?
        .as_array()
        .ok_or(Error::syntax("ICC Range must be an array"))?;
    if values.len() != count * 2 {
        return Err(Error::syntax("wrong ICC Range length"));
    }
    for (out, pair) in ranges.iter_mut().zip(values.as_chunks::<2>().0) {
        let bound = |value: &Value| -> Result<f64> {
            document
                .resolve(value)?
                .as_number()
                .ok_or(Error::syntax("invalid ICC Range bound"))
        };
        let (low, high) = (bound(&pair[0])?, bound(&pair[1])?);
        if low > high {
            return Err(Error::syntax("reversed ICC Range"));
        }
        *out = [low, high];
    }
    Ok(ranges)
}

/// Checks `Range` against what the profile accepts: CIELAB bounds for a
/// Lab-input profile, the unit interval otherwise.
pub(super) fn check_range(
    profile: Option<&Profile>,
    range: &[[f64; 2]; 4],
    components: u8,
) -> Result<()> {
    let Some(profile) = profile else {
        return Ok(());
    };
    if profile.lab_input() {
        for (i, pair) in range.iter().take(3).enumerate() {
            let expected = if i == 0 {
                [0.0, 100.0]
            } else {
                [-128.0, 127.0]
            };
            if pair[0] < expected[0] || pair[1] > expected[1] {
                return Err(Error::syntax("ICC Range exceeds Lab profile range"));
            }
        }
    } else if range
        .iter()
        .take(usize::from(components))
        .any(|p| p[0] < 0.0 || p[1] > 1.0)
    {
        return Err(Error::syntax("ICC Range exceeds profile range"));
    }
    Ok(())
}

/// The component count an `Alternate` value declares, without resolving it
/// fully.
pub(super) fn alternate_components(
    document: &Document,
    resources: &Dictionary,
    value: &Value,
    depth: usize,
) -> Result<usize> {
    if depth > document.limits().depth {
        return Err(Error::limit("ICC alternate depth exceeded"));
    }
    let value = document.resolve(value)?;
    let (family, args) = if let Some(name) = value.as_name() {
        (name.as_bytes(), &[][..])
    } else {
        let array = value
            .as_array()
            .ok_or(Error::syntax("invalid ICC alternate"))?;
        let family = array
            .first()
            .and_then(Value::as_name)
            .ok_or(Error::syntax("invalid ICC alternate family"))?;
        (family.as_bytes(), array.get(1..).unwrap_or_default())
    };
    Ok(match family {
        b"DeviceGray" | b"CalGray" | b"Indexed" | b"Separation" => 1,
        b"DeviceRGB" | b"CalRGB" | b"Lab" => 3,
        b"DeviceCMYK" => 4,
        b"DeviceN" => document
            .resolve(
                args.first()
                    .ok_or(Error::syntax("missing ICC alternate DeviceN names"))?,
            )?
            .as_array()
            .ok_or(Error::syntax("invalid ICC alternate DeviceN names"))?
            .len(),
        b"ICCBased" => {
            let id = args
                .first()
                .and_then(Value::as_reference)
                .ok_or(Error::syntax("invalid ICC alternate profile reference"))?;
            let object = document.object(id)?;
            let dictionary = object
                .dictionary()
                .filter(|_| object.is_stream())
                .ok_or(Error::syntax("invalid ICC alternate profile"))?;
            let n = document
                .resolve(
                    dictionary
                        .get(b"N")
                        .ok_or(Error::syntax("missing ICC alternate N"))?,
                )?
                .as_integer()
                .filter(|v| matches!(v, 1 | 3 | 4))
                .ok_or(Error::syntax("invalid ICC alternate N"))?;
            usize::try_from(n).map_err(|_| Error::syntax("invalid ICC alternate N"))?
        }
        b"Pattern" => return Err(Error::syntax("Pattern is not an ICC alternate")),
        name if args.is_empty() => {
            let spaces = resources
                .get(b"ColorSpace")
                .map(|v| document.resolve_dictionary(v))
                .transpose()?
                .ok_or(Error::syntax("undefined ICC alternate"))?;
            let value = spaces
                .get(name)
                .ok_or(Error::syntax("undefined ICC alternate"))?;
            return alternate_components(document, resources, value, depth + 1);
        }
        _ => return Err(Error::syntax("invalid ICC alternate family")),
    })
}
