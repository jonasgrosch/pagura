"""Independent ASN.1 envelopes derived from X.690 and the RFC schemas.

Python standard library only; never imports or runs Pagura. Each case is one
DER (or BER) value with the kind of structure it encodes and whether a strict
reader must accept it, following ITU-T X.690 (02/2021), RFC 5280, RFC 5652,
RFC 3161 and RFC 3279 2.2.3. These are synthetic envelopes; none contains a
cryptographically valid signature.
"""
from __future__ import annotations

from collections.abc import Iterator

from ..model import Vector

DER = ("pki-der",)
BER = ("pki-der", "pki-ber-subset")
CERTIFICATE = ("pki-x509-certificate",)
EXTENSION = ("pki-x509-extensions",)
CRL = ("pki-x509-crl",)
CMS = ("pki-cms-signed-data",)
TIMESTAMP = ("pki-timestamp",)
ECDSA = ("pki-ecdsa-envelope",)


def tlv(tag: int, value: bytes) -> bytes:
    size = len(value)
    if size < 128:
        return bytes([tag, size]) + value
    length = size.to_bytes((size.bit_length() + 7) // 8, "big")
    return bytes([tag, 128 + len(length)]) + length + value


def seq(*values: bytes) -> bytes:
    return tlv(0x30, b"".join(values))


def integer(value: int) -> bytes:
    # Minimal two's complement (X.690 8.3.2): one sign bit beyond the magnitude.
    magnitude = value if value >= 0 else ~value
    return tlv(2, value.to_bytes(magnitude.bit_length() // 8 + 1, "big", signed=True))


def oid(*arcs: int) -> bytes:
    values = [arcs[0] * 40 + arcs[1], *arcs[2:]]
    output = bytearray()
    for value in values:
        digits = [value & 127]
        while value > 127:
            value >>= 7
            digits.append(128 | (value & 127))
        output.extend(reversed(digits))
    return tlv(6, bytes(output))


def octets(data: bytes) -> bytes:
    return tlv(4, data)


def bitstring(data: bytes, unused: int = 0) -> bytes:
    return tlv(3, bytes([unused]) + data)


def set_of(*values: bytes) -> bytes:
    return tlv(0x31, b"".join(sorted(values)))


def explicit(number: int, value: bytes) -> bytes:
    return tlv(0xA0 + number, value)


TRUE = b"\x01\x01\xff"
FALSE = b"\x01\x01\x00"


def extension(number: int, value: bytes, critical: bool = False) -> bytes:
    return seq(oid(2, 5, 29, number), TRUE if critical else b"", octets(value))


SHA256 = seq(oid(2, 16, 840, 1, 101, 3, 4, 2, 1), b"\x05\x00")
SHA1 = seq(oid(1, 3, 14, 3, 2, 26), b"\x05\x00")
RSA = seq(oid(1, 2, 840, 113549, 1, 1, 1), b"\x05\x00")
RSA_SHA256 = seq(oid(1, 2, 840, 113549, 1, 1, 11), b"\x05\x00")
ID_DATA = oid(1, 2, 840, 113549, 1, 7, 1)
ID_SIGNED_DATA = oid(1, 2, 840, 113549, 1, 7, 2)
NAME = seq(set_of(seq(oid(2, 5, 4, 3), tlv(12, b"Example")), seq(oid(2, 5, 4, 10), tlv(19, b"Pagura"))))
BEFORE = tlv(23, b"240101000000Z")
AFTER = tlv(24, b"20500101000000Z")
SPKI = seq(RSA, bitstring(seq(integer(257), integer(65537))))
# A positive INTEGER one octet wider than u64 (2**64).
OVERSIZED = integer(1 << 64)
EXTENSIONS = seq(
    extension(19, seq(TRUE, integer(2)), True),
    extension(15, bitstring(b"\x86", 1), True),
    extension(14, octets(b"key1")),
    extension(35, seq(tlv(0x80, b"key1"))),
    extension(37, seq(oid(1, 3, 6, 1, 5, 5, 7, 3, 8))),
    extension(17, seq(tlv(0x82, b"example.test"), tlv(0x86, b"https://example.test"))),
    extension(32, seq(seq(oid(1, 2, 3, 4)))),
    extension(33, seq(seq(oid(1, 2, 3, 4), oid(1, 2, 3, 5)))),
    extension(36, seq(tlv(0x80, b"\x01"), tlv(0x81, b"\x02"))),
    extension(30, seq(tlv(0xA0, seq(tlv(0x82, b".example.test"))))),
    extension(54, integer(4)),
    extension(99, seq(tlv(12, b"opaque")), True),
)


def certificate(version: int = 3, extensions: bytes = EXTENSIONS, algorithm: bytes = RSA_SHA256,
                version_field: bytes | None = None, unique_ids: bytes = b"", name: bytes = NAME) -> bytes:
    if version_field is None:
        version_field = explicit(0, integer(version - 1)) if version != 1 else b""
    tbs = seq(version_field, integer(7), RSA_SHA256, name, seq(BEFORE, AFTER), name, SPKI, unique_ids,
              explicit(3, extensions) if extensions else b"")
    return seq(tbs, algorithm, bitstring(b"unverified"))


CRL_V2 = integer(1)
ENTRY_EXTENSIONS = seq(extension(21, tlv(10, b"\x01")))
CRL_EXTENSIONS = explicit(0, seq(extension(20, integer(2))))


def crl(version: bytes = CRL_V2, entry_extensions: bytes = ENTRY_EXTENSIONS,
        extensions: bytes = CRL_EXTENSIONS, algorithm: bytes = RSA_SHA256) -> bytes:
    entry = seq(integer(7), BEFORE, entry_extensions)
    tbs = seq(version, RSA_SHA256, NAME, BEFORE, AFTER, seq(entry), extensions)
    return seq(tbs, algorithm, bitstring(b"unverified"))


CONTENT_ATTR = seq(oid(1, 2, 840, 113549, 1, 9, 3), set_of(ID_DATA))
DIGEST_ATTR = seq(oid(1, 2, 840, 113549, 1, 9, 4), set_of(octets(bytes(range(32)))))
ATTRS = b"".join(sorted([CONTENT_ATTR, DIGEST_ATTR]))


def cms(attrs: bytes | None = None, ber: bool = False, key_id: bool = False, content: bool = False,
        certificates: bytes | None = None, revocation: bytes | None = None, extra: bytes = b"",
        version: bytes | None = None, signer_version: bytes | None = None,
        digests: bytes | None = None) -> bytes:
    if attrs is None:
        attrs = tlv(0xA0, ATTRS)
    if signer_version is None:
        signer_version = integer(3 if key_id else 1)
    signer = seq(signer_version, tlv(0x80, b"key1") if key_id else seq(NAME, integer(7)),
                 SHA256, attrs, RSA_SHA256, octets(b"signature"))
    encap = seq(ID_DATA, explicit(0, b"\x24\x80" + octets(b"abc") + tlv(0x24, octets(b"def")) + b"\x00\x00") if content else b"")
    if version is None:
        version = integer(3 if key_id else 1)
    if certificates is None:
        certificates = certificate()
    if revocation is None:
        revocation = crl()
    if digests is None:
        digests = set_of(SHA256)
    fields = version + digests + encap
    if certificates:
        fields += tlv(0xA0, certificates)
    if revocation:
        fields += tlv(0xA1, revocation)
    fields += set_of(signer) + extra
    signed_data = b"\x30\x80" + fields + b"\x00\x00" if ber else tlv(0x30, fields)
    return seq(ID_SIGNED_DATA, explicit(0, signed_data))


TST_V1 = integer(1)
TST_SERIAL = integer(123)
TST_NONCE = integer(256)


def timestamp(optional: bool = True, fraction: bytes = b".125", accuracy: bytes | None = None,
              ordering: bytes = TRUE, version: bytes = TST_V1, serial: bytes = TST_SERIAL,
              nonce: bytes = TST_NONCE) -> bytes:
    if accuracy is None:
        accuracy = seq(integer(2), tlv(0x80, b"\x01"), tlv(0x81, b"\x03\xe7"))
    rest = accuracy + ordering + nonce + explicit(0, explicit(4, NAME)) + tlv(0xA1, extension(20, integer(1)))
    return seq(version, oid(1, 2, 3, 4), seq(SHA256, octets(bytes(range(32)))), serial,
               tlv(24, b"20261018120000" + fraction + b"Z"), rest if optional else b"")


def case(kind: str, name: str, accept: bool, data: bytes, requirements: tuple[str, ...], description: str,
         error: str | None = None, **extra: str) -> Vector:
    fields = {"structure": kind, "expect": "accept" if accept else "reject", **extra}
    if error:
        fields["error"] = error
    return Vector(name, {".der": data}, requirements, description, fields)


def certificates() -> Iterator[Vector]:
    yield case("certificate", "x509-v3", True, certificate(), CERTIFICATE,
               "v3 certificate with every typed extension and one unknown critical one")
    yield case("certificate", "x509-v1", True, certificate(1, b""), CERTIFICATE,
               "v1 certificate: version omitted as DEFAULT (X.690 11.5)")
    yield case("certificate", "x509-v2-unique-ids", True,
               certificate(2, b"", unique_ids=tlv(0x81, b"\x00\x01") + tlv(0x82, b"\x00\x02")), CERTIFICATE,
               "v2 certificate with issuer and subject unique identifiers (RFC 5280 4.1.2.8)")
    yield case("certificate", "x509-v1-unique-id", False,
               certificate(1, b"", unique_ids=tlv(0x81, b"\x00\x01")), CERTIFICATE,
               "unique identifiers require v2 or v3 (RFC 5280 4.1.2.8)")
    yield case("certificate", "x509-v2-extensions", False, certificate(2), CERTIFICATE,
               "extensions require v3 (RFC 5280 4.1.2.9)")
    yield case("certificate", "x509-algorithm-mismatch", False, certificate(algorithm=RSA), CERTIFICATE,
               "outer and TBS signature algorithms differ (RFC 5280 4.1.1.2)")
    yield case("certificate", "x509-duplicate-extension", False,
               certificate(extensions=seq(extension(19, seq()), extension(19, seq()))), CERTIFICATE,
               "an extension type appears twice (RFC 5280 4.2)")
    yield case("certificate", "x509-explicit-default-version", False,
               certificate(version_field=explicit(0, integer(0)), extensions=b""), CERTIFICATE,
               "v1 encoded explicitly although DER omits the DEFAULT (X.690 11.5)")
    yield case("certificate", "x509-oversized-version", False,
               certificate(version_field=explicit(0, OVERSIZED)), CERTIFICATE,
               "a version INTEGER wider than 64 bits is malformed, not a budget failure", error="syntax")
    yield case("certificate", "x509-explicit-default-critical", False,
               certificate(extensions=seq(seq(oid(2, 5, 29, 19), FALSE, octets(seq())))), CERTIFICATE,
               "critical encoded as its DEFAULT FALSE (X.690 11.5)")
    yield case("certificate", "x509-empty-rdn", False, certificate(name=seq(tlv(0x31, b""))), CERTIFICATE,
               "a relative distinguished name has no attribute (RFC 5280 4.1.2.4)")


def crls() -> Iterator[Vector]:
    yield case("crl", "x509-crl-v2", True, crl(), CRL, "v2 CRL with entry and CRL extensions")
    yield case("crl", "x509-crl-v1", True, crl(b"", b"", b""), CRL, "v1 CRL: no version, no extensions")
    yield case("crl", "x509-crl-v1-entry-extensions", False, crl(b"", extensions=b""), CRL,
               "entry extensions require v2 (RFC 5280 5.3)")
    yield case("crl", "x509-crl-v1-extensions", False, crl(b"", b""), CRL,
               "CRL extensions require v2 (RFC 5280 5.1.2.7)")
    yield case("crl", "x509-crl-algorithm-mismatch", False, crl(algorithm=RSA), CRL,
               "outer and TBS signature algorithms differ (RFC 5280 5.1.1.2)")
    yield case("crl", "x509-crl-version-2", False, crl(integer(2)), CRL,
               "v3 does not exist for CRLs (RFC 5280 5.1.2.1)")
    yield case("crl", "x509-crl-oversized-version", False, crl(OVERSIZED), CRL,
               "a version INTEGER wider than 64 bits is malformed", error="syntax")
    yield case("crl", "x509-crl-duplicate-extension", False,
               crl(extensions=explicit(0, seq(extension(20, integer(2)), extension(20, integer(3))))), CRL,
               "a CRL extension type appears twice (RFC 5280 5.2)")


def extensions() -> Iterator[Vector]:
    def ext(view: str, name: str, accept: bool, data: bytes, description: str) -> Vector:
        return case("extension", name, accept, data, EXTENSION, description, view=view)

    yield ext("basic-constraints", "basic-constraints-ca", True, seq(TRUE, integer(0)),
              "cA with a zero path length (RFC 5280 4.2.1.9)")
    yield ext("basic-constraints", "basic-constraints-end-entity", True, seq(),
              "an end entity: cA absent as DEFAULT FALSE")
    yield ext("basic-constraints", "basic-constraints-path-without-ca", False, seq(integer(1)),
              "pathLenConstraint without cA TRUE (RFC 5280 4.2.1.9)")
    yield ext("basic-constraints", "basic-constraints-explicit-false", False, seq(FALSE),
              "cA encoded as its DEFAULT FALSE (X.690 11.5)")
    yield ext("basic-constraints", "basic-constraints-negative-path", False, seq(TRUE, integer(-1)),
              "a negative path length (RFC 5280 4.2.1.9: INTEGER (0..MAX))")
    yield ext("key-usage", "key-usage-digital-signature", True, bitstring(b"\x80", 7),
              "digitalSignature only")
    yield ext("key-usage", "key-usage-decipher-only", True, bitstring(b"\x00\x80", 7),
              "decipherOnly, the ninth named bit, in a second octet")
    yield ext("key-usage", "key-usage-trailing-zero", False, bitstring(b"\x80", 6),
              "a trailing zero named bit is kept (X.690 11.2.2)")
    yield ext("key-usage", "key-usage-empty", False, bitstring(b""),
              "no usage bit (RFC 5280 4.2.1.3: at least one bit is set)")
    yield ext("key-usage", "key-usage-unknown-bit", False, bitstring(b"\x00\x40", 6),
              "bit 9 is not a named key usage")
    yield ext("key-usage", "key-usage-three-octets", False, bitstring(b"\x80\x00\x80", 7),
              "more octets than nine named bits need")
    yield ext("authority-key-identifier", "aki-key-only", True, seq(tlv(0x80, b"key1")),
              "keyIdentifier alone (RFC 5280 4.2.1.1)")
    yield ext("authority-key-identifier", "aki-issuer-serial", True,
              seq(tlv(0x80, b"key1"), tlv(0xA1, explicit(4, NAME)), tlv(0x82, b"\x07")),
              "keyIdentifier with the paired authority issuer and serial")
    yield ext("authority-key-identifier", "aki-issuer-without-serial", False,
              seq(tlv(0xA1, explicit(4, NAME))),
              "authorityCertIssuer without authorityCertSerialNumber (RFC 5280 4.2.1.1)")
    every_choice = seq(tlv(0xA0, oid(1, 2, 3) + explicit(0, tlv(12, b"x"))), tlv(0x81, b"a@example.test"),
                       tlv(0x82, b"example.test"), explicit(4, NAME), tlv(0x86, b"https://example.test"),
                       tlv(0x87, b"\xc0\x00\x02\x01"), tlv(0x88, b"\x2a\x03"))
    yield ext("general-names", "general-names-every-choice", True, every_choice,
              "otherName, rfc822Name, dNSName, directoryName, URI, iPAddress and registeredID (RFC 5280 4.2.1.6)")
    yield ext("general-names", "general-name-non-ia5", False, seq(tlv(0x82, b"\xe9xample.test")),
              "a dNSName outside IA5String")
    yield ext("general-names", "general-name-constructed-dns", False, seq(tlv(0xA2, tlv(0x82, b"x"))),
              "a dNSName in constructed form")
    yield ext("general-names", "general-name-unknown-choice", False, seq(tlv(0x89, b"x")),
              "context tag 9 is not a GeneralName choice")
    yield ext("general-names", "general-name-bad-registered-id", False, seq(tlv(0x88, b"\x80\x01")),
              "a registeredID with a nonminimal arc (X.690 8.19.2)")
    yield ext("general-names", "general-names-empty", False, seq(), "GeneralNames is SIZE (1..MAX)")
    cps = seq(oid(1, 3, 6, 1, 5, 5, 7, 2, 1), tlv(22, b"https://example.test/cps"))
    yield ext("certificate-policies", "certificate-policies-qualified", True,
              seq(seq(oid(1, 2, 3, 4), seq(cps)), seq(oid(2, 5, 29, 32, 0))),
              "a qualified policy and anyPolicy (RFC 5280 4.2.1.4)")
    yield ext("certificate-policies", "certificate-policies-repeated", False,
              seq(seq(oid(1, 2, 3, 4)), seq(oid(1, 2, 3, 4))),
              "a policy OID appears twice (RFC 5280 4.2.1.4)")
    yield ext("certificate-policies", "certificate-policies-empty-qualifiers", False,
              seq(seq(oid(1, 2, 3, 4), seq())), "policyQualifiers is SIZE (1..MAX)")
    yield ext("policy-constraints", "policy-constraints-inhibit", True, seq(tlv(0x81, b"\x00")),
              "inhibitPolicyMapping alone (RFC 5280 4.2.1.11)")
    yield ext("policy-constraints", "policy-constraints-empty", False, seq(),
              "neither counter is present (RFC 5280 4.2.1.11)")
    yield ext("policy-constraints", "policy-constraints-negative", False, seq(tlv(0x80, b"\xff")),
              "a negative skip count (SkipCerts is INTEGER (0..MAX))")
    yield ext("name-constraints", "name-constraints-distances", True,
              seq(tlv(0xA1, seq(tlv(0x82, b"example.test"), tlv(0x80, b"\x01"), tlv(0x81, b"\x02")))),
              "excluded subtree with nonzero minimum and a maximum (RFC 5280 4.2.1.10)")
    yield ext("name-constraints", "name-constraints-explicit-minimum", False,
              seq(tlv(0xA0, seq(tlv(0x82, b"example.test"), tlv(0x80, b"\x00")))),
              "minimum encoded as its DEFAULT zero (X.690 11.5)")
    yield ext("name-constraints", "name-constraints-empty", False, seq(),
              "neither permitted nor excluded subtrees (RFC 5280 4.2.1.10)")
    yield ext("policy-mappings", "policy-mappings-pair", True, seq(seq(oid(1, 2, 3, 4), oid(1, 2, 3, 5))),
              "issuerDomainPolicy then subjectDomainPolicy (RFC 5280 4.2.1.5)")
    yield ext("policy-mappings", "policy-mappings-three-oids", False,
              seq(seq(oid(1, 2, 3, 4), oid(1, 2, 3, 5), oid(1, 2, 3, 6))), "a mapping has exactly two OIDs")
    yield ext("access-descriptions", "access-descriptions-ca-issuers", True,
              seq(seq(oid(1, 3, 6, 1, 5, 5, 7, 48, 2), tlv(0x86, b"https://example.test/ca.cer"))),
              "a caIssuers location (RFC 5280 4.2.2.1)")
    yield ext("object-identifiers", "extended-key-usage-empty", False, seq(),
              "ExtKeyUsageSyntax is SIZE (1..MAX) (RFC 5280 4.2.1.12)")


def signed_data() -> Iterator[Vector]:
    yield case("cms", "cms-detached", True, cms(), CMS, "detached id-data SignedData, v1 signer")
    yield case("cms", "cms-key-identifier", True, cms(key_id=True), CMS,
               "subjectKeyIdentifier signer: SignerInfo and SignedData v3 (RFC 5652 5.1, 5.3)")
    yield case("cms-ber", "cms-ber-content", True, cms(ber=True, content=True), BER + CMS,
               "indefinite SignedData with constructed encapsulated octets")
    yield case("cms", "cms-other-revocation", True,
               cms(version=integer(5), revocation=tlv(0xA1, oid(1, 3, 6, 1, 5, 5, 7, 16, 2) + seq())), CMS,
               "OtherRevocationInfoFormat requires SignedData v5 (RFC 5652 5.1)")
    yield case("cms", "cms-attribute-certificate", True,
               cms(version=integer(3), certificates=tlv(0xA1, b"")), CMS,
               "a v1 attribute certificate requires SignedData v3 (RFC 5652 5.1)")
    yield case("cms", "cms-invalid-certificate-choice", False, cms(certificates=tlv(0xA4, b"")), CMS,
               "context tag 4 is not a CertificateChoices alternative (RFC 5652 10.2.2)")
    yield case("cms", "cms-signer-version-mismatch", False,
               cms(key_id=True, signer_version=integer(1)), CMS,
               "subjectKeyIdentifier requires SignerInfo v3 (RFC 5652 5.3)")
    yield case("cms", "cms-version-mismatch", False, cms(version=integer(3)), CMS,
               "SignedData v3 for id-data content with v1 signers (RFC 5652 5.1)")
    yield case("cms", "cms-oversized-version", False, cms(version=OVERSIZED), CMS,
               "a version INTEGER wider than 64 bits is malformed", error="syntax")
    malformed = certificate(version_field=explicit(0, OVERSIZED))
    yield case("cms", "cms-malformed-certificate", True,
               cms(certificates=b"".join(sorted([certificate(), malformed]))), CMS,
               "an embedded certificate with a malformed schema is kept apart; the SignedData stays readable")
    unordered = tlv(0x31, b"".join(sorted([SHA256, SHA1], reverse=True)))
    yield case("cms", "cms-unordered-digests", False, cms(digests=unordered), CMS,
               "digestAlgorithms out of DER SET OF order (X.690 11.6)")
    yield case("cms-ber", "cms-ber-unordered-digests", True, cms(digests=unordered, ber=True), BER + CMS,
               "a BER container keeps its digestAlgorithms order as a reported repair")
    yield case("cms", "cms-wrong-attribute-order", False,
               cms(attrs=tlv(0xA0, b"".join(sorted([CONTENT_ATTR, DIGEST_ATTR], reverse=True)))), CMS,
               "signed attributes out of DER SET OF order (X.690 11.6)")
    yield case("cms-ber", "cms-ber-signed-attrs-length", False,
               cms(attrs=b"\xa0\x81" + bytes([len(ATTRS)]) + ATTRS, ber=True), BER + CMS,
               "BER length on signed attributes, which are always DER (RFC 5652 5.3)")
    yield case("cms-ber", "cms-ber-signed-attrs-indefinite", False,
               cms(attrs=b"\xa0\x80" + ATTRS + b"\x00\x00", ber=True), BER + CMS,
               "indefinite signed attributes in a BER container")
    yield case("cms", "cms-missing-digest", False, cms(attrs=tlv(0xA0, CONTENT_ATTR)), CMS,
               "message-digest attribute absent (RFC 5652 11.2)")
    yield case("cms", "cms-duplicate-digest", False,
               cms(attrs=tlv(0xA0, b"".join(sorted([CONTENT_ATTR, DIGEST_ATTR, DIGEST_ATTR])))), CMS,
               "message-digest attribute repeated (RFC 5652 11.2)")
    wrong_content = seq(oid(1, 2, 840, 113549, 1, 9, 3), set_of(oid(1, 2, 3, 4)))
    yield case("cms", "cms-wrong-content-type", False,
               cms(attrs=tlv(0xA0, b"".join(sorted([wrong_content, DIGEST_ATTR])))), CMS,
               "content-type attribute differs from eContentType (RFC 5652 11.1)")
    yield case("cms-ber", "cms-ber-unordered-attributes", False,
               cms(attrs=tlv(0xA0, b"".join(sorted([CONTENT_ATTR, DIGEST_ATTR], reverse=True))), ber=True),
               BER + CMS, "unordered signed attributes stay invalid in a BER container")
    yield case("cms", "cms-extra-field", False, cms(extra=integer(9)), CMS, "a field after signerInfos")
    yield case("signed-set", "cms-signature-input", True, tlv(0x31, ATTRS), CMS,
               "the DER SET that cms-detached's signer signs (RFC 5652 5.4)")


def timestamps() -> Iterator[Vector]:
    yield case("timestamp", "tst-all-fields", True, timestamp(), TIMESTAMP, "TSTInfo with every optional field")
    yield case("timestamp", "tst-minimal", True, timestamp(optional=False, fraction=b""), TIMESTAMP,
               "TSTInfo with required fields only and whole seconds")
    yield case("timestamp", "tst-negative-serial-nonce", True,
               timestamp(serial=integer(-5), nonce=integer(-129)), TIMESTAMP,
               "RFC 3161 2.4.2 restricts neither the serial number's nor the nonce's sign")
    yield case("timestamp", "tst-fraction-trailing-zero", False, timestamp(fraction=b".120"), TIMESTAMP,
               "fraction with a trailing zero (RFC 3161 2.4.2; X.690 11.7.3)")
    yield case("timestamp", "tst-default-ordering", False, timestamp(ordering=FALSE), TIMESTAMP,
               "ordering encoded as its DEFAULT FALSE (X.690 11.5)")
    yield case("timestamp", "tst-invalid-millis", False, timestamp(accuracy=seq(tlv(0x80, b"\x00"))), TIMESTAMP,
               "millis outside 1..999 (RFC 3161 2.4.2)")
    yield case("timestamp", "tst-invalid-micros", False, timestamp(accuracy=seq(tlv(0x81, b"\x03\xe8"))),
               TIMESTAMP, "micros outside 1..999 (RFC 3161 2.4.2)")
    yield case("timestamp", "tst-version-2", False, timestamp(version=integer(2)), TIMESTAMP,
               "TSTInfo version other than v1 (RFC 3161 2.4.2)")


def signatures() -> Iterator[Vector]:
    yield case("ecdsa", "ecdsa-signature", True, seq(integer(1), integer(255)), ECDSA,
               "two nonnegative INTEGERs (RFC 3279 2.2.3)")
    yield case("ecdsa", "ecdsa-negative", False, seq(integer(1), integer(-1)), ECDSA,
               "a negative s cannot be a signature component")
    yield case("ecdsa", "ecdsa-extra-field", False, seq(integer(1), integer(2), integer(3)), ECDSA,
               "a third INTEGER")


# The identifiers pagura-pki names, in the order tests/fixtures.rs lists them.
NAMED_OIDS = (
    (2, 5, 29, 35), (2, 5, 29, 14), (2, 5, 29, 19), (2, 5, 29, 15), (2, 5, 29, 37), (2, 5, 29, 32),
    (2, 5, 29, 33), (2, 5, 29, 30), (2, 5, 29, 36), (2, 5, 29, 54), (2, 5, 29, 17), (2, 5, 29, 31),
    (1, 3, 6, 1, 5, 5, 7, 1, 1), (1, 2, 840, 113549, 1, 7, 1), (1, 2, 840, 113549, 1, 7, 2),
    (1, 2, 840, 113549, 1, 9, 3), (1, 2, 840, 113549, 1, 9, 4), (1, 2, 840, 113549, 1, 9, 16, 1, 4),
    # oids: digests, keys, ESS/CMS attributes, curves, then signature algorithms.
    (1, 3, 14, 3, 2, 26), (2, 16, 840, 1, 101, 3, 4, 2, 4), (2, 16, 840, 1, 101, 3, 4, 2, 1),
    (2, 16, 840, 1, 101, 3, 4, 2, 2), (2, 16, 840, 1, 101, 3, 4, 2, 3), (1, 2, 840, 113549, 1, 1, 1),
    (1, 2, 840, 113549, 1, 1, 10), (1, 2, 840, 113549, 1, 1, 8), (1, 2, 840, 10045, 2, 1),
    (1, 2, 840, 113549, 1, 9, 16, 2, 12), (1, 2, 840, 113549, 1, 9, 16, 2, 47), (1, 2, 840, 113549, 1, 9, 52),
    (1, 2, 840, 10045, 3, 1, 7), (1, 3, 132, 0, 34), (1, 3, 132, 0, 35),
    (1, 2, 840, 113549, 1, 1, 5), (1, 2, 840, 10045, 4, 1), (1, 2, 840, 113549, 1, 1, 14),
    (1, 2, 840, 10045, 4, 3, 1), (1, 2, 840, 113549, 1, 1, 11), (1, 2, 840, 10045, 4, 3, 2),
    (1, 2, 840, 113549, 1, 1, 12), (1, 2, 840, 10045, 4, 3, 3), (1, 2, 840, 113549, 1, 1, 13),
    (1, 2, 840, 10045, 4, 3, 4),
)


def cases() -> Iterator[Vector]:
    yield case("oids", "named-oids", True, seq(*(oid(*arcs) for arcs in NAMED_OIDS)), DER,
               "the OBJECT IDENTIFIERs pagura-pki names as constants, encoded from their dotted form")
    yield from certificates()
    yield from crls()
    yield from extensions()
    yield from signed_data()
    yield from timestamps()
    yield from signatures()
