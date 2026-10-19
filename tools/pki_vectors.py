"""Export the cached PKITS and Wycheproof cases for pagura-pki's tests.

Reads NIST's `pkits/PKITS_data.zip` (public domain) and the Wycheproof
`wycheproof/testvectors_v1/ecdsa*test.json` files (Apache-2.0) from the private,
pinned signature reference cache, checks them against its `MANIFEST.tsv`, and
writes `export/pki/manifest.json` plus one DER file per case inside that cache.
Nothing is copied into the repository; `tools/signature_vectors.py` exports
pagura-crypto's suites beside it, in `export/crypto/`. Without a cache it reports
the suites as absent and exits successfully; `crates/pagura-pki/tests/fixtures.rs` then
skips them.

It also writes `export/pki/pkits.txt` and `export/pki/revocation.txt`: the PKITS
certification paths, policy procedures and offline CRL cases read from the
cached PKITS PDF (through `pdftotext`) and ZIP by `fixturegen.pki.pkits` and
`fixturegen.pki.revocation`, with signatures checked by independent Python
integer arithmetic.

The upstream classifications are the expectations for the selected encoding
cases, not a cryptographic oracle: Wycheproof's valid signatures and its
InvalidEncoding, InvalidTypesInSignature, BerEncodedSignature and MissingZero
cases exercise envelope acceptance only.

The cache is `$PAGURA_SIGNATURE_REFERENCES_DIR`, else
`$PAGURA_REFERENCES_DIR/signatures`, else `~/.cache/pagura-references/signatures`.
"""
from __future__ import annotations

import hashlib
import io
import json
import os
import shutil
from collections.abc import Callable, Iterator
from contextlib import redirect_stdout
from pathlib import Path
from zipfile import ZipFile

from fixturegen.pki import pkits, revocation

ENCODING_FLAGS = {"InvalidEncoding", "InvalidTypesInSignature", "BerEncodedSignature", "MissingZero"}


def cache() -> Path:
    if direct := os.environ.get("PAGURA_SIGNATURE_REFERENCES_DIR"):
        return Path(direct)
    root = os.environ.get("PAGURA_REFERENCES_DIR", "~/.cache/pagura-references")
    return Path(root).expanduser() / "signatures"


def verified(root: Path, path: Path) -> bytes:
    """The file's bytes, after checking its size and SHA-256 in MANIFEST.tsv."""
    entries = [line.split("\t") for line in (root / "MANIFEST.tsv").read_text().splitlines() if line]
    manifest = {name.removeprefix("./"): (int(size), digest) for size, digest, name in entries}
    data = path.read_bytes()
    size, digest = manifest[path.relative_to(root).as_posix()]
    if len(data) != size or hashlib.sha256(data).hexdigest() != digest:
        raise SystemExit(f"{path}: does not match MANIFEST.tsv")
    return data


def cases(root: Path) -> Iterator[tuple[str, str, bool, bytes]]:
    """(kind, upstream id, accepted, DER) for every exported case."""
    archive = root / "pkits/PKITS_data.zip"
    if archive.exists():
        verified(root, archive)
        with ZipFile(archive) as source:
            for name in sorted(source.namelist()):
                if name.startswith("certs/") and name.endswith(".crt"):
                    yield "certificate", name, True, source.read(name)
                elif name.startswith("crls/") and name.endswith(".crl"):
                    yield "crl", name, True, source.read(name)
    for path in sorted((root / "wycheproof/testvectors_v1").glob("ecdsa*test.json")):
        suite = json.loads(verified(root, path))
        for group in suite["testGroups"]:
            for test in group["tests"]:
                if test["result"] == "valid" or ENCODING_FLAGS.intersection(test["flags"]):
                    yield "ecdsa", f"{path.name}:{test['tcId']}", test["result"] == "valid", bytes.fromhex(test["sig"])


def records(main: Callable[[Path], None], root: Path) -> str:
    """The tab-separated records a PKITS reader prints."""
    output = io.StringIO()
    with redirect_stdout(output):
        main(root)
    return output.getvalue()


def main() -> None:
    root = cache()
    if not (root / "MANIFEST.tsv").exists():
        print(f"pki vectors: no signature reference cache at {root}; cached suites skipped")
        return
    output = root / "export/pki"
    if output.exists():
        shutil.rmtree(output)
    output.mkdir(parents=True)
    entries = []
    for index, (kind, source, accepted, data) in enumerate(cases(root)):
        file = f"{index:05}.der"
        (output / file).write_bytes(data)
        entries.append({"id": f"pki-references/{index:05}", "structure": kind, "source": source,
                        "expect": "accept" if accepted else "reject", "path": file})
    manifest = {"schema": 2, "area": "pki-references", "cases": entries}
    (output / "manifest.json").write_text(json.dumps(manifest, indent=1) + "\n")
    print(f"pki vectors: exported {len(entries)} cached encoding cases to {output}")
    pdf, archive = root / "pkits/PKITS.pdf", root / "pkits/PKITS_data.zip"
    if pdf.exists() and archive.exists():
        verified(root, pdf)
        verified(root, archive)
        for name, reader in (("pkits", pkits.main), ("revocation", revocation.main)):
            (output / f"{name}.txt").write_text(records(reader, root))
        print(f"pki vectors: exported the PKITS paths and CRL cases to {output}")


if __name__ == "__main__":
    main()
