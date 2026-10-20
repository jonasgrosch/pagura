//! Native editor metrics from stored appearance glyphs, with declared DA fallback.
use super::{Encoder, aliases, index, overlay_visible};
use pagura::forms::{FieldKind, Justification};
use pagura::{Document, Name, Result, Value};

pub(crate) fn encode(document: &Document, page: u32) -> Result<Vec<u8>> {
    let page = document.page(index(page)?)?;
    let mut out = Encoder::new(document.limits().document_bytes.min(32 * 1024 * 1024));
    let Some(form) = document.acro_form()? else {
        out.count(0)?;
        return Ok(out.finish());
    };
    let mut matcher = aliases::Matcher::new(document, &form)?;
    let mut selected = std::collections::BTreeSet::new();
    for (_, index) in matcher.page(document, page)? {
        if overlay_visible(document, &form.widgets[index])?
            && matches!(
                form.fields[form.widgets[index].field].kind,
                FieldKind::Text { .. } | FieldKind::Choice { .. }
            )
        {
            selected.insert(index);
        }
    }
    out.count(selected.len())?;
    let mut remaining = document.limits().document_bytes.min(32 * 1024 * 1024);
    for index in selected {
        let widget = &form.widgets[index];
        let field = &form.fields[widget.field];
        let (mut font, mut size) = field
            .default_appearance
            .and_then(|bytes| {
                pagura::form_appearance::default_text_style(bytes, document.limits()).ok()
            })
            .unwrap_or((Name::new(b"Helvetica"), 12.0));
        if size <= 0.0 {
            size = 12.0;
        }
        let (mut left, mut baseline) = (2.0, (widget.rectangle.height() + size * 0.6) / 2.0);
        if matches!(
            field.kind,
            FieldKind::Text {
                multiline: true,
                ..
            }
        ) {
            baseline = 2.0 + size * 0.8;
        }
        let appearance = widget
            .dictionary
            .get(b"AP")
            .and_then(|value| document.resolve_dictionary(value).ok())
            .and_then(|dictionary| dictionary.get(b"N"))
            .and_then(Value::as_reference);
        let appearance = appearance.filter(|id| {
            if remaining == 0 {
                return false;
            }
            let size = document
                .object(*id)
                .and_then(|object| document.stream_data(object))
                .map(|bytes| bytes.len());
            match size {
                Ok(size) if size <= remaining => {
                    remaining -= size;
                    true
                }
                _ => {
                    remaining = 0;
                    false
                }
            }
        });
        if let Some(id) = appearance
            && let Ok(text) = pagura::appearance_text(document, page, id, widget.rectangle)
            && let Some(glyph) = text.glyphs.first()
            && glyph.em.is_finite()
            && glyph.em > 0.0
            && glyph.axis.x > 0.0
            && glyph.axis.y.abs() < 1e-6
        {
            size = glyph.em;
            left = glyph.origin.x - widget.rectangle.x0();
            baseline = widget.rectangle.y1() - glyph.origin.y;
            if let Some(name) = text
                .fonts
                .get(glyph.font)
                .and_then(|font| font.base_name.as_ref())
            {
                font = name.clone();
            }
        }
        out.count(index)?;
        out.string(&font.to_string())?;
        for number in [size, left, baseline] {
            out.push(&number.to_le_bytes())?;
        }
        out.push(&[match field.justification {
            Justification::Left => 0,
            Justification::Center => 1,
            Justification::Right => 2,
        }])?;
    }
    Ok(out.finish())
}
