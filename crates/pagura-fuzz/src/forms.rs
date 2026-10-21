//! `forms`: the interactive form model loads, typed values fill its fields
//! in memory and the widget appearances are prepared, or each step fails
//! with a structured error, in both recovery modes (12.7). Nothing is saved,
//! and form actions stay inert data.
//!
//! Layout: a selector byte, then a PDF file. The selector's high bit selects
//! strict recovery; its two low bits choose the values filled in: a text,
//! a button and a choice value for each, listed with `TEXTS` and `value`.

use pagura::forms::{ButtonValue, ChoiceSelection, FieldKind, FillValue};
use pagura::{Document, OpenOptions};

/// How many fields are filled.
const FIELDS: usize = 16;

/// Text values, chosen by the selector: plain, multiline with CR LF (which
/// fills normalize), non-Latin, and longer than most comb or maximum
/// lengths.
const TEXTS: [&str; 4] = [
    "Pagura",
    "first\r\nsecond",
    "\u{3b1}\u{3b2}\u{3b3} \u{65e5}\u{672c}",
    "0123456789012345678901234567890123456789",
];

/// Loads the form, fills its first fields and prepares every widget's
/// appearance.
///
/// # Panics
///
/// When the input violates a checked property: that is a finding.
pub fn run(data: &[u8]) {
    let Some((&selector, pdf)) = data.split_first() else {
        return;
    };
    let options = OpenOptions {
        limits: super::limits(),
        recovery: super::recovery(selector & 0x80 != 0),
        ..OpenOptions::default()
    };
    let Some(document) = super::checked(Document::open(pdf.to_vec(), &options)) else {
        return;
    };
    if let Some(Some(form)) = super::checked(document.acro_form()) {
        let fields = form.fields.len();
        for (index, field) in form.fields.iter().enumerate() {
            assert!(
                field.parent.is_none_or(|parent| parent < index),
                "field {index} has parent {:?}",
                field.parent
            );
        }
        for widget in &form.widgets {
            assert!(widget.field < fields, "widget of field {}", widget.field);
        }
    }
    fill(&document, selector);
    super::assert_diagnostics(&document.diagnostics(), document.recovery());
}

fn fill(document: &Document, selector: u8) {
    let Some(Some(mut filler)) = super::checked(document.form_filler()) else {
        return;
    };
    for index in 0..filler.form().fields.len().min(FIELDS) {
        let Some(value) = value(&filler.form().fields[index].kind, selector) else {
            continue;
        };
        if let Some(filled) = super::checked(filler.set(index, value)) {
            assert_eq!(filled.field(), index, "filled another field");
        }
    }
    if let Some(appearances) = super::checked(pagura::prepare_form(&filler)) {
        let widgets = filler.form().widgets.len();
        assert_eq!(appearances.len(), widgets, "one appearance per widget");
        for (index, appearance) in appearances.iter().enumerate() {
            assert_eq!(appearance.widget(), index, "appearances in widget order");
        }
    }
}

/// The value the selector's two low bits choose for a field of `kind`:
/// one of [`TEXTS`]; a button cleared, set to its first export value, or to
/// its first state; a choice by index, by display text or by export value.
fn value(kind: &FieldKind<'_>, selector: u8) -> Option<FillValue<'static>> {
    let choice = usize::from(selector & 3);
    match kind {
        FieldKind::Text { .. } => Some(FillValue::Text(TEXTS[choice])),
        FieldKind::Button { .. } => Some(FillValue::Button(match choice {
            0 => ButtonValue::Off,
            1 => ButtonValue::Export("Yes"),
            _ => ButtonValue::Widget {
                index: choice - 2,
                state: None,
            },
        })),
        FieldKind::Choice { .. } => Some(FillValue::Choice(match choice {
            0 => ChoiceSelection::Indices(&[0]),
            1 => ChoiceSelection::Indices(&[0, 1]),
            2 => ChoiceSelection::Text(TEXTS[0]),
            _ => ChoiceSelection::Export("Yes"),
        })),
        _ => None,
    }
}
