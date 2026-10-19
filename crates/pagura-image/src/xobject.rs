//! Image XObjects (8.9.5): dictionary validation, stream reading and the
//! placement of positions in the document.
use crate::dispatch::{self, Codec};
use crate::filter_rules::{self, Source};
use crate::{DecodedImage, parameters, stream};
use pagura_core::{Diagnostic, Diagnostics, Error, Position, Result, Value};
use pagura_document::{Document, IndirectObject, ObjectSource};

pub(crate) fn decode(document: &Document, image: &IndirectObject) -> Result<DecodedImage> {
    let mut diagnostics = Diagnostics::new(document.recovery());
    let result = decode_stream(document, image, &mut diagnostics);
    // Codecs report offsets in the decoded stream data; faults of the
    // dictionary have no offset and are placed at the object.
    let data = Position::Stream {
        object: image.id(),
        offset: 0,
    };
    let object = match image.source() {
        ObjectSource::File(range) => Position::File(range.start),
        ObjectSource::Compressed { .. } => Position::Unknown,
    };
    document.report(
        diagnostics
            .take()
            .into_iter()
            .map(|diagnostic| diagnostic.within(data).at(object).in_object(image.id())),
    );
    result.map_err(|error| error.within(data).at(object).in_object(image.id()))
}

fn decode_stream(
    document: &Document,
    image: &IndirectObject,
    diagnostics: &mut Diagnostics,
) -> Result<DecodedImage> {
    let dictionary = image
        .dictionary()
        .ok_or(Error::syntax("image is not a dictionary"))?;
    if dictionary
        .get(b"Subtype")
        .map(|v| document.resolve(v))
        .transpose()?
        .and_then(Value::as_name)
        .is_none_or(|n| n != b"Image")
    {
        return Err(Error::syntax("image XObject requires Subtype Image"));
    }
    // Subtype identifies the image; the optional Type only repeats it.
    if let Some(kind) = dictionary
        .get(b"Type")
        .map(|v| document.resolve(v))
        .transpose()?
        && !kind.is_null()
        && kind.as_name().is_none_or(|n| n != b"XObject")
    {
        diagnostics.repair(Diagnostic::new(
            crate::repairs::XOBJECT_TYPE,
            "invalid image XObject Type ignored for Subtype Image (8.9.5 Table 87)",
        ))?;
    }
    let dictionary = parameters::resolved_dictionary(document, dictionary, diagnostics)?;
    filter_rules::validate(document, &dictionary, Source::XObject, diagnostics)?;
    let (width, height) = parameters::dimensions(&dictionary)?;
    parameters::budget(width, height, 1, 1, &document.limits())?;
    let stream = stream::read(document, image, diagnostics)?;
    let codec = stream.codec.as_ref().map(|(codec, params)| Codec {
        codec: *codec,
        params: params.as_ref(),
    });
    dispatch::decode(document, &dictionary, &stream.data, codec, diagnostics)
}
