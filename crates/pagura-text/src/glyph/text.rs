//! Unicode text of one glyph, cheap to clone along the decoding hot path.
use std::{fmt, ops::Deref, sync::Arc};

/// Text up to this many bytes is collected on the stack before it is shared.
const STACK: usize = 32;

/// The Unicode text of one glyph (9.10.2), from `/ToUnicode`, glyph names or
/// a character collection. The text is validated once, when it is created,
/// and shared: cloning never copies it and dereferencing to `str` is free.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct GlyphText(Arc<str>);

impl GlyphText {
    /// The text as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Collects `chars` with one allocation for the shared text.
    pub(crate) fn from_chars(chars: impl IntoIterator<Item = char>) -> Self {
        let mut bytes = [0; STACK];
        let mut len = 0;
        let mut chars = chars.into_iter();
        while let Some(scalar) = chars.next() {
            let end = len + scalar.len_utf8();
            let Some(slot) = bytes.get_mut(len..end) else {
                let mut text = String::from(std::str::from_utf8(&bytes[..len]).unwrap_or_default());
                text.push(scalar);
                text.extend(chars);
                return Self::from(text);
            };
            scalar.encode_utf8(slot);
            len = end;
        }
        Self::from(std::str::from_utf8(&bytes[..len]).unwrap_or_default())
    }

    /// Bytes charged for this text: its length, as for an owned string.
    /// Clones share one allocation, so each holder is charged once for the
    /// text it refers to.
    pub fn heap_bytes(&self) -> usize {
        self.0.len()
    }
}

impl From<&str> for GlyphText {
    fn from(text: &str) -> Self {
        Self(Arc::from(text))
    }
}

impl From<String> for GlyphText {
    fn from(text: String) -> Self {
        Self(Arc::from(text))
    }
}

impl From<char> for GlyphText {
    fn from(scalar: char) -> Self {
        Self::from(scalar.encode_utf8(&mut [0; 4]) as &str)
    }
}

impl Deref for GlyphText {
    type Target = str;

    fn deref(&self) -> &str {
        self.as_str()
    }
}

impl AsRef<str> for GlyphText {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl PartialEq<str> for GlyphText {
    fn eq(&self, other: &str) -> bool {
        self.as_str() == other
    }
}

impl PartialEq<&str> for GlyphText {
    fn eq(&self, other: &&str) -> bool {
        self.as_str() == *other
    }
}

impl fmt::Debug for GlyphText {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self.as_str(), f)
    }
}

impl fmt::Display for GlyphText {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_is_shared_and_charged_by_length() {
        let long = "b".repeat(STACK + 1);
        for text in ["", "A", "😀", "ﬃ", long.as_str()] {
            let value = GlyphText::from(text);
            assert_eq!(value, text);
            assert_eq!(value.heap_bytes(), text.len(), "{text}");
            assert_eq!(GlyphText::from(text.to_owned()), value);
            let clone = value.clone();
            assert!(std::ptr::eq(clone.as_str(), value.as_str()));
        }
        assert_eq!(GlyphText::from('Ω').to_string(), "Ω");
        for text in ["", "AB😀", long.as_str(), "😀".repeat(9).as_str()] {
            assert_eq!(GlyphText::from_chars(text.chars()), text);
        }
    }
}
