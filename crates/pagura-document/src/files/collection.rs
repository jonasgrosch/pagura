//! Portable collection declarations, without executing navigators (7.11.6, 12.3.5).

use super::read;
use crate::Document;
use crate::source::ReadResult;
use crate::view::{Context, Source};
use pagura_core::{Dictionary, Error, Name, ObjectId, Value};

/// Initial collection presentation, as declared by the producer (Table 153).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum CollectionView {
    /// Details view; also the default.
    #[default]
    Details,
    /// Tile view.
    Tiles,
    /// Initially hidden; no embedded document is opened automatically.
    Hidden,
    /// A navigator is declared; its code is never executed.
    Navigator,
}

/// A schema field's data source (Table 155).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum CollectionFieldKind {
    /// Collection item text.
    Text,
    /// Collection item date, retained as a source string.
    Date,
    /// Collection item number.
    Number,
    /// File UF/F name.
    FileName,
    /// File description.
    Description,
    /// Payload modification date.
    ModificationDate,
    /// Payload creation date.
    CreationDate,
    /// Declared decoded payload size.
    Size,
    /// Encoded stream length.
    CompressedSize,
}

/// One producer-defined collection field. Names remain inert text.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct CollectionField<'a> {
    /// Schema key used by CI entries and sort declarations.
    pub key: &'a Name,
    /// Original field dictionary, including extension entries.
    pub dictionary: &'a Dictionary,
    /// Display label.
    pub name: String,
    /// Declared type/source.
    pub kind: CollectionFieldKind,
    /// Declared presentation order, if present.
    pub order: Option<i64>,
    /// Initial visibility, default true.
    pub visible: bool,
    /// Producer editing hint, default false; no editing is performed.
    pub editable: bool,
}

/// One sort key in declared priority order (Table 156).
#[derive(Debug, Clone, Copy)]
#[non_exhaustive]
pub struct CollectionSort<'a> {
    /// Schema field name.
    pub key: &'a Name,
    /// Ascending when true; absent direction defaults to true.
    pub ascending: bool,
}

/// Inert portable collection view. Schema/sort order is preserved, not applied.
/// Folder/navigator/layout extensions remain in `dictionary` for typed follow-up.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct Collection<'a> {
    /// Collection dictionary identity, if indirect.
    pub object: Option<ObjectId>,
    /// Source dictionary including Navigator, Folders, Colors and Split.
    pub dictionary: &'a Dictionary,
    /// Initial EmbeddedFiles byte key; never a filesystem path.
    pub initial_file: Option<&'a [u8]>,
    /// Producer-declared initial view.
    pub view: CollectionView,
    /// Schema fields in source dictionary key order.
    pub fields: Vec<CollectionField<'a>>,
    /// Sort keys in source priority order.
    pub sort: Vec<CollectionSort<'a>>,
}

/// Collection item data keeps integer precision and string/date source text.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum CollectionValue {
    /// Text or date. The matching schema determines which it is.
    Text(String),
    /// Exact PDF integer.
    Integer(i64),
    /// Finite PDF real number.
    Real(f64),
}

/// One CI entry, including an optional presentation-only subitem prefix.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct CollectionItem<'a> {
    /// Corresponding schema key.
    pub key: &'a Name,
    /// Source data; absent D in a subitem remains absent.
    pub value: Option<CollectionValue>,
    /// Display prefix, ignored for sorting.
    pub prefix: Option<String>,
}

/// Encrypted-payload declaration (Table 28). It never installs a handler or opens a file.
#[derive(Debug, Clone, Copy)]
#[non_exhaustive]
pub struct EncryptedPayload<'a> {
    /// Original EP dictionary, retaining extensions.
    pub dictionary: &'a Dictionary,
    /// Declared cryptographic filter name.
    pub subtype: &'a Name,
    /// Declared version name, distinct from a floating-point number.
    pub version: Option<&'a Name>,
}

impl Document {
    /// Reads the catalog's portable collection declarations (12.3.5).
    /// No navigator is executed, file opened, value normalized or profile validated.
    ///
    /// # Errors
    /// Malformed declarations, cycles, text repairs and traversal/result budgets.
    pub fn collection(&self) -> ReadResult<Option<Collection<'_>>> {
        Context::run(self, |mut context| {
            let (catalog, source) = context.catalog()?;
            let Some(value) = context.present(catalog, b"Collection", source)? else {
                return Ok(None);
            };
            let indirect = value.as_reference().is_some();
            let (dictionary, source) = context.dictionary(value, source)?;
            context.type_name(dictionary, b"Collection", source)?;
            let view =
                match read::name(&mut context, dictionary, b"View", source)?.map(Name::as_bytes) {
                    None | Some(b"D") => CollectionView::Details,
                    Some(b"T") => CollectionView::Tiles,
                    Some(b"H") => CollectionView::Hidden,
                    Some(b"C") => {
                        let navigator = context.required(dictionary, b"Navigator", source)?;
                        context.dictionary(navigator, source)?;
                        CollectionView::Navigator
                    }
                    _ => {
                        return Err(source
                            .error(Error::syntax("invalid collection View"))
                            .into());
                    }
                };
            // Table 153 explicitly treats a non-string D as absent.
            let initial_file = context
                .entry(dictionary, b"D", source)?
                .and_then(Value::as_string);
            let fields = schema(&mut context, dictionary, source)?;
            let sort = sort(&mut context, dictionary, source)?;
            Ok(Some(Collection {
                object: source.object.filter(|_| indirect),
                dictionary,
                initial_file,
                view,
                fields,
                sort,
            }))
        })
    }
}

fn schema<'a>(
    context: &mut Context<'a>,
    owner: &'a Dictionary,
    source: Source,
) -> ReadResult<Vec<CollectionField<'a>>> {
    let Some(value) = context.present(owner, b"Schema", source)? else {
        return Ok(Vec::new());
    };
    let (dictionary, source) = context.dictionary(value, source)?;
    context.type_name(dictionary, b"CollectionSchema", source)?;
    context.allocate::<CollectionField<'_>>(dictionary.len(), source)?;
    let mut fields = Vec::new();
    for (key, value) in dictionary {
        if key == b"Type" {
            continue;
        }
        context.step(0, source)?;
        let (dictionary, source) = context.dictionary(value, source)?;
        context.type_name(dictionary, b"CollectionField", source)?;
        let kind = match read::name(context, dictionary, b"Subtype", source)?.map(Name::as_bytes) {
            Some(b"S") => CollectionFieldKind::Text,
            Some(b"D") => CollectionFieldKind::Date,
            Some(b"N") => CollectionFieldKind::Number,
            Some(b"F") => CollectionFieldKind::FileName,
            Some(b"Desc") => CollectionFieldKind::Description,
            Some(b"ModDate") => CollectionFieldKind::ModificationDate,
            Some(b"CreationDate") => CollectionFieldKind::CreationDate,
            Some(b"Size") => CollectionFieldKind::Size,
            Some(b"CompressedSize") => CollectionFieldKind::CompressedSize,
            _ => {
                return Err(source
                    .error(Error::syntax("invalid collection field Subtype"))
                    .into());
            }
        };
        let name = read::text(context, dictionary, b"N", source)?
            .ok_or_else(|| source.error(Error::syntax("collection field label is absent")))?;
        let order = context
            .entry(dictionary, b"O", source)?
            .map(|value| {
                value.as_integer().ok_or_else(|| {
                    source.error(Error::syntax("collection field order must be an integer"))
                })
            })
            .transpose()?;
        fields.push(CollectionField {
            key,
            dictionary,
            name,
            kind,
            order,
            visible: boolean(context, dictionary, b"V", source)?.unwrap_or(true),
            editable: boolean(context, dictionary, b"E", source)?.unwrap_or(false),
        });
    }
    Ok(fields)
}

fn boolean<'a>(
    context: &mut Context<'a>,
    dictionary: &'a Dictionary,
    key: &[u8],
    source: Source,
) -> ReadResult<Option<bool>> {
    Ok(context
        .entry(dictionary, key, source)?
        .map(|value| {
            value
                .as_bool()
                .ok_or_else(|| source.error(Error::syntax("collection flag must be boolean")))
        })
        .transpose()?)
}

fn sort<'a>(
    context: &mut Context<'a>,
    owner: &'a Dictionary,
    source: Source,
) -> ReadResult<Vec<CollectionSort<'a>>> {
    let Some(value) = context.present(owner, b"Sort", source)? else {
        return Ok(Vec::new());
    };
    let (dictionary, source) = context.dictionary(value, source)?;
    context.type_name(dictionary, b"CollectionSort", source)?;
    let keys = context.required(dictionary, b"S", source)?;
    let keys = match keys {
        Value::Array(values) => values.as_slice(),
        Value::Name(_) => std::slice::from_ref(keys),
        _ => {
            return Err(source
                .error(Error::syntax("collection sort requires name or array"))
                .into());
        }
    };
    let directions = context.entry(dictionary, b"A", source)?;
    if directions.is_some_and(|value| !matches!(value, Value::Boolean(_) | Value::Array(_))) {
        return Err(source
            .error(Error::syntax(
                "collection sort direction must be boolean or array",
            ))
            .into());
    }
    context.allocate::<CollectionSort<'_>>(keys.len(), source)?;
    let mut result = Vec::new();
    for (index, key) in keys.iter().enumerate() {
        context.step(0, source)?;
        let (key, source) = context.resolve(key, source)?;
        let key = key
            .as_name()
            .ok_or_else(|| source.error(Error::syntax("collection sort key must be a name")))?;
        let direction = match directions {
            Some(Value::Array(values)) => values.get(index),
            other => other,
        };
        let ascending = match direction
            .map(|value| context.resolve(value, source))
            .transpose()?
        {
            None | Some((Value::Null, _)) => true,
            Some((Value::Boolean(value), _)) => *value,
            _ => {
                return Err(source
                    .error(Error::syntax("collection sort direction must be boolean"))
                    .into());
            }
        };
        result.push(CollectionSort { key, ascending });
    }
    Ok(result)
}

pub(super) fn items<'a>(
    context: &mut Context<'a>,
    owner: &'a Dictionary,
    source: Source,
) -> ReadResult<Vec<CollectionItem<'a>>> {
    let Some(value) = context.present(owner, b"CI", source)? else {
        return Ok(Vec::new());
    };
    let (dictionary, source) = context.dictionary(value, source)?;
    context.type_name(dictionary, b"CollectionItem", source)?;
    context.allocate::<CollectionItem<'_>>(dictionary.len(), source)?;
    let mut result = Vec::new();
    for (key, value) in dictionary {
        if key == b"Type" {
            continue;
        }
        context.step(0, source)?;
        let (value, source) = context.resolve(value, source)?;
        let (value, prefix) = if let Value::Dictionary(dictionary) = value {
            context.type_name(dictionary, b"CollectionSubitem", source)?;
            (
                context.entry(dictionary, b"D", source)?,
                read::text(context, dictionary, b"P", source)?,
            )
        } else {
            (Some(value), None)
        };
        let value = match value {
            None => None,
            Some(Value::String(bytes)) => Some(CollectionValue::Text(context.text(bytes, source)?)),
            Some(Value::Integer(value)) => Some(CollectionValue::Integer(*value)),
            Some(Value::Real(value)) if value.is_finite() => Some(CollectionValue::Real(*value)),
            _ => {
                return Err(source
                    .error(Error::syntax(
                        "collection item requires text, date or finite number",
                    ))
                    .into());
            }
        };
        result.push(CollectionItem { key, value, prefix });
    }
    Ok(result)
}

pub(super) fn encrypted<'a>(
    context: &mut Context<'a>,
    owner: &'a Dictionary,
    source: Source,
) -> ReadResult<Option<EncryptedPayload<'a>>> {
    let Some(value) = context.present(owner, b"EP", source)? else {
        return Ok(None);
    };
    let (dictionary, source) = context.dictionary(value, source)?;
    context.type_name(dictionary, b"EncryptedPayload", source)?;
    let subtype = read::name(context, dictionary, b"Subtype", source)?
        .ok_or_else(|| source.error(Error::syntax("encrypted payload Subtype is absent")))?;
    let version = read::name(context, dictionary, b"Version", source)?;
    Ok(Some(EncryptedPayload {
        dictionary,
        subtype,
        version,
    }))
}
