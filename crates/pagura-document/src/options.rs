//! How a document is opened: budgets, recovery and the password.

use crate::features;
use pagura_core::{Error, Limits, Recovery, Result, text_string};
use std::fmt;

/// How to open a document.
///
/// A plain record built with functional record update, as [`Limits`] is:
/// `OpenOptions { recovery: Recovery::Strict, ..OpenOptions::default() }`.
/// The document keeps the limits and recovery mode, never the password.
#[derive(Debug, Clone, Default)]
pub struct OpenOptions {
    /// Resource budgets for every operation on the document.
    pub limits: Limits,
    /// Whether malformed input may be repaired (reported) or is an error.
    pub recovery: Recovery,
    /// Password for encrypted documents; the default is the empty password.
    pub password: Password,
}

/// A password for an encrypted document (7.6.4.3).
///
/// Text is encoded for the document's Standard security handler once its
/// revision is known: PDFDocEncoding for revisions 2–4 (7.6.4.3.2,
/// Algorithm 2 step a) and UTF-8 for revisions 5 and 6, which the handler
/// then prepares with SASLprep (7.6.4.3.3). Bytes reach the handler
/// unchanged. The stored bytes are overwritten with zeros when the password
/// is dropped (on a best-effort basis: safe Rust cannot prevent copies made
/// before the password was built), and `Debug` never shows them.
#[derive(Clone, Default)]
pub struct Password(Secret);

/// The password as given, zeroed when dropped.
#[derive(Clone)]
enum Secret {
    /// The exact bytes the handler consumes.
    Bytes(Vec<u8>),
    /// Text, encoded for the handler's revision.
    Text(String),
}

impl Default for Secret {
    fn default() -> Self {
        Self::Bytes(Vec::new())
    }
}

impl Password {
    /// A password given as the exact bytes the security handler consumes.
    pub fn bytes(bytes: impl Into<Vec<u8>>) -> Self {
        Self(Secret::Bytes(bytes.into()))
    }

    /// A password given as text, encoded for the handler's revision.
    pub fn text(text: impl Into<String>) -> Self {
        Self(Secret::Text(text.into()))
    }

    /// Whether this is the empty password, which opens documents that have
    /// no user password.
    pub fn is_empty(&self) -> bool {
        match &self.0 {
            Secret::Bytes(bytes) => bytes.is_empty(),
            Secret::Text(text) => text.is_empty(),
        }
    }

    /// Runs `authenticate` with the bytes the handler of `revision` expects.
    ///
    /// # Errors
    ///
    /// `Unsupported` ([`features::PASSWORD_ENCODING`]) for text that
    /// PDFDocEncoding cannot represent, for revisions 2–4; otherwise the
    /// errors of `authenticate`.
    pub(crate) fn with_handler_bytes<T>(
        &self,
        revision: u8,
        authenticate: impl FnOnce(&[u8]) -> Result<T>,
    ) -> Result<T> {
        let text = match &self.0 {
            Secret::Bytes(bytes) => return authenticate(bytes),
            Secret::Text(text) if revision >= 5 => return authenticate(text.as_bytes()),
            Secret::Text(text) => text,
        };
        let mut encoded = text_string::encode_pdf_doc(text).ok_or(Error::unsupported(
            features::PASSWORD_ENCODING,
            "text password is not representable in PDFDocEncoding",
        ))?;
        let result = authenticate(&encoded);
        zero(&mut encoded);
        result
    }
}

impl fmt::Debug for Password {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let encoding = match self.0 {
            Secret::Bytes(_) => "Bytes",
            Secret::Text(_) => "Text",
        };
        formatter
            .debug_struct("Password")
            .field("encoding", &format_args!("{encoding}"))
            .finish_non_exhaustive()
    }
}

impl Drop for Secret {
    fn drop(&mut self) {
        // Taking the buffer moves it without copying, so the allocation
        // that held the password is the one zeroed.
        let mut bytes = match self {
            Self::Bytes(bytes) => std::mem::take(bytes),
            Self::Text(text) => std::mem::take(text).into_bytes(),
        };
        zero(&mut bytes);
    }
}

fn zero(bytes: &mut Vec<u8>) {
    bytes.fill(0);
    // The buffer is freed next; keep the zeroing from being elided.
    std::hint::black_box(bytes);
}

#[cfg(test)]
mod tests {
    use super::Password;
    use crate::features;
    use pagura_core::Result;

    fn handler_bytes(password: &Password, revision: u8) -> Result<Vec<u8>> {
        password.with_handler_bytes(revision, |bytes| Ok(bytes.to_vec()))
    }

    #[test]
    fn text_is_encoded_for_the_handler_revision_and_bytes_are_not() {
        // Annex D.2: € is 0xA0, é is 0xE9, Ł is 0x95, ﬁ is 0x93, ˘ is 0x18.
        let text = Password::text("€éŁﬁ˘");
        for revision in 2..=4 {
            assert_eq!(
                handler_bytes(&text, revision).unwrap(),
                [0xa0, 0xe9, 0x95, 0x93, 0x18]
            );
        }
        for revision in [5, 6] {
            assert_eq!(handler_bytes(&text, revision).unwrap(), "€éŁﬁ˘".as_bytes());
        }
        let bytes = Password::bytes("€".as_bytes());
        assert_eq!(handler_bytes(&bytes, 3).unwrap(), "€".as_bytes());
        assert!(Password::default().is_empty());
        assert!(!text.is_empty());
    }

    #[test]
    fn unrepresentable_legacy_text_is_unsupported_and_debug_is_redacted() {
        for text in ["\u{a0}", "\u{ad}", "🙂", "\u{7f}", "\u{9f}"] {
            let error = handler_bytes(&Password::text(text), 4).unwrap_err();
            assert_eq!(error.feature(), Some(features::PASSWORD_ENCODING), "{text}");
            assert_eq!(
                handler_bytes(&Password::text(text), 6).unwrap(),
                text.as_bytes()
            );
        }
        let shown = format!("{:?}", Password::text("secret"));
        assert!(!shown.contains("secret"), "{shown}");
        assert_eq!(shown, "Password { encoding: Text, .. }");
    }
}
