"""RFC 5280 §§5.2–5.3/6.3 CRLs, constructed independently of the engine.

Structural fixtures use explicit synthetic signature attestations; PKITS tests
use exact records from independent RSA/DSA arithmetic instead.
"""
from fixtures import seq, explicit, integer, tlv, bitstring, extension, RSA_SHA256
from paths import ROOT, CA, LEAF, certificate, CA_BASIC


def point(uri, reasons=None):
    fields = explicit(0, tlv(0xa0, tlv(0x86, uri)))
    if reasons is not None:
        fields += tlv(0x81, reasons)
    return seq(fields)


def idp(uri=None, reasons=None):
    return extension(28, seq(explicit(0, tlv(0xa0, tlv(0x86, uri))) if uri else b'',
                             tlv(0x83, reasons) if reasons is not None else b''), True)


def revoked(serial=3, reason=None, date=b'241201000000Z', extra=()):
    extensions = [extension(21, tlv(10, bytes([reason])))] if reason is not None else []
    extensions += list(extra)
    return seq(integer(serial), tlv(23, date), seq(*extensions) if extensions else b'')


def crl(issuer=CA, entries=(), extensions=(), this=b'240101000000Z', next=b'300101000000Z', signature=b'crl'):
    body = seq(integer(1), RSA_SHA256, issuer, tlv(23, this), tlv(23, next) if next else b'',
               seq(*entries) if entries else b'', explicit(0, seq(*extensions)) if extensions else b'')
    return seq(body, RSA_SHA256, bitstring(signature))


def number(n):
    return extension(20, integer(n))


def delta(base, n):
    return (number(n), extension(27, integer(base), True))


URI = b'https://example.test/crl/a'
OTHER = b'https://example.test/crl/b'
FRESH = extension(46, seq(point(URI)))
BASE = crl(extensions=(number(7),))


def cases():
    yield 'good', 'Good', (), (crl(),)
    yield 'no-data', 'Unknown:NoData', (), ()
    yield 'stale', 'Unknown:Stale', (), (crl(next=b'240601000000Z'),)
    yield 'missing-next', 'Unknown:Stale', (), (crl(next=None),)
    yield 'future', 'Unknown:NotYetCurrent', (), (crl(this=b'260101000000Z'),)
    yield 'next-boundary', 'Unknown:Stale', (), (crl(next=b'250101000000Z'),)
    yield 'this-boundary', 'Good', (), (crl(this=b'250101000000Z'),)
    yield 'reversed-interval', 'Unknown:Stale', (), (crl(this=b'240601000000Z', next=b'240101000000Z'),)
    yield 'bad-signature', 'Unknown:VerificationFailed', (), (crl(signature=b'invalid'),)
    yield 'unavailable-signature', 'Unknown:VerificationFailed', (), (crl(signature=b'unavailable'),)
    yield 'critical-crl', 'Unknown:UnsupportedCriticalExtension', (), (crl(extensions=(extension(99, seq(), True),)),)
    yield 'noncritical-crl', 'Good', (), (crl(extensions=(extension(99, seq()),)),)
    yield 'critical-other-entry', 'Unknown:UnsupportedCriticalExtension', (), (crl(entries=(revoked(99, extra=(extension(99, seq(), True),)),)),)
    yield 'noncritical-entry', 'Revoked:Unspecified', (), (crl(entries=(revoked(extra=(extension(99, seq()),)),)),)
    yield 'revoked-reason-time', 'Revoked:KeyCompromise', (), (crl(entries=(revoked(reason=1),)),)
    yield 'future-revocation', 'Revoked:Unspecified', (), (crl(entries=(revoked(date=b'260101000000Z'),)),)
    yield 'matching-location', 'Good', (extension(31, seq(point(URI))),), (crl(extensions=(idp(URI),)),)
    yield 'different-uri-path', 'Unknown:OutOfScope', (extension(31, seq(point(URI))),), (crl(extensions=(idp(OTHER),)),)
    yield 'critical-dp-processed', 'Good', (extension(31, seq(point(URI)), True),), (crl(extensions=(idp(URI),)),)
    yield 'partial-reasons', 'Unknown:IncompleteReasons', (), (crl(extensions=(idp(reasons=b'\x06\x40'),)),)
    yield 'reason-union', 'Good', (), (crl(extensions=(idp(reasons=b'\x06\x40'),)), crl(extensions=(idp(reasons=b'\x07\xbf\x80'),)))
    yield 'delta-without-base', 'Unknown:MissingBase', (), (crl(extensions=delta(7, 8)),)
    yield 'missing-advertised-delta-crl', 'Unknown:MissingDelta', (), (crl(extensions=(number(7), FRESH)),)
    yield 'missing-advertised-delta-cert', 'Unknown:MissingDelta', (FRESH,), (BASE,)
    yield 'delta-good', 'Good', (FRESH,), (BASE, crl(extensions=delta(7, 8)))
    yield 'delta-stale-base', 'Good', (), (crl(extensions=(number(7),), next=b'240601000000Z'), crl(extensions=delta(7, 8)))
    yield 'delta-base-too-old', 'Unknown:Stale', (), (crl(extensions=(number(7),), next=b'240601000000Z'), crl(extensions=delta(8, 9)))
    yield 'delta-revoked', 'Revoked:CaCompromise', (), (BASE, crl(entries=(revoked(reason=2),), extensions=delta(7, 8)))
    yield 'delta-release-hold', 'Good', (), (crl(entries=(revoked(reason=6),), extensions=(number(7),)), crl(entries=(revoked(reason=8),), extensions=delta(7, 8)))
    yield 'delta-release-absent', 'Good', (), (BASE, crl(entries=(revoked(reason=8),), extensions=delta(7, 8)))
    yield 'delta-release-permanent', 'Unknown:Malformed', (), (crl(entries=(revoked(reason=1),), extensions=(number(7),)), crl(entries=(revoked(reason=8),), extensions=delta(7, 8)))
    yield 'remove-from-complete', 'Unknown:Malformed', (), (crl(entries=(revoked(reason=8),)),)
    yield 'delta-different-scope', 'Unknown:MissingDelta', (), (crl(extensions=(number(7), FRESH)), crl(extensions=(*delta(7, 8), idp(URI))))
    yield 'delta-number-over-u64', 'Good', (), (crl(extensions=(number(2**160),)), crl(extensions=delta(2**160, 2**160+1)))
    yield 'delta-number-order', 'Good', (), (BASE, crl(entries=(revoked(reason=1),), extensions=delta(7, 255)), crl(entries=(revoked(reason=8),), extensions=delta(7, 256)))
    yield 'latest-complete-release', 'Good', (), (crl(entries=(revoked(reason=6),), extensions=(number(7),)), crl(this=b'241201000000Z', extensions=(number(8),)))
    yield 'latest-complete-revoked', 'Revoked:KeyCompromise', (), (BASE, crl(this=b'241201000000Z', entries=(revoked(reason=1),), extensions=(number(8),)))
    yield 'nonminimal-reason-flags', 'Unknown:Malformed', (), (crl(extensions=(idp(reasons=b'\x00\x40'),)),)
    yield 'all-reason-flags', 'Good', (), (crl(extensions=(idp(reasons=b'\x07\x7f\x80'),)),)
    yield 'critical-freshest-cert', 'Unknown:Malformed', (extension(46, seq(point(URI)), True),), (BASE, crl(extensions=delta(7, 8)))
    yield 'freshest-crl-reasons', 'Unknown:Malformed', (), (crl(extensions=(number(7), extension(46, seq(point(URI, b'\x06\x40'))))),)
    yield 'freshest-in-delta', 'Unknown:Malformed', (), (crl(extensions=(number(7), FRESH)), crl(extensions=(*delta(7, 8), FRESH)))
    yield 'empty-certificate-dp', 'Unknown:Malformed', (extension(31, seq()),), (crl(),)
    yield 'certificate-signature-unknown', 'Signature', (), (crl(),)
    yield 'signature-budget', 'Limit', (), (crl(),)
    yield 'invalid-basic-with-signature-budget', 'InvalidBound', (extension(15, bitstring(b'\x20', 5)),), (crl(),)
    yield 'no-host-anchor', 'Untrusted', (), (crl(),)
    yield 'memo-repeated-crls', 'Good', (), (crl(), crl(), crl())


def main():
    root_crl = crl(ROOT, signature=b'root-crl')
    for ident, expected, extensions, crls in cases():
        certs = [certificate(ROOT, ROOT, 1), certificate(CA, ROOT, 2, (CA_BASIC,)), certificate(LEAF, CA, 3, extensions)]
        print('CASE', ident, expected, *(c.hex() for c in certs), *(c.hex() for c in (root_crl, *crls)), sep='\t')
        if len(crls) > 1:
            print('CASE', ident + '-reversed', expected, *(c.hex() for c in certs), *(c.hex() for c in (root_crl, *reversed(crls))), sep='\t')


if __name__ == '__main__':
    main()
