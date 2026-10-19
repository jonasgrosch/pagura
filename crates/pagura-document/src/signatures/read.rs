//! Field discovery through the read-only form model and typed signature views.

use super::byte_range::{self, Scans};
use super::{
    AlgorithmStatus, FieldLock, Integrity, PermittedModifications, RevisionCoverage,
    RevocationStatus, Signature, SignatureField, SignatureResults, Signatures, SignerTrust,
    SubFilter, ValidationTime, revisions, transforms,
};
use crate::Document;
use crate::forms::{AcroForm, Field, FieldKind, FieldValue};
use crate::view::{Context, Source};
use pagura_core::{Dictionary, Error, ErrorKind, Result};

struct Reader<'a> {
    context: Context<'a>,
    scans: Scans,
    boundaries: Option<Vec<Option<revisions::Boundary>>>,
}

/// Reads `document`'s signatures, charging the work to `shared`'s budgets so
/// one inspection can read several revisions.
pub(super) fn read<'a>(document: &'a Document, shared: &Context<'_>) -> Result<Signatures<'a>> {
    let form = document.acro_form()?;
    let mut reader = Reader {
        context: shared.for_document(document),
        scans: Scans::default(),
        boundaries: None,
    };
    let fields = match &form {
        Some(form) => reader.fields(form)?,
        None => Vec::new(),
    };
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
    // Coverage needs the structure as the standard defines it. The evidence
    // is fixed at opening or found by this inspection, so the result does not
    // depend on what else was loaded or reported before, and information
    // diagnostics (such as font substitution) never count.
    let repaired = document.repaired_at_open()
        || form.as_ref().is_some_and(|form| form.repaired)
        || reader.context.repaired();
    if repaired {
        for signature in result
            .fields
            .iter_mut()
            .filter_map(|field| field.signature.as_mut())
            .chain(result.usage_rights.iter_mut())
            .chain(result.certification.iter_mut())
            .filter(|signature| signature.byte_range.is_some())
        {
            signature.results.revision_coverage = RevisionCoverage::RepairedDocument;
        }
    }
    Ok(result)
}

impl<'a> Reader<'a> {
    fn fields(&mut self, form: &AcroForm<'a>) -> Result<Vec<SignatureField<'a>>> {
        self.context
            .allocate::<usize>(form.fields.len(), Source::default())?;
        let mut widgets = vec![0usize; form.fields.len()];
        for widget in &form.widgets {
            self.context.step(
                0,
                Source {
                    object: widget.object,
                    position: widget.position,
                },
            )?;
            if let Some(count) = widgets.get_mut(widget.field) {
                *count = count.saturating_add(1);
            }
        }
        let mut fields = Vec::new();
        for (field, &widgets) in form.fields.iter().zip(&widgets) {
            if matches!(field.kind, FieldKind::Signature { .. }) {
                fields.push(self.field(field, widgets)?);
            }
        }
        Ok(fields)
    }

    /// One signature field. A malformed lock or signature becomes the field's
    /// error, so it hides no other signature; resource limits still end the
    /// inspection.
    fn field(&mut self, field: &Field<'a>, widgets: usize) -> Result<SignatureField<'a>> {
        let source = field.source();
        self.context.step(0, source)?;
        self.context.allocate::<SignatureField<'_>>(1, source)?;
        self.context
            .allocate::<u8>(field.qualified_name.len(), source)?;
        let (lock, signature, error) = match retained(self.entries(field, widgets, source))? {
            Ok((lock, signature)) => (lock, signature, None),
            Err(error) => (None, None, Some(error)),
        };
        Ok(SignatureField {
            object: field.object,
            dictionary: field.dictionary,
            qualified_name: field.qualified_name.clone(),
            lock,
            error,
            signature,
        })
    }

    fn entries(
        &mut self,
        field: &Field<'a>,
        widgets: usize,
        source: Source,
    ) -> Result<(Option<FieldLock<'a>>, Option<Signature<'a>>)> {
        if widgets > 1 {
            return Err(source.error(Error::syntax(
                "signature field cannot have multiple widgets",
            )));
        }
        let lock = match self.context.present(field.dictionary, b"Lock", source)? {
            Some(value) => {
                if value.as_reference().is_none() {
                    return Err(
                        source.error(Error::syntax("signature field Lock must be indirect"))
                    );
                }
                let (dictionary, source) = self.context.dictionary(value, source)?;
                Some(transforms::lock(&mut self.context, dictionary, source)?)
            }
            None => None,
        };
        let signature = match (&field.value, field.value_source) {
            (Some(FieldValue::Signature(dictionary)), Some(value_source)) => {
                Some(self.signature(dictionary, value_source)?)
            }
            _ => None,
        };
        Ok((lock, signature))
    }

    /// A signature linked from the catalog's `Perms`: `DocMDP` certification
    /// (12.8.2.2) or `UR3` usage rights (12.8.2.3). A malformed one is kept
    /// as its error.
    fn catalog_signature(&mut self, key: &[u8]) -> Result<(Option<Signature<'a>>, Option<Error>)> {
        Ok(match retained(self.permission(key))? {
            Ok(signature) => (signature, None),
            Err(error) => (None, Some(error)),
        })
    }

    fn permission(&mut self, key: &[u8]) -> Result<Option<Signature<'a>>> {
        let (catalog, source) = self.context.catalog()?;
        let Some(permissions) = self.context.present(catalog, b"Perms", source)? else {
            return Ok(None);
        };
        let (permissions, source) = self.context.dictionary(permissions, source)?;
        let Some(value) = self.context.present(permissions, key, source)? else {
            return Ok(None);
        };
        let (dictionary, source) = self.context.dictionary(value, source)?;
        self.signature(dictionary, source).map(Some)
    }

    fn signature(&mut self, dictionary: &'a Dictionary, source: Source) -> Result<Signature<'a>> {
        self.context.step(0, source)?;
        if transforms::name(dictionary, b"Type", source)?
            .is_some_and(|name| !matches!(name.as_bytes(), b"Sig" | b"DocTimeStamp"))
        {
            return Err(source.error(Error::syntax("signature Type must be Sig or DocTimeStamp")));
        }
        let filter = transforms::name(dictionary, b"Filter", source)?
            .ok_or_else(|| source.error(Error::syntax("signature requires Filter")))?;
        let contents = transforms::string(dictionary, b"Contents", source)?
            .ok_or_else(|| source.error(Error::syntax("signature requires Contents")))?;
        let sub_filter = match transforms::optional_name(dictionary, b"SubFilter", source)? {
            Some(name) => match name.as_bytes() {
                b"adbe.pkcs7.detached" => SubFilter::Pkcs7Detached,
                b"ETSI.CAdES.detached" => SubFilter::CadesDetached,
                b"ETSI.RFC3161" => SubFilter::Rfc3161,
                b"adbe.pkcs7.sha1" => SubFilter::Pkcs7Sha1,
                b"adbe.x509.rsa_sha1" => SubFilter::X509RsaSha1,
                _ => SubFilter::Other(name),
            },
            None => SubFilter::Unspecified,
        };
        let checked = byte_range::check(&mut self.context, dictionary, source, &mut self.scans)?;
        let (byte_range, integrity, revision_coverage) = match checked {
            Ok(range) => {
                if self.boundaries.is_none() {
                    self.boundaries = Some(revisions::read(&mut self.context)?);
                }
                let coverage = revisions::coverage(
                    &self.context,
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
            dictionary,
            object: source.object,
            position: source.position,
            filter,
            sub_filter,
            contents,
            byte_range,
            references: transforms::references(&mut self.context, dictionary, source)?,
            signing_time: transforms::optional_string(dictionary, b"M", source)?,
            name: transforms::optional_string(dictionary, b"Name", source)?,
            reason: transforms::optional_string(dictionary, b"Reason", source)?,
            location: transforms::optional_string(dictionary, b"Location", source)?,
            results: SignatureResults {
                integrity,
                error: None,
                diagnostics: Vec::new(),
                certificate_errors: Vec::new(),
                revision_coverage,
                permitted_modifications: PermittedModifications::NotAnalyzedYet,
                signer_trust: SignerTrust::NotCheckedYet,
                validation_time: ValidationTime::NotCheckedYet,
                revocation: RevocationStatus::NotCheckedYet,
                algorithms,
            },
        })
    }
}

/// Keeps a document error as a result; resource limits still end the read.
fn retained<T>(parsed: Result<T>) -> Result<Result<T>> {
    match parsed {
        Err(error) if error.kind == ErrorKind::Limit => Err(error),
        parsed => Ok(parsed),
    }
}
