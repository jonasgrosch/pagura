//! `security`: the standard security handler authenticates a password and
//! decrypts strings and streams, or fails with a structured error, in both
//! recovery modes (7.6); the document is then walked as `document` does.
//!
//! Layout: a selector byte, the password bytes, [`super::wrapper::SEPARATOR`],
//! then a PDF file (without a separator, the whole rest is the file and the
//! password is empty). The selector's high bit selects strict recovery.

use super::wrapper::{self, SEPARATOR};
use pagura::{Document, OpenOptions};

/// Opens the file with the password and walks it.
///
/// # Panics
///
/// When the input violates a checked property: that is a finding.
pub fn run(data: &[u8]) {
    let Some((&selector, rest)) = data.split_first() else {
        return;
    };
    let (password, pdf) = match wrapper::find(rest, SEPARATOR) {
        Some(at) => (&rest[..at], &rest[at + SEPARATOR.len()..]),
        None => (&[][..], rest),
    };
    let options = OpenOptions {
        limits: super::limits(),
        recovery: super::recovery(selector & 0x80 != 0),
        password: password.to_vec(),
    };
    if let Some(document) = super::checked(Document::open(pdf.to_vec(), &options)) {
        super::document::walk(document);
    }
}
