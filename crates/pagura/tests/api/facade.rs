//! The facade is complete: a caller names every type it needs through
//! `pagura` alone. This module imports nothing from the other workspace
//! crates (fixtures come from `pagura-testkit`, which is std-only).

use crate::support::{open, open_strict};
use pagura::form_appearance::Appearance;
use pagura::forms::FillValue;
use pagura::provider::{
    CidToUnicode, FontProgram, PredefinedCMap, Provider, StandardFont, default_provider,
};
use pagura::text::{LayoutOptions, Quad, SearchOptions, TextMatch, TextPage, WritingMode};
use pagura::{Document, Limits, Point, Recovery, RenderOptions};
use pagura_testkit::fixture;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

/// A provider written against the facade: it counts requests and forwards
/// them to the feature-selected data.
struct Forwarding {
    inner: Option<Arc<dyn Provider>>,
    requests: AtomicUsize,
}

impl Forwarding {
    fn new(limits: Limits) -> Self {
        Self {
            inner: default_provider(limits),
            requests: AtomicUsize::new(0),
        }
    }

    fn inner(&self) -> Option<&dyn Provider> {
        self.requests.fetch_add(1, Ordering::Relaxed);
        self.inner.as_deref()
    }
}

impl Provider for Forwarding {
    fn cid_font(
        &self,
        registry: &[u8],
        ordering: &[u8],
    ) -> pagura::Result<Option<Arc<FontProgram>>> {
        self.inner()
            .map_or(Ok(None), |inner| inner.cid_font(registry, ordering))
    }

    fn standard_font(&self, name: &[u8]) -> pagura::Result<Option<Arc<StandardFont>>> {
        self.inner()
            .map_or(Ok(None), |inner| inner.standard_font(name))
    }

    fn predefined_cmap(&self, name: &[u8]) -> pagura::Result<Option<PredefinedCMap>> {
        self.inner()
            .map_or(Ok(None), |inner| inner.predefined_cmap(name))
    }

    fn cid_to_unicode(
        &self,
        registry: &[u8],
        ordering: &[u8],
    ) -> pagura::Result<Option<Arc<CidToUnicode>>> {
        self.inner()
            .map_or(Ok(None), |inner| inner.cid_to_unicode(registry, ordering))
    }
}

#[test]
fn a_facade_only_provider_matches_the_default() {
    let bytes = fixture("paint", "bundled-helvetica.pdf");
    let document = Arc::new(open(bytes.clone()).unwrap());
    let provider = Arc::new(Forwarding::new(document.limits()));
    let custom = pagura::render_context_with_provider(document, Some(provider.clone()));
    let default = pagura::render_context(Arc::new(open(bytes).unwrap()));
    let options = RenderOptions::default();
    let page = |context: &pagura::RenderContext| {
        let page = context.document().page(0).unwrap();
        context.render_page(page, &options)
    };
    match (page(&custom), page(&default)) {
        (Ok(custom), Ok(default)) => assert_eq!(custom.data(), default.data()),
        (Err(custom), Err(default)) => {
            assert_eq!(
                custom
                    .as_error()
                    .unwrap()
                    .feature()
                    .map(pagura::Feature::as_str),
                Some("font.non-embedded")
            );
            assert_eq!(
                custom.as_error().unwrap().feature(),
                default.as_error().unwrap().feature()
            );
        }
        (custom, default) => panic!("custom {custom:?}, default {default:?}"),
    }
    assert!(provider.requests.load(Ordering::Relaxed) > 0);
}

#[test]
fn text_layout_and_search_need_only_the_text_module() {
    let document = open(fixture("text", "extract-tj.pdf")).unwrap();
    let text: TextPage = pagura::text_page(&document, document.page(0).unwrap()).unwrap();
    let options = SearchOptions {
        case_insensitive: true,
        ..SearchOptions::default()
    };
    let matches: Vec<TextMatch> = text.find("abcd", options).unwrap();
    assert_eq!(matches.len(), 1);
    let Quad([corner, ..]) = matches[0].quads[0];
    assert_eq!(corner, Point { x: 10.0, y: 60.0 });
    assert!(
        text.glyphs()
            .iter()
            .all(|glyph| glyph.writing_mode == WritingMode::Horizontal)
    );
    let hidden = text
        .with_layout(LayoutOptions {
            include_hidden: true,
            ..LayoutOptions::default()
        })
        .unwrap();
    assert_eq!(hidden.text(), text.text());
}

#[test]
fn forms_and_their_appearances_need_only_the_facade() {
    let document: Document = open_strict(fixture("forms", "text-beside-signature.pdf")).unwrap();
    assert_eq!(document.recovery(), Recovery::Strict);
    let form = document.acro_form().unwrap().unwrap();
    assert_eq!(form.widgets.len(), 2);
    let mut filler = document.form_filler().unwrap().unwrap();
    filler.set(0, FillValue::Text("AB")).unwrap();
    let appearances = pagura::prepare_form(&filler).unwrap();
    assert!(matches!(
        appearances[0].appearance(),
        Appearance::Generated(_)
    ));
}

#[test]
fn every_notice_names_its_subject() {
    let notices: Vec<_> = pagura::notices().collect();
    assert!(
        notices
            .iter()
            .all(|(subject, text)| !subject.is_empty() && !text.is_empty())
    );
    assert_eq!(notices.len() > 1, pagura::provider::BUNDLED);
}
