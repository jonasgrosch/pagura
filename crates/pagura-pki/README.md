# pagura-pki

Bounded, verification-free ASN.1, X.509, CMS and timestamp parsing. The crate
uses only `std` and `pagura-core`. Encoded algorithms, signatures, certificate
names, URIs and policy assertions are data. Parsing does not establish
integrity, trust, validity at a given time, or revocation status.

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
  revocation choices not interpreted here remain explicit variants.
- `cms::SignedAttributes`: DER-only attributes, exact SET signature input,
  unique attribute lookup and required content-type/message-digest shapes.
  Signature input replaces only the original IMPLICIT [0] identifier with the
  universal SET identifier. It never sorts or reserializes signed contents.
- `timestamp::{TstInfo,MessageImprint,Accuracy}`: asserted timestamp policy,
  imprint, serial, fractional UTC time, accuracy, ordering, nonce, TSA name
  and extensions. No timestamp assertion is authenticated by this parser.

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

Primitive canonicality and schema SET OF ordering remain strict. BER sets
that are not in DER order are outside this compatibility subset. Certificates, CRLs, TSTInfo and ECDSA
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
`tests/fixtures.py`, malformed byte cases, bounded/deep inputs, exact signed
ranges, signed-attribute byte equality and extension views. Fixture identifiers
are emitted by the generator; no reference full text or external vector bytes
are stored in the repository.

When available, `tests/vectors.py` reads NIST's `pkits/PKITS_data.zip` and the
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

Algorithm semantics (including RSA-PSS parameters), certificate serial-number
policy, required time-tag selection by certificate year, distinguished-name
matching and string normalization, path construction, certificate/CRL/OCSP
verification, extension policy, revocation freshness and timestamp verification
belong to subsequent validation. Legacy string repertoires remain encoded.
Extended/attribute certificate formats and some extension schemas are exposed
as original elements. This parser has no trust anchors or network access.
