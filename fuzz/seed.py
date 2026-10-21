"""Builds libFuzzer seed corpora in fuzz/corpus/<target>/ from local data.

Seeds come from the generated fixtures and vectors in corpus/generated/,
found through corpus/manifests/, in the input layouts the drivers in
crates/pagura-fuzz/src/ document; from two review cases whose stream
/Length refers to its own object or to another stream's; from a few content
streams that show text on the fixed page of the content and extraction
targets; and, when present, from the pinned out-of-tree corpora. Files are
only read and copied, never executed.
"""
import argparse
import hashlib
import json
import os
import re
import shutil
import struct
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "tools"))
from fixturegen.pdf import document  # noqa: E402  (standard library only)

CORPUS = Path(__file__).resolve().parent / "corpus"
MAX_SEED = 256 * 1024
PER_SOURCE = 300
# Must match crates/pagura-fuzz/src/wrapper.rs.
SEPARATOR = b"\n%%%%\n"
FIRST_OBJECT = 3
MAX_OBJECTS = 32
HOST_TILE = bytes([1, 0, 15, 0, 0, 8, 8])  # protocol 1.1, repair, page 0, scale 1, 8 x 8 at the origin
# Must match crates/pagura-fuzz/src/host.rs: flag bit 3 adds the protocol 1.2 reading
# requests, bit 4 sends them in a v1 or v1.1 session instead.
HOST_READING = 8
HOST_OLDER = 16
# Content streams on the fixed page of crates/pagura-fuzz/src/content.rs, whose Type 3 font
# /F shows code 97 as `a`: words and lines, a TJ gap, replacement text, an
# Alt word break, a hyphen at a line break, reversed and hidden text. The
# last line, a comment, is a search query (crates/pagura-fuzz/src/extraction.rs).
EXTRACTION_SEEDS = [
    b"BT /F 6 Tf 2 24 Td (aa aaa) Tj 0 -8 Td [(a) -400 (aa)] TJ ET\n%A A",
    b"BT /F 6 Tf 2 24 Td /Span << /ActualText <FEFF0053007400720061007300DF0065> >> BDC (aaa) Tj EMC "
    b"/Span << /Alt (two words) >> BDC (a) Tj EMC ET\n%strasse",
    b"BT /F 6 Tf 2 24 Td /Span << /ActualText (con-) >> BDC (a) Tj EMC 0 -7 Td (aa) Tj ET\n%conaa",
    b"/ReversedChars BMC BT /F 6 Tf 2 16 Td (aaa) Tj ET EMC /OC /Off BDC BT /F 6 Tf 2 8 Td (a) Tj ET EMC\n%aa",
]


def add(target: str, data: bytes) -> None:
    if 0 < len(data) <= MAX_SEED:
        directory = CORPUS / target
        directory.mkdir(parents=True, exist_ok=True)
        (directory / hashlib.sha256(data).hexdigest()[:20]).write_bytes(data)


def add_structured(target: str, prefix: bytes, seed: bytes | None) -> None:
    """Adds a structured seed after its selector bytes, if it could be joined."""
    if seed is not None:
        add(target, prefix + seed)


def cases(area: str) -> list[dict]:
    return json.loads((ROOT / "corpus/manifests" / f"{area}.json").read_text())["cases"]


def read(case: dict, suffix: str) -> bytes | None:
    if suffix == ".pdf" and case["kind"] == "pdf":
        return (ROOT / case["path"]).read_bytes()
    entry = case.get("files", {}).get(suffix)
    return (ROOT / entry["path"]).read_bytes() if entry else None


def require(case: dict, suffix: str) -> bytes:
    data = read(case, suffix)
    if data is None:
        raise SystemExit(f"{case['id']}: generated case lacks {suffix}")
    return data


def params(case: dict) -> dict[str, str]:
    text = (read(case, ".params") or b"").decode()
    return dict(line.split("=", 1) for line in text.splitlines() if "=" in line)


def manifest_pdfs(areas) -> list[bytes]:
    found = []
    for area in areas:
        for case in cases(area):
            data = read(case, ".pdf")
            if data is not None:
                found.append(data)
    return found


def joined(*parts: bytes) -> bytes | None:
    """A structured input, unless a part already contains the separator."""
    if any(SEPARATOR in part for part in parts):
        return None
    return SEPARATOR.join(parts)


def objects(pdf: bytes) -> dict[int, bytes]:
    """Object bodies of a generated fixture, as wrapper parts: a stream is
    written `<< entries >>` + `stream` + data, without its /Length."""
    found = {}
    for match in re.finditer(rb"(?m)^(\d+) 0 obj\n", pdf):
        start = match.end()
        end = pdf.find(b"\nendobj\n", start)
        at = pdf.find(b"stream\n", start)
        if 0 <= at < end or (end < 0 <= at):
            head = pdf[start:at]
            length = re.search(rb"/Length (\d+)", head)
            if not length:
                continue
            data = pdf[at + 7:at + 7 + int(length.group(1))]
            found[int(match.group(1))] = head.replace(length.group(0), b"", 1) + b"stream\n" + data
        elif end >= 0:
            found[int(match.group(1))] = pdf[start:end]
    return found


def wrapper_parts(pdf: bytes) -> list[bytes] | None:
    """Objects 3, 4, … of a fixture, which the wrappers renumber identically."""
    found = objects(pdf)
    numbers = [n for n in sorted(found) if n >= FIRST_OBJECT]
    if not numbers or numbers != list(range(FIRST_OBJECT, FIRST_OBJECT + len(numbers))):
        return None
    return [found[n] for n in numbers[:MAX_OBJECTS]]


def stream_part(entries: bytes, data: bytes) -> bytes:
    return b"<< " + entries + b" >>\nstream\n" + data


def review_cases() -> list[bytes]:
    """Indirect /Length values that refer back to a stream being parsed
    (ISO 32000-2 clause 7.3.8.2): its own object, and two streams whose
    lengths refer to each other."""
    page = b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 64 64] /Resources << >> /Contents %s >>"
    content = b"0 0 1 rg 8 8 48 32 re f"
    pages = b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>"
    catalog = b"<< /Type /Catalog /Pages 2 0 R >>"
    itself = document([catalog, pages, page % b"4 0 R",
                       b"<< /Length 4 0 R >>\nstream\n" + content + b"\nendstream"])
    mutual = document([catalog, pages, page % b"[4 0 R 5 0 R]",
                       b"<< /Length 5 0 R >>\nstream\n" + content + b"\nendstream",
                       b"<< /Length 4 0 R >>\nstream\n" + content + b"\nendstream"])
    return [itself, mutual]


def content_streams(pdf: bytes) -> list[bytes]:
    """The unfiltered content streams of a fixture's pages."""
    found = objects(pdf)
    streams = []
    for body in found.values():
        if re.search(rb"/Type\s*/Page\b", body) and b"stream\n" not in body:
            contents = re.search(rb"/Contents\s*(\[[^\]]*\]|\d+ 0 R)", body)
            for number in re.findall(rb"(\d+) 0 R", contents.group(1) if contents else b""):
                part = found.get(int(number), b"")
                head, _, data = part.partition(b"stream\n")
                if data and b"/Filter" not in head:
                    streams.append(data)
    return streams


def jpeg_frame(data: bytes) -> tuple[int, int, int] | None:
    """Width, height and components of the first JPEG frame header (T.81 B.2.2)."""
    at = 2
    while at + 4 <= len(data) and data[at] == 0xFF:
        marker, length = data[at + 1], struct.unpack(">H", data[at + 2:at + 4])[0]
        if marker in (0xC0, 0xC1, 0xC2, 0xC3, 0xC5, 0xC6, 0xC7, 0xC9, 0xCA, 0xCB, 0xCD, 0xCE, 0xCF):
            if at + 10 > len(data):
                return None
            height, width = struct.unpack(">HH", data[at + 5:at + 9])
            return width, height, data[at + 9]
        at += 2 + length
    return None


def jpx_size(data: bytes) -> tuple[int, int, int] | None:
    """Image size and component count from the SIZ marker segment (T.800 A.5.1)."""
    at = data.find(b"\xff\x51")
    if at < 0 or at + 40 > len(data):
        return None
    x, y, x0, y0 = struct.unpack(">IIII", data[at + 6:at + 22])
    components = struct.unpack(">H", data[at + 38:at + 40])[0]
    return (x - x0, y - y0, components) if x > x0 and y > y0 else None


def image_object(entries: bytes, data: bytes, extra: tuple[bytes, ...] = ()) -> bytes | None:
    return joined(stream_part(b"/Type /XObject /Subtype /Image " + entries, data), *extra)


DEVICE_SPACES = {1: b"/DeviceGray", 3: b"/DeviceRGB", 4: b"/DeviceCMYK"}


def image_seeds() -> None:
    for case in cases("image"):
        p = params(case)
        data = read(case, ".in")
        pdf = read(case, ".pdf")
        if p.get("kind") == "dct" and data:
            transform = {"0": 1, "1": 2, "2": 3}.get(p.get("transform", ""), 0)
            add("image", bytes([transform << 3]) + data)
            frame = jpeg_frame(data)
            if frame and frame[2] in DEVICE_SPACES:
                entries = b"/Width %d /Height %d /BitsPerComponent 8 /ColorSpace %s /Filter /DCTDecode" % (
                    frame[0], frame[1], DEVICE_SPACES[frame[2]])
                add_structured("image", b"\x04", image_object(entries, data))
        elif p.get("kind") == "ccitt" and data:
            k, width, height = int(p.get("k", 0)), int(p.get("width", 1728)), int(p.get("height", 0))
            flags = (int(p.get("eol", 0)) | int(p.get("align", 0)) << 1 | int(p.get("eob", 1)) << 2
                     | int(p.get("black", 0)) << 3)
            damaged = int(p.get("damaged", 0))
            if -128 <= k < 128 and width < 65536 and height < 65536:
                add("image", b"\x01" + struct.pack("<bBHHB", k, flags, width, height, damaged) + data)
            if height:
                parms = (b"/K %d /Columns %d /Rows %d /EndOfLine %s /EncodedByteAlign %s /EndOfBlock %s "
                         b"/BlackIs1 %s /DamagedRowsBeforeError %d") % (
                    k, width, height, *(b"true" if flags & bit else b"false" for bit in (1, 2, 4, 8)), damaged)
                entries = (b"/Width %d /Height %d /BitsPerComponent 1 /ColorSpace /DeviceGray "
                           b"/Filter /CCITTFaxDecode /DecodeParms << %s >>") % (width, height, parms)
                add_structured("image", b"\x04", image_object(entries, data))
        elif p.get("kind") == "inline" and data:
            add_structured("image", b"\x05", joined(read(case, ".dict") or b"", data))
        elif pdf:
            parts = wrapper_parts(pdf)
            if parts:
                add_structured("image", b"\x04", joined(*parts))
    for case in cases("jbig2"):
        p, data, globals_ = params(case), read(case, ".jb2") or b"", read(case, ".globals") or b""
        if globals_ and len(globals_) < 65536:
            add("image", b"\x0a" + struct.pack("<H", len(globals_)) + globals_ + data)
        else:
            add("image", b"\x02" + data)
        if int(p.get("width", 0)) > 0 and int(p.get("height", 0)) > 0:
            entries = b"/Width %s /Height %s /BitsPerComponent 1 /ColorSpace /DeviceGray /Filter /JBIG2Decode" % (
                p["width"].encode(), p["height"].encode())
            extra = ()
            if globals_:
                entries += b" /DecodeParms << /JBIG2Globals 4 0 R >>"
                extra = (stream_part(b"", globals_),)
            add_structured("image", b"\x04", image_object(entries, data, extra))
    for case in cases("jpx"):
        data = read(case, ".jpx") or b""
        add("image", b"\x03" + data)
        size = jpx_size(data)
        if size:
            space = b" /ColorSpace " + DEVICE_SPACES[size[2]] if size[2] in DEVICE_SPACES else b""
            entries = b"/Width %d /Height %d /Filter /JPXDecode" % size[:2] + space
            add_structured("image", b"\x04", image_object(entries, data))


FILLER_PAGE = b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 10 10] >>"
TO_UNICODE = stream_part(b"", b"/CIDInit /ProcSet findresource begin 12 dict begin begincmap "
                         b"1 begincodespacerange <00> <ff> endcodespacerange "
                         b"1 beginbfrange <20> <7e> <0020> endbfrange endcmap end end")
CID_SYSTEM = b"/CIDSystemInfo << /Registry (Adobe) /Ordering (Identity) /Supplement 0 >>"


def descriptor(flags: int, key: bytes, program: int) -> bytes:
    return (b"<< /Type /FontDescriptor /FontName /Fuzz /Flags %d /FontBBox [0 0 1000 1000] /ItalicAngle 0 "
            b"/Ascent 800 /Descent -200 /CapHeight 700 /StemV 80 /%s %d 0 R >>") % (flags, key, program)


def embedded_fonts(program: bytes, format_: str, base: int = 4, cmap: bytes = b"/Identity-H",
                   to_unicode: bytes | None = None) -> list[tuple[bool, list[bytes], bytes]]:
    """Simple and composite font dictionaries embedding `program` as
    objects `base`, `base` + 1, …, each with whether it is composite and a
    string to decode (clauses 9.6-9.9)."""
    font_descriptor, font_file_number, unicode, descendant_number = base + 1, base + 2, base + 3, base + 4
    to_unicode = to_unicode or b"%d 0 R" % unicode
    key, entries, simple, cid = {
        "truetype": (b"FontFile2", b"/Length1 %d" % len(program), b"TrueType", b"CIDFontType2"),
        "opentype": (b"FontFile3", b"/Subtype /OpenType", b"TrueType", b"CIDFontType2"),
        "cff": (b"FontFile3", b"/Subtype /Type1C", b"Type1", b"CIDFontType0"),
        "type1": (b"FontFile", b"/Length1 %d /Length2 0 /Length3 0" % len(program), b"Type1", None),
    }[format_]
    font_file = stream_part(entries, program)
    widths = b"[" + b" ".join([b"500"] * 95) + b"]"
    found = []
    for flags in (4, 32):
        font = (b"<< /Type /Font /Subtype /%s /BaseFont /Fuzz /FirstChar 32 /LastChar 126 /Widths %s "
                b"/FontDescriptor %d 0 R /Encoding /WinAnsiEncoding /ToUnicode %s >>") % (
            simple, widths, font_descriptor, to_unicode)
        found.append((False, [font, descriptor(flags, key, font_file_number), font_file, TO_UNICODE], b" !AZaz~"))
    if cid:
        if format_ == "cff":
            font_file = stream_part(b"/Subtype /CIDFontType0C", program)
        font = (b"<< /Type /Font /Subtype /Type0 /BaseFont /Fuzz /Encoding %s /DescendantFonts [%d 0 R] "
                b"/ToUnicode %s >>") % (cmap, descendant_number, to_unicode)
        descendant = (b"<< /Type /Font /Subtype /%s /BaseFont /Fuzz %s /FontDescriptor %d 0 R /DW 1000 "
                      b"/W [0 [500 600 700] 3 9 400]%s >>") % (
            cid, CID_SYSTEM, font_descriptor, b" /CIDToGIDMap /Identity" if cid == b"CIDFontType2" else b"")
        found.append((True, [font, descriptor(4, key, font_file_number), font_file, TO_UNICODE, descendant],
                      b"\x00\x00\x00\x01\x00\x02\x00\x03\x00\x41"))
    return found


def text_seeds() -> None:
    for case in cases("text"):
        pdf = read(case, ".pdf")
        parts = pdf and wrapper_parts(pdf)
        if parts:
            add_structured("text", b"\x01", joined(read(case, ".input") or b"\x00\x41\x42 ", *parts))
    for case in cases("font"):
        for _, parts, string in embedded_fonts(require(case, ".font"), case["format"]):
            add_structured("text", b"\x01", joined(string, FILLER_PAGE, *parts))
    # The CMap and ToUnicode vectors, used by an embedded TrueType font that
    # follows the fixture's own objects.
    truetype = next(require(case, ".font") for case in cases("font") if case["id"] == "font/truetype-cmap4")
    for case in cases("text"):
        pdf = read(case, ".pdf")
        parts = pdf and wrapper_parts(pdf)
        driver = case.get("driver")
        base = FIRST_OBJECT + len(parts or [])
        if not parts or driver not in ("cid-cmap", "unicode-cmap") or base - FIRST_OBJECT > 7:
            continue
        if driver == "cid-cmap":
            fonts = [f for f in embedded_fonts(truetype, "truetype", base, cmap=b"4 0 R") if f[0]]
        else:
            fonts = [f for f in embedded_fonts(truetype, "truetype", base, to_unicode=b"4 0 R") if not f[0]]
        for _, font_parts, string in fonts:
            seed = joined(read(case, ".input") or string, *parts, *font_parts)
            add_structured("text", bytes([base - FIRST_OBJECT]), seed)


def host_reading(flags: int, query: bytes, pdf: bytes) -> bytes:
    """A host seed with the reading requests: a one-page search window from
    page 0, offset 0, at most four matches, case-insensitive, for `query`."""
    query = query[:255]
    return bytes([flags, *HOST_TILE[1:], 1, 0, 4, 0, len(query)]) + query + pdf


def text_page_cases() -> list[tuple[bytes, bytes]]:
    """The text extraction fixtures, each with a query: the first one its
    case searches for, else the first word of its expected text."""
    found = []
    for case in cases("text"):
        pdf = read(case, ".pdf")
        if case.get("driver") != "text-page" or pdf is None:
            continue
        queries = re.findall(rb"\[<([0-9A-Fa-f]*)>", read(case, ".queries") or b"")
        words = (read(case, ".text") or b"").split()
        query = bytes.fromhex(queries[0].decode()) if queries else (words[0] if words else b"a")
        found.append((pdf, query))
    return found


def reading_seeds() -> None:
    """Text extraction on the fixed page, and the host's reading requests
    over the text and navigation fixtures, in all three protocol versions."""
    for data in EXTRACTION_SEEDS:
        add("extraction", data)
    for pdf, query in text_page_cases():
        if b"\n" not in query and b"\r" not in query:
            for stream_data in content_streams(pdf):
                add("extraction", stream_data + b"\n%" + query)
        add("host", host_reading(HOST_TILE[0] | HOST_READING, query, pdf))
    for case in cases("document"):
        pdf = read(case, ".pdf")
        if case["id"].startswith("document/navigation-") and pdf is not None:
            for flags in (HOST_READING, HOST_READING | HOST_OLDER, HOST_TILE[0] | HOST_READING | HOST_OLDER):
                add("host", host_reading(flags, b"a", pdf))


def main() -> int:
    global CORPUS
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, default=CORPUS, help="corpus directory (default fuzz/corpus)")
    CORPUS = parser.parse_args().output
    shutil.rmtree(CORPUS, ignore_errors=True)
    generated = manifest_pdfs(["classic", "document", "content", "text", "image"])
    review = review_cases()
    for data in generated + review:
        for target in ("document", "render", "syntax"):
            add(target, data)
    for data in manifest_pdfs(["classic", "document"]) + review:
        add("navigation", data)
    cache = Path(os.environ.get("PAGURA_CORPORA_DIR", "~/.cache/pagura-corpora")).expanduser()
    for source in [cache / "upstream", cache / "safedocs-unsafe/pdf", cache / "safedocs-cc-main-2021-31/pdf"]:
        if source.exists():
            files = sorted(p for p in source.rglob("*") if p.is_file() and p.suffix.lower() == ".pdf")
            for path in [p for p in files if p.stat().st_size <= MAX_SEED][:PER_SOURCE]:
                data = path.read_bytes()
                for target in ("document", "render", "syntax", "navigation"):
                    add(target, data)
    for data in manifest_pdfs(["classic", "document", "content"]) + review:
        add("host", HOST_TILE + data)
        for stream_data in content_streams(data):
            add("content", stream_data)
            add("extraction", stream_data)
    for case in cases("codec"):
        data = require(case, ".in")
        for selector in range(8):
            add("filters", bytes([selector]) + data)
    for case in cases("font"):
        data = require(case, ".font")
        for selector in range(4):
            add("font", bytes([selector]) + data)
    image_seeds()
    text_seeds()
    reading_seeds()
    counts = {target.name: len(list(target.iterdir())) for target in sorted(CORPUS.iterdir())}
    print(json.dumps(counts))
    return 0


if __name__ == "__main__":
    sys.exit(main())
