//! Request decoding: the only code that knows request layouts.

mod progressive;

use super::{
    Field, OpenFlags, PageRef, PngRequest, Request, RequestFrame, RequestKind, TileRequest,
};
use crate::editing;
use crate::reading::{self, MAX_QUERY_BYTES, Search};
use pagura::{Error, Limits, Position, Result, Tile};

/// Decode a body (excluding the four-byte length). No allocation is needed.
///
/// # Errors
/// Returns positioned `Syntax` for truncated, unknown or noncanonical payloads,
/// or `Limit` for oversized PDF, password or query byte strings.
pub fn decode<'a>(body: &'a [u8], limits: &Limits) -> Result<RequestFrame<'a>> {
    let mut reader = Reader::new(body);
    let kind: RequestKind = reader.code()?;
    let id = reader.u32()?;
    let request = match kind {
        RequestKind::SourceCreate
        | RequestKind::SourceSupply
        | RequestKind::SourceFinish
        | RequestKind::SourceClose
        | RequestKind::OpenJob
        | RequestKind::PageJob
        | RequestKind::RenderJob
        | RequestKind::JobAdvance
        | RequestKind::JobAcknowledge
        | RequestKind::JobCancel
        | RequestKind::JobClose
        | RequestKind::JobStats => Request::Progressive(reader.progressive(kind, limits)?),
        RequestKind::Reauthenticate => Request::Reauthenticate {
            document: reader.field(Reader::u32)?,
            password: reader.blob(limits.string_bytes)?.get(),
        },
        RequestKind::Files
        | RequestKind::FileBytes
        | RequestKind::FileCheck
        | RequestKind::ObjectMetadata
        | RequestKind::ObjectMetadataBytes
        | RequestKind::Collection
        | RequestKind::Images
        | RequestKind::ImageBytes
        | RequestKind::Thumbnail => Request::Reading(reading::Request::Service {
            document: reader.field(Reader::u32)?,
            request: reader.service(kind)?,
        }),
        RequestKind::Hello => Request::Hello(reader.field(Reader::u32)?),
        RequestKind::Open => reader.open(limits)?,
        RequestKind::PageInfo => Request::PageInfo(reader.page()?),
        RequestKind::RenderTile => Request::RenderTile(reader.tile()?),
        RequestKind::Diagnostics => Request::Diagnostics(reader.field(Reader::u32)?),
        RequestKind::Close => Request::Close(reader.field(Reader::u32)?),
        RequestKind::Generation => Request::Generation {
            document: reader.field(Reader::u32)?,
            generation: reader.field(Reader::u32)?,
        },
        RequestKind::GenerationTile => Request::GenerationTile {
            generation: reader.field(Reader::u32)?,
            tile: reader.tile()?,
        },
        RequestKind::Stats => Request::Stats,
        RequestKind::PageText => Request::Reading(reading::Request::Text(reader.page()?)),
        RequestKind::Search => Request::Reading(reading::Request::Search(reader.search(limits)?)),
        RequestKind::Links => Request::Reading(reading::Request::Links(reader.page()?)),
        RequestKind::Outlines => {
            Request::Reading(reading::Request::Outlines(reader.field(Reader::u32)?))
        }
        RequestKind::Labels => {
            Request::Reading(reading::Request::Labels(reader.field(Reader::u32)?))
        }
        RequestKind::ReadingStats => Request::Reading(reading::Request::Stats),
        RequestKind::Bounds => Request::Reading(reading::Request::Bounds(reader.page()?)),
        RequestKind::DiagnosticDetails => Request::DiagnosticDetails(reader.field(Reader::u32)?),
        RequestKind::FormFields => Request::FormFields(reader.field(Reader::u32)?),
        RequestKind::SaveForm => Request::SaveForm {
            document: reader.field(Reader::u32)?,
            changes: reader.blob(limits.document_bytes.min(editing::MAX_BYTES))?,
        },
        RequestKind::FormWidgets => Request::FormWidgets(reader.page()?),
        RequestKind::FormStyles => Request::FormStyles(reader.page()?),
        RequestKind::PagePng => Request::PagePng(PngRequest {
            page: reader.page()?,
            scale: reader.field(Reader::f64)?,
            background: reader.field(Reader::code)?,
        }),
        RequestKind::Metadata => {
            Request::Reading(reading::Request::Metadata(reader.field(Reader::u32)?))
        }
        RequestKind::Attachments => {
            Request::Reading(reading::Request::Attachments(reader.field(Reader::u32)?))
        }
        RequestKind::MetadataBytes => {
            Request::Reading(reading::Request::MetadataBytes(reader.field(Reader::u32)?))
        }
        RequestKind::AttachmentCheck => Request::Reading(reading::Request::AttachmentCheck {
            document: reader.field(Reader::u32)?,
            index: reader.field(Reader::u32)?,
            key: reader.field(Reader::code)?,
        }),
        RequestKind::AttachmentBytes => Request::Reading(reading::Request::AttachmentBytes {
            document: reader.field(Reader::u32)?,
            index: reader.field(Reader::u32)?,
            key: reader.field(Reader::code)?,
        }),
        RequestKind::PageAttachments => {
            Request::Reading(reading::Request::PageAttachments(reader.page()?))
        }
        RequestKind::PageAttachmentBytes => {
            Request::Reading(reading::Request::PageAttachmentBytes {
                page: reader.page()?,
                index: reader.field(Reader::u32)?,
                key: reader.field(Reader::code)?,
            })
        }
        RequestKind::PageAttachmentCheck => {
            Request::Reading(reading::Request::PageAttachmentCheck {
                page: reader.page()?,
                index: reader.field(Reader::u32)?,
                key: reader.field(Reader::code)?,
            })
        }
    };
    reader.finish()?;
    Ok(RequestFrame { id, request })
}

/// A little-endian reader over one request body, or over a nested payload
/// such as a form change batch. Positions are offsets into the bytes it
/// reads. (`pagura_core::bytes::Cursor` reads big-endian PDF-embedded
/// formats and is not part of the facade.)
pub(crate) struct Reader<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> Reader<'a> {
    pub(crate) const fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, offset: 0 }
    }

    /// Fails unless every byte has been read.
    pub(crate) fn finish(&self) -> Result<()> {
        if self.rest().is_empty() {
            Ok(())
        } else {
            Err(self.error("trailing request payload"))
        }
    }

    fn rest(&self) -> &'a [u8] {
        self.bytes.get(self.offset..).unwrap_or_default()
    }

    fn error(&self, message: &'static str) -> Error {
        Error::syntax(message).at(Position::Buffer(self.offset))
    }

    fn take(&mut self, count: usize) -> Result<&'a [u8]> {
        let (bytes, _) = self
            .rest()
            .split_at_checked(count)
            .ok_or_else(|| self.error("truncated request payload"))?;
        self.offset += count;
        Ok(bytes)
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N]> {
        let (bytes, _) = self
            .rest()
            .split_first_chunk::<N>()
            .ok_or_else(|| self.error("truncated request payload"))?;
        self.offset += N;
        Ok(*bytes)
    }

    pub(crate) fn u8(&mut self) -> Result<u8> {
        self.array().map(u8::from_le_bytes)
    }

    pub(crate) fn u32(&mut self) -> Result<u32> {
        self.array().map(u32::from_le_bytes)
    }

    fn u64(&mut self) -> Result<u64> {
        self.array().map(u64::from_le_bytes)
    }

    fn f64(&mut self) -> Result<f64> {
        self.array().map(f64::from_le_bytes)
    }

    /// A value and the offset it starts at.
    fn field<T: Copy>(&mut self, read: fn(&mut Self) -> Result<T>) -> Result<Field<T>> {
        let offset = self.offset;
        Ok(Field::new(read(self)?, offset))
    }

    /// A wire code; an unassigned one is reported at its byte.
    fn code<C: TryFrom<u8, Error = Error>>(&mut self) -> Result<C> {
        let position = Position::Buffer(self.offset);
        C::try_from(self.u8()?).map_err(|error| error.at(position))
    }

    /// `u32 length | bytes`, at most `cap` bytes. The field's offset is that
    /// of its bytes; a length over `cap` is reported at the length.
    pub(crate) fn blob(&mut self, cap: usize) -> Result<Field<&'a [u8]>> {
        let position = Position::Buffer(self.offset);
        let count = usize::try_from(self.u32()?)
            .map_err(|_| Error::limit("byte string length overflows").at(position))?;
        if count > cap {
            return Err(Error::limit("byte string budget exceeded").at(position));
        }
        let offset = self.offset;
        Ok(Field::new(self.take(count)?, offset))
    }

    fn object(&mut self) -> Result<pagura::ObjectId> {
        Ok(pagura::ObjectId::new(
            self.u32()?,
            u16::from_le_bytes(self.array()?),
        ))
    }

    fn index(&mut self) -> Result<usize> {
        self.field(Self::u32)?.index()
    }

    fn owner(&mut self) -> Result<pagura::metadata::MetadataOwner> {
        use super::MetadataOwnerCode;
        use pagura::metadata::MetadataOwner;
        Ok(match self.code()? {
            MetadataOwnerCode::Document => MetadataOwner::Document,
            MetadataOwnerCode::Page => MetadataOwner::Page(self.index()?),
            MetadataOwnerCode::Object => MetadataOwner::Object(self.object()?),
        })
    }

    fn file_scope(&mut self) -> Result<pagura::files::FileScope> {
        use super::FileScopeCode;
        use pagura::files::FileScope;
        Ok(match self.code()? {
            FileScopeCode::Catalog => FileScope::Catalog,
            FileScopeCode::Associated => FileScope::Associated,
            FileScopeCode::PageAssociated => FileScope::PageAssociated(self.index()?),
            FileScopeCode::ObjectAssociated => FileScope::ObjectAssociated(self.object()?),
            FileScopeCode::PageAnnotations => FileScope::PageAnnotations(self.index()?),
        })
    }

    fn file_selection(&mut self) -> Result<reading::FileSelection> {
        let scope = self.file_scope()?;
        let index = self.index()?;
        let key: super::AttachmentKey = self.code()?;
        let payload = match self.code()? {
            super::PayloadKind::Embedded => pagura::files::FilePayload::Embedded(key.pdf_name()),
            super::PayloadKind::Related => pagura::files::FilePayload::Related {
                key: key.pdf_name(),
                index: self.index()?,
            },
        };
        Ok(reading::FileSelection {
            scope,
            index,
            payload,
        })
    }

    fn service(&mut self, kind: RequestKind) -> Result<reading::ServiceRequest> {
        use reading::ServiceRequest;
        Ok(match kind {
            RequestKind::Files => ServiceRequest::Files(self.file_scope()?),
            RequestKind::FileBytes => ServiceRequest::FileBytes(self.file_selection()?),
            RequestKind::FileCheck => ServiceRequest::FileCheck(self.file_selection()?),
            RequestKind::ObjectMetadata => ServiceRequest::Metadata(self.owner()?),
            RequestKind::ObjectMetadataBytes => ServiceRequest::MetadataBytes(self.owner()?),
            RequestKind::Collection => ServiceRequest::Collection,
            RequestKind::Images => ServiceRequest::Images(self.owner()?),
            RequestKind::Thumbnail => ServiceRequest::Thumbnail(self.owner()?),
            RequestKind::ImageBytes => ServiceRequest::ImageBytes {
                owner: self.owner()?,
                thumbnail: self.code::<super::ImageAssetCode>()?
                    == super::ImageAssetCode::Thumbnail,
                mode: match self.code()? {
                    super::ImageTransferCode::Stored => pagura::images::ImageTransferMode::Stored,
                    super::ImageTransferCode::Encoded => pagura::images::ImageTransferMode::Encoded,
                },
            },
            _ => return Err(self.error("not an extended service request")),
        })
    }

    fn open(&mut self, limits: &Limits) -> Result<Request<'a>> {
        let flags = self.field(Self::u8)?;
        let flags = OpenFlags::from_bits(flags.get())
            .ok_or_else(|| Error::syntax("unknown recovery mode").at(flags.position()))?;
        let password = self.blob(limits.string_bytes)?.get();
        let bytes = self.blob(limits.file_bytes)?.get();
        Ok(Request::Open {
            bytes,
            recovery: flags.recovery(),
            password,
        })
    }

    fn page(&mut self) -> Result<PageRef> {
        Ok(PageRef::new(self.field(Self::u32)?, self.field(Self::u32)?))
    }

    fn tile(&mut self) -> Result<TileRequest> {
        Ok(TileRequest {
            page: self.page()?,
            scale: self.field(Self::f64)?,
            tile: self.field(|reader| {
                Ok(Tile {
                    x: reader.u32()?,
                    y: reader.u32()?,
                    width: reader.u32()?,
                    height: reader.u32()?,
                })
            })?,
        })
    }

    fn search(&mut self, limits: &Limits) -> Result<Search<'a>> {
        let document = self.field(Self::u32)?;
        let start = self.field(Self::u32)?;
        let end = self.field(Self::u32)?;
        let offset = self.field(Self::u32)?;
        let limit = self.field(Self::u32)?;
        let flags = self.field(Self::u8)?;
        let query = self.blob(limits.string_bytes.min(MAX_QUERY_BYTES))?;
        let text = std::str::from_utf8(query.get())
            .map_err(|_| Error::syntax("search query is not UTF-8").at(query.position()))?;
        Ok(Search {
            document,
            start,
            end,
            offset,
            limit,
            flags,
            query: Field::new(text, query.offset()),
        })
    }
}

/// The offsets the decoder records match PROTOCOL.md's tables.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::layout;
    use pagura_testkit::frame;

    fn body(kind: RequestKind, payload: &[u8]) -> Vec<u8> {
        frame(kind.code(), 1, payload)[layout::LENGTH_BYTES..].to_vec()
    }

    #[test]
    fn fields_carry_their_documented_body_offsets() {
        let mut tile = vec![0; 8];
        tile.extend(2_f64.to_le_bytes());
        tile.extend([0; 16]);
        let render = body(RequestKind::RenderTile, &tile);
        let Request::RenderTile(request) = decode(&render, &Limits::default()).unwrap().request
        else {
            panic!("render request");
        };
        assert_eq!(
            [
                request.page.document.offset(),
                request.page.page.offset(),
                request.scale.offset(),
                request.tile.offset()
            ],
            [5, 9, 13, 21]
        );
        let mut generated = vec![0; 4];
        generated.extend(&tile);
        let generated = body(RequestKind::GenerationTile, &generated);
        let Request::GenerationTile { generation, tile } =
            decode(&generated, &Limits::default()).unwrap().request
        else {
            panic!("generation tile");
        };
        assert_eq!(
            [
                generation.offset(),
                tile.page.document.offset(),
                tile.scale.offset()
            ],
            [5, 9, 17]
        );
        let mut search = vec![0; 20];
        search.push(0);
        search.extend(1_u32.to_le_bytes());
        search.push(b'q');
        let search = body(RequestKind::Search, &search);
        let Request::Reading(reading::Request::Search(search)) =
            decode(&search, &Limits::default()).unwrap().request
        else {
            panic!("search");
        };
        assert_eq!(
            [
                search.document.offset(),
                search.start.offset(),
                search.end.offset(),
                search.offset.offset(),
                search.limit.offset(),
                search.flags.offset(),
                search.query.offset()
            ],
            [5, 9, 13, 17, 21, 25, 30]
        );
        assert_eq!(search.query.get(), "q");
    }

    #[test]
    fn every_kind_decodes_exhaustively_and_unassigned_kinds_are_terminal() {
        for byte in 0..=u8::MAX {
            let body = [byte, 1, 0, 0, 0];
            let decoded = decode(&body, &Limits::default());
            if let Ok(kind) = RequestKind::try_from(byte) {
                // Payload-free kinds decode; the rest are truncated.
                if let Ok(frame) = decoded {
                    assert_eq!(frame.request.kind(), kind);
                }
            } else {
                let error = decoded.unwrap_err();
                assert_eq!(error.position, Position::Buffer(layout::KIND_OFFSET));
            }
        }
    }
}
