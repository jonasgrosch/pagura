"""Acquire official references into a private cache; keep a provenance ledger.

    python3 tools/references.py sync              # refresh the PDF 2.0 reference index
    python3 tools/references.py fetch [ID ...]    # download the format standards below
    python3 tools/references.py register PATH --id ID

Fetched standards are freely published by their owners but not redistributable
by this project: they stay in $PAGURA_REFERENCES_DIR, never in the repository.
Searchable text goes to $PAGURA_REFERENCES_DIR/text/<ID>.txt.
"""
from __future__ import annotations

import argparse
import concurrent.futures
import hashlib
import json
import os
import subprocess
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


# Non-PDF format standards the codecs, fonts and crypto are verified against.
# (ID, title, official download URL, normative-index record it fulfils, users)
STANDARDS = [
    ("itu-t88-2000", "ITU-T T.88 (02/2000): JBIG2 bi-level image compression",
     "https://www.itu.int/rec/dologin_pub.asp?lang=e&id=T-REC-T.88-200002-S!!PDF-E&type=items", None, "jbig2"),
    ("itu-t800-2019", "ITU-T T.800 (06/2019): JPEG 2000 core coding system",
     "https://www.itu.int/rec/dologin_pub.asp?lang=e&id=T-REC-T.800-201906-S!!PDF-E&type=items", None, "jpx"),
    ("iec-sycc-amendment-preview", "IEC 61966-2-1:1999/AMD1:2003 publisher preview, Annex F sYCC",
     "https://preview.sist.si/sist-preview/10795/ae461684569b40bbbb2d9a22b1047f05/IEC-61966-2-1-1999-AMD1-2003.pdf", None, "color, paint_jpx"),
    ("itu-t81-1992", "ITU-T T.81 (09/1992): JPEG digital compression of still images",
     "https://www.w3.org/Graphics/JPEG/itu-t81.pdf", None, "image"),
    ("itu-t4-2003", "ITU-T T.4 (07/2003): Group 3 facsimile coding",
     "https://www.itu.int/rec/dologin_pub.asp?lang=e&id=T-REC-T.4-200307-I!!PDF-E&type=items", "ref-c9db1781d544", "image"),
    ("itu-t6-1988", "ITU-T T.6 (11/1988): Group 4 facsimile coding",
     "https://www.itu.int/rec/dologin_pub.asp?lang=e&id=T-REC-T.6-198811-I!!PDF-E&type=items", "ref-334231984265", "image"),
    ("rfc1950", "RFC 1950: ZLIB compressed data format", "https://www.rfc-editor.org/rfc/rfc1950.txt", "ref-cbcb41294671", "codec"),
    ("rfc1951", "RFC 1951: DEFLATE compressed data format", "https://www.rfc-editor.org/rfc/rfc1951.txt", "ref-e25089654d67", "codec"),
    ("rfc1321", "RFC 1321: The MD5 message-digest algorithm", "https://www.rfc-editor.org/rfc/rfc1321.txt", "ref-3dc7af1c3b24", "crypto"),
    ("nist-fips180-4", "FIPS 180-4: Secure Hash Standard",
     "https://nvlpubs.nist.gov/nistpubs/FIPS/NIST.FIPS.180-4.pdf", "ref-5b9a7774758f", "crypto"),
    ("nist-fips197-upd1", "FIPS 197 (2023 update): Advanced Encryption Standard",
     "https://nvlpubs.nist.gov/nistpubs/FIPS/NIST.FIPS.197-upd1.pdf", "ref-55dc3d1c068a", "crypto"),
    ("w3c-png-2003", "PNG (Second Edition), W3C Recommendation 2003-11-10",
     "https://www.w3.org/TR/2003/REC-PNG-20031110/", None, "codec"),
    ("tiff6", "TIFF Revision 6.0", "https://www.itu.int/itudoc/itu-t/com16/tiff-fx/docs/tiff6.pdf", "ref-3ad2fe4b0863", "codec"),
    ("adobe-type1", "Adobe Type 1 Font Format",
     "https://adobe-type-tools.github.io/font-tech-notes/pdfs/T1_SPEC.pdf", None, "font"),
    ("adobe-tn5176-cff", "Adobe Technical Note #5176: The Compact Font Format Specification",
     "https://adobe-type-tools.github.io/font-tech-notes/pdfs/5176.CFF.pdf", "ref-8527494a2d0e", "font"),
    ("adobe-tn5177-type2", "Adobe Technical Note #5177: The Type 2 Charstring Format",
     "https://adobe-type-tools.github.io/font-tech-notes/pdfs/5177.Type2.pdf", "ref-a27a03167d23", "font"),
    ("adobe-tn5014-cidfont", "Adobe Technical Note #5014: Adobe CMap and CIDFont Files Specification",
     "https://adobe-type-tools.github.io/font-tech-notes/pdfs/5014.CIDFont_Spec.pdf", "ref-66573d1d29aa", "text, font"),
    ("rfc3454", "RFC 3454: Preparation of Internationalized Strings (stringprep)",
     "https://www.rfc-editor.org/rfc/rfc3454.txt", None, "crypto"),
    ("rfc4013", "RFC 4013: SASLprep, the stringprep profile for user names and passwords",
     "https://www.rfc-editor.org/rfc/rfc4013.txt", None, "crypto"),
    ("rfc6229", "RFC 6229: Test Vectors for the Stream Cipher RC4",
     "https://www.rfc-editor.org/rfc/rfc6229.txt", None, "crypto"),
    ("nist-sp800-38a", "NIST SP 800-38A: Block Cipher Modes of Operation (CBC)",
     "https://nvlpubs.nist.gov/nistpubs/Legacy/SP/nistspecialpublication800-38a.pdf", None, "crypto"),
    ("adobe-glyph-list", "Adobe Glyph List (glyphlist.txt)",
     "https://raw.githubusercontent.com/adobe-type-tools/agl-aglfn/master/glyphlist.txt", "ref-7f1ab0d524b6", "text"),
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
        extension = (".pdf" if data.startswith(b"%PDF-") else ".html" if "html" in mime
                     else ".txt" if mime == "text/plain" else ".bin")
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
    """Refreshing web metadata must never forget a supplied or fetched document."""
    standards = [r for r in previous["sources"] if r.get("kind") == "format-standard"]
    for record in standards:
        if hashlib.sha256(cached_path(record["cache_path"]).read_bytes()).hexdigest() != record["sha256"]:
            raise ValueError(f"{record['id']}: cached standard changed; fetch it again explicitly")
    acquired = {r["id"]: r["acquired_as"] for r in previous["sources"] if r.get("acquired_as")}
    for record in records:
        if record["id"] in acquired and record["status"] == "indexed-only":
            record.update(status="acquired-separately", acquired_as=acquired[record["id"]])
    records = [r for r in records if r.get("kind") != "format-standard"] + standards
    supplied = {r["id"]: r for r in previous["sources"] if r["status"] == "locally-supplied"}
    for record in supplied.values():
        path = cached_path(record["cache_path"]).resolve()
        if not path.is_relative_to((ROOT / "references/private").resolve()):
            raise ValueError("Local reference escapes private directory")
        if hashlib.sha256(path.read_bytes()).hexdigest() != record["sha256"]:
            raise ValueError("Locally supplied reference changed; register it again explicitly")
    merged = [supplied.pop(r["id"], r) for r in records]
    return merged + list(supplied.values())


def text_of(path: Path, data: bytes) -> str | None:
    """Searchable text: pdftotext for PDFs when available, tags stripped from HTML."""
    if data.startswith(b"%PDF-"):
        try:
            result = subprocess.run(["pdftotext", "-layout", "-enc", "UTF-8", str(path), "-"],
                                    capture_output=True, timeout=300, check=True)
        except (OSError, subprocess.SubprocessError):
            return None
        return result.stdout.decode("utf-8", errors="replace")
    if path.suffix == ".html":
        parser = TextParser()
        parser.feed(data.decode("utf-8", errors="replace"))
        return parser.text()
    return data.decode("utf-8", errors="replace")


class TextParser(HTMLParser):
    """Visible text of an HTML document, one block per line."""

    def __init__(self):
        super().__init__()
        self.parts, self.skip = [], 0

    def handle_starttag(self, tag, attrs):
        if tag in ("script", "style"):
            self.skip += 1
        elif tag in ("p", "div", "br", "tr", "li", "h1", "h2", "h3", "h4", "pre", "table"):
            self.parts.append("\n")

    def handle_endtag(self, tag):
        if tag in ("script", "style") and self.skip:
            self.skip -= 1

    def handle_data(self, data):
        if not self.skip:
            self.parts.append(data)

    def text(self) -> str:
        return "".join(self.parts)


def fetch(idents):
    """Download the selected format standards, verify them and record them."""
    unknown = set(idents) - {s[0] for s in STANDARDS}
    if unknown:
        raise SystemExit(f"unknown standards: {', '.join(sorted(unknown))}")
    lock_path = ROOT / "references/lock.json"
    lock = json.loads(lock_path.read_text())
    by_id = {r["id"]: r for r in lock["sources"]}
    (CACHE / "text").mkdir(parents=True, exist_ok=True)
    for ident, title, url, index_entry, users in STANDARDS:
        if idents and ident not in idents:
            continue
        known = by_id.get(ident)
        if not idents and known and known.get("kind") == "format-standard" \
                and cached_path(known["cache_path"]).is_file() \
                and hashlib.sha256(cached_path(known["cache_path"]).read_bytes()).hexdigest() == known["sha256"]:
            print(f"{ident}: cached", flush=True)
            continue
        evidence, data = acquire(url)
        expected_pdf = not url.endswith((".txt", "/"))
        if expected_pdf != data.startswith(b"%PDF-"):
            raise SystemExit(f"{ident}: unexpected content ({evidence['mime']}, {len(data)} bytes)")
        record = {"id": ident, "title": title, "kind": "format-standard", "used_by": users,
                  "redistribution": "freely published by its owner; private cache only", **evidence}
        if ident in by_id:
            by_id[ident].clear()
            by_id[ident].update(record)
        else:
            lock["sources"].append(record)
            by_id[ident] = record
        if index_entry in by_id:
            by_id[index_entry].update(status="acquired-separately", acquired_as=ident)
        text = text_of(cached_path(record["cache_path"]), data)
        if text is not None:
            (CACHE / "text" / f"{ident}.txt").write_text(text)
        print(f"{ident}: {len(data)} bytes, sha256 {evidence['sha256'][:16]}…"
              + ("" if text is not None else " (no text extraction)"), flush=True)
    lock_path.write_text(json.dumps(lock, indent=2) + "\n")
    report(lock)


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
    get = sub.add_parser("fetch", help="download format standards (all when no ID is given)")
    get.add_argument("ids", nargs="*")
    reg = sub.add_parser("register")
    reg.add_argument("path")
    reg.add_argument("--id", required=True)
    args = argument.parse_args()
    if args.command == "sync":
        sync()
    elif args.command == "fetch":
        fetch(args.ids)
    else:
        register(args.path, args.id)
