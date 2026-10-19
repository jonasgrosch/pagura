use super::*;
use crate::{Document, OpenOptions};
use pagura_core::{ErrorKind, Limits, Recovery};
use pagura_testkit::{fixture, fixture_path};
use std::collections::BTreeMap;

fn input<'a>(mode: &str, value: &'a str, name: &'a Name, indices: &'a [usize]) -> FillValue<'a> {
    match mode {
        "text" => FillValue::Text(value),
        "indices" => FillValue::Choice(ChoiceSelection::Indices(indices)),
        "choice-text" => FillValue::Choice(ChoiceSelection::Text(value)),
        "choice-export" => FillValue::Choice(ChoiceSelection::Export(value)),
        "off" => FillValue::Button(ButtonValue::Off),
        "widget" => FillValue::Button(ButtonValue::Widget {
            index: value.parse().unwrap(),
            state: None,
        }),
        "state" => FillValue::Button(ButtonValue::State(name)),
        "button-export" => FillValue::Button(ButtonValue::Export(value)),
        _ => panic!("unknown fixture input"),
    }
}
fn snapshot(change: &FilledField) -> String {
    match change.value() {
        FilledValue::Text(text) => format!("text|{text}"),
        FilledValue::Choice(c) => format!(
            "choice|{}|{}|{}",
            c.values().join(","),
            c.exports().join(","),
            c.indices()
                .iter()
                .map(usize::to_string)
                .collect::<Vec<_>>()
                .join(",")
        ),
        FilledValue::Button(n) => format!(
            "button|{}|{}",
            String::from_utf8_lossy(n.as_bytes()),
            change
                .widget_states()
                .iter()
                .map(|s| format!(
                    "{}:{}",
                    s.widget(),
                    String::from_utf8_lossy(s.state().as_bytes())
                ))
                .collect::<Vec<_>>()
                .join(",")
        ),
    }
}
#[test]
fn independent_field_replacements() {
    let mut count = 0;
    for entry in std::fs::read_dir(fixture_path("forms", "")).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_none_or(|e| e != "params") {
            continue;
        }
        let name = path.file_stem().unwrap().to_str().unwrap();
        let params = std::fs::read_to_string(&path).unwrap();
        let params: BTreeMap<_, _> = params.lines().map(|l| l.split_once('=').unwrap()).collect();
        if params["changed"] == "false" {
            continue;
        }
        count += 1;
        let doc = Document::open(
            fixture("forms", &format!("{name}.pdf")),
            &OpenOptions {
                recovery: Recovery::Strict,
                ..Default::default()
            },
        )
        .unwrap();
        let before = doc.acro_form().unwrap().unwrap().fields[0].value.clone();
        let mut filler = doc.form_filler().unwrap().unwrap();
        let bytes = String::from_utf8(fixture("forms", &format!("{name}.value"))).unwrap();
        let indices: Vec<_> = if params["mode"] == "indices" {
            bytes
                .split(',')
                .filter(|s| !s.is_empty())
                .map(|n| n.parse().unwrap())
                .collect()
        } else {
            Vec::new()
        };
        let state = Name::new(bytes.as_bytes());
        let initial = filler.storage.used();
        let result = filler.set(0, input(params["mode"], &bytes, &state, &indices));
        if params["error"].is_empty() {
            assert_eq!(
                snapshot(result.unwrap()),
                String::from_utf8(fixture("forms", &format!("{name}.expected"))).unwrap(),
                "{name}"
            );
            assert_eq!(filler.changes().count(), 1);
            // Idempotent replacement charges storage once.
            let used = filler.storage.used();
            filler
                .set(0, input(params["mode"], &bytes, &state, &indices))
                .unwrap();
            assert_eq!(filler.storage.used(), used, "{name}");
            filler.reset(0);
            assert_eq!(filler.storage.used(), initial);
            assert!(filler.get(0).is_none());
        } else {
            let error = result.unwrap_err();
            let actual = error.feature().map_or_else(
                || error.kind.name().to_string(),
                |f| format!("unsupported:{f}"),
            );
            assert_eq!(actual, params["error"], "{name}: {error}");
            assert!(error.object.is_some(), "{name}");
            assert_ne!(error.position, pagura_core::Position::Unknown, "{name}");
            assert_eq!(filler.changes().count(), 0);
        }
        assert_eq!(
            doc.acro_form().unwrap().unwrap().fields[0].value,
            before,
            "{name}"
        );
    }
    assert!(count >= 40);
}
#[test]
fn failed_sets_preserve_prior_values_and_source() {
    let doc = Document::open(fixture("forms", "text-comb.pdf"), &OpenOptions::default()).unwrap();
    let mut filler = doc.form_filler().unwrap().unwrap();
    filler.set(0, FillValue::Text("AB")).unwrap();
    let prior = filler.get(0).cloned();
    for input in [
        FillValue::Text("ABCDE"),
        FillValue::Button(ButtonValue::Off),
    ] {
        assert!(filler.set(0, input).is_err());
        assert_eq!(filler.get(0), prior.as_ref());
    }
    assert_eq!(
        filler.set(20, FillValue::Text("AB")).unwrap_err().kind,
        ErrorKind::InvalidArgument
    );
    assert_eq!(filler.get(0), prior.as_ref());
    assert_eq!(
        doc.object(pagura_core::ObjectId::new(11, 0))
            .unwrap()
            .dictionary()
            .unwrap()
            .get(b"V"),
        None
    );
}
#[test]
fn input_and_replacement_storage_budgets() {
    let doc = Document::open(
        fixture("forms", "text-quadding-0.pdf"),
        &OpenOptions {
            limits: Limits {
                string_bytes: 128,
                ..Default::default()
            },
            ..Default::default()
        },
    )
    .unwrap();
    let mut filler = doc.form_filler().unwrap().unwrap();
    assert_eq!(
        filler
            .set(0, FillValue::Text(&"A".repeat(129)))
            .unwrap_err()
            .kind,
        ErrorKind::Limit
    );
    filler.set(0, FillValue::Text("A")).unwrap();
}

#[test]
fn aggregate_replacements_release_storage_on_reset() {
    let document = Document::open(
        fixture("document", "forms-wide.pdf"),
        &OpenOptions {
            limits: Limits {
                document_bytes: 1_000_000,
                ..Default::default()
            },
            recovery: Recovery::Strict,
            ..Default::default()
        },
    )
    .unwrap();
    let mut filler = document.form_filler().unwrap().unwrap();
    let text = "A".repeat(16_000);
    let mut rejected = None;
    for index in 0..filler.form().fields.len() {
        if let Err(error) = filler.set(index, FillValue::Text(&text)) {
            assert_eq!(error.kind, ErrorKind::Limit);
            assert!(filler.get(index).is_none());
            rejected = Some(index);
            break;
        }
    }
    let rejected = rejected.unwrap();
    assert!(rejected > 0);
    assert_eq!(filler.changes().count(), rejected);
    filler.reset(0);
    filler.set(rejected, FillValue::Text(&text)).unwrap();
    assert_eq!(filler.changes().count(), rejected);
}
