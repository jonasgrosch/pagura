//! Document-bound ICC profile cache for one compilation (8.6.5.5). Profile
//! parsing and evaluation live in `pagura-color`.

use pagura_core::{Dictionary, Error, ObjectId, Result, Value};
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
impl<'a> ColorContext<'a> {
    pub(crate) fn new(document: &'a Document, cache: &'a Cache) -> Self {
        Self { document, cache }
    }
    /// The document whose objects colour spaces refer to.
    pub(crate) fn document(&self) -> &'a Document {
        self.document
    }
    pub(crate) fn limits(&self) -> pagura_core::Limits {
        self.document.limits()
    }
    pub(crate) fn resolve<'v>(&self, value: &'v Value) -> Result<&'v Value>
    where
        'a: 'v,
    {
        self.document.resolve(value)
    }
    pub(crate) fn resolve_dictionary<'v>(&self, value: &'v Value) -> Result<&'v Dictionary>
    where
        'a: 'v,
    {
        self.document.resolve_dictionary(value)
    }
    pub(crate) fn object(&self, id: ObjectId) -> Result<&'a pagura_document::IndirectObject> {
        self.document.object(id)
    }
    pub(crate) fn stream_data(
        &self,
        object: &pagura_document::IndirectObject,
    ) -> Result<std::borrow::Cow<'a, [u8]>> {
        self.document.stream_data(object)
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
        let profile = pagura_color::Profile::parse(&bytes, components, &self.limits())
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
    use pagura_core::Value;
    use pagura_document::OpenOptions;

    #[test]
    fn profiles_are_cached_by_object_without_rewriting_the_stream() {
        let document = Document::open(
            pagura_testkit::fixture("content", "icc-3-sampled.pdf"),
            &OpenOptions::default(),
        )
        .unwrap();
        let id = ObjectId::new(5, 0);
        let cache = Cache::default();
        let context = ColorContext::new(&document, &cache);
        let first = context.profile(id, 3).unwrap().unwrap();
        let retained = cache.bytes.get();
        let second = context.profile(id, 3).unwrap().unwrap();
        assert!(Arc::ptr_eq(&first, &second));
        assert_eq!(cache.bytes.get(), retained);
        assert_eq!(
            context.profile(id, 1).unwrap_err().kind,
            pagura_core::ErrorKind::Syntax
        );
        // The stream dictionary is immutable; profile admission never rewrites it.
        let object = document.object(id).unwrap();
        assert_eq!(
            object.dictionary().unwrap().get(b"N"),
            Some(&Value::Integer(3))
        );
    }
}
