"""Export the cached signature reference suites for pagura-crypto's tests.

    python3 tools/signature_vectors.py

Reads the NIST CAVP/ACVP and Wycheproof files in
$PAGURA_REFERENCES_DIR/signatures (each checked against its MANIFEST.tsv
size and SHA-256) and writes the line formats that pagura-crypto's tests read
to $PAGURA_REFERENCES_DIR/signatures/export/crypto/ (pagura-pki's exporter uses
export/pki/ beside it):

    sha.txt          CAVP SHA-1/SHA-224 byte and Monte Carlo suites
    rsa.txt          Wycheproof, ACVP and CAVP RSA PKCS #1 v1.5 and PSS suites
    rsa-timing.txt   Wycheproof keys for the rsa_profile example
    ecdsa.txt        Wycheproof, ACVP and CAVP ECDSA suites

Nothing is written to the repository: Wycheproof (Apache-2.0) stays in the
private cache. `tools/check.py` runs this before the cargo tests; without a
cache it reports that and exits successfully, and the tests note the missing
export and skip those suites. The bundled and fixture vectors always run.
After `cargo test` alone, export first to include the cached suites.
"""
from __future__ import annotations

from fixturegen.ecdsa_vectors import ROOT
from fixturegen import ecdsa_vectors, rsa_oracle

SUITES = {
    "sha": rsa_oracle.hashes,
    "rsa": lambda: (*rsa_oracle.wycheproof(), *rsa_oracle.acvp(), *rsa_oracle.cavp()),
    "rsa-timing": rsa_oracle.timing,
    "ecdsa": ecdsa_vectors.suites,
}


def main() -> None:
    if not ROOT.exists():
        print(f"{ROOT}: no signature reference cache; the cached suites are skipped")
        return
    output = ROOT / "export/crypto"
    output.mkdir(parents=True, exist_ok=True)
    for name, suite in SUITES.items():
        lines = list(suite())
        (output / f"{name}.txt").write_text("".join(line + "\n" for line in lines))
        notes = [line for line in lines if line.startswith("#")]
        print(f"{name}: {len(lines) - len(notes)} vectors", *notes, sep="\n  ")


if __name__ == "__main__":
    main()
