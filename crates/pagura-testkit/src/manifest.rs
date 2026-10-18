//! Fixture manifests, `corpus/manifests/<area>.json`, written by the
//! independent generators in `tools/fixturegen`.

use crate::json::{Json, JsonError};
use crate::workspace_root;
use std::path::PathBuf;

/// A fixture manifest: its area and its cases in generator order.
#[derive(Debug, Clone, PartialEq)]
pub struct Manifest {
    area: String,
    cases: Vec<Json>,
}

impl Manifest {
    /// Parses a manifest: an object with a string `area` and a `cases` array
    /// of objects that each have a string `id`.
    ///
    /// # Errors
    ///
    /// A [`JsonError`] for invalid JSON or a different shape.
    pub fn parse(text: &str) -> Result<Self, JsonError> {
        let shape = JsonError::shape;
        let Json::Object(members) = Json::parse(text)? else {
            return Err(shape("manifest is not a JSON object"));
        };
        let mut area = None;
        let mut cases = None;
        for (key, value) in members {
            match (key.as_str(), value) {
                ("area", Json::String(value)) => area = Some(value),
                ("cases", Json::Array(values)) => cases = Some(values),
                _ => {}
            }
        }
        let cases = cases.ok_or_else(|| shape("manifest has no cases array"))?;
        if !cases
            .iter()
            .all(|case| case.get("id").and_then(Json::as_str).is_some())
        {
            return Err(shape("manifest case without a string id"));
        }
        Ok(Self {
            area: area.ok_or_else(|| shape("manifest has no area"))?,
            cases,
        })
    }

    /// Reads `corpus/manifests/<area>.json`.
    ///
    /// # Panics
    ///
    /// Panics if the manifest is missing or malformed; run
    /// `python3 tools/fixtures.py`.
    #[track_caller]
    pub fn load(area: &str) -> Self {
        let path = workspace_root()
            .join("corpus/manifests")
            .join(format!("{area}.json"));
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("manifest {}: {error}", path.display()));
        Self::parse(&text).unwrap_or_else(|error| panic!("manifest {}: {error}", path.display()))
    }

    /// The fixture area, such as `classic`.
    pub fn area(&self) -> &str {
        &self.area
    }

    /// The cases in generator order.
    pub fn cases(&self) -> impl ExactSizeIterator<Item = Case<'_>> {
        self.cases.iter().map(|json| Case { json })
    }

    /// The case whose name ([`Case::name`]) is `name`.
    pub fn case(&self, name: &str) -> Option<Case<'_>> {
        self.cases().find(|case| case.name() == name)
    }
}

/// One manifest case: an object with at least a string `id`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Case<'a> {
    json: &'a Json,
}

impl<'a> Case<'a> {
    /// The case identifier, `area/name`.
    pub fn id(&self) -> &'a str {
        self.string("id").unwrap_or_default()
    }

    /// The identifier without its `area/` prefix, which is also the stem of
    /// the case's fixture files.
    pub fn name(&self) -> &'a str {
        let id = self.id();
        id.split_once('/').map_or(id, |(_, name)| name)
    }

    /// The field `key`, if present.
    pub fn get(&self, key: &str) -> Option<&'a Json> {
        self.json.get(key)
    }

    /// The field `key`, if it is a string.
    pub fn string(&self, key: &str) -> Option<&'a str> {
        self.get(key).and_then(Json::as_str)
    }

    /// The case's `path` field, resolved against the workspace root.
    pub fn path(&self) -> Option<PathBuf> {
        self.string("path").map(|path| workspace_root().join(path))
    }
}

impl JsonError {
    /// A manifest-shape error, which has no meaningful offset.
    fn shape(message: &'static str) -> Self {
        Self::new(0, message)
    }
}

#[cfg(test)]
mod tests {
    use super::Manifest;

    const TEXT: &str = r#"{
      "schema": 2,
      "area": "demo",
      "cases": [
        {"id": "demo/first", "expected": "render", "path": "corpus/generated/demo/first.pdf", "pages": 1},
        {"id": "demo/second", "expected": "unsupported", "feature": "filter.X"}
      ]
    }"#;

    #[test]
    fn cases_expose_ids_names_and_fields() {
        let manifest = Manifest::parse(TEXT).unwrap();
        assert_eq!(manifest.area(), "demo");
        let names: Vec<_> = manifest.cases().map(|case| case.name()).collect();
        assert_eq!(names, ["first", "second"]);
        let first = manifest.case("first").unwrap();
        assert_eq!(first.id(), "demo/first");
        assert_eq!(first.string("expected"), Some("render"));
        assert_eq!(first.get("pages").and_then(crate::Json::as_u64), Some(1));
        assert!(
            first
                .path()
                .unwrap()
                .ends_with("corpus/generated/demo/first.pdf")
        );
        let second = manifest.case("second").unwrap();
        assert_eq!(
            (second.path(), second.string("feature")),
            (None, Some("filter.X"))
        );
        assert!(manifest.case("third").is_none());
    }

    #[test]
    fn other_shapes_are_rejected() {
        for text in [
            "[]",
            r#"{"area": "a"}"#,
            r#"{"cases": []}"#,
            r#"{"area": "a", "cases": [{"name": "x"}]}"#,
            r#"{"area": "a", "cases": [1]}"#,
            "{",
        ] {
            assert!(Manifest::parse(text).is_err(), "{text}");
        }
    }

    #[test]
    fn every_checked_in_manifest_parses() {
        let directory = crate::workspace_root().join("corpus/manifests");
        let mut count = 0;
        for entry in std::fs::read_dir(directory).unwrap() {
            let path = entry.unwrap().path();
            let area = path.file_stem().unwrap().to_str().unwrap().to_owned();
            let manifest = Manifest::load(&area);
            assert_eq!(manifest.area(), area);
            assert!(manifest.cases().next().is_some(), "{area}");
            assert!(
                manifest
                    .cases()
                    .all(|case| case.id() == format!("{area}/{}", case.name())),
                "{area}"
            );
            count += 1;
        }
        assert!(count >= 10);
    }
}
