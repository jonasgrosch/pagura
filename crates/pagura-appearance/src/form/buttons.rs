//! Check mark generation only for widgets without existing appearances (12.7.5.2).

use super::{bounds, inset, style::Style};
use crate::program::Program;
use crate::stream::{Appearance, AppearanceStream, form_dictionary};
use pagura_core::{Dictionary, Name, Value};
use pagura_document::source::ReadResult;
use pagura_document::{
    Document,
    forms::{Field, FieldValue, Widget},
};

/// A ZapfDingbats check mark (`4`, a8) in the padded widget box for any
/// state but Off, over the widget's background and border.
pub(super) fn check_mark(
    document: &Document,
    widget: &Widget<'_>,
    state: Option<&Name>,
    field: &Field<'_>,
) -> ReadResult<Appearance> {
    let limits = document.limits();
    let bbox = bounds(widget)?;
    let style = Style::read(document, widget.dictionary)?;
    let mut content = Program::new(&limits);
    style.paint(&mut content, bbox)?;
    let state = state.or(match &field.value {
        Some(FieldValue::Name(name)) => Some(*name),
        _ => None,
    });
    let mut resources = Dictionary::new();
    if state.is_some_and(|name| name.as_bytes() != b"Off") {
        let area = inset(bbox, style.inset)?;
        let size = area.height().min(area.width());
        resources.insert(Name::new(b"Font"), Value::Dictionary(zapf_dingbats()));
        content.text(&format!(
            "q\n{} {} {} {} re W n\n0 g\nBT\n/ZaDb {size} Tf\n1 0 0 1 {} {} Tm\n<34> Tj\nET\nQ\n",
            area.x0(),
            area.y0(),
            area.width(),
            area.height(),
            area.x0(),
            area.y0()
        ))?;
    }
    let mut dictionary = form_dictionary(bbox);
    dictionary.insert(Name::new(b"Resources"), Value::Dictionary(resources));
    AppearanceStream::new(document, dictionary, content.finish()).map(Appearance::Generated)
}

/// A font dictionary with the standard ZapfDingbats font as `/ZaDb`.
fn zapf_dingbats() -> Dictionary {
    let mut font = Dictionary::new();
    font.insert(Name::new(b"Type"), Value::name(b"Font"));
    font.insert(Name::new(b"Subtype"), Value::name(b"Type1"));
    font.insert(Name::new(b"BaseFont"), Value::name(b"ZapfDingbats"));
    let mut fonts = Dictionary::new();
    fonts.insert(Name::new(b"ZaDb"), Value::Dictionary(font));
    fonts
}
