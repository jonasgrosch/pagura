use super::*;
use crate::{Document, OpenOptions};
use pagura_core::{ErrorKind, Limits, Position, Recovery, Result};
use pagura_testkit::{fixture, fixture_path};
use std::{collections::BTreeMap, fmt::Write};

fn open(name: &str, recovery: Recovery, limits: Limits) -> Document {
    Document::open(
        fixture("document", &format!("{name}.pdf")),
        &OpenOptions {
            limits,
            recovery,
            ..OpenOptions::default()
        },
    )
    .unwrap()
}
fn value(value: Option<&FieldValue<'_>>) -> String {
    match value {
        None => "_".into(),
        Some(FieldValue::Text(s)) => s.clone(),
        Some(FieldValue::Name(n)) => format!("/{}", String::from_utf8_lossy(n.as_bytes())),
        Some(FieldValue::Texts(v)) => format!("[{}]", v.join(",")),
        Some(FieldValue::Signature(_)) => "signature".into(),
        Some(FieldValue::Other(_)) => "other".into(),
    }
}
fn kind(kind: &FieldKind<'_>) -> String {
    match kind {
        FieldKind::Group => "Group".into(),
        FieldKind::Button {
            kind,
            export_values,
        } => format!("Btn:{kind:?}:{}", export_values.join(",")),
        FieldKind::Text {
            max_length,
            multiline,
            password,
            comb,
            rich_text,
            rich_value,
        } => format!(
            "Tx:{}:{multiline}:{password}:{comb}:{rich_text}:{}",
            max_length.map_or_else(|| "_".into(), |n| n.to_string()),
            rich_value.is_some()
        ),
        FieldKind::Choice {
            combo,
            editable,
            multi_select,
            options,
        } => format!(
            "Ch:{combo}:{editable}:{multi_select}:{}",
            options
                .iter()
                .map(|o| format!("{}>{}", o.export, o.display))
                .collect::<Vec<_>>()
                .join(",")
        ),
        FieldKind::Signature { signed } => format!("Sig:{signed}"),
        FieldKind::Other(n) => format!("Other:{}", String::from_utf8_lossy(n.as_bytes())),
    }
}
fn actions(sequence: Option<&crate::navigation::ActionSequence<'_>>) -> String {
    sequence.map_or_else(
        || "_".into(),
        |s| {
            s.actions
                .iter()
                .map(|a| {
                    String::from_utf8_lossy(
                        a.dictionary
                            .get(b"S")
                            .unwrap()
                            .as_name()
                            .unwrap()
                            .as_bytes(),
                    )
                })
                .collect::<Vec<_>>()
                .join(",")
        },
    )
}
fn additional(entries: &[AdditionalAction<'_>]) -> String {
    if entries.is_empty() {
        return "_".into();
    }
    entries
        .iter()
        .map(|a| {
            format!(
                "{}:{}",
                String::from_utf8_lossy(a.trigger.as_bytes()),
                actions(Some(&a.actions))
            )
        })
        .collect::<Vec<_>>()
        .join(",")
}
fn appearance(value: Option<&[u8]>) -> String {
    value.map_or_else(|| "_".into(), |v| String::from_utf8(v.to_vec()).unwrap())
}
fn justification(q: Justification) -> usize {
    match q {
        Justification::Left => 0,
        Justification::Centre => 1,
        Justification::Right => 2,
    }
}
fn snapshot(doc: &Document) -> Result<String> {
    let Some(form) = doc.acro_form()? else {
        return Ok("absent\n".into());
    };
    let mut out = String::new();
    writeln!(
        out,
        "form|{}|{}|{}|{}|{}|{}|{}",
        form.need_appearances,
        form.signature_flags,
        form.has_xfa,
        appearance(form.default_appearance),
        justification(form.justification),
        form.calculation_order.len(),
        form.default_resources.is_some()
    )
    .unwrap();
    for (i, f) in form.fields.iter().enumerate() {
        writeln!(
            out,
            "field|{i}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}",
            f.parent.map_or_else(|| "_".into(), |p| p.to_string()),
            f.qualified_name,
            f.partial_name.as_deref().unwrap_or("_"),
            kind(&f.kind),
            f.flags.0,
            value(f.value.as_ref()),
            value(f.default_value.as_ref()),
            appearance(f.default_appearance),
            justification(f.justification),
            f.alternative_name.as_deref().unwrap_or("_"),
            f.mapping_name.as_deref().unwrap_or("_"),
            additional(&f.additional_actions)
        )
        .unwrap();
    }
    for w in &form.widgets {
        let r = w.rectangle;
        writeln!(
            out,
            "widget|{}|{}|{},{},{},{}|{}|{}|{}|{}",
            w.field,
            w.merged,
            r.x0,
            r.y0,
            r.x1,
            r.y1,
            if w.on_states.is_empty() {
                "_".into()
            } else {
                w.on_states
                    .iter()
                    .map(|n| String::from_utf8_lossy(n.as_bytes()))
                    .collect::<Vec<_>>()
                    .join(",")
            },
            w.appearance_state
                .map_or_else(|| "_".into(), |n| String::from_utf8_lossy(n.as_bytes())),
            actions(w.action.as_ref()),
            additional(&w.additional_actions)
        )
        .unwrap();
        assert!(w.object.is_some());
        assert_ne!(w.position, Position::Unknown);
    }
    Ok(out)
}
#[test]
fn independent_form_models_and_reported_repairs() {
    let mut count = 0;
    for path in std::fs::read_dir(fixture_path("document", ""))
        .unwrap()
        .map(|p| p.unwrap().path())
    {
        if path.extension().is_none_or(|s| s != "params") {
            continue;
        }
        let name = path.file_stem().unwrap().to_str().unwrap();
        if !name.starts_with("forms-") {
            continue;
        }
        count += 1;
        let params = std::fs::read_to_string(&path).unwrap();
        let params: BTreeMap<_, _> = params.lines().map(|l| l.split_once('=').unwrap()).collect();
        let expected = String::from_utf8(fixture("document", &format!("{name}.expected"))).unwrap();
        let strict = open(name, Recovery::Strict, Limits::default());
        let actual = snapshot(&strict);
        if !params["error"].is_empty() {
            assert_eq!(actual.unwrap_err().kind.name(), params["error"], "{name}");
            let repaired = open(name, Recovery::Repair, Limits::default());
            assert_eq!(
                snapshot(&repaired).unwrap_err().kind.name(),
                params["error"],
                "{name}"
            );
            continue;
        }
        if params["repair"].is_empty() {
            assert_eq!(actual.unwrap(), expected, "{name}");
        } else {
            let error = actual.unwrap_err();
            assert_eq!(
                error.repair.unwrap().as_str(),
                params["repair"],
                "{name}: {error}"
            );
            assert!(error.object.is_some(), "{name}");
            assert_ne!(error.position, Position::Unknown, "{name}");
        }
        let repaired = open(name, Recovery::Repair, Limits::default());
        assert_eq!(snapshot(&repaired).unwrap(), expected, "{name}");
        if params["repair"].is_empty() {
            assert_eq!(repaired.diagnostics(), []);
        } else {
            assert!(
                repaired
                    .diagnostics()
                    .iter()
                    .any(|d| d.tag() == params["repair"]),
                "{name}"
            );
        }
        let diagnostics = repaired.diagnostics();
        assert_eq!(snapshot(&repaired).unwrap(), expected, "{name}");
        assert_eq!(repaired.diagnostics(), diagnostics);
    }
    assert!(count >= 20);
}
#[test]
fn form_budgets_and_absence() {
    for limits in [
        Limits {
            depth: 16,
            ..Limits::default()
        },
        Limits {
            value_nodes: 120,
            ..Limits::default()
        },
        Limits {
            document_bytes: 32_000,
            ..Limits::default()
        },
    ] {
        let doc = open("forms-deep", Recovery::Repair, limits);
        let error = doc.acro_form().unwrap_err();
        assert_eq!(error.kind, ErrorKind::Limit, "{error}");
        assert!(error.object.is_some());
    }
    let doc = open(
        "navigation-destinations",
        Recovery::Strict,
        Limits::default(),
    );
    assert!(doc.acro_form().unwrap().is_none());
}
#[test]
fn resources_are_borrowed_and_cache_admission_is_not_repeated() {
    let doc = open(
        "forms-types-inheritance",
        Recovery::Strict,
        Limits::default(),
    );
    let first = doc.acro_form().unwrap().unwrap();
    let bytes = doc.retained.used();
    let second = doc.acro_form().unwrap().unwrap();
    assert!(std::ptr::eq(
        first.default_resources.unwrap(),
        second.default_resources.unwrap()
    ));
    assert_eq!(doc.retained.used(), bytes);
    assert!(first.fields[0].flags.read_only());
    assert!(!first.fields[0].flags.required());
    assert!(!first.fields[0].flags.no_export());
    assert!(!FieldFlags(u32::MAX).contains(0));
    assert!(!FieldFlags(u32::MAX).contains(33));
    assert!(FieldFlags(u32::MAX).contains(32));
    let radio: Vec<_> = first
        .widgets
        .iter()
        .filter(|w| {
            matches!(
                first.fields[w.field].kind,
                FieldKind::Button {
                    kind: ButtonKind::Radio,
                    ..
                }
            )
        })
        .collect();
    assert_eq!(radio.len(), 2);
    assert_eq!(radio[0].on_states, radio[1].on_states);
}

#[test]
fn duplicate_comparison_shares_the_traversal_budget() {
    let doc = open(
        "forms-duplicate-equivalent",
        Recovery::Strict,
        Limits {
            value_nodes: 32,
            ..Limits::default()
        },
    );
    let a = pagura_core::Value::Array(vec![pagura_core::Value::Integer(1); 64]);
    let b = a.clone();
    let mut ctx = crate::navigation::context::Context::new(&doc);
    let source = ctx.source(pagura_core::ObjectId::new(11, 0)).unwrap();
    let error = names::equivalent(&mut ctx, Some(&a), Some(&b), source, false).unwrap_err();
    assert_eq!(error.kind, ErrorKind::Limit);
    assert_eq!(error.object, source.object);
}
