//! Persist validated form values and normal appearance programs together.

use super::{SaveOptions, Transaction, features};
use crate::form_appearance::{self, Appearance};
use crate::forms::{ButtonKind, Field, FieldFlags, FieldKind, FieldValue, FilledValue, FormFiller};
use crate::{Dictionary, Document, Error, Name, NewObject, ObjectId, Result, Value};
use pagura_core::text_string;

/// Saves a filled form as one incremental revision, leaving its source intact.
///
/// Text and choice fields receive explicit normal appearances. Button widgets
/// preserve original state dictionaries or generate both Off and on states.
/// Rollover/down appearances, default values and unrelated entries survive.
/// NeedAppearances is cleared only after all requested appearances are prepared.
/// No calculation, validation or other action is executed. Password changes are
/// refused, never silently omitted. An empty unchanged form saves byte-for-byte.
///
/// # Errors
/// Returns transaction/source-policy errors and appearance-generation errors.
/// Nested direct fields/widgets and password changes currently return explicit
/// `Unsupported` errors.
pub fn save_filled_form(filler: &FormFiller<'_>, options: &SaveOptions) -> Result<Vec<u8>> {
    let mut transaction = Transaction::new(filler.document());
    if filler.changes().next().is_none() && !filler.form().need_appearances {
        return transaction.commit(options);
    }
    for change in filler.changes() {
        let field = filler
            .form()
            .fields
            .get(change.field())
            .ok_or(Error::invalid_argument("filled field index is absent"))?;
        refuse_password(field)?;
        let id = identity(filler.document(), field.object, field.dictionary)?;
        let value = field_value(change.value(), filler.document())?;
        transaction.edit_dictionary(id, |dictionary| {
            dictionary.insert(Name::new(b"V"), value);
            if let FilledValue::Choice(choice) = change.value() {
                if field.flags.contains(FieldFlags::MULTI_SELECT) {
                    let indices = choice
                        .indices()
                        .iter()
                        .map(|&index| {
                            i64::try_from(index)
                                .map(Value::Integer)
                                .map_err(|_| Error::limit("choice index exceeds integer range"))
                        })
                        .collect::<Result<Vec<_>>>()?;
                    dictionary.insert(Name::new(b"I"), Value::Array(indices));
                } else {
                    dictionary.remove(b"I");
                }
            }
            Ok(())
        })?;
        for state in change.widget_states() {
            let widget = filler
                .form()
                .widgets
                .get(state.widget())
                .ok_or(Error::invalid_argument("filled widget index is absent"))?;
            let id = identity(filler.document(), widget.object, widget.dictionary)?;
            transaction.edit_dictionary(id, |dictionary| {
                dictionary.insert(Name::new(b"AS"), Value::Name(state.state().clone()));
                Ok(())
            })?;
        }
    }
    prepare_appearances(filler, &mut transaction, options.form_appearances)?;
    if filler.form().need_appearances {
        clear_need_appearances(filler, &mut transaction)?;
    }
    transaction.commit(options)
}

fn prepare_appearances(
    filler: &FormFiller<'_>,
    transaction: &mut Transaction<'_>,
    policy: form_appearance::UpdatePolicy,
) -> Result<()> {
    #[cfg(feature = "bundled-data")]
    let provider = pagura_data::Bundled::new(filler.document().limits());
    #[cfg(feature = "bundled-data")]
    let provider = Some(&provider as &dyn crate::Provider);
    #[cfg(not(feature = "bundled-data"))]
    let provider = None;
    for (index, widget) in filler.form().widgets.iter().enumerate() {
        if filler.get(widget.field).is_none() && !filler.form().need_appearances {
            continue;
        }
        let field = filler
            .form()
            .fields
            .get(widget.field)
            .ok_or(Error::invalid_argument("widget field index is absent"))?;
        refuse_password(field)?;
        let appearance =
            form_appearance::prepare_widget_with_policy(filler, index, provider, policy)?;
        if let Appearance::Generated(stream) = appearance {
            let id = identity(filler.document(), widget.object, widget.dictionary)?;
            let generated_button = matches!(field.kind, FieldKind::Button { .. });
            let normal = normal_appearance(filler, transaction, index, field, stream)?;
            let mut appearances = match widget.dictionary.get(b"AP") {
                Some(value) => match filler.document().resolve(value)? {
                    Value::Null => Dictionary::new(),
                    Value::Dictionary(dictionary) => dictionary.clone(),
                    _ => return Err(Error::syntax("widget AP must be a dictionary")),
                },
                None => Dictionary::new(),
            };
            appearances.insert(Name::new(b"N"), normal);
            transaction.edit_dictionary(id, |dictionary| {
                dictionary.insert(Name::new(b"AP"), Value::Dictionary(appearances));
                if generated_button && filler.get(widget.field).is_none() {
                    // NeedAppearances may generate an untouched button. Make
                    // its preview selection explicit for the new state dictionary.
                    let state = widget.appearance_state.or(match &field.value {
                        Some(FieldValue::Name(name)) => Some(*name),
                        _ => None,
                    });
                    dictionary.insert(
                        Name::new(b"AS"),
                        Value::Name(state.cloned().unwrap_or_else(|| Name::new(b"Off"))),
                    );
                }
                Ok(())
            })?;
        }
    }
    Ok(())
}

fn normal_appearance(
    filler: &FormFiller<'_>,
    transaction: &mut Transaction<'_>,
    index: usize,
    field: &Field<'_>,
    stream: form_appearance::AppearanceStream,
) -> Result<Value> {
    if matches!(
        field.kind,
        FieldKind::Button {
            kind: ButtonKind::Push,
            ..
        }
    ) {
        return Err(Error::unsupported(
            features::BUTTON_APPEARANCE,
            "saving push buttons requires an existing appearance",
        ));
    }
    if matches!(field.kind, FieldKind::Button { .. }) {
        // Drop the preview state before reserving both persistent states.
        drop(stream);
        let mut states = Dictionary::new();
        for (name, stream) in form_appearance::prepare_button_states(filler, index)? {
            let appearance_id = add_appearance(transaction, &stream)?;
            states.insert(name, Value::Reference(appearance_id));
        }
        Ok(Value::Dictionary(states))
    } else {
        Ok(Value::Reference(add_appearance(transaction, &stream)?))
    }
}

fn add_appearance(
    transaction: &mut Transaction<'_>,
    stream: &form_appearance::AppearanceStream,
) -> Result<ObjectId> {
    transaction.add(NewObject::Stream {
        dictionary: stream.dictionary().clone(),
        data: stream.content().to_vec(),
    })
}

fn field_value(value: &FilledValue, document: &Document) -> Result<Value> {
    match value {
        FilledValue::Text(text) => text_value(text, document),
        FilledValue::Button(name) => Ok(Value::Name(name.clone())),
        FilledValue::Choice(choice) => match choice.values() {
            [] => Ok(Value::Null),
            [text] => text_value(text, document),
            values => values
                .iter()
                .map(|text| text_value(text, document))
                .collect::<Result<Vec<_>>>()
                .map(Value::Array),
        },
        _ => Err(Error::invalid_argument("unsupported filled value kind")),
    }
}

fn text_value(text: &str, document: &Document) -> Result<Value> {
    // UTF-16 is the longest encoding this encoder can choose. Check before
    // allocating, including the BOM; avoid accepting an over-budget expansion.
    let bytes = text
        .encode_utf16()
        .count()
        .checked_mul(2)
        .and_then(|size| size.checked_add(2))
        .ok_or(Error::limit("written text size overflow"))?;
    if bytes > document.limits().document_bytes {
        return Err(Error::limit("written text storage budget exceeded"));
    }
    let encoded = text_string::encode(text);
    if encoded.len() > document.limits().string_bytes {
        return Err(Error::limit("written text string budget exceeded"));
    }
    Ok(Value::String(encoded))
}

fn refuse_password(field: &Field<'_>) -> Result<()> {
    if matches!(field.kind, FieldKind::Text { password: true, .. }) {
        return Err(
            Error::unsupported(features::PASSWORD, "password fields cannot be persisted")
                .at(field.position)
                .in_object_opt(field.object),
        );
    }
    Ok(())
}

fn identity(
    document: &Document,
    id: Option<ObjectId>,
    dictionary: &Dictionary,
) -> Result<ObjectId> {
    let id = id.ok_or(Error::unsupported(
        features::DIRECT_FIELD,
        "saving requires an indirect field or widget",
    ))?;
    let object = document.object(id)?;
    if object.is_stream()
        || !object
            .dictionary()
            .is_some_and(|source| std::ptr::eq(source, dictionary))
    {
        return Err(Error::unsupported(
            features::DIRECT_FIELD,
            "nested direct field or widget cannot be replaced by its containing object",
        )
        .in_object(id));
    }
    Ok(id)
}

fn clear_need_appearances(
    filler: &FormFiller<'_>,
    transaction: &mut Transaction<'_>,
) -> Result<()> {
    let document = filler.document();
    let root = document
        .trailer()
        .get(b"Root")
        .and_then(Value::as_reference)
        .ok_or(Error::syntax("Root must be indirect"))?;
    let catalog = document.catalog()?;
    let value = catalog
        .get(b"AcroForm")
        .ok_or(Error::syntax("AcroForm is absent"))?;
    if let Some(id) = value.as_reference() {
        let id = identity(document, Some(id), filler.form().dictionary)?;
        transaction.edit_dictionary(id, |dictionary| {
            dictionary.insert(Name::new(b"NeedAppearances"), Value::Boolean(false));
            Ok(())
        })
    } else {
        // The direct form dictionary is owned by the catalog, unlike a field
        // buried in an arbitrary array: its exact update path is known here.
        let mut form = filler.form().dictionary.clone();
        form.insert(Name::new(b"NeedAppearances"), Value::Boolean(false));
        transaction.edit_dictionary(root, |dictionary| {
            dictionary.insert(Name::new(b"AcroForm"), Value::Dictionary(form));
            Ok(())
        })
    }
}
