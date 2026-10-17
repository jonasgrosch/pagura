//! PDF name objects (ISO 32000-2 clause 7.3.5).

use std::borrow::Borrow;
use std::cmp::Ordering;
use std::fmt;
use std::hash::{Hash, Hasher};

/// Names up to this length are stored inline, without a heap allocation.
/// Almost every name in real documents (`Type`, `Font`, `FlateDecode`) fits.
const INLINE_CAPACITY: usize = 23;

/// A PDF name: an arbitrary byte sequence without NUL, not necessarily UTF-8.
///
/// Escapes such as `#20` are already decoded. Equality, ordering and hashing
/// use the decoded bytes, so a `Name` can be looked up by `&[u8]`.
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
    /// Creates a name from decoded bytes.
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

impl From<&[u8]> for Name {
    fn from(bytes: &[u8]) -> Self {
        Self::new(bytes)
    }
}

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
    fn display_escapes_delimiters_and_non_graphic_bytes() {
        assert_eq!(Name::new(b"A B#\xff").to_string(), "/A#20B#23#FF");
    }
}
