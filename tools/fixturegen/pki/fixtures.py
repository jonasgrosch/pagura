"""Independent ASN.1 examples derived from X.690 and the RFC schemas.

Python standard library only. Output: kind, case ID, expected parse result, hex.
These are synthetic envelopes; none contains a cryptographically valid signature.
"""


def tlv(tag, value):
    size = len(value)
    length = bytes([size]) if size < 128 else size.to_bytes((size.bit_length() + 7) // 8, "big")
    if size >= 128:
        length = bytes([128 + len(length)]) + length
    return bytes([tag]) + length + value


def seq(*values):
    return tlv(0x30, b"".join(values))


def integer(value):
    data = value.to_bytes(max(1, (value.bit_length() + 8) // 8), "big", signed=True)
    return tlv(2, data)


def oid(*arcs):
    values = [arcs[0] * 40 + arcs[1], *arcs[2:]]
    output = bytearray()
    for value in values:
        digits = [value & 127]
        while value > 127:
            value >>= 7
            digits.append(128 | (value & 127))
        output.extend(reversed(digits))
    return tlv(6, bytes(output))


def octets(data):
    return tlv(4, data)


def bitstring(data, unused=0):
    return tlv(3, bytes([unused]) + data)


def set_of(*values):
    return tlv(0x31, b"".join(sorted(values)))


def explicit(number, value):
    return tlv(0xA0 + number, value)


def extension(number, value, critical=False):
    return seq(oid(2, 5, 29, number), b"\x01\x01\xff" if critical else b"", octets(value))


SHA256 = seq(oid(2, 16, 840, 1, 101, 3, 4, 2, 1), b"\x05\x00")
RSA = seq(oid(1, 2, 840, 113549, 1, 1, 1), b"\x05\x00")
RSA_SHA256 = seq(oid(1, 2, 840, 113549, 1, 1, 11), b"\x05\x00")
ID_DATA = oid(1, 2, 840, 113549, 1, 7, 1)
ID_SIGNED_DATA = oid(1, 2, 840, 113549, 1, 7, 2)
NAME = seq(set_of(seq(oid(2, 5, 4, 3), tlv(12, b"Example")), seq(oid(2, 5, 4, 10), tlv(19, b"Pagura"))))
BEFORE = tlv(23, b"240101000000Z")
AFTER = tlv(24, b"20500101000000Z")
SPKI = seq(RSA, bitstring(seq(integer(257), integer(65537))))
EXTENSIONS = seq(
    extension(19, seq(b"\x01\x01\xff", integer(2)), True),
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


def certificate(version=3, extensions=EXTENSIONS, algorithm=RSA_SHA256):
    version_field = explicit(0, integer(version - 1)) if version != 1 else b""
    tbs = seq(version_field, integer(7), RSA_SHA256, NAME, seq(BEFORE, AFTER), NAME, SPKI,
              explicit(3, extensions) if extensions else b"")
    return seq(tbs, algorithm, bitstring(b"unverified"))


def crl():
    entry = seq(integer(7), BEFORE, seq(extension(21, tlv(10, b"\x01"))))
    tbs = seq(integer(1), RSA_SHA256, NAME, BEFORE, AFTER, seq(entry), explicit(0, seq(extension(20, integer(2)))))
    return seq(tbs, RSA_SHA256, bitstring(b"unverified"))


CONTENT_ATTR = seq(oid(1, 2, 840, 113549, 1, 9, 3), set_of(ID_DATA))
DIGEST_ATTR = seq(oid(1, 2, 840, 113549, 1, 9, 4), set_of(octets(bytes(range(32)))))
ATTRS = b"".join(sorted([CONTENT_ATTR, DIGEST_ATTR]))


def cms(attrs=None, ber=False, key_id=False, content=False, certificates=True, extra=b""):
    if attrs is None:
        attrs = tlv(0xA0, ATTRS)
    signer = seq(integer(3 if key_id else 1), tlv(0x80, b"key1") if key_id else seq(NAME, integer(7)),
                 SHA256, attrs, RSA_SHA256, octets(b"signature"))
    encap = seq(ID_DATA, explicit(0, b"\x24\x80" + octets(b"abc") + tlv(0x24, octets(b"def")) + b"\x00\x00") if content else b"")
    fields = integer(3 if key_id else 1) + set_of(SHA256) + encap
    if certificates:
        fields += tlv(0xA0, certificate()) + tlv(0xA1, crl())
    fields += set_of(signer) + extra
    signed_data = b"\x30\x80" + fields + b"\x00\x00" if ber else tlv(0x30, fields)
    return seq(ID_SIGNED_DATA, explicit(0, signed_data))


def timestamp(optional=True, fraction=b".125", accuracy=None, ordering=None):
    if accuracy is None:
        accuracy = seq(integer(2), tlv(0x80, b"\x01"), tlv(0x81, b"\x03\xe7"))
    if ordering is None:
        ordering = b"\x01\x01\xff"
    return seq(integer(1), oid(1, 2, 3, 4), seq(SHA256, octets(bytes(range(32)))), integer(123),
               tlv(24, b"20261018120000" + fraction + b"Z"),
               accuracy + ordering + integer(256) + explicit(0, explicit(4, NAME)) + tlv(0xA1, extension(20, integer(1))) if optional else b"")


def cases():
    yield "certificate", "x509-v3", True, certificate()
    yield "certificate", "x509-v1", True, certificate(1, b"")
    yield "certificate", "x509-algorithm-mismatch", False, certificate(algorithm=RSA)
    yield "certificate", "x509-duplicate-extension", False, certificate(extensions=seq(extension(19, seq()), extension(19, seq())))
    yield "certificate", "x509-explicit-default-version", False, seq(seq(explicit(0, integer(0)), integer(7), RSA_SHA256, NAME, seq(BEFORE, AFTER), NAME, SPKI), RSA_SHA256, bitstring(b"x"))
    yield "certificate", "x509-explicit-default-critical", False, certificate(extensions=seq(seq(oid(2, 5, 29, 19), b"\x01\x01\x00", octets(seq()))))
    yield "crl", "x509-crl-v2", True, crl()
    yield "cms", "cms-detached", True, cms()
    yield "cms", "cms-key-identifier", True, cms(key_id=True)
    yield "cms-ber", "cms-ber-content", True, cms(ber=True, content=True)
    yield "cms", "cms-wrong-attribute-order", False, cms(attrs=tlv(0xA0, b"".join(sorted([CONTENT_ATTR, DIGEST_ATTR], reverse=True))))
    yield "cms-ber", "cms-ber-signed-attrs-length", False, cms(attrs=b"\xa0\x81" + bytes([len(ATTRS)]) + ATTRS, ber=True)
    yield "cms-ber", "cms-ber-signed-attrs-indefinite", False, cms(attrs=b"\xa0\x80" + ATTRS + b"\x00\x00", ber=True)
    yield "cms", "cms-missing-digest", False, cms(attrs=tlv(0xA0, CONTENT_ATTR))
    yield "cms", "cms-duplicate-digest", False, cms(attrs=tlv(0xA0, b"".join(sorted([CONTENT_ATTR, DIGEST_ATTR, DIGEST_ATTR]))))
    wrong_content = seq(oid(1, 2, 840, 113549, 1, 9, 3), set_of(oid(1, 2, 3, 4)))
    yield "cms", "cms-wrong-content-type", False, cms(attrs=tlv(0xA0, b"".join(sorted([wrong_content, DIGEST_ATTR]))))
    yield "cms-ber", "cms-ber-unordered-attributes", False, cms(attrs=tlv(0xA0, b"".join(sorted([CONTENT_ATTR, DIGEST_ATTR], reverse=True))), ber=True)
    yield "cms", "cms-extra-field", False, cms(extra=integer(9))
    yield "timestamp", "tst-all-fields", True, timestamp()
    yield "timestamp", "tst-minimal", True, timestamp(optional=False, fraction=b"")
    yield "timestamp", "tst-fraction-trailing-zero", False, timestamp(fraction=b".120")
    yield "timestamp", "tst-default-ordering", False, timestamp(ordering=b"\x01\x01\x00")
    yield "timestamp", "tst-invalid-millis", False, timestamp(accuracy=seq(tlv(0x80, b"\x00")))
    yield "signed-set", "cms-signature-input", True, tlv(0x31, ATTRS)


if __name__ == "__main__":
    for kind, name, success, data in cases():
        print(kind, name, int(success), data.hex(), sep="\t")
