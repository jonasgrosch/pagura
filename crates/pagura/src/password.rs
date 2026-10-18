//! CLI Unicode passwords: legacy PDFDocEncoding versus R5/R6 UTF-8.
//!
//! ISO 32000-2 7.6.4.3.2 Algorithm 2(a), 7.6.4.3.3, and Annex D.3.
//! The byte-oriented Document API and host protocol remain unchanged.

use pagura::OpenOptions;
use pagura_document::Document;
use std::sync::Arc;

fn legacy(document: &Document) -> bool {
    document
        .trailer()
        .get(b"Encrypt")
        .and_then(|value| document.resolve(value).ok())
        .and_then(|value| value.as_dictionary())
        .and_then(|dictionary| dictionary.get(b"R"))
        .and_then(pagura_core::Value::as_integer)
        .is_some_and(|revision| (2..=4).contains(&revision))
}

pub(super) fn open(bytes: Arc<[u8]>, options: &OpenOptions) -> pagura_core::Result<Document> {
    let encoded = std::str::from_utf8(&options.password).ok().and_then(encode);
    let Some(encoded) = encoded.filter(|encoded| *encoded != options.password) else {
        let document = Document::open(bytes, options)?;
        if !options.password.is_ascii()
            && std::str::from_utf8(&options.password)
                .ok()
                .and_then(encode)
                .is_none()
            && legacy(&document)
        {
            return Err(pagura_core::Error::unsupported(
                pagura_core::Feature::new("security.password-encoding"),
                "Unicode password is not representable in PDFDocEncoding",
            ));
        }
        return Ok(document);
    };
    // Select the encoding by the authenticated handler, without a new public
    // preflight parser or any change to the library's byte-password contract.
    let legacy_options = OpenOptions {
        password: encoded,
        ..options.clone()
    };
    let first_error = match Document::open(Arc::clone(&bytes), &legacy_options) {
        Ok(document) if legacy(&document) => return Ok(document),
        Ok(_) => None,
        Err(error) => Some(error),
    };
    match Document::open(bytes, options) {
        Ok(document) if !legacy(&document) => Ok(document),
        Ok(_) => Err(first_error.unwrap_or_else(|| {
            pagura_core::Error::password("legacy password must use PDFDocEncoding")
        })),
        Err(error) => Err(first_error
            .filter(|first| first.repair.is_some() || first.kind == pagura_core::ErrorKind::Limit)
            .unwrap_or(error)),
    }
}

/// Annex D.3's defined code points only. Unrepresentable input is not replaced
/// with question marks or truncated UTF-8 bytes and cannot name a legacy password.
fn encode(text: &str) -> Option<Vec<u8>> {
    const HIGH: &str = "•†‡…—–ƒ⁄‹›−‰„“”‘’‚™ﬁﬂŁŒŠŸŽıłœšž";
    const ACCENTS: &str = "˘ˇˆ˙˝˛˚˜";
    text.chars()
        .map(|character| {
            let code = u32::from(character);
            match code {
                9 | 10 | 13 | 0x20..=0x7e | 0xa1..=0xac | 0xae..=0xff => u8::try_from(code).ok(),
                0x20ac => Some(0xa0),
                _ => HIGH
                    .chars()
                    .position(|c| c == character)
                    .and_then(|i| u8::try_from(i).ok())
                    .and_then(|i| i.checked_add(0x80))
                    .or_else(|| {
                        ACCENTS
                            .chars()
                            .position(|c| c == character)
                            .and_then(|i| u8::try_from(i).ok())
                            .and_then(|i| i.checked_add(0x18))
                    }),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{encode, open};
    use pagura::OpenOptions;
    use pagura_core::{ErrorKind, ObjectId};
    use pagura_testkit::fixture;

    #[test]
    fn annex_d_password_encoding_has_no_lossy_replacements() {
        assert_eq!(encode("€éŁﬁ˘"), Some(vec![0xa0, 0xe9, 0x95, 0x93, 0x18]));
        assert_eq!(encode("ASCII"), Some(b"ASCII".to_vec()));
        for text in ["\u{a0}", "\u{ad}", "🙂", "\u{7f}", "\u{9f}"] {
            assert_eq!(encode(text), None);
        }
    }

    #[test]
    fn unicode_cli_password_uses_pdfdocencoding_for_legacy_handlers() {
        let bytes = fixture("crypto", "r3-pdfdocencoding-euro.pdf");
        let document = open(
            bytes.clone().into(),
            &OpenOptions {
                password: "€".as_bytes().to_vec(),
                ..OpenOptions::default()
            },
        )
        .unwrap();
        assert_eq!(document.page_count().unwrap(), 1);
        assert_eq!(
            document
                .stream_data(document.object(ObjectId::new(4, 0)).unwrap())
                .unwrap()
                .as_ref(),
            b"1 0 0 rg 8 8 48 32 re f\n0 0 1 rg 0 0 8 8 re f\n"
        );
        assert_eq!(
            open(
                bytes.into(),
                &OpenOptions {
                    password: "wrong".as_bytes().to_vec(),
                    ..OpenOptions::default()
                }
            )
            .unwrap_err()
            .kind,
            ErrorKind::Password
        );
    }
}
