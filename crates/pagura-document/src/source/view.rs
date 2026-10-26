//! Admitted complete backing and historical prefixes of the same source.

use super::{ByteLease, ByteRange, ByteSource, ReadResult};
use pagura_core::convert::usize_to_u64;
use pagura_core::{Error, Limits, Result};
use std::{ops::Deref, sync::Arc};

#[derive(Debug, Clone)]
enum Backing {
    /// Complete-buffer convenience callers have document-local identity.
    Local(ByteLease),
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
    pub(crate) fn local(bytes: Arc<[u8]>, limits: &Limits) -> Result<Self> {
        let (lease, _) = super::complete::admit(bytes, limits, None)?;
        let end = lease.bytes().len();
        Ok(Self {
            backing: Backing::Local(lease),
            end,
        })
    }

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

    pub(crate) fn materialize(&self) -> ReadResult<ByteLease> {
        self.check()?;
        let lease = match &self.backing {
            Backing::Local(lease) | Backing::Registered { lease, .. } => lease,
        };
        Ok(lease.slice(ByteRange::whole(usize_to_u64(self.end)))?)
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
            Backing::Local(lease) | Backing::Registered { lease, .. } => lease.bytes(),
        };
        &bytes[..self.end]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn materialized_historical_prefix_keeps_absolute_extent_and_shared_storage() {
        let original: Arc<[u8]> = Arc::from(&b"older revision\nnewer revision\n"[..]);
        let view = View::local(Arc::clone(&original), &Limits::default()).unwrap();
        let prefix = view.prefix(15).unwrap();
        let lease = prefix.materialize().unwrap();
        assert_eq!(lease.bytes(), b"older revision\n");
        assert_eq!(lease.range(), ByteRange::new(0, 15).unwrap());
        assert_eq!(lease.bytes().as_ptr(), original.as_ptr());
        assert!(prefix.prefix(16).is_err());
        drop(view);
        drop(prefix);
        assert_eq!(lease.bytes(), b"older revision\n");
    }
}
