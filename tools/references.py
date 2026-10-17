"""Acquire official references into a private cache; keep a provenance ledger."""
from __future__ import annotations

import argparse
import concurrent.futures
import hashlib
import json
import os
from datetime import datetime, timezone
from html.parser import HTMLParser
from pathlib import Path
from urllib.parse import urljoin, urlsplit
from urllib.request import Request, urlopen

ROOT = Path(__file__).resolve().parents[1]
# Private full texts live outside every checkout so all worktrees share them.
# Lock entries keep the historical ".cache/references/<file>" spelling, which
# `cached_path` maps into this directory.
CACHE = Path(os.environ.get("PAGURA_REFERENCES_DIR", "~/.cache/pagura-references")).expanduser()
CACHE_PREFIX = ".cache/references/"


def cached_path(cache_path: str) -> Path:
    """The local file for a lock entry's `cache_path`."""
    if cache_path.startswith(CACHE_PREFIX):
        return CACHE / cache_path.removeprefix(CACHE_PREFIX)
    return ROOT / cache_path
NORMATIVE = "https://pdfa.org/iso-32000-normative-references/"
ARCHIVE = "https://pdfa.org/resource/pdf-specification-archive/"
MAX_BYTES = 40 * 1024 * 1024
SEEDS = [
    ("adobe-pdf17", "Adobe equivalent of ISO 32000-1:2008", "https://opensource.adobe.com/dc-acrobat-sdk-docs/pdfstandards/PDF32000_2008.pdf", "normative-legacy-core"),
    ("pdf20-access", "PDF 2.0 / ISO 32000-2:2020, Errata Collection 3", "https://pdfa.org/resource/iso-32000-2/", "access-page"),
    ("normative-index", "Complete PDF 2.0 normative reference index", NORMATIVE, "index"),
    ("legacy-index", "Legacy Adobe specifications and extensions", ARCHIVE, "index"),
    ("pdf-errata", "PDF specification errata; retain approval status", "https://pdf-issues.pdfa.org/", "errata"),
    ("iso-status", "Current ISO PDF standards and extensions", "https://pdfa.org/iso-status/", "index"),
    ("icc-specifications", "ICC specification versions", "https://www.color.org/icc_specs2/", "index"),
    ("icc-v43", "ICC.1:2010 (PDF 2.0 dated reference)", "https://www.color.org/specification/ICC1v43_2010-12.pdf", "specification"),
    ("pades-baseline", "ETSI EN 319 142-1 V1.2.1", "https://www.etsi.org/deliver/etsi_en/319100_319199/31914201/01.02.01_60/en_31914201v010201p.pdf", "specification"),
    ("verapdf-rules", "veraPDF PDF/A and PDF/UA machine validation", "https://docs.verapdf.org/validation/", "validation"),
    ("ghent-suite", "Ghent PDF Output Suite 5", "https://gwg.org/resources/ghent-pdf-output-suite/", "corpus-access"),
    ("adobe-automation", "Adobe desktop automation policy", "https://helpx.adobe.com/acrobat/kb/automation-and-document-workflows.html", "license-policy"),
]


class IndexParser(HTMLParser):
    """Collect normative paragraphs and direct document links, not navigation."""

    def __init__(self):
        super().__init__()
        self.heading = None
        self.heading_text = []
        self.section = ""
        self.paragraph = None
        self.links = []
        self.entries = []
        self.all_links = []

    def handle_starttag(self, tag, attrs):
        attributes = dict(attrs)
        if tag in ("h1", "h2", "h3"):
            self.heading, self.heading_text = tag, []
        if tag == "p":
            self.paragraph, self.links = [], []
        if tag == "a" and attributes.get("href"):
            link = attributes["href"]
            self.all_links.append(link)
            if self.paragraph is not None:
                self.links.append(link)

    def handle_data(self, data):
        if self.heading:
            self.heading_text.append(data)
        if self.paragraph is not None:
            self.paragraph.append(data)

    def handle_endtag(self, tag):
        if tag == self.heading:
            self.section = " ".join("".join(self.heading_text).split())
            self.heading = None
        if tag == "p" and self.paragraph is not None:
            description = " ".join("".join(self.paragraph).split())
            if self.section == "Normative references" and description and not description.startswith("NOTE"):
                links = list(dict.fromkeys(x for x in self.links if x.startswith("http")))
                if links:
                    self.entries.append({"description": description, "urls": links})
            self.paragraph = None


def acquire(url):
    """No cookies, credentials, form submissions or access-control bypasses."""
    request = Request(url, headers={"User-Agent": "Pagura-reference-registry/0.1"})
    with urlopen(request, timeout=25) as response:
        data = response.read(MAX_BYTES + 1)
        if len(data) > MAX_BYTES:
            raise ValueError("resource exceeds 40 MiB acquisition cap")
        if response.url.startswith("http:"):
            raise ValueError("HTTPS downgrade refused")
        mime = response.headers.get_content_type()
        digest = hashlib.sha256(data).hexdigest()
        extension = ".pdf" if data.startswith(b"%PDF-") else ".html" if "html" in mime else ".bin"
        CACHE.mkdir(parents=True, exist_ok=True)
        path = CACHE / (digest + extension)
        path.write_bytes(data)
        return {"status": "cached", "url": url, "resolved_url": response.url,
                "sha256": digest, "bytes": len(data), "mime": mime,
                "cache_path": CACHE_PREFIX + path.name,
                "retrieved_at": datetime.now(timezone.utc).isoformat()}, data


def sync():
    lock_path = ROOT / "references/lock.json"
    previous = json.loads(lock_path.read_text()) if lock_path.exists() else {"sources": []}
    records, pages = [], {}
    jobs = [(ident, title, url, kind) for ident, title, url, kind in SEEDS]
    for ident, title, url, kind in jobs:
        record = {"id": ident, "title": title, "url": url, "kind": kind,
                  "redistribution": "not granted by this project; private cache only"}
        try:
            evidence, data = acquire(url)
            record.update(evidence)
            pages[url] = data
        except Exception as error:
            record.update(status="unavailable", error=f"{type(error).__name__}: {error}")
        records.append(record)
        print(f"{ident}: {record['status']}", flush=True)

    parser = IndexParser()
    parser.feed(pages.get(NORMATIVE, b"").decode("utf-8", errors="replace"))
    normative = parser.entries
    if not normative:
        raise RuntimeError("Normative index was unavailable or changed shape; refusing empty inventory")
    (ROOT / "references/normative-index.json").write_text(json.dumps({
        "source": NORMATIVE, "entries": normative,
        "note": "Dated references retain their cited editions; newer is not automatically compatible."
    }, indent=2) + "\n")

    direct = {url for item in normative for url in item["urls"] if urlsplit(url).path.lower().endswith(".pdf")}
    archive = IndexParser()
    archive.feed(pages.get(ARCHIVE, b"").decode("utf-8", errors="replace"))
    direct.update(urljoin(ARCHIVE, u) for u in archive.all_links
                  if urlsplit(u).hostname == "opensource.adobe.com" and urlsplit(u).path.lower().endswith(".pdf"))
    # Cache all published errata pages, preserving their explicit ISO/industry status.
    errata = IndexParser()
    errata.feed(pages.get("https://pdf-issues.pdfa.org/", b"").decode("utf-8", errors="replace"))
    direct.update(urljoin("https://pdf-issues.pdfa.org/", u) for u in errata.all_links
                  if urlsplit(urljoin("https://pdf-issues.pdfa.org/", u)).hostname == "pdf-issues.pdfa.org"
                  and ".html" in u)
    direct = sorted(u for u in direct if u.startswith("https://") and u not in pages)

    def download(url):
        record = {"id": "ref-" + hashlib.sha256(url.encode()).hexdigest()[:12], "url": url,
                  "kind": "official-linked-reference", "redistribution": "private cache only"}
        try:
            evidence, _ = acquire(url)
            record.update(evidence)
        except Exception as error:
            record.update(status="unavailable", error=f"{type(error).__name__}: {error}")
        return record

    with concurrent.futures.ThreadPoolExecutor(max_workers=4) as pool:
        for record in pool.map(download, direct):
            records.append(record)
            print(f"{record['id']}: {record['status']}", flush=True)

    known = {r["url"] for r in records}
    for item in normative:
        for url in item["urls"]:
            if url not in known:
                records.append({"id": "ref-" + hashlib.sha256(url.encode()).hexdigest()[:12],
                                "url": url, "title": item["description"], "status": "indexed-only",
                                "reason": "Publisher page/repository; full document access not established."})
                known.add(url)
    records.append({"id": "iso32000-2-full-text", "url": "https://www.pdfa-inc.org/cart/?add-to-cart=2009",
                    "status": "requires-user-acquisition", "kind": "normative-core",
                    "title": "ISO 32000-2:2020 + Errata Collection 3 + ISO/TS 32001-32004 bundle",
                    "reason": "Sponsored publisher checkout; no account, terms or personal data submitted."})
    lock = {"schema": 1, "sources": preserve_local(previous, records)}
    (ROOT / "references/lock.json").write_text(json.dumps(lock, indent=2) + "\n")
    report(lock)


def report(lock):
    records = lock["sources"]
    normative = json.loads((ROOT / "references/normative-index.json").read_text())["entries"]
    counts = {status: sum(r["status"] == status for r in records)
              for status in sorted({r["status"] for r in records})}
    lines = ["# Reference acquisition", "", "Generated by `python3 tools/references.py sync`.", "",
             f"Normative reference entries indexed: {len(normative)}.", "",
             "| Status | Resources |", "|---|---:|"]
    lines.extend(f"| {k} | {v} |" for k, v in counts.items())
    full_text = next((r for r in records if r["id"] == "iso32000-2-full-text"), {})
    acquisition = "The full PDF 2.0 bundle is locally supplied; consult its hash and access record." if full_text.get("status") == "locally-supplied" else "The full PDF 2.0 bundle is not acquired: the publisher requires a checkout."
    lines += ["", "## Access gaps", "", acquisition,
              "Place a legitimately acquired copy in `$PAGURA_REFERENCES_DIR` (shared by all checkouts)",
              "or `references/private/` and register it with",
              "`python3 tools/references.py register PATH --id iso32000-2-full-text`.",
              "Indexed-only references are not claimed as downloaded or read. Some ISO standards",
              "require purchase or separate access. This ledger makes those gaps explicit.", "",
              "Failures:"]
    lines.extend(f"- [{r['id']}]({r['url']}): {r['error']}" for r in records if r["status"] == "unavailable")
    (ROOT / "references/ACQUISITION.md").write_text("\n".join(lines) + "\n")
    print(json.dumps(counts))


def preserve_local(previous, records):
    """Refreshing web metadata must never forget a legitimately supplied document."""
    supplied = {r["id"]: r for r in previous["sources"] if r["status"] == "locally-supplied"}
    for record in supplied.values():
        path = cached_path(record["cache_path"]).resolve()
        if not path.is_relative_to((ROOT / "references/private").resolve()):
            raise ValueError("Local reference escapes private directory")
        if hashlib.sha256(path.read_bytes()).hexdigest() != record["sha256"]:
            raise ValueError("Locally supplied reference changed; register it again explicitly")
    merged = [supplied.pop(r["id"], r) for r in records]
    return merged + list(supplied.values())


def register(path, ident):
    path = Path(path).resolve()
    shared = path.parent == CACHE.resolve()
    if not (shared or path.is_relative_to((ROOT / "references/private").resolve())):
        raise ValueError("Licensed local documents must be in $PAGURA_REFERENCES_DIR or references/private/")
    if path.stat().st_size > MAX_BYTES:
        raise ValueError("Local reference exceeds 40 MiB budget")
    data = path.read_bytes()
    if not data.startswith(b"%PDF-"):
        raise ValueError("Expected a PDF document")
    lock_path = ROOT / "references/lock.json"
    lock = json.loads(lock_path.read_text())
    for record in lock["sources"]:
        if record["id"] == ident:
            record.update(status="locally-supplied", sha256=hashlib.sha256(data).hexdigest(),
                          cache_path=CACHE_PREFIX + path.name if shared else str(path.relative_to(ROOT)), bytes=len(data))
            break
    else:
        raise ValueError("Unknown reference ID")
    lock_path.write_text(json.dumps(lock, indent=2) + "\n")
    report(lock)


if __name__ == "__main__":
    argument = argparse.ArgumentParser(description=__doc__)
    sub = argument.add_subparsers(dest="command", required=True)
    sub.add_parser("sync")
    reg = sub.add_parser("register")
    reg.add_argument("path")
    reg.add_argument("--id", required=True)
    args = argument.parse_args()
    if args.command == "sync":
        sync()
    else:
        register(args.path, args.id)
