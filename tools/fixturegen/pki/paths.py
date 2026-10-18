"""Independent RFC 5280/9549/9598 path assertions, standard library only.

The signature callback in these structural tests explicitly attests synthetic
inputs. Mathematical signatures are exercised separately by PKITS records.
"""
from fixtures import seq, set_of, oid, tlv, explicit, integer, bitstring, extension, RSA_SHA256, SPKI


def name(text, tag=12):
    return seq(set_of(seq(oid(2, 5, 4, 3), tlv(tag, text.encode('utf8')))))


ROOT, CA, LEAF = name('Root'), name('Issuer'), name('Target')
GOOD_TIME = seq(tlv(23, b'240101000000Z'), tlv(23, b'300101000000Z'))
CA_BASIC = extension(19, seq(b'\x01\x01\xff'), True)
P1, P2, ANY = oid(1, 2, 3, 1), oid(1, 2, 3, 2), oid(2, 5, 29, 32, 0)


def certificate(subject, issuer, serial, extensions=(), validity=GOOD_TIME):
    body = seq(explicit(0, integer(2)), integer(serial), RSA_SHA256, issuer,
               validity, subject, SPKI, explicit(3, seq(*extensions)) if extensions else b'')
    return seq(body, RSA_SHA256, bitstring(b'synthetic-' + bytes([serial])))


def nc(permitted=(), excluded=()):
    return extension(30, seq(tlv(0xa0, b''.join(seq(v) for v in permitted)) if permitted else b'',
                             tlv(0xa1, b''.join(seq(v) for v in excluded)) if excluded else b''), True)


def san(*names):
    return extension(17, seq(*names))


def smtp(local, domain):
    return tlv(0xa0, oid(1, 3, 6, 1, 5, 5, 7, 8, 9) + explicit(0, tlv(12, (local + '@' + domain).encode('utf8'))))


def policies(*ids):
    return extension(32, seq(*(seq(p) for p in ids)), True)


def cases():
    yield 'host-anchor', 'valid', (), (), LEAF, CA
    yield 'target-key-usage', 'KeyUsage', (), (extension(15, bitstring(b'\x20', 5)),), LEAF, CA
    yield 'ca-key-usage', 'KeyCertSign', (extension(15, bitstring(b'\x80', 7)),), (), LEAF, CA
    yield 'target-eku', 'ExtendedKeyUsage', (), (extension(37, seq(oid(1, 3, 6, 1, 5, 5, 7, 3, 1))),), LEAF, CA
    yield 'ca-eku', 'ExtendedKeyUsage', (extension(37, seq(oid(1, 3, 6, 1, 5, 5, 7, 3, 1))),), (), LEAF, CA
    yield 'any-eku', 'valid', (), (extension(37, seq(oid(2, 5, 29, 37, 0))),), LEAF, CA
    yield 'unknown-critical', 'CriticalExtension', (), (extension(99, seq(), True),), LEAF, CA
    yield 'unknown-noncritical', 'valid', (), (extension(99, seq()),), LEAF, CA
    yield 'unicode-casefold', 'valid', (), (), LEAF, name('ISSUER', 19)
    yield 'unicode-compatibility', 'NameForm', (), (), LEAF, name('Ｉｓｓｕｅｒ')
    yield 'directory-subtree', 'valid', (nc([explicit(4, LEAF)]),), (), LEAF, CA
    yield 'directory-excluded', 'NameConstraint', (nc(excluded=[explicit(4, LEAF)]),), (), LEAF, CA
    yield 'dns-descendant', 'valid', (nc([tlv(0x82, b'example.test')]),), (san(tlv(0x82, b'www.example.test')),), LEAF, CA
    yield 'dns-leading-dot-base', 'NameConstraint', (nc([tlv(0x82, b'.example.test')]),), (san(tlv(0x82, b'example.test')),), LEAF, CA
    yield 'dns-label-boundary', 'NameConstraint', (nc([tlv(0x82, b'example.test')]),), (san(tlv(0x82, b'evil-example.test')),), LEAF, CA
    yield 'dns-case', 'valid', (nc([tlv(0x82, b'EXAMPLE.test')]),), (san(tlv(0x82, b'www.example.TEST')),), LEAF, CA
    yield 'mail-host', 'valid', (nc([tlv(0x81, b'example.test')]),), (san(tlv(0x81, b'Me@example.test')),), LEAF, CA
    yield 'mail-host-no-subdomain', 'NameConstraint', (nc([tlv(0x81, b'example.test')]),), (san(tlv(0x81, b'Me@sub.example.test')),), LEAF, CA
    yield 'smtp-utf8', 'valid', (nc([tlv(0x81, b'XN--BCHER-KVA.test')]),), (san(smtp('mü', 'xn--bcher-kva.test')),), LEAF, CA
    yield 'quoted-smtp-utf8', 'valid', (nc([tlv(0x81, b'example.test')]),), (san(smtp('"mü@quoted"', 'example.test')),), LEAF, CA
    yield 'invalid-mail-local', 'NameConstraint', (nc([tlv(0x81, b'example.test')]),), (san(tlv(0x81, b'me..invalid@example.test')),), LEAF, CA
    yield 'smtp-utf8-excluded', 'NameConstraint', (nc(excluded=[tlv(0x81, b'.test')]),), (san(smtp('mü', 'xn--bcher-kva.test')),), LEAF, CA
    yield 'smtp-utf8-ulabel', 'NameConstraint', (nc([tlv(0x81, b'.test')]),), (san(smtp('mü', 'bücher.test')),), LEAF, CA
    yield 'smtp-ascii-local', 'NameConstraint', (nc([tlv(0x81, b'example.test')]),), (san(smtp('me', 'example.test')),), LEAF, CA
    yield 'legacy-mailbox-constraint', 'Extension', (nc([tlv(0x81, b'me@example.test')]),), (), LEAF, CA
    yield 'mail-base-excluded', 'NameConstraint', (nc([tlv(0x81, b'.example.test')]),), (san(tlv(0x81, b'me@example.test')),), LEAF, CA
    yield 'uri-domain', 'valid', (nc([tlv(0x86, b'.example.test')]),), (san(tlv(0x86, b'https://me@www.example.test:443/path')),), LEAF, CA
    yield 'uri-host-absent', 'NameConstraint', (nc([tlv(0x86, b'.example.test')]),), (san(tlv(0x86, b'mailto:me@example.test')),), LEAF, CA
    yield 'uri-ip-host', 'NameConstraint', (nc([tlv(0x86, b'.example.test')]),), (san(tlv(0x86, b'https://127.0.0.1')),), LEAF, CA
    yield 'ipv4-mask', 'valid', (nc([tlv(0x87, bytes([10, 1, 0, 0, 255, 255, 0, 0]))]),), (san(tlv(0x87, bytes([10, 1, 20, 5]))),), LEAF, CA
    yield 'ipv4-mask-excluded', 'NameConstraint', (nc(excluded=[tlv(0x87, bytes([10, 1, 0, 0, 255, 255, 0, 0]))]),), (san(tlv(0x87, bytes([10, 1, 20, 5]))),), LEAF, CA
    yield 'noncontiguous-ip-mask', 'Extension', (nc([tlv(0x87, bytes([10, 1, 0, 0, 255, 0, 255, 0]))]),), (), LEAF, CA
    yield 'ipv6-mask', 'valid', (nc([tlv(0x87, bytes.fromhex('20010db8000000000000000000000000ffffffff000000000000000000000000'))]),), (san(tlv(0x87, bytes.fromhex('20010db8deadbeef0000000000000001'))),), LEAF, CA
    yield 'positive-subtree-minimum', 'Extension', (extension(30, seq(tlv(0xa0, seq(tlv(0x82, b'example.test'), tlv(0x80, b'\1')))), True),), (), LEAF, CA
    yield 'subtree-maximum', 'Extension', (extension(30, seq(tlv(0xa0, seq(tlv(0x82, b'example.test'), tlv(0x81, b'\1')))), True),), (), LEAF, CA
    yield 'unsupported-constraint', 'CriticalExtension', (nc([tlv(0x88, bytes([42, 3]))]),), (), LEAF, CA
    yield 'policy-present', 'valid', (policies(P1),), (policies(P1),), LEAF, CA
    yield 'policy-mismatch', 'Policy', (policies(P1), extension(36, seq(tlv(0x80, b'\0')), True)), (policies(P2),), LEAF, CA
    yield 'duplicate-policy', 'Extension', (policies(P1, P1),), (), LEAF, CA
    yield 'any-mapping', 'AnyPolicyMapping', (extension(33, seq(seq(P1, ANY)), True),), (), LEAF, CA
    unknown_name = tlv(0xa0, oid(1, 2, 3) + explicit(0, tlv(12, b'opaque')))
    yield 'unknown-critical-san-name', 'valid', (), (extension(17, seq(unknown_name), True),), LEAF, CA
    yield 'unknown-noncritical-san-name', 'valid', (), (san(unknown_name),), LEAF, CA
    qualifier = seq(seq(P1, seq(seq(oid(1, 2, 3), tlv(12, b'opaque')))))
    yield 'critical-policy-qualifier', 'Extension', (), (extension(32, qualifier, True),), LEAF, CA
    yield 'noncritical-policy-qualifier', 'valid', (), (extension(32, qualifier),), LEAF, CA
    yield 'counter-arbitrary-width', 'valid', (extension(54, integer(2**160), True), extension(36, seq(tlv(0x80, b'\1' + b'\0'*20)), True)), (), LEAF, CA


if __name__ == '__main__':
    # A deliberately unusable certificate container for an explicitly trusted
    # name/key: anchor certificate validity and extensions must not be evaluated.
    anchor = certificate(ROOT, ROOT, 1, [extension(99, seq(), True)],
                         seq(tlv(23, b'000101000000Z'), tlv(23, b'010101000000Z')))
    for ident, expected, ca_ext, leaf_ext, subject, issuer in cases():
        ca = certificate(CA, ROOT, 2, [CA_BASIC, *ca_ext])
        leaf = certificate(subject, issuer, 3, leaf_ext)
        print(ident, expected, anchor.hex(), ca.hex(), leaf.hex(), sep='\t')
