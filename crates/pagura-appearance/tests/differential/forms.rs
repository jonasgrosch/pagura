//! Form appearances: [`pagura_appearance::prepare`] and
//! [`pagura_appearance::prepare_widget_with_policy`] against
//! `pagura_legacy::form_appearance`, byte for byte and error for error.

use crate::support::{corpus, open, open_with};
use pagura_appearance as new;
use pagura_core::{Diagnostic, Dictionary, Error, Limits, Name, ObjectId, Recovery, Result};
use pagura_document::OpenOptions;
use pagura_document::forms::{ButtonValue, ChoiceSelection, FieldKind, FillValue, FormFiller};
use pagura_legacy::form_appearance as old;
use pagura_text::Provider;
use std::collections::BTreeMap;
use std::path::Path;

/// An appearance in a form both implementations can be compared in.
#[derive(Debug, PartialEq)]
enum Common {
    Existing(ObjectId),
    Hidden,
    Generated {
        dictionary: Dictionary,
        content: Vec<u8>,
    },
}

fn from_new(appearance: new::Appearance) -> Common {
    match appearance {
        new::Appearance::Existing(id) => Common::Existing(id),
        new::Appearance::Hidden => Common::Hidden,
        new::Appearance::Generated(stream) => Common::Generated {
            dictionary: stream.dictionary().clone(),
            content: stream.content().to_vec(),
        },
    }
}

fn from_old(appearance: old::Appearance) -> Common {
    match appearance {
        old::Appearance::Existing(id) => Common::Existing(id),
        old::Appearance::Hidden => Common::Hidden,
        old::Appearance::Generated(stream) => Common::Generated {
            dictionary: stream.dictionary().clone(),
            content: stream.content().to_vec(),
        },
    }
}

/// One value set on one field before preparing.
#[derive(Debug, Clone)]
enum Edit {
    Text(String),
    Indices(Vec<usize>),
    ChoiceText(String),
    ChoiceExport(String),
    Widget(usize),
    Off,
    State(Name),
    Export(String),
}

impl Edit {
    fn apply(&self, filler: &mut FormFiller<'_>, field: usize) -> Result<()> {
        let value = match self {
            Self::Text(text) => FillValue::Text(text),
            Self::Indices(indices) => FillValue::Choice(ChoiceSelection::Indices(indices)),
            Self::ChoiceText(text) => FillValue::Choice(ChoiceSelection::Text(text)),
            Self::ChoiceExport(text) => FillValue::Choice(ChoiceSelection::Export(text)),
            Self::Widget(index) => FillValue::Button(ButtonValue::Widget {
                index: *index,
                state: None,
            }),
            Self::Off => FillValue::Button(ButtonValue::Off),
            Self::State(name) => FillValue::Button(ButtonValue::State(name)),
            Self::Export(text) => FillValue::Button(ButtonValue::Export(text)),
        };
        filler.set(field, value).map(|_| ())
    }
}

/// What was compared, to show that the comparison covers the fixtures.
#[derive(Debug, Default)]
struct Coverage {
    sessions: usize,
    generated: usize,
    existing: usize,
    hidden: usize,
    errors: BTreeMap<String, usize>,
}

impl Coverage {
    fn count(&mut self, result: &std::result::Result<Common, Error>) {
        match result {
            Ok(Common::Generated { .. }) => self.generated += 1,
            Ok(Common::Existing(_)) => self.existing += 1,
            Ok(Common::Hidden) => self.hidden += 1,
            Err(error) => {
                let tag = error.feature().map_or_else(
                    || error.kind.name().to_string(),
                    |feature| feature.as_str().to_string(),
                );
                *self.errors.entry(tag).or_default() += 1;
            }
        }
    }
}

/// Runs both implementations on twin documents with the same edits and
/// compares every prepared form, every widget under both policies, and the
/// diagnostics each document collected.
fn compare(
    name: &str,
    bytes: &[u8],
    options: &OpenOptions,
    edits: &[(usize, Edit)],
    provider: Option<&dyn Provider>,
    coverage: &mut Coverage,
) {
    let (Some(old_document), Some(new_document)) =
        (open_with(bytes, options), open_with(bytes, options))
    else {
        return;
    };
    let (Ok(Some(mut old_filler)), Ok(Some(mut new_filler))) =
        (old_document.form_filler(), new_document.form_filler())
    else {
        return;
    };
    for (field, edit) in edits {
        let old_set = edit.apply(&mut old_filler, *field);
        let new_set = edit.apply(&mut new_filler, *field);
        assert_eq!(old_set, new_set, "{name}: edit {field}");
    }
    coverage.sessions += 1;
    let expected = old::prepare(&old_filler, provider).map(|all| {
        all.into_iter()
            .map(|w| (w.widget(), from_old(w.appearance().clone())))
            .collect::<Vec<_>>()
    });
    let actual = new::prepare(&new_filler, provider).map(|all| {
        all.into_iter()
            .map(|w| (w.widget(), from_new(w.appearance().clone())))
            .collect::<Vec<_>>()
    });
    assert_eq!(actual, expected, "{name}: prepare");
    for index in 0..new_filler.form().widgets.len() {
        for (old_policy, new_policy) in [
            (
                old::UpdatePolicy::Specification,
                new::UpdatePolicy::Specification,
            ),
            (
                old::UpdatePolicy::PreserveExisting,
                new::UpdatePolicy::PreserveExisting,
            ),
        ] {
            let expected =
                old::prepare_widget_with_policy(&old_filler, index, provider, old_policy)
                    .map(from_old);
            let actual = new::prepare_widget_with_policy(&new_filler, index, provider, new_policy)
                .map(from_new);
            assert_eq!(actual, expected, "{name}: widget {index} {new_policy:?}");
            coverage.count(&actual);
        }
    }
    let diagnostics =
        |document: &pagura_document::Document| -> Vec<Diagnostic> { document.diagnostics() };
    assert_eq!(
        diagnostics(&new_document),
        diagnostics(&old_document),
        "{name}: diagnostics"
    );
}

/// A value for every field of the form, the same in every run: text,
/// the first option, and the first widget of a button.
fn synthetic_edits(bytes: &[u8]) -> Vec<(usize, Edit)> {
    let Some(document) = open(bytes, Recovery::Repair) else {
        return Vec::new();
    };
    let Ok(Some(filler)) = document.form_filler() else {
        return Vec::new();
    };
    let form = filler.form();
    let mut edits = Vec::new();
    for (index, field) in form.fields.iter().enumerate() {
        let edit = match &field.kind {
            FieldKind::Text { multiline, .. } => Edit::Text(if *multiline {
                "First line of text\nsecond line, wrapped where it does not fit".into()
            } else {
                "Ab 12".into()
            }),
            FieldKind::Choice { options, .. } if !options.is_empty() => {
                Edit::Indices(vec![options.len() - 1])
            }
            FieldKind::Choice { .. } => Edit::ChoiceText("Ab".into()),
            FieldKind::Button { .. } => match form.widgets.iter().position(|w| w.field == index) {
                Some(widget) => Edit::Widget(widget),
                None => continue,
            },
            _ => continue,
        };
        edits.push((index, edit));
    }
    edits
}

/// The edits a forms fixture's `.params` or `.render` file prescribes.
#[expect(
    clippy::unwrap_used,
    clippy::panic,
    reason = "a malformed fixture fails the test"
)]
fn fixture_edits(path: &Path) -> Vec<(usize, Edit)> {
    let mut edits = Vec::new();
    if let Ok(params) = std::fs::read_to_string(path.with_extension("params")) {
        let params: BTreeMap<_, _> = params.lines().filter_map(|l| l.split_once('=')).collect();
        let value = std::fs::read_to_string(path.with_extension("value")).unwrap_or_default();
        if params.get("changed") == Some(&"true") {
            let edit = match params["mode"] {
                "text" => Edit::Text(value),
                "indices" => Edit::Indices(
                    value
                        .split(',')
                        .filter(|s| !s.is_empty())
                        .map(|n| n.parse().unwrap())
                        .collect(),
                ),
                "choice-text" => Edit::ChoiceText(value),
                "choice-export" => Edit::ChoiceExport(value),
                "widget" => Edit::Widget(value.parse().unwrap()),
                "off" => Edit::Off,
                "state" => Edit::State(Name::new(value.as_bytes())),
                "button-export" => Edit::Export(value),
                mode => panic!("unknown fixture input {mode}"),
            };
            edits.push((0, edit));
        }
    }
    if let Ok(render) = std::fs::read_to_string(path.with_extension("render")) {
        for line in render.lines().filter_map(|l| l.strip_prefix("edit=")) {
            let mut parts = line.splitn(3, '|');
            let field = parts.next().unwrap().parse().unwrap();
            let kind = parts.next().unwrap();
            let value = parts.next().unwrap();
            let edit = match kind {
                "text" => Edit::Text(value.into()),
                "indices" => Edit::Indices(
                    value
                        .split(',')
                        .filter(|s| !s.is_empty())
                        .map(|n| n.parse().unwrap())
                        .collect(),
                ),
                "widget" => Edit::Widget(value.parse().unwrap()),
                "off" => Edit::Off,
                kind => panic!("unknown fixture edit {kind}"),
            };
            edits.push((field, edit));
        }
    }
    edits
}

/// Recovery modes and budgets small enough to stop generation at each of
/// its limits on some fixture.
fn option_sets() -> Vec<OpenOptions> {
    let limited = |limits: Limits| OpenOptions {
        limits,
        ..Default::default()
    };
    vec![
        OpenOptions::default(),
        OpenOptions {
            recovery: Recovery::Strict,
            ..Default::default()
        },
        limited(Limits {
            document_bytes: 24_000,
            ..Default::default()
        }),
        limited(Limits {
            operations: 300,
            ..Default::default()
        }),
        limited(Limits {
            stream_bytes: 160,
            ..Default::default()
        }),
        limited(Limits {
            string_bytes: 6,
            ..Default::default()
        }),
        limited(Limits {
            value_nodes: 4,
            ..Default::default()
        }),
        limited(Limits {
            depth: 2,
            ..Default::default()
        }),
    ]
}

#[test]
fn every_form_fixture_prepares_as_the_previous_generator() {
    let bundled = pagura_data::Bundled::new(Limits::default());
    let mut coverage = Coverage::default();
    let options = option_sets();
    for path in corpus() {
        let bytes = std::fs::read(&path).unwrap();
        if !bytes.windows(8).any(|w| w == b"AcroForm") {
            continue;
        }
        let name = path.display().to_string();
        let mut runs = vec![Vec::new(), synthetic_edits(&bytes)];
        let prescribed = fixture_edits(&path);
        if !prescribed.is_empty() {
            runs.push(prescribed);
        }
        for options in &options {
            for edits in &runs {
                for provider in [None, Some(&bundled as &dyn Provider)] {
                    compare(&name, &bytes, options, edits, provider, &mut coverage);
                }
            }
        }
    }
    eprintln!("form differential coverage: {coverage:?}");
    assert!(coverage.sessions > 3000, "{coverage:?}");
    assert!(coverage.generated > 2000, "{coverage:?}");
    assert!(coverage.errors.contains_key("limit"), "{coverage:?}");
}

#[test]
fn text_styles_match_the_previous_parser() {
    let limits = Limits::default();
    for bytes in [
        b"/Helv 0 Tf 0 g".as_slice(),
        b"0 g /F1 10 Tf 1 0 0 2 0 0 Tm",
        b"/F1 10 Tf 1 1 0 1 0 0 Tm",
        b"/F1 -1 Tf",
        b"/F1 12 Tf BT",
        b"12 Tf",
        b"",
    ] {
        let expected = old::default_text_style(bytes, limits);
        let actual = new::default_text_style(bytes, &limits)
            .map(|style| (style.font().clone(), style.size()));
        assert_eq!(actual, expected, "{}", String::from_utf8_lossy(bytes));
    }
}
