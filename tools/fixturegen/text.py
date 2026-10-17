"""Clause 9 font dictionaries and text vectors, independently authored.

All expectations derive from explicit code/name/width tuples below and the
ISO 32000-2 mapping rules. No product executable is imported or invoked.
"""
from __future__ import annotations

from .model import Vector
from .pdf import document, stream

AREA = "text"


def pdf(font: bytes, objects=()) -> bytes:
    return document([
        b"<< /Type /Catalog /Pages 2 0 R >>",
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 10 10] >>",
        font, *objects,
    ])


def scalar(value) -> bytes:
    if value is None:
        return b"null"
    if isinstance(value, bool):
        return b"true" if value else b"false"
    if isinstance(value, str):
        return b"<" + value.encode().hex().encode() + b">"
    return str(value).encode()


def expected(rows) -> bytes:
    return b"[\n" + b"\n".join(b"[" + b" ".join(map(scalar, row)) + b"]" for row in rows) + b"\n]\n"


def type3(encoding: bytes, first: int, widths: list[int], matrix=b"[0.001 0 0 0.001 0 0]", extra=b"", names=()) -> bytes:
    charprocs = b" ".join(b"/" + name + b" %d 0 R" % number for name, number in names)
    return (b"<< /Type /Font /Subtype /Type3 /FontBBox [0 0 1000 1000] /FontMatrix " + matrix
            + b" /CharProcs << " + charprocs + b" >> /Resources << /ProcSet [/PDF] >> /Encoding " + encoding
            + b" /FirstChar %d /LastChar %d /Widths [" % (first, first + len(widths) - 1)
            + b" ".join(str(w).encode() for w in widths) + b"] " + extra + b" >>")


def simple_case(name, pairs, *, encoding=None, first=32, widths=None, matrix=b"[0.001 0 0 0.001 0 0]", scale=.001, unicode_map=None):
    """Pairs specify (byte, glyph name, Unicode); widths are explicit font data."""
    widths = widths if widths is not None else [500] * (max(c for c, _, _ in pairs) - first + 1)
    differences = b" ".join(str(c).encode() + b" /" + n for c, n, _ in pairs)
    base = None
    if encoding is not None:
        base = encoding.split(b"/")[2].split()[0]
        if base in (b"MacRomanEncoding", b"MacExpertEncoding", b"WinAnsiEncoding"):
            encoding = b"<< /BaseEncoding /" + base + b" /Differences [" + differences + b"] >>"
        else:
            encoding = b"<< /Differences [" + differences + b"] >>"
    else:
        encoding = b"<< /Differences [" + differences + b"] >>"
    procedures = {}
    objects = []
    for c, glyph_name, _ in pairs:
        if glyph_name in procedures:
            continue
        procedures[glyph_name] = 5 + len(objects)
        width = widths[c-first] if first <= c < first+len(widths) else 0
        objects.append(stream(b"%d 0 d0\n" % width))
    extra = b"/ToUnicode %d 0 R" % (5+len(objects)) if unicode_map else b""
    font = type3(encoding, first, widths, matrix, extra, procedures.items())
    if unicode_map:
        objects.append(stream(unicode_map))
    rows = [(c, 1, widths[c-first]*scale if first <= c < first+len(widths) else 0,
             c == 32, text, procedures[glyph_name]) for c, glyph_name, text in pairs]
    files = {".pdf": pdf(font, objects), ".input": bytes(c for c, _, _ in pairs), ".out": expected(rows)}
    if base is not None:
        files[".base"] = base
    return Vector(name, files,
                  ("text-type3", "text-simple-encoding", "text-unicode"), "Type3 character codes, advances, word spacing, Unicode and procedures")


def cmap_case(name, cmap, data, rows, *, extra=b"", parents=(), unicode=False):
    return Vector(name, {".pdf": pdf(stream(cmap, extra), parents), ".input": data, ".out": expected(rows)},
                  ("text-unicode" if unicode else "text-cmap",), "CMap code lengths, mappings and undefined-code consumption",
                  {"driver": "unicode-cmap" if unicode else "cid-cmap"})


def error_case(name, font, tag, objects=()):
    return Vector(name, {".pdf": pdf(font, objects), ".error": tag.encode() + b"\n"},
                  ("text-font-policy",), "Font loading reports malformed or unavailable capability explicitly")


def cases():
    yield simple_case("type3-differences", [(32, b"space", " "), (65, b"A", "A"), (66, b"uni00660069", "fi"), (67, b"u1F600", "😀"), (68, b"A_B.alt", "AB"), (90, b"unknown", None)],
                      first=65, widths=[600, 450, 1000, 750])
    yield simple_case("type3-matrix", [(65, b"A", "A"), (66, b"B", "B")], first=65, widths=[300, -100], matrix=b"[0.002 0.003 -0.004 0.005 2 3]", scale=.002)
    for name, base, pairs in [
        ("winansi", b"WinAnsiEncoding", [(32,b"space"," "),(65,b"A","A"),(128,b"Euro","€"),(129,b"bullet","•"),(160,b"space"," "),(173,b"hyphen","-")]),
        ("macroman", b"MacRomanEncoding", [(65,b"A","A"),(128,b"Adieresis","Ä"),(202,b"space"," "),(219,b"currency","¤")]),
        ("standard", b"StandardEncoding", [(39,b"quoteright","’"),(96,b"quoteleft","‘"),(174,b"fi","ﬁ")]),
        ("macexpert", b"MacExpertEncoding", [(32,b"space"," "),(86,b"ff","ﬀ"),(87,b"fi","ﬁ"),(88,b"fl","ﬂ")]),
        ("symbol", b"Symbol", [(65,b"Alpha","Α"),(68,b"Delta","∆"),(87,b"Omega","Ω")]),
        ("dingbats", b"ZapfDingbats", [(33,b"a1","✁"),(34,b"a2","✂"),(35,b"a202","✃")]),
        ("pdfdoc", b"PDFDocEncoding", [(9,b"uni0009","\t"),(10,b"uni000A","\n"),(13,b"uni000D","\r"),(65,b"A","A"),(128,b"bullet","•"),(160,b"Euro","€")]),
    ]:
        yield simple_case("encoding-" + name, pairs, encoding=b"<< /BaseEncoding /" + base + b" /Differences [] >>")
    unicode_map = (b"/CIDInit /ProcSet findresource begin 12 dict begin begincmap\n"
                   b"/CMapName /UnicodeMap def /CMapType 2 def\n"
                   b"1 begincodespacerange <00> <ff> endcodespacerange\n"
                   b"1 beginbfchar <41> <D83DDE00> endbfchar\n"
                   b"1 beginbfrange <42> <43> [<00660069> <00610062>] endbfrange\n"
                   b"1 beginbfrange <44> <45> <03B1> endbfrange\n"
                   b"endcmap CMapName currentdict /CMap defineresource pop end end")
    yield simple_case("tounicode-precedence", [(65,b"A","😀"),(66,b"B","fi"),(67,b"C","ab"),(68,b"D","α"),(69,b"E","β"),(70,b"F","F")], unicode_map=unicode_map)
    mixed = (b"/WMode 1 def 2 begincodespacerange <00> <7f> <8100> <82ff> endcodespacerange "
             b"1 begincidrange <20> <22> 7 endcidrange 1 begincidchar <8140> 500 endcidchar "
             b"1 beginnotdefrange <8100> <82ff> 12 endnotdefrange 1 beginnotdefchar <23> 99 endnotdefchar")
    yield cmap_case("cmap-mixed", mixed, b" !\x81@\x82\xff#", [(32,1,7,0,None),(33,1,8,0,None),(0x8140,2,500,12,None),(0x82ff,2,None,12,None),(35,1,None,99,None)])
    bytewise = b"1 begincodespacerange <8140> <82fc> endcodespacerange 1 begincidrange <8140> <8142> 20 endcidrange"
    yield cmap_case("cmap-bytewise", bytewise, b"\x81@\x82\x10\x81", [(0x8140,2,20,0,None),(0x8210,2,None,0,None),(0x81,1,None,0,None)])
    yield cmap_case("cmap-usecmap", b"/Identity-V usecmap 1 begincidchar <0041> 9 endcidchar", b"\0A\0B", [(65,2,9,0,None),(66,2,66,0,None)], extra=b"/UseCMap /Identity-V")
    parent = b"1 begincodespacerange <00> <ff> endcodespacerange 1 begincidrange <20> <22> 40 endcidrange"
    yield cmap_case("cmap-parent-stream", b"/Parent usecmap 1 begincidchar <21> 90 endcidchar", b" !\"", [(32,1,40,0,None),(33,1,90,0,None),(34,1,42,0,None)], extra=b"/UseCMap 5 0 R", parents=[stream(parent,b"/CMapName /Parent")])
    yield cmap_case("unicode-parent", b"1 beginbfchar <41> <005A> endbfchar", b"AB", [(65,1,None,0,"Z"),(66,1,None,0,"B")], extra=b"/UseCMap 5 0 R", parents=[stream(b"1 begincodespacerange <00> <ff> endcodespacerange 1 beginbfrange <41> <42> <0041> endbfrange")], unicode=True)
    metrics = b"<< /DW 900 /W [10 [400 500] 10 12 700 20 21 600] /DW2 [850 -950] /W2 [10 [-800 200 700] 10 12 -900 300 750 20 21 -1100 310 780] >>"
    rows = [(9,.9,-.95,.45,.85),(10,.4,-.8,.2,.7),(11,.5,-.9,.3,.75),(12,.7,-.9,.3,.75),(20,.6,-1.1,.31,.78),(22,.9,-.95,.45,.85)]
    yield Vector("cid-metrics", {".pdf": pdf(metrics), ".out": expected(rows)}, ("text-cid-metrics",), "W and W2 list/range formats, first-wins overlap and defaults", {"driver": "metrics"})
    yield Vector("cid-default-metrics", {".pdf": pdf(b"<< >>"), ".out": expected([(0,1,-1,.5,.88),(65535,1,-1,.5,.88)])}, ("text-cid-metrics",), "Default CID widths and vertical metrics", {"driver": "metrics"})
    for subtype in [b"Type1", b"MMType1", b"TrueType"]:
        yield error_case("nonembedded-" + subtype.decode().lower(), b"<< /Subtype /"+subtype+b" /BaseFont /Helvetica >>", "font.non-embedded")
    yield error_case("predefined-cmap", b"<< /Subtype /Type0 /Encoding /UniJIS-UTF16-H /DescendantFonts [<< /Subtype /CIDFontType0 >>] >>", "font.predefined-cmap")
    yield error_case("differences-overflow", type3(b"<< /Differences [255 /A /B] >>", 65, [500]), "syntax")
    yield error_case("width-count", type3(b"<< /Differences [65 /A] >>", 65, [500]).replace(b"/LastChar 65", b"/LastChar 66"), "syntax")
    yield error_case("unicode-surrogate", type3(b"<< /Differences [65 /A] >>", 65, [500], extra=b"/ToUnicode 5 0 R"), "syntax", [stream(b"1 begincodespacerange <00> <ff> endcodespacerange 1 beginbfchar <41> <D800> endbfchar")])
    yield error_case("unicode-cycle", type3(b"<< /Differences [65 /A] >>", 65, [500], extra=b"/ToUnicode 5 0 R"), "cycle", [stream(b"", b"/UseCMap 5 0 R")])
    yield error_case("unicode-range-overflow", type3(b"<< /Differences [65 /A] >>", 65, [500], extra=b"/ToUnicode 5 0 R"), "syntax", [stream(b"1 begincodespacerange <00> <ff> endcodespacerange 1 beginbfrange <41> <42> <00ff> endbfrange")])
    blank_font = type3(b"<< /Differences [65 /A 66 /.notdef] >>", 65, [500, 600], names=[(b".notdef", 5)])
    yield Vector("type3-undefined", {".pdf": pdf(blank_font, [stream(b"600 0 d0\n")]), ".input": b"\0AB", ".out": expected([(0,1,0,False,None,None),(65,1,.5,False,"A",None),(66,1,.6,False,None,5)])}, ("text-type3",), "An undefined Type3 glyph has no implicit default procedure")
    for name, contents in [
        ("unicode-odd-destination", b"1 begincodespacerange <00> <ff> endcodespacerange 1 beginbfchar <41> <00> endbfchar"),
        ("unicode-two-byte", b"1 begincodespacerange <0000> <ffff> endcodespacerange 1 beginbfchar <0041> <0041> endbfchar"),
        ("unicode-prefix-overlap", b"2 begincodespacerange <00> <ff> <8100> <81ff> endcodespacerange"),
        ("unicode-range-array-length", b"1 begincodespacerange <00> <ff> endcodespacerange 1 beginbfrange <41> <42> [<0041>] endbfrange"),
        ("unicode-block-count", b"1 begincodespacerange <00> <ff> endcodespacerange 2 beginbfchar <41> <0041> endbfchar"),
    ]:
        yield error_case(name, type3(b"<< /Differences [65 /A] >>", 65, [500], extra=b"/ToUnicode 5 0 R"), "syntax", [stream(contents)])
    yield error_case("unicode-usecmap-mismatch", type3(b"<< /Differences [65 /A] >>", 65, [500], extra=b"/ToUnicode 5 0 R"), "syntax", [stream(b"/Identity-V usecmap", b"/UseCMap /Identity-H")])
    for name, dictionary in [
        ("cid-width-reversed", b"<< /W [20 19 500] >>"),
        ("cid-width-overflow", b"<< /W [65535 [500 600]] >>"),
        ("cid-w2-truncated", b"<< /W2 [1 [-1000 200]] >>"),
        ("cid-range-truncated", b"<< /W2 [1 2 -1000 200] >>"),
        ("cid-dw2-length", b"<< /DW2 [880] >>"),
    ]:
        yield Vector(name, {".pdf": pdf(dictionary), ".error": b"syntax\n"}, ("text-cid-metrics",), "Malformed CID metric arrays fail without invented widths", {"driver": "metrics-error"})
    yield Vector("cid-to-gid", {".pdf": pdf(b"<< /Subtype /CIDFontType2 /CIDToGIDMap 5 0 R >>", [stream(bytes.fromhex("000000080100ffff"))]), ".out": expected([(0,0),(1,8),(2,256),(3,65535),(4,0),(65535,0)])}, ("text-cid-to-gid",), "CIDToGIDMap is big-endian and absent entries select glyph zero", {"driver": "cid-to-gid"})
    yield Vector("simple-missing-width", {".pdf": pdf(b"<< /Subtype /Type1 /FirstChar 65 /LastChar 66 /Widths [500 -200] /FontDescriptor << /MissingWidth 700 >> >>"), ".out": expected([(32,.7),(65,.5),(66,-.2),(67,.7)])}, ("text-simple-widths",), "Simple font widths override MissingWidth within the declared interval", {"driver": "simple-metrics"})
    yield simple_case("tounicode-empty", [(65,b"A","")], first=65, widths=[500], unicode_map=b"1 begincodespacerange <00> <ff> endcodespacerange 1 beginbfchar <41> <> endbfchar")
    yield cmap_case("cmap-indirect-name", b"/Identity-V usecmap", b"\0A", [(65,2,65,0,None)], extra=b"/UseCMap 5 0 R", parents=[b"/Identity-V"])
    # Python codecs are independent of the engine's name/Unicode tables.
    # Annex D differs from Windows and modern Mac OS at these explicit slots.
    for name, codec, base in [("winansi", "cp1252", b"WinAnsiEncoding"), ("macroman", "mac_roman", b"MacRomanEncoding")]:
        values = {}
        for code in range(32,256):
            try:
                values[code] = bytes([code]).decode(codec)
            except UnicodeDecodeError:
                values[code] = "•"
        if name == "winansi":
            values.update({127:"•", 160:" ", 173:"-"})
        else:
            for code in [127,173,176,178,179,182,183,184,185,186,189,195,197,198,215,240]:
                values.pop(code, None)
            values.update({202:" ",219:"¤"})
        yield Vector("encoding-table-"+name, {".base": base, ".input": bytes(range(256)), ".out": expected([(code,1,0,code==32,values.get(code),None) for code in range(256)])}, ("text-simple-encoding",), "All 256 base-encoding slots from independent Python codecs with Annex D corrections", {"driver":"base-encoding"})
