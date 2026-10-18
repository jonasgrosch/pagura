//! Encoded stream access, decryption and bounded filter application (7.3.8, 7.4, 7.6).

use crate::{Document, IndirectObject, features, repairs};
use pagura_codec::{Filter, FilterChain};
use pagura_core::{Diagnostics, Error, ObjectId, Result, Value};
use pagura_crypto::standard::CryptMethod;
use std::borrow::Cow;

/// Whether a stream inherits its default cipher or overrides it explicitly.
#[derive(Debug)]
pub(crate) enum CryptSelection {
    Default,
    Identity,
    Named(pagura_core::Name),
}

pub(crate) fn raw<'a>(document: &'a Document, object: &IndirectObject) -> Result<&'a [u8]> {
    let range = object
        .stream_range()
        .ok_or_else(|| Error::syntax("object is not a stream").in_object(object.id()))?;
    document
        .bytes
        .get(range)
        .ok_or_else(|| Error::syntax("stream lies outside the file").in_object(object.id()))
}

pub(crate) fn decoded<'a>(
    document: &'a Document,
    object: &IndirectObject,
) -> Result<Cow<'a, [u8]>> {
    let (data, chain) = document.prepare_stream(object, &mut Vec::new())?;
    let mut diagnostics = Diagnostics::new(document.recovery());
    let decoded = chain
        .decode(&data, document.limits().decoded_bytes, &mut diagnostics)
        .map_err(|error| error.in_object(object.id()))?;
    document.report(
        diagnostics
            .take()
            .into_iter()
            .map(|diagnostic| diagnostic.in_object(object.id())),
    );
    Ok(match decoded {
        Cow::Owned(bytes) => Cow::Owned(bytes),
        Cow::Borrowed(_) => data,
    })
}

/// Encoded image data with every filter except a trailing image codec
/// applied (clauses 7.4 and 8.9.5).
#[derive(Debug)]
pub struct ImageStream<'a> {
    /// The data after decryption and all non-image filters.
    pub data: Cow<'a, [u8]>,
    /// The trailing image codec and its `/DecodeParms`, if the stream has one.
    pub codec: Option<(pagura_codec::ImageCodec, Option<pagura_core::Dictionary>)>,
}

pub(crate) fn image<'a>(
    document: &'a Document,
    object: &IndirectObject,
    parameters: Option<&Value>,
) -> Result<ImageStream<'a>> {
    let (data, mut chain) =
        document.prepare_stream_with_parameters(object, parameters, &mut Vec::new())?;
    let codec = chain.split_image();
    let mut diagnostics = Diagnostics::new(document.recovery());
    let decoded = chain
        .decode(&data, document.limits().decoded_bytes, &mut diagnostics)
        .map_err(|error| error.in_object(object.id()))?;
    document.report(
        diagnostics
            .take()
            .into_iter()
            .map(|diagnostic| diagnostic.in_object(object.id())),
    );
    let data = match decoded {
        Cow::Owned(bytes) => Cow::Owned(bytes),
        Cow::Borrowed(_) => data,
    };
    Ok(ImageStream { data, codec })
}

impl Document {
    pub(crate) fn prepare_stream<'a>(
        &'a self,
        object: &IndirectObject,
        path: &mut Vec<ObjectId>,
    ) -> Result<(Cow<'a, [u8]>, FilterChain)> {
        self.prepare_stream_with_parameters(
            object,
            object
                .dictionary()
                .and_then(|dictionary| dictionary.get(b"DecodeParms")),
            path,
        )
    }

    fn prepare_stream_with_parameters<'a>(
        &'a self,
        object: &IndirectObject,
        parameters: Option<&Value>,
        path: &mut Vec<ObjectId>,
    ) -> Result<(Cow<'a, [u8]>, FilterChain)> {
        let id = object.id();
        let raw = self.raw_stream_data(object)?;
        let dictionary = object
            .dictionary()
            .ok_or_else(|| Error::syntax("stream has no dictionary").in_object(id))?;
        if dictionary.contains_key(b"F") {
            return Err(Error::unsupported(
                features::EXTERNAL_STREAM,
                "external stream data is not supported",
            )
            .in_object(id));
        }
        let filter = dictionary
            .get(b"Filter")
            .map(|value| self.resolve_deep(value, path))
            .transpose()?;
        let params = parameters
            .map(|value| self.resolve_deep(value, path))
            .transpose()?;
        validate_crypt(filter.as_ref(), params.as_ref()).map_err(|error| error.in_object(id))?;
        let mut chain = FilterChain::from_entries(filter.as_ref(), params.as_ref())
            .map_err(|error| error.in_object(id))?;
        let decrypted = decrypt(self, object, raw, &mut chain)?;
        Ok((decrypted, chain))
    }
}

fn decrypt<'a>(
    document: &Document,
    object: &IndirectObject,
    raw: &'a [u8],
    chain: &mut FilterChain,
) -> Result<Cow<'a, [u8]>> {
    let explicit = chain.take_crypt();
    if chain
        .filters()
        .iter()
        .any(|filter| matches!(filter, Filter::Crypt(_)))
    {
        return Err(
            Error::syntax("Crypt must be the first and only Crypt filter").in_object(object.id()),
        );
    }
    let Some(security) = &document.security else {
        if explicit
            .flatten()
            .is_some_and(|name| name.as_bytes() != b"Identity")
        {
            return Err(
                Error::syntax("Crypt filter requires an encryption dictionary")
                    .in_object(object.id()),
            );
        }
        return Ok(Cow::Borrowed(raw));
    };
    let dictionary = object
        .dictionary()
        .ok_or(Error::syntax("stream requires a dictionary"))?;
    if dictionary.is_name(b"Type", b"XRef") && !object.is_direct() {
        if explicit.is_some() {
            return Err(Error::syntax("xref streams cannot be encrypted").in_object(object.id()));
        }
        return Ok(Cow::Borrowed(raw));
    }
    let selection = match explicit {
        None => CryptSelection::Default,
        Some(None) => CryptSelection::Identity,
        Some(Some(name)) => CryptSelection::Named(name),
    };
    let method = security
        .stream_method(document, object, selection)
        .map_err(|error| error.in_object(object.id()))?;
    if method == CryptMethod::Identity {
        return Ok(Cow::Borrowed(raw));
    }
    security
        .decrypt_stream(method, object.id(), raw)
        .map(Cow::Owned)
        .map_err(|error| error.in_object(object.id()))
}

/// The codec contract leaves Crypt parameters to the document layer.
pub(crate) fn validate_crypt(filter: Option<&Value>, params: Option<&Value>) -> Result<()> {
    let names = match filter {
        Some(Value::Name(name)) => std::slice::from_ref(name),
        _ => &[],
    };
    if names.first().is_some_and(|name| name == b"Crypt") {
        let params = match params {
            Some(Value::Array(values)) => values.first(),
            params => params,
        };
        crypt_params(params)?;
    } else if let Some(Value::Array(names)) = filter {
        for (index, name) in names.iter().enumerate() {
            if name.as_name().is_some_and(|name| name == b"Crypt") {
                let params = params
                    .and_then(Value::as_array)
                    .and_then(|params| params.get(index));
                crypt_params(params)?;
            }
        }
    }
    Ok(())
}

fn crypt_params(value: Option<&Value>) -> Result<()> {
    if let Some(Value::Dictionary(params)) = value {
        if params
            .get(b"Name")
            .is_some_and(|value| value.as_name().is_none())
        {
            return Err(Error::syntax("Crypt Name must be a name"));
        }
        if params.get(b"Type").is_some_and(|value| {
            value
                .as_name()
                .is_none_or(|name| name != b"CryptFilterDecodeParms")
        }) {
            return Err(Error::syntax("invalid Crypt DecodeParms Type"));
        }
    }
    Ok(())
}

/// Stream EOL syntax is stricter than ordinary PDF white space (7.3.8.1).
pub(crate) fn start(
    parser: &pagura_core::syntax::Parser<'_>,
    diagnostics: &mut Diagnostics,
    id: Option<ObjectId>,
) -> Result<usize> {
    let offset = parser.offset();
    match parser.data().get(offset..) {
        Some([b'\r', b'\n', ..]) => Ok(offset + 2),
        Some([b'\n', ..]) => Ok(offset + 1),
        Some([b'\r', ..]) => {
            let mut diagnostic = pagura_core::Diagnostic::new(
                repairs::STREAM_EOL,
                "bare CR after stream keyword; one CR used as the line terminator",
            )
            .at(parser.position());
            if let Some(id) = id {
                diagnostic = diagnostic.in_object(id);
            }
            diagnostics.repair(diagnostic)?;
            Ok(offset + 1)
        }
        _ => Err(
            Error::syntax("stream keyword must be followed by LF or CRLF").at(parser.position()),
        ),
    }
}

#[cfg(test)]
mod tests {
    use crate::{Document, OpenOptions};
    use pagura_core::{ErrorKind, Limits, Name, ObjectId, Recovery, Value};
    use pagura_testkit::fixture;

    fn open(name: &str, decoded_bytes: usize) -> Document {
        Document::open(
            fixture("image", &format!("{name}.pdf")),
            &OpenOptions {
                recovery: Recovery::Strict,
                limits: Limits {
                    decoded_bytes,
                    ..Limits::default()
                },
                ..OpenOptions::default()
            },
        )
        .unwrap()
    }

    fn parameters(document: &Document, object: &crate::IndirectObject) -> Value {
        let original = object.dictionary().unwrap().get(b"DecodeParms").unwrap();
        let Value::Array(mut entries) = document.resolve_deep(original, &mut Vec::new()).unwrap()
        else {
            panic!("fixture requires aligned parameters");
        };
        *entries.last_mut().unwrap() = Value::Null;
        Value::Array(entries)
    }

    #[test]
    fn parameter_override_preserves_decryption_predictors_and_cache_accounting() {
        let baseline = open("jpx-params-null", Limits::default().decoded_bytes);
        let baseline = baseline
            .image_stream(baseline.object(ObjectId::new(3, 0)).unwrap())
            .unwrap();
        for name in ["params-encrypted-string-null", "params-crypt-string-null"] {
            let document = open(name, Limits::default().decoded_bytes);
            let object = document.object(ObjectId::new(3, 0)).unwrap();
            let original = object.dictionary().unwrap().clone();
            assert_eq!(
                document.image_stream(object).unwrap_err().kind,
                ErrorKind::Syntax
            );
            let parameters = parameters(&document, object);
            let retained = document.retained.used();
            for _ in 0..2 {
                let stream = document
                    .image_stream_with_parameters(object, Some(&parameters))
                    .unwrap();
                assert_eq!(stream.data, baseline.data);
                assert_eq!(stream.codec, baseline.codec);
                assert_eq!(document.retained.used(), retained);
            }
            assert_eq!(object.dictionary(), Some(&original));
            assert!(document.image_stream(object).is_err());
            assert_eq!(document.diagnostics(), []);

            let limited = open(name, 1);
            let object = limited.object(ObjectId::new(3, 0)).unwrap();
            let error = limited
                .image_stream_with_parameters(object, Some(&parameters))
                .unwrap_err();
            assert_eq!(error.kind, ErrorKind::Limit);
            assert_eq!(error.object, Some(object.id()));
        }
    }

    #[test]
    fn absent_override_and_security_validation_keep_their_meaning() {
        let document = open("jpx-params-string-null", Limits::default().decoded_bytes);
        let object = document.object(ObjectId::new(3, 0)).unwrap();
        assert!(document.image_stream(object).is_err());
        let absent = document.image_stream_with_parameters(object, None).unwrap();
        assert_eq!(absent.data, document.raw_stream_data(object).unwrap());
        let mut external = object.clone();
        let Value::Dictionary(dictionary) = &mut external.value else {
            panic!("fixture requires a dictionary");
        };
        dictionary.insert(Name::new(b"F"), Value::Null);
        let error = document
            .image_stream_with_parameters(&external, None)
            .unwrap_err();
        assert_eq!(error.feature(), Some(crate::features::EXTERNAL_STREAM));
        assert_eq!(error.object, Some(object.id()));

        let document = open("params-crypt-string-null", Limits::default().decoded_bytes);
        let object = document.object(ObjectId::new(3, 0)).unwrap();
        let mut parameters = parameters(&document, object);
        let Value::Array(entries) = &mut parameters else {
            panic!("fixture requires an array");
        };
        let Value::Dictionary(crypt) = &mut entries[0] else {
            panic!("fixture requires Crypt parameters");
        };
        crypt.insert(Name::new(b"Name"), Value::Integer(1));
        assert_eq!(
            document
                .image_stream_with_parameters(object, Some(&parameters))
                .unwrap_err()
                .kind,
            ErrorKind::Syntax
        );
    }
}
