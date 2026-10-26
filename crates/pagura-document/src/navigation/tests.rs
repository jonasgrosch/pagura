use super::*;
use pagura_core::{ErrorKind, Limits, Name, Position, Recovery, Value};
use pagura_testkit::{fixture, fixture_path};
use std::{collections::BTreeMap, fmt::Write};

use crate::tests::document as open;

fn view(v: DestinationView) -> String {
    let num = |n: Option<f64>| n.map_or_else(|| "_".into(), |n| n.to_string());
    match v {
        DestinationView::Xyz { left, top, zoom } => {
            format!("XYZ:{},{},{}", num(left), num(top), num(zoom))
        }
        DestinationView::Fit => "Fit".into(),
        DestinationView::FitB => "FitB".into(),
        DestinationView::FitH(v) => format!("FitH:{}", num(v)),
        DestinationView::FitV(v) => format!("FitV:{}", num(v)),
        DestinationView::FitBh(v) => format!("FitBH:{}", num(v)),
        DestinationView::FitBv(v) => format!("FitBV:{}", num(v)),
        DestinationView::FitR(r) => format!("FitR:{},{},{},{}", r.x0(), r.y0(), r.x1(), r.y1()),
    }
}

fn action(kind: &ActionKind<'_>) -> String {
    match kind {
        ActionKind::GoTo(dictionary) => {
            format!("GoTo:{}:{}", dictionary.page, view(dictionary.view))
        }
        ActionKind::GoToR {
            destination: RemoteDestination::Explicit { page, view: v },
            new_window,
            ..
        } => format!("GoToR:{page}:{}:{}", view(*v), new_window.unwrap_or(false)),
        ActionKind::GoToR {
            destination: RemoteDestination::Named(name),
            ..
        } => format!("GoToR:name:{}", String::from_utf8_lossy(name.as_bytes())),
        ActionKind::Uri { uri, .. } => format!("URI:{}", String::from_utf8_lossy(uri)),
        ActionKind::Named(name) => format!("Named:{name:?}"),
        ActionKind::Launch {
            file: Some(file), ..
        } => format!(
            "Launch:{}",
            String::from_utf8_lossy(file.as_string().unwrap())
        ),
        ActionKind::Launch { file: None, .. } => "Launch:platform".into(),
        ActionKind::JavaScript(Script::String(_)) => "JavaScript:string".into(),
        ActionKind::JavaScript(Script::Stream(_)) => "JavaScript:stream".into(),
        _ => panic!("unexpected fixture action"),
    }
}

fn target(target: Option<&Target<'_>>) -> String {
    match target {
        Some(Target::Destination(dictionary)) => {
            format!("dest:{}:{}", dictionary.page, view(dictionary.view))
        }
        Some(Target::Actions(actions)) => actions
            .actions
            .iter()
            .map(|a| action(&a.kind))
            .collect::<Vec<_>>()
            .join(","),
        None => "none".into(),
    }
}

fn root(document: &Document) -> &Value {
    document
        .resolve_dictionary(document.catalog().unwrap().get(b"Names").unwrap())
        .unwrap()
        .get(b"Dests")
        .unwrap()
}

fn snapshot(document: &Document, kind: &str) -> ReadResult<String> {
    let mut out = String::new();
    match kind {
        "destinations" => {
            for dictionary in document.named_destinations()? {
                writeln!(
                    out,
                    "{}|{}|{}",
                    String::from_utf8_lossy(dictionary.name.as_bytes()),
                    dictionary.destination.page,
                    view(dictionary.destination.view)
                )
                .unwrap();
            }
        }
        "trees" => {
            for (key, value) in document.name_tree(root(document)).entries()? {
                let dictionary = document.destination(value)?;
                writeln!(
                    out,
                    "{}|{}|{}",
                    String::from_utf8_lossy(key),
                    dictionary.page,
                    view(dictionary.view)
                )
                .unwrap();
            }
        }
        "outlines" => {
            for item in document.outlines()? {
                writeln!(
                    out,
                    "{}|{}|{}|{}|{},{},{}|{},{}|{}",
                    item.title,
                    item.parent.map_or_else(|| "-".into(), |v| v.to_string()),
                    item.depth,
                    u8::from(item.open),
                    item.color[0],
                    item.color[1],
                    item.color[2],
                    u8::from(item.italic),
                    u8::from(item.bold),
                    target(item.target.as_ref())
                )
                .unwrap();
            }
        }
        "links" => {
            for (i, link) in document.pages()?[0].links(document)?.iter().enumerate() {
                let r = link.rectangle;
                let h = match link.highlight {
                    HighlightMode::None => 0,
                    HighlightMode::Invert => 1,
                    HighlightMode::Outline => 2,
                    HighlightMode::Push => 3,
                };
                writeln!(
                    out,
                    "{i}|{},{},{},{}|{}|{}|{h}|{}",
                    r.x0(),
                    r.y0(),
                    r.x1(),
                    r.y1(),
                    link.quad_points.len(),
                    link.border.width,
                    target(link.target.as_ref())
                )
                .unwrap();
            }
        }
        "labels" => {
            for page in document.pages()? {
                writeln!(out, "{}", page.label(document)?).unwrap();
            }
        }
        _ => panic!("unknown navigation vector"),
    }
    Ok(out)
}

#[test]
fn independent_navigation_structures_and_reported_repairs() {
    for path in std::fs::read_dir(fixture_path("document", ""))
        .unwrap()
        .map(|p| p.unwrap().path())
    {
        if path.extension().is_none_or(|s| s != "params") {
            continue;
        }
        let name = path.file_stem().unwrap().to_str().unwrap();
        if !name.starts_with("navigation-") {
            continue;
        }
        let params = std::fs::read_to_string(&path).unwrap();
        let params: BTreeMap<_, _> = params.lines().map(|l| l.split_once('=').unwrap()).collect();
        let expected = String::from_utf8(fixture("document", &format!("{name}.expected"))).unwrap();
        let strict = open(name, Recovery::Strict, Limits::default());
        let actual = snapshot(&strict, params["view"]);
        if let Some(error) = params.get("error") {
            assert_eq!(
                actual.unwrap_err().as_error().unwrap().kind.name(),
                *error,
                "{name}"
            );
            let repaired = open(name, Recovery::Repair, Limits::default());
            assert_eq!(
                snapshot(&repaired, params["view"])
                    .unwrap_err()
                    .as_error()
                    .unwrap()
                    .kind
                    .name(),
                *error,
                "{name}"
            );
            continue;
        }
        if params["repair"].is_empty() {
            assert_eq!(actual.unwrap(), expected, "{name}");
        } else {
            let fault = actual.unwrap_err();
            let error = fault.as_error().unwrap();
            assert_eq!(
                error.repair.unwrap().as_str(),
                params["repair"],
                "{name}: {error}"
            );
            assert!(error.object.is_some(), "{name}: {error}");
            assert_ne!(error.position, Position::Unknown, "{name}: {error}");
        }
        let document = open(name, Recovery::Repair, Limits::default());
        assert_eq!(
            snapshot(&document, params["view"]).unwrap(),
            expected,
            "{name}"
        );
        if !params["repair"].is_empty() {
            assert!(
                document
                    .diagnostics()
                    .iter()
                    .any(|dictionary| dictionary.tag() == params["repair"]),
                "{name}"
            );
        }
        let reports = document.diagnostics();
        assert_eq!(snapshot(&document, params["view"]).unwrap(), expected);
        assert_eq!(document.diagnostics(), reports);
        if let Some(styles) = params.get("border_styles") {
            let links = document.pages().unwrap()[0].links(&document).unwrap();
            assert_eq!(
                links
                    .iter()
                    .map(|l| format!("{:?}", l.border.style))
                    .collect::<Vec<_>>()
                    .join(","),
                *styles
            );
        }
        if let Some(query) = params.get("query") {
            let value = document
                .name_tree(root(&document))
                .get(query.as_bytes())
                .unwrap()
                .unwrap();
            assert_eq!(
                document.destination(value).unwrap().page,
                params["page"].parse::<usize>().unwrap()
            );
        }
    }
}

#[test]
fn lookup_uses_limits_and_both_named_destination_locations() {
    let document = open("navigation-tree-wide", Recovery::Strict, Limits::default());
    let value = document
        .name_tree(root(&document))
        .get(b"k073")
        .unwrap()
        .unwrap();
    assert_eq!(document.destination(value).unwrap().page, 1);
    let loaded = document
        .slots
        .values()
        .filter(|s| s.object.get().is_some())
        .count();
    assert!(loaded < 30, "lookup eagerly loaded {loaded} objects");
    assert!(
        document
            .name_tree(root(&document))
            .get(b"absent")
            .unwrap()
            .is_none()
    );
    let document = open(
        "navigation-destinations",
        Recovery::Strict,
        Limits::default(),
    );
    for value in [
        Value::Name(Name::new(b"alias")),
        Value::String(b"alias".to_vec()),
    ] {
        assert_eq!(document.destination(&value).unwrap().page, 1);
    }
    for value in [
        Value::Name(Name::new(b"Fit")),
        Value::String(b"Fit".to_vec()),
    ] {
        assert_eq!(document.destination(&value).unwrap().page, 1);
    }
}

#[test]
fn tree_action_outline_and_result_budgets_return_no_partial_views() {
    for (name, kind) in [
        ("navigation-tree-deep", "trees"),
        ("navigation-outlines", "outlines"),
        ("navigation-links-actions", "links"),
    ] {
        let mut document = open(name, Recovery::Repair, Limits::default());
        for id in document.object_ids() {
            document.object(id).unwrap();
        }
        document.pages().unwrap();
        document.limits.depth = 1;
        assert_eq!(
            snapshot(&document, kind)
                .unwrap_err()
                .as_error()
                .unwrap()
                .kind,
            ErrorKind::Limit,
            "{name}"
        );
    }
    let document = open("navigation-tree-wide", Recovery::Repair, Limits::default());
    let before = document.retained.used();
    let _ = snapshot(&document, "trees").unwrap();
    let retained = document.retained.used();
    assert!(retained >= before);
    let _ = snapshot(&document, "trees").unwrap();
    assert_eq!(document.retained.used(), retained);
    // Limit changes are test-local; parsed source caches stay admitted once.
    let mut document = document;
    document.limits.traversal_work = 2;
    assert_eq!(
        snapshot(&document, "trees")
            .unwrap_err()
            .as_error()
            .unwrap()
            .kind,
        ErrorKind::Limit
    );
    document.limits.traversal_work = Limits::default().traversal_work;
    document.limits.document_bytes = 1;
    assert_eq!(
        snapshot(&document, "trees")
            .unwrap_err()
            .as_error()
            .unwrap()
            .kind,
        ErrorKind::Limit
    );
}

#[test]
fn actions_retain_base_files_scripts_and_next_order_as_inert_data() {
    let document = open(
        "navigation-links-actions",
        Recovery::Strict,
        Limits::default(),
    );
    let links = document.pages().unwrap()[0].links(&document).unwrap();
    let Some(Target::Actions(sequence)) = &links[3].target else {
        panic!("missing actions")
    };
    assert_eq!(
        sequence
            .actions
            .iter()
            .map(|a| a.parent)
            .collect::<Vec<_>>(),
        [None, Some(0), Some(1), Some(0)]
    );
    let ActionKind::Uri { base, is_map, .. } = &sequence.actions[0].kind else {
        panic!("missing URI")
    };
    assert_eq!(*base, Some(b"https://example.invalid/docs/".as_slice()));
    assert!(*is_map);
    assert_eq!(links[0].border.style, BorderStyle::Dashed);
    assert_eq!(links[0].border.dash, [3.0, 1.0]);
    let Some(Target::Actions(sequence)) = &links[10].target else {
        panic!("missing script action")
    };
    let ActionKind::JavaScript(Script::Stream(stream)) = sequence.actions[0].kind else {
        panic!("missing script stream")
    };
    assert_eq!(
        document.stream_data(stream).unwrap().as_ref(),
        b"also never execute"
    );
}

#[test]
fn lookup_query_does_not_borrow_for_the_tree_lifetime_and_labels_are_bounded() {
    let document = open("navigation-tree-wide", Recovery::Strict, Limits::default());
    let tree = document.name_tree(root(&document));
    {
        let query = String::from("k073");
        assert!(tree.get(query.as_bytes()).unwrap().is_some());
    }
    assert!(tree.get(b"k000").unwrap().is_some());
    let document = open(
        "navigation-link-border-array",
        Recovery::Strict,
        Limits::default(),
    );
    let links = document.pages().unwrap()[0].links(&document).unwrap();
    assert_eq!(links[0].border.horizontal_radius, 4.0);
    assert_eq!(links[0].border.vertical_radius, 5.0);
    assert_eq!(links[0].border.dash, [3.0, 1.0]);
    let mut document = open(
        "navigation-page-labels",
        Recovery::Strict,
        Limits::default(),
    );
    assert_eq!(document.page_label(0).unwrap(), "Intro");
    document.limits.string_bytes = 4;
    assert_eq!(
        document.page_label(0).unwrap_err().as_error().unwrap().kind,
        ErrorKind::Limit
    );
    assert_eq!(
        document
            .page_label(10)
            .unwrap_err()
            .as_error()
            .unwrap()
            .kind,
        ErrorKind::InvalidArgument
    );
}

#[test]
fn parsed_label_view_reuses_ranges_for_lookup_and_linear_iteration() {
    let document = open(
        "navigation-page-labels",
        Recovery::Strict,
        Limits::default(),
    );
    let labels = document.page_labels().unwrap();
    let expected =
        String::from_utf8(fixture("document", "navigation-page-labels.expected")).unwrap();
    assert_eq!(
        labels
            .iter()
            .collect::<Result<Vec<_>>>()
            .unwrap()
            .join("\n")
            + "\n",
        expected
    );
    for (index, expected) in expected.lines().enumerate() {
        assert_eq!(labels.label(index).unwrap(), expected);
    }
    assert_eq!(labels.ranges().len(), 9);
    assert_eq!(
        labels.label(10).unwrap_err().as_error().unwrap().kind,
        ErrorKind::InvalidArgument
    );
    let tree = document.number_tree(document.catalog().unwrap().get(b"PageLabels").unwrap());
    assert!(tree.get(&3).unwrap().is_some());
    assert!(tree.get(&2).unwrap().is_none());
}

#[test]
fn indirect_null_is_absence_for_optional_navigation_metadata() {
    let document = open(
        "navigation-optional-null",
        Recovery::Strict,
        Limits::default(),
    );
    assert_eq!(document.outlines().unwrap(), []);
    assert_eq!(document.named_destinations().unwrap(), []);
    assert_eq!(document.page_label(0).unwrap(), "1");
    assert_eq!(document.page_labels().unwrap().ranges(), []);
    let error = document
        .destination(&Value::Name(Name::new(b"missing")))
        .unwrap_err();
    assert_eq!(error.kind, ErrorKind::MissingObject);
    let links = document.pages().unwrap()[0].links(&document).unwrap();
    let Some(Target::Actions(sequence)) = &links[1].target else {
        panic!("missing URI")
    };
    let ActionKind::Uri { base, .. } = sequence.actions[0].kind else {
        panic!("missing URI")
    };
    assert_eq!(base, None);
    assert_eq!(sequence.actions.len(), 1);
    assert_eq!(document.diagnostics(), []);
}
