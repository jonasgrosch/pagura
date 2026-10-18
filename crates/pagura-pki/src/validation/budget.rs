use super::{Check, Issue, SignatureInput, SignatureResult, SignatureVerifier, Uncertainty};
use pagura_core::Limits;
use std::collections::BTreeMap;

pub(super) struct Budget<'a> {
    pub limits: &'a Limits,
    work: usize,
    nodes: usize,
    bytes: usize,
    signatures: BTreeMap<Vec<Vec<u8>>, SignatureResult>,
    calls: usize,
}

impl<'a> Budget<'a> {
    pub fn new(limits: &'a Limits) -> Self {
        Self {
            limits,
            work: 0,
            nodes: 0,
            bytes: 0,
            signatures: BTreeMap::new(),
            calls: 0,
        }
    }
    // A separate call count derived from Limits::operations prevents a padded
    // unsigned certificate set from amplifying expensive host cryptography.
    // Default 50,000,000 operations admits at most 100 distinct verifications.
    pub fn verify(
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
        if self.calls >= self.limits.operations / 500_000 {
            return Err(Issue::Unknown(Uncertainty::Limit("pki.signatures")));
        }
        self.node()?;
        self.bytes(256)?;
        self.calls += 1;
        let result = verifier.verify(input, self.limits);
        self.signatures.insert(key, result);
        Ok(result)
    }
    pub fn work(&mut self, amount: usize) -> Check<()> {
        Self::charge(
            &mut self.work,
            amount,
            self.limits.operations,
            "pki.operations",
        )
    }
    pub fn node(&mut self) -> Check<()> {
        self.nodes(1)
    }
    pub fn nodes(&mut self, amount: usize) -> Check<()> {
        self.work(amount)?;
        Self::charge(
            &mut self.nodes,
            amount,
            self.limits.value_nodes,
            "pki.value-nodes",
        )
    }
    pub fn der_limits(&self) -> Limits {
        Limits {
            value_nodes: self.limits.value_nodes.saturating_sub(self.nodes),
            ..*self.limits
        }
    }
    pub fn bytes(&mut self, amount: usize) -> Check<()> {
        Self::charge(
            &mut self.bytes,
            amount,
            self.limits.document_bytes,
            "pki.document-bytes",
        )
    }
    pub fn string(&mut self, amount: usize) -> Check<()> {
        if amount > self.limits.string_bytes {
            return Err(Issue::Unknown(Uncertainty::Limit("pki.string-bytes")));
        }
        self.bytes(amount)?;
        self.work(amount)
    }
    pub fn push<T>(&mut self, values: &mut Vec<T>, value: T) -> Check<()> {
        if values.len() == values.capacity() {
            let growth = values.capacity().max(1);
            let capacity = values
                .capacity()
                .checked_add(growth)
                .ok_or(Issue::Unknown(Uncertainty::Limit("pki.document-bytes")))?;
            self.bytes(capacity.saturating_mul(size_of::<T>()))?;
            self.work(values.len().saturating_mul(size_of::<T>()))?;
            values
                .try_reserve_exact(growth)
                .map_err(|_| Issue::Unknown(Uncertainty::Limit("pki.allocation")))?;
        }
        values.push(value);
        Ok(())
    }
    pub fn depth(&self, amount: usize) -> Check<()> {
        if amount > self.limits.depth {
            return Err(Issue::Unknown(Uncertainty::Limit("pki.depth")));
        }
        Ok(())
    }
    fn charge(value: &mut usize, amount: usize, limit: usize, name: &'static str) -> Check<()> {
        *value = value
            .checked_add(amount)
            .filter(|&n| n <= limit)
            .ok_or(Issue::Unknown(Uncertainty::Limit(name)))?;
        Ok(())
    }
}
