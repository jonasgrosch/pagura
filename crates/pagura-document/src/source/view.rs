//! Admitted complete backing and historical prefixes of the same source.

use super::{ByteLease, ByteSource};
use pagura_core::{Error, Result};
use std::{ops::Deref, sync::Arc};

#[derive(Debug, Clone)]
enum Backing {
    /// Complete-buffer convenience callers have document-local identity.
    Local(Arc<[u8]>),
    /// Explicit source admission retains its lease and consistency provider.
    Registered {
        source: Arc<dyn ByteSource>,
        lease: ByteLease,
    },
}

/// Every byte in this prefix has already been admitted. Dereferencing this
/// owner cannot trigger source I/O, hide a missing range or allocate a copy.
#[derive(Debug, Clone)]
pub(crate) struct View {
    backing: Backing,
    end: usize,
}

impl View {
    pub(crate) fn admitted(source: Arc<dyn ByteSource>, lease: ByteLease) -> Result<Self> {
        source.check()?;
        if lease.range().start() != 0 || lease.range().end() != source.len() {
            return Err(Error::invalid_argument(
                "materialized lease does not cover its source",
            ));
        }
        let end = lease.bytes().len();
        Ok(Self {
            backing: Backing::Registered { source, lease },
            end,
        })
    }

    /// Source closure/conflict takes precedence over already-cached reads.
    pub(crate) fn check(&self) -> Result<()> {
        match &self.backing {
            Backing::Local(_) => Ok(()),
            Backing::Registered { source, .. } => source.check(),
        }
    }

    pub(crate) fn prefix(&self, end: usize) -> Result<Self> {
        self.check()?;
        if end > self.end {
            return Err(Error::syntax("revision prefix lies outside its source"));
        }
        Ok(Self {
            backing: self.backing.clone(),
            end,
        })
    }
}

impl Deref for View {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        let bytes: &[u8] = match &self.backing {
            Backing::Local(bytes) => bytes,
            Backing::Registered { lease, .. } => lease.bytes(),
        };
        &bytes[..self.end]
    }
}

impl From<Arc<[u8]>> for View {
    fn from(bytes: Arc<[u8]>) -> Self {
        let end = bytes.len();
        Self {
            backing: Backing::Local(bytes),
            end,
        }
    }
}

impl From<Vec<u8>> for View {
    fn from(bytes: Vec<u8>) -> Self {
        Self::from(Arc::<[u8]>::from(bytes))
    }
}
