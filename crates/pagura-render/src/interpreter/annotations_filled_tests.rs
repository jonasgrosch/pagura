use crate::{RenderContext, RenderOptions, compile_filled_page, compile_page};
use pagura_core::{DiagnosticKind, ErrorKind, Recovery};
use pagura_document::{
    Document, OpenOptions,
    forms::{ButtonValue, ChoiceSelection, FillValue, FormFiller},
};
use pagura_testkit::{fixture, fixture_path};
use std::sync::Arc;

fn edit(filler: &mut FormFiller<'_>, line: &str) {
    let mut parts = line.splitn(3, '|');
    let field = parts.next().unwrap().parse().unwrap();
    let kind = parts.next().unwrap();
    let value = parts.next().unwrap();
    let indices: Vec<usize> = if kind == "indices" {
        value
            .split(',')
            .filter(|s| !s.is_empty())
            .map(|n| n.parse().unwrap())
            .collect()
    } else {
        Vec::new()
    };
    let input = match kind {
        "text" => FillValue::Text(value),
        "indices" => FillValue::Choice(ChoiceSelection::Indices(&indices)),
        "widget" => FillValue::Button(ButtonValue::Widget {
            index: value.parse().unwrap(),
            state: None,
        }),
        "off" => FillValue::Button(ButtonValue::Off),
        _ => panic!("unknown fixture edit"),
    };
    filler.set(field, input).unwrap();
}

fn assert_pixels(list: &crate::DisplayList, name: &str, scale: f64) {
    let options = RenderOptions {
        scale,
        ..Default::default()
    };
    let image = list.render(&options).unwrap();
    let actual = pagura_testkit::ppm(
        image.width(),
        image.height(),
        &image.to_rgb_over(pagura_raster::Rgba8::WHITE),
    );
    let expected = fixture("forms", &format!("{name}.rgb"));
    assert_eq!(actual.len(), expected.len(), "{name}");
    let header = format!("P6\n{} {}\n255\n", image.width(), image.height());
    assert_eq!(&actual[..header.len()], &expected[..header.len()], "{name}");
    for (i, (&a, &b)) in actual[header.len()..]
        .iter()
        .zip(&expected[header.len()..])
        .enumerate()
    {
        assert!(a.abs_diff(b) <= 1, "{name}: sample {i}: {a}, exact {b}");
    }
    // A compiled filled page has the same tile contract as an ordinary page.
    let tile = crate::Tile {
        x: 1,
        y: 3,
        width: 17,
        height: 13,
    };
    let tile_image = list.render_tile(&options, tile).unwrap();
    let rgb = image.to_rgb_over(pagura_raster::Rgba8::WHITE);
    let expected: Vec<u8> = (tile.y..tile.y + tile.height)
        .flat_map(|y| {
            let start = ((y * image.width() + tile.x) * 3) as usize;
            rgb[start..start + tile.width as usize * 3].iter().copied()
        })
        .collect();
    assert_eq!(
        tile_image.to_rgb_over(pagura_raster::Rgba8::WHITE),
        expected,
        "{name}: tile"
    );
}

#[test]
fn filled_widgets_match_independent_exact_area_pixels() {
    let mut count = 0;
    for entry in std::fs::read_dir(fixture_path("forms", "")).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_none_or(|e| e != "render") {
            continue;
        }
        count += 1;
        let name = path.file_stem().unwrap().to_str().unwrap();
        let document = Arc::new(
            Document::open(
                fixture("forms", &format!("{name}.pdf")),
                &OpenOptions {
                    recovery: Recovery::Strict,
                    ..Default::default()
                },
            )
            .unwrap(),
        );
        let params = std::fs::read_to_string(&path).unwrap();
        let mut filler = document.form_filler().unwrap().unwrap();
        for line in params.lines().filter_map(|l| l.strip_prefix("edit=")) {
            edit(&mut filler, line);
        }
        let value = |key: &str| params.lines().find_map(|l| l.strip_prefix(key)).unwrap();
        let provider = if value("provider=") == "rectangle" {
            Some(Arc::new(RectangleProvider::new(name)) as Arc<dyn pagura_text::Provider>)
        } else {
            None
        };
        let context = provider.as_ref().map_or_else(
            || RenderContext::new(Arc::clone(&document)),
            |p| RenderContext::with_provider(Arc::clone(&document), Arc::clone(p)),
        );
        let page = document.page(value("page=").parse().unwrap()).unwrap();
        if !value("error=").is_empty() {
            let error = compile_filled_page(&filler, page, None).unwrap_err();
            assert_eq!(error.kind.name(), value("error="), "{name}: {error}");
            assert!(error.object.is_some(), "{name}");
            if name == "render-degenerate-matrix" {
                assert_eq!(error.message, "degenerate appearance bounds");
            }
            if name == "render-content-error-position" {
                assert_eq!(error.object, Some(pagura_core::ObjectId::new(11, 0)));
                assert_eq!(error.position, pagura_core::Position::Buffer(0));
            }
            assert!(context.compile_filled_page(&filler, page).is_err());
            continue;
        }
        let scale = value("scale=").parse().unwrap();
        let list = compile_filled_page(&filler, page, provider.as_deref())
            .unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_pixels(&list, name, scale);
        let cached = context
            .compile_filled_page(&filler, page)
            .unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_pixels(&cached, name, scale);
        let kinds: Vec<_> = document.diagnostics().iter().map(|d| d.kind).collect();
        let mut expected = Vec::new();
        if provider.is_some() {
            expected.push(DiagnosticKind::Information(pagura_text::SUBSTITUTED));
        }
        if let Some(tag) = params.lines().find_map(|l| l.strip_prefix("diagnostic=")) {
            assert_eq!(tag, crate::repairs::APPEARANCE_KEPT);
            let kept = DiagnosticKind::Information(crate::repairs::APPEARANCE_KEPT);
            expected.push(kept);
            let diagnostic = document
                .diagnostics()
                .into_iter()
                .find(|d| d.kind == kept)
                .unwrap();
            assert_eq!(diagnostic.object, Some(pagura_core::ObjectId::new(11, 0)));
            assert_ne!(diagnostic.position, pagura_core::Position::Unknown);
        }
        assert_eq!(kinds, expected, "{name}");
    }
    assert_eq!(count, 56);
}

#[test]
fn repeated_edits_and_reset_do_not_mutate_source_or_cache_stale_appearances() {
    let document = Arc::new(
        Document::open(
            fixture("forms", "render-text-update.pdf"),
            &OpenOptions::default(),
        )
        .unwrap(),
    );
    let context = RenderContext::new(Arc::clone(&document));
    let page = document.page(0).unwrap();
    let source = compile_page(&document, page)
        .unwrap()
        .render(&RenderOptions::default())
        .unwrap();
    let mut filler = document.form_filler().unwrap().unwrap();
    for _ in 0..3 {
        filler.set(0, FillValue::Text("AB")).unwrap();
        assert_pixels(
            &context.compile_filled_page(&filler, page).unwrap(),
            "render-text-update",
            1.0,
        );
        filler.set(0, FillValue::Text("C")).unwrap();
        let changed = context
            .compile_filled_page(&filler, page)
            .unwrap()
            .render(&RenderOptions::default())
            .unwrap();
        assert_ne!(changed.data(), source.data());
        filler.reset(0);
        let reset = context
            .compile_filled_page(&filler, page)
            .unwrap()
            .render(&RenderOptions::default())
            .unwrap();
        assert_eq!(reset.data(), source.data());
    }
    assert_eq!(
        compile_page(&document, page)
            .unwrap()
            .render(&RenderOptions::default())
            .unwrap()
            .data(),
        source.data()
    );
}

#[test]
fn filled_page_and_context_reject_foreign_document_identities() {
    let open = || {
        Arc::new(
            Document::open(fixture("forms", "render-text.pdf"), &OpenOptions::default()).unwrap(),
        )
    };
    let a = open();
    let b = open();
    let filler = a.form_filler().unwrap().unwrap();
    assert_eq!(
        compile_filled_page(&filler, b.page(0).unwrap(), None)
            .unwrap_err()
            .kind,
        ErrorKind::InvalidArgument
    );
    let context = RenderContext::new(Arc::clone(&b));
    assert_eq!(
        context
            .compile_filled_page(&filler, a.page(0).unwrap())
            .unwrap_err()
            .kind,
        ErrorKind::InvalidArgument
    );
    assert_eq!(
        context
            .compile_filled_page(&filler, b.page(0).unwrap())
            .unwrap_err()
            .kind,
        ErrorKind::InvalidArgument
    );
}

#[test]
fn empty_appearances_keep_reported_recovery_and_skip_generation() {
    let document = Document::open(
        fixture("forms", "render-empty-bbox.pdf"),
        &OpenOptions::default(),
    )
    .unwrap();
    let mut filler = document.form_filler().unwrap().unwrap();
    filler.set(0, FillValue::Text("AB")).unwrap();
    for _ in 0..2 {
        let image = compile_filled_page(&filler, document.page(0).unwrap(), None)
            .unwrap()
            .render(&RenderOptions::default())
            .unwrap();
        assert!(
            image
                .to_rgb_over(pagura_raster::Rgba8::WHITE)
                .iter()
                .all(|&v| v == 255)
        );
    }
    let diagnostics = document.diagnostics();
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(
        diagnostics[0].repair(),
        Some(crate::repairs::APPEARANCE_EMPTY)
    );
    assert_eq!(
        diagnostics[0].object,
        Some(pagura_core::ObjectId::new(80, 0))
    );
}

#[test]
fn filled_compilation_keeps_operation_and_display_budgets() {
    for limits in [
        pagura_core::Limits {
            operations: 1,
            ..Default::default()
        },
        pagura_core::Limits {
            display_commands: 1,
            ..Default::default()
        },
        pagura_core::Limits {
            decoded_bytes: 32,
            ..Default::default()
        },
    ] {
        let document = Document::open(
            fixture("forms", "render-text.pdf"),
            &OpenOptions {
                limits,
                ..Default::default()
            },
        )
        .unwrap();
        let mut filler = document.form_filler().unwrap().unwrap();
        filler.set(0, FillValue::Text("A")).unwrap();
        let error = compile_filled_page(&filler, document.page(0).unwrap(), None).unwrap_err();
        assert_eq!(error.kind, ErrorKind::Limit);
        assert!(error.object.is_some());
    }
}

struct RectangleProvider(Arc<pagura_text::StandardFont>);
impl RectangleProvider {
    fn new(name: &str) -> Self {
        let limits = pagura_core::Limits::default();
        let program = pagura_font::FontProgram::parse(
            fixture("forms", &format!("{name}.font")),
            pagura_font::FontFormat::TrueType,
            &limits,
        )
        .unwrap();
        let mut encoding = std::array::from_fn(|_| None);
        encoding[52] = Some(pagura_core::Name::new(b"A"));
        Self(Arc::new(pagura_text::StandardFont {
            metrics: Arc::new(pagura_text::StandardMetrics {
                name: pagura_core::Name::new(b"ZapfDingbats"),
                encoding,
                widths: [(pagura_core::Name::new(b"A"), 600.0)]
                    .into_iter()
                    .collect(),
                descriptor: pagura_text::StandardDescriptor::default(),
            }),
            program: Some(Arc::new(program)),
        }))
    }
}
impl pagura_text::Provider for RectangleProvider {
    fn standard_font(
        &self,
        base: &[u8],
    ) -> pagura_core::Result<Option<Arc<pagura_text::StandardFont>>> {
        Ok((base == b"ZapfDingbats").then(|| Arc::clone(&self.0)))
    }
    fn predefined_cmap(
        &self,
        _: &[u8],
    ) -> pagura_core::Result<Option<Arc<pagura_text::PredefinedCMap>>> {
        Ok(None)
    }
    fn cid_to_unicode(
        &self,
        _: &[u8],
        _: &[u8],
    ) -> pagura_core::Result<Option<Arc<pagura_text::CidToUnicode>>> {
        Ok(None)
    }
}

mod cache;
mod fallback;

#[test]
fn an_untouched_form_renders_as_compile_page_renders_it() {
    let mut compared = 0;
    for area in ["forms", "signatures", "recovery"] {
        for name in pagura_testkit::fixture_names(area, "pdf") {
            let Ok(document) = Document::open(
                fixture(area, &format!("{name}.pdf")),
                &OpenOptions::default(),
            ) else {
                continue;
            };
            let Ok(Some(filler)) = document.form_filler() else {
                continue;
            };
            if filler.form().need_appearances {
                continue;
            }
            for page in document.pages().unwrap() {
                let Ok(plain) = compile_page(&document, page) else {
                    continue;
                };
                let filled = compile_filled_page(&filler, page, None).unwrap();
                let options = RenderOptions::default();
                assert_eq!(
                    filled.render(&options).map(|p| p.data().to_vec()).ok(),
                    plain.render(&options).map(|p| p.data().to_vec()).ok(),
                    "{area}/{name}"
                );
                compared += 1;
            }
        }
    }
    assert!(compared > 20, "{compared}");
}
