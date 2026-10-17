"""Builds libFuzzer seed corpora in fuzz/corpus/<target>/ from local data.

Seeds come from generated fixtures (in the repository) and, when present,
the pinned out-of-tree corpora. Files are only copied, never executed.
"""
import hashlib
import json
import os
import shutil
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
CORPUS = Path(__file__).resolve().parent / "corpus"
MAX_SEED = 256 * 1024
PER_SOURCE = 300


def add(target: str, data: bytes) -> None:
    if 0 < len(data) <= MAX_SEED:
        directory = CORPUS / target
        directory.mkdir(parents=True, exist_ok=True)
        (directory / hashlib.sha256(data).hexdigest()[:20]).write_bytes(data)


def pdfs(directory: Path) -> list[Path]:
    files = sorted(p for p in directory.rglob("*") if p.is_file() and p.suffix.lower() == ".pdf")
    return [p for p in files if p.stat().st_size <= MAX_SEED][:PER_SOURCE]


def main() -> int:
    shutil.rmtree(CORPUS, ignore_errors=True)
    sources = [ROOT / "corpus/generated"]
    cache = Path(os.environ.get("PAGURA_CORPORA_DIR", "~/.cache/pagura-corpora")).expanduser()
    sources += [p for p in [cache / "upstream", cache / "safedocs-unsafe/pdf", cache / "safedocs-cc-main-2021-31/pdf"] if p.exists()]
    for source in sources:
        for path in pdfs(source):
            data = path.read_bytes()
            for target in ("document", "render", "syntax"):
                add(target, data)
    for vector in sorted((ROOT / "corpus/generated/codec").glob("*.in")):
        data = vector.read_bytes()
        for selector in range(8):
            add("filters", bytes([selector]) + data)
    counts = {target.name: len(list(target.iterdir())) for target in sorted(CORPUS.iterdir())}
    print(json.dumps(counts))
    return 0


if __name__ == "__main__":
    sys.exit(main())
