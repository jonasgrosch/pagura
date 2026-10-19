//! Choice values use display names for V and sorted option indexes (Table 234).

use crate::forms::{ChoiceStyle, Field, FieldFlags, FieldKind};
use pagura_core::{Error, Limits, Meter, Result};

/// A choice selection. Indexes disambiguate repeated display/export strings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ChoiceSelection<'a> {
    /// Select options by index. Order is canonicalized; duplicates are rejected.
    Indices(&'a [usize]),
    /// Select one option by its display string, or enter custom editable text.
    Text(&'a str),
    /// Select one option by its export string; ambiguous matches are rejected.
    Export(&'a str),
}

/// Canonical choice replacement (12.7.5.4, Table 234).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChoiceValue {
    values: Vec<String>,
    exports: Vec<String>,
    indices: Vec<usize>,
}

impl ChoiceValue {
    /// Display names for V; one string for a single selection, an array for
    /// several. Empty means null. A custom editable value has no option index.
    pub fn values(&self) -> &[String] {
        &self.values
    }

    /// Export strings, kept separately from displayed V values.
    pub fn exports(&self) -> &[String] {
        &self.exports
    }

    /// Sorted, distinct option indexes; empty for custom editable text. A
    /// writer stores them as `I` for a multiple-selection field (Table 234).
    pub fn indices(&self) -> &[usize] {
        &self.indices
    }
}

/// Validates `input`, charging the value it builds to `budget`. String
/// comparisons cost no more than the parsed options themselves.
pub(super) fn validate(
    field: &Field<'_>,
    input: ChoiceSelection<'_>,
    limits: &Limits,
    budget: &mut Meter,
) -> Result<ChoiceValue> {
    let FieldKind::Choice {
        style,
        multi_select,
        options,
    } = &field.kind
    else {
        return Err(Error::invalid_argument("field is not a choice"));
    };
    // Edit needs Combo, and DoNotSpellCheck needs both (Table 233).
    let flags = field.flags;
    if (flags.contains(FieldFlags::EDIT) && !flags.contains(FieldFlags::COMBO))
        || (flags.contains(FieldFlags::DO_NOT_SPELL_CHECK)
            && *style != (ChoiceStyle::Combo { editable: true }))
    {
        return Err(Error::syntax(
            "choice Edit and DoNotSpellCheck require an editable combo",
        ));
    }
    let indices = match input {
        ChoiceSelection::Indices(indices) => {
            if indices.len() > 1 && !multi_select {
                return Err(Error::invalid_argument(
                    "field does not permit multiple selections",
                ));
            }
            budget.charge_items::<usize>(indices.len())?;
            let mut result = indices.to_vec();
            result.sort_unstable();
            if result.windows(2).any(|pair| pair[0] == pair[1]) {
                return Err(Error::invalid_argument("choice indexes must be distinct"));
            }
            result
        }
        ChoiceSelection::Text(text) | ChoiceSelection::Export(text) => {
            if text.len() > limits.string_bytes {
                return Err(Error::limit("choice text input budget exceeded"));
            }
            let export = matches!(input, ChoiceSelection::Export(_));
            let mut matches = options.iter().enumerate().filter(|(_, option)| {
                if export {
                    option.export == text
                } else {
                    option.display == text
                }
            });
            let first = matches.next().map(|(index, _)| index);
            if matches.next().is_some() {
                return Err(Error::invalid_argument(
                    "ambiguous choice string; select an option index",
                ));
            }
            match first {
                Some(index) => {
                    budget.charge_items::<usize>(1)?;
                    vec![index]
                }
                None if *style == (ChoiceStyle::Combo { editable: true }) && !export => {
                    if text.contains(['\r', '\n']) {
                        return Err(Error::invalid_argument("combo text must be a single line"));
                    }
                    budget.charge_items::<String>(2)?;
                    budget.charge_product(text.len(), 2)?;
                    return Ok(ChoiceValue {
                        values: vec![text.into()],
                        exports: vec![text.into()],
                        indices: Vec::new(),
                    });
                }
                None => return Err(Error::invalid_argument("value is not a choice option")),
            }
        }
    };
    let mut selected = Vec::new();
    for &index in &indices {
        let option = options
            .get(index)
            .ok_or(Error::invalid_argument("choice index is outside Opt"))?;
        budget.charge_items::<String>(2)?;
        budget.charge(option.display.len().saturating_add(option.export.len()))?;
        selected.push(option);
    }
    Ok(ChoiceValue {
        values: selected
            .iter()
            .map(|option| option.display.clone())
            .collect(),
        exports: selected
            .iter()
            .map(|option| option.export.clone())
            .collect(),
        indices,
    })
}
