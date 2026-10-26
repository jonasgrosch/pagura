//! Standard-handler integration and encryption exceptions, clause 7.6.

mod padding;
mod policy;
mod repair;

use crate::source::{ReadFault, ReadResult};
use crate::{Document, IndirectObject, Location, ObjectSource, features, repairs};
use pagura_core::{Dictionary, Error, ErrorKind, Name, ObjectId, Result, Value};
use pagura_crypto::standard::{CryptMethod, FileKey, SecurityParams};
use policy::Policy;
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
    derivation: padding::Derivation,
}

#[derive(Debug, Clone, Copy)]
struct CryptFilter {
    method: CryptMethod,
    embedded_only: bool,
}

/// Table 22 bits 9, 11 and 12, which only revision 3 and later define.
const REVISION_3_PERMISSIONS: u32 = 0x0d00;

/// Reads the encryption dictionary and authenticates `password` (7.6.4).
pub(crate) fn read(document: &Document, password: &[u8]) -> ReadResult<Option<Security>> {
    let Some(encrypt) = document
        .trailer()
        .get(b"Encrypt")
        .filter(|value| !value.is_null())
    else {
        return Ok(None);
    };
    let id = encrypt.as_reference();
    handler(document, encrypt, id, password)
        .map_err(|fault| fault.in_object_opt(id))
        .map(Some)
}

/// The Standard handler of the encryption dictionary `encrypt`, object `id`.
fn handler(
    document: &Document,
    encrypt: &Value,
    id: Option<ObjectId>,
    password: &[u8],
) -> ReadResult<Security> {
    uncompressed(document, encrypt, "Encrypt dictionary cannot be compressed")?;
    let dictionary = document.resolve_dictionary(encrypt)?;
    let filter = dictionary
        .get_name(b"Filter")
        .ok_or(Error::syntax("Encrypt Filter must be a name"))?;
    if filter != b"Standard" {
        let feature = if filter == b"Adobe.PubSec" {
            features::PUBSEC
        } else {
            features::UNKNOWN_SECURITY
        };
        return Err(Error::unsupported(
            feature,
            "non-Standard security handler is not implemented",
        )
        .into());
    }
    let mut policy = Policy::new(document, id)?;
    let permissions = policy.unsigned_permissions(dictionary)?;
    // Normalize one scalar without cloning the source dictionary/value tree.
    let mut params = parameters(dictionary, document.trailer(), permissions)?;
    policy.identifiers(document.trailer())?;
    repair::hashes(document, id, &mut params, &mut policy.diagnostics)?;
    let methods = methods(
        dictionary,
        &params,
        |value| {
            uncompressed(
                document,
                value,
                "crypt filter dictionary cannot be compressed",
            )?;
            document.resolve(value)
        },
        &mut policy,
    )?;
    let length = repair::KeyLength::of(dictionary);
    let key = authenticate_document(
        document,
        id,
        &params,
        length,
        password,
        &methods,
        &mut policy,
    )?;
    let derivation = padding::select(document, key.as_ref(), &params, &methods, &mut policy)?;
    let Methods {
        filters,
        strings,
        streams,
        embedded,
    } = methods;
    document.report(policy.diagnostics.take());
    let embedded_authorization = dictionary
        .get_name(b"EFF")
        .and_then(|name| filters.get(name.as_bytes()))
        .is_some_and(|filter| filter.embedded_only);
    Ok(Security {
        key,
        encrypt: id,
        filters,
        strings,
        streams,
        embedded,
        embedded_authorization,
        encrypt_metadata: params.encrypt_metadata,
        crypt_filters: params.version >= 4,
        derivation,
    })
}

/// A V=4 RC4 key shorter than 128 bits, which Table 20 does not allow.
fn short_rc4_key(params: &SecurityParams) -> bool {
    params.version == 4 && params.revision == 4 && params.key_length != 16
}

fn authenticate_document(
    document: &Document,
    id: Option<ObjectId>,
    params: &SecurityParams,
    length: repair::KeyLength,
    password: &[u8],
    methods: &Methods,
    policy: &mut Policy,
) -> ReadResult<Option<FileKey>> {
    let short_key = short_rc4_key(params);
    if short_key {
        policy.repair(
            repairs::SHORT_KEY,
            "nonstandard short V=4 RC4 key size requires explicit legacy authentication",
        )?;
    }
    let deferred = params.version >= 4
        && methods.strings == CryptMethod::Identity
        && methods.streams == CryptMethod::Identity
        && methods.filters.values().all(|filter| filter.embedded_only);
    let selector = params.version == 1
        && params.revision == 2
        && params.permissions.cast_unsigned() & REVISION_3_PERMISSIONS != REVISION_3_PERMISSIONS;
    if selector {
        policy.repair(
            repairs::PERMISSION_SELECTOR,
            "R2 revision-only permissions conflict; original revision and hash bytes preserved",
        )?;
    }
    let authentication = if short_key || selector {
        pagura_crypto::standard::authenticate_legacy_compatibility(params, password)
            .map_err(ReadFault::from)
    } else {
        repair::authenticate(
            document,
            id,
            params,
            length,
            password,
            &mut policy.diagnostics,
        )
    };
    let key = match authentication {
        Ok(key) => Some(key),
        Err(ReadFault::Invalid(error)) if deferred && error.kind == ErrorKind::Password => None,
        Err(error) => return Err(error),
    };
    if let Some(key) = &key {
        policy.permissions(key)?;
    }
    Ok(key)
}

/// The encryption dictionary and its crypt filters are read before the handler
/// exists. In an object stream they would need the handler they define to be
/// decrypted (7.5.7), so such a reference is an error, never a cached guess.
fn uncompressed(document: &Document, value: &Value, message: &'static str) -> Result<()> {
    match value.as_reference() {
        Some(id)
            if document
                .slots
                .get(&id)
                .is_some_and(|slot| matches!(slot.location, Location::Compressed { .. })) =>
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
            features::SECURITY_VERSION,
            "unsupported encryption algorithm version",
        ));
    }
    let revision = u8::try_from(integer(dictionary, b"R")?)
        .map_err(|_| Error::syntax("invalid security R"))?;
    let bits = match version {
        1 => 40,
        5 => 256,
        4 => dictionary
            .get(b"Length")
            .map_or(Ok(128), |_| integer(dictionary, b"Length"))?,
        _ => dictionary
            .get(b"Length")
            .map_or(Ok(40), |_| integer(dictionary, b"Length"))?,
    };
    if bits % 8 != 0 || !(40..=256).contains(&bits) || (version != 5 && bits > 128) {
        return Err(Error::syntax("invalid encryption key Length"));
    }
    let encrypt_metadata = match dictionary.get(b"EncryptMetadata").filter(|_| version >= 4) {
        None => true,
        Some(Value::Boolean(value)) => *value,
        _ => return Err(Error::syntax("EncryptMetadata must be a boolean")),
    };
    let document_id = trailer
        .get(b"ID")
        .and_then(Value::as_array)
        .and_then(|ids| ids.first())
        .and_then(Value::as_string);
    let document_id = match document_id {
        Some(bytes) if !bytes.is_empty() => bytes.to_vec(),
        _ if version == 5 => Vec::new(),
        _ => {
            return Err(Error::syntax(
                "encrypted legacy document requires its first trailer ID",
            ));
        }
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

/// The crypt filters of a dictionary and the methods it selects (7.6.5).
#[derive(Debug)]
struct Methods {
    filters: BTreeMap<Vec<u8>, CryptFilter>,
    /// `StrF`: strings.
    strings: CryptMethod,
    /// `StmF`: streams.
    streams: CryptMethod,
    /// `EFF`: embedded file streams.
    embedded: CryptMethod,
}

// CF and its entries are not encrypted and may be indirect (7.6.2, Table 20).
// Resolve dependencies before any authentication or cache initialization lock.
fn methods<'a>(
    dictionary: &'a Dictionary,
    params: &SecurityParams,
    resolve: impl Fn(&'a Value) -> ReadResult<&'a Value>,
    policy: &mut Policy,
) -> ReadResult<Methods> {
    if params.version < 4 {
        return Ok(Methods {
            filters: BTreeMap::new(),
            strings: CryptMethod::Rc4,
            streams: CryptMethod::Rc4,
            embedded: CryptMethod::Rc4,
        });
    }
    let mut filters = BTreeMap::new();
    let defaults = [dictionary.get_name(b"StrF"), dictionary.get_name(b"StmF")];
    if let Some(value) = dictionary.get(b"CF") {
        let dictionary = resolve(value)?
            .as_dictionary()
            .ok_or(Error::syntax("CF must be a dictionary"))?;
        for (name, value) in dictionary {
            if name == b"Identity" {
                continue;
            }
            let filter = resolve(value)?
                .as_dictionary()
                .ok_or(Error::syntax("crypt filter must be a dictionary"))?;
            if name != b"StdCF" {
                policy.repair(
                    repairs::CRYPT_NAME,
                    "nonstandard crypt-filter name used with the Standard security handler",
                )?;
            }
            let method = crypt_method(filter, params, policy)?;
            if filter.is_name(b"AuthEvent", b"EFOpen") && !defaults.contains(&Some(name)) {
                policy.repair(
                    repairs::CRYPT_EVENT,
                    "EFOpen-only filter used as a Standard-handler extension",
                )?;
            }
            if let Some(event) = filter.get(b"AuthEvent").filter(|value| !value.is_null())
                && !defaults.contains(&Some(name))
                && !matches!(
                    event.as_name().map(Name::as_bytes),
                    Some(b"DocOpen" | b"EFOpen")
                )
            {
                return Err(Error::syntax("invalid crypt filter AuthEvent").into());
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
    Ok(Methods {
        filters,
        strings,
        streams,
        embedded,
    })
}

fn crypt_method(
    filter: &Dictionary,
    params: &SecurityParams,
    policy: &mut Policy,
) -> Result<CryptMethod> {
    if filter
        .get(b"Type")
        .filter(|value| !value.is_null())
        .is_some_and(|value| !matches!(value, Value::Name(name) if name == b"CryptFilter"))
    {
        policy.repair(
            repairs::CRYPT_TYPE,
            "crypt-filter Type corrected from its explicit CF dictionary membership",
        )?;
    }
    if filter.contains_key(b"CFM") && filter.get_name(b"CFM").is_none() {
        return Err(Error::syntax("CFM must be a name"));
    }
    let method = match filter.get_name(b"CFM").map(Name::as_bytes) {
        None | Some(b"None") => {
            return Err(Error::unsupported(
                features::CRYPT_METHOD,
                "Standard handler cannot dispatch the delegated None crypt-filter method",
            ));
        }
        Some(b"V2") if params.version == 4 => CryptMethod::Rc4,
        Some(b"AESV2") if params.version == 4 => CryptMethod::AesV2,
        Some(b"AESV3") if params.version == 5 => CryptMethod::AesV3,
        _ => {
            return Err(Error::unsupported(
                features::CRYPT_METHOD,
                "unsupported crypt filter method",
            ));
        }
    };
    if let Some(length) = filter.get(b"Length").filter(|value| !value.is_null()) {
        let length = length
            .as_integer()
            .and_then(|n| usize::try_from(n).ok())
            .ok_or(Error::syntax("crypt filter Length must be nonnegative"))?;
        if method != CryptMethod::Identity && length != params.key_length {
            return Err(Error::unsupported(
                features::CRYPT_KEY_LENGTH,
                "crypt filter key length differs from the file key",
            ));
        }
    }
    if filter.get(b"Length").is_none_or(Value::is_null) {
        policy.repair(
            repairs::CRYPT_LENGTH,
            "missing crypt-filter Length inferred from the file key size",
        )?;
    }
    if method == CryptMethod::AesV2 && params.key_length != 16 {
        return Err(Error::syntax("AESV2 requires a 128-bit file key"));
    }
    Ok(method)
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
    pub(crate) fn authenticated(&self) -> bool {
        self.key.is_some()
    }

    /// Select a cipher using the security dictionary; placement and execution
    /// of Crypt filters belong to the stream-access module (7.6).
    pub(crate) fn stream_method(
        &self,
        document: &Document,
        object: &IndirectObject,
        explicit: crate::stream::CryptSelection,
        usage: crate::stream::Usage,
    ) -> ReadResult<CryptMethod> {
        let dictionary = object
            .dictionary()
            .ok_or_else(|| Error::syntax("stream requires a dictionary").in_object(object.id()))?;
        let embedded = usage == crate::stream::Usage::EmbeddedFile
            || dictionary.is_name(b"Type", b"EmbeddedFile");
        if !matches!(explicit, crate::stream::CryptSelection::Default) && !self.crypt_filters {
            return Err(Error::syntax("Crypt filters require encryption version 4 or 5").into());
        }
        if !self.encrypt_metadata
            && !object.is_direct()
            && dictionary.is_name(b"Type", b"Metadata")
            && document
                .catalog()?
                .get(b"Metadata")
                .and_then(Value::as_reference)
                == Some(object.id())
        {
            return Ok(CryptMethod::Identity);
        }
        let authorization = match &explicit {
            crate::stream::CryptSelection::Named(name) => self
                .filters
                .get(name.as_bytes())
                .is_some_and(|filter| filter.embedded_only),
            crate::stream::CryptSelection::Default => embedded && self.embedded_authorization,
            crate::stream::CryptSelection::Identity => false,
        };
        let method = match explicit {
            crate::stream::CryptSelection::Named(name) => {
                lookup(&self.filters, name.as_bytes(), embedded)?
            }
            crate::stream::CryptSelection::Identity => CryptMethod::Identity,
            crate::stream::CryptSelection::Default if embedded => self.embedded,
            crate::stream::CryptSelection::Default => self.streams,
        };
        if self.key.is_none() && authorization {
            return Err(
                Error::password("embedded file access requires an authenticated password").into(),
            );
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
            (Some(key), CryptMethod::Rc4) if self.derivation == padding::Derivation::ZeroPadded => {
                key.decrypt_legacy_zero_padded(id, data)
            }
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
        decrypt_strings(key, self.strings, self.derivation, id, &mut object.value)?;
        for direct in object.direct_objects_mut() {
            decrypt_strings(key, self.strings, self.derivation, id, &mut direct.value)?;
        }
        Ok(())
    }
}

fn decrypt_strings(
    key: &FileKey,
    method: CryptMethod,
    derivation: padding::Derivation,
    id: ObjectId,
    value: &mut Value,
) -> Result<()> {
    match value {
        Value::String(bytes) => {
            *bytes = if derivation == padding::Derivation::ZeroPadded && method == CryptMethod::Rc4
            {
                key.decrypt_legacy_zero_padded(id, bytes)
            } else {
                key.decrypt(method, id, bytes)
            }
            .map_err(|error| error.in_object(id))?;
        }
        Value::Array(values) => {
            for value in values {
                decrypt_strings(key, method, derivation, id, value)?;
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
                decrypt_strings(key, method, derivation, id, value)?;
            }
        }
        _ => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests;

impl Document {
    /// Creates a new immutable snapshot authenticated with `password` (7.6).
    /// Unlike ordinary opening, this operation refuses an incorrect EFOpen-only
    /// password immediately. Existing caches and authorization remain untouched.
    /// Password bytes are not retained; permissions remain a separate host policy.
    ///
    /// # Errors
    /// Opening/budget errors, or `Password` when credentials do not authenticate.
    pub fn reauthenticate(&self, password: &[u8]) -> ReadResult<Self> {
        self.bytes.check()?;
        if password.len() > self.limits.string_bytes {
            return Err(Error::limit("password byte budget exceeded").into());
        }
        let options = crate::OpenOptions {
            limits: self.limits,
            recovery: self.recovery,
            password: password.to_vec(),
        };
        let authenticated = crate::builder::open(self.bytes.clone(), &options)?;
        if authenticated
            .security
            .as_ref()
            .is_some_and(|security| !security.authenticated())
        {
            return Err(Error::password("password did not authorize embedded file access").into());
        }
        Ok(authenticated)
    }
}
