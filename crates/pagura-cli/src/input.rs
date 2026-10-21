//! Reading input files within the file budget, opening them, and creating
//! outputs without overwriting.

use crate::args::Source;
use crate::failure::Failure;
use crate::password;
use pagura::{Document, Error, Limits, OpenOptions, Recovery, convert};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::sync::Arc;

impl Source {
    /// The options that open this source with `recovery`.
    pub(crate) fn options(&self, recovery: Recovery) -> OpenOptions {
        OpenOptions {
            recovery,
            password: self.password.clone(),
            ..OpenOptions::default()
        }
    }

    /// Reads the file within the default file budget.
    ///
    /// # Errors
    /// A `Limit` failure for a file over the budget, also when it grows
    /// while being read, and I/O failures.
    pub(crate) fn load(&self) -> Result<Arc<[u8]>, Failure> {
        load(&self.path, &OpenOptions::default().limits)
    }

    /// Reads and opens the document with the source's own recovery mode.
    pub(crate) fn open(&self) -> Result<Document, Failure> {
        Ok(self.open_bytes(self.load()?, self.recovery)?)
    }

    /// Opens already loaded bytes with `recovery`, encoding a Unicode
    /// password as the document's security handler requires.
    pub(crate) fn open_bytes(
        &self,
        bytes: Arc<[u8]>,
        recovery: Recovery,
    ) -> pagura::Result<Document> {
        password::open(bytes, &self.options(recovery))
    }
}

/// Reads `path`, refusing more than `limits.file_bytes`, including bytes
/// appended after the size check. A regular file is read once into storage
/// of its length; anything else, or a file whose length changes while it is
/// read, is read again to its end.
fn load(path: &Path, limits: &Limits) -> Result<Arc<[u8]>, Failure> {
    let mut file = File::open(path)?;
    let too_large = || Failure::Engine(Error::limit("input exceeds the file size budget"));
    let limit = convert::usize_to_u64(limits.file_bytes);
    let metadata = file.metadata()?;
    if metadata.len() > limit {
        return Err(too_large());
    }
    if metadata.is_file() {
        let length = usize::try_from(metadata.len()).map_err(|_| too_large())?;
        let mut bytes: Arc<[u8]> = std::iter::repeat_n(0, length).collect();
        if let Some(buffer) = Arc::get_mut(&mut bytes) {
            match file.read_exact(buffer) {
                Ok(()) if file.read(&mut [0])? == 0 => return Ok(bytes),
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => {}
                Err(error) => return Err(error.into()),
            }
        }
        file.seek(SeekFrom::Start(0))?;
    }
    let mut data = Vec::new();
    file.take(limit.saturating_add(1)).read_to_end(&mut data)?;
    if data.len() > limits.file_bytes {
        return Err(too_large());
    }
    Ok(data.into())
}

/// Creates `path` and writes `bytes`; never truncates an existing file.
pub(crate) fn write_new(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut file = File::options().write(true).create_new(true).open(path)?;
    file.write_all(bytes)
}
