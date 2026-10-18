"""Read complete ECDSA suites from the private, pinned reference cache.

NIST ACVP/CAVP vectors are public domain. Wycheproof (Apache-2.0) stays
outside the repository, with its original licence and MANIFEST.tsv hashes.
No downloaded vector data is embedded here.
"""
import hashlib
import json
import io
import os
from pathlib import Path
import zipfile
if __package__:
    from .ecdsa_oracle import CURVES, emit, signature
else:
    from ecdsa_oracle import CURVES, emit, signature

ROOT = Path(os.environ.get('PAGURA_REFERENCES_DIR', '~/.cache/pagura-references')).expanduser() / 'signatures'
NAMES = {'P-256': 'P256', 'P-384': 'P384', 'P-521': 'P521',
         'secp256r1': 'P256', 'secp384r1': 'P384', 'secp521r1': 'P521'}


def reference_bytes(path):
    data = path.read_bytes()
    entries = [line.split('\t') for line in (ROOT / 'MANIFEST.tsv').read_text().splitlines()]
    manifest = {name.removeprefix('./'): (int(size), sha) for size, sha, name in entries}
    size, sha = manifest[path.relative_to(ROOT).as_posix()]
    assert len(data) == size and hashlib.sha256(data).hexdigest() == sha, path
    return data


def reference_json(path):
    return json.loads(reference_bytes(path))


def digest(algorithm, message):
    name = {'SHA-1': 'sha1', 'SHA-224': 'sha224', 'SHA-256': 'sha256',
            'SHA-384': 'sha384', 'SHA-512': 'sha512', 'SHA2-256': 'sha256',
            'SHA2-512': 'sha512', 'SHA2-512/256': 'sha512_256',
            'SHA3-256': 'sha3_256', 'SHA3-512': 'sha3_512',
            'SHA-512/224': 'sha512_224', 'SHA-512/256': 'sha512_256',
            'SHA-512224': 'sha512_224', 'SHA-512256': 'sha512_256',
            'SHAKE-128': 'shake_128', 'SHAKE-256': 'shake_256'}[algorithm]
    h = hashlib.new(name, bytes.fromhex(message))
    return h.digest(32 if name == 'shake_128' else 64) if name.startswith('shake') else h.digest()


def key_bytes(name, x, y):
    size = (CURVES[name][0].bit_length()+7)//8
    return b'\4' + int(x, 16).to_bytes(size, 'big') + int(y, 16).to_bytes(size, 'big')


def wycheproof():
    for path in sorted((ROOT / 'wycheproof/testvectors_v1').glob('ecdsa_secp*_test.json')):
        if 'p1363' in path.name:
            continue  # IEEE P1363 signature encoding, not DER
        data = reference_json(path)
        for group in data['testGroups']:
            if group['publicKey']['curve'] not in NAMES:
                continue  # curves outside FIPS 186-5's NIST prime set
            name = NAMES[group['publicKey']['curve']]
            key = bytes.fromhex(group['publicKey']['uncompressed'])
            for case in group['tests']:
                # This API is strict DER. Wycheproof's optional BER and
                # missing-sign-octet legacy cases are deliberately rejected.
                emit(path.stem, case['tcId'], name, digest(group['sha'], case['msg']),
                     key, bytes.fromhex(case['sig']), case['result'] == 'valid')


def acvp():
    path = ROOT / 'acvp/ECDSA-SigVer-FIPS186-5'
    if not path.exists():
        return
    prompt = reference_json(path / 'prompt.json')
    results = reference_json(path / 'expectedResults.json')
    expected = {t['tcId']: t['testPassed'] for g in results['testGroups'] for t in g['tests']}
    for group in prompt['testGroups']:
        if group['curve'] not in NAMES:
            continue
        name = NAMES[group['curve']]
        for t in group['tests']:
            emit('ACVP-ECDSA-SigVer-FIPS186-5', t['tcId'], name, digest(group['hashAlg'], t['message']),
                 key_bytes(name, t['qx'], t['qy']), signature(int(t['r'], 16), int(t['s'], 16)), expected[t['tcId']])


def cavp():
    path = ROOT / 'cavp/186-4ecdsatestvectors.zip'
    if not path.exists():
        return
    with zipfile.ZipFile(io.BytesIO(reference_bytes(path))) as archive:
        for filename in archive.namelist():
            if Path(filename).name not in ['SigVer.rsp', 'SigVer_TruncatedSHAs.rsp']:
                continue
            group, case, index = None, {}, 0
            for line in archive.read(filename).decode('ascii').splitlines():
                if line.startswith('['):
                    group = line.strip('[]').split(',')
                elif ' = ' in line:
                    k, v = line.split(' = ', 1)
                    case[k] = v
                    if k == 'Result':
                        index += 1
                        if group[0] in NAMES:
                            name = NAMES[group[0]]
                            emit('CAVP-' + Path(filename).stem, index, name, digest(group[1], case['Msg']),
                                 key_bytes(name, case['Qx'], case['Qy']),
                                 signature(int(case['R'], 16), int(case['S'], 16)), v.startswith('P'))
                        case = {}


if __name__ == '__main__':
    wycheproof()
    acvp()
    cavp()
