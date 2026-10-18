//! Choice values use display names for V, and sorted indexes for I (Table 234).
use super::super::{ChoiceOption, Field, FieldKind};
use pagura_core::{Error, Limits, Result};

/// A choice selection. Indexes disambiguate repeated display/export strings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
    /// Sorted, distinct option indexes for I; empty for custom editable text.
    pub fn indices(&self) -> &[usize] {
        &self.indices
    }
    pub(super) fn bytes(&self) -> usize {
        self.values.iter().chain(&self.exports).fold(
            self.values
                .capacity()
                .saturating_add(self.exports.capacity())
                .saturating_mul(size_of::<String>())
                .saturating_add(self.indices.capacity().saturating_mul(size_of::<usize>())),
            |bytes, s| bytes.saturating_add(s.capacity()),
        )
    }
}
pub(super) fn validate(
    field: &Field<'_>,
    input: ChoiceSelection<'_>,
    limits: &Limits,
) -> Result<ChoiceValue> {
    let FieldKind::Choice {
        combo,
        editable,
        multi_select,
        options,
    } = &field.kind
    else {
        return Err(Error::invalid_argument("field is not a choice"));
    };
    if *editable && !combo || field.flags.contains(23) && !(*combo && *editable) {
        return Err(Error::invalid_argument(
            "choice Edit and DoNotSpellCheck require an editable combo",
        ));
    }
    let mut indices = match input {
        ChoiceSelection::Indices(indices) => {
            if indices.len() > limits.value_nodes
                || indices.len() > limits.operations
                || indices.len().saturating_mul(size_of::<usize>()) > limits.document_bytes
            {
                return Err(Error::limit("choice selection budget exceeded"));
            }
            if indices.len() > 1 && !multi_select {
                return Err(Error::invalid_argument(
                    "field does not permit multiple selections",
                ));
            }
            let mut result = indices.to_vec();
            result.sort_unstable();
            if result.windows(2).any(|w| w[0] == w[1]) {
                return Err(Error::invalid_argument("choice indexes must be distinct"));
            }
            result
        }
        ChoiceSelection::Text(text) | ChoiceSelection::Export(text) => {
            if text.len() > limits.string_bytes || text.len() > limits.operations {
                return Err(Error::limit("choice text input budget exceeded"));
            }
            // Charge comparisons, including repeated references to long strings.
            if options.len().saturating_mul(text.len().max(1)) > limits.operations {
                return Err(Error::limit("choice lookup work budget exceeded"));
            }
            let export = matches!(input, ChoiceSelection::Export(_));
            let mut matches = options.iter().enumerate().filter(|(_, o)| {
                if export {
                    o.export == text
                } else {
                    o.display == text
                }
            });
            let first = matches.next().map(|(i, _)| i);
            if matches.next().is_some() {
                return Err(Error::invalid_argument(
                    "ambiguous choice string; select an option index",
                ));
            }
            if let Some(index) = first {
                vec![index]
            } else if *combo && *editable && !export {
                if text.contains(['\r', '\n']) {
                    return Err(Error::invalid_argument("combo text must be a single line"));
                }
                if text
                    .len()
                    .saturating_mul(2)
                    .saturating_add(2 * size_of::<String>())
                    > limits.document_bytes
                {
                    return Err(Error::limit("choice value storage budget exceeded"));
                }
                return Ok(ChoiceValue {
                    values: vec![text.into()],
                    exports: vec![text.into()],
                    indices: Vec::new(),
                });
            } else {
                return Err(Error::invalid_argument("value is not a choice option"));
            }
        }
    };
    let mut bytes = indices
        .len()
        .saturating_mul(2 * size_of::<String>() + size_of::<usize>());
    for &i in &indices {
        let option = options
            .get(i)
            .ok_or(Error::invalid_argument("choice index is outside Opt"))?;
        bytes = bytes
            .saturating_add(option.display.len())
            .saturating_add(option.export.len());
        if bytes > limits.document_bytes {
            return Err(Error::limit("choice value storage budget exceeded"));
        }
    }
    let selected =
        |f: fn(&ChoiceOption) -> &String| indices.iter().map(|i| f(&options[*i]).clone()).collect();
    Ok(ChoiceValue {
        values: selected(|o| &o.display),
        exports: selected(|o| &o.export),
        indices: std::mem::take(&mut indices),
    })
}
