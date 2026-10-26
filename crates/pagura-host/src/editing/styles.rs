//! Native editor metrics from stored appearance glyphs, with declared DA fallback.
use super::{MAX_BYTES, Writer, aliases, overlay_visible};
use pagura::ReadResult;
use pagura::forms::{FieldKind, Justification};
use pagura::{Document, ErrorKind, Name, Page, ReadFault, Value};

pub(crate) fn encode(document: &Document, page: &Page) -> ReadResult<Vec<u8>> {
    let mut out = Writer::new(document.limits().document_bytes.min(MAX_BYTES));
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
    let mut remaining = document.limits().document_bytes.min(MAX_BYTES);
    for index in selected {
        let widget = &form.widgets[index];
        let field = &form.fields[widget.field];
        let (mut font, mut size) = field
            .default_appearance
            .and_then(|bytes| {
                pagura::form_appearance::default_text_style(bytes, &document.limits()).ok()
            })
            .map_or((Name::new(b"Helvetica"), 12.0), |style| {
                (style.font().clone(), style.size())
            });
        if size <= 0.0 {
            size = 12.0;
        }
        let (mut left, mut baseline) = (2.0, f64::midpoint(widget.rectangle.height(), size * 0.6));
        if matches!(
            field.kind,
            FieldKind::Text {
                multiline: true,
                ..
            }
        ) {
            baseline = 2.0 + size * 0.8;
        }
        if let Some(text) = stored_text(document, page, widget, &mut remaining)?
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
        out.text(&font.to_string())?;
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

/// Appearance metrics are advisory. Malformed/unsupported appearance data keeps
/// the declared DA fallback, while unavailable data and terminal source/budget
/// failures cannot become successful substitute metrics.
fn stored_text(
    document: &Document,
    page: &Page,
    widget: &pagura::forms::Widget<'_>,
    remaining: &mut usize,
) -> ReadResult<Option<pagura::text::PositionedText>> {
    let result = (|| -> ReadResult<_> {
        let appearance = widget
            .dictionary
            .get(b"AP")
            .map(|value| document.resolve_dictionary(value))
            .transpose()?
            .and_then(|dictionary| dictionary.get(b"N"))
            .and_then(Value::as_reference);
        let Some(id) = appearance.filter(|_| *remaining > 0) else {
            return Ok(None);
        };
        let size = document.stream_data(document.object(id)?)?.len();
        if size > *remaining {
            *remaining = 0;
            return Ok(None);
        }
        *remaining -= size;
        pagura::appearance_text(document, page, id, widget.rectangle).map(Some)
    })();
    match result {
        Err(ReadFault::Invalid(error))
            if matches!(
                error.kind,
                ErrorKind::Syntax
                    | ErrorKind::MissingObject
                    | ErrorKind::Cycle
                    | ErrorKind::Unsupported(_)
            ) =>
        {
            Ok(None)
        }
        result => result,
    }
}
