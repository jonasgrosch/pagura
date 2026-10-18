//! Field discovery through the read-only form model and typed signature views.
use super::{
    AlgorithmStatus, Integrity, PermittedModifications, RevisionCoverage, RevocationStatus,
    Signature, SignatureField, SignatureResults, Signatures, SignerTrust, SubFilter,
    ValidationTime, byte_range, revisions, transforms,
};
use crate::{
    Document,
    forms::{Field, FieldKind, FieldValue},
    navigation::context::{Context, Source},
};
use pagura_core::{Dictionary, Error, Result};

struct Reader<'a> {
    ctx: Context<'a>,
    scanned: usize,
    boundaries: Option<Vec<Option<revisions::Boundary>>>,
}
pub(super) fn read(doc: &Document) -> Result<Signatures<'_>> {
    let form = doc.acro_form()?;
    let mut reader = Reader {
        ctx: Context::new(doc),
        scanned: 0,
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
            if widget_counts[index] > 1 {
                return Err(source.error(Error::syntax(
                    "signature field cannot have multiple widgets",
                )));
            }
            reader.ctx.allocate::<SignatureField<'_>>(1, source)?;
            reader
                .ctx
                .allocate::<u8>(field.qualified_name.len(), source)?;
            let lock = reader
                .ctx
                .present(field.dictionary, b"Lock", source)?
                .map(|value| {
                    if value.as_reference().is_none() {
                        return Err(
                            source.error(Error::syntax("signature field Lock must be indirect"))
                        );
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
            fields.push(SignatureField {
                object: field.object,
                dictionary: field.dictionary,
                qualified_name: field.qualified_name.clone(),
                lock,
                signature,
            });
        }
    }
    let (catalog, source) = reader.ctx.catalog()?;
    let usage_rights = if let Some(perms) = reader.ctx.present(catalog, b"Perms", source)? {
        let (perms, source) = reader.ctx.dictionary(perms, source)?;
        reader
            .ctx
            .present(perms, b"UR3", source)?
            .map(|value| {
                let (dictionary, source) = reader.ctx.dictionary(value, source)?;
                reader.signature(dictionary, source)
            })
            .transpose()?
    } else {
        None
    };
    let mut result = Signatures {
        fields,
        usage_rights,
    };
    // Later lazy loads can discover repairs. Make every coverage result
    // conservative after all source dictionaries have been inspected.
    if !doc.diagnostics().is_empty() || doc.suppressed_diagnostics() != 0 {
        for signature in result
            .fields
            .iter_mut()
            .filter_map(|f| f.signature.as_mut())
            .chain(result.usage_rights.iter_mut())
        {
            signature.results.revision_coverage = RevisionCoverage::RepairedDocument;
        }
    }
    Ok(result)
}

fn origin<'a>(ctx: &mut Context<'a>, fields: &[Field<'a>], mut index: usize) -> Result<Source> {
    for depth in 0..=ctx.doc.limits().depth {
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
    Err(Error::syntax("signature field lacks a source value"))
}

impl<'a> Reader<'a> {
    fn signature(&mut self, d: &'a Dictionary, source: Source) -> Result<Signature<'a>> {
        self.ctx.step(0, source)?;
        if transforms::name(d, b"Type", source)?
            .is_some_and(|n| !matches!(n.as_bytes(), b"Sig" | b"DocTimeStamp"))
        {
            return Err(source.error(Error::syntax("signature Type must be Sig or DocTimeStamp")));
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
        let checked = byte_range::check(&mut self.ctx, d, source, &mut self.scanned)?;
        let (byte_range, integrity, revision_coverage) = match checked {
            Ok(range) => {
                if self.boundaries.is_none() {
                    self.boundaries = Some(revisions::read(&mut self.ctx, &mut self.scanned)?);
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
