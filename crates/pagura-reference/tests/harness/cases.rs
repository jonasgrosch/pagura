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

/// A case's first page recorded as a scene, with its device geometry.
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
    /// Opens the case and records its first page.
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
        let page = document.page(0)?;
        let scene = Interpreter::new(&document)
            .with_provider(Some(provider))
            .scene(page)?;
        let scale = self.dpi / 72.0;
        let (width, height) = scene.page.device_size(scale)?;
        Ok(Recorded {
            transform: scene.page.device_transform(scale),
            page: DeviceRect {
                x0: 0,
                y0: 0,
                x1: width,
                y1: height,
            },
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
