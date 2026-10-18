//! Check mark generation only for widgets without existing appearances (12.7.5.2).
use super::{Appearance, Program, bounds, finish, form_dictionary, inset, style::Style};
use pagura_core::{Dictionary, Name, Result, Value};
use pagura_document::{
    Document,
    forms::{Field, FieldValue, Widget},
};
pub(super) fn check_mark(
    document: &Document,
    widget: &Widget<'_>,
    state: Option<&Name>,
    field: &Field<'_>,
) -> Result<Appearance> {
    let bbox = bounds(widget)?;
    let style = Style::read(document, widget.dictionary)?;
    let mut content = Program::new(document.limits());
    style.paint(&mut content, bbox)?;
    let state = state.or(match &field.value {
        Some(FieldValue::Name(n)) => Some(*n),
        _ => None,
    });
    let mut resources = Dictionary::new();
    if state.is_some_and(|n| n.as_bytes() != b"Off") {
        let area = inset(bbox, style.inset)?;
        let size = area.height().min(area.width());
        let mut font = Dictionary::new();
        font.insert(Name::new(b"Type"), Value::name(b"Font"));
        font.insert(Name::new(b"Subtype"), Value::name(b"Type1"));
        font.insert(Name::new(b"BaseFont"), Value::name(b"ZapfDingbats"));
        let mut fonts = Dictionary::new();
        fonts.insert(Name::new(b"ZaDb"), Value::Dictionary(font));
        resources.insert(Name::new(b"Font"), Value::Dictionary(fonts));
        content.text(&format!(
            "q\n{} {} {} {} re W n\n0 g\nBT\n/ZaDb {size} Tf\n1 0 0 1 {} {} Tm\n<34> Tj\nET\nQ\n",
            area.x0,
            area.y0,
            area.width(),
            area.height(),
            area.x0,
            area.y0
        ))?;
    }
    let mut dictionary = form_dictionary(bbox);
    dictionary.insert(Name::new(b"Resources"), Value::Dictionary(resources));
    finish(document, dictionary, content.finish())
}
