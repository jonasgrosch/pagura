"""Milestone scoreboard (ADR 0007): what Pagura opens and renders in a corpus.

    python3 tools/scoreboard.py CORPUS_DIR --name upstream [--max-pages 20]

Runs `pagura scan` on every PDF below CORPUS_DIR, with a per-file timeout, and
reports: files opened strictly, opened only with repairs, failed to open (by
error kind and feature tag), pages found and rendered, the histogram of
Feature tags that stop pages from rendering, repairs applied, crashes and
timeouts. The raw report goes to the ignored benchmarks/results/local/; the
Markdown summary (with corpus and binary digests) goes to stdout or --summary.
Corpus files are untrusted data and are never executed or modified.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import subprocess
import sys
from collections import Counter
from concurrent.futures import ThreadPoolExecutor
from datetime import datetime, timezone
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
BINARY = ROOT / "target/release/pagura"
LOCAL = ROOT / "benchmarks/results/local"


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as file:
        for block in iter(lambda: file.read(1 << 20), b""):
            digest.update(block)
    return digest.hexdigest()


def looks_like_pdf(path: Path) -> bool:
    """Whether a `%PDF-` header occurs in the first 1024 bytes (clause 7.5.2
    requires offset 0; readers commonly accept leading junk, see ADR 0003)."""
    with path.open("rb") as file:
        return b"%PDF-" in file.read(1024)


def scan(path: Path, arguments) -> dict:
    if not looks_like_pdf(path):
        return {"outcome": "not-pdf"}
    command = [str(BINARY), "scan", str(path), str(arguments.dpi)]
    if arguments.max_pages:
        command[2:2] = ["--max-pages", str(arguments.max_pages)]
    try:
        result = subprocess.run(command, capture_output=True, timeout=arguments.timeout)
    except subprocess.TimeoutExpired:
        return {"outcome": "timeout"}
    if result.returncode != 0:
        return {"outcome": "crash", "exit_code": result.returncode,
                "stderr": result.stderr.decode(errors="replace")[-2000:]}
    try:
        return {"outcome": "scanned", **json.loads(result.stdout)}
    except json.JSONDecodeError:
        return {"outcome": "crash", "exit_code": 0, "stderr": "invalid JSON from scan"}


def tag(error: dict) -> str:
    return error["feature"] or error["kind"]


def summarize(name: str, report: dict) -> str:
    files = report["files"]
    total = len(files)
    count = Counter()
    open_failures, page_failures, repairs = Counter(), Counter(), Counter()
    pages = rendered = 0
    for entry in files:
        result = entry["result"]
        if result["outcome"] != "scanned":
            count[result["outcome"]] += 1
            continue
        if result["open"] is not None:
            count["open-failed"] += 1
            open_failures[tag(result["open"])] += 1
            continue
        count["strict" if result["strict"] else "repaired"] += 1
        repairs.update(result["repairs"])
        pages += result["pages"] or 0
        rendered += result["rendered"]
        page_failures.update(tag(failure["error"]) for failure in result["failures"])
    opened = count["strict"] + count["repaired"]
    candidates = total - count["not-pdf"]

    def percent(part: int, whole: int) -> str:
        return f"{100 * part / whole:.1f}%" if whole else "n/a"

    def table(counter: Counter, heading: str) -> list[str]:
        if not counter:
            return []
        lines = [f"| {heading} | Count |", "|---|---:|"]
        lines += [f"| `{key}` | {value} |" for key, value in counter.most_common()]
        return lines + [""]

    lines = [
        f"# Scoreboard: {name}", "",
        f"Recorded {report['created_at']} at commit `{report['commit']}`; binary SHA-256 `{report['binary_sha256'][:16]}…`;",
        f"corpus digest `{report['corpus_sha256'][:16]}…` over {total} files. Pages scanned at {report['dpi']} dpi"
        + (f", at most {report['max_pages']} per file." if report["max_pages"] else "."), "",
        "| Metric | Value |", "|---|---:|",
        f"| Not PDF (no `%PDF-` in the first 1024 bytes) | {count['not-pdf']} |",
        f"| PDF files opened | {opened} / {candidates} ({percent(opened, candidates)}) |",
        f"| … strictly | {count['strict']} |",
        f"| … only with repairs | {count['repaired']} |",
        f"| Failed to open | {count['open-failed']} |",
        f"| Crashes | {count['crash']} |",
        f"| Timeouts | {count['timeout']} |",
        f"| Pages rendered | {rendered} / {pages} ({percent(rendered, pages)}) |", "",
    ]
    lines += table(open_failures, "Open failure")
    lines += table(page_failures, "Page failure")
    lines += table(repairs, "Repair")
    return "\n".join(lines)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("corpus", type=Path)
    parser.add_argument("--name", required=True)
    parser.add_argument("--jobs", type=int, default=8)
    parser.add_argument("--timeout", type=int, default=60)
    parser.add_argument("--dpi", type=float, default=72)
    parser.add_argument("--max-pages", type=int, default=0)
    parser.add_argument("--summary", type=Path)
    parser.add_argument("--no-build", action="store_true")
    arguments = parser.parse_args()
    if not arguments.no_build:
        subprocess.run(["cargo", "build", "--locked", "--offline", "--release", "-q", "-p", "pagura"], cwd=ROOT, check=True)
    paths = sorted(p for p in arguments.corpus.rglob("*") if p.is_file() and p.suffix.lower() == ".pdf")
    if not paths:
        print(f"no PDF files below {arguments.corpus}", file=sys.stderr)
        return 2
    with ThreadPoolExecutor(max_workers=arguments.jobs) as pool:
        results = list(pool.map(lambda path: scan(path, arguments), paths))
    hashes = [sha256(path) for path in paths]
    commit = subprocess.run(["git", "rev-parse", "--short", "HEAD"], capture_output=True, text=True, cwd=ROOT).stdout.strip()
    dirty = subprocess.run(["git", "status", "--porcelain", "--", "crates"], capture_output=True, text=True, cwd=ROOT).stdout
    commit += "-dirty" if dirty.strip() else ""
    report = {
        "schema": 1, "name": arguments.name, "created_at": datetime.now(timezone.utc).isoformat(timespec="seconds"),
        "commit": commit, "binary_sha256": sha256(BINARY), "dpi": arguments.dpi, "max_pages": arguments.max_pages,
        "corpus_sha256": hashlib.sha256("".join(sorted(hashes)).encode()).hexdigest(),
        "files": [{"path": str(path.relative_to(arguments.corpus)), "sha256": digest, "result": result}
                  for path, digest, result in zip(paths, hashes, results)],
    }
    LOCAL.mkdir(parents=True, exist_ok=True)
    raw = LOCAL / f"scoreboard-{arguments.name}.json"
    raw.write_text(json.dumps(report, indent=1) + "\n")
    summary = summarize(arguments.name, report) + f"\nRaw report: `{raw.relative_to(ROOT)}` (not committed).\n"
    if arguments.summary:
        arguments.summary.parent.mkdir(parents=True, exist_ok=True)
        arguments.summary.write_text(summary)
    print(summary)
    return 0


if __name__ == "__main__":
    sys.exit(main())
