use super::*;
use crate::display::DisplayList;
use crate::interpreter::session::Session;
use crate::interpreter::text::cache::FontCache;
use pagura_core::{ErrorKind, Name, Position};
use pagura_document::{Document, OpenOptions, Page};
use pagura_testkit::fixture;
use std::sync::Mutex;

fn session<'a>(document: &'a Document, page: &'a Page, fonts: &'a Mutex<FontCache>) -> Session<'a> {
    Session::new(document, page, None, fonts, None, None).unwrap()
}

fn list(page: &Page, document: &Document, commands: Vec<crate::display::Command>) -> DisplayList {
    let mut list = DisplayList::new(page, document.limits());
    list.commands = commands;
    list
}

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
    let fonts = Mutex::new(FontCache::new(document.limits()));
    let stored_session = session(&document, page, &fonts);
    let generated_session = session(&document, page, &fonts);
    let mut stored = Compiler::new(&stored_session);
    stored.position = Position::Buffer(19);
    stored
        .form_body(FormBody::Stored(object), page.resources())
        .unwrap();
    let mut generated = Compiler::new(&generated_session);
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
    assert_eq!(stored_session.marks(), generated_session.marks());
    assert_eq!(stored_session.operations(), generated_session.operations());
    assert_eq!(stored.commands.len(), generated.commands.len());
    for commands in [&stored.commands, &generated.commands] {
        let [crate::display::Command::Group(group)] = commands.as_slice() else {
            panic!("expected form group");
        };
        assert_eq!(group.position, Position::Buffer(19));
    }
    let options = crate::RenderOptions::default();
    assert_eq!(
        list(page, &document, stored.commands)
            .render(&options)
            .unwrap()
            .data(),
        list(page, &document, generated.commands)
            .render(&options)
            .unwrap()
            .data()
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
        let fonts = Mutex::new(FontCache::new(document.limits()));
        let session = session(&document, page, &fonts);
        let mut compiler = Compiler::new(&session);
        // Empty annotation view groups can occupy commands without painting marks.
        for _ in 0..2 {
            compiler
                .commands
                .push(crate::display::Command::Group(crate::transparency::Group {
                    position: Position::Unknown,
                    bounds: None,
                    commands: Vec::new(),
                    attributes: crate::transparency::Attributes::default(),
                    composite: crate::transparency::Composite::default(),
                }));
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
    let fonts = Mutex::new(FontCache::new(document.limits()));
    let session = session(&document, page, &fonts);
    let mut compiler = Compiler::new(&session);
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
