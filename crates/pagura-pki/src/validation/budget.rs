//! The one budget a path validation or search charges, including its CRL
//! processing (`Limits::path_*`, ADR 0016).

use super::{Check, Issue, SignatureInput, SignatureResult, SignatureVerifier, Uncertainty};
use pagura_core::convert::u32_to_usize;
use pagura_core::{Limits, Meter, SignatureLimits};
use std::collections::BTreeMap;

/// The stable names `Uncertainty::Limit` reports for each exhausted bound.
const WORK: &str = "pki.path-work";
const NODES: &str = "pki.path-nodes";
const BYTES: &str = "pki.path-bytes";
const CERTIFICATES: &str = "pki.path-certificates";
const VERIFICATIONS: &str = "pki.path-verifications";
const ALLOCATION: &str = "pki.allocation";

pub(super) struct Budget<'a> {
    pub(super) limits: &'a Limits,
    work: Meter,
    nodes: Meter,
    bytes: Meter,
    verifications: Meter,
    signatures: BTreeMap<Vec<Vec<u8>>, SignatureResult>,
}

impl<'a> Budget<'a> {
    pub(super) const fn new(limits: &'a Limits) -> Self {
        Self {
            limits,
            work: Meter::new(
                u32_to_usize(limits.signatures.path_work),
                "path validation work budget exceeded",
            ),
            nodes: Meter::new(
                u32_to_usize(limits.signatures.path_nodes),
                "path validation node budget exceeded",
            ),
            bytes: Meter::new(
                u32_to_usize(limits.signatures.path_bytes),
                "path validation byte budget exceeded",
            ),
            verifications: Meter::new(
                u32_to_usize(limits.signatures.path_verifications),
                "path validation verification budget exceeded",
            ),
            signatures: BTreeMap::new(),
        }
    }
    /// Verifies through the host once per distinct input. Repeated inputs are
    /// memoized, and `path_verifications` caps the distinct calls, so a padded
    /// unsigned certificate set cannot amplify expensive host cryptography.
    pub(super) fn verify(
        &mut self,
        input: SignatureInput<'_>,
        verifier: &mut impl SignatureVerifier,
    ) -> Check<SignatureResult> {
        let parameters = input.issuer_key.parameters.map_or(&[][..], |p| p.encoded());
        let parts = [
            input.algorithm.element.encoded(),
            input.message,
            input.signature.as_bytes(),
            &[input.signature.unused_bits()],
            input.issuer_key.algorithm.as_bytes(),
            parameters,
            input.issuer_key.key.as_bytes(),
            &[input.issuer_key.key.unused_bits()],
        ];
        let bytes = parts.iter().fold(0usize, |n, p| n.saturating_add(p.len()));
        let comparisons = usize::try_from(self.signatures.len().checked_ilog2().unwrap_or(0) + 2)
            .unwrap_or(usize::MAX);
        self.work(bytes.saturating_mul(comparisons))?;
        self.bytes(bytes.saturating_add(256))?;
        let key = parts.iter().map(|p| p.to_vec()).collect::<Vec<_>>();
        if let Some(result) = self.signatures.get(&key) {
            return Ok(*result);
        }
        charge(&mut self.verifications, 1, VERIFICATIONS)?;
        self.node()?;
        self.bytes(256)?;
        let result = verifier.verify(input, self.limits);
        self.signatures.insert(key, result);
        Ok(result)
    }
    pub(super) fn work(&mut self, amount: usize) -> Check<()> {
        charge(&mut self.work, amount, WORK)
    }
    pub(super) fn node(&mut self) -> Check<()> {
        self.nodes(1)
    }
    pub(super) fn nodes(&mut self, amount: usize) -> Check<()> {
        self.work(amount)?;
        charge(&mut self.nodes, amount, NODES)
    }
    /// Limits for reparsing an extension value: its elements also count
    /// against the nodes this validation has left.
    pub(super) fn der_limits(&self) -> Limits {
        let remaining = u32::try_from(self.nodes.remaining()).unwrap_or(u32::MAX);
        Limits {
            signatures: SignatureLimits {
                asn1_elements: self.limits.signatures.asn1_elements.min(remaining),
                ..self.limits.signatures
            },
            ..*self.limits
        }
    }
    pub(super) fn bytes(&mut self, amount: usize) -> Check<()> {
        charge(&mut self.bytes, amount, BYTES)
    }
    /// A copied string: its bytes and the work of scanning them.
    pub(super) fn string(&mut self, amount: usize) -> Check<()> {
        self.bytes(amount)?;
        self.work(amount)
    }
    pub(super) fn push<T>(&mut self, values: &mut Vec<T>, value: T) -> Check<()> {
        if values.len() == values.capacity() {
            let growth = values.capacity().max(1);
            let capacity = values
                .capacity()
                .checked_add(growth)
                .ok_or(Issue::Unknown(Uncertainty::Limit(BYTES)))?;
            self.bytes(capacity.saturating_mul(size_of::<T>()))?;
            self.work(values.len().saturating_mul(size_of::<T>()))?;
            values
                .try_reserve_exact(growth)
                .map_err(|_| Issue::Unknown(Uncertainty::Limit(ALLOCATION)))?;
        }
        values.push(value);
        Ok(())
    }
    /// The most certificates a path may hold, anchor excluded.
    pub(super) const fn max_certificates(&self) -> usize {
        u32_to_usize(self.limits.signatures.path_certificates)
    }
    /// Checks that a path of `certificates`, anchor excluded, is admitted.
    pub(super) fn depth(&self, certificates: usize) -> Check<()> {
        if certificates > self.max_certificates() {
            return Err(Issue::Unknown(Uncertainty::Limit(CERTIFICATES)));
        }
        Ok(())
    }
}

/// Charges `amount`; an exhausted meter reports the stable `limit` name.
fn charge(meter: &mut Meter, amount: usize, limit: &'static str) -> Check<()> {
    meter
        .charge(amount)
        .map_err(|_| Issue::Unknown(Uncertainty::Limit(limit)))
}
