"""Pinned upstream test corpora (ADR 0007): public test suites and other
projects' own test PDFs, used as *input documents* only.

    python3 tools/corpus.py fetch [ID ...]      # pinned sparse checkout, verify, lock
    python3 tools/corpus.py fetch --refresh ID  # move ID to its current HEAD (review the diff)
    python3 tools/corpus.py verify              # every locked file present and unchanged
    python3 tools/corpus.py path ID             # directory to pass to tools/scoreboard.py
    python3 tools/corpus.py list

Only files matching each source's patterns (PDFs, models, licence notices)
are downloaded, through a shallow, blob-less sparse checkout: the projects'
source code is never fetched. Data lives in $PAGURA_CORPORA_DIR (default
~/.cache/pagura-corpora), outside the repository; corpus/upstream/<ID>.lock.json
records the revision and the SHA-256 of every file. Files are untrusted data:
nothing in them is executed, and Git runs without hooks or filters.
Redistribution rights vary per file; never commit or publish them.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
import shutil
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
LOCKS = ROOT / "corpus/upstream"
LICENSES = ("/LICENSE*", "/LICENCE*", "/COPYING*", "/NOTICE*", "/README*")
MAX_FILE_BYTES = 256 << 20


@dataclass(frozen=True)
class Source:
    ident: str
    repository: str
    patterns: tuple[str, ...]
    purpose: str


SOURCES = {source.ident: source for source in [
    Source("pdf20", "https://github.com/pdf-association/pdf20examples", ("*.pdf",),
           "PDF Association PDF 2.0 example files"),
    Source("differences", "https://github.com/pdf-association/pdf-differences", ("*.pdf",),
           "PDF Association cases where viewers disagree"),
    Source("arlington", "https://github.com/pdf-association/arlington-pdf-model", ("/tsv/**",),
           "machine-readable PDF object model (TSV), for future structural checks"),
    Source("verapdf", "https://github.com/veraPDF/veraPDF-corpus", ("*.pdf",),
           "veraPDF PDF/A and PDF/UA validation corpus with documented intent"),
    Source("pdfjs", "https://github.com/mozilla/pdf.js", ("/test/pdfs/*.pdf",),
           "pdf.js regression PDFs (bug reproductions)"),
    Source("pdfium", "https://pdfium.googlesource.com/pdfium", ("/testing/resources/**/*.pdf",),
           "PDFium test resources"),
    Source("qpdf", "https://github.com/qpdf/qpdf", ("*.pdf",), "qpdf test suite inputs"),
    Source("pdfbox", "https://github.com/apache/pdfbox", ("*.pdf",), "Apache PDFBox test resources"),
    Source("poppler", "https://gitlab.freedesktop.org/poppler/test", ("*.pdf",), "Poppler test-data repository"),
]}


def cache_root() -> Path:
    return Path(os.environ.get("PAGURA_CORPORA_DIR", "~/.cache/pagura-corpora")).expanduser() / "upstream"


def lock_path(ident: str) -> Path:
    return LOCKS / f"{ident}.lock.json"


def git(*args: str, cwd: Path | None = None) -> str:
    """Runs Git with hooks, filters and credential prompts disabled."""
    environment = {**os.environ, "GIT_TERMINAL_PROMPT": "0", "GIT_LFS_SKIP_SMUDGE": "1"}
    command = ["git", "-c", "core.hooksPath=/dev/null", "-c", "filter.lfs.process=", "-c", "protocol.file.allow=never", *args]
    result = subprocess.run(command, cwd=cwd, capture_output=True, text=True, timeout=3600, env=environment)
    if result.returncode != 0:
        raise SystemExit(f"git {' '.join(args[:2])} failed: {result.stderr.strip()[-500:]}")
    return result.stdout


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as file:
        for block in iter(lambda: file.read(1 << 20), b""):
            digest.update(block)
    return digest.hexdigest()


def checkout(source: Source, revision: str) -> Path:
    """A shallow sparse checkout of `revision` containing only matching blobs."""
    directory = cache_root() / source.ident / revision
    if (directory / ".git").exists():
        return directory
    partial = directory.with_name(revision + ".partial")
    shutil.rmtree(partial, ignore_errors=True)
    partial.mkdir(parents=True)
    git("init", "-q", cwd=partial)
    git("remote", "add", "origin", source.repository, cwd=partial)
    git("fetch", "-q", "--depth", "1", "--filter=blob:none", "origin", revision, cwd=partial)
    git("sparse-checkout", "set", "--no-cone", *source.patterns, *LICENSES, cwd=partial)
    git("checkout", "-q", "FETCH_HEAD", cwd=partial)
    partial.rename(directory)
    return directory


def tracked_files(directory: Path) -> list[str]:
    names = git("ls-files", "-z", cwd=directory).split("\0")
    return sorted(name for name in names if name and (directory / name).is_file())


def fetch(idents: list[str], refresh: bool) -> None:
    LOCKS.mkdir(parents=True, exist_ok=True)
    for ident in idents:
        source = SOURCES[ident]
        previous = json.loads(lock_path(ident).read_text()) if lock_path(ident).exists() else None
        revision = previous["revision"] if previous and not refresh else git("ls-remote", source.repository, "HEAD").split()[0]
        directory = checkout(source, revision)
        files, skipped = [], []
        for name in tracked_files(directory):
            size = (directory / name).stat().st_size
            if size > MAX_FILE_BYTES:
                skipped.append({"path": name, "size": size, "reason": "exceeds per-file cap"})
                continue
            files.append({"path": name, "size": size, "sha256": sha256(directory / name)})
        if previous and not refresh and {f["path"]: f["sha256"] for f in files} != {f["path"]: f["sha256"] for f in previous["files"]}:
            raise SystemExit(f"{ident}: checkout differs from its lock; use --refresh to change the pin deliberately")
        lock = {"schema": 2, "id": ident, "repository": source.repository, "revision": revision,
                "purpose": source.purpose, "patterns": list(source.patterns),
                "use": "input documents only; no expected outputs, never normative",
                "pdf_files": sum(f["path"].lower().endswith(".pdf") for f in files),
                "bytes": sum(f["size"] for f in files), "files": files, "skipped": skipped}
        lock_path(ident).write_text(json.dumps(lock, indent=1) + "\n")
        print(f"{ident}: {lock['pdf_files']} PDFs, {lock['bytes'] >> 20} MiB at {revision[:12]}", flush=True)


def verify() -> None:
    checked = 0
    for path in sorted(LOCKS.glob("*.lock.json")):
        lock = json.loads(path.read_text())
        directory = cache_root() / lock["id"] / lock["revision"]
        for entry in lock["files"]:
            target = directory / entry["path"]
            if not target.is_file() or sha256(target) != entry["sha256"]:
                raise ValueError(f"missing or changed upstream corpus file: {target}")
            checked += 1
    print(f"Verified {checked} locked upstream files")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("command", choices=["fetch", "verify", "path", "list"])
    parser.add_argument("ids", nargs="*")
    parser.add_argument("--refresh", action="store_true", help="re-pin to the current upstream HEAD")
    arguments = parser.parse_args()
    unknown = set(arguments.ids) - SOURCES.keys()
    if unknown:
        parser.error(f"unknown sources: {', '.join(sorted(unknown))}")
    if arguments.command == "fetch":
        fetch(arguments.ids or list(SOURCES), arguments.refresh)
    elif arguments.command == "verify":
        verify()
    elif arguments.command == "path":
        if len(arguments.ids) != 1:
            parser.error("path takes exactly one source ID")
        lock = json.loads(lock_path(arguments.ids[0]).read_text())
        print(cache_root() / lock["id"] / lock["revision"])
    else:
        for source in SOURCES.values():
            pinned = lock_path(source.ident).exists()
            print(f"{source.ident:12} {'pinned ' if pinned else 'unpinned'} {source.repository}  {source.purpose}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
