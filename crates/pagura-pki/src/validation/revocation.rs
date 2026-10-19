//! Offline CRL validation (RFC 5280 §6.3). URLs are inert names, never fetched.
//!
//! Only supplied CRLs and certificates are considered. CRL issuers must reach
//! the same host anchor as the certificate path. Delta processing is enabled;
//! advertised but unavailable delta evidence leaves status unknown. No system
//! clock, operating-system trust store or network access participates.
pub(super) mod engine;
mod parse;
mod status;

use super::budget::Budget;
use super::{PathReport, PathSearch, SignatureVerifier, TrustAnchor, ValidationOptions};
use crate::Time;
use crate::x509::{Certificate, CertificateList};
use pagura_core::Limits;

/// RFC 5280 §5.3.1 reasonCode values. Value 7 is reserved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RevocationReason {
    /// No more specific reason was supplied.
    Unspecified,
    /// The subject's private key was compromised.
    KeyCompromise,
    /// The CA's private key was compromised.
    CaCompromise,
    /// Subject affiliation changed.
    AffiliationChanged,
    /// A replacement certificate superseded this certificate.
    Superseded,
    /// The subject ceased operation.
    CessationOfOperation,
    /// The certificate is temporarily on hold.
    CertificateHold,
    /// A delta releases a hold or removes an expired certificate.
    RemoveFromCrl,
    /// Privileges were withdrawn.
    PrivilegeWithdrawn,
    /// The attribute authority's key was compromised.
    AaCompromise,
}
impl RevocationReason {
    fn from_number(number: u8) -> Option<Self> {
        Some(match number {
            0 => Self::Unspecified,
            1 => Self::KeyCompromise,
            2 => Self::CaCompromise,
            3 => Self::AffiliationChanged,
            4 => Self::Superseded,
            5 => Self::CessationOfOperation,
            6 => Self::CertificateHold,
            8 => Self::RemoveFromCrl,
            9 => Self::PrivilegeWithdrawn,
            10 => Self::AaCompromise,
            _ => return None,
        })
    }
}

/// Why caller-supplied offline evidence does not establish non-revocation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnknownReason {
    /// No applicable revocation evidence was supplied.
    NoData,
    /// Evidence's nextUpdate has passed, or nextUpdate is absent.
    Stale,
    /// Evidence was issued after the caller's evaluation time.
    NotYetCurrent,
    /// Distribution point, certificate class or issuer is outside the CRL scope.
    OutOfScope,
    /// The available CRLs cover only some revocation reasons.
    IncompleteReasons,
    /// A delta cannot be paired with a suitable complete CRL.
    MissingBase,
    /// A freshestCRL advertisement has no usable current delta.
    MissingDelta,
    /// A signature was invalid or the verifier could not decide.
    VerificationFailed,
    /// No authorised, non-revoked CRL signer reaches the selected host anchor.
    SignerUntrusted,
    /// A critical CRL or entry extension cannot be processed.
    UnsupportedCriticalExtension,
    /// Evidence or its extension syntax violates the CRL profile.
    Malformed,
    /// Required name comparison uses an unsupported matching rule.
    NameForm,
    /// Shared resource budget was exhausted.
    Limit(&'static str),
}

/// Authenticated revocation outcome for one certificate at the caller's time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RevocationOutcome {
    /// Authenticated, current evidence covers every reason without revocation.
    Good,
    /// Authenticated evidence lists the certificate as revoked.
    Revoked {
        /// The asserted reason; absent reasonCode becomes Unspecified.
        reason: RevocationReason,
        /// CRL entry revocationDate, expressed in UTC with whole seconds.
        time: Time<'static>,
    },
    /// Supplied evidence cannot establish the status.
    Unknown(UnknownReason),
}

/// Validates basic checks and offline CRLs with one shared resource budget.
/// `path` is root-to-target, anchor excluded; `certificates` additionally supplies
/// any separate or indirect CRL signing certificates. Neither pool grants trust.
pub fn validate_path_with_crls(
    path: &[&Certificate<'_>],
    anchor: TrustAnchor<'_>,
    certificates: &[&Certificate<'_>],
    crls: &[&CertificateList<'_>],
    options: &ValidationOptions<'_>,
    verifier: &mut impl SignatureVerifier,
    limits: &Limits,
) -> PathReport {
    let mut budget = Budget::new(limits);
    let mut report = super::path::validate(path, anchor, options, verifier, &mut budget);
    let _ = engine::resolve(
        &mut report,
        path,
        engine::Evidence { certificates, crls },
        anchor,
        options,
        verifier,
        &mut budget,
    );
    report
}

/// Builds paths and checks supplied CRLs within the same enumeration budget.
/// Every candidate retains its own basic result, final state and certificate
/// outcomes. Incomplete searches retain the reason that stopped exploration.
pub fn build_paths_with_crls(
    target: usize,
    certificates: &[&Certificate<'_>],
    anchors: &[TrustAnchor<'_>],
    crls: &[&CertificateList<'_>],
    options: &ValidationOptions<'_>,
    verifier: &mut impl SignatureVerifier,
    limits: &Limits,
) -> PathSearch {
    let mut budget = Budget::new(limits);
    super::build::with_evidence(
        target,
        certificates,
        anchors,
        Some(crls),
        options,
        verifier,
        &mut budget,
    )
}

#[cfg(test)]
mod tests;
