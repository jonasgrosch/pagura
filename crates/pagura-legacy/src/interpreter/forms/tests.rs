use super::*;
use pagura_core::{ErrorKind, Name, Position};
use pagura_document::{Document, OpenOptions};
use pagura_testkit::fixture;

#[test]
fn source_and_generated_bodies_share_groups_positions_and_mark_accounting() {
    let document = Document::open(
        fixture("forms", "render-text-transparency-group.pdf"),
        &OpenOptions::default(),
    )
    .unwrap();
    let page = document.page(0).unwrap();
    let object = document.object(ObjectId::new(80, 0)).unwrap();
    let bytes = document.stream_data(object).unwrap();
    let mut stored = Compiler::new(&document, page).unwrap();
    stored.position = Position::Buffer(19);
    stored
        .form_body(FormBody::Stored(object), page.resources())
        .unwrap();
    let mut generated = Compiler::new(&document, page).unwrap();
    generated.position = stored.position;
    generated
        .form_body(
            FormBody::Generated {
                dictionary: object.dictionary().unwrap(),
                bytes: &bytes,
            },
            page.resources(),
        )
        .unwrap();
    assert_eq!(stored.marks, generated.marks);
    assert_eq!(stored.operations, generated.operations);
    assert_eq!(stored.list.commands.len(), generated.list.commands.len());
    for list in [&stored.list, &generated.list] {
        let [crate::display::Command::Group(group)] = list.commands.as_slice() else {
            panic!("expected form group");
        };
        assert_eq!(group.position, Position::Buffer(19));
    }
    let options = crate::RenderOptions::default();
    assert_eq!(
        stored.list.render(&options).unwrap().data(),
        generated.list.render(&options).unwrap().data()
    );
}

#[test]
fn both_form_inputs_reject_an_overfull_parent_display_list() {
    let document = Document::open(
        fixture("forms", "render-radio.pdf"),
        &OpenOptions {
            limits: pagura_core::Limits {
                display_commands: 2,
                ..Default::default()
            },
            ..Default::default()
        },
    )
    .unwrap();
    let page = document.page(0).unwrap();
    let object = document.object(ObjectId::new(82, 0)).unwrap();
    let bytes = document.stream_data(object).unwrap();
    for body in [
        FormBody::Stored(object),
        FormBody::Generated {
            dictionary: object.dictionary().unwrap(),
            bytes: &bytes,
        },
    ] {
        let mut compiler = Compiler::new(&document, page).unwrap();
        // Empty annotation view groups can occupy commands without painting marks.
        for _ in 0..2 {
            compiler.list.commands.push(crate::display::Command::Group(
                crate::transparency::Group {
                    position: Position::Unknown,
                    bounds: None,
                    commands: Vec::new(),
                    attributes: crate::transparency::Attributes::default(),
                    composite: crate::transparency::Composite::opaque(),
                },
            ));
        }
        let error = compiler.form_body(body, page.resources()).unwrap_err();
        assert_eq!(error.kind, ErrorKind::Limit);
        assert_eq!(error.message, "display list budget exceeded");
    }
}

#[test]
fn generated_input_uses_the_form_subtype_error_contract() {
    let document =
        Document::open(fixture("forms", "render-text.pdf"), &OpenOptions::default()).unwrap();
    let page = document.page(0).unwrap();
    let dictionary: Dictionary = [(Name::new(b"Subtype"), Value::name(b"Unknown"))]
        .into_iter()
        .collect();
    let mut compiler = Compiler::new(&document, page).unwrap();
    let error = compiler
        .form_body(
            FormBody::Generated {
                dictionary: &dictionary,
                bytes: b"",
            },
            page.resources(),
        )
        .unwrap_err();
    assert_eq!(
        error.kind,
        ErrorKind::Unsupported(features::XOBJECT_SUBTYPE)
    );
}
