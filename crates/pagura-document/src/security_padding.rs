//! Short RC4 object-key recovery, departing from 7.6.3.2 Algorithm 1.
//!
//! Decide before any encrypted non-security object is cached. A complete zlib
//! checksum and xref-consistent object directory are the witness, never a random
//! raw-DEFLATE prefix or a guessed string plaintext. Only one physical stream
//! is probed, and probe storage obeys the decoded/retained byte limits.

use crate::{Document, Location, object, object_stream};
use pagura_codec::{Filter, FilterChain};
use pagura_core::{Diagnostic, Diagnostics, Error, ErrorKind, ObjectId, Recovery, Result};
use pagura_crypto::standard::{CryptMethod, FileKey};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Derivation {
    Standard,
    ZeroPadded,
}

pub(super) fn select(
    document: &Document,
    key: Option<&FileKey>,
    short: bool,
    methods: (CryptMethod, CryptMethod),
    policy: &mut crate::security_dictionary::Policy,
) -> Result<Derivation> {
    if short
        && methods == (CryptMethod::Rc4, CryptMethod::Rc4)
        && let Some(key) = key
        && let Some(witness) = detect(document, key)?
    {
        let mut diagnostic = Diagnostic::new(
            crate::tags::KEY_PADDING,
            "authenticated short RC4 file key zero-padded for xref-validated object decryption",
        )
        .in_object(witness);
        if let Some(crate::Slot {
            location: Location::Offset(offset),
            ..
        }) = document.slots.get(&witness)
        {
            diagnostic = diagnostic.at(pagura_core::Position::File(*offset));
        }
        policy.diagnostics.repair(diagnostic)?;
        return Ok(Derivation::ZeroPadded);
    }
    Ok(Derivation::Standard)
}

fn detect(document: &Document, key: &FileKey) -> Result<Option<ObjectId>> {
    let Some(&number) = document.object_streams.keys().next() else {
        return Ok(None);
    };
    let id = ObjectId::new(number, 0);
    let Some(slot) = document.slots.get(&id) else {
        return Ok(None);
    };
    let Location::Offset(offset) = slot.location else {
        return Ok(None);
    };
    // This parse is deliberately not published: encrypted strings, decoded
    // contents and any failure must not enter a cache under an undecided key.
    let (object, _) = object::parse_at(document, id, offset, &mut vec![id])?;
    let Some(dictionary) = object.dictionary() else {
        return Ok(None);
    };
    if !dictionary.is_name(b"Type", b"ObjStm") || dictionary.contains_key(b"F") {
        return Ok(None);
    }
    let chain = match FilterChain::from_entries(
        dictionary.get(b"Filter"),
        dictionary.get(b"DecodeParms"),
    ) {
        Ok(chain) if matches!(chain.filters(), [Filter::Flate { .. }]) => chain,
        Err(error) if error.kind == ErrorKind::Limit => return Err(error.in_object(id)),
        Ok(_) | Err(_) => return Ok(None),
    };
    let raw = document.raw_stream_data(&object)?;
    let maximum = document
        .limits()
        .decoded_bytes
        .min(document.limits().document_bytes);
    let valid = |padded| -> Result<bool> {
        let decoded = if padded {
            key.decrypt_legacy_zero_padded(id, raw)?
        } else {
            key.decrypt(CryptMethod::Rc4, id, raw)?
        };
        let mut diagnostics = Diagnostics::new(Recovery::Strict);
        let result = chain
            .decode(&decoded, maximum, &mut diagnostics)
            .and_then(|data| {
                let contents = object_stream::headers(
                    data.into_owned(),
                    dictionary,
                    document.limits(),
                    id,
                    &mut diagnostics,
                )?;
                if contents.retained_bytes() > document.limits().document_bytes {
                    return Err(
                        Error::limit("key recovery object stream exceeds retained budget")
                            .in_object(id),
                    );
                }
                let indexes: std::collections::BTreeMap<_, _> = contents.identities().collect();
                let mut witness = false;
                for (object_id, slot) in &document.slots {
                    if let Location::Compressed { stream, index } = slot.location
                        && stream == number
                    {
                        if indexes.get(&object_id.number) != Some(&index) {
                            return Ok(false);
                        }
                        witness = true;
                    }
                }
                Ok(witness)
            });
        match result {
            Ok(valid) => Ok(valid),
            Err(error) if error.kind == ErrorKind::Limit => Err(error.in_object(id)),
            Err(_) => Ok(false),
        }
    };
    if valid(false)? || !valid(true)? {
        return Ok(None);
    }
    Ok(Some(id))
}
