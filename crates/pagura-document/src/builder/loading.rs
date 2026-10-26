//! Persistent security/catalog construction over the authoritative object table.

use super::DocumentBuilder;
use crate::source::{ByteSource, NeedData, ReadFault, ReadResult};
use crate::xref::{XrefOrigin, rebuild};
use crate::{Document, catalog, security};
use pagura_core::{Error, ObjectId};

#[derive(Debug)]
enum Stage {
    Security(XrefOrigin),
    Streams {
        streams: Vec<ObjectId>,
        remaining: usize,
    },
    Root,
    Catalog,
}

impl From<XrefOrigin> for Stage {
    fn from(origin: XrefOrigin) -> Self {
        match origin {
            XrefOrigin::Primary => Self::Catalog,
            XrefOrigin::Rebuilt { streams, remaining } => Self::Streams { streams, remaining },
        }
    }
}

#[derive(Debug)]
struct Active {
    document: Document,
    stage: Stage,
    mark: usize,
    waiting: Option<NeedData>,
}

#[derive(Debug)]
enum State {
    Active(Box<Active>),
    Complete,
    Failed(Error),
}

/// Owns unpublished state; a wait cannot discard completed dependencies or
/// authenticate the same scratch cache under a different security handler.
#[derive(Debug)]
pub(crate) struct Build {
    state: State,
}

impl Build {
    pub(crate) fn new(mut builder: DocumentBuilder) -> Self {
        let origin = std::mem::take(&mut builder.origin);
        let stage = if builder
            .trailer
            .get(b"Encrypt")
            .is_some_and(|value| !value.is_null())
        {
            Stage::Security(origin)
        } else {
            Stage::from(origin)
        };
        let mark = builder.retained.used();
        Self {
            state: State::Active(Box::new(Active {
                document: builder.freeze(),
                stage,
                mark,
                waiting: None,
            })),
        }
    }

    /// Advances one security/table/catalog unit; delivers the document once.
    /// Terminal errors discard scratch storage; ordinary waits retain it.
    pub(crate) fn advance(&mut self, password: &[u8]) -> ReadResult<Option<Document>> {
        let active = match &mut self.state {
            State::Active(active) => active,
            State::Complete => return Ok(None),
            State::Failed(error) => return Err(error.clone().into()),
        };
        match active.advance(password) {
            Ok(false) => Ok(None),
            Ok(true) => {
                let previous = std::mem::replace(&mut self.state, State::Complete);
                debug_assert!(matches!(previous, State::Active(_)));
                if let State::Active(active) = previous {
                    Ok(Some(active.document))
                } else {
                    Ok(None)
                }
            }
            Err(ReadFault::Invalid(error)) => {
                self.state = State::Failed(error.clone());
                Err(error.into())
            }
            Err(ReadFault::Unavailable(missing)) => {
                active.waiting = Some(missing.clone());
                Err(ReadFault::Unavailable(missing))
            }
        }
    }
}

impl Active {
    fn advance(&mut self, password: &[u8]) -> ReadResult<bool> {
        self.document.bytes.check()?;
        if let Some(missing) = &self.waiting
            && missing.generation() == self.document.bytes.generation()
        {
            return Err(ReadFault::Unavailable(missing.clone()));
        }
        self.waiting = None;
        match &mut self.stage {
            Stage::Security(origin) => {
                let security = security::read(&self.document, password)?;
                self.document.reset_construction_caches(self.mark);
                self.document.security = security;
                self.stage = Stage::from(std::mem::take(origin));
            }
            Stage::Streams { streams, remaining } => {
                let plan = rebuild::object_streams(&self.document, streams, *remaining)?;
                self.document.reset_construction_caches(self.mark);
                self.document.apply(plan)?;
                self.mark = self.document.retained.used();
                self.stage = Stage::Root;
            }
            Stage::Root => {
                let root = rebuild::root(&self.document)?;
                self.document.reset_construction_caches(self.mark);
                if let Some(root) = root {
                    self.document.set_root(root)?;
                }
                self.mark = self.document.retained.used();
                self.stage = Stage::Catalog;
            }
            Stage::Catalog => {
                catalog::validate(&self.document)?;
                self.document.bytes.check()?;
                return Ok(true);
            }
        }
        Ok(false)
    }
}

#[cfg(test)]
mod tests;
