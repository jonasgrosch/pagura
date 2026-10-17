"""Bundled third-party data: pinned sources, extraction and verification.

    python3 tools/bundled_data.py fetch    # download the pinned sources
    python3 tools/bundled_data.py build    # extract the selected files into data/
    python3 tools/bundled_data.py verify   # every file in data/ matches data/lock.json

Font metrics, substitute outlines and predefined CMaps serve non-embedded
fonts (ADR 0009); Unicode 3.2.0 normalization data serves SASLprep for
revision 6 passwords (RFC 3454 section 4, ISO 32000-2 7.6.4.3.3).

Sources are pinned by SHA-256. Archives are cached in $PAGURA_DATA_SOURCES
(default ~/.cache/pagura-data-sources) and read member by member; nothing in
them is executed or extracted wholesale. `build` is deterministic: it writes
only the members named below, plus a column projection of Adobe's
cid2code tables, and records every output file in data/lock.json.
"""
from __future__ import annotations

import argparse
import hashlib
import io
import json
import os
import sys
import tarfile
import zipfile
from dataclasses import dataclass
from pathlib import Path
from urllib.request import Request, urlopen

ROOT = Path(__file__).resolve().parents[1]
DATA = ROOT / "data"
LOCK = DATA / "lock.json"
MAX_ARCHIVE = 64 << 20


@dataclass(frozen=True)
class Source:
    url: str
    sha256: str
    file: str
    licence: str


SOURCES = {
    "core14-afm": Source(
        "https://download.macromedia.com/pub/developer/opentype/tech-notes/Core14_AFMs.zip",
        "8c892c3c49553cfd2d2a27c4495b4bb12e2875115be7fd127ed3876df19d8654",
        "Core14_AFMs.zip", "Adobe Core 14 AFM notice (data/afm/MustRead.html)"),
    "croscorefonts": Source(
        "https://commondatastorage.googleapis.com/chromeos-localmirror/distfiles/croscorefonts-1.31.0.tar.bz2",
        "672c3487883ec1ef83d9254240d4327b014212abc823d06d15816095867315e1",
        "croscorefonts-1.31.0.tar.bz2", "Apache-2.0 (data/fonts/LICENSE-APACHE, data/fonts/NOTICE)"),
    "cmap-resources": Source(
        "https://github.com/adobe-type-tools/cmap-resources/archive/refs/tags/20231115.tar.gz",
        "a88c3beee8d2f139b47f9d7e932ec198be24991dd46fcaef2961b4b5ea855ea2",
        "cmap-resources-20231115.tar.gz", "BSD-3-Clause (data/cmaps/LICENSE.md)"),
    # Stringprep fixes Unicode 3.2 (RFC 3454 sections 3 and 4).
    "unicode-data": Source(
        "https://www.unicode.org/Public/3.2-Update/UnicodeData-3.2.0.txt",
        "5e444028b6e76d96f9dc509609c5e3222bf609056f35e5fcde7e6fb8a58cd446",
        "UnicodeData-3.2.0.txt", "Unicode License V3 (data/unicode/LICENSE)"),
    "composition-exclusions": Source(
        "https://www.unicode.org/Public/3.2-Update/CompositionExclusions-3.2.0.txt",
        "1d3a450d0f39902710df4972ac4a60ec31fbcb54ffd4d53cd812fc1200c732cb",
        "CompositionExclusions-3.2.0.txt", "Unicode License V3 (data/unicode/LICENSE)"),
    "derived-normalization": Source(
        "https://www.unicode.org/Public/3.2-Update/DerivedNormalizationProps-3.2.0.txt",
        "bab49295e5f9064213762447224ccd83cea0cced0db5dcfc96f9c8a935ef67ee",
        "DerivedNormalizationProps-3.2.0.txt", "Unicode License V3 (data/unicode/LICENSE)"),
    "normalization-corrections": Source(
        "https://www.unicode.org/Public/3.2-Update/NormalizationCorrections-3.2.0.txt",
        "baaf9e087853800dd850f7925420b629b27b686ecf446379a2a3d32bcba2fa35",
        "NormalizationCorrections-3.2.0.txt", "Unicode License V3 (data/unicode/LICENSE)"),
    "normalization-test": Source(
        "https://www.unicode.org/Public/3.2-Update/NormalizationTest-3.2.0.txt",
        "c4513869bb7098d19838be4a1fd5d760843c5804bfe03bd6bbb20623ceb6e57d",
        "NormalizationTest-3.2.0.txt", "Unicode License V3 (data/unicode/LICENSE)"),
    "unicode-license": Source(
        "https://www.unicode.org/license.txt",
        "e7a93b009565cfce55919a381437ac4db883e9da2126fa28b91d12732bc53d96",
        "unicode-license.txt", "Unicode License V3"),
}

# Derived normalization properties NFKC needs (UAX #15): composition
# exclusions and the NFKC quick-check values.
NORMALIZATION_PROPERTIES = ("Full_Composition_Exclusion", "NFKC_NO", "NFKC_MAYBE")

# The standard 14 fonts (9.6.2.2): Adobe metrics, and a metric-compatible
# Apache-2.0 outline where one exists. Symbol and ZapfDingbats have none.
STANDARD_14 = {
    "Courier": "Cousine-Regular", "Courier-Bold": "Cousine-Bold",
    "Courier-Oblique": "Cousine-Italic", "Courier-BoldOblique": "Cousine-BoldItalic",
    "Helvetica": "Arimo-Regular", "Helvetica-Bold": "Arimo-Bold",
    "Helvetica-Oblique": "Arimo-Italic", "Helvetica-BoldOblique": "Arimo-BoldItalic",
    "Times-Roman": "Tinos-Regular", "Times-Bold": "Tinos-Bold",
    "Times-Italic": "Tinos-Italic", "Times-BoldItalic": "Tinos-BoldItalic",
    "Symbol": None, "ZapfDingbats": None,
}

# ISO 32000-2:2020, Table 116 (predefined CJK CMap names), without the
# Identity-H and Identity-V CMaps, which need no data.
PREDEFINED_CMAPS = """
GB-EUC-H GB-EUC-V GBpc-EUC-H GBpc-EUC-V GBK-EUC-H GBK-EUC-V GBKp-EUC-H GBKp-EUC-V
GBK2K-H GBK2K-V UniGB-UCS2-H UniGB-UCS2-V UniGB-UTF16-H UniGB-UTF16-V
B5pc-H B5pc-V HKscs-B5-H HKscs-B5-V ETen-B5-H ETen-B5-V ETenms-B5-H ETenms-B5-V
CNS-EUC-H CNS-EUC-V UniCNS-UCS2-H UniCNS-UCS2-V UniCNS-UTF16-H UniCNS-UTF16-V
83pv-RKSJ-H 90ms-RKSJ-H 90ms-RKSJ-V 90msp-RKSJ-H 90msp-RKSJ-V 90pv-RKSJ-H
Add-RKSJ-H Add-RKSJ-V EUC-H EUC-V Ext-RKSJ-H Ext-RKSJ-V H V
UniJIS-UCS2-H UniJIS-UCS2-V UniJIS-UCS2-HW-H UniJIS-UCS2-HW-V UniJIS-UTF16-H UniJIS-UTF16-V
KSC-EUC-H KSC-EUC-V KSCms-UHC-H KSCms-UHC-V KSCms-UHC-HW-H KSCms-UHC-HW-V KSCpc-EUC-H
UniKS-UCS2-H UniKS-UCS2-V UniKS-UTF16-H UniKS-UTF16-V
""".split()

# CID-to-Unicode data for the character collections of the predefined CMaps
# (9.10.2): Adobe now publishes it as cid2code.txt; only the UTF-32 column
# is kept.
UNICODE = {
    "Adobe-GB1": ("Adobe-GB1-6", "UniGB-UTF32"),
    "Adobe-CNS1": ("Adobe-CNS1-7", "UniCNS-UTF32"),
    "Adobe-Japan1": ("Adobe-Japan1-7", "UniJIS-UTF32"),
    "Adobe-Korea1": ("Adobe-Korea1-2", "UniKS-UTF32"),
}


def cache() -> Path:
    return Path(os.environ.get("PAGURA_DATA_SOURCES", "~/.cache/pagura-data-sources")).expanduser()


def unicode_selection() -> dict[str, bytes]:
    """Unicode 3.2.0 normalization data, projected to what NFKC needs. Kept
    lines are byte-for-byte originals; the projections are described in
    data/README.md and data/lock.json."""
    out: dict[str, bytes] = {}
    data = archive("unicode-data").decode("utf-8")
    kept = [line for line in data.splitlines()
            if line and (line.split(";")[3] != "0" or line.split(";")[5] != "")]
    out["unicode/UnicodeData-3.2.0-normalization.txt"] = ("\n".join(kept) + "\n").encode()
    out["unicode/CompositionExclusions-3.2.0.txt"] = archive("composition-exclusions")
    derived = archive("derived-normalization").decode("utf-8").splitlines()
    head = []
    for line in derived:
        if not line.startswith("#"):
            break
        head.append(line)
    rows = [line for line in derived if line and not line.startswith("#")
            and line.split(";")[1].split("#")[0].strip() in NORMALIZATION_PROPERTIES]
    out["unicode/DerivedNormalizationProps-3.2.0-nfkc.txt"] = ("\n".join(head + rows) + "\n").encode()
    out["unicode/NormalizationCorrections-3.2.0.txt"] = archive("normalization-corrections")
    out["unicode/NormalizationTest-3.2.0.txt"] = archive("normalization-test")
    out["unicode/LICENSE"] = archive("unicode-license")
    return out


def sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def archive(ident: str) -> bytes:
    source = SOURCES[ident]
    data = (cache() / source.file).read_bytes()
    if sha256(data) != source.sha256:
        raise SystemExit(f"{ident}: cached archive does not match its pin; fetch it again")
    return data


def fetch() -> None:
    cache().mkdir(parents=True, exist_ok=True)
    for ident, source in SOURCES.items():
        path = cache() / source.file
        if path.is_file() and sha256(path.read_bytes()) == source.sha256:
            print(f"{ident}: cached")
            continue
        request = Request(source.url, headers={"User-Agent": "Pagura-data/0.1"})
        with urlopen(request, timeout=60) as response:
            if not response.url.startswith("https:"):
                raise SystemExit(f"{ident}: HTTPS downgrade refused")
            data = response.read(MAX_ARCHIVE + 1)
        if len(data) > MAX_ARCHIVE or sha256(data) != source.sha256:
            raise SystemExit(f"{ident}: download does not match its pin; review the source before re-pinning")
        path.write_bytes(data)
        print(f"{ident}: {len(data)} bytes")


def selection() -> dict[str, bytes]:
    """Every output file of data/, by path relative to it."""
    out: dict[str, bytes] = {}
    with zipfile.ZipFile(io.BytesIO(archive("core14-afm"))) as afm:
        for name in [*STANDARD_14, "MustRead"]:
            member = f"{name}.html" if name == "MustRead" else f"{name}.afm"
            out[f"afm/{member}"] = afm.read(member)
    with tarfile.open(fileobj=io.BytesIO(archive("croscorefonts")), mode="r:bz2") as fonts:
        for outline in sorted({o for o in STANDARD_14.values() if o}):
            member = fonts.extractfile(f"croscorefonts-1.31.0/{outline}.ttf")
            out[f"fonts/{outline}.ttf"] = member.read()
    out["fonts/LICENSE-APACHE"] = (ROOT / "LICENSE-APACHE").read_bytes()
    out["fonts/NOTICE"] = (
        "Arimo, Tinos and Cousine (croscorefonts 1.31.0)\n"
        "Copyright 2010-2014 Google Inc. All Rights Reserved.\n"
        "Licensed under the Apache License, Version 2.0 (LICENSE-APACHE), as stated in\n"
        "each font's name table. The files are unmodified.\n").encode()
    with tarfile.open(fileobj=io.BytesIO(archive("cmap-resources")), mode="r:gz") as cmaps:
        prefix = "cmap-resources-20231115/"
        members = {m.name: m for m in cmaps.getmembers() if m.isfile()}
        located = {}
        for name, member in members.items():
            parts = name.removeprefix(prefix).split("/")
            if len(parts) == 3 and parts[1] == "CMap" and parts[2] in PREDEFINED_CMAPS:
                located[parts[2]] = (parts[0], member)
        missing = sorted(set(PREDEFINED_CMAPS) - located.keys())
        if missing:
            raise SystemExit(f"predefined CMaps missing from the source: {missing}")
        for name, (collection, member) in sorted(located.items()):
            out[f"cmaps/{collection}/{name}"] = cmaps.extractfile(member).read()
        out["cmaps/LICENSE.md"] = cmaps.extractfile(members[prefix + "LICENSE.md"]).read()
        for ordering, (collection, column) in UNICODE.items():
            text = cmaps.extractfile(members[f"{prefix}{collection}/cid2code.txt"]).read().decode("utf-8")
            out[f"cmaps/{collection}/cid2unicode.txt"] = project(text, collection, column).encode()
    out.update(unicode_selection())
    return out


def project(text: str, collection: str, column: str) -> str:
    """The CID and one Unicode column of cid2code.txt, rows without a value
    omitted; the original comment header is retained unchanged."""
    lines = text.splitlines()
    comments = [line for line in lines if line.startswith("#")]
    rows = [line.split("\t") for line in lines if line and not line.startswith("#")]
    header, body = rows[0], rows[1:]
    if header[0] != "CID" or column not in header:
        raise SystemExit(f"{collection}: unexpected cid2code.txt header")
    index = header.index(column)
    kept = [f"{row[0]}\t{row[index]}" for row in body if len(row) > index and row[index] != "*"]
    note = (f"# Projection of {collection}/cid2code.txt from cmap-resources 20231115:\n"
            f"# the CID and {column} columns, rows without a {column} value omitted.\n")
    return note + "\n".join(comments) + f"\nCID\t{column}\n" + "\n".join(kept) + "\n"


def build() -> None:
    files = selection()
    for path in sorted(DATA.rglob("*")):
        relative = path.relative_to(DATA).as_posix()
        if path.is_file() and relative not in files and relative not in ("lock.json", "README.md"):
            raise SystemExit(f"data/{relative} is not produced by this tool; remove it deliberately")
    for relative, data in files.items():
        target = DATA / relative
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_bytes(data)
    lock = {
        "schema": 1,
        "sources": {ident: {"url": s.url, "sha256": s.sha256, "licence": s.licence}
                    for ident, s in SOURCES.items()},
        "standard_14": {name: {"metrics": f"afm/{name}.afm",
                               "outline": f"fonts/{outline}.ttf" if outline else None}
                        for name, outline in STANDARD_14.items()},
        "files": {relative: sha256(data) for relative, data in sorted(files.items())},
    }
    LOCK.write_text(json.dumps(lock, indent=1) + "\n")
    print(f"wrote {len(files)} files, {sum(map(len, files.values())) >> 10} KiB")


def verify() -> None:
    lock = json.loads(LOCK.read_text())
    for relative, digest in lock["files"].items():
        path = DATA / relative
        if not path.is_file() or sha256(path.read_bytes()) != digest:
            raise SystemExit(f"data/{relative} is missing or changed")
    print(f"Verified {len(lock['files'])} bundled data files")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("command", choices=["fetch", "build", "verify"])
    {"fetch": fetch, "build": build, "verify": verify}[parser.parse_args().command]()
    return 0


if __name__ == "__main__":
    sys.exit(main())
