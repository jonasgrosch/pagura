//! PDF name objects (ISO 32000-2 clause 7.3.5).

use crate::{Error, Result};
use std::borrow::Borrow;
use std::cmp::Ordering;
use std::fmt;
use std::hash::{Hash, Hasher};

/// Names up to this length are stored inline, without a heap allocation.
/// Almost every name in real documents (`Type`, `Font`, `FlateDecode`) fits.
const INLINE_CAPACITY: usize = 23;

/// A PDF name: a byte sequence, not necessarily UTF-8.
///
/// Escapes such as `#20` are already decoded. Equality, ordering and hashing
/// use the decoded bytes, so a `Name` can be looked up by `&[u8]`.
///
/// Clause 7.3.5 forbids the NUL byte (0x00) in a name. The type does not
/// enforce that: [`Name::new`] accepts any bytes, for names that code spells
/// out such as `b"Type"`. Bytes from any other source go through
/// [`Name::try_new`] (or `TryFrom<&[u8]>`), which rejects NUL. The parser
/// rejects NUL in names it reads, and the writer refuses to serialize one.
#[derive(Clone)]
pub struct Name(Repr);

#[derive(Clone)]
enum Repr {
    Inline {
        len: u8,
        bytes: [u8; INLINE_CAPACITY],
    },
    Heap(Box<[u8]>),
}

impl Name {
    /// Creates a name from decoded bytes without checking them; use it for
    /// names written in code. Untrusted bytes go through [`Name::try_new`].
    pub fn new(bytes: &[u8]) -> Self {
        match u8::try_from(bytes.len()) {
            Ok(len) if bytes.len() <= INLINE_CAPACITY => {
                let mut inline = [0; INLINE_CAPACITY];
                inline[..bytes.len()].copy_from_slice(bytes);
                Self(Repr::Inline { len, bytes: inline })
            }
            _ => Self(Repr::Heap(bytes.into())),
        }
    }

    /// Creates a name from decoded bytes, enforcing clause 7.3.5.
    ///
    /// # Errors
    ///
    /// Returns an invalid-argument error if `bytes` contains NUL.
    pub fn try_new(bytes: &[u8]) -> Result<Self> {
        if bytes.contains(&0) {
            return Err(Error::invalid_argument("name contains a NUL byte"));
        }
        Ok(Self::new(bytes))
    }

    pub(crate) fn heap_bytes(&self) -> usize {
        match &self.0 {
            Repr::Heap(bytes) => bytes.len(),
            Repr::Inline { .. } => 0,
        }
    }

    /// The decoded bytes of the name, without the leading solidus.
    pub fn as_bytes(&self) -> &[u8] {
        match &self.0 {
            Repr::Inline { len, bytes } => &bytes[..usize::from(*len)],
            Repr::Heap(bytes) => bytes,
        }
    }

    /// Number of decoded bytes.
    pub fn len(&self) -> usize {
        self.as_bytes().len()
    }

    /// Whether this is the empty name `/`.
    pub fn is_empty(&self) -> bool {
        self.as_bytes().is_empty()
    }
}

/// Checks the bytes like [`Name::try_new`].
impl TryFrom<&[u8]> for Name {
    type Error = Error;

    fn try_from(bytes: &[u8]) -> Result<Self> {
        Self::try_new(bytes)
    }
}

/// For names written in code, like [`Name::new`].
impl<const N: usize> From<&[u8; N]> for Name {
    fn from(bytes: &[u8; N]) -> Self {
        Self::new(bytes)
    }
}

impl AsRef<[u8]> for Name {
    fn as_ref(&self) -> &[u8] {
        self.as_bytes()
    }
}

impl Borrow<[u8]> for Name {
    fn borrow(&self) -> &[u8] {
        self.as_bytes()
    }
}

impl PartialEq for Name {
    fn eq(&self, other: &Self) -> bool {
        self.as_bytes() == other.as_bytes()
    }
}

impl Eq for Name {}

impl PartialEq<[u8]> for Name {
    fn eq(&self, other: &[u8]) -> bool {
        self.as_bytes() == other
    }
}

impl<const N: usize> PartialEq<[u8; N]> for Name {
    fn eq(&self, other: &[u8; N]) -> bool {
        self.as_bytes() == other
    }
}

impl PartialOrd for Name {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Name {
    fn cmp(&self, other: &Self) -> Ordering {
        self.as_bytes().cmp(other.as_bytes())
    }
}

impl Hash for Name {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.as_bytes().hash(state);
    }
}

impl fmt::Debug for Name {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

/// Formats the name in PDF syntax, escaping bytes outside the regular set.
impl fmt::Display for Name {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("/")?;
        for &byte in self.as_bytes() {
            if byte.is_ascii_graphic() && !b"()<>[]{}/%#".contains(&byte) {
                write!(f, "{}", char::from(byte))?;
            } else {
                write!(f, "#{byte:02X}")?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::Name;

    #[test]
    fn short_and_long_names_compare_by_bytes() {
        let short = Name::new(b"Type");
        let long = Name::new(&[b'x'; 40]);
        assert_eq!(short, *b"Type");
        assert_eq!(long.len(), 40);
        assert_eq!(Name::new(&[b'x'; 40]), long);
        assert!(short < long);
    }

    #[test]
    fn checked_construction_rejects_nul_only() {
        assert_eq!(Name::try_new(b"A B").unwrap(), *b"A B");
        assert_eq!(Name::try_from([b'x'; 40].as_slice()).unwrap().len(), 40);
        for bad in [b"\0".as_slice(), b"A\0B", &[b'x'; 40][..39], b""] {
            let checked = Name::try_new(bad);
            assert_eq!(checked.is_err(), bad.contains(&0), "{bad:?}");
        }
        assert_eq!(
            Name::try_new(b"\0").unwrap_err().kind,
            crate::ErrorKind::InvalidArgument
        );
    }

    #[test]
    fn display_escapes_delimiters_and_non_graphic_bytes() {
        assert_eq!(Name::new(b"A B#\xff").to_string(), "/A#20B#23#FF");
    }
}
