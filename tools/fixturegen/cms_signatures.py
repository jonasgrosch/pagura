"""Independently signed PDF/CMS/TSTInfo fixtures (standard library only).

RFC 5652 §§5.3–5.6, RFC 5035 §§3–4, RFC 3161 §2.4.2, RFC 4055 §3,
ISO 32000-2 §12.8.3. Public deterministic test keys and nonces are unsuitable
for real signing. No external signer or Pagura implementation is executed.
"""
import hashlib
from functools import lru_cache
from . import rsa_oracle, ecdsa_oracle as ec
from .signatures import Pdf, structure, signature, patch
from .model import Vector

AREA = 'cms-signatures'
SHA = {'sha1': '2b0e03021a', 'sha224': '608648016503040204',
       'sha256': '608648016503040201', 'sha384': '608648016503040202',
       'sha512': '608648016503040203'}
RSA = '2a864886f70d010101'
PSS = '2a864886f70d01010a'
DATA = '2a864886f70d010701'
SIGNED = '2a864886f70d010702'
TST = '2a864886f70d0109100104'
CONTENT = '2a864886f70d010903'
DIGEST = '2a864886f70d010904'
ESS = '2a864886f70d010910022f'
NULL = b'\x05\0'
KEY = rsa_oracle.test_key()


def tlv(tag, data):
    size = len(data)
    length = size.to_bytes(max(1, (size.bit_length()+7)//8), 'big')
    return bytes([tag]) + (bytes([size]) if size < 128 else bytes([0x80 | len(length)]) + length) + data


def seq(*values):
    return tlv(0x30, b''.join(values))


def integer(number):
    data = number.to_bytes(max(1, (number.bit_length()+7)//8), 'big')
    return tlv(2, (b'\0' if data[0] & 128 else b'') + data)


def oid(value):
    return tlv(6, bytes.fromhex(value))


def alg(value, parameters=b''):
    return seq(oid(value), parameters)


def hash_alg(name):
    return alg(SHA[name])


def digest(name, data):
    return hashlib.new(name, data).digest()


def octets(data):
    return tlv(4, data)


def ordered(tag, values):
    return tlv(tag, b''.join(sorted(values)))


def pss_parameters(name, mgf=None, salt=32):
    mgf = name if mgf is None else mgf
    return seq(*( [tlv(0xa0, hash_alg(name))] if name != 'sha1' else []),
               *( [tlv(0xa1, alg('2a864886f70d010108', hash_alg(mgf)))] if mgf != 'sha1' else []),
               *( [tlv(0xa2, integer(salt))] if salt != 20 else []))


def sign_rsa(data, name='sha256', pss=False, mgf=None, salt=32):
    hashed = digest(name, data)
    n, _, _ = KEY
    width = (n.bit_length()+7)//8
    if pss:
        salt_bytes = bytes(range(salt))
        h = digest(name, b'\0'*8 + hashed + salt_bytes)
        bits = n.bit_length()-1
        size = (bits+7)//8
        db = b'\0'*(size-len(h)-salt-2) + b'\1' + salt_bytes
        mask_name = mgf or name
        mask = b''.join(digest(mask_name, h + i.to_bytes(4, 'big'))
                        for i in range((len(db)+len(digest(mask_name, b''))-1)//len(digest(mask_name, b''))))
        masked = bytearray(a ^ b for a, b in zip(db, mask))
        masked[0] &= 255 >> (size*8-bits)
        encoded = bytes(masked) + h + b'\xbc'
    else:
        tail = seq(alg(SHA[name], NULL), octets(hashed))
        encoded = b'\0\1' + b'\xff'*(width-len(tail)-3) + b'\0' + tail
    return rsa_oracle.sign_representative(encoded, KEY)


@lru_cache(None)
def ec_key(curve):
    p, n, _, gx, gy = ec.CURVES[curve]
    private = 37
    return private, ec.multiply(private, (gx, gy), p)


def sign_ec(data, name, curve):
    private, point = ec_key(curve)
    p, n, _, gx, gy = ec.CURVES[curve]
    hashed = digest(name, data)
    e = int.from_bytes(hashed, 'big') >> max(0, len(hashed)*8-n.bit_length())
    nonce = 101
    r = ec.multiply(nonce, (gx, gy), p)[0] % n
    s = (e + r*private) * pow(nonce, -1, n) % n
    assert ec.verify(curve, hashed, point, r, s)
    return ec.signature(r, s)


def signature_alg(name, pss=False, curve=None, mgf=None, salt=32):
    if curve:
        return alg('2a8648ce3d0401' if name == 'sha1' else '2a8648ce3d0403' + {'sha224':'01', 'sha256':'02', 'sha384':'03', 'sha512':'04'}[name])
    if pss:
        return alg(PSS, pss_parameters(name, mgf, salt))
    return alg(RSA, NULL)


def sign(data, name='sha256', pss=False, curve=None, mgf=None, salt=32):
    return sign_ec(data, name, curve) if curve else sign_rsa(data, name, pss, mgf, salt)


NAME = seq(ordered(0x31, [seq(oid('550403'), tlv(12, b'Fixture signer'))]))


@lru_cache(None)
def certificate(curve=None, serial=1, pss_key=False, wrong_key=False, bad_ski=False, missing_null=False):
    sig_alg = signature_alg('sha256', curve=curve)
    if curve:
        key = ec.point_bytes(curve, ec_key(curve)[1])
        key_alg = alg('2a8648ce3d0201', oid({'P256':'2a8648ce3d030107', 'P384':'2b81040022', 'P521':'2b81040023'}[curve]))
    else:
        key = seq(integer(KEY[0]), integer(KEY[1]+2 if wrong_key else KEY[1]))
        key_alg = alg(PSS, pss_parameters('sha256')) if pss_key else alg(RSA, b'' if missing_null else NULL)
    ski = digest('sha1', key)
    spki = seq(key_alg, tlv(3, b'\0' + key))
    tbs = seq(tlv(0xa0, integer(2)), integer(serial), sig_alg, NAME,
              seq(tlv(23, b'260101000000Z'), tlv(23, b'300101000000Z')), NAME, spki,
              tlv(0xa3, seq(seq(oid('551d0e'), octets(seq(integer(0)) if bad_ski else octets(ski))))))
    return seq(tbs, sig_alg, tlv(3, b'\0' + sign(tbs, curve=curve))), ski


def attribute(ident, value):
    return seq(oid(ident), ordered(0x31, [value]))


def token(signed_bytes, *, subfilter=b'adbe.pkcs7.detached', name='sha256', curve=None,
          pss=False, mgf=None, salt=32, mutation='', ski=False, ber=False, pss_key=False, imprint_name=None):
    cert, key_id = certificate(curve, pss_key=pss_key)
    timestamp = subfilter == b'ETSI.RFC3161'
    content_type = TST if timestamp else DATA
    encapsulated = b''
    if timestamp:
        imprint_hash = imprint_name or name
        imprint = digest(imprint_hash, signed_bytes)
        if mutation == 'imprint':
            imprint = bytes([imprint[0] ^ 1]) + imprint[1:]
        encapsulated = seq(integer(1), oid('2a0304'), seq(hash_alg(imprint_hash), octets(imprint)),
                           integer(7), tlv(24, b'20261018090000Z'))
    hashed = digest(name, encapsulated if timestamp else signed_bytes)
    if mutation == 'message-digest':
        hashed = bytes([hashed[0] ^ 1]) + hashed[1:]
    attrs = [attribute(CONTENT, oid(DATA if mutation == 'content-type' and timestamp else TST if mutation == 'content-type' else content_type)),
             attribute(DIGEST, octets(hashed))]
    if (subfilter == b'ETSI.CAdES.detached' or timestamp or mutation == 'ess-optional') and mutation != 'ess-missing':
        cert_hash = digest('sha1' if mutation == 'ess-v1' else 'sha256', cert)
        if mutation == 'ess-hash':
            cert_hash = bytes([cert_hash[0] ^ 1]) + cert_hash[1:]
        issuer_serial = seq(seq(tlv(0xa4, NAME)), integer(2 if mutation == 'ess-serial' else 1))
        attrs.append(attribute('2a864886f70d010910020c' if mutation == 'ess-v1' else ESS, seq(seq(seq(octets(cert_hash), issuer_serial)))))
    if mutation == 'unsupported-asn1':
        attrs.append(attribute('2a030405', tlv(9, b'')))
    if mutation == 'duplicate-digest':
        attrs.append(attribute(DIGEST, octets(hashed)))
    if mutation.startswith('protection-'):
        protected_digest = hash_alg('sha384' if mutation == 'protection-digest-mismatch' else name)
        protected_signature = signature_alg(name, pss, curve, mgf, 0 if mutation == 'protection-pss-parameters' else salt)
        # Remove only the SEQUENCE wrapper to apply the IMPLICIT context tag.
        assert protected_signature[0] == 0x30 and protected_signature[1] < 128
        if mutation == 'protection-pss-unknown-mgf':
            protected_signature = alg(PSS, seq(tlv(0xa0, hash_alg(name)), tlv(0xa1, alg('2a030405')), tlv(0xa2, integer(salt))))
        if mutation == 'protection-signature-mismatch':
            protected_signature = alg('2a864886f70d01010c', NULL)
        if mutation == 'protection-null-digest':
            protected_digest = alg(SHA[name], NULL)
        protected = seq(protected_digest, tlv(0xa1, protected_signature[2:]))
        if mutation == 'protection-mac':
            protected = seq(protected_digest, tlv(0xa2, protected_signature[2:]))
        attrs.append(attribute('2a864886f70d010934', protected))
        if mutation == 'protection-duplicate':
            attrs.append(attribute('2a864886f70d010934', protected))
    attrs.sort()
    signed_attrs = tlv(0x31, b''.join(attrs))
    sig = sign(signed_bytes if mutation == 'no-attributes' else signed_attrs, name, pss, curve, mgf, salt)
    if mutation == 'signature':
        # Still a well-formed representative; only its equation is wrong.
        sig = sign(signed_attrs + b'changed', name, pss, curve, mgf, salt)
    if mutation == 'unsorted-signed':
        attrs.reverse()
    if mutation == 'wrong-ski':
        key_id = bytes([key_id[0] ^ 1]) + key_id[1:]
    identifier = tlv(0x80, key_id) if ski else seq(NAME, integer(9 if mutation == 'sid' else 1))
    digest_algorithm = hash_alg(name) if mutation != 'unsupported-hash' else alg('2a030405')
    sig_alg = signature_alg(name, pss, curve, mgf, salt)
    if mutation == 'unsupported-signature':
        sig_alg = alg('2a030405')
    if mutation == 'pss-digest':
        sig_alg = signature_alg('sha384', pss=True)
    info = seq(integer(3 if ski else 1), identifier, digest_algorithm,
               b'' if mutation == 'no-attributes' else tlv(0xa0, b''.join(attrs)), sig_alg, octets(sig))
    if mutation == 'algorithm-mismatch':
        sig_alg = signature_alg('sha384', curve=curve) if curve else alg('2a864886f70d01010c', NULL)
        info = seq(integer(3 if ski else 1), identifier, digest_algorithm, tlv(0xa0, b''.join(attrs)), sig_alg, octets(sig))
    if mutation == 'wrong-certificate':
        cert = certificate(curve, serial=2)[0]
    if mutation == 'missing-key-null':
        cert = certificate(missing_null=True)[0]
    if mutation == 'wrong-key':
        value = int.from_bytes(sig, 'big')
        assert pow(value, KEY[1]+2, KEY[0]) != pow(value, KEY[1], KEY[0])
        cert = certificate(wrong_key=True)[0]
    certs = [cert]
    if mutation == 'bad-ski-certificate':
        certs.append(certificate(serial=2, bad_ski=True)[0])
    if mutation == 'many-certificates':
        certs.extend([seq(integer(1))]*256)
    if mutation in ('bad-certificate', 'only-bad-certificate'):
        certs.append(seq(integer(1)))
    if mutation == 'only-bad-certificate':
        certs.pop(0)
    infos = [info]
    if mutation in ('two-signers', '64-signers'):
        infos *= 64 if mutation == '64-signers' else 2
    if mutation == 'two-signers-one-invalid':
        # Extra signed data makes the invalid SignerInfo longer and therefore
        # sorts it after the valid one in DER SET order. Its signature is a
        # well-formed RSA signature over a different input, not malformed bytes.
        invalid_attrs = sorted(attrs + [attribute('2a030405', octets(b'additional signed data'))])
        invalid_sig = sign(tlv(0x31, b''.join(invalid_attrs)) + b'changed', name, pss, curve, mgf, salt)
        invalid = seq(integer(3 if ski else 1), identifier, digest_algorithm,
                      tlv(0xa0, b''.join(invalid_attrs)), sig_alg, octets(invalid_sig))
        assert info < invalid
        infos.append(invalid)
    if mutation == 'many-signers':
        infos = [seq(integer(1), seq(NAME, integer(1)), digest_algorithm, sig_alg, octets(b''))]*65
    if mutation == 'empty-signers':
        infos = []
    digests = [digest_algorithm]
    if mutation == 'extra-unknown-digest':
        digests.append(alg('2a030405'))
    if ber:
        # These are valid BER sets, but not sorted DER sets.
        digests = sorted([hash_alg('sha512'), digest_algorithm], reverse=True)
        certs.append(certificate(curve, serial=2)[0])
        certs.sort(reverse=True)
    else:
        digests.sort()
        certs.sort()
    data = seq(integer(3 if timestamp or ski else 1), tlv(0x31, b''.join(digests)),
               seq(oid(content_type), tlv(0xa0, octets(encapsulated)) if timestamp else b''),
               tlv(0xa0, b''.join(certs)), ordered(0x31, infos))
    return seq(oid(SIGNED), tlv(0xa0, data))


def case(ident, expected='Verified', algorithms='Supported', *, mutation='', certification=False, capacity=4096, **kwargs):
    subfilter = kwargs.get('subfilter', b'adbe.pkcs7.detached')
    body, span = signature(subfilter, contents=bytes(capacity))
    pdf = Pdf()
    pdf.data += b'% signed marker\n'
    objects = structure(catalog_extra=b'/Perms << /DocMDP 5 0 R >>' if certification else b'') | {5: body}
    positions = pdf.append(objects)
    ranges = patch(pdf, positions[5], body, span)
    _, start, end, _ = ranges
    signed_bytes = bytes(pdf.data[:start] + pdf.data[end:])
    cms = token(signed_bytes, mutation=mutation, **kwargs)
    assert len(cms) < capacity
    contents = cms + bytes(capacity-len(cms))
    if mutation == 'padding':
        contents = contents[:-1] + b'\1'
    pdf.data[start+1:end-1] = contents.hex().encode()
    if mutation == 'byte-flip':
        at = pdf.data.index(b'signed marker')
        pdf.data[at] ^= 1
    if expected != 'Verified' and algorithms == 'Supported':
        algorithms = 'NotCheckedYet'
    rows = [expected + '|' + algorithms]
    return Vector(ident, {'.pdf': bytes(pdf.data), '.expect': ('\n'.join(rows)+'\n').encode()},
                  ('cms-integrity', *(('cms-algorithm-protection',) if mutation.startswith('protection-') else ()), 'cms-timestamp' if subfilter == b'ETSI.RFC3161' else 'cms-detached'),
                  'Independent signed PDF: ' + ident)


def cases():
    yield case('rsa-sha256')
    yield case('unsupported-asn1', 'UnsupportedEncoding', mutation='unsupported-asn1')
    yield case('unsupported-subfilter', 'UnsupportedEncoding', algorithms='UnsupportedEncoding', subfilter=b'Example.encoding')
    yield case('missing-key-null', 'Malformed', mutation='missing-key-null')
    yield case('wrong-key', 'SignatureInvalid', mutation='wrong-key')
    yield case('bad-ski-certificate', mutation='bad-ski-certificate', ski=True)
    yield case('many-certificates', 'ResourceLimit', mutation='many-certificates')
    yield case('many-signers', 'ResourceLimit', mutation='many-signers', capacity=8192)
    yield case('sha1-unsupported-signature', 'UnsupportedAlgorithm', algorithms='WeakSha1UnsupportedAlgorithm', name='sha1', mutation='unsupported-signature')
    yield case('rsa-no-attributes', mutation='no-attributes')
    yield case('rsa-two-signers', mutation='two-signers')
    yield case('rsa-two-signers-one-invalid', 'SignatureInvalid', mutation='two-signers-one-invalid')
    yield case('extra-unknown-digest', mutation='extra-unknown-digest')
    yield case('rsa-empty-signers', 'Malformed', mutation='empty-signers')
    yield case('ess-optional', mutation='ess-optional')
    yield case('wrong-ski', 'SignerCertificateMissing', mutation='wrong-ski', ski=True)
    yield case('algorithm-mismatch', 'Malformed', mutation='algorithm-mismatch')
    yield case('only-malformed-certificate', 'SignerCertificateMissing', mutation='only-bad-certificate')
    for name in SHA:
        yield case('rsa-'+name, algorithms='WeakSha1' if name == 'sha1' else 'Supported', name=name) if name != 'sha256' else case('rsa-sha256-ski', ski=True)
    for curve, name in [('P256','sha256'), ('P384','sha384'), ('P521','sha512')]:
        yield case('ecdsa-'+curve.lower(), curve=curve, name=name)
    yield case('rsa-pss', pss=True)
    yield case('rsa-pss-default-sha1', pss=True, name='sha1', salt=20, algorithms='WeakSha1')
    yield case('ecdsa-sha1', curve='P256', name='sha1', algorithms='WeakSha1')
    yield case('rsa-pss-salt-zero', pss=True, salt=0)
    yield case('rsa-pss-mgf-sha1', pss=True, mgf='sha1', algorithms='WeakSha1')
    yield case('rsa-pss-restricted-key', pss=True, pss_key=True)
    yield case('rsa-pss-restricted-short-salt', 'Malformed', pss=True, pss_key=True, salt=0)
    yield case('rsa-pss-key-rejects-v15', expected='Malformed', pss_key=True)
    yield case('cades', subfilter=b'ETSI.CAdES.detached')
    yield case('cades-ess-v1-sha1', subfilter=b'ETSI.CAdES.detached', name='sha1', mutation='ess-v1', algorithms='WeakSha1')
    yield case('cades-ess-v1-sha256', subfilter=b'ETSI.CAdES.detached', mutation='ess-v1', algorithms='WeakSha1')
    yield case('protection-match', mutation='protection-match')
    yield case('protection-null-digest', mutation='protection-null-digest')
    yield case('protection-ecdsa', curve='P256', mutation='protection-match')
    yield case('protection-pss', pss=True, mutation='protection-match')
    yield case('protection-digest-mismatch', 'SignatureInvalid', mutation='protection-digest-mismatch')
    yield case('protection-signature-mismatch', 'SignatureInvalid', mutation='protection-signature-mismatch')
    yield case('protection-pss-unknown-mgf', 'SignatureInvalid', pss=True, mutation='protection-pss-unknown-mgf')
    yield case('protection-pss-parameters', 'SignatureInvalid', pss=True, mutation='protection-pss-parameters')
    yield case('protection-duplicate', 'Malformed', mutation='protection-duplicate')
    yield case('protection-mac', 'Malformed', mutation='protection-mac')
    yield case('timestamp', subfilter=b'ETSI.RFC3161')
    yield case('timestamp-different-digests', subfilter=b'ETSI.RFC3161', name='sha384', imprint_name='sha256')
    yield case('timestamp-sha1-imprint', subfilter=b'ETSI.RFC3161', imprint_name='sha1', algorithms='WeakSha1')
    yield case('timestamp-no-ess', 'Malformed', subfilter=b'ETSI.RFC3161', mutation='ess-missing')
    yield case('timestamp-ess-v1', algorithms='WeakSha1', subfilter=b'ETSI.RFC3161', mutation='ess-v1')
    yield case('timestamp-two-signers', 'Malformed', subfilter=b'ETSI.RFC3161', mutation='two-signers')
    yield case('cades-two-signers', 'Malformed', subfilter=b'ETSI.CAdES.detached', mutation='two-signers')
    yield case('timestamp-pss', subfilter=b'ETSI.RFC3161', pss=True)
    yield case('timestamp-ecdsa', subfilter=b'ETSI.RFC3161', curve='P256', ski=True)
    yield case('timestamp-wrong-imprint', expected='DigestMismatch', subfilter=b'ETSI.RFC3161', mutation='imprint')
    yield case('timestamp-wrong-content-type', expected='Malformed', subfilter=b'ETSI.RFC3161', mutation='content-type')
    yield case('certification', certification=True)
    yield case('certification-64-signers', certification=True, mutation='64-signers', capacity=32768)
    yield case('certification-ber', certification=True, ber=True)
    yield case('ber-unsorted', ber=True)
    yield case('malformed-extra-certificate', mutation='bad-certificate')
    for mutation, expected in [('byte-flip','DigestMismatch'), ('message-digest','DigestMismatch'),
                               ('signature','SignatureInvalid'), ('wrong-certificate','SignerCertificateMissing'),
                               ('sid','SignerCertificateMissing'), ('content-type','Malformed'),
                               ('duplicate-digest','Malformed'), ('unsorted-signed','Malformed'), ('padding','Malformed'),
                               ('unsupported-hash','UnsupportedAlgorithm'), ('unsupported-signature','UnsupportedAlgorithm')]:
        yield case(mutation, expected, algorithms='UnsupportedAlgorithm' if mutation.startswith('unsupported') else 'Supported', mutation=mutation)
    yield case('pss-digest-mismatch', 'Malformed', mutation='pss-digest', pss=True)
    for mutation, expected in [('ess-hash','SignatureInvalid'), ('ess-serial','SignatureInvalid'), ('ess-missing','Malformed')]:
        yield case(mutation, expected, subfilter=b'ETSI.CAdES.detached', mutation=mutation)

    pdf = Pdf()
    pdf.data += b'% signed marker\n'
    capacity = 4096
    first, span = signature(contents=bytes(capacity))
    positions = pdf.append(structure() | {5: first})
    _, start, end, _ = patch(pdf, positions[5], first, span)
    cms = token(bytes(pdf.data[:start]+pdf.data[end:]))
    pdf.data[start+1:end-1] = (cms+bytes(capacity-len(cms))).hex().encode()
    pdf.data[pdf.data.index(b'signed marker')] ^= 1
    second, span = signature(contents=bytes(capacity))
    positions = pdf.append({4:b'<< /Fields [6 0 R 10 0 R] /SigFlags 3 >>', 9:second,
                            10:b'<< /FT /Sig /T (Second) /V 9 0 R >>'})
    _, start, end, _ = patch(pdf, positions[9], second, span, revision=1)
    cms = token(bytes(pdf.data[:start]+pdf.data[end:]), curve='P256')
    pdf.data[start+1:end-1] = (cms+bytes(capacity-len(cms))).hex().encode()
    yield Vector('two-independent-revisions', {'.pdf':bytes(pdf.data),
                 '.expect':b'DigestMismatch|NotCheckedYet\nVerified|Supported\n'},
                 ('cms-integrity', 'cms-detached'), 'Two independent signatures across revisions with a changed first signed range')

    # Two direct dictionaries can share their enclosing object's identity.
    # Only the field is signed; the certification Contents stays malformed.
    field_body, field_span = signature(contents=bytes(4096))
    certification_body, certification_span = signature(contents=bytes(32))
    catalog = (b'<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields ['
               b'<< /FT /Sig /T (Approval) /V ' + field_body + b' >>] >>'
               b' /Perms << /DocMDP ' + certification_body + b' >> >>')
    objects = structure()
    objects[1] = catalog
    del objects[4], objects[6]
    pdf = Pdf()
    positions = pdf.append(objects)
    field_at = positions[1] + catalog.index(field_body)
    certification_at = positions[1] + catalog.index(certification_body)
    _, start, end, _ = patch(pdf, field_at, field_body, field_span)
    patch(pdf, certification_at, certification_body, certification_span)
    cms = token(bytes(pdf.data[:start]+pdf.data[end:]))
    pdf.data[start+1:end-1] = (cms+bytes(4096-len(cms))).hex().encode()
    yield Vector('certification-shared-container', {'.pdf':bytes(pdf.data),
                 '.expect':b'Verified|Supported\n'}, ('cms-integrity', 'cms-detached'),
                 'Distinct signature dictionaries in one object retain separate integrity results')
