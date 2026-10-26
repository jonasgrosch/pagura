//! Source image assets and stored thumbnails (8.9, 12.3.4).
//!
//! These views do not claim painted visibility or standalone image appearance.
//! Stored bytes may be encrypted; encoded bytes retain codec/PDF interpretation
//! dependencies. Normalized image/page rendering belongs to the interpreter.

use crate::source::ReadResult;
use crate::view::Context;
use crate::{Document, ImageCodec, ImageStream, IndirectObject, StreamData};
use pagura_core::{Dictionary, Error, Name, ObjectId, Value};

/// A declared source image or thumbnail, whose payload is decoded only on request.
#[derive(Debug, Clone, Copy)]
pub struct ImageAsset<'a> {
    document: &'a Document,
    object: &'a IndirectObject,
    dictionary: &'a Dictionary,
    width: u32,
    height: u32,
    thumbnail: bool,
}

/// Exact source bytes or bytes with general-purpose filters and encryption removed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ImageTransferMode {
    /// Exact stored stream bytes, possibly encrypted or wrapped in PDF filters.
    Stored,
    /// General filters/decryption applied; a trailing image codec is retained.
    Encoded,
}

/// Transferred bytes and their codec. Neither mode promises a standalone image.
#[derive(Debug)]
#[non_exhaustive]
pub struct ImageTransfer {
    /// Exact mode requested by the caller.
    pub mode: ImageTransferMode,
    /// Stream bytes. A codec of None in Encoded mode means packed PDF samples.
    pub data: StreamData,
    /// Retained image codec and its parameters; None for Stored mode.
    pub codec: Option<(ImageCodec, Option<Dictionary>)>,
}

impl<'a> ImageAsset<'a> {
    /// Indirect source identity in the immutable document.
    pub fn object(&self) -> ObjectId {
        self.object.id()
    }
    /// Original image dictionary, including colour, Decode, mask and filter entries.
    pub fn dictionary(&self) -> &'a Dictionary {
        self.dictionary
    }
    /// Declared sample width; transfer does not validate the codestream's dimensions.
    pub const fn width(&self) -> u32 {
        self.width
    }
    /// Declared sample height.
    pub const fn height(&self) -> u32 {
        self.height
    }
    /// Whether this asset was selected through a Thumb entry (12.3.4).
    pub const fn is_thumbnail(&self) -> bool {
        self.thumbnail
    }
    /// Transfers original or encoded bytes without applying colour/masks/Decode.
    /// Content is never executed and no external stream is fetched.
    ///
    /// # Errors
    /// Stream bounds, authorization, filter and decoded-byte budgets. Stored mode
    /// preserves ciphertext and does not imply successful authentication.
    pub fn transfer(&self, mode: ImageTransferMode) -> ReadResult<ImageTransfer> {
        let (data, codec) = match mode {
            ImageTransferMode::Stored => (self.document.raw_stream_data(self.object)?.into(), None),
            ImageTransferMode::Encoded => {
                let ImageStream { data, codec, .. } = self.document.image_stream(self.object)?;
                (data, codec)
            }
        };
        Ok(ImageTransfer { mode, data, codec })
    }
}

impl Document {
    /// Selects an indirect image XObject by exact number/generation (8.9.5).
    /// Dictionary inspection does not decode samples, masks or general filters.
    ///
    /// # Errors
    /// Missing identity, non-image/non-stream source, malformed dimensions or budgets.
    pub fn image_asset(&self, id: ObjectId) -> ReadResult<ImageAsset<'_>> {
        asset(&mut Context::new(self), id, AssetKind::Image)
    }

    /// Reads the stored thumbnail of an object dictionary (12.3.4).
    /// Absent/null Thumb returns None; page, Filespec and collection-folder owners
    /// all use this same source view. A thumbnail is separate from a rendered page.
    ///
    /// # Errors
    /// Invalid owner/stream identity, cycles, dimensions and view budgets.
    pub fn thumbnail(&self, owner: ObjectId) -> ReadResult<Option<ImageAsset<'_>>> {
        let mut context = Context::new(self);
        let source = context.source(owner)?;
        let dictionary = self
            .object(owner)?
            .dictionary()
            .ok_or_else(|| source.error(Error::syntax("thumbnail owner must be a dictionary")))?;
        let Some(value) = context.present(dictionary, b"Thumb", source)? else {
            return Ok(None);
        };
        if value.as_reference().is_none() {
            return Err((source.error(Error::syntax("Thumb must reference a stream"))).into());
        }
        let (_, location) = context.resolve(value, source)?;
        let id = location
            .object
            .ok_or_else(|| location.error(Error::syntax("Thumb must reference a stream")))?;
        asset(&mut context, id, AssetKind::Thumbnail).map(Some)
    }

    /// Lists direct image XObjects in a page's inherited Resources dictionary.
    /// This deliberately names its scope: it includes unused/hidden resources and
    /// excludes form/pattern/annotation/inline occurrences and their dependencies.
    /// Repeated resource names for one object retain separate resource records.
    ///
    /// # Errors
    /// Page/resource type and reference errors, cycles and view budgets.
    pub fn page_image_resources(&self, page: usize) -> ReadResult<Vec<ImageResource<'_>>> {
        let page = self.page(page)?;
        let mut context = Context::new(self);
        let source = context.source(page.id())?;
        let resources = page.resources();
        let Some(value) = context.present(resources, b"XObject", source)? else {
            return Ok(Vec::new());
        };
        let (objects, source) = context.dictionary(value, source)?;
        context.allocate::<ImageResource<'_>>(objects.len(), source)?;
        let mut images = Vec::new();
        for (name, _) in objects {
            context.step(0, source)?;
            let Some(value) = context.present(objects, name.as_bytes(), source)? else {
                continue;
            };
            let (dictionary, location) = context.dictionary(value, source)?;
            if context
                .entry(dictionary, b"Subtype", location)?
                .and_then(Value::as_name)
                .is_some_and(|name| name == b"Image")
            {
                let id = location
                    .object
                    .filter(|_| value.as_reference().is_some())
                    .ok_or_else(|| {
                        location.error(Error::syntax("image XObject must be indirect"))
                    })?;
                images.push(ImageResource {
                    name,
                    asset: asset(&mut context, id, AssetKind::Image)?,
                });
            }
        }
        Ok(images)
    }
}

/// A page resource name and its source asset; this does not count painted uses.
#[derive(Debug, Clone, Copy)]
#[non_exhaustive]
pub struct ImageResource<'a> {
    /// Original resource name.
    pub name: &'a Name,
    /// Source image asset.
    pub asset: ImageAsset<'a>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum AssetKind {
    Image,
    Thumbnail,
}

fn asset<'a>(
    context: &mut Context<'a>,
    id: ObjectId,
    kind: AssetKind,
) -> ReadResult<ImageAsset<'a>> {
    let source = context.source(id)?;
    let object = context.document.object(id)?;
    let dictionary = object
        .dictionary()
        .filter(|_| object.is_stream())
        .ok_or_else(|| source.error(Error::syntax("image asset must be a stream")))?;
    let subtype = context.entry(dictionary, b"Subtype", source)?;
    if (kind == AssetKind::Image || subtype.is_some())
        && subtype
            .and_then(Value::as_name)
            .is_none_or(|name| name != b"Image")
    {
        return Err((source.error(Error::syntax("image asset Subtype must be Image"))).into());
    }
    let mut dimension = |key| -> ReadResult<u32> {
        Ok(context
            .required(dictionary, key, source)?
            .as_integer()
            .and_then(|value| u32::try_from(value).ok())
            .filter(|value| *value > 0)
            .ok_or_else(|| {
                source.error(Error::syntax("image dimension must be a positive integer"))
            })?)
    };
    let width = dimension(b"Width")?;
    let height = dimension(b"Height")?;
    context.allocate::<ImageAsset<'_>>(1, source)?;
    Ok(ImageAsset {
        document: context.document,
        object,
        dictionary,
        width,
        height,
        thumbnail: kind == AssetKind::Thumbnail,
    })
}
