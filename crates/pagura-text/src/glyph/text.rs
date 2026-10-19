//! Unicode text of one glyph, cheap to clone along the decoding hot path.
use std::{fmt, ops::Deref, sync::Arc};

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
        let long = "b".repeat(40);
        for text in ["", "A", "😀", "ﬃ", long.as_str()] {
            let value = GlyphText::from(text);
            assert_eq!(value, text);
            assert_eq!(value.heap_bytes(), text.len(), "{text}");
            assert_eq!(GlyphText::from(text.to_owned()), value);
            let clone = value.clone();
            assert!(std::ptr::eq(clone.as_str(), value.as_str()));
        }
        assert_eq!(GlyphText::from('Ω').to_string(), "Ω");
    }
}
