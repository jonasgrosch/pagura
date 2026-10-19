"""
PKITS cases and exact signature records, independently of the Rust engine.

Expectations and initial settings are read from the cached NIST PDF. Certificate
signatures are independently evaluated with Python integer arithmetic (RSA
PKCS#1 v1.5/SHA-1 and DSA/SHA-1); the Rust verifier accepts only exact recorded
input tuples. CRLs are deliberately not converted into expected revocation
assertions. No external bytes or document text are redistributed.
"""
from __future__ import annotations

import hashlib
import itertools
import pathlib
import re
import subprocess
import zipfile
from dataclasses import dataclass

from .fixtures import oid


class Der:
    def __init__(self, data):
        self.tag = data[0]
        n = data[1]
        start = 2
        if n >= 128:
            start += n & 127
            n = int.from_bytes(data[2:start], 'big')
        self.encoded = data[:start + n]
        self.content = data[start:start + n]

    def children(self):
        tail = self.content
        while tail:
            value = Der(tail)
            yield value
            tail = tail[len(value.encoded):]

    def number(self):
        return int.from_bytes(self.content, 'big')


@dataclass(frozen=True)
class Signed:
    """A signed PKITS object: its TBS value, algorithm and signature."""
    tbs: Der
    algorithm: Der
    signature: Der


@dataclass(frozen=True)
class Cert(Signed):
    """The fields of a PKITS certificate that path tests and signatures use."""
    issuer: Der
    subject: Der
    oid: Der
    params: Der | None
    bits: Der
    fields: list[Der]


def cert(data):
    tbs, algorithm, signature = list(Der(data).children())
    fields = list(tbs.children())
    k = 1 if fields[0].tag == 0xa0 else 0
    spki = fields[k + 5]
    alg, bits = list(spki.children())
    key_oid, *parameters = list(alg.children())
    return Cert(tbs=tbs, algorithm=algorithm, signature=signature,
                issuer=fields[k + 2], subject=fields[k + 4], oid=key_oid,
                params=parameters[0] if parameters and parameters[0].tag != 5 else None,
                bits=bits, fields=fields)


def verify(child, issuer, params):
    signature = child.signature.content
    if signature[0] != 0 or issuer.bits.content[0] != 0:
        return False
    algorithm = list(child.algorithm.children())[0].content
    sha1_rsa = Der(oid(1, 2, 840, 113549, 1, 1, 5)).content
    sha256_rsa = Der(oid(1, 2, 840, 113549, 1, 1, 11)).content
    sha1_dsa = Der(oid(1, 2, 840, 10040, 4, 3)).content
    digest = hashlib.sha1(child.tbs.encoded).digest()
    if algorithm in (sha1_rsa, sha256_rsa):
        if issuer.oid.content != Der(oid(1, 2, 840, 113549, 1, 1, 1)).content:
            return False
        n, e = [v.number() for v in Der(issuer.bits.content[1:]).children()]
        if int.from_bytes(signature[1:], 'big') >= n:
            return False
        size = (n.bit_length() + 7) // 8
        decoded = pow(int.from_bytes(signature[1:], 'big'), e, n).to_bytes(size, 'big')
        # RFC 8017 DigestInfo for SHA-1; strict padding, not suffix matching.
        if algorithm == sha256_rsa:
            digest_info = bytes.fromhex('3031300d060960864801650304020105000420') + hashlib.sha256(child.tbs.encoded).digest()
        else:
            digest_info = bytes.fromhex('3021300906052b0e03021a05000414') + digest
        expected = b'\0\1' + b'\xff' * (size - len(digest_info) - 3) + b'\0' + digest_info
        return len(signature[1:]) == size and decoded == expected
    if algorithm == sha1_dsa:
        if issuer.oid.content != Der(oid(1, 2, 840, 10040, 4, 1)).content:
            return False
        if params is None:
            return None
        p, q, g = [v.number() for v in params.children()]
        y = Der(issuer.bits.content[1:]).number()
        r, s = [v.number() for v in Der(signature[1:]).children()]
        if not (0 < r < q and 0 < s < q):
            return False
        w = pow(s, -1, q)
        u1 = int.from_bytes(digest, 'big') * w % q
        u2 = r * w % q
        return (pow(g, u1, p) * pow(y, u2, p) % p) % q == r
    raise AssertionError(f'Unrecorded PKITS algorithm: {algorithm.hex()}')


def normalized(value):
    return re.sub('[^a-z0-9]', '', value.lower())


def metadata(text, files):
    headings = list(re.finditer(r'(?m)^\s*(4\.(\d+)\.(\d+))\s+([^\n]+)', text))
    assert len(headings) == 224, len(headings)
    lookup = {normalized(pathlib.Path(n).stem): n for n in files}
    defaults = {'4.8.' + str(i): i not in (4, 5, 7, 8, 9, 12) for i in range(1, 21)}
    defaults['4.12.3'] = True
    defaults['4.4.1'] = False
    for i, heading in enumerate(headings):
        ident, section, case, title = heading.groups()
        body = text[heading.end():headings[i + 1].start() if i + 1 < len(headings) else len(text)]
        body = re.sub(r'(?m)^\s*\d+\s*$', '', body).replace('\f', '')
        description, paths = body.split('Certification Path:', 1)
        path = []
        for bullet in paths.split('•')[1:]:
            # Every bullet starts with a certificate, sometimes followed by CRLs.
            first = bullet.split(',', 1)[0]
            found = None
            for key, filename in lookup.items():
                if normalized(first).startswith(key) and (found is None or len(key) > found[0]):
                    found = (len(key), filename)
            assert found, (ident, first)
            path.append(found[1])
        assert path and path[0] == 'certs/TrustAnchorRootCertificate.crt', (ident, path)
        before = description.split('Procedure:', 1)[0]
        starts = list(re.finditer(r'(?m)^\s+(\d+)\.\s+default settings', before))
        variants = []
        for j, start in enumerate(starts):
            item = before[start.end():starts[j + 1].start() if j + 1 < len(starts) else len(before)]
            item = re.sub(r'-\s+', '-', item)
            item = re.sub(r'\s+', ' ', item)
            assert 'should validate' in item or 'should not validate' in item, (ident, item)
            explicit = 'initial-explicit-policy set' in item
            mapping = 'initial-policy-mapping-inhibit set' in item
            any_policy = 'initial-inhibit-any-policy set' in item
            requested = re.search(r'initial-policy-set\s*=\s*\{([^}]+)\}', item)
            policies = re.findall(r'NIST-test-policy-(\d+)', requested[1]) if requested else ['any']
            variants.append((start[1], explicit, mapping, any_policy, policies, 'should not validate' not in item))
        default = defaults.get(ident, title.startswith('Valid'))
        assert ident in defaults or title.startswith(('Valid', 'Invalid'))
        yield ident, section, body, path, [('default', False, False, False, ['any'], default)] + variants


def expected_policies(section, body, initial):
    """NIST's stated authority set, intersected with its procedure's input."""
    if int(section) not in range(8, 13):
        return '1'  # Section 3 specifies policy 1 for non-policy test paths.
    result = body.split('Expected Result:', 1)[1].split('Certification Path:', 1)[0]
    result = re.sub(r'-\s+', '-', result)
    result = re.sub(r'\s+', ' ', result)
    match = re.search(r'authorities-constrained-policy-set will be (\{[^}]+\}|any-policy)', result)
    if match:
        authority = set(re.findall(r'NIST-test-policy-(\d+)', match[1])) if match[1] != 'any-policy' else {'any'}
    elif re.search(r'(?:will be|is) empty', result):
        authority = set()
    else:
        return '?'  # Some requireExplicitPolicy descriptions state status only.
    requested = set(initial)
    output = authority if 'any' in requested else requested if 'any' in authority else authority & requested
    return ','.join(sorted(output)) or '-'


def main(root):
    pdf = root / 'pkits/PKITS.pdf'
    archive = root / 'pkits/PKITS_data.zip'
    if not pdf.exists() or not archive.exists():
        print('ABSENT\tPKITS')
        return
    text = subprocess.check_output(['pdftotext', '-layout', str(pdf), '-'], text=True)
    with zipfile.ZipFile(archive) as z:
        files = [n for n in z.namelist() if n.startswith('certs/') and n.endswith('.crt')]
        certificates = {name: cert(z.read(name)) for name in files}
        for name in files:
            print('CERT', name, z.read(name).hex(), sep='\t')
        records = set()
        for ident, section, body, path, variants in metadata(text, files):
            pool = [certificates[name] for name in path]
            for child, issuer in itertools.product(pool[1:], pool):
                effective = [issuer.params]
                if issuer.params is None:
                    effective += [c.params for c in pool if c.oid.content == issuer.oid.content and c.params]
                for params in effective:
                    record = (child.algorithm.encoded.hex(), child.tbs.encoded.hex(),
                              child.signature.content.hex(), issuer.oid.content.hex(),
                              params.encoded.hex() if params else '-', issuer.bits.content.hex())
                    if record not in records:
                        records.add(record)
                        verified = verify(child, issuer, params)
                        print('SIGNATURE', *record, 'unknown' if verified is None else int(verified), sep='\t')
            # These sections need CRL processing; 4.5.8 is a basic CA/KU failure.
            revocation = section in ('4', '14', '15') or (section == '5' and ident != '4.5.8') or ident in ('4.7.4', '4.7.5')
            for variant, explicit, mapping, any_policy, policies, expected in variants:
                print('CASE', ident + '/' + variant, section, int(expected), int(revocation),
                      int(explicit), int(mapping), int(any_policy), ','.join(policies),
                      expected_policies(section, body, policies) if expected else '-', *path, sep='\t')

