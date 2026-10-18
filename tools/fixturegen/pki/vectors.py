"""Read approved external vectors without copying source material into the tree.

PKITS is NIST public-domain material. Wycheproof is Apache-2.0; no vectors are
redistributed here. The upstream classifications are the expectations for the
selected encoding cases, not a cryptographic oracle for this parser.
"""
import json
from pathlib import Path
import sys
from zipfile import ZipFile

base = Path(sys.argv[1])
archive = base / "pkits/PKITS_data.zip"
if archive.exists():
    with ZipFile(archive) as source:
        for name in sorted(source.namelist()):
            if name.startswith("certs/") and name.endswith(".crt"):
                print("certificate", name, 1, source.read(name).hex(), sep="\t")
            elif name.startswith("crls/") and name.endswith(".crl"):
                print("crl", name, 1, source.read(name).hex(), sep="\t")

encoding_flags = {"InvalidEncoding", "InvalidTypesInSignature", "BerEncodedSignature", "MissingZero"}
for path in sorted((base / "wycheproof/testvectors_v1").glob("ecdsa*test.json")):
    source = json.loads(path.read_text())
    for group in source["testGroups"]:
        for case in group["tests"]:
            if case["result"] == "valid" or encoding_flags.intersection(case["flags"]):
                print("ecdsa", f"{path.name}:{case['tcId']}", int(case["result"] == "valid"), case["sig"], sep="\t")
