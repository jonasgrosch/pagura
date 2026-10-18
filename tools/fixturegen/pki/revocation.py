#!/usr/bin/env python3
"""NIST PKITS CRL cases, PDF expectations and exact independent signatures.

No PKITS data or reference prose is copied into the repository. This harness
selects the objects named by each PDF case, without interpreting CRL statuses.
RSA/DSA arithmetic is independent of the product's callback-based verification.
"""
import itertools
import pathlib
import re
import subprocess
import sys
import zipfile

from pkits import Der, cert, metadata, normalized, verify


def main(root):
    archive = root / 'pkits/PKITS_data.zip'
    pdf = root / 'pkits/PKITS.pdf'
    if not archive.exists() or not pdf.exists():
        print('ABSENT\tPKITS')
        return
    text = subprocess.check_output(['pdftotext', '-layout', str(pdf), '-'], text=True)
    with zipfile.ZipFile(archive) as z:
        files = [n for n in z.namelist() if n.startswith('certs/') and n.endswith('.crt')]
        crls = [n for n in z.namelist() if n.startswith('crls/') and n.endswith('.crl')]
        certificates = {name: cert(z.read(name)) for name in files}
        for name in files:
            print('CERT', name, z.read(name).hex(), sep='\t')
        for name in crls:
            print('CRL', name, z.read(name).hex(), sep='\t')
        records = set()
        for ident, section, body, path, variants in metadata(text, files):
            if section not in ('4', '5', '7', '14', '15'):
                continue
            chosen = []
            additional = []
            for bullet in body.split('Certification Path:', 1)[1].split('•')[1:]:
                for piece in re.split(r'\n\s*\n', bullet, maxsplit=1)[0].split(',')[1:]:
                    matches = [(len(normalized(pathlib.Path(n).stem)), n) for n in [*files, *crls]
                               if normalized(piece).startswith(normalized(pathlib.Path(n).stem))]
                    assert matches, (ident, piece)
                    selected = max(matches)[1]
                    (additional if selected.startswith("certs/") else chosen).append(selected)
            path = path[:-1] + additional + path[-1:]
            pool = [certificates[name] for name in path]
            signed = pool[1:]
            for name in chosen:
                tbs, algorithm, signature = list(Der(z.read(name)).children())
                signed.append(dict(tbs=tbs, algorithm=algorithm, signature=signature))
            for child, issuer in itertools.product(signed, pool):
                effective = [issuer['params']]
                if issuer['params'] is None:
                    effective += [c['params'] for c in pool if c['oid'].content == issuer['oid'].content and c['params']]
                for params in effective:
                    record = (child['algorithm'].encoded.hex(), child['tbs'].encoded.hex(),
                              child['signature'].content.hex(), issuer['oid'].content.hex(),
                              params.encoded.hex() if params else '-', issuer['bits'].content.hex())
                    if record not in records:
                        records.add(record)
                        verified = verify(child, issuer, params)
                        print('SIGNATURE', *record, 'unknown' if verified is None else int(verified), sep='\t')
            result = body.split('Expected Result:', 1)[1].split('Certification Path:', 1)[0]
            result = re.sub(r'\s+', ' ', result)
            expected = variants[0][-1]
            if expected:
                category = 'Good'
            elif 'revoked' in result or 'on hold' in result:
                category = 'Revoked'
            elif 'status' in result or 'revocation' in result:
                category = 'Unknown'
            elif 'has been revoked' in re.sub(r'\s+', ' ', body.split('Procedure:', 1)[0]):
                category = 'Revoked'
            else:
                category = 'BasicInvalid'
            print('CASE', ident, section, int(expected), category, ','.join(path), ','.join(chosen), sep='\t')


if __name__ == '__main__':
    main(pathlib.Path(sys.argv[1]))
