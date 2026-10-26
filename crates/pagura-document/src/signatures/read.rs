//! Field discovery through the read-only form model and typed signature views.
use super::{
    AlgorithmStatus, Integrity, PermittedModifications, RevisionCoverage, RevocationStatus,
    Signature, SignatureField, SignatureResults, Signatures, SignerTrust, SubFilter,
    ValidationTime, byte_range, revisions, transforms,
};
use crate::source::{ReadFault, ReadResult};
use crate::{
    Document,
    forms::{Field, FieldKind, FieldValue},
    view::{Context, Source},
};
use pagura_core::{Dictionary, Error};

struct Reader<'a> {
    ctx: Context<'a>,
    boundaries: Option<Vec<Option<revisions::Boundary>>>,
}
pub(super) fn read<'a>(doc: &'a Document, ctx: &Context<'_>) -> ReadResult<Signatures<'a>> {
    let form = doc.acro_form()?;
    let mut reader = Reader {
        ctx: ctx.for_document(doc),
        boundaries: None,
    };
    let mut fields = Vec::new();
    if let Some(form) = form {
        reader
            .ctx
            .allocate::<usize>(form.fields.len(), Source::default())?;
        let mut widget_counts = vec![0usize; form.fields.len()];
        for widget in &form.widgets {
            let source = Source {
                object: widget.object,
                position: widget.position,
            };
            reader.ctx.step(0, source)?;
            widget_counts[widget.field] = widget_counts[widget.field].saturating_add(1);
        }
        for (index, field) in form
            .fields
            .iter()
            .enumerate()
            .filter(|(_, f)| matches!(f.kind, FieldKind::Signature { .. }))
        {
            let source = Source {
                object: field.object,
                position: field.position,
            };
            reader.ctx.step(0, source)?;
            reader.ctx.allocate::<SignatureField<'_>>(1, source)?;
            reader
                .ctx
                .allocate::<u8>(field.qualified_name.len(), source)?;
            let parsed = (|| {
                if widget_counts[index] > 1 {
                    return Err((source.error(Error::syntax(
                        "signature field cannot have multiple widgets",
                    )))
                    .into());
                }
                let lock = reader
                    .ctx
                    .present(field.dictionary, b"Lock", source)?
                    .map(|value| {
                        if value.as_reference().is_none() {
                            return Err(source
                                .error(Error::syntax("signature field Lock must be indirect")));
                        }
                        let (d, source) = reader.ctx.dictionary(value, source)?;
                        transforms::lock(&mut reader.ctx, d, source)
                    })
                    .transpose()?;
                let signature = if let Some(FieldValue::Signature(dictionary)) = &field.value {
                    let origin = origin(&mut reader.ctx, &form.fields, index)?;
                    Some(reader.signature(dictionary, origin)?)
                } else {
                    None
                };
                Ok((lock, signature))
            })();
            let (lock, signature, error) = match parsed {
                Ok((lock, signature)) => (lock, signature, None),
                Err(ReadFault::Invalid(error)) if error.kind == pagura_core::ErrorKind::Limit => {
                    return Err(error.into());
                }
                Err(ReadFault::Invalid(error)) => (None, None, Some(error)),
                Err(unavailable) => return Err(unavailable),
            };
            fields.push(SignatureField {
                object: field.object,
                dictionary: field.dictionary,
                qualified_name: field.qualified_name.clone(),
                lock,
                error,
                signature,
            });
        }
    }
    let (usage_rights, usage_rights_error) = reader.catalog_signature(b"UR3")?;
    let (certification, certification_error) = reader.catalog_signature(b"DocMDP")?;
    let mut result = Signatures {
        fields,
        certification,
        certification_error,
        usage_rights_error,
        usage_rights,
        historical: Vec::new(),
        history_complete: true,
    };
    // Later lazy loads can discover repairs. Make every coverage result
    // conservative after all source dictionaries have been inspected.
    if !doc.diagnostics().is_empty() || doc.suppressed_diagnostics() != 0 {
        for signature in result
            .fields
            .iter_mut()
            .filter_map(|f| f.signature.as_mut())
            .chain(result.usage_rights.iter_mut())
            .chain(result.certification.iter_mut())
        {
            signature.results.revision_coverage = RevisionCoverage::RepairedDocument;
        }
    }
    Ok(result)
}

fn origin<'a>(ctx: &mut Context<'a>, fields: &[Field<'a>], mut index: usize) -> ReadResult<Source> {
    for depth in 0..=ctx.document.limits().depth {
        let field = &fields[index];
        let source = Source {
            object: field.object,
            position: field.position,
        };
        ctx.step(depth, source)?;
        if let Some(value) = field.dictionary.get(b"V") {
            let (value, source) = ctx.resolve(value, source)?;
            if !value.is_null() {
                return Ok(source);
            }
        }
        if let Some(parent) = field.parent {
            index = parent;
        } else {
            break;
        }
    }
    Err((Error::syntax("signature field lacks a source value")).into())
}

impl<'a> Reader<'a> {
    fn catalog_signature(
        &mut self,
        key: &[u8],
    ) -> ReadResult<(Option<Signature<'a>>, Option<Error>)> {
        let parsed = (|| {
            let (catalog, source) = self.ctx.catalog()?;
            let Some(perms) = self.ctx.present(catalog, b"Perms", source)? else {
                return Ok(None);
            };
            let (perms, source) = self.ctx.dictionary(perms, source)?;
            self.ctx
                .present(perms, key, source)?
                .map(|value| {
                    let (dictionary, source) = self.ctx.dictionary(value, source)?;
                    self.signature(dictionary, source)
                })
                .transpose()
        })();
        match parsed {
            Ok(signature) => Ok((signature, None)),
            Err(ReadFault::Invalid(error)) if error.kind == pagura_core::ErrorKind::Limit => {
                Err(error.into())
            }
            Err(ReadFault::Invalid(error)) => Ok((None, Some(error))),
            Err(unavailable) => Err(unavailable),
        }
    }

    fn signature(&mut self, d: &'a Dictionary, source: Source) -> ReadResult<Signature<'a>> {
        self.ctx.step(0, source)?;
        if transforms::name(d, b"Type", source)?
            .is_some_and(|n| !matches!(n.as_bytes(), b"Sig" | b"DocTimeStamp"))
        {
            return Err((source
                .error(Error::syntax("signature Type must be Sig or DocTimeStamp")))
            .into());
        }
        let filter = transforms::name(d, b"Filter", source)?
            .ok_or_else(|| source.error(Error::syntax("signature requires Filter")))?;
        let contents = transforms::string(d, b"Contents", source)?
            .ok_or_else(|| source.error(Error::syntax("signature requires Contents")))?;
        let sub_filter = match transforms::name(d, b"SubFilter", source)? {
            Some(n) => match n.as_bytes() {
                b"adbe.pkcs7.detached" => SubFilter::Pkcs7Detached,
                b"ETSI.CAdES.detached" => SubFilter::CadesDetached,
                b"ETSI.RFC3161" => SubFilter::Rfc3161,
                b"adbe.pkcs7.sha1" => SubFilter::Pkcs7Sha1,
                b"adbe.x509.rsa_sha1" => SubFilter::X509RsaSha1,
                _ => SubFilter::Other(n),
            },
            None => SubFilter::Unspecified,
        };
        let checked = byte_range::check(&mut self.ctx, d, source)?;
        let (byte_range, integrity, revision_coverage) = match checked {
            Ok(range) => {
                if self.boundaries.is_none() {
                    self.boundaries = Some(revisions::read(&mut self.ctx)?);
                }
                let coverage = revisions::coverage(
                    &self.ctx,
                    self.boundaries.as_deref().unwrap_or_default(),
                    range.signed_end(),
                );
                (
                    Some(range),
                    Integrity::ByteRangeCheckedCryptographyNotChecked,
                    coverage,
                )
            }
            Err(error) => (
                None,
                Integrity::InvalidByteRange(error),
                RevisionCoverage::NotCheckedYet,
            ),
        };
        let algorithms = match sub_filter {
            SubFilter::Pkcs7Sha1 => AlgorithmStatus::WeakSha1Unsupported,
            SubFilter::X509RsaSha1 => AlgorithmStatus::LegacyEncodingUnsupported,
            SubFilter::Other(_) | SubFilter::Unspecified => AlgorithmStatus::UnsupportedEncoding,
            _ => AlgorithmStatus::NotCheckedYet,
        };
        Ok(Signature {
            dictionary: d,
            object: source.object,
            position: source.position,
            filter,
            sub_filter,
            contents,
            byte_range,
            references: transforms::references(&mut self.ctx, d, source)?,
            signing_time: transforms::string(d, b"M", source)?,
            name: transforms::string(d, b"Name", source)?,
            reason: transforms::string(d, b"Reason", source)?,
            location: transforms::string(d, b"Location", source)?,
            results: SignatureResults {
                integrity,
                error: None,
                diagnostics: Vec::new(),
                certificate_errors: Vec::new(),
                revision_coverage,
                permitted_modifications: PermittedModifications::NotAnalysedYet,
                signer_trust: SignerTrust::NotCheckedYet,
                validation_time: ValidationTime::NotCheckedYet,
                revocation: RevocationStatus::NotCheckedYet,
                algorithms,
            },
        })
    }
}
