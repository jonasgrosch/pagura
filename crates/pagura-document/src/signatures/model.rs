//! Borrowed PDF signature data and separate validation aspects (ADR 0015).
use super::{ByteRange, ByteRangeError, PermittedModifications};
use crate::Revision;
use pagura_core::{Diagnostic, Dictionary, Error, Name, ObjectId, Position, Value};

/// Signature encoding (Table 255). Recognition does not imply verification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum SubFilter<'a> {
    /// Detached CMS signature.
    Pkcs7Detached,
    /// Detached CAdES signature.
    CadesDetached,
    /// RFC 3161 document timestamp token.
    Rfc3161,
    /// Deprecated encapsulated SHA-1 digest; weak and unsupported here.
    Pkcs7Sha1,
    /// Deprecated PKCS #1 encoding; unsupported here (Table 260 note a).
    X509RsaSha1,
    /// An unrecognized encoding, preserved as data.
    Other(&'a Name),
    /// No encoding specified; no handler is inferred.
    Unspecified,
}
/// Document modification permission (Tables 236 and 257).
/// DSS/timestamp exceptions in 12.8.2.2 still require modification analysis.
///
/// Tables 257 and 259 define exactly these three values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MdpPermission {
    /// Permission 1.
    NoChanges,
    /// Permission 2 (the DocMDP default).
    FormsAndSignatures,
    /// Permission 3.
    FormsSignaturesAndAnnotations,
}
/// Field selection (Tables 236 and 259). Names retain PDF text-string encoding.
/// The tables' Action entry has exactly these three values.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FieldSelection<'a> {
    /// All fields.
    All,
    /// Only the named fields.
    Include(Vec<&'a [u8]>),
    /// All fields except the named fields.
    Exclude(Vec<&'a [u8]>),
}
/// Signature field lock dictionary (Table 236); analysis freezes signed locks.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct FieldLock<'a> {
    /// Unmodified source dictionary.
    pub dictionary: &'a Dictionary,
    /// Fields affected by this lock.
    pub fields: FieldSelection<'a>,
    /// Optional restriction; unlike DocMDP there is no default here.
    pub permission: Option<MdpPermission>,
}
/// A modification transform; its presence never proves changes are permitted.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum Transform<'a> {
    /// Certification signature permissions (12.8.2.2).
    DocMdp(MdpPermission),
    /// Locked fields (12.8.2.4).
    FieldMdp(FieldSelection<'a>),
    /// Deprecated usage rights (12.8.2.3), not granted or evaluated.
    UsageRights,
    /// Extension transform, not evaluated.
    Other(&'a Name),
}
/// One Reference array entry (Table 256).
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct SignatureReference<'a> {
    /// Unmodified source dictionary, retaining Data, DigestMethod and extensions.
    pub dictionary: &'a Dictionary,
    /// Typed transform.
    pub transform: Transform<'a>,
    /// Unmodified transform parameters, when supplied.
    pub parameters: Option<&'a Dictionary>,
    /// Original Data reference; required for FieldMDP, never followed here.
    pub data: Option<&'a Value>,
}
/// Signed-byte integrity, separate from trust and algorithm policy (ADR 0015).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Integrity {
    /// No integrity checks performed.
    NotCheckedYet,
    /// Exactly two source-bound ranges checked; digest and CMS not checked yet.
    ByteRangeCheckedCryptographyNotChecked,
    /// Byte ranges, CMS signature and required attributes verified mathematically.
    Verified,
    /// Signed content, a CMS messageDigest or timestamp imprint differs.
    DigestMismatch,
    /// The signature equation or ESS signer-certificate binding failed.
    SignatureInvalid,
    /// No usable embedded certificate matches the signer identifier.
    SignerCertificateMissing,
    /// A digest, signature or public-key algorithm is unsupported.
    UnsupportedAlgorithm,
    /// The PDF sub-filter or an ASN.1 encoding is outside the implemented set.
    UnsupportedEncoding,
    /// Malformed CMS, parameters or signature encoding; see the detailed error.
    Malformed,
    /// An explicit parsing or verification budget was exhausted.
    ResourceLimit,
    /// A byte-range structure check failed.
    InvalidByteRange(ByteRangeError),
}
/// Revision coverage from the original cross-reference chain (7.5.6, 12.8.1).
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum RevisionCoverage<'a> {
    /// Cannot be checked before a usable byte range is established.
    NotCheckedYet,
    /// A complete revision boundary matched; no authentication is implied.
    Covered {
        /// Index in Document::revisions(), newest first.
        index: usize,
        /// Covered revision from the original Prev chain.
        revision: &'a Revision,
        /// Later revisions, newest first; empty for the latest revision.
        later_revisions: &'a [Revision],
    },
    /// The signed end does not match a confirmed revision boundary.
    NotRevisionBoundary,
    /// A Prev-chain revision terminator could not be confirmed from source.
    Indeterminate,
    /// Repaired structure is unsuitable for establishing revision coverage.
    RepairedDocument,
}
/// Trust must eventually be established from host-supplied anchors only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum SignerTrust {
    /// No certificate path or host trust anchors checked.
    NotCheckedYet,
}
/// Validation time is independent of the PDF's asserted signing time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ValidationTime {
    /// No host time/policy or timestamp token checked.
    NotCheckedYet,
}
/// Revocation evidence must be validated offline against a selected time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum RevocationStatus {
    /// No OCSP, CRL or DSS evidence checked; never implies good status.
    NotCheckedYet,
}
/// Algorithm policy, distinct from mathematical verification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum AlgorithmStatus {
    /// The encoding is recognized, but actual CMS algorithms are unchecked.
    NotCheckedYet,
    /// All examined algorithms are implemented; this is not a strength policy.
    Supported,
    /// SHA-1 was encountered in content, attributes, an imprint or a PSS mask.
    WeakSha1,
    /// At least one examined algorithm is unsupported.
    UnsupportedAlgorithm,
    /// An unsupported algorithm occurred alongside an examined SHA-1 hash.
    WeakSha1UnsupportedAlgorithm,
    /// Legacy SHA-1 encoding is weak and is not verified by this reader.
    WeakSha1Unsupported,
    /// Deprecated PKCS #1 encoding; its name alone does not identify the digest.
    LegacyEncodingUnsupported,
    /// Encoding is unknown or unspecified; no algorithm is inferred.
    UnsupportedEncoding,
}
/// The independent result aspects of ADR 0015 decision 2.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct SignatureResults<'a> {
    /// Signed-byte integrity.
    pub integrity: Integrity,
    /// Positioned failure when parsing or verification could not proceed.
    pub error: Option<Error>,
    /// Reported CMS BER deviations. No signed bytes are normalized.
    pub diagnostics: Vec<Diagnostic>,
    /// Malformed embedded certificate schemas or identifier extensions, retained
    /// while selecting a usable signer certificate.
    pub certificate_errors: Vec<Error>,
    /// Revision coverage.
    pub revision_coverage: RevisionCoverage<'a>,
    /// Permission analysis.
    pub permitted_modifications: PermittedModifications,
    /// Signer trust.
    pub signer_trust: SignerTrust,
    /// Validation time and timestamp verification.
    pub validation_time: ValidationTime,
    /// Revocation evidence.
    pub revocation: RevocationStatus,
    /// Algorithm policy.
    pub algorithms: AlgorithmStatus,
}
/// A signature dictionary (Table 255), with unauthenticated metadata.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct Signature<'a> {
    /// Original dictionary, including all unrecognized entries.
    pub dictionary: &'a Dictionary,
    /// Object containing this dictionary, when known.
    pub object: Option<ObjectId>,
    /// Source position.
    pub position: Position,
    /// Preferred handler, not loaded or executed.
    pub filter: &'a Name,
    /// Signature encoding.
    pub sub_filter: SubFilter<'a>,
    /// Decoded Contents, including zero padding (12.8.3.3.1).
    /// Presence alone authenticates nothing; consult the separate result states.
    pub contents: &'a [u8],
    /// Checked source ranges, or None when structure checks failed.
    pub byte_range: Option<ByteRange<'a>>,
    /// Modification-detection references, in source order.
    pub references: Vec<SignatureReference<'a>>,
    /// Asserted M date string; never a validated signing time.
    pub signing_time: Option<&'a [u8]>,
    /// Asserted Name in its original PDF text-string encoding.
    pub name: Option<&'a [u8]>,
    /// Asserted Reason in its original PDF text-string encoding.
    pub reason: Option<&'a [u8]>,
    /// Asserted Location in its original PDF text-string encoding.
    pub location: Option<&'a [u8]>,
    /// Independent result states; no unqualified validity flag exists.
    pub results: SignatureResults<'a>,
}
impl<'a> Signature<'a> {
    /// Extracted CMS bytes for a recognized CMS encoding, including PDF zero
    /// padding. The CMS reader must frame DER before interpreting them; trimming
    /// trailing zero octets would corrupt legitimate DER values. This accessor does
    /// not parse CMS or establish any CMS integrity (12.8.3.3.1).
    pub fn cms_blob(&self) -> Option<&'a [u8]> {
        matches!(
            self.sub_filter,
            SubFilter::Pkcs7Detached
                | SubFilter::CadesDetached
                | SubFilter::Rfc3161
                | SubFilter::Pkcs7Sha1
        )
        .then_some(self.contents)
    }
}
/// One signature field, including unsigned fields (12.7.5.5).
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct SignatureField<'a> {
    /// Field identity, independent of its display name.
    pub object: Option<ObjectId>,
    /// Unmodified field dictionary.
    pub dictionary: &'a Dictionary,
    /// Qualified display name from the form model; not a unique identity.
    pub qualified_name: String,
    /// Optional inert Lock dictionary.
    pub lock: Option<FieldLock<'a>>,
    /// A malformed field/lock/signature error, isolated from other signatures.
    /// When present, `signature == None` does not mean an unsigned field.
    pub error: Option<Error>,
    /// None for an empty or malformed field; consult `error` first.
    /// Appearances do not make a field signed.
    pub signature: Option<Signature<'a>>,
}
/// The signature fields and optional catalog usage-rights signature.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct Signatures<'a> {
    /// Terminal signature fields in form preorder, including empty fields.
    pub fields: Vec<SignatureField<'a>>,
    /// Certification signature linked directly from catalog Perms/DocMDP.
    /// Its object identity links it to a matching signature field, if any.
    pub certification: Option<Signature<'a>>,
    /// Error reading the catalog certification signature.
    pub certification_error: Option<Error>,
    /// Error reading the catalog usage-rights signature.
    pub usage_rights_error: Option<Error>,
    /// UR3 signature, which is referenced from Perms rather than a form field.
    pub usage_rights: Option<Signature<'a>>,
    /// Signatures found in earlier revisions but absent or replaced in the
    /// current field/UR3 views. Entries are flat and deduplicated by source.
    pub historical: Vec<HistoricalSignature<'a>>,
    /// False if any earlier revision could not be inspected without repair or
    /// unsupported source handling. An empty history then does not establish absence.
    pub history_complete: bool,
}
/// A signature hidden by a later field removal or replacement.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct HistoricalSignature<'a> {
    /// Revision in which this historical source view was found, newest first.
    pub revision: usize,
    /// Qualified name in that revision; None for catalog usage rights.
    pub field_name: Option<String>,
    /// Field identity in that revision, when applicable.
    pub field_object: Option<ObjectId>,
    /// Original signature with coverage indexes rebased to the full document.
    pub signature: Signature<'a>,
}
