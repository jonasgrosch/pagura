# ADR 0015: Verification-only digital signatures with explicit trust and revision status

Status: accepted, 2026-10-18. Scheduled after the current M6 work.

## Context

A reader must show whether a signed PDF's signatures hold. ISO 32000-2 defines
the parts involved (12.7.5.5, 12.8.1–12.8.5, Tables 255–259 and 261):
- signature fields and their dictionaries;
- the signed byte ranges;
- the `adbe.pkcs7.detached` and `ETSI.CAdES.detached` sub-filters;
- document timestamps;
- the DSS for long-term validation.

Validating them needs:
- CMS (RFC 5652);
- X.509 certificates and path validation (RFC 5280);
- RSA PKCS#1 v1.5 and PSS (RFC 8017) and ECDSA on P-256, P-384 and P-521
  (FIPS 186-5);
- SHA-1 and SHA-2;
- timestamps (RFC 3161);
- revocation data (RFC 6960, CRLs).

Two options were compared:
- **Third-party crates under ADR 0008.** The candidate crates (der, x509-cert,
  cms, rsa, p256, p384, p521, ecdsa, sha1, sha2) were inspected at the versions
  in the local registry cache. Their stack fails ADR 0008: unsafe code
  appears transitively, for example in `subtle`, `zeroize`, `generic-array`
  and `num-bigint-dig`, or in native hash backends, and several dependencies
  have build scripts. Disabling signing or acceleration features does not
  change that.
- **A native implementation, verification only.** Verification processes
  only public data, so constant-time arithmetic is not required. This is the
  option chosen.

## Decision

1. **Native, verification only.** Pagura implements, from the standards:
   - DER parsing;
   - CMS, X.509, CRL, OCSP and timestamp structures;
   - bounded big-integer RSA verification;
   - ECDSA verification on P-256, P-384 and P-521;
   - certificate path construction and RFC 5280 validation.

   Signing, decryption and any private-key operation are out of scope and need
   a separate decision. Planning estimates are 19,000–33,500 lines for the
   cryptography and PKI, plus 3,000–6,000 lines for PDF integration.
2. **Separate results, never one "valid" flag.** Each signature reports:
   - **Signed-byte integrity:** ByteRange structure, bounds and coverage, the
     hash of the original bytes, and the CMS signature and required
     attributes.
   - **Revision coverage:** which revision the signature covers, and any later
     incremental updates.
   - **Permitted modifications:** DocMDP and FieldMDP evaluated against the
     signed and later revisions, with inconclusive analysis reported as such.
   - **Signer trust:** a path to **host-supplied trust anchors**. Embedded or
     self-signed certificates never become anchors. Without one the signer is
     reported as untrusted, separately from integrity.
   - **Validation time:** a host-supplied time and policy; an asserted
     signing time is distinguished from a validated timestamp.
   - **Revocation and long-term validation:** supplied OCSP, CRL, timestamp
     and DSS evidence is validated. Missing or stale evidence is
     indeterminate, never good.
   - **Algorithms:** SHA-1 signatures are reported weak. Mathematical
     verification and policy acceptance are distinct, and unsupported
     algorithms stay unsupported.
3. **Offline and inert.** Validation is deterministic: the host supplies
   anchors, time, policy and evidence. AIA, OCSP and CRL URLs are data, and
   the engine never fetches them.
4. **Shown outside PDF content.** Results appear in the viewer's own
   interface, never in a signature field's appearance (12.7.5.5). A painted
   appearance is never evidence.
5. **Staged delivery.** Intermediate states stay precise: integrity verified,
   signer untrusted, revocation unknown, later changes unclassified, or
   algorithm unsupported. No partial implementation may present them as an
   unqualified valid signature.
6. **Bounded work.** Public input still gets limits: modulus and exponent
   sizes, ASN.1 depth, certificate counts, path branches, revocation entries
   and total verification work.

## Evidence required

- **Hashes, RSA and ECDSA:** NIST CAVP vectors and Wycheproof's valid and
  malformed cases.
- **Arithmetic:** independent Python integer checks.
- **Certificate paths:** NIST PKITS.
- **CMS, timestamps and revocation:** standards-derived fixtures and
  independently generated material.
- **PDF integration:** independently signed PDFs with byte mutations,
  coverage gaps, multiple revisions, MDP changes, DSS additions and
  timestamp chains.

Other readers are interoperability evidence, not oracles (ADR 0006). An
independent cryptographic and PKI review is a release gate.

## Consequences

- The PKI work is larger than the existing handwritten cryptography, and
  needs its own review.
- The standards and test vectors are downloaded only with recorded versions
  and hashes, after explicit approval when the work starts:
  - RFC 5652, 5280, 8017, 3161, 5816, 6960, 5035, 4055, 5480, 5758, 3279 and
    4518;
  - FIPS 186-5, NIST SP 800-186 and ITU-T X.690;
  - ETSI EN 319 122-1/-2 and 319 142-1/-2, V1.1.1;
  - CAVP, PKITS and Wycheproof.
- Reconsider third-party crates only if a fully inspected dependency closure
  meets ADR 0008, or if ADR 0008 itself changes.
