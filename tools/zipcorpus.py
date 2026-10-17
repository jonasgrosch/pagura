"""Pinned zip corpora (ADR 0007), extracted outside the repository.

    python3 tools/zipcorpus.py fetch NAME     # download (if needed), verify, extract
    python3 tools/zipcorpus.py path NAME      # print the extracted directory
    python3 tools/zipcorpus.py lock NAME      # write corpus/NAME.lock.json (review the diff)

realworld    SafeDocs CC-MAIN-2021-31-PDF-UNTRUNCATED zip 0000: about 1,000
             PDFs crawled from the public web in 2021 (the M1 metric).
adversarial  SafeDocs CC-MAIN-2021-31-UNSAFE corpora-pdf zip x001: 1,416
             fuzzer- and grammar-mutated PDFs for crash, hang and budget tests.

Both are published by Digital Corpora for the DARPA SafeDocs program and are
subject to Common Crawl's terms of use. Files stay in $PAGURA_CORPORA_DIR
(default ~/.cache/pagura-corpora); only locks with hashes are committed.
Files are untrusted data: extraction accepts only members matching the
corpus's name pattern, caps sizes, writes flat `<name>.pdf` files and never
executes anything.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import sys
import urllib.request
import zipfile
from dataclasses import dataclass
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
BASE = "https://digitalcorpora.s3.amazonaws.com/corpora/files"
TERMS = "https://commoncrawl.org/terms-of-use/full/"
MAX_MEMBER_BYTES = 1 << 30
MAX_DOWNLOAD_BYTES = 3 << 30


@dataclass(frozen=True)
class Corpus:
    name: str
    directory: str
    description: str
    sources: dict[str, str]
    member: re.Pattern
    content_addressed: bool  # member names are the SHA-256 of their content


CORPORA = {corpus.name: corpus for corpus in [
    Corpus("realworld", "safedocs-cc-main-2021-31",
           "SafeDocs CC-MAIN-2021-31-PDF-UNTRUNCATED zip 0000: PDFs crawled from the public web, July-August 2021",
           {"zip": f"{BASE}/CC-MAIN-2021-31-PDF-UNTRUNCATED/zipfiles/0000-0999/0000.zip",
            "readme": f"{BASE}/CC-MAIN-2021-31-PDF-UNTRUNCATED/README.md",
            "pdfinfo": f"{BASE}/CC-MAIN-2021-31-PDF-UNTRUNCATED/metadata/pdfinfo-20230324-1k.csv"},
           re.compile(r"^(\d{7})\.pdf$"), False),
    Corpus("adversarial", "safedocs-unsafe",
           "SafeDocs CC-MAIN-2021-31-UNSAFE corpora-pdf zip x001: fuzzer- and grammar-mutated PDFs",
           {"zip": f"{BASE}/CC-MAIN-2021-31-UNSAFE/corpora-pdf/corpora-pdf-x001.zip",
            "readme": f"{BASE}/CC-MAIN-2021-31-UNSAFE/README.md"},
           # Names look like SHA-256 digests but are not digests of the content.
           re.compile(r"^corpora-pdf/[A-Za-z0-9]+/[0-9a-f]{2}/[0-9a-f]{2}/([0-9a-f]{64})$"), False),
]}


def lock_path(corpus: Corpus) -> Path:
    return ROOT / "corpus" / f"{corpus.name}.lock.json"


def cache(corpus: Corpus) -> Path:
    base = Path(os.environ.get("PAGURA_CORPORA_DIR", "~/.cache/pagura-corpora")).expanduser()
    return base / corpus.directory


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as file:
        for block in iter(lambda: file.read(1 << 20), b""):
            digest.update(block)
    return digest.hexdigest()


def download(url: str, target: Path) -> None:
    if target.exists():
        return
    target.parent.mkdir(parents=True, exist_ok=True)
    partial = target.with_suffix(target.suffix + ".part")
    print(f"Downloading {url}", flush=True)
    with urllib.request.urlopen(url, timeout=60) as response, partial.open("wb") as file:
        received = 0
        for block in iter(lambda: response.read(1 << 20), b""):
            received += len(block)
            if received > MAX_DOWNLOAD_BYTES:
                partial.unlink()
                raise SystemExit(f"{url}: larger than {MAX_DOWNLOAD_BYTES} bytes")
            file.write(block)
    partial.rename(target)


def members(corpus: Corpus, archive: zipfile.ZipFile) -> list[tuple[zipfile.ZipInfo, str]]:
    """Accepted members with their flat extracted names; everything else is skipped."""
    accepted = []
    for info in archive.infolist():
        match = None if info.is_dir() else corpus.member.match(info.filename)
        if match is None:
            continue
        if info.file_size > MAX_MEMBER_BYTES:
            raise SystemExit(f"{info.filename}: declared size {info.file_size} exceeds the cap")
        stem = match.group(1)
        accepted.append((info, f"{stem}.pdf"))
    return accepted


def extract(archive: zipfile.ZipFile, info: zipfile.ZipInfo, target: Path) -> str:
    """Writes one member to `target` with a size cap; returns its SHA-256."""
    digest, size = hashlib.sha256(), 0
    partial = target.with_suffix(".part")
    with archive.open(info) as source, partial.open("wb") as file:
        for block in iter(lambda: source.read(1 << 20), b""):
            size += len(block)
            if size > MAX_MEMBER_BYTES:
                raise SystemExit(f"{target.name}: exceeds the size cap while extracting")
            digest.update(block)
            file.write(block)
    partial.rename(target)
    return digest.hexdigest()


def zip_file(corpus: Corpus) -> Path:
    return cache(corpus) / "download" / corpus.sources["zip"].rsplit("/", 1)[-1]


def fetch(corpus: Corpus) -> Path:
    downloads = cache(corpus) / "download"
    for url in corpus.sources.values():
        download(url, downloads / url.rsplit("/", 1)[-1])
    lock = json.loads(lock_path(corpus).read_text()) if lock_path(corpus).exists() else None
    if lock and sha256_file(zip_file(corpus)) != lock["zip"]["sha256"]:
        raise SystemExit(f"downloaded zip does not match {lock_path(corpus).relative_to(ROOT)}")
    pdfs = cache(corpus) / "pdf"
    pdfs.mkdir(parents=True, exist_ok=True)
    expected = {entry["name"]: entry["sha256"] for entry in lock["files"]} if lock else {}
    with zipfile.ZipFile(zip_file(corpus)) as archive:
        for info, name in members(corpus, archive):
            target = pdfs / name
            if target.exists() and expected.get(name) == sha256_file(target):
                continue
            digest = extract(archive, info, target)
            if corpus.content_addressed and digest != name.removesuffix(".pdf"):
                raise SystemExit(f"{name}: content does not match its SHA-256 name")
            if name in expected and digest != expected[name]:
                raise SystemExit(f"{name}: hash differs from the lock")
    missing = sorted(set(expected) - {path.name for path in pdfs.glob("*.pdf")})
    if missing:
        raise SystemExit(f"missing extracted files: {missing[:5]}")
    print(f"{corpus.name} corpus ready: {pdfs}")
    return pdfs


def write_lock(corpus: Corpus) -> None:
    if not zip_file(corpus).exists():
        raise SystemExit("run `fetch` first")
    pdfs = cache(corpus) / "pdf"
    with zipfile.ZipFile(zip_file(corpus)) as archive:
        files = sorted(({"name": name, "size": info.file_size, "sha256": sha256_file(pdfs / name)}
                        for info, name in members(corpus, archive)), key=lambda entry: entry["name"])
    downloads = cache(corpus) / "download"
    lock = {
        "schema": 1,
        "name": corpus.name,
        "description": corpus.description,
        "publisher": "Digital Corpora (corpus by NASA JPL for the DARPA SafeDocs program)",
        "terms": TERMS,
        "sources": corpus.sources,
        "zip": {"size": zip_file(corpus).stat().st_size, "sha256": sha256_file(zip_file(corpus))},
        "metadata_sha256": {key: sha256_file(downloads / url.rsplit("/", 1)[-1])
                            for key, url in corpus.sources.items() if key != "zip"},
        "files": files,
    }
    lock_path(corpus).write_text(json.dumps(lock, indent=1) + "\n")
    print(f"Wrote {lock_path(corpus).relative_to(ROOT)}: {len(files)} files")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("command", choices=["fetch", "path", "lock"])
    parser.add_argument("name", choices=sorted(CORPORA))
    arguments = parser.parse_args()
    corpus = CORPORA[arguments.name]
    if arguments.command == "fetch":
        fetch(corpus)
    elif arguments.command == "path":
        print(cache(corpus) / "pdf")
    else:
        write_lock(corpus)
    return 0


if __name__ == "__main__":
    sys.exit(main())
