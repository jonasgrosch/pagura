# pagura-pki

Bounded ASN.1, X.509, CMS and timestamp parsing, plus host-anchored certificate
path validation. The crate uses only `std` and `pagura-core`. Parsing alone
establishes no integrity or trust. Validation requires caller-selected anchors,
an explicit evaluation time and a signature verifier. It never reads a clock,
fetches an issuer, uses a system trust store or promotes an embedded certificate
to an anchor.

## API

```rust
use pagura_core::Limits;
use pagura_pki::{der, x509};

fn inspect(bytes: &[u8]) -> pagura_core::Result<()> {
    let encoding = der::Document::parse(bytes, &Limits::default())?;
    let certificate = x509::Certificate::parse(encoding.root())?;
    let signed_bytes = certificate.tbs.encoded();
    assert_eq!(signed_bytes, &bytes[certificate.tbs_range()]);
    Ok(())
}
```

The document owns a flat element index and borrows immutable input. Typed views
borrow the document, so signed bytes and algorithm parameters retain their
original encoding. `Element::{range,content_offset}` refer to the original
buffer, including when an extension value is parsed with `Extension::value_der`.

- `der::{Document,Element,Tag,Class}`: strict DER, identifiers, exact ranges,
  direct-child and preorder iterators, schema-directed SET OF ordering.
  `Fields` and `single` share positional schema checks with verification callers.
  `Document::{parse_prefix,parse_ber_prefix}` return the consumed length without
  trimming legitimate zero octets; PDF callers separately check zero padding.
- `oids`: named digest, signature, curve and ESS/CMS attribute identifiers.
- `types::{Integer,ObjectIdentifier,BitString,StringValue,Time}` and
  `types::{boolean,octets}`: borrowed primitive views. INTEGER preserves
  minimal signed big-endian bytes; `unsigned()` removes only a positive sign
  octet. OIDs retain arbitrary-width base-128 arcs without machine-integer
  conversion. Legacy character sets stay encoded, without guessed decoding.
- `types::EcdsaSignature`: the DER pair of nonnegative INTEGERs. Zero and
  values outside the curve order remain unverified data for the verifier.
- `x509::{Certificate,Name,SubjectPublicKeyInfo,AlgorithmIdentifier}`: v1–v3
  certificates, original TBS bytes, names, public keys, algorithms, validity,
  unique identifiers and extension envelopes.
- `x509::{Extension,CertificateList,RevokedCertificate}`: extension contents,
  exact CRL TBS bytes, update times, entries and entry/CRL extensions.
- `x509::extensions`: basic constraints, key usage, authority key identifier,
  GeneralNames, OID lists (including EKU), certificate policies and qualifiers,
  policy mappings, policy constraints, name constraints and AIA descriptions.
  SKI and inhibitAnyPolicy use the primitive OCTET STRING and INTEGER views.
  Other extension values remain available as DER trees. Recognition by a caller
  must be explicit; an unknown critical extension is never marked understood.
- `cms::SignedData::{from_content_info,parse}`: outer ContentInfo or bare
  SignedData; detached/encapsulated content, digest algorithms, certificate
  choices, CRLs/other revocation evidence and signer infos. Certificate and
  revocation choices not interpreted here remain explicit variants. Malformed
  certificate schemas are retained as `CertificateChoice::Malformed`, including
  the original element and error; a limit failure still aborts. The reported
  entry points route BER set ordering through the caller's diagnostics.
- `cms::SignedAttributes`: DER-only attributes, exact SET signature input,
  unique attribute lookup and required content-type/message-digest shapes.
  Signature input replaces only the original IMPLICIT [0] identifier with the
  universal SET identifier. It never sorts or reserializes signed contents.
- `timestamp::{TstInfo,MessageImprint,Accuracy}`: asserted timestamp policy,
  imprint, serial, fractional UTC time, accuracy, ordering, nonce, TSA name
  and extensions. No timestamp assertion is authenticated by this parser.

## Certification paths

`validation::validate_path` accepts certificates in root-to-target order,
**excluding** the anchor. `TrustAnchor` is an explicit trusted name and public
key. `TrustAnchor::from_certificate` merely extracts that information from a
container the host has already selected as trusted; the container's validity,
signature and extensions are not certification-path inputs. Initial host name
constraints belong in `ValidationOptions`, rather than being inferred from the
anchor's certificate (RFC 5280 6.1.1).

```rust
use pagura_core::Limits;
use pagura_pki::{Time, x509::Certificate};
use pagura_pki::validation::{
    PathReport, SignatureVerifier, TrustAnchor, ValidationOptions, validate_path,
};

fn check_path(
    certificates: &[&Certificate<'_>],
    selected_anchor: TrustAnchor<'_>,
    evaluation_time: Time<'_>,
    verifier: &mut impl SignatureVerifier,
    limits: &Limits,
) -> PathReport {
    let options = ValidationOptions::new(evaluation_time);
    validate_path(certificates, selected_anchor, &options, verifier, limits)
}
```

The verifier receives `SignatureInput`: exact original TBSCertificate DER,
algorithm and parameters, signature bits and the effective issuer key. Parameter
inheritance occurs only when the key algorithm stays the same (6.1.4(d)),
including inheritance through self-issued rollover certificates. It returns
`Verified`, `Invalid` or `Unavailable`; an unavailable algorithm/result is never
an invalid-signature assertion or a successful verification. The callback must
bound its own arithmetic; the engine bounds calls and supplies `Limits`.
Within one search, results are memoized by all signed inputs and the effective
issuer key, including inherited parameters and BIT STRING padding counts.
The shared call cap is `Limits.operations / 500_000`: 100 distinct host
verifications at default limits. Memo hits do not call the host. Input processing,
memo storage and comparisons also consume the ordinary shared budgets.

`PathReport::basic()` reports `PathState::{Valid, Invalid, Indeterminate}` for basic
path processing. Invalid and indeterminate states identify the certificate
position and a typed reason. The report also returns surviving `Oid`s and
the effective target key, including inherited parameters. The key is data;
its authentication depends on the associated result.

`PathReport::state()` additionally includes revocation. It starts indeterminate
for an otherwise acceptable path. `PathReport` and `PathSearch` are `must_use`;
report states are private and exposed through read accessors. Only the validation
stage produces the final state; there is no unchecked revocation-assertion setter.
Unprocessed critical certificate extensions remain explicit failures or deferred
CRL semantics until the CRL stage processes them.

`ValidationOptions` specifies the initial policy set and three inhibition flags,
optional initial name constraints, empty permitted name-form sets, intended EKU
and required target KU bits.
CA basic constraints, keyCertSign, non-self-issued path length, inclusive validity
and all policy counters follow RFC 5280 6.1. Self-issued intermediates skip name
checks and counter decrements; the final certificate does not. EKU restrictions
are applied throughout the path. A requested common purpose additionally requires
a compatible target KU (4.2.1.12); a private purpose needs a caller-specified KU
mask when the target asserts KU. No hostname, mailbox identity, timestamp-token
or CMS application policy is inferred from basic-path success.

`build_paths(target_index, certificates, anchors, options, verifier, limits)`
returns `PathSearch`: individual `BuiltPath`s with pool indices, anchor index
and report, plus completion, exhaustiveness and stopping reason. By default,
search stops at the first basically acceptable path; set
`ValidationOptions::search = SearchMode::All` for full enumeration. A resource
stop marks
`complete = false` and leaves the unfinished check indeterminate.
Missing issuers and cycles end in untrusted paths. Different certificates with
the same name remain candidates, including self-issued rollovers. Exact
certificate bytes detect cycles. AKI/SKI are parsed but do not eliminate
otherwise signature-valid issuer candidates. Unknown DN attributes remain
opaque and can match exact encoded values.
No result aggregates all paths into one validity flag.

### Names and policies

Directory names compare byte-identical attribute values directly. For known
caseIgnore attributes, the first-stage RFC 4518 subset also compares ASCII
values with case folding and insignificant-space handling. Non-identical values
that require Unicode preparation or an unknown matching rule remain
`Indeterminate(NameForm)`. Unknown attributes do not affect a comparison when
their OID and encoded values match exactly. RDN attributes are multisets; the
RDN sequence stays ordered. Sharing pagura-crypto's Unicode 3.2 stringprep tables
and adding RFC 3454 B.2 is deferred; this crate has no duplicate Unicode table.
SAN matching rules are applied only to forms covered by accumulated constraints.
Wildcard and underscore DNS names therefore do not invalidate an unconstrained
path. Unsupported constrained comparisons cannot establish a positive result.

Name constraints support directory, DNS, email, URI-host and IPv4/IPv6 forms.
Permitted groups intersect across issuers; each group is a union, and exclusions
always win. This avoids materializing cross products. Minimum must be zero
(absent in DER) and maximum absent. Unsupported constraint forms cannot create
a positive result. RFC 9549 domains remain ASCII NR-LDH/A-labels and compare
without U-label or UTS46 conversion. RFC 9598 SmtpUTF8Mailbox uses the existing
rfc822Name constraint namespace, with domain-only constraints. Individual-mailbox
constraints removed by RFC 9549 are rejected. SMTPUTF8 local parts are never
case folded or normalized; domain constraints do not compare local parts.

RFC 9618's policy graph has one node per OID per depth and shared parent edges;
it never reconstructs the exponential policy tree. OIDs are interned before
graph operations. The graph processes anyPolicy, grouped many-to-many mappings,
requireExplicitPolicy, inhibitPolicyMapping, inhibitAnyPolicy, pruning and the
final user-policy intersection. Qualifier aggregation is omitted as permitted
by RFC 9618 5.5 for applications that do not use qualifiers. CPS/UserNotice
syntax is understood; unknown critical qualifiers fail. Long user notices are
handled within string budgets (RFC 5280 4.2.1.4), rather than imposing the CA's
200-character producer bound on readers.

### Validation bounds

A single budget covers an entire path search, including CRL signing paths,
CRL signatures and every candidate's validation; it does not reset per branch. Standalone `validate_path` gets one
budget. The existing core fields are used without changing `Limits`:

| Limit | Validation meaning |
|---|---|
| `depth` (64 by default) | Maximum certificates per path; reparsed DER nesting |
| `value_nodes` (1,000,000) | Prepared values, inner DER nodes, queued branches, result paths, policy nodes/expected entries/parent edges, CRLs and entry processing |
| `operations` (50,000,000) | Scanning, comparisons, graph work and verifier inputs; at most `operations / 500_000` distinct verifier calls (100 by default) |
| `document_bytes` (128 MiB) | Conservative cumulative allocation estimates, including temporary extension indexes, copied names, policy storage, signature memo and results |
| `string_bytes` (32 MiB) | Names, OIDs, CRL numbers and opaque matching values |

The allocation accounting is conservative and cumulative, not a claim about
peak RSS or an allocator sandbox. Input certificate documents retain their own
DER admission limits. Extension indexes use the remaining validation node
limit; parser budget failures stay indeterminate. Exhaustion never becomes
an invalid-signature assertion or an exhaustive-search claim. Fixed report
metadata and caller-owned inputs are outside the cumulative allocation estimate.

### PKITS path tests

`tools/fixturegen/pki/pkits.py` reads **all 224** test descriptions from the cached NIST PDF,
resolves their certificate lists in the ZIP, and emits the default inputs plus
all 39 additional policy procedures. Tests check both the specified prospective
paths and path discovery for basic cases, plus policy output sets when the PDF
states them. Evaluation is fixed at **2020-01-01T00:00:00Z**, inside the ordinary
certificates' renewed 2010–2030 interval.

An exact recorded-result callback binds algorithm DER, TBS bytes, signature
bits, issuer algorithm, effective parameters and key bits. Independent Python
integer arithmetic supplies RSA/SHA-1, RSA/SHA-256 and DSA/SHA-1 results. Unknown
input tuples stay unavailable. This tests callback integration, not product RSA
or DSA implementations. No PDF expected result is used as revocation evidence.

| PKITS section | Executions | Basic matches | Requires CRL stage |
|---|---:|---:|---:|
| 4.1 Signatures | 6 | 6 | 0 |
| 4.2 Validity | 8 | 8 | 0 |
| 4.3 Name chaining | 11 | 11 | 0 |
| 4.4 Basic revocation | 21 | 0 | 21 |
| 4.5 Self-issued | 8 | 1 | 7 |
| 4.6 Basic constraints | 17 | 17 | 0 |
| 4.7 Key usage | 5 | 3 | 2 |
| 4.8 Certificate policies | 44 | 44 | 0 |
| 4.9 Explicit policy | 8 | 8 | 0 |
| 4.10 Policy mappings | 27 | 27 | 0 |
| 4.11 Mapping inhibition | 11 | 11 | 0 |
| 4.12 anyPolicy inhibition | 12 | 12 | 0 |
| 4.13 Name constraints | 38 | 38 | 0 |
| 4.14 Distribution points | 35 | 0 | 35 |
| 4.15 Delta CRLs | 10 | 0 | 10 |
| 4.16 Private extensions | 2 | 2 | 0 |
| **Total** | **263** | **188** | **75** |

The basic-only entry points deliberately leave 75 revocation-dependent cases
unresolved. The 188 basic matches comprise 149 default cases and 39 additional
policy procedures. These results are unchanged by the conservative DN matching
subset. Otherwise acceptable basic paths remain indeterminate until evidence is
validated. This table is a basic-validation result, not a whole-PKITS claim.

### Offline revocation

`validation::revocation::validate_path_with_crls(path, anchor, certificates,
crls, options, verifier, limits)` checks a supplied path. The certificate pool
also supplies separate or indirect CRL signing certificates. The corresponding
`build_paths_with_crls(target, certificates, anchors, crls, options, verifier,
limits)` discovers paths and stops at the first fully Valid path by default;
`SearchMode::All` requests full enumeration. CRL checks share the signature memo,
verification cap and all work/allocation budgets with certificate validation.
No URL is dereferenced and no embedded certificate becomes an anchor.

`PathReport::revocation()` returns one `RevocationOutcome` per certificate in
root-to-target order, excluding the anchor: `Good`, `Revoked { reason, time }`,
or `Unknown(UnknownReason)`. The final state is Valid only when every outcome is
Good and basic validation succeeds. Revoked produces Invalid with its certificate
position. Missing, stale, out-of-scope, unsupported, unauthenticated or incomplete
evidence remains Indeterminate with its reason. A resource stop may prevent a
complete outcome list; the state and search stopping reason expose that bound.
The basic result is retained separately, including a critical distribution-point
extension's deferred CRL semantics. Deferred CRL semantics are tracked separately
from signature or other uncertainties: Good CRLs cannot resolve an unverified
certificate signature, including one on a CRL signer's path. Unsupported name
comparisons are confined to their candidate; other certificate and CRL candidates
remain eligible. CRL scope names are compared only after signature
authentication.

RFC 5280 6.3 checks issuer names, signature and cRLSign authorisation, signer
certification paths to the same anchor, indirect certificateIssuer inheritance,
full/relative distribution-point names, certificate-class scope and union of
reason coverage. Whole distribution-point URIs compare as locations; matching
only their hostname would incorrectly merge distinct CRL scopes. Unknown critical
CRL or entry extensions make the CRL unusable (5.2/5.3), including extensions on
unrelated entries. `thisUpdate <= evaluationTime < nextUpdate` is required;
missing nextUpdate is unknown. Reason code 0 is unspecified and the unused reason
flag contributes no coverage. Serial comparisons preserve signed arbitrary-width
INTEGER values. A matching authenticated entry produces Revoked even if its
revocationDate is later than the evaluation time: RFC 5280 6.3.3(i–j) assigns
the entry's reason without a date comparison. The asserted date is preserved;
it cannot make an authenticated revocation appear Good.

Delta processing checks the issuer, authority identifier, issuing distribution
point, numeric CRL/base numbers and signing key. A current delta may update a
stale complete CRL. The newest usable delta is selected by thisUpdate and then
numeric CRL number. A valid removeFromCRL releases a hold or removes an expired
certificate; it cannot release an unexpired permanent revocation. Missing bases
and missing advertised deltas remain unknown. Newer authenticated complete CRLs
supersede older evidence for the same scope and key. CRL signer authorisations
whose own paths lack Good status are removed to a fixed point, permitting the
self-covering CRL signer cases while excluding revoked signing keys. Every
changing pass removes an authorisation and consumes the same budget.

`tools/fixturegen/pki/revocation.py` obtains the selected CRLs and auxiliary
certificates from the PKITS PDF object lists. It verifies their signatures using
the same independent integer arithmetic and exact recorded-input callback.

| PKITS section | Cases | Matches | Valid | Revoked | Unknown | Basic invalid |
|---|---:|---:|---:|---:|---:|---:|
| 4.4 Revocation | 21 | 21 | 6 | 5 | 10 | 0 |
| 4.5 Self-issued | 8 | 8 | 4 | 3 | 0 | 1 |
| 4.7 Key usage | 5 | 5 | 1 | 0 | 2 | 2 |
| 4.14 Distribution points | 35 | 35 | 15 | 10 | 10 | 0 |
| 4.15 Delta CRLs | 10 | 10 | 4 | 4 | 2 | 0 |
| **Total** | **79** | **79** | **30** | **22** | **24** | **3** |

The two CRL-signing cases in 4.7 both match. Of the 75 previously deferred cases,
29 become Valid, 22 are Revoked and 24 correctly stay Unknown because their
supplied evidence is missing, stale or unusable. None remain deferred for an
unimplemented CRL check. PKITS permits an unknown-status warning for unavailable
revocation evidence; this implementation preserves that state rather than
claiming an authenticated revocation. The other three negative cases fail basic
constraints/key usage.

Independent synthetic fixtures in `tools/fixturegen/pki/paths.py` and `tools/fixturegen/pki/graphs.py` cover
updated email/domain constraints, IPv4/IPv6 masks, unknown extensions, arbitrary
width counters, host-only trust, cycles, branching bounds, mixed path outcomes,
KU/EKU consistency and policy-graph sharing. `path_inputs.py` adds conservative
DN/unconstrained SAN cases and a padded issuer pool that stops at 100 verifier
calls. `crls.py` adds 64 independently constructed CRL cases (including evidence-order reversals) covering scope,
freshness, reason coverage, delta/removal ordering, critical extensions and the
shared signature memo/cap. `path_security.py` adds exact synthetic signature
attestations for unavailable signatures after deferred CRL extensions, CRL signer
path authentication, and junk certificate/CRL isolation in both input orders and
both search modes. Cached suites are conditional when unavailable; all
generated structural tests run without the cache. Requirement references are
RFC 5280 4.2, 5.2–5.3, 6.1, 6.3 and 7.1–7.5, RFC 4518 2, RFC 9549 2, RFC 9598 3–6 and
RFC 9618 5. Shared fixture/requirement catalogs are not changed by this crate.

## Encoding rules

| Rule | Reference | Enforcement |
|---|---|---|
| Minimal identifier, complete base-128 tag | X.690 8.1.2 | Reader; u32 tag overflow is a Limit error |
| Definite, minimally encoded length | X.690 8.1.3, 10.1 | Reader; checked sizes and parent boundaries |
| Minimal signed INTEGER | X.690 8.3 | Primitive and IMPLICIT INTEGER views |
| BOOLEAN 00/FF; empty NULL | X.690 8.2, 8.8, 11.1 | Primitive validation |
| Primitive string encoding | X.690 10.2 | Strict reader |
| BIT STRING padding and named-bit truncation | X.690 8.6, 11.2 | Reader padding; keyUsage named bits |
| Minimal, complete OID arcs | X.690 8.19 | Reader, including IMPLICIT registered IDs |
| SET OF encoding order | X.690 11.6 | Schema views and `Element::set_of` |
| Omitted DEFAULT values | X.690 11.5 | Certificate version, extension criticality, basic constraints, subtree minimum, timestamp ordering |
| Canonical UTC times | X.690 11.7–11.8 | Reader; calendar dates and UTC suffix |
| PKIX whole-second time profile and pivot | RFC 5280 4.1.2.5 | `Time::parse`; fractions rejected |
| Timestamp fractional time profile | RFC 3161 2.4.2 | `Time::generalized`; exact decimal fraction |
| DER signed attributes and SET signature input | RFC 5652 5.3–5.4 | `SignedAttributes::parse`, including BER outer containers |
| Required signed-attribute shapes/content type | RFC 5652 11.1–11.2 | SignedData parsing; no digest computation |

A generic ASN.1 reader cannot infer whether universal tag 17 denotes SET or
SET OF, or identify an IMPLICIT primitive or a DEFAULT field. It validates the
encoding tree; the typed schema supplies those additional checks. It does not
claim to validate arbitrary ASN.1 schemas. Unknown universal types fail with
an explicit `asn1.universal-type` or `asn1.constructed-type` feature.

`Document::parse_ber` is an explicit CMS compatibility subset. Its deviations
pass through `Diagnostics::repair`, so a strict sink rejects them:

| Repair | Tolerance |
|---|---|
| `asn1.ber-indefinite` | Constructed indefinite length, bounded and EOC-terminated |
| `asn1.ber-length` | Redundant definite-length octets, including long form below 128 |
| `asn1.ber-octets` | Constructed OCTET STRING, with octet-string children only |
| `asn1.ber-set-order` | SET OF members retained in noncanonical source order, through `set_of_reported` |

Primitive canonicality remains strict. `set_of` always checks DER order;
`set_of_reported` admits BER misordering only with a reported repair.
Certificates, CRLs, TSTInfo and ECDSA
signature envelopes are DER even in a BER tree. Signed attributes are always
rechecked as DER. This entry point is not a complete BER decoder.

## Bounds

`Limits::file_bytes` bounds the original encoding; `depth` bounds its nesting;
`value_nodes` bounds the element count; `document_bytes` bounds the element
index, including admitted spare capacity; `string_bytes` bounds each primitive
content; and `decoded_bytes` bounds concatenated CMS octets and copied signed
attributes. Each schema traverses only admitted elements, with no recursive
ASN.1 parse or recursive tree destruction. This is a per-operation accounting
model, not an allocator or operating-system sandbox. Typed view collections
and reparsed extension indexes are bounded by the admitted source tree rather
than charged to a shared document cache counter.

## Tests and references

`cargo test -p pagura-pki` runs the independent standard-library generator
`tools/fixturegen/pki/fixtures.py`, malformed byte cases, bounded/deep inputs, exact signed
ranges, signed-attribute byte equality and extension views. Fixture identifiers
are emitted by the generator; no reference full text or external vector bytes
are stored in the repository.

When available, `tools/fixturegen/pki/vectors.py` reads NIST's `pkits/PKITS_data.zip` and the
six ECDSA files in `wycheproof/testvectors_v1`. The cache defaults to
`~/.cache/pagura-references/signatures`; `PAGURA_REFERENCES_DIR` changes the
reference root and `PAGURA_SIGNATURE_REFERENCES_DIR` overrides the signatures
folder directly. Absent external suites are reported; generated tests still
run. Wycheproof's valid signatures and its InvalidEncoding,
InvalidTypesInSignature, BerEncodedSignature and MissingZero cases exercise
encoding acceptance, not mathematical verification. No upstream vectors are
redistributed, so no third-party product data or dependency is introduced.

The normative sources are ITU-T X.690 (02/2021), RFC 5280, RFC 5652, RFC 3161
and the ECDSA envelope in RFC 3279 2.2.3. Exact source hashes are supplied by the
external reference cache's `MANIFEST.tsv`.

## Remaining validation work

Product signature arithmetic, RSA-PSS parameter semantics, algorithm-strength
policy, OCSP verification, CMS signature verification and
RFC 3161 timestamp verification remain separate stages. An application must
supply authenticated host anchors and the appropriate evaluation time and
usage. General path validation is not a timestamp, document-integrity or
revocation result. OCSP remains deferred pending the shared digest API needed
for responder-key and CertID hash matching. Offline OCSP has no originating
request here, so no nonce checking is performed. Extended/attribute certificate formats and some extension
schemas remain exposed as original elements. The parser and validator have no
network access or implicit trust source.
