"""Standards-derived conservative DN, unconstrained SAN and search cases."""
from .fixtures import seq, set_of, oid, tlv
from .paths import name, certificate, CA_BASIC, san, nc


def distinguished(value, attribute=(2, 5, 4, 3), tag=12):
    return seq(set_of(seq(oid(*attribute), tlv(tag, value.encode()))))


def main():
    root = name('Root')
    issuer = name('Issuer')
    target = name('Target')
    cases = [
        ('wildcard-unconstrained', 'valid', issuer, issuer, (), (san(tlv(0x82, b'*.example.com')),)),
        ('underscore-unconstrained', 'valid', issuer, issuer, (), (san(tlv(0x82, b'_service.example.com')),)),
        ('wildcard-other-constraint', 'valid', issuer, issuer,
         (nc(permitted=(tlv(0x87, bytes([10, 0, 0, 0, 255, 0, 0, 0])),)),), (san(tlv(0x82, b'*.example.com')),)),
        ('wildcard-dns-constraint', 'NameConstraint', issuer, issuer,
         (nc(permitted=(tlv(0x82, b'example.com'),)),), (san(tlv(0x82, b'*.example.com')),)),
        ('unknown-identical', 'valid', distinguished('Jurisdiction', (1, 3, 6, 1, 4, 1, 311, 60, 2, 1, 3)),
         distinguished('Jurisdiction', (1, 3, 6, 1, 4, 1, 311, 60, 2, 1, 3)), (), ()),
        ('unknown-different', 'NameForm', distinguished('Jurisdiction', (1, 2, 3)), distinguished('JURISDICTION', (1, 2, 3)), (), ()),
        ('telephone-matching-rule', 'NameForm', distinguished('12-34', (2, 5, 4, 20)), distinguished('1234', (2, 5, 4, 20)), (), ()),
        ('nonascii-identical', 'valid', name('Straße'), name('Straße'), (), ()),
        ('nonascii-folding', 'NameForm', name('Straße'), name('STRASSE'), (), ()),
        ('nonascii-normalization', 'NameForm', name('Å'), name('A\u030a'), (), ()),
        ('ascii-case-and-spaces', 'valid', name('  IsSuEr  CA ', 19), name('issuer\t\v\f\r\n ca'), (), ()),
        ('ascii-different', 'IssuerName', issuer, name('Other'), (), ()),
    ]
    for ident, expected, ca_name, ee_issuer, ca_ext, ee_ext in cases:
        print('CASE', ident, expected, certificate(root, root, 1).hex(),
              certificate(ca_name, root, 2, (CA_BASIC, *ca_ext)).hex(),
              certificate(target, ee_issuer, 3, ee_ext).hex(), sep='\t')
    print('GRAPH', certificate(root, root, 1).hex(),
          *[certificate(issuer, root, n, (CA_BASIC,)).hex() for n in range(2, 112)],
          certificate(target, issuer, 120).hex(), sep='\t')

