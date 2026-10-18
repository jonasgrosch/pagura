//! Host-anchored certification paths (RFC 5280 §6, updated by RFC 9618).
//!
//! Certificates supply assertions, never trust anchors. Signature verification
//! is supplied by the host; no algorithm, clock, trust store or network is used.
//! Basic validation and revocation are separate, explicit results.

mod budget;
mod build;
mod extensions;
mod names;
mod path;
mod policy;
pub mod revocation;

use crate::x509::{AlgorithmIdentifier, Certificate, Name, SubjectPublicKeyInfo};
use crate::{BitString, ObjectIdentifier, Time, der::Element};
use pagura_core::Limits;

pub use build::build_paths;
pub use path::validate_path;

/// DER OBJECT IDENTIFIER contents, copied independently of an extension index.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Oid(Vec<u8>);

impl Oid {
    /// Makes an owned policy identifier from an already parsed OID.
    pub fn new(oid: ObjectIdentifier<'_>) -> Self {
        Self(oid.as_bytes().to_vec())
    }
    /// anyPolicy (2.5.29.32.0), also denoting an unrestricted initial policy set.
    pub fn any() -> Self {
        Self(vec![85, 29, 32, 0])
    }
    /// Original, canonical OID content octets.
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}

/// Trust information explicitly selected by the host (RFC 5280 §6.1.1(d)).
/// The container certificate's signature, validity and extensions are irrelevant.
#[derive(Debug, Clone, Copy)]
pub struct TrustAnchor<'a> {
    /// Trusted issuer distinguished name.
    pub name: Name<'a>,
    /// Trusted issuer public key and its parameters.
    pub public_key: SubjectPublicKeyInfo<'a>,
}

impl<'a> TrustAnchor<'a> {
    /// Extracts name/key from a certificate explicitly trusted by the caller.
    /// This is never called by path discovery for an embedded certificate.
    pub const fn from_certificate(certificate: &Certificate<'a>) -> Self {
        Self {
            name: certificate.subject,
            public_key: certificate.public_key,
        }
    }
}

/// A working public key, including inherited parameters (§6.1.4(d)).
#[derive(Debug, Clone, Copy)]
pub struct WorkingPublicKey<'a> {
    /// Public key algorithm OID.
    pub algorithm: ObjectIdentifier<'a>,
    /// Effective parameters, inherited only when the algorithm remains the same.
    pub parameters: Option<Element<'a>>,
    /// Original subjectPublicKey bits.
    pub key: BitString<'a>,
}

/// Exact signed bytes and effective issuer key for a single verification.
#[derive(Debug, Clone, Copy)]
pub struct SignatureInput<'a> {
    /// Signed object signature algorithm, with exact parameter encoding.
    pub algorithm: AlgorithmIdentifier<'a>,
    /// Entire original signed DER TLV (TBSCertificate or TBSCertList).
    pub message: &'a [u8],
    /// Original signature bits, including the unused-bit count in the view.
    pub signature: BitString<'a>,
    /// Effective issuer key, after parameter inheritance.
    pub issuer_key: WorkingPublicKey<'a>,
}

/// A verifier must distinguish a bad signature from unavailable verification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignatureResult {
    /// The signature is mathematically verified for these exact inputs.
    Verified,
    /// These inputs have an invalid signature or invalid algorithm parameters.
    Invalid,
    /// No supported implementation or recorded result exists for these inputs.
    Unavailable,
}

/// Pluggable algorithm implementation. Hosts must bound each invocation's work;
/// the engine bounds the number of calls and supplies the operation limits.
pub trait SignatureVerifier {
    /// Verifies one exact signature input.
    fn verify(&mut self, input: SignatureInput<'_>, limits: &Limits) -> SignatureResult;
}

impl<F> SignatureVerifier for F
where
    F: FnMut(SignatureInput<'_>, &Limits) -> SignatureResult,
{
    fn verify(&mut self, input: SignatureInput<'_>, limits: &Limits) -> SignatureResult {
        self(input, limits)
    }
}

/// Name namespaces supported by the initial subtree inputs (§6.1.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NameForm {
    /// Distinguished names in the subject or SAN.
    Directory,
    /// dNSName.
    Dns,
    /// Both rfc822Name and SmtpUTF8Mailbox (RFC 9598 §6).
    Mailbox,
    /// uniformResourceIdentifier.
    Uri,
    /// Both IPv4 and IPv6 iPAddress values.
    Ip,
}
impl NameForm {
    fn number(self) -> u32 {
        match self {
            Self::Directory => 4,
            Self::Dns => 2,
            Self::Mailbox => 1,
            Self::Uri => 6,
            Self::Ip => 7,
        }
    }
}

/// Requested path discovery behavior.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchMode {
    /// Stop at the first path whose requested checks succeed.
    FirstAcceptable,
    /// Enumerate every branch within the shared budget.
    All,
}

/// Caller inputs, independent of certificate assertions (§6.1.1).
#[derive(Debug, Clone)]
pub struct ValidationOptions<'a> {
    /// Evaluation time supplied by the caller, never read from a system clock.
    pub time: Time<'a>,
    /// Explore every branch instead of stopping at the first acceptable path.
    pub search: SearchMode,
    /// Initial policy set; an empty set means no policies, not anyPolicy.
    pub policies: Vec<Oid>,
    /// Require a policy path from the start.
    pub explicit_policy: bool,
    /// Inhibit policy mappings from the start.
    pub inhibit_mapping: bool,
    /// Inhibit anyPolicy from the start.
    pub inhibit_any_policy: bool,
    /// Optional intended EKU OID. Applied to every certificate that asserts EKU.
    pub extended_key_usage: Option<Oid>,
    /// Required target keyUsage bits, numbered as in RFC 5280 §4.2.1.3.
    /// An absent keyUsage extension imposes no restriction. Known EKU purposes
    /// additionally require a compatible usage; private purposes need this mask.
    pub key_usage: u16,
    /// Optional host name constraints, using a parsed nameConstraints extension
    /// value. They constrain every certificate, including the first one.
    pub name_constraints: Option<Element<'a>>,
    /// Initial empty permitted sets for these forms (§6.1.2(b)); all names of
    /// each listed form are disallowed. Other forms remain unconstrained.
    pub denied_name_forms: Vec<NameForm>,
}

impl<'a> ValidationOptions<'a> {
    /// Default policy inputs (§6.1.1), with an explicit evaluation time.
    pub fn new(time: Time<'a>) -> Self {
        Self {
            time,
            search: SearchMode::FirstAcceptable,
            policies: vec![Oid::any()],
            explicit_policy: false,
            inhibit_mapping: false,
            inhibit_any_policy: false,
            extended_key_usage: None,
            key_usage: 0,
            name_constraints: None,
            denied_name_forms: Vec::new(),
        }
    }
}

/// Definitive basic-path or authenticated revocation failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InvalidReason {
    /// Signature verification explicitly failed.
    Signature,
    /// Current issuer name does not match the working issuer name.
    IssuerName,
    /// Evaluation precedes notBefore.
    NotYetValid,
    /// Evaluation exceeds notAfter.
    Expired,
    /// An intermediate certificate is not a CA.
    NotCa,
    /// An intermediate keyUsage excludes keyCertSign.
    KeyCertSign,
    /// A target keyUsage excludes a requested usage.
    KeyUsage,
    /// An asserted EKU excludes the requested purpose.
    ExtendedKeyUsage,
    /// A non-self-issued intermediate exceeds pathLenConstraint.
    PathLength,
    /// A subject name lies outside the permitted subtrees or in an exclusion.
    NameConstraint,
    /// No policy survives when an explicit policy is required.
    Policy,
    /// anyPolicy occurs in a policy mapping.
    AnyPolicyMapping,
    /// Extension syntax or profile is invalid; includes its OID content bytes.
    Extension(Vec<u8>),
    /// A critical extension cannot be processed; includes its OID.
    CriticalExtension(Vec<u8>),
    /// No caller-supplied anchor or issuer completes this path.
    Untrusted,
    /// Authenticated offline evidence identifies a revoked certificate.
    Revoked,
    /// No target certificate was supplied, or an index is outside the pool.
    Input,
}

/// Reasons a path cannot yet receive a definitive positive result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Uncertainty {
    /// Signature verifier returned no decision.
    Signature,
    /// A name uses a matching rule or repertoire this implementation lacks.
    NameForm,
    /// A private EKU needs caller-specified keyUsage requirements.
    KeyUsagePurpose,
    /// A bound prevented complete validation or path enumeration.
    Limit(&'static str),
    /// Revocation validation is a separate stage, absent or incomplete here.
    Revocation,
    /// Offline evidence could not establish a certificate status.
    RevocationData(revocation::UnknownReason),
}

/// A precise result, never a single boolean.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathState {
    /// All checks represented by this state completed successfully.
    Valid,
    /// A definitive failure, indexed from the anchor's child (zero).
    Invalid {
        /// Certificate position, or None for path inputs/anchor constraints.
        certificate: Option<usize>,
        /// Why validation failed.
        reason: InvalidReason,
    },
    /// Validation could not be completed.
    Indeterminate {
        /// Certificate position, if known.
        certificate: Option<usize>,
        /// What remains unresolved.
        reason: Uncertainty,
    },
}

/// Effective target key after RFC 5280 §6.1.4 parameter inheritance.
/// Its authentication depends on the associated path and revocation states.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectivePublicKey {
    /// Algorithm OID contents.
    pub algorithm: Vec<u8>,
    /// Effective DER parameters, or None when unavailable/not applicable.
    pub parameters: Option<Vec<u8>>,
    /// subjectPublicKey octets, excluding the BIT STRING padding-count byte.
    pub key: Vec<u8>,
    /// Number of unused bits in the final key octet.
    pub unused_bits: u8,
}

/// One path's basic result and policy output. Revocation starts indeterminate.
#[derive(Debug, Clone)]
#[must_use]
pub struct PathReport {
    /// Basic validation (§6.1), without revocation (§6.3).
    basic: PathState,
    /// Final result including the separately supplied revocation evidence.
    state: PathState,
    /// Surviving user policies under RFC 9618. Qualifiers are not aggregated.
    pub policies: Vec<Oid>,
    /// Number of certificates, excluding the trust anchor.
    pub certificates: usize,
    /// Effective target key when basic processing reached its end. This can
    /// accompany an indeterminate signature result and does not itself assert trust.
    pub effective_public_key: Option<EffectivePublicKey>,
    /// Offline outcomes in root-to-target order; empty before evidence processing.
    revocation: Vec<revocation::RevocationOutcome>,
}

impl PathReport {
    /// Basic certification-path result, independent of revocation.
    pub const fn basic(&self) -> &PathState {
        &self.basic
    }
    /// Final result, including revocation evidence when processed.
    pub const fn state(&self) -> &PathState {
        &self.state
    }
    /// Offline certificate outcomes in root-to-target order.
    pub fn revocation(&self) -> &[revocation::RevocationOutcome] {
        &self.revocation
    }
}

/// One candidate discovered from the caller's certificate pool.
#[derive(Debug, Clone)]
pub struct BuiltPath {
    /// Certificate pool indices in root-to-target order, anchor excluded.
    pub certificates: Vec<usize>,
    /// Caller anchor index, absent for an untrusted dead end.
    pub anchor: Option<usize>,
    /// Independent result for this path.
    pub report: PathReport,
}

/// Bounded enumeration; complete=false explicitly prevents an exhaustive claim.
#[derive(Debug, Clone)]
#[must_use]
pub struct PathSearch {
    /// Completed paths and untrusted dead ends, each with its own result.
    pub paths: Vec<BuiltPath>,
    /// Whether the requested search completed without a resource/feature stop.
    pub complete: bool,
    /// Whether every branch was explored; false after the default early success.
    pub exhaustive: bool,
    /// A bound that stopped enumeration, if any.
    pub stopped: Option<Uncertainty>,
}

#[derive(Debug)]
enum Issue {
    Invalid(InvalidReason),
    Unknown(Uncertainty),
}

type Check<T> = Result<T, Issue>;

#[cfg(test)]
mod tests;
