//! Appearance streams kept as data.

use crate::values::{optional, optional_dictionary};
use pagura_core::{Dictionary, Error, Name, ObjectId, Rect, Result, Value};
use pagura_document::Document;

/// The three interaction appearances defined by Table 170. The set is closed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum AppearanceMode {
    /// Ordinary display and printing.
    #[default]
    Normal,
    /// The pointer is inside the annotation without a pressed button.
    Rollover,
    /// A pointer button is pressed inside the annotation.
    Down,
}

/// Selects stored annotation artwork for `mode` and an optional state override.
/// Missing rollover/down entries use N; state dictionaries use the override or
/// the annotation's AS. An undefined state selects [`Appearance::Hidden`].
/// Missing/null AP returns `None`, allowing the caller's ordinary generation.
/// Selection is inert: it neither changes the annotation nor executes actions.
///
/// # Errors
/// Malformed appearance dictionaries, missing required states, non-stream
/// selections, and document resolution limits (12.5.5; Table 170).
pub fn stored_appearance(
    document: &Document,
    annotation: &Dictionary,
    mode: AppearanceMode,
    state: Option<&Name>,
) -> Result<Option<Appearance>> {
    let Some(ap) = optional_dictionary(document, annotation, b"AP")? else {
        return Ok(None);
    };
    let key = match mode {
        AppearanceMode::Normal => b"N",
        AppearanceMode::Rollover => b"R",
        AppearanceMode::Down => b"D",
    };
    let selected = match ap.get(key) {
        Some(value) if !document.resolve(value)?.is_null() => value,
        _ => ap
            .get(b"N")
            .ok_or(Error::syntax("appearance dictionary requires N"))?,
    };
    if let Some(id) = selected.as_reference()
        && document.object(id)?.is_stream()
    {
        return Ok(Some(Appearance::Existing(id)));
    }
    let states = document.resolve_dictionary(selected)?;
    let state = match state {
        Some(state) => state,
        None => optional(document, annotation, b"AS")?
            .and_then(Value::as_name)
            .ok_or(Error::syntax("annotation appearance state is absent"))?,
    };
    let Some(value) = states.get(state.as_bytes()) else {
        return Ok(Some(Appearance::Hidden));
    };
    if document.resolve(value)?.is_null() {
        return Ok(Some(Appearance::Hidden));
    }
    let id = value
        .as_reference()
        .ok_or(Error::syntax("appearance state must reference a stream"))?;
    if !document.object(id)?.is_stream() {
        return Err(Error::syntax("appearance state must be a stream"));
    }
    Ok(Some(Appearance::Existing(id)))
}

/// The normal appearance selected for one widget (12.5.5). The set is
/// closed: an appearance is a stored stream, nothing, or generated data.
#[derive(Debug, Clone, PartialEq)]
pub enum Appearance {
    /// An existing indirect stream, selected using the replacement AS state.
    Existing(ObjectId),
    /// An undefined appearance state paints nothing (12.5.5).
    Hidden,
    /// An unencoded appearance program and its complete form dictionary.
    Generated(AppearanceStream),
}

/// An appearance stream kept as data, without allocating an indirect object
/// ID: a complete Form XObject dictionary and its decoded content (8.10).
///
/// The dictionary carries `BBox`, `Resources` and, when the appearance
/// replaces the text of an existing stream, that stream's other entries
/// (such as `Matrix` and `Group`), which may still refer to objects of the
/// source document. Read them as a stored form's entries are read.
#[derive(Debug, Clone, PartialEq)]
pub struct AppearanceStream {
    dictionary: Dictionary,
    content: Vec<u8>,
}

impl AppearanceStream {
    /// Complete Form XObject dictionary, including BBox and Resources.
    /// Filter, DecodeParms and Length are absent: content is decoded data.
    pub fn dictionary(&self) -> &Dictionary {
        &self.dictionary
    }

    /// Content program, before any stream encoding.
    pub fn content(&self) -> &[u8] {
        &self.content
    }

    /// Estimated caller-owned storage; source references count only their IDs.
    pub fn retained_bytes(&self) -> usize {
        self.dictionary
            .retained_bytes()
            .saturating_add(self.content.capacity())
            .saturating_add(size_of::<Self>())
    }

    /// Keeps `dictionary` and `content` as a stream within the document's
    /// storage budget.
    pub(crate) fn new(
        document: &Document,
        dictionary: Dictionary,
        content: Vec<u8>,
    ) -> Result<Self> {
        let stream = Self {
            dictionary,
            content,
        };
        if stream.retained_bytes() > document.limits().document_bytes {
            return Err(Error::limit("generated appearance storage budget exceeded"));
        }
        Ok(stream)
    }
}

/// A Form XObject dictionary with `bbox` and the identity matrix (Table 93).
pub(crate) fn form_dictionary(bbox: Rect) -> Dictionary {
    let mut dictionary = Dictionary::new();
    dictionary.insert(Name::new(b"Type"), Value::name(b"XObject"));
    dictionary.insert(Name::new(b"Subtype"), Value::name(b"Form"));
    dictionary.insert(Name::new(b"FormType"), Value::Integer(1));
    dictionary.insert(
        Name::new(b"BBox"),
        Value::Array(
            [bbox.x0(), bbox.y0(), bbox.x1(), bbox.y1()]
                .map(Value::Real)
                .to_vec(),
        ),
    );
    dictionary
}

#[cfg(test)]
mod tests {
    use super::*;
    use pagura_core::{ErrorKind, Limits, syntax};
    use pagura_document::OpenOptions;
    use pagura_testkit::fixture;

    fn dictionary(source: &[u8]) -> Dictionary {
        syntax::Parser::new(source, syntax::Origin::Buffer, Limits::default())
            .parse()
            .unwrap()
            .as_dictionary()
            .unwrap()
            .clone()
    }

    #[test]
    fn all_modes_select_their_stream_or_current_state() {
        let document = Document::open(
            fixture("content", "annotation-appearance-streams.pdf"),
            &OpenOptions::default(),
        )
        .unwrap();
        let streams = dictionary(b"<< /AP << /N 5 0 R /R 6 0 R /D 7 0 R >> /AS 42 >>");
        let states = dictionary(b"<< /AP << /N << /Off 5 0 R /On 6 0 R >> /R << /Off 6 0 R /On 7 0 R >> /D << /Off 7 0 R /On 5 0 R >> >> /AS /On >>");
        for (mode, normal, selected, off) in [
            (AppearanceMode::Normal, 5, 6, 5),
            (AppearanceMode::Rollover, 6, 7, 6),
            (AppearanceMode::Down, 7, 5, 7),
        ] {
            for (annotation, state, expected) in [
                (&streams, None, normal),
                (&states, None, selected),
                (&states, Some(Name::new(b"Off")), off),
            ] {
                assert_eq!(
                    stored_appearance(&document, annotation, mode, state.as_ref()).unwrap(),
                    Some(Appearance::Existing(ObjectId::new(expected, 0)))
                );
            }
            assert_eq!(
                stored_appearance(&document, &states, mode, Some(&Name::new(b"Missing"))).unwrap(),
                Some(Appearance::Hidden)
            );
        }
    }

    #[test]
    fn optional_modes_default_to_normal_without_resolving_unselected_artwork() {
        let document = Document::open(
            fixture("content", "annotation-appearance-streams.pdf"),
            &OpenOptions::default(),
        )
        .unwrap();
        for source in [
            b"<< /AP << /N 5 0 R >> >>".as_slice(),
            b"<< /AP << /N 5 0 R /R null /D 8 0 R >> >>",
        ] {
            for mode in [
                AppearanceMode::Normal,
                AppearanceMode::Rollover,
                AppearanceMode::Down,
            ] {
                assert_eq!(
                    stored_appearance(&document, &dictionary(source), mode, None).unwrap(),
                    Some(Appearance::Existing(ObjectId::new(5, 0)))
                );
            }
        }
        let inactive = dictionary(b"<< /AP << /N 5 0 R /R 42 /D false >> >>");
        assert_eq!(
            stored_appearance(&document, &inactive, AppearanceMode::Normal, None).unwrap(),
            Some(Appearance::Existing(ObjectId::new(5, 0)))
        );
        for source in [b"<< >>".as_slice(), b"<< /AP null >>", b"<< /AP 8 0 R >>"] {
            assert_eq!(
                stored_appearance(&document, &dictionary(source), AppearanceMode::Normal, None)
                    .unwrap(),
                None
            );
        }
        for source in [
            b"<< /AP << /N << /On null >> >> /AS /On >>".as_slice(),
            b"<< /AP << /N << /On 8 0 R >> >> /AS /On >>",
        ] {
            assert_eq!(
                stored_appearance(&document, &dictionary(source), AppearanceMode::Normal, None)
                    .unwrap(),
                Some(Appearance::Hidden)
            );
        }
    }

    #[test]
    fn invalid_selected_artwork_and_missing_states_fail_explicitly() {
        let document = Document::open(
            fixture("content", "annotation-appearance-streams.pdf"),
            &OpenOptions::default(),
        )
        .unwrap();
        for source in [
            b"<< /AP 42 >>".as_slice(),
            b"<< /AP << >> >>",
            b"<< /AP << /N null >> >>",
            b"<< /AP << /N << /On 5 0 R >> >> >>",
            b"<< /AP << /N << /On 5 0 R >> >> /AS 42 >>",
            b"<< /AP << /N << /On true >> >> /AS /On >>",
            b"<< /AP << /N << /On 1 0 R >> >> /AS /On >>",
        ] {
            assert_eq!(
                stored_appearance(&document, &dictionary(source), AppearanceMode::Normal, None)
                    .unwrap_err()
                    .kind,
                ErrorKind::Syntax
            );
        }
    }
}
