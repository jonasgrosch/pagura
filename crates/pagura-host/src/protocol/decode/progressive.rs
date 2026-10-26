//! Version-two layouts, read by the existing request cursor.

use super::Reader;
use crate::progressive::Request;
use crate::protocol::{OpenFlags, RequestKind, ValidationCode};
use pagura::{Error, Limits, Position, Result, Tile, Validation};

impl<'a> Reader<'a> {
    pub(super) fn progressive(
        &mut self,
        kind: RequestKind,
        limits: &Limits,
    ) -> Result<Request<'a>> {
        Ok(match kind {
            RequestKind::SourceCreate => Request::SourceCreate {
                length: self.field(Self::u64)?,
                version: self.field(Self::array::<32>)?,
            },
            RequestKind::SourceSupply => Request::SourceSupply {
                source: self.field(Self::u32)?,
                version: self.field(Self::array::<32>)?,
                offset: self.field(Self::u64)?,
                bytes: self.blob(limits.file_bytes)?,
            },
            RequestKind::SourceFinish => Request::SourceFinish(self.field(Self::u32)?),
            RequestKind::SourceClose => Request::SourceClose(self.field(Self::u32)?),
            RequestKind::OpenJob => {
                let source = self.field(Self::u32)?;
                let validation = match self.code()? {
                    ValidationCode::CompleteDocument => Validation::CompleteDocument,
                    ValidationCode::RequestedOperation => Validation::RequestedOperation,
                };
                let flags = self.field(Self::u8)?;
                let recovery = OpenFlags::from_bits(flags.get())
                    .ok_or_else(|| Error::syntax("unknown recovery mode").at(flags.position()))?
                    .recovery();
                let password = self.blob(limits.string_bytes)?.get();
                Request::Open {
                    source,
                    validation,
                    recovery,
                    password,
                }
            }
            RequestKind::PageJob => Request::Page(self.page()?),
            RequestKind::RenderJob => Request::Render {
                page: self.page()?,
                scale: self.field(Self::f64)?,
                background: self.field(Self::code)?,
                output: self.field(Self::code)?,
                tile: if self.flag()? {
                    Some(self.field(|reader| {
                        Ok(Tile {
                            x: reader.u32()?,
                            y: reader.u32()?,
                            width: reader.u32()?,
                            height: reader.u32()?,
                        })
                    })?)
                } else {
                    None
                },
            },
            RequestKind::JobAdvance => Request::Advance(self.field(Self::u32)?),
            RequestKind::JobAcknowledge => Request::Acknowledge {
                job: self.field(Self::u32)?,
                sequence: self.field(Self::u64)?,
            },
            RequestKind::JobCancel => Request::Cancel(self.field(Self::u32)?),
            RequestKind::JobClose => Request::Close(self.field(Self::u32)?),
            RequestKind::JobStats => Request::Stats,
            _ => {
                return Err(Error::invalid_argument(
                    "request is not a progressive operation",
                ));
            }
        })
    }

    fn flag(&mut self) -> Result<bool> {
        let position = Position::Buffer(self.offset);
        match self.u8()? {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(Error::syntax("noncanonical optional field flag").at(position)),
        }
    }
}
