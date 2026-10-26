//! Choice-field display values, rows and selection highlights (12.7.5.4).

use super::{
    da::DefaultAppearance,
    font::Font,
    layout::{self, TextLayout},
};
use crate::program::Program;
use crate::values::optional;
use pagura_core::{Error, Rect};
use pagura_document::source::ReadResult;
use pagura_document::{
    Document,
    forms::{ChoiceOption, Field, FieldKind, FieldValue, FilledField, FilledValue, FormFiller},
};

/// The field's selected display values: the in-memory choice, else V.
pub(super) fn values<'a>(
    filler: &'a FormFiller<'_>,
    field: &'a Field<'_>,
    index: usize,
) -> Vec<&'a str> {
    match filler.get(index).map(FilledField::value) {
        Some(FilledValue::Choice(value)) => value.values().iter().map(String::as_str).collect(),
        _ => match &field.value {
            Some(FieldValue::Text(s)) => vec![s],
            Some(FieldValue::Texts(v)) => v.iter().map(String::as_str).collect(),
            _ => Vec::new(),
        },
    }
}

/// One row per option from the top index TI down to the box's bottom, with
/// selected rows highlighted.
pub(super) fn list(
    out: &mut Program,
    filler: &FormFiller<'_>,
    index: usize,
    area: Rect,
    font: &Font,
    da: &DefaultAppearance,
    values: &[&str],
) -> ReadResult<()> {
    let document = filler.document();
    let field = filler
        .form()
        .fields
        .get(index)
        .ok_or(Error::invalid_argument("filled field index is absent"))?;
    let FieldKind::Choice {
        options,
        multi_select,
        ..
    } = &field.kind
    else {
        return Err(Error::syntax("list requires choice options").into());
    };
    if !multi_select && values.len() > 1 {
        return Err(Error::syntax("single-select choice has several values").into());
    }
    let size = if da.size == 0.0 { 12.0 } else { da.size };
    let height = if da.leading == 0.0 {
        size * 1.2
    } else {
        da.leading
    } * da.matrix[3];
    if !height.is_finite() || height <= 0.0 {
        return Err(Error::syntax("list row height must be positive").into());
    }
    let top = top_index(document, field)?;
    if top > options.len() {
        return Err(Error::syntax("choice TI is outside Opt").into());
    }
    let selected =
        if let Some(FilledValue::Choice(value)) = filler.get(index).map(FilledField::value) {
            value.indices().to_vec()
        } else {
            source_indices(document, field, options, values)?
        };
    let row_da = DefaultAppearance { size, ..da.clone() };
    for (row, (i, option)) in options.iter().enumerate().skip(top).enumerate() {
        let y1 = area.y1() - row as f64 * height;
        if y1 <= area.y0() {
            break;
        }
        let row_area = Rect::from_corners(area.x0(), y1 - height, area.x1(), y1)
            .ok_or(Error::syntax("list row height must be positive"))?;
        if selected.binary_search(&i).is_ok() {
            out.text(&format!(
                "q\n0.6 0.75 0.9 rg\n{} {} {} {} re f\nQ\n",
                row_area.x0(),
                row_area.y0(),
                row_area.width(),
                row_area.height()
            ))?;
        }
        out.text("BT\n")?;
        layout::write(
            out,
            font,
            &row_da,
            row_area,
            &TextLayout {
                text: &option.display,
                multiline: false,
                comb: None,
                password: false,
                quadding: field.justification,
                no_scroll: false,
            },
            &document.limits(),
        )?;
        out.text("ET\n")?;
    }
    Ok(())
}

/// TI, the first visible option (Table 234).
fn top_index(document: &Document, field: &Field<'_>) -> ReadResult<usize> {
    Ok(optional(document, field.dictionary, b"TI")?
        .map(|value| -> ReadResult<usize> {
            document
                .resolve(value)?
                .as_integer()
                .and_then(|n| usize::try_from(n).ok())
                .ok_or_else(|| Error::syntax("choice TI must be a nonnegative integer").into())
        })
        .transpose()?
        .unwrap_or(0))
}

/// The selected option indices: I when it agrees with V, else the options
/// whose display text V names, since V is authoritative (Table 234).
fn source_indices(
    document: &Document,
    field: &Field<'_>,
    options: &[ChoiceOption],
    values: &[&str],
) -> ReadResult<Vec<usize>> {
    if let Some(indices) = optional(document, field.dictionary, b"I")? {
        let indices = document
            .resolve(indices)?
            .as_array()
            .ok_or(Error::syntax("choice I must be an array"))?;
        let mut result: Vec<usize> = Vec::new();
        for value in indices {
            let i = document
                .resolve(value)?
                .as_integer()
                .and_then(|n| usize::try_from(n).ok())
                .filter(|i| *i < options.len())
                .ok_or(Error::syntax("choice I index is outside Opt"))?;
            if result.last().is_some_and(|previous| *previous >= i) {
                return Err(Error::syntax("choice I must be sorted and distinct").into());
            }
            result.push(i);
        }
        if result.len() == values.len()
            && result.iter().zip(values).all(|(i, value)| {
                options
                    .get(*i)
                    .is_some_and(|option| option.display == *value)
            })
        {
            return Ok(result);
        }
    }
    if options.len().saturating_mul(
        values
            .iter()
            .fold(0usize, |n, v| n.saturating_add(v.len().max(1))),
    ) > document.limits().operations
    {
        return Err(Error::limit("choice appearance lookup budget exceeded").into());
    }
    Ok(options
        .iter()
        .enumerate()
        .filter(|(_, option)| values.contains(&option.display.as_str()))
        .map(|(i, _)| i)
        .collect())
}
