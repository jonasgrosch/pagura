"""Independent RFC 5280 6.1/6.3 assertions for isolated candidate failures.

Exact synthetic signature attestations bind each signed object to its issuer
key. They test state transitions, not signature arithmetic (covered by PKITS).
"""
from .fixtures import seq, explicit, integer, bitstring, extension, oid, tlv, RSA, RSA_SHA256
from .paths import name, GOOD_TIME, CA_BASIC
from .crls import point, URI


KEYS = {n: seq(integer(257 + n * 2), integer(65537)) for n in (*range(1, 12), 99)}
ROOT, CA, TARGET, INTERMEDIATE = map(name, ('Root', 'Issuer', 'Target', 'Intermediate'))
JUNK = name('Íssuer')
OBJECTS = []


def signed(kind, ident, body, signature, issuer, status='1'):
    value = seq(body, RSA_SHA256, bitstring(signature))
    print(kind, ident, value.hex(), sep='\t')
    OBJECTS.append((body, signature, issuer, status))
    return value


def cert(ident, subject, issuer_name, serial, public_key, issuer_key, extensions=(), status='1'):
    body = seq(explicit(0, integer(2)), integer(serial), RSA_SHA256, issuer_name,
               GOOD_TIME, subject, seq(RSA, bitstring(KEYS[public_key])),
               explicit(3, seq(*extensions)) if extensions else b'')
    signed('CERT', ident, body, ident.encode(), issuer_key, status)


def list_(ident, issuer_name, issuer_key, status='1'):
    # Build the signed range directly, independently of the Rust reader.
    body = seq(integer(1), RSA_SHA256, issuer_name, tlv(23, b'240101000000Z'), tlv(23, b'300101000000Z'))
    signed('CRL', ident, body, ident.encode(), issuer_key, status)


def case(ident, target, expected, pool, crls, anchor='root'):
    print('CASE', ident, target, expected, anchor, ','.join(pool), ','.join(crls), sep='\t')


def main():
    critical_dp = extension(31, seq(point(URI)), True)
    cert('root', ROOT, ROOT, 1, 1, 1)
    cert('ca', CA, ROOT, 10, 10, 1, (CA_BASIC,))
    cert('target', TARGET, CA, 3, 3, 10)
    cert('critical-ca', CA, ROOT, 2, 2, 1, (CA_BASIC, critical_dp))
    cert('unknown-target', TARGET, CA, 3, 3, 2, status='?')
    cert('intermediate', INTERMEDIATE, ROOT, 2, 2, 1, (CA_BASIC, critical_dp))
    cert('unknown-signer', CA, INTERMEDIATE, 4, 4, 2, (CA_BASIC,), status='?')
    cert('verified-signer', CA, INTERMEDIATE, 5, 4, 2, (CA_BASIC,))
    cert('junk', JUNK, JUNK, 99, 99, 99, (CA_BASIC,), status='0')
    opaque = name('Róót')
    cert('opaque-root', opaque, opaque, 21, 1, 1)
    cert('opaque-ca', CA, opaque, 22, 10, 1, (CA_BASIC,))
    cert('opaque-target', TARGET, JUNK, 23, 3, 99)
    # An extra AVA field is DER but not a supported relative directory name.
    malformed_relative = explicit(0, tlv(0xa1, seq(oid(2, 5, 4, 3), tlv(12, b'point'), integer(1))))
    cert('junk-relative', JUNK, JUNK, 98, 99, 99,
         (CA_BASIC, extension(31, seq(seq(malformed_relative)))), status='0')
    list_('opaque-crl', opaque, 1)
    list_('root-crl', ROOT, 1)
    list_('ca-crl', CA, 10)
    list_('critical-ca-crl', CA, 2)
    list_('signer-crl', CA, 4)
    list_('intermediate-crl', INTERMEDIATE, 2)
    list_('junk-crl', JUNK, 99, status='0')
    case('deferred-crl-then-unknown-signature', 'unknown-target', 'Signature',
         ['root', 'critical-ca', 'unknown-target'], ['root-crl', 'critical-ca-crl'])
    for signer, expected in [('unknown-signer', 'Unknown'), ('verified-signer', 'Valid')]:
        case(signer + '-critical-ancestor', 'target', expected,
             ['root', 'ca', 'target', 'intermediate', signer],
             ['root-crl', 'intermediate-crl', 'signer-crl'])
    case('clean', 'target', 'Valid', ['root', 'ca', 'target'], ['root-crl', 'ca-crl'])
    case('opaque-anchor', 'target', 'Valid', ['opaque-root', 'opaque-ca', 'target'],
         ['opaque-crl', 'ca-crl'], anchor='opaque-root')
    case('unsupported-chain', 'opaque-target', 'NameForm', ['root', 'opaque-target'], ['root-crl'])
    for position in ['before', 'after']:
        pool = ['root', 'ca', 'target']
        pool = ['junk-relative', *pool] if position == 'before' else [*pool, 'junk-relative']
        case('relative-point-' + position, 'target', 'Valid', pool, ['root-crl', 'ca-crl'])
    pool, crls = ['root'], ['root-crl']
    for i in range(10):
        ident = f'branch-ca-{i}'
        subject = name(f'Branch {i}')
        cert(ident, subject, ROOT, 40 + i, i + 2, 1, (CA_BASIC,))
        list_(ident + '-crl', subject, i + 2)
        pool.append(ident)
        crls.append(ident + '-crl')
    cert('branch-target', TARGET, name('Branch 0'), 50, 3, 2)
    case('signer-filtering', 'branch-target', 'Valid', [*pool, 'branch-target'], crls)
    for position in ['before', 'after']:
        for junk_cert, junk_crl in [(True, False), (False, True), (True, True)]:
            pool = ['root', 'ca', 'target']
            crls = ['root-crl', 'ca-crl']
            if junk_cert:
                pool = ['junk', *pool] if position == 'before' else [*pool, 'junk']
            if junk_crl:
                crls = ['junk-crl', *crls] if position == 'before' else [*crls, 'junk-crl']
            case(f'junk-{position}-{junk_cert}-{junk_crl}', 'target', 'Valid', pool, crls)
    for body, signature, issuer, status in OBJECTS:
        for key_id, key in KEYS.items():
            result = status if key_id == issuer else '0'
            print('SIGNATURE', RSA_SHA256.hex(), body.hex(), (b'\0' + signature).hex(),
                  '2a864886f70d010101', '-', (b'\0' + key).hex(), result, sep='\t')

