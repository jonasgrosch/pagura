//! The conformance fixtures' render cases, recorded as scenes.

use pagura_core::{Error, Limits, Result};
use pagura_document::{Document, OpenOptions};
use pagura_interpret::{Interpreter, Scene};
use pagura_raster::{DeviceRect, Transform};
use pagura_testkit::{Json, Manifest, workspace_root};

/// The environment variable that restricts the cases to ids containing it.
const FILTER: &str = "PAGURA_REFERENCE_FILTER";

/// One render case of a fixture manifest.
#[derive(Debug, Clone)]
pub struct Case {
    /// `area/name`.
    pub id: String,
    /// The PDF.
    pub path: std::path::PathBuf,
    /// The user password, if any.
    pub password: Vec<u8>,
    /// Zero-based page index; legacy render cases select page zero.
    pub index: usize,
    /// Rendering resolution.
    pub dpi: f64,
    /// The binary PPM oracle, if any.
    pub oracle: Option<std::path::PathBuf>,
    /// Allowed difference from the oracle per channel.
    pub tolerance: u8,
}

/// Every render case of every manifest, in manifest order, restricted by
/// `PAGURA_REFERENCE_FILTER` when it is set.
pub fn render_cases() -> Vec<Case> {
    let filter = std::env::var(FILTER).ok();
    let directory = workspace_root().join("corpus/manifests");
    let mut areas: Vec<String> = std::fs::read_dir(&directory)
        .map(|entries| {
            entries
                .filter_map(|entry| entry.ok()?.path().file_stem()?.to_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();
    areas.sort();
    let mut cases = Vec::new();
    for area in areas {
        let manifest = Manifest::load(&area);
        for case in manifest.cases() {
            if let Some(value) = case.get("page_outcomes") {
                let rows = page_rows(value, case.get("pages"), case.string("expected"));
                assert!(rows.is_ok(), "{}: {:?}", case.id(), rows.as_ref().err());
                let dpi = case.get("dpi").map_or(Some(72.0), Json::as_f64);
                assert!(
                    dpi.is_some_and(|v| v.is_finite() && v > 0.0),
                    "{}: invalid page contract DPI",
                    case.id()
                );
                assert!(
                    case.path().is_some(),
                    "{}: missing page contract source",
                    case.id()
                );
                for (index, row) in rows.unwrap_or_default() {
                    if row.get("expected").and_then(Json::as_str) != Some("render") {
                        continue;
                    }
                    let id = if index == 0 {
                        case.id().to_owned()
                    } else {
                        format!("{}/p{index}", case.id())
                    };
                    if filter.as_deref().is_some_and(|f| !id.contains(f)) {
                        continue;
                    }
                    cases.push(Case {
                        id,
                        path: case.path().unwrap_or_default(),
                        index,
                        password: case
                            .string("password")
                            .unwrap_or_default()
                            .as_bytes()
                            .to_vec(),
                        dpi: dpi.unwrap_or(72.0),
                        oracle: row
                            .get("oracle")
                            .and_then(Json::as_str)
                            .map(|p| workspace_root().join(p)),
                        tolerance: byte(row.get("tolerance")),
                    });
                }
                continue;
            }
            if case.string("expected") != Some("render") {
                continue;
            }
            if filter.as_deref().is_some_and(|f| !case.id().contains(f)) {
                continue;
            }
            let Some(path) = case.path() else {
                continue;
            };
            cases.push(Case {
                id: case.id().to_owned(),
                index: 0,
                path,
                password: case
                    .string("password")
                    .unwrap_or_default()
                    .as_bytes()
                    .to_vec(),
                dpi: case.get("dpi").and_then(Json::as_f64).unwrap_or(72.0),
                oracle: case.string("oracle").map(|p| workspace_root().join(p)),
                tolerance: byte(case.get("tolerance")),
            });
        }
    }
    cases
}

/// A tolerance field; 0 when absent.
fn byte(value: Option<&Json>) -> u8 {
    value
        .and_then(Json::as_u64)
        .and_then(|t| u8::try_from(t).ok())
        .unwrap_or(0)
}

/// A case's selected page recorded as a scene, with its device geometry.
pub struct Recorded {
    /// The scene.
    pub scene: Scene,
    /// Default user space to device pixels.
    pub transform: Transform,
    /// The page's device pixels.
    pub page: DeviceRect,
    /// Whether overprint is simulated (the default preview policy: only
    /// under a PDF/X print intent).
    pub simulate: bool,
}

impl Case {
    /// Opens the case and records its explicitly selected page.
    ///
    /// # Errors
    ///
    /// Returns the document's and the interpreter's errors.
    pub fn record(&self, provider: &dyn pagura_text::Provider) -> Result<Recorded> {
        let bytes =
            std::fs::read(&self.path).map_err(|_| Error::invalid_argument("fixture missing"))?;
        let options = OpenOptions {
            password: self.password.clone(),
            ..OpenOptions::default()
        };
        let document = Document::open(bytes, &options)?;
        let page = document.page(self.index)?;
        let scene = Interpreter::new(&document)
            .with_provider(Some(provider))
            .scene(page)?;
        let scale = self.dpi / 72.0;
        let device = pagura_paint::RenderRequest::page(&scene.page, scale)?;
        Ok(Recorded {
            transform: device.transform,
            page: device.page,
            simulate: scene.print_intent,
            scene,
        })
    }
}

/// Whether an error is a dependency's placeholder for work not landed yet.
pub fn pending(error: &Error) -> bool {
    error.message.contains("not implemented")
}

/// The limits every harness rendering uses.
pub fn limits() -> Limits {
    Limits::default()
}

/// Exact object keys, including no duplicate or unrecognized members.
fn fields(value: &Json, names: &[&str]) -> bool {
    value.as_object().is_some_and(|members| {
        members.len() == names.len()
            && names
                .iter()
                .all(|name| members.iter().filter(|(key, _)| key == name).count() == 1)
    })
}

fn nonempty(value: Option<&Json>) -> bool {
    value.and_then(Json::as_str).is_some_and(|s| !s.is_empty())
}

/// Validate both positive and negative rows before selecting appearances.
fn valid_row(row: &Json) -> bool {
    match row.get("expected").and_then(Json::as_str) {
        Some("render") => {
            fields(
                row,
                &[
                    "page",
                    "expected",
                    "oracle",
                    "oracle_sha256",
                    "width",
                    "height",
                    "tolerance",
                ],
            ) && nonempty(row.get("oracle"))
                && row
                    .get("oracle_sha256")
                    .and_then(Json::as_str)
                    .is_some_and(|hash| {
                        hash.len() == 64
                            && hash
                                .bytes()
                                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
                    })
                && ["width", "height"]
                    .iter()
                    .all(|name| row.get(name).and_then(Json::as_u64).is_some_and(|n| n > 0))
                && row
                    .get("tolerance")
                    .and_then(Json::as_u64)
                    .is_some_and(|n| u8::try_from(n).is_ok())
        }
        Some("refused") => {
            let Some(error) = row.get("error") else {
                return false;
            };
            fields(row, &["page", "expected", "error"])
                && fields(error, &["kind", "message", "feature", "repair"])
                && nonempty(error.get("kind"))
                && nonempty(error.get("message"))
                && ["feature", "repair"].iter().all(|name| {
                    error
                        .get(name)
                        .is_some_and(|v| v.is_null() || nonempty(Some(v)))
                })
        }
        _ => false,
    }
}

/// Full known coverage; callers cannot silently skip a malformed contract.
fn page_rows<'a>(
    value: &'a Json,
    count: Option<&Json>,
    expected: Option<&str>,
) -> std::result::Result<Vec<(usize, &'a Json)>, &'static str> {
    let count = count
        .and_then(Json::as_u64)
        .and_then(|n| usize::try_from(n).ok())
        .filter(|&n| n > 0)
        .ok_or("page contract requires a known positive page count")?;
    if !matches!(expected, Some("open" | "render")) {
        return Err("page contract requires open or render");
    }
    let rows = value.as_array().ok_or("page contract must be an array")?;
    if rows.len() != count {
        return Err("page contract incomplete coverage");
    }
    let mut result = Vec::with_capacity(count);
    for row in rows {
        if !valid_row(row) {
            return Err("page contract malformed row");
        }
        let page = row
            .get("page")
            .and_then(Json::as_u64)
            .and_then(|n| usize::try_from(n).ok())
            .filter(|&n| n < count)
            .ok_or("page contract invalid page index")?;
        if expected == Some("render")
            && row.get("expected").and_then(Json::as_str) != Some("render")
        {
            return Err("page contract render document contains refusal");
        }
        result.push((page, row));
    }
    result.sort_by_key(|&(page, _)| page);
    if result
        .iter()
        .enumerate()
        .any(|(index, &(page, _))| index != page)
    {
        return Err("page contract duplicate or missing page");
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(text: &str) -> Json {
        let parsed = Json::parse(text);
        assert!(parsed.is_ok());
        parsed.unwrap_or(Json::Null)
    }

    const MIXED: &str = r#"[
        {"page":0,"expected":"render","oracle":"x.ppm","oracle_sha256":"0000000000000000000000000000000000000000000000000000000000000000","width":1,"height":1,"tolerance":0},
        {"page":1,"expected":"refused","error":{"kind":"limit","message":"bound","feature":null,"repair":null}}
    ]"#;

    #[test]
    fn complete_mixed_page_contract_keeps_every_positive() {
        let value = rows(MIXED);
        let count = Json::Number(2.0);
        let result = page_rows(&value, Some(&count), Some("open"));
        assert!(result.is_ok());
        let pages = result.unwrap_or_default();
        assert_eq!(pages.len(), 2);
        assert_eq!(pages[0].0, 0);
        assert_eq!(pages[1].0, 1);
        let reverse = rows(
            &MIXED
                .replace("\"page\":0", "\"page\":2")
                .replace("\"page\":1", "\"page\":0")
                .replace("\"page\":2", "\"page\":1"),
        );
        let result = page_rows(&reverse, Some(&count), Some("open"));
        assert!(result.is_ok());
        assert_eq!(
            result.unwrap_or_default()[1]
                .1
                .get("expected")
                .and_then(Json::as_str),
            Some("render")
        );
    }

    #[test]
    fn malformed_page_coverage_and_rows_never_disappear() {
        let count = Json::Number(2.0);
        for text in [
            MIXED.replace("\"page\":1", "\"page\":0"),
            MIXED.replace("\"page\":1", "\"page\":2"),
            MIXED.replace("\"page\":1", "\"page\":true"),
            MIXED.replace("\"repair\":null", "\"repair\":7"),
            MIXED.replace("\"tolerance\":0", "\"tolerance\":true"),
            MIXED.replace("\"width\":1", "\"width\":0"),
            MIXED.replace("\"message\":\"bound\",", ""),
            "[]".to_owned(),
        ] {
            assert!(
                page_rows(&rows(&text), Some(&count), Some("open")).is_err(),
                "{text}"
            );
        }
        assert!(page_rows(&rows(MIXED), Some(&count), Some("render")).is_err());
        assert!(page_rows(&rows(MIXED), Some(&Json::Bool(true)), Some("open")).is_err());
        assert!(page_rows(&rows(MIXED), None, Some("open")).is_err());
    }
}
