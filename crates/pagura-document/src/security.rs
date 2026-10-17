//! Standard-handler integration and encryption exceptions, clause 7.6.

use crate::{Document, IndirectObject, ObjectSource, tags};
use pagura_core::{
    Diagnostic, Diagnostics, Dictionary, Error, Feature, Name, ObjectId, Result, Value,
};
use pagura_crypto::standard::{CryptMethod, FileKey, SecurityParams};
use std::collections::BTreeMap;

#[derive(Debug)]
pub(crate) struct Security {
    // EFOpen-only encryption does not authorize or require a key at DocOpen.
    // Reopening with a password supplies the key before embedded data access.
    key: Option<FileKey>,
    encrypt: Option<ObjectId>,
    filters: BTreeMap<Vec<u8>, CryptFilter>,
    strings: CryptMethod,
    streams: CryptMethod,
    embedded: CryptMethod,
    embedded_authorization: bool,
    encrypt_metadata: bool,
    crypt_filters: bool,
}

#[derive(Debug, Clone, Copy)]
struct CryptFilter {
    method: CryptMethod,
    embedded_only: bool,
}

pub(crate) fn read(document: &Document) -> Result<Option<Security>> {
    let Some(encrypt) = document
        .trailer()
        .get(b"Encrypt")
        .filter(|value| !value.is_null())
    else {
        return Ok(None);
    };
    let id = encrypt.as_reference();
    uncompressed(document, encrypt, "Encrypt dictionary cannot be compressed")?;
    let dictionary = document.resolve_dictionary(encrypt)?;
    let filter = dictionary
        .get_name(b"Filter")
        .ok_or(Error::syntax("Encrypt Filter must be a name"))?;
    if filter != b"Standard" {
        let feature = if filter == b"Adobe.PubSec" {
            tags::PUBSEC
        } else {
            tags::UNKNOWN_SECURITY
        };
        return Err(Error::unsupported(
            feature,
            "non-Standard security handler is not implemented",
        ));
    }
    let mut diagnostics = Diagnostics::new(document.recovery());
    let position = crate::security_repair::position(document, id);
    let mut permissions = None;
    if let Ok(value) = integer(dictionary, b"P")
        && i32::try_from(value).is_err()
        && let Ok(unsigned) = u32::try_from(value)
    {
        let mut diagnostic = Diagnostic::new(
            tags::PERMISSIONS_UNSIGNED,
            "unsigned 32-bit permissions reinterpreted as signed two's complement",
        )
        .at(position);
        if let Some(id) = id {
            diagnostic = diagnostic.in_object(id);
        }
        diagnostics.repair(diagnostic)?;
        permissions = Some(i32::from_le_bytes(unsigned.to_le_bytes()));
    }
    // Normalize one scalar without cloning the source dictionary/value tree.
    let mut params = parameters(dictionary, document.trailer(), permissions)?;
    crate::security_repair::hashes(document, id, &mut params, &mut diagnostics)?;
    let (filters, strings, streams, embedded) = methods(dictionary, &params, |value| {
        uncompressed(
            document,
            value,
            "crypt filter dictionary cannot be compressed",
        )?;
        document.resolve(value)
    })?;
    let deferred = params.version >= 4
        && strings == CryptMethod::Identity
        && streams == CryptMethod::Identity
        && filters.values().all(|filter| filter.embedded_only);
    let key = match crate::security_repair::authenticate(
        document,
        id,
        &params,
        !dictionary.contains_key(b"Length"),
        &mut diagnostics,
    ) {
        Ok(key) => Some(key),
        Err(error) if deferred && error.kind == pagura_core::ErrorKind::Password => None,
        Err(error) => return Err(error),
    };
    document.report(diagnostics.take());
    let embedded_authorization = dictionary
        .get_name(b"EFF")
        .and_then(|name| filters.get(name.as_bytes()))
        .is_some_and(|filter| filter.embedded_only);
    Ok(Some(Security {
        key,
        encrypt: id,
        filters,
        strings,
        streams,
        embedded,
        embedded_authorization,
        encrypt_metadata: params.encrypt_metadata,
        crypt_filters: params.version >= 4,
    }))
}

/// The encryption dictionary and its crypt filters are read before the handler
/// exists. In an object stream they would need the handler they define to be
/// decrypted (7.5.7), so such a reference is an error, never a cached guess.
fn uncompressed(document: &Document, value: &Value, message: &'static str) -> Result<()> {
    match value.as_reference() {
        Some(id)
            if document.slots.get(&id).is_some_and(|slot| {
                matches!(slot.location, crate::Location::Compressed { .. })
            }) =>
        {
            Err(Error::syntax(message).in_object(id))
        }
        _ => Ok(()),
    }
}

fn integer(dictionary: &Dictionary, key: &[u8]) -> Result<i64> {
    dictionary
        .get(key)
        .and_then(Value::as_integer)
        .ok_or(Error::syntax("security parameter must be an integer"))
}

fn bytes(dictionary: &Dictionary, key: &[u8]) -> Result<Vec<u8>> {
    dictionary
        .get(key)
        .and_then(Value::as_string)
        .map(<[u8]>::to_vec)
        .ok_or(Error::syntax("security parameter must be a string"))
}

fn optional_bytes(dictionary: &Dictionary, key: &[u8]) -> Result<Option<Vec<u8>>> {
    dictionary
        .get(key)
        .map(|_| bytes(dictionary, key))
        .transpose()
}

fn parameters(
    dictionary: &Dictionary,
    trailer: &Dictionary,
    permissions: Option<i32>,
) -> Result<SecurityParams> {
    let version = dictionary
        .get(b"V")
        .map_or(Ok(0), |_| integer(dictionary, b"V"))?;
    let version = u8::try_from(version).map_err(|_| Error::syntax("invalid security V"))?;
    if !matches!(version, 1 | 2 | 4 | 5) {
        return Err(Error::unsupported(
            Feature::new("security.version"),
            "unsupported encryption algorithm version",
        ));
    }
    let revision = u8::try_from(integer(dictionary, b"R")?)
        .map_err(|_| Error::syntax("invalid security R"))?;
    let bits = match version {
        1 => 40,
        5 => 256,
        _ => dictionary
            .get(b"Length")
            .map_or(Ok(40), |_| integer(dictionary, b"Length"))?,
    };
    if bits % 8 != 0 || !(40..=256).contains(&bits) || (version != 5 && bits > 128) {
        return Err(Error::syntax("invalid encryption key Length"));
    }
    let encrypt_metadata = match dictionary.get(b"EncryptMetadata") {
        None => true,
        Some(Value::Boolean(value)) => *value,
        _ => return Err(Error::syntax("EncryptMetadata must be a boolean")),
    };
    let document_id = match trailer.get(b"ID") {
        Some(Value::Array(values)) => values
            .first()
            .and_then(Value::as_string)
            .map(<[u8]>::to_vec)
            .ok_or(Error::syntax("trailer ID must begin with a string"))?,
        None if version == 5 => Vec::new(),
        _ => return Err(Error::syntax("encrypted document requires a trailer ID")),
    };
    Ok(SecurityParams {
        version,
        revision,
        key_length: usize::try_from(bits / 8).map_err(|_| Error::syntax("invalid key length"))?,
        owner_hash: bytes(dictionary, b"O")?,
        user_hash: bytes(dictionary, b"U")?,
        owner_encrypted_key: optional_bytes(dictionary, b"OE")?,
        user_encrypted_key: optional_bytes(dictionary, b"UE")?,
        permissions: match permissions {
            Some(permissions) => permissions,
            None => i32::try_from(integer(dictionary, b"P")?)
                .map_err(|_| Error::syntax("permissions out of range"))?,
        },
        encrypted_permissions: optional_bytes(dictionary, b"Perms")?,
        encrypt_metadata,
        document_id,
    })
}

type Methods = (
    BTreeMap<Vec<u8>, CryptFilter>,
    CryptMethod,
    CryptMethod,
    CryptMethod,
);

// CF and its entries are not encrypted and may be indirect (7.6.2, Table 20).
// Resolve dependencies before any authentication or cache initialization lock.
fn methods<'a>(
    dictionary: &'a Dictionary,
    params: &SecurityParams,
    resolve: impl Fn(&'a Value) -> Result<&'a Value>,
) -> Result<Methods> {
    if params.version < 4 {
        return Ok((
            BTreeMap::new(),
            CryptMethod::Rc4,
            CryptMethod::Rc4,
            CryptMethod::Rc4,
        ));
    }
    let mut filters = BTreeMap::new();
    let defaults = [dictionary.get_name(b"StrF"), dictionary.get_name(b"StmF")];
    if let Some(value) = dictionary.get(b"CF") {
        let dictionary = resolve(value)?
            .as_dictionary()
            .ok_or(Error::syntax("CF must be a dictionary"))?;
        for (name, value) in dictionary.iter() {
            if name == b"Identity" {
                return Err(Error::syntax("CF must not redefine Identity"));
            }
            let filter = resolve(value)?
                .as_dictionary()
                .ok_or(Error::syntax("crypt filter must be a dictionary"))?;
            let method = match filter.get_name(b"CFM").map(Name::as_bytes) {
                None | Some(b"None") => CryptMethod::Identity,
                Some(b"V2") if params.version == 4 => CryptMethod::Rc4,
                Some(b"AESV2") if params.version == 4 => CryptMethod::AesV2,
                Some(b"AESV3") if params.version == 5 => CryptMethod::AesV3,
                _ => {
                    return Err(Error::unsupported(
                        Feature::new("security.CFM"),
                        "unsupported crypt filter method",
                    ));
                }
            };
            if filter.contains_key(b"CFM") && filter.get_name(b"CFM").is_none() {
                return Err(Error::syntax("CFM must be a name"));
            }
            if let Some(length) = filter.get(b"Length") {
                let length = length
                    .as_integer()
                    .and_then(|n| usize::try_from(n).ok())
                    .ok_or(Error::syntax("crypt filter Length must be nonnegative"))?;
                if method != CryptMethod::Identity && length != params.key_length {
                    return Err(Error::unsupported(
                        Feature::new("security.crypt-length"),
                        "crypt filter key length differs from the file key",
                    ));
                }
            }
            if let Some(event) = filter.get(b"AuthEvent")
                && !matches!(
                    event.as_name().map(Name::as_bytes),
                    Some(b"DocOpen" | b"EFOpen")
                )
            {
                return Err(Error::syntax("invalid crypt filter AuthEvent"));
            }
            if method == CryptMethod::AesV2 && params.key_length != 16 {
                return Err(Error::syntax("AESV2 requires a 128-bit file key"));
            }
            filters.insert(
                name.as_bytes().to_vec(),
                CryptFilter {
                    method,
                    // Table 25: StrF/StmF override AuthEvent to DocOpen.
                    embedded_only: filter.is_name(b"AuthEvent", b"EFOpen")
                        && !defaults.contains(&Some(name)),
                },
            );
        }
    }
    let select = |key: &[u8], default| -> Result<CryptMethod> {
        match dictionary.get(key) {
            None => Ok(default),
            Some(Value::Name(name)) => lookup(&filters, name.as_bytes(), key == b"EFF"),
            _ => Err(Error::syntax("default crypt filter must be a name")),
        }
    };
    let strings = select(b"StrF", CryptMethod::Identity)?;
    let streams = select(b"StmF", CryptMethod::Identity)?;
    let embedded = select(b"EFF", streams)?;
    Ok((filters, strings, streams, embedded))
}

fn lookup(
    filters: &BTreeMap<Vec<u8>, CryptFilter>,
    name: &[u8],
    embedded: bool,
) -> Result<CryptMethod> {
    if name == b"Identity" {
        return Ok(CryptMethod::Identity);
    }
    let filter = filters
        .get(name)
        .ok_or(Error::syntax("unknown crypt filter name"))?;
    if filter.embedded_only && !embedded {
        return Err(Error::syntax(
            "EFOpen crypt filter used outside an embedded file",
        ));
    }
    Ok(filter.method)
}

impl Security {
    /// Select a cipher using the security dictionary; placement and execution
    /// of Crypt filters belong to the stream-access module (7.6).
    pub(crate) fn stream_method(
        &self,
        dictionary: &Dictionary,
        explicit: crate::stream::CryptSelection,
    ) -> Result<CryptMethod> {
        if !matches!(explicit, crate::stream::CryptSelection::Default) && !self.crypt_filters {
            return Err(Error::syntax(
                "Crypt filters require encryption version 4 or 5",
            ));
        }
        if !self.encrypt_metadata && dictionary.is_name(b"Type", b"Metadata") {
            return Ok(CryptMethod::Identity);
        }
        let authorization = match &explicit {
            crate::stream::CryptSelection::Named(name) => self
                .filters
                .get(name.as_bytes())
                .is_some_and(|filter| filter.embedded_only),
            crate::stream::CryptSelection::Default => {
                dictionary.is_name(b"Type", b"EmbeddedFile") && self.embedded_authorization
            }
            crate::stream::CryptSelection::Identity => false,
        };
        let method = match explicit {
            crate::stream::CryptSelection::Named(name) => lookup(
                &self.filters,
                name.as_bytes(),
                dictionary.is_name(b"Type", b"EmbeddedFile"),
            )?,
            crate::stream::CryptSelection::Identity => CryptMethod::Identity,
            crate::stream::CryptSelection::Default
                if dictionary.is_name(b"Type", b"EmbeddedFile") =>
            {
                self.embedded
            }
            crate::stream::CryptSelection::Default => self.streams,
        };
        if self.key.is_none() && authorization {
            return Err(Error::password(
                "embedded file access requires an authenticated password",
            ));
        }
        Ok(method)
    }

    pub(crate) fn decrypt_stream(
        &self,
        method: CryptMethod,
        id: ObjectId,
        data: &[u8],
    ) -> Result<Vec<u8>> {
        match (&self.key, method) {
            (Some(key), _) => key.decrypt(method, id, data),
            (None, CryptMethod::Identity) => {
                let mut copy = Vec::new();
                copy.try_reserve_exact(data.len())
                    .map_err(|_| Error::limit("stream copy allocation failed").in_object(id))?;
                copy.extend_from_slice(data);
                Ok(copy)
            }
            (None, _) => Err(Error::password(
                "encrypted embedded data requires an authenticated password",
            )
            .in_object(id)),
        }
    }

    pub(crate) fn decrypt_object(&self, object: &mut IndirectObject) -> Result<()> {
        if self.encrypt == Some(object.id())
            || matches!(object.source(), ObjectSource::Compressed { .. })
            || object
                .dictionary()
                .is_some_and(|dictionary| dictionary.is_name(b"Type", b"XRef"))
        {
            return Ok(());
        }
        if self.strings == CryptMethod::Identity {
            return Ok(());
        }
        let id = object.id();
        let key = self.key.as_ref().ok_or_else(|| {
            Error::password("encrypted strings require an authenticated password").in_object(id)
        })?;
        decrypt_strings(key, self.strings, id, &mut object.value)?;
        for direct in object.direct_objects_mut() {
            decrypt_strings(key, self.strings, id, &mut direct.value)?;
        }
        Ok(())
    }
}

fn decrypt_strings(
    key: &FileKey,
    method: CryptMethod,
    id: ObjectId,
    value: &mut Value,
) -> Result<()> {
    match value {
        Value::String(bytes) => {
            *bytes = key
                .decrypt(method, id, bytes)
                .map_err(|error| error.in_object(id))?;
        }
        Value::Array(values) => {
            for value in values {
                decrypt_strings(key, method, id, value)?;
            }
        }
        Value::Dictionary(dictionary) => {
            let signature = dictionary.is_name(b"Type", b"Sig")
                || dictionary.is_name(b"Type", b"DocTimeStamp")
                || (!dictionary.contains_key(b"Type")
                    && dictionary.contains_key(b"ByteRange")
                    && dictionary.contains_key(b"Filter")
                    && dictionary.contains_key(b"Contents"));
            for (name, value) in dictionary.iter_mut() {
                // Signature Contents are signature bytes, not ciphertext (7.6.3).
                if signature && name.as_bytes() == b"Contents" {
                    continue;
                }
                decrypt_strings(key, method, id, value)?;
            }
        }
        _ => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{lookup, methods};
    use crate::stream::validate_crypt;
    use crate::{Document, OpenOptions};
    use pagura_core::syntax::{Origin, Parser};
    use pagura_core::{Dictionary, ErrorKind, Limits, Value};
    use pagura_crypto::standard::CryptMethod;
    use pagura_testkit::fixture;

    fn parameters(
        dictionary: &Dictionary,
        trailer: &Dictionary,
    ) -> pagura_core::Result<pagura_crypto::standard::SecurityParams> {
        super::parameters(dictionary, trailer, None)
    }

    fn dictionary(bytes: &[u8]) -> Dictionary {
        Parser::new(bytes, Origin::Buffer, Limits::default())
            .parse()
            .unwrap()
            .as_dictionary()
            .unwrap()
            .clone()
    }

    fn trailer() -> Dictionary {
        dictionary(b"<< /ID [<00112233> <44556677>] >>")
    }

    #[test]
    fn versions_key_lengths_and_plain_parameters_match_the_standard() {
        for (version, revision, length) in
            [(1, 2, 5), (2, 3, 16), (4, 4, 16), (5, 5, 32), (5, 6, 32)]
        {
            let encryption = dictionary(format!("<< /V {version} /R {revision} /Length 128 /O (owner) /U (user) /P -4 /OE (oe) /UE (ue) /Perms (perms) /EncryptMetadata false >>").as_bytes());
            let params = parameters(&encryption, &trailer()).unwrap();
            assert_eq!(params.version, version);
            assert_eq!(params.revision, revision);
            assert_eq!(params.key_length, length);
            assert_eq!(params.permissions, -4);
            assert_eq!(params.owner_hash, b"owner");
            assert_eq!(params.user_hash, b"user");
            assert_eq!(params.owner_encrypted_key.as_deref(), Some(&b"oe"[..]));
            assert_eq!(params.user_encrypted_key.as_deref(), Some(&b"ue"[..]));
            assert_eq!(params.encrypted_permissions.as_deref(), Some(&b"perms"[..]));
            assert_eq!(params.document_id, [0, 0x11, 0x22, 0x33]);
            assert!(!params.encrypt_metadata);
        }
    }

    #[test]
    fn crypt_filter_defaults_identity_and_embedded_file_events() {
        let encryption = dictionary(b"<< /V 4 /R 4 /Length 128 /O () /U () /P -4 /CF << /RC << /CFM /V2 /Length 16 >> /AES << /CFM /AESV2 >> /EF << /CFM /V2 /AuthEvent /EFOpen >> >> /StrF /Identity /StmF /AES /EFF /EF >>");
        let params = parameters(&encryption, &trailer()).unwrap();
        let (filters, strings, streams, embedded) = methods(&encryption, &params, Ok).unwrap();
        assert_eq!(strings, CryptMethod::Identity);
        assert_eq!(streams, CryptMethod::AesV2);
        assert_eq!(embedded, CryptMethod::Rc4);
        assert_eq!(lookup(&filters, b"RC", false).unwrap(), CryptMethod::Rc4);
        assert_eq!(
            lookup(&filters, b"Identity", false).unwrap(),
            CryptMethod::Identity
        );
        assert_eq!(
            lookup(&filters, b"EF", false).unwrap_err().kind,
            ErrorKind::Syntax
        );
        let mut encryption = encryption;
        encryption.remove(b"StrF");
        encryption.remove(b"StmF");
        encryption.remove(b"EFF");
        let (_, strings, streams, embedded) = methods(&encryption, &params, Ok).unwrap();
        assert_eq!(
            (strings, streams, embedded),
            (
                CryptMethod::Identity,
                CryptMethod::Identity,
                CryptMethod::Identity
            )
        );
    }

    #[test]
    fn invalid_security_parameters_are_never_silently_defaulted() {
        for entry in [
            b"/Length 41".as_slice(),
            b"/Length -8",
            b"/EncryptMetadata 1",
            b"/P 2147483648",
            b"/U 9",
        ] {
            let base = b"<< /V 4 /R 4 /Length 128 /O () /U () /P -4 >>";
            let mut encryption = dictionary(base);
            encryption.extend(dictionary(&[b"<< ".as_slice(), entry, b" >>"].concat()));
            assert_eq!(
                parameters(&encryption, &trailer()).unwrap_err().kind,
                ErrorKind::Syntax
            );
        }
        for entry in [
            b"/CF << /X << /CFM 1 >> >>".as_slice(),
            b"/StmF /Missing",
            b"/StrF 1",
            b"/CF << /Identity << >> >>",
            b"/CF << /X << /AuthEvent /Wrong >> >>",
        ] {
            let mut encryption = dictionary(b"<< /V 4 /R 4 /Length 128 /O () /U () /P -4 >>");
            encryption.extend(dictionary(&[b"<< ".as_slice(), entry, b" >>"].concat()));
            let params = parameters(&encryption, &trailer()).unwrap();
            assert_eq!(
                methods(&encryption, &params, Ok).unwrap_err().kind,
                ErrorKind::Syntax
            );
        }
        assert_eq!(
            validate_crypt(
                Some(&Value::name(b"Crypt")),
                Some(&Value::Dictionary(dictionary(b"<< /Name 7 >>")))
            )
            .unwrap_err()
            .kind,
            ErrorKind::Syntax
        );
    }

    #[test]
    fn handler_errors_propagate_and_password_debug_is_redacted() {
        let mut document =
            Document::open(fixture("classic", "rectangle.pdf"), &OpenOptions::default()).unwrap();
        document.trailer.insert(
            pagura_core::Name::new(b"Encrypt"),
            Value::Dictionary(dictionary(
                b"<< /Filter /Standard /V 1 /R 2 /O () /U () /P -4 >>",
            )),
        );
        document.trailer.extend(trailer());
        let parameters = parameters(
            document
                .trailer
                .get(b"Encrypt")
                .unwrap()
                .as_dictionary()
                .unwrap(),
            document.trailer(),
        )
        .unwrap();
        let expected = pagura_crypto::standard::authenticate(&parameters, b"").unwrap_err();
        assert_eq!(super::read(&document).unwrap_err(), expected);
        *document.trailer.get_mut(b"Encrypt").unwrap() =
            Value::Dictionary(dictionary(b"<< /Filter /Adobe.PubSec >>"));
        assert_eq!(
            super::read(&document).unwrap_err().feature(),
            Some(crate::tags::PUBSEC)
        );
        let options = OpenOptions {
            password: b"private-password".to_vec(),
            ..OpenOptions::default()
        };
        assert!(!format!("{options:?}").contains("private-password"));
    }
}

#[cfg(test)]
mod indirect_filters {
    use crate::{Document, OpenOptions};
    use pagura_core::{ErrorKind, ObjectId, Recovery};
    use pagura_testkit::fixture;

    #[test]
    fn indirect_filter_dictionaries_authenticate_in_strict_mode() {
        for name in ["security-indirect-cf", "security-indirect-filter"] {
            let bytes = fixture("document", &format!("{name}.pdf"));
            for password in [b"".as_slice(), b"owner"] {
                let document = Document::open(
                    bytes.clone(),
                    &OpenOptions {
                        recovery: Recovery::Strict,
                        password: password.to_vec(),
                        ..OpenOptions::default()
                    },
                )
                .unwrap();
                assert_eq!(
                    document
                        .stream_data(document.object(ObjectId::new(4, 0)).unwrap())
                        .unwrap()
                        .as_ref(),
                    b"1 0 0 rg 8 8 48 32 re f\n"
                );
                assert_eq!(document.diagnostics(), []);
            }
            assert_eq!(
                Document::open(
                    bytes,
                    &OpenOptions {
                        password: b"wrong".to_vec(),
                        ..OpenOptions::default()
                    }
                )
                .unwrap_err()
                .kind,
                ErrorKind::Password
            );
        }
    }
}

#[cfg(test)]
mod authorization_events {
    use crate::{Document, OpenOptions};
    use pagura_core::{ErrorKind, ObjectId, Recovery};
    use pagura_testkit::fixture;

    #[test]
    fn embedded_file_authorization_does_not_block_identity_page_data() {
        for name in [
            "security-embedded-event",
            "security-embedded-identity-filter",
        ] {
            let bytes = fixture("document", &format!("{name}.pdf"));
            for recovery in [Recovery::Strict, Recovery::Repair] {
                for password in [b"".as_slice(), b"wrong"] {
                    let document = Document::open(
                        bytes.clone(),
                        &OpenOptions {
                            recovery,
                            password: password.to_vec(),
                            ..OpenOptions::default()
                        },
                    )
                    .unwrap();
                    assert_eq!(document.page_count().unwrap(), 1);
                    assert_eq!(
                        document
                            .catalog()
                            .unwrap()
                            .get(b"Marker")
                            .unwrap()
                            .as_string(),
                        Some(b"independent".as_slice())
                    );
                    let content = document.object(ObjectId::new(4, 0)).unwrap();
                    assert_eq!(
                        document.stream_data(content).unwrap().as_ref(),
                        b"1 0 0 rg 8 8 48 32 re f\n"
                    );
                    let embedded = document.object(ObjectId::new(6, 0)).unwrap();
                    let error = document.stream_data(embedded).unwrap_err();
                    assert_eq!(error.kind, ErrorKind::Password);
                    assert_eq!(error.object, Some(embedded.id()));
                    let retained = document.retained.used();
                    assert_eq!(document.stream_data(embedded).unwrap_err(), error);
                    assert_eq!(document.retained.used(), retained);
                    assert_eq!(document.diagnostics(), []);
                }
                for password in [b"attachment".as_slice(), b"owner"] {
                    let document = Document::open(
                        bytes.clone(),
                        &OpenOptions {
                            recovery,
                            password: password.to_vec(),
                            ..OpenOptions::default()
                        },
                    )
                    .unwrap();
                    assert_eq!(
                        document
                            .stream_data(document.object(ObjectId::new(6, 0)).unwrap())
                            .unwrap()
                            .as_ref(),
                        b"attached file bytes"
                    );
                }
            }
        }
    }

    #[test]
    fn doc_open_is_required_for_default_filters_and_explicit_doc_open_events() {
        let bytes = fixture("document", "security-embedded-doc-open.pdf");
        for recovery in [Recovery::Strict, Recovery::Repair] {
            assert_eq!(
                Document::open(
                    bytes.clone(),
                    &OpenOptions {
                        recovery,
                        ..OpenOptions::default()
                    }
                )
                .unwrap_err()
                .kind,
                ErrorKind::Password
            );
            let document = Document::open(
                bytes.clone(),
                &OpenOptions {
                    recovery,
                    password: b"attachment".to_vec(),
                    ..OpenOptions::default()
                },
            )
            .unwrap();
            assert_eq!(document.page_count().unwrap(), 1);
            let default = Document::open(
                fixture("document", "security-default-event-override.pdf"),
                &OpenOptions {
                    recovery,
                    ..OpenOptions::default()
                },
            )
            .unwrap();
            assert_eq!(
                default
                    .stream_data(default.object(ObjectId::new(4, 0)).unwrap())
                    .unwrap()
                    .as_ref(),
                b"1 0 0 rg 8 8 48 32 re f\n"
            );
            assert_eq!(default.diagnostics(), []);
        }
    }
}

#[cfg(test)]
mod missing_length {
    use crate::{Document, OpenOptions, tags};
    use pagura_core::{ErrorKind, ObjectId, Recovery};
    use pagura_testkit::fixture;

    #[test]
    fn only_a_unique_authenticated_size_replaces_the_missing_default() {
        let bytes = fixture("document", "security-length-missing.pdf");
        for password in [b"".as_slice(), b"owner"] {
            let options = OpenOptions {
                password: password.to_vec(),
                ..OpenOptions::default()
            };
            let strict = Document::open(
                bytes.clone(),
                &OpenOptions {
                    recovery: Recovery::Strict,
                    ..options.clone()
                },
            )
            .unwrap_err();
            assert_eq!(strict.repair, Some(tags::KEY_LENGTH_MISSING));
            assert_eq!(strict.object, Some(ObjectId::new(5, 0)));
            let document = Document::open(bytes.clone(), &options).unwrap();
            assert_eq!(document.page_count().unwrap(), 1);
            assert_eq!(
                document
                    .catalog()
                    .unwrap()
                    .get(b"Marker")
                    .unwrap()
                    .as_string(),
                Some(b"independent".as_slice())
            );
            assert_eq!(
                document
                    .stream_data(document.object(ObjectId::new(4, 0)).unwrap())
                    .unwrap()
                    .as_ref(),
                b"1 0 0 rg 8 8 48 32 re f\n"
            );
            assert_eq!(
                document
                    .diagnostics()
                    .iter()
                    .filter(|d| d.repair == tags::KEY_LENGTH_MISSING)
                    .count(),
                1
            );
        }
        for name in [
            "security-length-missing",
            "security-length-missing-password",
        ] {
            let bytes = fixture("document", &format!("{name}.pdf"));
            let error = Document::open(
                bytes,
                &OpenOptions {
                    password: b"wrong".to_vec(),
                    ..OpenOptions::default()
                },
            )
            .unwrap_err();
            assert_eq!(error.kind, ErrorKind::Password);
            assert_eq!(error.repair, None);
        }
    }
}
