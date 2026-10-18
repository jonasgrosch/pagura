//! Document-bound ICC profile cache for one compilation (8.6.5.5). Profile
//! parsing and evaluation live in `pagura-color`.

use pagura_core::{Error, ObjectId, Result};
use pagura_document::Document;
use std::collections::BTreeMap;
use std::{
    cell::{Cell, RefCell},
    sync::Arc,
};

pub(crate) use pagura_color::Intent;

/// A parsed profile and the stream it came from, so that evaluation errors
/// name the profile object.
#[derive(Debug)]
pub(crate) struct Profile {
    profile: pagura_color::Profile,
    object: ObjectId,
}
impl Profile {
    pub(crate) fn lab_input(&self) -> bool {
        self.profile.lab_input()
    }
    pub(crate) fn convert(&self, inputs: &[f64], intent: Intent) -> Result<[f64; 3]> {
        self.profile
            .convert(inputs, intent)
            .map_err(|e| e.in_object(self.object))
    }
    fn retained_bytes(&self) -> usize {
        self.profile.retained_bytes()
    }
}

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
        let profile = pagura_color::Profile::parse(&bytes, components, self.limits())
            .map_err(|e| e.in_object(id))?
            .map(|profile| {
                Arc::new(Profile {
                    profile,
                    object: id,
                })
            });
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

#[cfg(test)]
mod tests {
    use super::*;
    use pagura_color::Intent;
    use pagura_core::Limits;
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
            let result = pagura_color::Profile::parse(
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
        let error = pagura_color::Profile::parse(
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
            pagura_color::Profile::parse(&data, 3, Limits::default())
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
                pagura_color::Profile::parse(&data[..end], 3, Limits::default())
                    .unwrap_err()
                    .kind,
                ErrorKind::Syntax
            );
        }
        assert_eq!(
            pagura_color::Profile::parse(&data, 1, Limits::default())
                .unwrap_err()
                .kind,
            ErrorKind::Syntax
        );
        assert_eq!(
            pagura_color::Profile::parse(
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
