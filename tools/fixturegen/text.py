"""Clause 9 font dictionaries and text vectors, independently authored.

All expectations derive from explicit code/name/width tuples below and the
ISO 32000-2 mapping rules. No product executable is imported or invoked.
"""
from __future__ import annotations

import struct
from functools import cache

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
    if base == b"ZapfDingbats":
        extra += b" /BaseFont /ZapfDingbats"
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
    yield from structure_cases()
    yield from extraction_cases()
    yield from tolerance_paint_cases()
    yield from tolerance_cases()
    yield from strict_substitution_cases()
    for case in original_cases():
        if case.name.startswith("unicode-") and ".error" in case.files:
            from dataclasses import replace
            files = dict(case.files)
            if case.name in ("unicode-cid-code-length","unicode-simple-source-length"):
                files[".matrix"] = expected([(.001,0,0,.001,0,0)])
                length = 2 if case.name=="unicode-cid-code-length" else 1
                data = b"\0\1" if length==2 else b"A"
                gid = 1
                fallback = None if length==2 else "A"
                out = [(1 if length==2 else 65,length,1.0 if length==2 else .6,False,fallback,gid,None,None,None)]
                repaired = [list(row) for row in out]
                if length==1: repaired[0][4]="A"
            else:
                data=b"A"
                out=[(65,1,.5,False,"A",None)]
                repaired=[list(row) for row in out]
                if case.name=="unicode-range-overflow":repaired[0][4]="ÿ"
            tag = {
                "unicode-surrogate":"font.tounicode-utf16",
                "unicode-odd-destination":"font.tounicode-utf16",
                "unicode-two-byte":"font.tounicode-simple-codes",
                "unicode-simple-source-length":"font.tounicode-simple-codes",
                "unicode-range-overflow":"font.tounicode-range",
            }.get(case.name,"font.tounicode-invalid")
            files.update({".input":data,".out":expected(repaired),".strict.error":tag.encode()+b"\n",".repair":tag.encode()+b"\n"})
            yield replace(case,files=files,requirements=tuple(dict.fromkeys((*case.requirements,"text-tounicode-recovery"))),description="Invalid ToUnicode rejects font loading in strict mode; repair mode retains drawable glyphs",extra={"driver":"unicode-fault"})
        else: yield case

def original_cases():
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
    yield from annex_cases()
    yield from primary_regression_cases()
    yield from program_cases()
    yield from truetype_fallback_cases()
    yield from glyph_name_cases()
    yield from embedding_error_cases()
    yield from bundled_cases()
    yield cmap_case("unicode-ignored-stream-metadata", b"1 begincodespacerange <00> <ff> endcodespacerange 1 beginbfchar <41> <0042> endbfchar", b"A", [(65,1,None,0,"B")], extra=b"/WMode 7 /CMapName false /CIDSystemInfo false", unicode=True)
    yield cmap_case("unicode-inherited-ignored-name", b"/Parent usecmap", b"A", [(65,1,None,0,"B")], extra=b"/UseCMap 5 0 R", parents=[stream(b"/CMapName /Parent def 1 begincodespacerange <00> <ff> endcodespacerange 1 beginbfchar <41> <0042> endbfchar", b"/CMapName false /WMode 7")], unicode=True)


# Numeric code/name assignments checked against ISO 32000-2 Tables D.2–D.6.
# Table D.2 notes 3, 5 and 6 add aliases; Table D.3 adds TAB, LF and CR.
ANNEX_ENCODINGS = {
    "StandardEncoding": (
        "32:space 33:exclam 34:quotedbl 35:numbersign 36:dollar 37:percent 38:ampersand 39:quoteright "
        "40:parenleft 41:parenright 42:asterisk 43:plus 44:comma 45:hyphen 46:period 47:slash 48:zero 49:one "
        "50:two 51:three 52:four 53:five 54:six 55:seven 56:eight 57:nine 58:colon 59:semicolon 60:less "
        "61:equal 62:greater 63:question 64:at 65:A 66:B 67:C 68:D 69:E 70:F 71:G 72:H 73:I 74:J 75:K 76:L "
        "77:M 78:N 79:O 80:P 81:Q 82:R 83:S 84:T 85:U 86:V 87:W 88:X 89:Y 90:Z 91:bracketleft 92:backslash "
        "93:bracketright 94:asciicircum 95:underscore 96:quoteleft 97:a 98:b 99:c 100:d 101:e 102:f 103:g "
        "104:h 105:i 106:j 107:k 108:l 109:m 110:n 111:o 112:p 113:q 114:r 115:s 116:t 117:u 118:v 119:w "
        "120:x 121:y 122:z 123:braceleft 124:bar 125:braceright 126:asciitilde 161:exclamdown 162:cent "
        "163:sterling 164:fraction 165:yen 166:florin 167:section 168:currency 169:quotesingle "
        "170:quotedblleft 171:guillemotleft 172:guilsinglleft 173:guilsinglright 174:fi 175:fl 177:endash "
        "178:dagger 179:daggerdbl 180:periodcentered 182:paragraph 183:bullet 184:quotesinglbase "
        "185:quotedblbase 186:quotedblright 187:guillemotright 188:ellipsis 189:perthousand 191:questiondown "
        "193:grave 194:acute 195:circumflex 196:tilde 197:macron 198:breve 199:dotaccent 200:dieresis "
        "202:ring 203:cedilla 205:hungarumlaut 206:ogonek 207:caron 208:emdash 225:AE 227:ordfeminine "
        "232:Lslash 233:Oslash 234:OE 235:ordmasculine 241:ae 245:dotlessi 248:lslash 249:oslash 250:oe "
        "251:germandbls "
    ),
    "MacRomanEncoding": (
        "32:space 33:exclam 34:quotedbl 35:numbersign 36:dollar 37:percent 38:ampersand 39:quotesingle "
        "40:parenleft 41:parenright 42:asterisk 43:plus 44:comma 45:hyphen 46:period 47:slash 48:zero 49:one "
        "50:two 51:three 52:four 53:five 54:six 55:seven 56:eight 57:nine 58:colon 59:semicolon 60:less "
        "61:equal 62:greater 63:question 64:at 65:A 66:B 67:C 68:D 69:E 70:F 71:G 72:H 73:I 74:J 75:K 76:L "
        "77:M 78:N 79:O 80:P 81:Q 82:R 83:S 84:T 85:U 86:V 87:W 88:X 89:Y 90:Z 91:bracketleft 92:backslash "
        "93:bracketright 94:asciicircum 95:underscore 96:grave 97:a 98:b 99:c 100:d 101:e 102:f 103:g 104:h "
        "105:i 106:j 107:k 108:l 109:m 110:n 111:o 112:p 113:q 114:r 115:s 116:t 117:u 118:v 119:w 120:x "
        "121:y 122:z 123:braceleft 124:bar 125:braceright 126:asciitilde 128:Adieresis 129:Aring 130:Ccedilla "
        "131:Eacute 132:Ntilde 133:Odieresis 134:Udieresis 135:aacute 136:agrave 137:acircumflex "
        "138:adieresis 139:atilde 140:aring 141:ccedilla 142:eacute 143:egrave 144:ecircumflex 145:edieresis "
        "146:iacute 147:igrave 148:icircumflex 149:idieresis 150:ntilde 151:oacute 152:ograve 153:ocircumflex "
        "154:odieresis 155:otilde 156:uacute 157:ugrave 158:ucircumflex 159:udieresis 160:dagger 161:degree "
        "162:cent 163:sterling 164:section 165:bullet 166:paragraph 167:germandbls 168:registered "
        "169:copyright 170:trademark 171:acute 172:dieresis 174:AE 175:Oslash 177:plusminus 180:yen 181:mu "
        "187:ordfeminine 188:ordmasculine 190:ae 191:oslash 192:questiondown 193:exclamdown 194:logicalnot "
        "196:florin 199:guillemotleft 200:guillemotright 201:ellipsis 202:space 203:Agrave 204:Atilde "
        "205:Otilde 206:OE 207:oe 208:endash 209:emdash 210:quotedblleft 211:quotedblright 212:quoteleft "
        "213:quoteright 214:divide 216:ydieresis 217:Ydieresis 218:fraction 219:currency 220:guilsinglleft "
        "221:guilsinglright 222:fi 223:fl 224:daggerdbl 225:periodcentered 226:quotesinglbase "
        "227:quotedblbase 228:perthousand 229:Acircumflex 230:Ecircumflex 231:Aacute 232:Edieresis 233:Egrave "
        "234:Iacute 235:Icircumflex 236:Idieresis 237:Igrave 238:Oacute 239:Ocircumflex 241:Ograve 242:Uacute "
        "243:Ucircumflex 244:Ugrave 245:dotlessi 246:circumflex 247:tilde 248:macron 249:breve 250:dotaccent "
        "251:ring 252:cedilla 253:hungarumlaut 254:ogonek 255:caron "
    ),
    "WinAnsiEncoding": (
        "32:space 33:exclam 34:quotedbl 35:numbersign 36:dollar 37:percent 38:ampersand 39:quotesingle "
        "40:parenleft 41:parenright 42:asterisk 43:plus 44:comma 45:hyphen 46:period 47:slash 48:zero 49:one "
        "50:two 51:three 52:four 53:five 54:six 55:seven 56:eight 57:nine 58:colon 59:semicolon 60:less "
        "61:equal 62:greater 63:question 64:at 65:A 66:B 67:C 68:D 69:E 70:F 71:G 72:H 73:I 74:J 75:K 76:L "
        "77:M 78:N 79:O 80:P 81:Q 82:R 83:S 84:T 85:U 86:V 87:W 88:X 89:Y 90:Z 91:bracketleft 92:backslash "
        "93:bracketright 94:asciicircum 95:underscore 96:grave 97:a 98:b 99:c 100:d 101:e 102:f 103:g 104:h "
        "105:i 106:j 107:k 108:l 109:m 110:n 111:o 112:p 113:q 114:r 115:s 116:t 117:u 118:v 119:w 120:x "
        "121:y 122:z 123:braceleft 124:bar 125:braceright 126:asciitilde 127:bullet 128:Euro 129:bullet "
        "130:quotesinglbase 131:florin 132:quotedblbase 133:ellipsis 134:dagger 135:daggerdbl 136:circumflex "
        "137:perthousand 138:Scaron 139:guilsinglleft 140:OE 141:bullet 142:Zcaron 143:bullet 144:bullet "
        "145:quoteleft 146:quoteright 147:quotedblleft 148:quotedblright 149:bullet 150:endash 151:emdash "
        "152:tilde 153:trademark 154:scaron 155:guilsinglright 156:oe 157:bullet 158:zcaron 159:Ydieresis "
        "160:space 161:exclamdown 162:cent 163:sterling 164:currency 165:yen 166:brokenbar 167:section "
        "168:dieresis 169:copyright 170:ordfeminine 171:guillemotleft 172:logicalnot 173:hyphen "
        "174:registered 175:macron 176:degree 177:plusminus 178:twosuperior 179:threesuperior 180:acute "
        "181:mu 182:paragraph 183:periodcentered 184:cedilla 185:onesuperior 186:ordmasculine "
        "187:guillemotright 188:onequarter 189:onehalf 190:threequarters 191:questiondown 192:Agrave "
        "193:Aacute 194:Acircumflex 195:Atilde 196:Adieresis 197:Aring 198:AE 199:Ccedilla 200:Egrave "
        "201:Eacute 202:Ecircumflex 203:Edieresis 204:Igrave 205:Iacute 206:Icircumflex 207:Idieresis 208:Eth "
        "209:Ntilde 210:Ograve 211:Oacute 212:Ocircumflex 213:Otilde 214:Odieresis 215:multiply 216:Oslash "
        "217:Ugrave 218:Uacute 219:Ucircumflex 220:Udieresis 221:Yacute 222:Thorn 223:germandbls 224:agrave "
        "225:aacute 226:acircumflex 227:atilde 228:adieresis 229:aring 230:ae 231:ccedilla 232:egrave "
        "233:eacute 234:ecircumflex 235:edieresis 236:igrave 237:iacute 238:icircumflex 239:idieresis 240:eth "
        "241:ntilde 242:ograve 243:oacute 244:ocircumflex 245:otilde 246:odieresis 247:divide 248:oslash "
        "249:ugrave 250:uacute 251:ucircumflex 252:udieresis 253:yacute 254:thorn 255:ydieresis "
    ),
    "PDFDocEncoding": (
        "9:uni0009 10:uni000A 13:uni000D 24:breve 25:caron 26:circumflex 27:dotaccent 28:hungarumlaut "
        "29:ogonek 30:ring 31:tilde 32:space 33:exclam 34:quotedbl 35:numbersign 36:dollar 37:percent "
        "38:ampersand 39:quotesingle 40:parenleft 41:parenright 42:asterisk 43:plus 44:comma 45:hyphen "
        "46:period 47:slash 48:zero 49:one 50:two 51:three 52:four 53:five 54:six 55:seven 56:eight 57:nine "
        "58:colon 59:semicolon 60:less 61:equal 62:greater 63:question 64:at 65:A 66:B 67:C 68:D 69:E 70:F "
        "71:G 72:H 73:I 74:J 75:K 76:L 77:M 78:N 79:O 80:P 81:Q 82:R 83:S 84:T 85:U 86:V 87:W 88:X 89:Y 90:Z "
        "91:bracketleft 92:backslash 93:bracketright 94:asciicircum 95:underscore 96:grave 97:a 98:b 99:c "
        "100:d 101:e 102:f 103:g 104:h 105:i 106:j 107:k 108:l 109:m 110:n 111:o 112:p 113:q 114:r 115:s "
        "116:t 117:u 118:v 119:w 120:x 121:y 122:z 123:braceleft 124:bar 125:braceright 126:asciitilde "
        "128:bullet 129:dagger 130:daggerdbl 131:ellipsis 132:emdash 133:endash 134:florin 135:fraction "
        "136:guilsinglleft 137:guilsinglright 138:minus 139:perthousand 140:quotedblbase 141:quotedblleft "
        "142:quotedblright 143:quoteleft 144:quoteright 145:quotesinglbase 146:trademark 147:fi 148:fl "
        "149:Lslash 150:OE 151:Scaron 152:Ydieresis 153:Zcaron 154:dotlessi 155:lslash 156:oe 157:scaron "
        "158:zcaron 160:Euro 161:exclamdown 162:cent 163:sterling 164:currency 165:yen 166:brokenbar "
        "167:section 168:dieresis 169:copyright 170:ordfeminine 171:guillemotleft 172:logicalnot "
        "174:registered 175:macron 176:degree 177:plusminus 178:twosuperior 179:threesuperior 180:acute "
        "181:mu 182:paragraph 183:periodcentered 184:cedilla 185:onesuperior 186:ordmasculine "
        "187:guillemotright 188:onequarter 189:onehalf 190:threequarters 191:questiondown 192:Agrave "
        "193:Aacute 194:Acircumflex 195:Atilde 196:Adieresis 197:Aring 198:AE 199:Ccedilla 200:Egrave "
        "201:Eacute 202:Ecircumflex 203:Edieresis 204:Igrave 205:Iacute 206:Icircumflex 207:Idieresis 208:Eth "
        "209:Ntilde 210:Ograve 211:Oacute 212:Ocircumflex 213:Otilde 214:Odieresis 215:multiply 216:Oslash "
        "217:Ugrave 218:Uacute 219:Ucircumflex 220:Udieresis 221:Yacute 222:Thorn 223:germandbls 224:agrave "
        "225:aacute 226:acircumflex 227:atilde 228:adieresis 229:aring 230:ae 231:ccedilla 232:egrave "
        "233:eacute 234:ecircumflex 235:edieresis 236:igrave 237:iacute 238:icircumflex 239:idieresis 240:eth "
        "241:ntilde 242:ograve 243:oacute 244:ocircumflex 245:otilde 246:odieresis 247:divide 248:oslash "
        "249:ugrave 250:uacute 251:ucircumflex 252:udieresis 253:yacute 254:thorn 255:ydieresis "
    ),
    "MacExpertEncoding": (
        "32:space 33:exclamsmall 34:Hungarumlautsmall 35:centoldstyle 36:dollaroldstyle 37:dollarsuperior "
        "38:ampersandsmall 39:Acutesmall 40:parenleftsuperior 41:parenrightsuperior 42:twodotenleader "
        "43:onedotenleader 44:comma 45:hyphen 46:period 47:fraction 48:zerooldstyle 49:oneoldstyle "
        "50:twooldstyle 51:threeoldstyle 52:fouroldstyle 53:fiveoldstyle 54:sixoldstyle 55:sevenoldstyle "
        "56:eightoldstyle 57:nineoldstyle 58:colon 59:semicolon 61:threequartersemdash 63:questionsmall "
        "68:Ethsmall 71:onequarter 72:onehalf 73:threequarters 74:oneeighth 75:threeeighths 76:fiveeighths "
        "77:seveneighths 78:onethird 79:twothirds 86:ff 87:fi 88:fl 89:ffi 90:ffl 91:parenleftinferior "
        "93:parenrightinferior 94:Circumflexsmall 95:hypheninferior 96:Gravesmall 97:Asmall 98:Bsmall "
        "99:Csmall 100:Dsmall 101:Esmall 102:Fsmall 103:Gsmall 104:Hsmall 105:Ismall 106:Jsmall 107:Ksmall "
        "108:Lsmall 109:Msmall 110:Nsmall 111:Osmall 112:Psmall 113:Qsmall 114:Rsmall 115:Ssmall 116:Tsmall "
        "117:Usmall 118:Vsmall 119:Wsmall 120:Xsmall 121:Ysmall 122:Zsmall 123:colonmonetary 124:onefitted "
        "125:rupiah 126:Tildesmall 129:asuperior 130:centsuperior 135:Aacutesmall 136:Agravesmall "
        "137:Acircumflexsmall 138:Adieresissmall 139:Atildesmall 140:Aringsmall 141:Ccedillasmall "
        "142:Eacutesmall 143:Egravesmall 144:Ecircumflexsmall 145:Edieresissmall 146:Iacutesmall "
        "147:Igravesmall 148:Icircumflexsmall 149:Idieresissmall 150:Ntildesmall 151:Oacutesmall "
        "152:Ogravesmall 153:Ocircumflexsmall 154:Odieresissmall 155:Otildesmall 156:Uacutesmall "
        "157:Ugravesmall 158:Ucircumflexsmall 159:Udieresissmall 161:eightsuperior 162:fourinferior "
        "163:threeinferior 164:sixinferior 165:eightinferior 166:seveninferior 167:Scaronsmall "
        "169:centinferior 170:twoinferior 172:Dieresissmall 174:Caronsmall 175:osuperior 176:fiveinferior "
        "178:commainferior 179:periodinferior 180:Yacutesmall 182:dollarinferior 185:Thornsmall "
        "187:nineinferior 188:zeroinferior 189:Zcaronsmall 190:AEsmall 191:Oslashsmall 192:questiondownsmall "
        "193:oneinferior 194:Lslashsmall 201:Cedillasmall 207:OEsmall 208:figuredash 209:hyphensuperior "
        "214:exclamdownsmall 216:Ydieresissmall 218:onesuperior 219:twosuperior 220:threesuperior "
        "221:foursuperior 222:fivesuperior 223:sixsuperior 224:sevensuperior 225:ninesuperior "
        "226:zerosuperior 228:esuperior 229:rsuperior 230:tsuperior 233:isuperior 234:ssuperior 235:dsuperior "
        "241:lsuperior 242:Ogoneksmall 243:Brevesmall 244:Macronsmall 245:bsuperior 246:nsuperior "
        "247:msuperior 248:commasuperior 249:periodsuperior 250:Dotaccentsmall 251:Ringsmall "
    ),
    "Symbol": (
        "32:space 33:exclam 34:universal 35:numbersign 36:existential 37:percent 38:ampersand 39:suchthat "
        "40:parenleft 41:parenright 42:asteriskmath 43:plus 44:comma 45:minus 46:period 47:slash 48:zero "
        "49:one 50:two 51:three 52:four 53:five 54:six 55:seven 56:eight 57:nine 58:colon 59:semicolon "
        "60:less 61:equal 62:greater 63:question 64:congruent 65:Alpha 66:Beta 67:Chi 68:Delta 69:Epsilon "
        "70:Phi 71:Gamma 72:Eta 73:Iota 74:theta1 75:Kappa 76:Lambda 77:Mu 78:Nu 79:Omicron 80:Pi 81:Theta "
        "82:Rho 83:Sigma 84:Tau 85:Upsilon 86:sigma1 87:Omega 88:Xi 89:Psi 90:Zeta 91:bracketleft "
        "92:therefore 93:bracketright 94:perpendicular 95:underscore 96:radicalex 97:alpha 98:beta 99:chi "
        "100:delta 101:epsilon 102:phi 103:gamma 104:eta 105:iota 106:phi1 107:kappa 108:lambda 109:mu 110:nu "
        "111:omicron 112:pi 113:theta 114:rho 115:sigma 116:tau 117:upsilon 118:omega1 119:omega 120:xi "
        "121:psi 122:zeta 123:braceleft 124:bar 125:braceright 126:similar 160:Euro 161:Upsilon1 162:minute "
        "163:lessequal 164:fraction 165:infinity 166:florin 167:club 168:diamond 169:heart 170:spade "
        "171:arrowboth 172:arrowleft 173:arrowup 174:arrowright 175:arrowdown 176:degree 177:plusminus "
        "178:second 179:greaterequal 180:multiply 181:proportional 182:partialdiff 183:bullet 184:divide "
        "185:notequal 186:equivalence 187:approxequal 188:ellipsis 189:arrowvertex 190:arrowhorizex "
        "191:carriagereturn 192:aleph 193:Ifraktur 194:Rfraktur 195:weierstrass 196:circlemultiply "
        "197:circleplus 198:emptyset 199:intersection 200:union 201:propersuperset 202:reflexsuperset "
        "203:notsubset 204:propersubset 205:reflexsubset 206:element 207:notelement 208:angle 209:gradient "
        "210:registerserif 211:copyrightserif 212:trademarkserif 213:product 214:radical 215:dotmath "
        "216:logicalnot 217:logicaland 218:logicalor 219:arrowdblboth 220:arrowdblleft 221:arrowdblup "
        "222:arrowdblright 223:arrowdbldown 224:lozenge 225:angleleft 226:registersans 227:copyrightsans "
        "228:trademarksans 229:summation 230:parenlefttp 231:parenleftex 232:parenleftbt 233:bracketlefttp "
        "234:bracketleftex 235:bracketleftbt 236:bracelefttp 237:braceleftmid 238:braceleftbt 239:braceex "
        "241:angleright 242:integral 243:integraltp 244:integralex 245:integralbt 246:parenrighttp "
        "247:parenrightex 248:parenrightbt 249:bracketrighttp 250:bracketrightex 251:bracketrightbt "
        "252:bracerighttp 253:bracerightmid 254:bracerightbt "
    ),
    "ZapfDingbats": (
        "32:space 33:a1 34:a2 35:a202 36:a3 37:a4 38:a5 39:a119 40:a118 41:a117 42:a11 43:a12 44:a13 45:a14 "
        "46:a15 47:a16 48:a105 49:a17 50:a18 51:a19 52:a20 53:a21 54:a22 55:a23 56:a24 57:a25 58:a26 59:a27 "
        "60:a28 61:a6 62:a7 63:a8 64:a9 65:a10 66:a29 67:a30 68:a31 69:a32 70:a33 71:a34 72:a35 73:a36 74:a37 "
        "75:a38 76:a39 77:a40 78:a41 79:a42 80:a43 81:a44 82:a45 83:a46 84:a47 85:a48 86:a49 87:a50 88:a51 "
        "89:a52 90:a53 91:a54 92:a55 93:a56 94:a57 95:a58 96:a59 97:a60 98:a61 99:a62 100:a63 101:a64 102:a65 "
        "103:a66 104:a67 105:a68 106:a69 107:a70 108:a71 109:a72 110:a73 111:a74 112:a203 113:a75 114:a204 "
        "115:a76 116:a77 117:a78 118:a79 119:a81 120:a82 121:a83 122:a84 123:a97 124:a98 125:a99 126:a100 "
        "161:a101 162:a102 163:a103 164:a104 165:a106 166:a107 167:a108 168:a112 169:a111 170:a110 171:a109 "
        "172:a120 173:a121 174:a122 175:a123 176:a124 177:a125 178:a126 179:a127 180:a128 181:a129 182:a130 "
        "183:a131 184:a132 185:a133 186:a134 187:a135 188:a136 189:a137 190:a138 191:a139 192:a140 193:a141 "
        "194:a142 195:a143 196:a144 197:a145 198:a146 199:a147 200:a148 201:a149 202:a150 203:a151 204:a152 "
        "205:a153 206:a154 207:a155 208:a156 209:a157 210:a158 211:a159 212:a160 213:a161 214:a163 215:a164 "
        "216:a196 217:a165 218:a192 219:a166 220:a167 221:a168 222:a169 223:a170 224:a171 225:a172 226:a173 "
        "227:a162 228:a174 229:a175 230:a176 231:a177 232:a178 233:a179 234:a193 235:a180 236:a199 237:a181 "
        "238:a200 239:a182 241:a201 242:a183 243:a184 244:a197 245:a185 246:a194 247:a198 248:a186 249:a195 "
        "250:a187 251:a188 252:a189 253:a190 254:a191 "
    ),
}


def annex_cases():
    for name, assignments in ANNEX_ENCODINGS.items():
        names = {int(entry.split(":")[0]): entry.split(":")[1] for entry in assignments.split()}
        yield Vector("annex-"+name, {".base": name.encode(), ".out": expected([(code,names.get(code)) for code in range(256)])}, ("text-simple-encoding",), "All code-to-name slots, including undefined slots, from Annex D", {"driver": "encoding-names"})


def embedded_document(program, *, subtype=b"TrueType", fontfile=b"/FontFile2", font_extra=b"", stream_extra=b"", encoding=b"/WinAnsiEncoding", flags=32, first=32, last=None, widths=None, cmap=None, cmap_extra=b"", unicode_map=None, cid_map=None, descendant_extra=b"", cid_subtype=b"CIDFontType2", omit_cid_map=False, unicode_extra=b"", parents=()):
    """PDF-level bindings; the independent font writer supplies program bytes."""
    widths = widths if widths is not None else [600] * 192
    last = last if last is not None else first + len(widths) - 1
    encoding_entry = b" /Encoding " + encoding if encoding else b""
    if subtype == b"Type0":
        root = b"<< /Type /Font /Subtype /Type0 /BaseFont /Fixture /DescendantFonts [7 0 R]" + encoding_entry
    else:
        root = b"<< /Type /Font /Subtype /" + subtype + b" /BaseFont /Fixture /FontDescriptor 5 0 R" + encoding_entry
        root += b" /FirstChar %d /LastChar %d /Widths [" % (first,last) + b" ".join(str(w).encode() for w in widths) + b"]"
    root += (b" /ToUnicode 9 0 R" if unicode_map is not None else b"") + b" " + font_extra + b" >>"
    descriptor = b"<< /Type /FontDescriptor /FontName /Fixture /Flags %d /FontBBox [0 0 300 300] /ItalicAngle 0 /Ascent 800 /Descent -200 /CapHeight 700 /StemV 80 " % flags + fontfile + b" 6 0 R >>"
    descendant = b"<< /Type /Font /Subtype /" + cid_subtype + b" /BaseFont /Fixture /CIDSystemInfo << /Registry (Adobe) /Ordering (Identity) /Supplement 0 >> /FontDescriptor 5 0 R " + descendant_extra
    if cid_subtype == b"CIDFontType2" and not omit_cid_map:
        descendant += b" /CIDToGIDMap 10 0 R" if cid_map is not None else b" /CIDToGIDMap /Identity"
    descendant += b" >>"
    return pdf(root, [descriptor, stream(program,stream_extra), descendant,
                     stream(cmap,b"/Type /CMap /CMapName /Fixture /CIDSystemInfo << /Registry (Adobe) /Ordering (Identity) /Supplement 0 >> " + cmap_extra) if cmap is not None else b"null",
                     stream(unicode_map,unicode_extra) if unicode_map is not None else b"null",
                     stream(cid_map) if cid_map is not None else b"null", *parents])


def binding_vector(name, program, data, rows, *, matrix=(.001,0,0,.001,0,0), requirements=("text-embedded-fonts",), **options):
    return Vector(name, {".pdf":embedded_document(program,**options), ".input":data, ".out":expected(rows), ".matrix":expected([matrix])}, requirements, "Independent font program bindings, selectors and PDF-space metrics", {"driver":"program"})


def primary_regression_cases():
    from .font import truetype
    program=without_sfnt_tables(truetype(), {b"cmap"})
    cmap=b"/Identity-H usecmap 1 beginnotdefchar <0041> 1 endnotdefchar"
    yield binding_vector("cid-map-missing-entry", program, b"\0A", [(65,2,.5,False,None,0,None,None,None)], subtype=b"Type0", encoding=b"8 0 R", cmap=cmap, cmap_extra=b"/UseCMap /Identity-H", cid_map=bytes.fromhex("00000001"), descendant_extra=b"/DW 500 /W [1 [600]]", stream_extra=b"/Length1 %d" % len(program), requirements=("text-cid-to-gid","text-cmap"))
    cmap=b"/Identity-H usecmap 1 begincidchar <0041> 1 endcidchar 1 beginnotdefchar <0041> 2 endnotdefchar"
    yield binding_vector("cid-map-explicit-zero", program, b"\0A", [(65,2,.5,False,None,0,None,None,None)], subtype=b"Type0", encoding=b"8 0 R", cmap=cmap, cmap_extra=b"/UseCMap /Identity-H", cid_map=bytes.fromhex("000000000002"), descendant_extra=b"/DW 500 /W [2 [600]]", stream_extra=b"/Length1 %d" % len(program), requirements=("text-cid-to-gid","text-cmap"))



def without_sfnt_tables(program, excluded):
    from .font import sfnt
    count=struct.unpack_from(">H",program,4)[0]
    tables={}
    for index in range(count):
        tag,_,offset,length=struct.unpack_from(">4sIII",program,12+16*index)
        if tag not in excluded:
            tables[tag]=program[offset:offset+length]
    return sfnt(tables, otto=program[:4]==b"OTTO")


def text_cmap_records(records):
    """Independent sfnt cmap writer: Mac format 0, BMP format 4, full format 12."""
    from .font import u16, u32
    subtables=[]
    for platform,encoding,mapping in records:
        if (platform,encoding)==(1,0):
            sub=u16(0)+u16(262)+u16(0)+bytes(mapping.get(code,0) for code in range(256))
        elif (platform,encoding)==(3,10):
            groups=b"".join(u32(code)+u32(code)+u32(glyph) for code,glyph in sorted(mapping.items()))
            sub=u16(12)+u16(0)+u32(16+len(groups))+u32(0)+u32(len(mapping))+groups
        else:
            codes=sorted(mapping)
            count=len(codes)+1
            power=1 << (count.bit_length()-1)
            sub=(u16(4)+u16(16+8*count)+u16(0)+u16(2*count)+u16(2*power)
                 +u16(power.bit_length()-1)+u16(2*count-2*power)
                 +b"".join(u16(code) for code in codes)+u16(65535)+u16(0)
                 +b"".join(u16(code) for code in codes)+u16(65535)
                 +b"".join(u16((mapping[code]-code)&65535) for code in codes)+u16(1)
                 +bytes(2*count))
        subtables.append((platform,encoding,sub))
    offset=4+8*len(subtables)
    result=u16(0)+u16(len(subtables))
    for platform,encoding,sub in subtables:
        result+=u16(platform)+u16(encoding)+u32(offset)
        offset+=len(sub)
    return result+b"".join(sub for _,_,sub in subtables)


def text_truetype(records, names=(b"A",b"B",b"Aacute",b"D"), units=1000):
    from .font import truetype, u16, u32
    post=u32(0x20000)+bytes(28)+u16(5)+u16(0)+b"".join(u16(258+i) for i in range(4))
    post+=b"".join(bytes([len(name)])+name for name in names)
    program=truetype(table_overrides={b"cmap":text_cmap_records(records),b"post":post})
    if units!=1000:
        # Change unitsPerEm in the independent writer's head, then serialize.
        count=struct.unpack_from(">H",program,4)[0]
        data=bytearray(program)
        for index in range(count):
            tag,_,offset,_=struct.unpack_from(">4sIII",program,12+16*index)
            if tag==b"head":data[offset+18:offset+20]=u16(units)
        program=bytes(data)
    return program


def program_cases():
    from .font import cff_font, type1_font, truetype
    def row(code, glyph, text=None, width=.6, length=1, vertical=None):
        return (code,length,width,code==32 and length==1,text,glyph,*(vertical or (None,None,None)))
    def tt(name, records, data=b"A", rows=None, *, names=(b"A",b"B",b"Aacute",b"D"), units=1000, **options):
        program=text_truetype(records,names,units)
        return binding_vector(name,program,data,rows or [row(65,1,"A")],matrix=(1/units,0,0,1/units,0,0),stream_extra=b"/Length1 %d" % len(program),requirements=("text-truetype-selection","text-embedded-fonts"),**options)
    yield tt("tt-full-unicode-mac",[(3,10,{65:1}),(1,0,{65:2})],rows=[row(65,2,"A")])
    yield tt("tt-platform-zero-mac",[(0,3,{65:1}),(1,0,{65:2})],rows=[row(65,2,"A")])
    yield tt("tt-bmp-before-full",[(3,1,{65:1}),(3,10,{65:2})])
    yield tt("tt-unicode-priority",[(3,1,{65:1}),(1,0,{65:2})])
    # A zero from a present format-4 subtable is not a missing subtable.
    yield tt("tt-unicode-missing-post",[(3,1,{66:2}),(1,0,{65:2})])
    yield tt("tt-macroman-fallback",[(1,0,{65:2})],rows=[row(65,2,"A")])
    yield tt("tt-mac-euro",[(1,0,{219:2})],data=b"\x80",rows=[row(128,2,"€")],names=(b"A",b"Euro",b"Aacute",b"D"))
    yield tt("tt-mac-currency-post",[(1,0,{219:1})],data=b"\xdb",rows=[row(219,2,"¤")],encoding=b"/MacRomanEncoding",names=(b"A",b"currency",b"Aacute",b"D"))
    yield tt("tt-mac-extra-apple",[(1,0,{240:2})],rows=[row(65,2)],encoding=b"<< /BaseEncoding /WinAnsiEncoding /Differences [65 /apple] >>")
    yield tt("tt-post-without-cmap",[],rows=[row(65,1,"A")])
    yield tt("tt-post-differences",[(3,1,{65:1})],rows=[row(65,3,"Á")],encoding=b"<< /BaseEncoding /WinAnsiEncoding /Differences [65 /Aacute] >>")
    yield tt("tt-name-not-found",[(3,1,{66:2})],rows=[row(65,0)],encoding=b"<< /Differences [65 /unknown] >>")
    yield tt("tt-named-mac-undefined",[(3,1,{0x203a:2})],data=b"\xad",rows=[row(173,0)],encoding=b"/MacRomanEncoding")
    yield tt("tt-dictionary-standard-fill",[(3,1,{0x203a:2})],data=b"\xad",rows=[row(173,2,"›")],encoding=b"<< /BaseEncoding /MacRomanEncoding /Differences [] >>")
    for high in (0,0xf000,0xf100,0xf200):
        yield tt("tt-symbol-%04x" % high,[(3,0,{high+65:1}),(1,0,{65:2})],rows=[row(65,1)],flags=4,encoding=b"/InvalidEncoding")
    yield tt("tt-symbol-mac-only",[(1,0,{65:2})],rows=[row(65,2)],flags=4,encoding=None)
    # The mandated symbol route fails: the final paragraph permits Mac fallback.
    yield tt("tt-symbol-present-missing",[(3,0,{66:1}),(1,0,{65:2})],rows=[row(65,2)],flags=4,encoding=None)
    yield tt("tt-noencoding-direct",[(3,1,{65:1}),(1,0,{65:2})],rows=[row(65,2)],encoding=None)
    yield tt("tt-noencoding-symbol",[(3,0,{0xf041:2}),(3,1,{65:1})],rows=[row(65,2)],encoding=None)
    yield tt("tt-units-per-em",[(3,1,{65:1})],rows=[row(65,1,"A",600/1280)],units=1280,widths=[600/1280*1000]*192)
    unicode_map=b"1 begincodespacerange <00> <ff> endcodespacerange 1 beginbfchar <41> <0042> endbfchar"
    yield tt("tt-tounicode-does-not-select",[(3,1,{65:1,66:2})],rows=[row(65,1,"B")],unicode_map=unicode_map)
    additions=[(b"notequal",173,"≠"),(b"infinity",176,"∞"),(b"lessequal",178,"≤"),(b"greaterequal",179,"≥"),(b"partialdiff",182,"∂"),(b"summation",183,"∑"),(b"product",184,"∏"),(b"pi",185,"π"),(b"integral",186,"∫"),(b"Omega",189,"Ω"),(b"radical",195,"√"),(b"approxequal",197,"≈"),(b"Delta",198,"∆"),(b"lozenge",215,"◊"),(b"Euro",219,"€"),(b"apple",240,None)]
    encoding=b"<< /BaseEncoding /MacRomanEncoding /Differences [32 "+b" ".join(b"/"+name for name,_,_ in additions)+b"] >>"
    yield tt("tt-mac-os-additions",[(1,0,{code:1+i%2 for i,(_,code,_) in enumerate(additions)})],data=bytes(range(32,48)),rows=[row(32+i,1+i%2,value) for i,(_,_,value) in enumerate(additions)],encoding=encoding)
    cff=cff_font()
    yield binding_vector("embedded-cff",cff,b"AB\xc1\xc2",[row(65,1,"A"),row(66,2),row(193,3,"Á"),row(194,4,"´")],subtype=b"Type1",fontfile=b"/FontFile3",stream_extra=b"/Subtype /Type1C",encoding=None,flags=4)
    open_cff=truetype(cff=cff)
    yield binding_vector("embedded-opentype-cff",open_cff,b"A",[row(65,1,"A")],subtype=b"Type1",fontfile=b"/FontFile3",stream_extra=b"/Subtype /OpenType",encoding=None,flags=4)
    open_tt=text_truetype([(3,1,{65:1})])
    yield binding_vector("embedded-opentype-truetype",open_tt,b"A",[row(65,1,"A")],fontfile=b"/FontFile3",stream_extra=b"/Subtype /OpenType /Length1 %d" % len(open_tt))
    cid=cff_font(cid=True)
    yield binding_vector("embedded-cid-cff",cid,bytes.fromhex("0011002a00410058"),[row(code,glyph,length=2) for code,glyph in [(17,1),(42,2),(65,3),(88,4)]],subtype=b"Type0",fontfile=b"/FontFile3",stream_extra=b"/Subtype /CIDFontType0C",encoding=b"/Identity-H",flags=4,cid_subtype=b"CIDFontType0",descendant_extra=b"/DW 600")
    yield binding_vector("embedded-cid-cff-gid",cff,bytes.fromhex("000100020004"),[row(code,code,length=2) for code in (1,2,4)],subtype=b"Type0",fontfile=b"/FontFile3",stream_extra=b"/Subtype /CIDFontType0C",encoding=b"/Identity-H",flags=4,cid_subtype=b"CIDFontType0",descendant_extra=b"/DW 600")
    type1=type1_font()
    length1=type1.index(b"currentfile eexec\n")+len(b"currentfile eexec\n")
    yield binding_vector("embedded-type1",type1,b"A\xc2\xc1",[row(65,1,"A"),row(194,4,"´"),row(193,0,"`")],subtype=b"Type1",fontfile=b"/FontFile",stream_extra=b"/Length1 %d /Length2 %d /Length3 0" % (length1,len(type1)-length1),encoding=None,flags=4)
    type1_scaled=type1.replace(b"[0.001 0 0 0.001 0 0]",b"[0.002 0 0 0.003 0 0]")
    length1=type1_scaled.index(b"currentfile eexec\n")+len(b"currentfile eexec\n")
    yield binding_vector("embedded-type1-matrix",type1_scaled,b"A",[row(65,1,"A",1.2)],subtype=b"Type1",fontfile=b"/FontFile",stream_extra=b"/Length1 %d /Length2 %d /Length3 0" % (length1,len(type1_scaled)-length1),encoding=None,flags=4,widths=[1200]*192,matrix=(.002,0,0,.003,0,0))
    tt_cid=without_sfnt_tables(truetype(),{b"cmap"})
    yield binding_vector("embedded-cid-truetype-vertical",tt_cid,bytes.fromhex("0001000200030004"),[row(1,2,length=2,vertical=(-.8,.2,.7)),row(2,1,length=2,vertical=(-1,.3,.88)),row(3,4,length=2,vertical=(-1,.3,.88)),row(4,0,width=.5,length=2,vertical=(-1,.25,.88))],subtype=b"Type0",encoding=b"/Identity-V",cid_map=bytes.fromhex("0000000200010004"),descendant_extra=b"/DW 600 /W [0 [500]] /W2 [1 [-800 200 700]]",stream_extra=b"/Length1 %d" % len(tt_cid),requirements=("text-embedded-fonts","text-cid-metrics","text-cid-to-gid"))
    cmap=b"2 begincodespacerange <00> <7f> <8100> <81ff> endcodespacerange 2 begincidchar <20> 1 <8120> 2 endcidchar"
    to_unicode=b"2 begincodespacerange <00> <7f> <8100> <81ff> endcodespacerange 2 beginbfchar <20> <0020> <8120> <D83DDE00> endbfchar"
    yield binding_vector("embedded-cid-variable-codes",tt_cid,b" \x81 ",[row(32,1," "),row(0x8120,2,"😀",length=2)],subtype=b"Type0",encoding=b"8 0 R",cmap=cmap,unicode_map=to_unicode,descendant_extra=b"/DW 600 /W [0 [500]]",stream_extra=b"/Length1 %d" % len(tt_cid),requirements=("text-embedded-fonts","text-cmap","text-unicode","text-string-contract"))
    cmap=b"/Identity-H usecmap 1 beginnotdefchar <0041> 2 endnotdefchar"
    yield binding_vector("embedded-cid-notdef",tt_cid,b"\0A\0B",[row(65,2,length=2),row(66,0,width=.5,length=2)],subtype=b"Type0",encoding=b"8 0 R",cmap=cmap,cmap_extra=b"/UseCMap /Identity-H",descendant_extra=b"/DW 600 /W [0 [500]]",stream_extra=b"/Length1 %d" % len(tt_cid),requirements=("text-embedded-fonts","text-cmap"))


def truetype_fallback_cases():
    """9.6.5.4 normative routes, then explicitly chosen reader policy.

    The records deliberately disagree. Expected glyphs follow the ISO route
    or the stated final-paragraph policy, never a product executable.
    """
    def vector(name, records, glyph, *, raw=False, code=65, text: str | None = "A", **options):
        program = text_truetype(records, names=(b"X", b"Y", b"Z", b"Q"))
        return binding_vector(name, program, bytes([code]),
            [(code, 1, .6, False, None if raw else text, glyph, None, None, None)],
            requirements=("text-truetype-selection", "text-truetype-fallback"),
            encoding=None if raw else options.pop("encoding", b"/WinAnsiEncoding"),
            stream_extra=b"/Length1 %d" % len(program), **options)
    yield vector("tt-mac-by-name", [(1,0,{128:2,196:1})], 2, code=196, text="Ä")
    yield vector("tt-symbol-encoding-ignored", [(3,0,{0xf041:2}),(3,1,{65:1})], 2,
                 flags=4, text=None)
    yield vector("tt-policy-name-mac", [(3,1,{66:1}),(1,0,{65:2})], 2)
    yield vector("tt-policy-name-full", [(3,10,{65:2})], 2)
    yield vector("tt-policy-name-platform-zero", [(0,3,{65:2})], 2)
    yield vector("tt-policy-full-before-platform-zero", [(3,10,{65:2}),(0,3,{65:1})], 2)
    yield vector("tt-policy-mac-before-full", [(3,1,{66:1}),(1,0,{65:2}),(3,10,{65:3})], 2)
    yield vector("tt-policy-name-symbol", [(3,0,{0xf041:2})], 2)
    yield vector("tt-policy-raw-unicode", [(3,1,{65:2}),(1,0,{66:1})], 2, raw=True)
    yield vector("tt-policy-raw-unicode-f000", [(3,1,{0xf041:2})], 2, raw=True, flags=4)
    yield vector("tt-policy-raw-full", [(3,10,{65:2})], 2, raw=True)
    yield vector("tt-policy-raw-platform-zero", [(0,3,{65:2})], 2, raw=True)
    yield vector("tt-policy-raw-bmp-before-full", [(3,1,{65:2}),(3,10,{65:1})], 2, raw=True)
    yield vector("tt-policy-raw-unmapped", [(3,1,{66:2})], 0, raw=True)
    yield vector("tt-policy-multiscalar-unmapped", [(3,10,{102:2})], 0,
                 encoding=b"<< /Differences [65 /f_f] >>", text="ff")
    # A successful post lookup is binding even when it explicitly names gid 0.
    yield vector("tt-post-notdef-before-policy", [(3,0,{0xf041:2})], 0,
                 encoding=b"<< /Differences [65 /.notdef] >>", text=None)
    yield vector("tt-policy-name-nonbmp", [(3,10,{0x1f600:2})], 2,
                 encoding=b"<< /Differences [65 /u1F600] >>", text="😀")


def glyph_name_cases():
    # AGL primary text is unavailable: these are explicit known-answer data,
    # not a claim of primary-text verification of that external algorithm.
    answers=[(b"A","A"),(b"A.alt","A"),(b"A.\xff","A"),(b"uni0041","A"),
             (b"uni004100420043","ABC"),(b"u0041","A"),(b"u10000","𐀀"),
             (b"u1F600","😀"),(b"u10FFFF",chr(0x10ffff)),(b"uni0066_uni0069","fi"),
             (b"f_f_i","ffi"),(b"A_B.alt","AB"),(b"A__B","AB"),(b"unknown_A","A"),(b"A_","A"),(b"uni0041D800_B","B"),(b"\xff_A","A"),(b"ff","ﬀ"),(b"fi","ﬁ"),
             (b"fl","ﬂ"),(b"ffi","ﬃ"),(b"ffl","ﬄ"),(b"uni0000","\0")]
    answers += [(name,None) for name in (b"",b".alt",b"uni",b"uni004",b"uni00411",b"uni00e9",b"u123",b"u1234567",b"u110000",b"uD800",b"uniD83DDE00",b"\xff.alt")]
    data=b"[\n"+b"\n".join(b"[<"+name.hex().encode()+b"> "+scalar(value)+b"]" for name,value in answers)+b"\n]\n"
    yield Vector("glyph-name-forms",{".out":data},("text-glyph-names","text-unicode"),"Known-answer legacy ligatures, suffixes, components and numeric glyph names",{"driver":"glyph-names","primary_text_status":"pending Adobe Glyph List primary-text review"})


def embedding_error_cases():
    from .font import truetype, cff_font, type1_font
    font=truetype()
    base={"stream_extra":b"/Length1 %d" % len(font)}
    cid=without_sfnt_tables(font,{b"cmap"})
    cases=[
        ("binding-wrong-fontfile", font, dict(base,subtype=b"Type1")),
        ("binding-length1", font, {"stream_extra":b"/Length1 %d" % (len(font)-1)}),
        ("binding-cff-length", cff_font(), {"subtype":b"Type1","fontfile":b"/FontFile3","stream_extra":b"/Subtype /Type1C /Length1 0"}),
        ("binding-type1-lengths", type1_font(), {"subtype":b"Type1","fontfile":b"/FontFile","stream_extra":b"/Length1 0 /Length2 0 /Length3 0"}),
        ("unicode-cid-code-length",cid,{"subtype":b"Type0","encoding":b"/Identity-H","stream_extra":b"/Length1 %d" % len(cid),"unicode_map":b"1 begincodespacerange <00> <ff> endcodespacerange 1 beginbfchar <41> <0041> endbfchar"}),
        ("unicode-simple-source-length",font,dict(base,unicode_map=b"1 begincodespacerange <00> <ff> endcodespacerange 1 beginbfchar <0041> <0041> endbfchar")),
    ]
    # Length1 to Length3 that disagree with the decoded program (Table 125) are
    # bookkeeping faults: strict mode rejects them, repair mode loads the
    # program as decoded and reports font.stream-lengths.
    lengths = {"binding-length1", "binding-cff-length", "binding-type1-lengths"}
    for name,program,options in cases:
        if name in lengths:
            yield Vector(name,{".pdf":embedded_document(program,**options),".repair":b"font.stream-lengths\n"},("text-embedded-fonts",),"Font stream Length entries disagree with the decoded program (Table 125); a reported repair",{"driver":"font-repair"})
            continue
        yield Vector(name,{".pdf":embedded_document(program,**options),".error":b"syntax\n"},("text-unicode",) if name.startswith("unicode-") else ("text-embedded-fonts",),"PDF binding or ToUnicode consistency violation derived from the ISO rules",{"driver":"font-error"})


def bundled_afm(path):
    """AFM oracle: independent record parsing, retaining every glyph width."""
    properties, glyphs = {}, []
    for line in path.read_text().splitlines():
        if line.startswith("C "):
            fields = dict(part.strip().split(maxsplit=1) for part in line.split(";") if part.strip())
            glyphs.append((int(fields["C"]), fields["N"], float(fields["WX"])))
        elif line and not line.startswith("Comment"):
            key, _, value = line.partition(" ")
            properties[key] = value.strip()
    return properties, glyphs


def bundled_tt_glyph(path, scalar):
    """Read the substitute's Windows BMP format-4 cmap, independently of Rust."""
    raw = path.read_bytes()
    count = struct.unpack_from(">H", raw, 4)[0]
    tables = {tag: offset for tag, _, offset, _ in
              (struct.unpack_from(">4sIII", raw, 12+16*i) for i in range(count))}
    start = tables[b"cmap"]
    records = struct.unpack_from(">H", raw, start+2)[0]
    for i in range(records):
        platform, encoding, offset = struct.unpack_from(">HHI", raw, start+4+8*i)
        if (platform, encoding) != (3, 1):
            continue
        base = start+offset
        assert struct.unpack_from(">H", raw, base)[0] == 4
        segments = struct.unpack_from(">H", raw, base+6)[0]//2
        ends = base+14
        starts = ends+2*segments+2
        deltas = starts+2*segments
        offsets = deltas+2*segments
        for j in range(segments):
            first = struct.unpack_from(">H", raw, starts+2*j)[0]
            last = struct.unpack_from(">H", raw, ends+2*j)[0]
            if first <= scalar <= last:
                delta = struct.unpack_from(">h", raw, deltas+2*j)[0]
                extra = struct.unpack_from(">H", raw, offsets+2*j)[0]
                if extra == 0:
                    return (scalar+delta) & 0xffff
                glyph = struct.unpack_from(">H", raw, offsets+2*j+extra+2*(scalar-first))[0]
                return ((glyph+delta) & 0xffff) if glyph else 0
    return 0


def bundled_cff_charset(path):
    """TN5176 sections 4, 5, 8-10, 13 and 14: read INDEX, Top DICT and name charset.

    Only the pinned bare CFF resources use this oracle. Standard SIDs below
    are facts from TN5176 Appendix A; custom SIDs index the file's String INDEX.
    """
    raw = path.read_bytes()

    def index(at):
        count = int.from_bytes(raw[at:at+2], "big")
        at += 2
        if not count:
            return [], at
        width = raw[at]
        at += 1
        offsets = [int.from_bytes(raw[at+i*width:at+(i+1)*width], "big")
                   for i in range(count+1)]
        at += width*(count+1)
        return [raw[at+a-1:at+b-1] for a,b in zip(offsets,offsets[1:])], at+offsets[-1]-1

    _, at = index(raw[2])
    tops, at = index(at)
    strings, _ = index(at)
    stack, fields, at = [], {}, 0
    top = tops[0]
    while at < len(top):
        byte = top[at]
        at += 1
        if byte <= 21:
            if byte == 12:
                byte = (12, top[at])
                at += 1
            fields[byte], stack = stack, []
        elif byte in (28,29):
            length = 2 if byte == 28 else 4
            stack.append(int.from_bytes(top[at:at+length], "big", signed=True))
            at += length
        elif byte == 30:
            while top[at] & 15 != 15 and top[at] >> 4 != 15:
                at += 1
            at += 1
            stack.append(None)  # Real operands are not offsets.
        elif 32 <= byte <= 246:
            stack.append(byte-139)
        elif 247 <= byte <= 254:
            extra = top[at]
            at += 1
            stack.append((byte-247)*256+extra+108 if byte <= 250
                         else -(byte-251)*256-extra-108)
        else:
            raise ValueError("unexpected CFF DICT operand")
    glyphs, _ = index(fields[17][0])
    at = fields[15][0]
    fmt = raw[at]
    at += 1
    sids = [0]
    while len(sids) < len(glyphs):
        first = int.from_bytes(raw[at:at+2], "big")
        at += 2
        if fmt == 0:
            count = 1
        else:
            width = 1 if fmt == 1 else 2
            assert fmt in (1,2)
            count = int.from_bytes(raw[at:at+width], "big")+1
            at += width
        sids.extend(range(first, first+count))
    assert len(sids) == len(glyphs)
    standard = {0:".notdef",1:"space"}
    # All other standard names used by the pinned symbol charset.
    standard.update(CFF_SYMBOL_SIDS)
    return {strings[sid-391].decode() if sid >= 391 else standard[sid]:gid
            for gid,sid in enumerate(sids)}


CFF_SYMBOL_SIDS = {
    2:"exclam",4:"numbersign",6:"percent",7:"ampersand",9:"parenleft",
    10:"parenright",12:"plus",13:"comma",15:"period",16:"slash",
    **dict(enumerate("zero one two three four five six seven eight nine".split(),17)),
    27:"colon",28:"semicolon",29:"less",30:"equal",31:"greater",32:"question",
    60:"bracketleft",62:"bracketright",64:"underscore",92:"braceleft",93:"bar",
    94:"braceright",99:"fraction",101:"florin",116:"bullet",121:"ellipsis",
    151:"logicalnot",152:"mu",156:"plusminus",159:"divide",161:"degree",
    166:"minus",168:"multiply",
}


def bundled_cmap(path, inventory, cache):
    """Independent file oracle. Expand intervals in Python, overlaying parents."""
    import re
    if path.name in cache:
        return cache[path.name]
    text = re.sub(r"%[^\n]*", "", path.read_text())
    parent = re.search(r"/(\S+)\s+usecmap", text)
    codes, inherited = {}, set()
    if parent:
        codes.update(bundled_cmap(inventory[parent[1]], inventory, cache)[0])
        inherited.update(codes)
    local = []
    for block in re.finditer(r"(\d+)\s+begincid(char|range)\s+(.*?)\s+endcid\2", text, re.S):
        if block[2] == "char":
            entries = [(a,a,c) for a,c in re.findall(r"<([0-9a-fA-F]+)>\s+(\d+)",block[3])]
        else:
            entries = re.findall(r"<([0-9a-fA-F]+)>\s+<([0-9a-fA-F]+)>\s+(\d+)",block[3])
        assert len(entries) == int(block[1])
        for low, high, cid in entries:
            length = len(low)//2
            lo, hi, dest = int(low,16), int(high,16), int(cid)
            local.append((length,lo))
            local.append((length,hi))
            for value in range(lo,hi+1):
                codes[length,value] = dest+value-lo
    mode = re.search(r"/WMode\s+(\d+)\s+def",text)
    assert mode
    cache[path.name] = codes, int(mode[1]), local, inherited
    return cache[path.name]


def bundled_unicode(path):
    """Aliases represent alternatives: prefer first horizontal, else first."""
    result = {}
    for line in path.read_text().splitlines():
        if not line or line.startswith(("#","CID")):
            continue
        cid, values = line.split("\t")
        aliases = [value.strip() for value in values.split(",")]
        chosen = next((v for v in aliases if not v.endswith("v")),aliases[0])
        result[int(cid)] = chr(int(chosen.removesuffix("v"),16))
    return result


@cache
def bundled_font_bytes(path):
    return path.read_bytes()


def bundled_otf_glyph(path, scalar):
    """Independent sfnt format 4/12 lookup; no use of the OTF's CFF CIDs."""
    raw = bundled_font_bytes(path)
    count = struct.unpack_from(">H", raw, 4)[0]
    tables = {tag: offset for tag,_,offset,_ in
              (struct.unpack_from(">4sIII", raw,12+16*i) for i in range(count))}
    cmap = tables[b"cmap"]
    count = struct.unpack_from(">H",raw,cmap+2)[0]
    records = [struct.unpack_from(">HHI",raw,cmap+4+8*i) for i in range(count)]
    for platform,encoding in [(3,10),(3,1)]+[(0,e) for p,e,_ in records if p==0]:
        for p,e,offset in records:
            if (p,e) != (platform,encoding):
                continue
            base = cmap+offset
            fmt = struct.unpack_from(">H",raw,base)[0]
            if fmt == 12:
                groups = struct.unpack_from(">I",raw,base+12)[0]
                for i in range(groups):
                    first,last,glyph = struct.unpack_from(">III",raw,base+16+12*i)
                    if first <= scalar <= last:
                        return glyph+scalar-first
            elif fmt == 4 and scalar <= 0xffff:
                segments = struct.unpack_from(">H",raw,base+6)[0]//2
                ends = base+14
                starts = ends+2*segments+2
                deltas = starts+2*segments
                offsets = deltas+2*segments
                for j in range(segments):
                    first = struct.unpack_from(">H",raw,starts+2*j)[0]
                    last = struct.unpack_from(">H",raw,ends+2*j)[0]
                    if first <= scalar <= last:
                        delta = struct.unpack_from(">h",raw,deltas+2*j)[0]
                        extra = struct.unpack_from(">H",raw,offsets+2*j)[0]
                        if not extra:
                            return (scalar+delta)&65535
                        gid = struct.unpack_from(">H",raw,offsets+2*j+extra+2*(scalar-first))[0]
                        return ((gid+delta)&65535) if gid else 0
    return 0


def symbol_substitute_cases(data, lock):
    for base in ("Symbol","ZapfDingbats"):
        item = lock["standard_14"][base]
        _, afm = bundled_afm(data/item["metrics"])
        charset = bundled_cff_charset(data/item["outline"])
        encoded = [(c,n,w,charset[n]) for c,n,w in afm if c>=0]
        name = "substitute-"+base+"-charset"
        yield Vector(name,{".pdf":pdf(b"<< /Subtype /Type1 /BaseFont /"+base.encode()+b" >>"),
                           ".input":bytes(c for c,_,_,_ in encoded),".out":expected(encoded),".base":base.encode()},
                     ("text-symbol-substitution",),"Every encoded AFM glyph selects the independently decoded CFF charset name",{"driver":"bundled-symbol"})
        selected = "theta1" if base=="Symbol" else "a202"
        width = next(w for _,n,w in afm if n==selected)
        for override in (False,True):
            name = "substitute-"+base+"-differences"+("-widths" if override else "")
            extra = b" /FirstChar 1 /LastChar 1 /Widths [731]" if override else b""
            font = b"<< /Subtype /Type1 /BaseFont /"+base.encode()+b" /Encoding << /Differences [1 /"+selected.encode()+b"] >>"+extra+b" >>"
            yield Vector(name,{".pdf":pdf(font),".input":b"\1",".out":expected([(1,selected,731 if override else width,charset[selected])]),".base":base.encode()},
                         ("text-symbol-substitution",),"Differences selects CFF names; explicit PDF widths take precedence over AFM",{"driver":"bundled-symbol"})
        if base=="Symbol":
            font=b"<< /Subtype /TrueType /BaseFont /Symbol /Encoding /MacRomanEncoding /FontDescriptor 5 0 R >>"
            yield Vector("substitute-Symbol-truetype-symbolic",{".pdf":pdf(font,[b"<< /Flags 4 >>"]),".input":b"A",".out":expected([(65,"Alpha",722,charset["Alpha"])]),".base":b"Symbol"},
                         ("text-symbol-substitution",),"A symbolic TrueType substitute retains the Symbol encoding despite a Latin Encoding entry",{"driver":"bundled-symbol"})

    charset = bundled_cff_charset(data/lock["standard_14"]["Symbol"]["outline"])
    yield Vector("substitute-Symbol-truetype-nonsymbolic",{
        ".pdf":pdf(b"<< /Subtype /TrueType /BaseFont /Symbol /Encoding /MacRomanEncoding /FontDescriptor 5 0 R >>",[b"<< /Flags 32 >>"]),
        ".input":b"A", ".out":expected([(65,"Alpha",722,charset["Alpha"])]), ".base":b"Symbol"},
        ("text-symbol-substitution",),"After the named Latin route lacks a glyph, a known Symbol TrueType face falls back to its AFM code mapping",{"driver":"bundled-symbol"})


def cid_substitute_document(ordering, cmap, *, extra=b"", descendant=b"", subtype=b"CIDFontType2", unicode_map=None, flags=4):
    font = b"<< /Subtype /Type0 /BaseFont /Collection /DescendantFonts [5 0 R] /Encoding "+cmap
    font += (b" /ToUnicode 8 0 R" if unicode_map else b"")+b" >>"
    cidfont = b"<< /Subtype /"+subtype+b" /BaseFont /Collection /CIDSystemInfo << /Registry (Adobe) /Ordering ("+ordering.encode()+b") /Supplement 0 >> /FontDescriptor 6 0 R "+descendant+b" >>"
    return pdf(font,[cidfont,b"<< /Flags %d /FontName /Collection >>" % flags,stream(extra) if extra else b"null",stream(unicode_map) if unicode_map else b"null"])


def cid_substitute_cases(data, lock):
    folders = {p.name.split('-')[1]:p for p in (data/"cmaps").iterdir() if p.is_dir()}
    inventory = {p.name:p for folder in folders.values() for p in folder.iterdir() if p.name!="cid2unicode.txt"}
    cache = {}
    prefixes = {"Japan1":"UniJIS","GB1":"UniGB","CNS1":"UniCNS","Korea1":"UniKS"}
    for collection, outline in lock["cid_substitutes"].items():
        ordering = collection.removeprefix("Adobe-")
        cmap_name = prefixes[ordering]+"-UCS2-H"
        codes,_,_,_ = bundled_cmap(inventory[cmap_name],inventory,cache)
        unicode = bundled_unicode(folders[ordering]/"cid2unicode.txt")
        samples = [0x4e00,0x4e8c]
        cids = [codes[2,c] for c in samples]
        widths = b" /DW 923 /W [%d [731]]" % cids[0]
        rows = [(code,cid,731 if i==0 else 923,unicode.get(cid),bundled_otf_glyph(data/outline,ord(unicode[cid]))) for i,(code,cid) in enumerate(zip(samples,cids))]
        assert all(row[4] for row in rows)
        for subtype in (b"CIDFontType0",b"CIDFontType2"):
            name="substitute-cid-"+ordering+"-"+subtype.decode()
            yield Vector(name,{".pdf":cid_substitute_document(ordering,b"/"+cmap_name.encode(),descendant=widths,subtype=subtype),".input":b"".join(c.to_bytes(2,'big') for c in samples),".out":expected(rows),".collection":ordering.encode()},
                         ("text-cid-substitution",),"CIDs select collection Unicode and regional OTF cmap glyphs; PDF W and DW supply advances",{"driver":"bundled-cid"})
        # Force a different extraction string: it must never select an outline.
        tu=b"1 begincodespacerange <0000> <ffff> endcodespacerange 1 beginbfchar <4e00> <0041> endbfchar"
        name="substitute-cid-"+ordering+"-tounicode"
        changed=[list(row) for row in rows];changed[0][3]="A"
        yield Vector(name,{".pdf":cid_substitute_document(ordering,b"/"+cmap_name.encode(),descendant=widths+b" /CIDToGIDMap /Identity",unicode_map=tu),".input":b"".join(c.to_bytes(2,'big') for c in samples),".out":expected(changed),".collection":ordering.encode()},
                     ("text-cid-substitution",),"ToUnicode affects extraction only and an external font ignores CIDToGIDMap",{"driver":"bundled-cid"})
        name="substitute-cid-"+ordering+"-vertical"
        yield Vector(name,{".pdf":cid_substitute_document(ordering,b"/"+(prefixes[ordering]+"-UCS2-V").encode()),".mode":b"vertical\n"},
                     ("text-cid-substitution",),"Vertical collection substitutes use GSUB forms and retain PDF metrics",{"driver":"bundled-cid-vertical"})
    # Unknown Unicode CID and a known Unicode value absent from the JP subset.
    ordering="Japan1"
    unicode=bundled_unicode(folders[ordering]/"cid2unicode.txt")
    outline=data/lock["cid_substitutes"]["Adobe-Japan1"]
    absent=next(cid for cid,text in unicode.items() if cid and not bundled_otf_glyph(outline,ord(text)))
    for name,cid,description in [("unicode",65535,"CID has no collection Unicode value"),("glyph",absent,"Regional substitute lacks the collection Unicode value")]:
        assert cid not in unicode if name=="unicode" else cid in unicode
        cmap=b"1 begincodespacerange <0000> <ffff> endcodespacerange 1 begincidchar <0001> %d endcidchar" % cid
        fixture="substitute-cid-missing-"+name
        yield Vector(fixture,{".pdf":cid_substitute_document(ordering,b"7 0 R",extra=cmap,descendant=b"/W [%d [731]]" % cid),".input":b"\0\1",".out":expected([(1,cid,731,unicode.get(cid),None)]),".collection":ordering.encode()},
                     ("text-cid-substitution",),description+"; no mark but the exact PDF advance remains",{"driver":"bundled-cid"})
    for name,ordering,encoding,error in [
        ("substitute-cid-mismatched-collection","Japan1",b"/UniGB-UCS2-H",b"syntax"),
        ("substitute-cid-identity-unknown","Identity",b"/Identity-H",b"font.non-embedded"),
    ]:
        yield Vector(name,{".pdf":cid_substitute_document(ordering,encoding),".error":error+b"\n"},
                     ("text-cid-substitution",),"Incompatible or unknown character collections never select guessed glyphs",{"driver":"bundled-cid-error"})
    cmap=b"/WMode 1 def 1 begincodespacerange <0000> <ffff> endcodespacerange 1 begincidchar <0001> 1 endcidchar"
    yield Vector("substitute-cid-wmode",{".pdf":cid_substitute_document("Japan1",b"7 0 R",extra=cmap),".mode":b"vertical\n"},
                 ("text-cid-substitution",),"Embedded CMap WMode 1 selects provider vertical forms",{"driver":"bundled-cid-vertical"})
    # Table 121: Symbolic and Nonsymbolic both set (36) contradict each other,
    # which is a descriptor repair even when a substitute ignores the flags.
    yield Vector("repair-cid-substitute-flags",{".pdf":cid_substitute_document("Japan1",b"/Identity-H",flags=36),".repair":b"font.descriptor-flags\n"},
                 ("text-cid-substitution",),"A substituted CIDFont reports a contradictory descriptor flag pair; strict mode rejects it",{"driver":"text-repair"})


def bundled_cases():
    import json
    from pathlib import Path
    from .font import truetype
    data = Path(__file__).resolve().parents[2]/"data"
    lock = json.loads((data/"lock.json").read_text())
    yield from symbol_substitute_cases(data, lock)
    yield from cid_substitute_cases(data, lock)
    names = []
    aliases = {
        "Helvetica": ["Arial", "ArialMT", "Arial,Regular"],
        "Helvetica-Bold": ["Arial-BoldMT", "Arial,Bold"],
        "Helvetica-Oblique": ["Arial-ItalicMT", "Arial,Italic"],
        "Helvetica-BoldOblique": ["Arial-BoldItalicMT", "Arial,BoldItalic"],
        "Times-Roman": ["TimesNewRomanPSMT", "TimesNewRoman,Regular"],
        "Times-Bold": ["TimesNewRomanPS-BoldMT", "TimesNewRoman,Bold"],
        "Times-Italic": ["TimesNewRomanPS-ItalicMT", "TimesNewRoman,Italic"],
        "Times-BoldItalic": ["TimesNewRomanPS-BoldItalicMT", "TimesNewRoman,BoldItalic"],
        "Courier": ["CourierNewPSMT", "CourierNew,Regular"],
        "Courier-Bold": ["CourierNewPS-BoldMT", "CourierNew,Bold"],
        "Courier-Oblique": ["CourierNewPS-ItalicMT", "CourierNew,Italic"],
        "Courier-BoldOblique": ["CourierNewPS-BoldItalicMT", "CourierNew,BoldItalic"],
    }
    short_aliases = {
        "Helvetica-Oblique": ["helveticai"],
        "Helvetica-BoldOblique": ["helveticaiB"],
        "Times-Bold": ["timesb", "timesbB"],
    }
    for base, item in lock["standard_14"].items():
        props,glyphs = bundled_afm(data/item["metrics"])
        widths = {name:width for _,name,width in glyphs}
        encoding = {code:name for code,name,_ in glyphs if code >= 0}
        # These sample names have explicit known Unicode values; the complete
        # AFM name/width/code set is separately checked without assuming AGL.
        sample_unicode = {"space":" ","A":"A","a":"a","fi":"ﬁ","Alpha":"Α","alpha":"α","arrowright":"→","a10":"✡","a60":"❁","a122":"③"}
        samples = [32,65,97,174,0]
        outlines = item["outline"]
        charset = bundled_cff_charset(data/outlines) if outlines.endswith(".cff") else None
        rows = [(code,widths.get(encoding.get(code),0)/1000,
                 sample_unicode.get(encoding.get(code)),  # pyright: ignore[reportArgumentType]
                 (charset.get(encoding.get(code),0) if charset is not None else
                  bundled_tt_glyph(data/outlines,ord(sample_unicode[encoding[code]]))
                  if outlines and encoding.get(code) in sample_unicode else 0))
                for code in samples]
        variants = [(base,base),(base+"-subset","ABCDEF+"+base)]
        variants += [(base+"-alias-%d" % i,name) for i,name in enumerate(aliases.get(base,[]))]
        for alias in short_aliases.get(base, []):
            variants += [("short-"+alias,alias), ("short-"+alias+"-subset","ABCDEF+"+alias)]
        for fixture,name in variants:
            fixture = "standard14-"+fixture
            names.append(fixture)
            files = {".pdf":pdf(b"<< /Type /Font /Subtype /Type1 /BaseFont /"+name.replace(",","#2C").encode()+b" >>"),
                     ".input":bytes(samples), ".out":expected(rows), ".base":base.encode()}
            if fixture == "standard14-"+base:
                files[".metrics"] = expected([(code,name,width) for code,name,width in glyphs])
                fields = [*map(float,props["FontBBox"].split()), float(props["ItalicAngle"]),props["IsFixedPitch"]=="true"]
                fields += [float(props[key]) if key in props else None for key in ("Ascender","Descender","CapHeight","XHeight","StdVW","StdHW")]
                files[".descriptor"] = expected([fields])
            requirements = ("text-standard-fonts","text-provider")
            if fixture.startswith("standard14-short-"):
                requirements += ("text-latin-substitution",)
            yield Vector(fixture,files,requirements,"AFM-derived standard font widths, encoding, descriptors and substitute glyphs",{"driver":"bundled-standard"})
    for fixture, encoding, code, glyph_name, unicode_value, width in [
        ("standard14-pdf-widths",b"/WinAnsiEncoding",65,"A","A",731),
        ("standard14-differences",b"<< /Differences [65 /fi] >>",65,"fi","ﬁ",None),
    ]:
        item = lock["standard_14"]["Helvetica"]
        _, glyphs = bundled_afm(data/item["metrics"])
        width = width if width is not None else next(w for _,n,w in glyphs if n == glyph_name)
        extra = b" /FirstChar 65 /LastChar 65 /Widths [731]" if fixture.endswith("widths") else b""
        names.append(fixture)
        yield Vector(fixture,{".pdf":pdf(b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding "+encoding+extra+b" >>"),
                              ".input":bytes([code]),".out":expected([(code,width/1000,unicode_value,bundled_tt_glyph(data/item["outline"],ord(unicode_value)))]),".base":b"Helvetica"},
                     ("text-standard-fonts","text-provider"),"PDF widths override AFM; Differences selects AFM widths by glyph name",{"driver":"bundled-standard"})
    yield Vector("bundled-standard-inventory",{".out":expected([(name,) for name in names])},("text-standard-fonts",),"All standard-14 canonical names and supported alias families",{"driver":"inventory"})
    inventory = {Path(file).name:data/file for file in lock["files"] if file.startswith("cmaps/") and Path(file).name not in ("cid2unicode.txt","LICENSE.md")}
    cache = {}
    cmaps = []
    program = without_sfnt_tables(truetype(),{b"cmap"})
    for name,path in sorted(inventory.items()):
        mappings,mode,local,inherited = bundled_cmap(path,inventory,cache)
        candidates = sorted(key for key in mappings if key[0] > 1)
        assert candidates
        keys = [candidates[0],candidates[-1]]
        keys += [key for key in local[:4] if key in mappings]
        keys += sorted(inherited)[:1]
        keys = list(dict.fromkeys(keys))
        rows = [(code,length,mappings[length,code]) for length,code in keys]
        inp = b"".join(code.to_bytes(length,"big") for length,code in keys)
        unicode = bundled_unicode(path.parent/"cid2unicode.txt")
        collection = path.parent.name.split("-")[1]
        cidmap = bytearray((max(row[2] for row in rows)+1)*2)
        for _,_,cid in rows:
            cidmap[cid*2:cid*2+2] = b"\0\1"
        widths = b" ".join(b"%d [%d]" % (cid,400+cid%100) for _,_,cid in rows)
        doc = embedded_document(program,subtype=b"Type0",encoding=b"/"+name.encode(),cid_map=bytes(cidmap),stream_extra=b"/Length1 %d" % len(program),descendant_extra=b"/W ["+widths+b"]")
        full_rows = [(code,length,(400+cid%100)/1000,code==32 and length==1,unicode.get(cid),1,
                      -1 if mode else None, (400+cid%100)/2000 if mode else None,.88 if mode else None)
                     for code,length,cid in rows]
        fixture = "predefined-"+name
        cmaps.append(fixture)
        yield Vector(fixture,{".cmap":name.encode(),".input":inp,".out":expected(rows),".pdf":doc,".glyphs":expected(full_rows),".mode":str(mode).encode(),".collection":collection.encode()},("text-predefined-cmaps","text-collection-unicode","text-provider"),"Data-derived multibyte codes, parent overlays, writing mode, CID and Unicode selection",{"driver":"bundled-cmap"})
    # An embedded map may inherit a bundled map, and explicit ToUnicode
    # still takes priority over the collection projection.
    parent_name = "90ms-RKSJ-V"
    path = inventory[parent_name]
    mapping,mode,_,_ = bundled_cmap(path,inventory,cache)
    length,code = min(key for key in mapping if key[0] > 1)
    raw = code.to_bytes(length,"big")
    cid = mapping[length,code]
    for fixture, override, explicit in [("predefined-stream-parent",True,False),("predefined-tounicode",False,True)]:
        selected = 2 if override else cid
        cidmap = bytearray((selected+1)*2)
        cidmap[selected*2:selected*2+2] = b"\0\1"
        local = b"/90ms-RKSJ-V usecmap 1 begincidchar <"+raw.hex().encode()+b"> 2 endcidchar" if override else None
        unicode_map = (b"1 begincodespacerange <"+raw.hex().encode()+b"> <"+raw.hex().encode()+b"> endcodespacerange 1 beginbfchar <"+raw.hex().encode()+b"> <D83DDE00> endbfchar") if explicit else None
        doc = embedded_document(program,subtype=b"Type0",encoding=b"8 0 R" if override else b"/90ms-RKSJ-V",cmap=local,cmap_extra=b"/UseCMap /90ms-RKSJ-V",cid_map=bytes(cidmap),unicode_map=unicode_map,stream_extra=b"/Length1 %d" % len(program),descendant_extra=b"/DW 615")
        cmaps.append(fixture)
        yield Vector(fixture,{".cmap":parent_name.encode(),".input":raw,".out":expected([(code,length,cid)]),".pdf":doc,
                              ".glyphs":expected([(code,length,.615,False,"😀" if explicit else bundled_unicode(path.parent/"cid2unicode.txt").get(selected),1,-1,.3075,.88)]),
                              ".mode":b"1",".collection":b"Japan1"},("text-predefined-cmaps","text-collection-unicode","text-provider"),"Embedded UseCMap resolves through provider; explicit ToUnicode wins",{"driver":"bundled-cmap"})
    yield Vector("bundled-cmap-inventory",{".out":expected([(name,) for name in cmaps])},("text-predefined-cmaps",),"All 59 bundled Table 116 names",{"driver":"inventory"})
    for path in sorted((data/"cmaps").glob("*/cid2unicode.txt")):
        collection = path.parent.name.split("-")[1]
        values = bundled_unicode(path)
        chosen = [min(values),max(values)]
        # Include ambiguous and vertical-only aliases and non-BMP values.
        for line in path.read_text().splitlines():
            if not line or line.startswith(("#","CID")):
                continue
            cid,aliases = line.split("\t")
            if "," in aliases or aliases.endswith("v") or ord(values[int(cid)])>65535:
                chosen.append(int(cid))
                if len(chosen)>=12:
                    break
        yield Vector("collection-"+collection,{".collection":collection.encode(),".out":expected([(cid,values[cid]) for cid in dict.fromkeys(chosen)])},("text-collection-unicode",),"Independent collection Unicode rows, ambiguity choices and vertical aliases",{"driver":"collection"})
    yield from substitution_cases(data,lock)
    yield from substitution_recovery_cases(data,lock)
    yield from style_substitution_cases(data,lock)


def substitution_cases(data, lock):
    choices = []
    for flags,weight,base in [(32,400,"Helvetica"),(32,700,"Helvetica-Bold"),(96,400,"Helvetica-Oblique"),(96,700,"Helvetica-BoldOblique"),
                              (34,400,"Times-Roman"),(34,700,"Times-Bold"),(98,400,"Times-Italic"),(98,700,"Times-BoldItalic"),
                              (33,400,"Courier"),(33,700,"Courier-Bold"),(97,400,"Courier-Oblique"),(97,700,"Courier-BoldOblique")]:
        item = lock["standard_14"][base]
        glyph = bundled_tt_glyph(data/item["outline"],65)
        for subtype in ("Type1","TrueType"):
            fixture = "substitute-%s-%s" % (subtype,base)
            choices.append(fixture)
            font = b"<< /Type /Font /Subtype /"+subtype.encode()+b" /BaseFont /UnknownFont /Encoding /WinAnsiEncoding /FirstChar 65 /LastChar 65 /Widths [731] /FontDescriptor 5 0 R >>"
            descriptor = b"<< /Flags %d /FontWeight %d >>" % (flags,weight)
            yield Vector(fixture,{".pdf":pdf(font,[descriptor]),".input":b"A",".out":expected([(65,.731,"A",glyph)]),".base":base.encode()},("text-font-substitution","text-provider"),"Descriptor-based substitution preserves the PDF width",{"driver":"substitute"})
    yield Vector("bundled-substitute-inventory",{".out":expected([(name,) for name in choices])},("text-font-substitution",),"All serif, fixed-pitch, weight and italic substitution routes",{"driver":"inventory"})
    for name,base,flags in [("unsafe-symbolic","UnknownSymbol",4),("unsafe-symbolic-both","UnknownSymbol",36),("unsafe-symbolic-known-base","Arial",4)]:
        font = b"<< /Type /Font /Subtype /TrueType /BaseFont /"+base.encode()+b" /FontDescriptor 5 0 R /Encoding /WinAnsiEncoding /FirstChar 65 /LastChar 65 /Widths [731] >>"
        yield Vector("substitute-"+name,{".pdf":pdf(font,[b"<< /Flags %d >>" % flags]),".error":b"font.non-embedded\n"},("text-font-substitution",),"Unsafe symbolic substitution fails explicitly",{"driver":"provider-error"})


def style_substitution_cases(data, lock):
    """Descriptor styles and known symbol aliases retain PDF advances."""
    inventory = []
    for suffix, attributes, selected in [
        ("angle", b"/ItalicAngle -11 /FontFamily /Ignored", "Helvetica-Oblique"),
        ("medium-weight", b"/FontWeight 500", "Helvetica"),
        ("bold-weight", b"/FontWeight 600", "Helvetica-Bold"),
    ]:
        name = "substitute-style-"+suffix
        inventory.append(name)
        font = b"<< /Subtype /TrueType /BaseFont /Unbundled /FontDescriptor 5 0 R /Encoding /WinAnsiEncoding /FirstChar 65 /LastChar 67 /Widths [731 187 923] >>"
        descriptor = b"<< /Flags 32 "+attributes+b" /MissingWidth 251 >>"
        program = data/lock["standard_14"][selected]["outline"]
        binary = program.read_bytes()
        head = next(offset for tag,_,offset,_ in (struct.unpack_from(">4sIII",binary,12+16*i) for i in range(struct.unpack_from(">H",binary,4)[0])) if tag == b"head")
        unit = 1/struct.unpack_from(">H",binary,head+18)[0]
        rows = [(code,width/1000,chr(code),bundled_tt_glyph(program,code))
                for code,width in zip(range(65,69),(731,187,923,251))]
        yield Vector(name,{".pdf":pdf(font,[descriptor]),".input":b"ABCD",".out":expected(rows),
                           ".positions":b"[0 .731 .918 1.841]\n",".base":selected.encode(),
                           ".matrix":expected([(unit,0,0,unit,0,0)])},
                     ("text-font-substitution",),
                     "Descriptor angle and weight choose bundled outlines; Widths and MissingWidth set positions; unused FontFamily is ignored",{"driver":"style-substitution"})
    for name, base_name, subtype, selected, explicit, indirect in [
        ("symbol-italic", b"Symbol,Italic", b"TrueType", "Symbol", False, False),
        ("symbol-bold", b"Symbol-Bold", b"Type1", "Symbol", True, False),
        ("symbol-subset", b"ABCDEF+Symbol,BoldItalic", b"TrueType", "Symbol", True, True),
        ("dingbats-italic", b"ZapfDingbats,Italic", b"Type1", "ZapfDingbats", False, False),
    ]:
        name = "substitute-style-"+name
        inventory.append(name)
        _, afm = bundled_afm(data/lock["standard_14"][selected]["metrics"])
        charset = bundled_cff_charset(data/lock["standard_14"][selected]["outline"])
        entries = {code:(glyph,width) for code,glyph,width in afm if code >= 0}
        font = b"<< /Subtype /"+subtype+b" /BaseFont /"+base_name
        font += b" /FontDescriptor 5 0 R /FirstChar 65 /LastChar 67 /Widths [731 187 923]" if explicit else b""
        if indirect:
            font += b" /Encoding << /BaseEncoding /MacRomanEncoding /Differences [65 /theta1] >>"
        font += b" >>"
        scalars = "ΑΒΧ" if selected=="Symbol" else "✡✢✣"
        widths = (731,187,923) if explicit else [entries[c][1] for c in range(65,68)]
        rows = [(code,width/1000,scalar,charset[entries[code][0]])
                for code,width,scalar in zip(range(65,68),widths,scalars)]
        positions: list[float] = [0]
        for _,width,_,_ in rows[:-1]: positions.append(positions[-1]+width)
        descriptors = ([b"<< /Flags 6 0 R >>",b"4"] if indirect else [b"<< /Flags 4 >>"]) if explicit else []
        yield Vector(name,{".pdf":pdf(font,descriptors),".input":b"ABC",".out":expected(rows),
                           ".positions":("["+" ".join(format(p,".12g") for p in positions)+"]\n").encode(),
                           ".base":selected.encode(),".matrix":expected([(.001,0,0,.001,0,0)])},
                     ("text-font-substitution",),
                     "Known symbol style aliases use reported regular outlines; PDF widths override AFM and indirect symbolic Flags suppress Differences",{"driver":"style-substitution"})
    yield Vector("substitute-style-inventory",{".out":expected([(name,) for name in inventory])},
                 ("text-font-substitution",),"Descriptor style and symbol alias cases",{"driver":"inventory"})


def cyrillic_names():
    # AGL data records, independently expressed as ordered alphabets rather
    # than the numeric ranges used by the implementation. Includes Yo/yo.
    for start, alphabet in [(10017,"АБВГДЕЁЖЗИЙКЛМНОПРСТУФХЦЧШЩЪЫЬЭЮЯ"),
                            (10065,"абвгдеёжзийклмнопрстуфхцчшщъыьэюя")]:
        for index, scalar in enumerate(alphabet):
            yield "afii"+str(start+index), scalar


def substitution_recovery_cases(data, lock):
    inventory = []
    # Table 120 permits only the nine hundreds. Invalid optional weights are
    # ignored, retaining the normal default and independent ForceBold hint.
    choices = [("zero",32,b"0","Helvetica"),
               ("serif",34,b"0","Times-Roman"),
               ("italic",98,b"0","Times-Italic"),
               ("fraction",32,b"650.5","Helvetica"),
               ("negative",32,b"-100","Helvetica"),
               ("large",32,b"1000","Helvetica"),
               ("string",32,b"(bold)","Helvetica"),
               ("force-bold",262176,b"0","Helvetica-Bold"),
               ("valid",32,b"400","Helvetica"),
               ("cyrillic",34,b"0","Times-Roman")]
    for suffix, flags, weight, base in choices:
        name = "substitute-weight-"+suffix
        inventory.append(name)
        letters = list(cyrillic_names()) if suffix == "cyrillic" else [("A","A")]
        first = 128 if suffix == "cyrillic" else 65
        encoding = b"<< /BaseEncoding /WinAnsiEncoding /Differences [%d " % first
        encoding += b" ".join(b"/"+glyph.encode() for glyph,_ in letters)+b"] >>"
        font = b"<< /Type /Font /Subtype /TrueType /BaseFont /UnbundledLatin /FontDescriptor 5 0 R /Encoding "+encoding
        font += b" /FirstChar %d /LastChar %d /Widths [" % (first,first+len(letters)-1)+b" ".join([b"731"]*len(letters))+b"] >>"
        descriptor = b"<< /Type /FontDescriptor /FontName /UnbundledLatin /Flags %d /FontWeight " % flags + weight+b" >>"
        program = data/lock["standard_14"][base]["outline"]
        binary = program.read_bytes()
        head = next(offset for tag,_,offset,_ in (struct.unpack_from(">4sIII",binary,12+16*i) for i in range(struct.unpack_from(">H",binary,4)[0])) if tag == b"head")
        unit = 1/struct.unpack_from(">H",binary,head+18)[0]
        rows = [(first+i,.731,scalar,bundled_tt_glyph(program,ord(scalar))) for i,(_,scalar) in enumerate(letters)]
        assert all(row[3] for row in rows), "substitutes must cover the specified Cyrillic glyphs"
        yield Vector(name,{".pdf":pdf(font,[descriptor]),".input":bytes(range(first,first+len(letters))),
                           ".out":expected(rows),".base":base.encode(),".tag":b"" if suffix=="valid" else b"font.descriptor-weight",
                           ".matrix":expected([(unit,0,0,unit,0,0)])},
                     ("text-descriptor-weight","text-latin-substitution"),
                     "Table 120 weight checks and reported normal-weight fallback; locked substitute cmap supplies glyph IDs",{"driver":"substitution-recovery"})
    yield Vector("substitution-recovery-inventory",{".out":expected([(name,) for name in inventory])},
                 ("text-descriptor-weight",),"Optional weight recovery cases",{"driver":"inventory"})
    yield Vector("glyph-name-cyrillic",{".out":expected(list(cyrillic_names()))},
                 ("text-latin-substitution","text-glyph-names"),"Russian AFII names checked against the local AGL data records",{"driver":"glyph-name"})


def tolerance_cases():
    """Paired strict font-loading rejections and exact tolerant glyph outcomes.

    Malformed input is independently constructed. Expected code, Unicode,
    advance, glyph ID and matrix never come from the product executable.
    """
    from .font import truetype
    program = text_truetype([(3,1,{65:1,66:2,67:3,68:4})],names=(b"A",b"B",b"C",b"D"))
    cid_program = without_sfnt_tables(truetype(),{b"cmap"})
    for name, value in [("direct",b"null"),("indirect",b"9 0 R"),("undefined",b"99 0 R")]:
        yield binding_vector("tounicode-null-"+name,program,b"A",[(65,1,.6,False,"A",1,None,None,None)],
                             font_extra=b"/ToUnicode "+value,stream_extra=b"/Length1 %d" % len(program),
                             requirements=("text-unicode","text-strict-repair-boundary"))
    space = b"1 begincodespacerange <00> <ff> endcodespacerange "
    def row(code,glyph,text=None,*,width=.6,length=1):
        return (code,length,width,code==32 and length==1,text,glyph,None,None,None)
    def vector(name, tags, inp, repaired, *, extraction_only=False, cmap=None, **options):
        selected = cid_program if options.get("subtype")==b"Type0" else program
        files = {".pdf":embedded_document(selected,stream_extra=b"/Length1 %d" % len(selected),cmap=cmap,**options),
                 ".input":inp,".out":expected(repaired),".repair":("\n".join(tags)+"\n").encode(),".matrix":expected([(.001,0,0,.001,0,0)])}
        files[".strict.error"] = (tags[0]+"\n").encode()
        requirements = ("text-tounicode-recovery",) if extraction_only else ("text-cmap-recovery",)
        if name in ("repair-tounicode-checksum","repair-tounicode-key-solidus"):
            requirements += ("text-strict-repair-boundary",)
        return Vector(name,files,requirements,
                      "Strict rejects with the refused repair tag; repair mode preserves exact usable mappings and glyph selection",{"driver":"text-repair","extraction_only":extraction_only})
    original = [row(65,1,"A"),row(66,2,"B"),row(67,3,"C"),row(68,4,"D")]
    for name, sources in [("simple",b"<0041> <005A> <0142> <03A9>"),("mixed-sources",b"<41> <005A> <0142> <03A9>")]:
        cmap=b"1 begincodespacerange <0000> <ffff> endcodespacerange 2 beginbfchar "+sources+b" endbfchar"
        yield vector("repair-tounicode-"+name,["font.tounicode-simple-codes"],b"AB",[row(65,1,"Z"),row(66,2,"Ω")],extraction_only=True,unicode_map=cmap)
    cmap=b"1 begincodespacerange <0000> <ffff> endcodespacerange 3 beginbfchar <0041> <005A> <0141> <03A9> <0042> <0059> endbfchar"
    yield vector("repair-tounicode-alias-conflict",["font.tounicode-simple-codes"],b"AB",[row(65,1,"A"),row(66,2,"Y")],extraction_only=True,unicode_map=cmap)
    cmap=space+b"5 beginbfchar <41> <005A> <42> <D800> <43> <DC00> <44> <00> <45> <D83DDE00> endbfchar"
    yield vector("repair-tounicode-utf16",["font.tounicode-utf16"],b"ABCDE",[row(65,1,"Z"),*original[1:],row(69,0,"😀")],extraction_only=True,unicode_map=cmap)
    cmap=space+b"1 beginbfrange <41> <44> <00FE> endbfrange"
    yield vector("repair-tounicode-range",["font.tounicode-range"],b"ABCD",[row(65+i,i+1,chr(254+i)) for i in range(4)],extraction_only=True,unicode_map=cmap)
    cmap=space+b"1 beginbfrange <41> <43> <FFFF> endbfrange"
    yield vector("repair-tounicode-range-end",["font.tounicode-range"],b"ABC",[row(65,1,chr(65535)),*original[1:3]],extraction_only=True,unicode_map=cmap)
    cmap=space+b"1 beginbfrange <41> <44> <D7FF> endbfrange"
    yield vector("repair-tounicode-range-surrogates",["font.tounicode-range","font.tounicode-utf16"],b"ABCD",[row(65,1,chr(0xd7ff)),*original[1:]],extraction_only=True,unicode_map=cmap)
    parent=space+b"2 beginbfchar <41> <0058> <42> <0059> endbfchar"
    child=b"1 beginbfchar <41> <D800> endbfchar"
    yield vector("repair-tounicode-parent-invalid",["font.tounicode-utf16"],b"AB",[row(65,1,"A"),row(66,2,"Y")],extraction_only=True,unicode_map=child,unicode_extra=b"/UseCMap 11 0 R",parents=[stream(parent)])
    block=b"101 beginbfchar "+b" ".join(b"<%02X> <%04X>" % (i,256+i) for i in range(101))+b" endbfchar"
    yield vector("repair-tounicode-block-count",["font.cmap-block-count"],b"AB",[row(65,1,chr(321)),row(66,2,chr(322))],extraction_only=True,unicode_map=space+block)
    for name,cmap,extra,options in [
        ("cycle",b"",b"/UseCMap 9 0 R",{}),
        ("unsupported",b"run",b"",{}),
        ("truncated",space+b"2 beginbfchar <41> <005A> endbfchar",b"",{}),
        ("limit",b"1 begincodespacerange <00000000> <ffffffff> endcodespacerange 1 beginbfrange <00000000> <ffffffff> <0041> endbfrange",b"",{}),
        ("object",None,b"",{"font_extra":b"/ToUnicode false"}),
    ]:
        yield vector("repair-tounicode-"+name,["font.tounicode-invalid"],b"A",original[:1],extraction_only=True,unicode_map=cmap,unicode_extra=extra,**options)
    # Errors from lazy stream dictionaries and filters must not be swallowed
    # by the optional ToUnicode boundary (7.3.5, 7.3.7; 7.4.4; RFC 1950 2.2).
    import zlib
    cmap=space+b"1 beginbfchar <41> <005A> endbfchar"
    compressed=zlib.compress(cmap)
    damaged=compressed[:-1]+bytes([compressed[-1]^1])
    yield vector("repair-tounicode-checksum",["filter.checksum-mismatch"],b"A",[row(65,1,"Z")],extraction_only=True,unicode_map=damaged,unicode_extra=b"/Filter /FlateDecode")
    yield vector("repair-tounicode-key-solidus",["syntax.key-solidus"],b"A",[row(65,1,"Z")],extraction_only=True,unicode_map=cmap,unicode_extra=b"Unused true")
    widths=b"/DW 600 /W [1 [500 600 700 800]]"
    block=b"1 begincodespacerange <0000> <ffff> endcodespacerange 101 begincidchar "+b" ".join(b"<%04X> %d" % (i,1+i%4) for i in range(101))+b" endcidchar"
    yield vector("repair-cmap-block-count",["font.cmap-block-count"],b"\0@\0A",[row(64,1,width=.5,length=2),row(65,2,width=.6,length=2)],subtype=b"Type0",encoding=b"8 0 R",cmap=block,descendant_extra=widths)
    cmap=b"1 begincodespacerange <00ff> <0102> endcodespacerange 1 begincidrange <00ff> <0103> 1 endcidrange"
    inp=b"".join(code.to_bytes(2,"big") for code in range(255,260))
    result=[row(255+i,i+1,width=.5+.1*i,length=2) for i in range(4)]+[row(259,0,length=2)]
    yield vector("repair-cmap-codespace",["font.cmap-codespace"],inp,result,subtype=b"Type0",encoding=b"8 0 R",cmap=cmap,descendant_extra=widths)
    cmap=b"1 begincodespacerange <00ff> <0102> endcodespacerange 1 beginbfrange <00ff> <0102> <0057> endbfrange"
    inp=b"".join(code.to_bytes(2,"big") for code in range(255,259))
    cidmap=bytearray(259*2)
    for i in range(4):cidmap[(255+i)*2:(256+i)*2]=(i+1).to_bytes(2,"big")
    result=[row(255+i,i+1,chr(87+i),width=.5+.1*i,length=2) for i in range(4)]
    # Widths belong to CIDs 255..258, independently of replacement GIDs.
    yield vector("repair-tounicode-codespace",["font.cmap-codespace"],inp,result,extraction_only=True,subtype=b"Type0",encoding=b"/Identity-H",cid_map=bytes(cidmap),unicode_map=cmap,descendant_extra=b"/W [255 [500 600 700 800]]")
    lo,hi=0x00ffffff,0x01000002
    cmap=b"1 begincodespacerange <00ffffff> <01000002> endcodespacerange 1 begincidrange <00ffffff> <01000002> 1 endcidrange"
    inp=b"".join(code.to_bytes(4,"big") for code in range(lo,hi+2))
    result=[row(lo+i,i+1,width=.5+.1*i,length=4) for i in range(4)]+[row(hi+1,0,length=4)]
    yield vector("repair-cmap-codespace-wide",["font.cmap-codespace"],inp,result,subtype=b"Type0",encoding=b"8 0 R",cmap=cmap,descendant_extra=widths)
    # One more distinct failure than the retained diagnostic cap (ADR 0012),
    # followed by the simple-code repair: 1024 retained, two suppressed.
    blocks=[]
    for start in range(0,1025,100):
        values=range(start,min(start+100,1025))
        blocks.append(b"%d beginbfchar " % len(values)+b" ".join(b"<%04X> <D800>" % code for code in values)+b" endbfchar")
    cmap=b"1 begincodespacerange <0000> <ffff> endcodespacerange "+b" ".join(blocks)
    yield Vector("repair-tounicode-diagnostic-cap",{".pdf":embedded_document(program,stream_extra=b"/Length1 %d" % len(program),unicode_map=cmap),".out":expected([(1024,2)])},("text-tounicode-recovery",),"Unicode faults use the bounded document diagnostic sink without accumulating a temporary error list",{"driver":"diagnostic-cap"})
    inp=b"\0\1\0\2\0\3\0\4\0\5"
    result=[row(i,i,width=.4+.1*i,length=2) for i in range(1,5)]+[row(5,0,length=2)]
    yield vector("repair-cid-to-gid-identity",["font.cid-to-gid-identity"],inp,result,subtype=b"Type0",encoding=b"/Identity-H",omit_cid_map=True,descendant_extra=widths)
    # Table 109: Widths has LastChar - FirstChar + 1 entries. Codes the array
    # does not reach use MissingWidth (0 here); entries past LastChar are unused.
    for name, last, widths in [("short", 68, [500, 600]), ("long", 66, [500, 600, 700, 800]), ("reversed", 64, [500, 600])]:
        reached = {65: .5, 66: .6} if name != "reversed" else {}
        rows = [row(65+i, i+1, chr(65+i), width=reached.get(65+i, 0)) for i in range(4)]
        files = {".pdf": embedded_document(program, stream_extra=b"/Length1 %d" % len(program), first=65, last=last, widths=widths),
                 ".input": b"ABCD", ".out": expected(rows), ".repair": b"font.widths-length\n", ".matrix": expected([(.001,0,0,.001,0,0)])}
        yield Vector("repair-widths-"+name, files, ("text-simple-widths",), "Strict rejects a Widths length that disagrees with FirstChar and LastChar; repair mode uses MissingWidth past the array", {"driver": "text-repair"})
    # Table 112 names only three predefined encodings. Another name, directly or
    # as BaseEncoding, falls back to the default (StandardEncoding for a
    # nonsymbolic TrueType font, 9.6.5.4); Differences still apply.
    for name, encoding, rows in [
        ("name", b"/StandardEncoding", [row(65+i, i+1, chr(65+i)) for i in range(4)]),
        ("base", b"<< /BaseEncoding /Identity-H /Differences [66 /C] >>", [row(65, 1, "A"), row(66, 3, "C"), row(67, 3, "C"), row(68, 4, "D")]),
    ]:
        files = {".pdf": embedded_document(program, stream_extra=b"/Length1 %d" % len(program), encoding=encoding),
                 ".input": b"ABCD", ".out": expected(rows), ".repair": b"font.encoding-name\n", ".matrix": expected([(.001,0,0,.001,0,0)])}
        yield Vector("repair-encoding-"+name, files, ("text-simple-encoding",), "Strict rejects an encoding name Table 112 does not define; repair mode uses the font's default encoding", {"driver": "text-repair"})


def tolerance_paint_cases():
    """Actual page pixels prove optional Unicode never selects the outline,
    and that painting never examines ToUnicode: malformed maps neither fail
    a strict rendering nor are repaired by one (ADR 0034)."""
    from .paint import ttf, page
    from .model import Case, RENDER
    from .pdf import opaque_rectangles
    oracle=opaque_rectangles(32,32,[((4,4,8,12),(0,0,0))])
    space=b"1 begincodespacerange <00> <ff> endcodespacerange "
    block=b"101 beginbfchar "+b" ".join(b"<%02X> <%04X>" % (i,256+i) for i in range(101))+b" endbfchar"
    for name,cmap,tag in [
        ("simple",b"1 begincodespacerange <0000> <ffff> endcodespacerange 1 beginbfchar <0041> <005A> endbfchar","font.tounicode-simple-codes"),
        ("utf16",space+b"1 beginbfchar <41> <D800> endbfchar","font.tounicode-utf16"),
        ("range",space+b"1 beginbfrange <41> <42> <00FF> endbfrange","font.tounicode-range"),
        ("block-count",space+block,"font.cmap-block-count"),
        ("invalid",b"run","font.tounicode-invalid"),
    ]:
        objects=ttf()
        objects[0]=objects[0][:-2]+b" /ToUnicode 8 0 R >>"
        objects.append(stream(cmap))
        yield Case("paint-tounicode-"+name,page(b"BT /F 20 Tf 4 4 Td (A) Tj ET",objects,b"<< /Font << /F 5 0 R >> >>"),RENDER,
                   ("text-tounicode-recovery",),"Painting never reads ToUnicode, so data that extraction rejects with "+tag+" neither fails nor repairs a rendering of the same analytic rectangle glyph",oracle=oracle)
    for name,cmap,tag in [
        ("identity",None,"font.cid-to-gid-identity"),
        ("codespace",b"1 begincodespacerange <00ff> <0102> endcodespacerange 1 begincidchar <00ff> 1 endcidchar","font.cmap-codespace"),
        ("block-count",b"1 begincodespacerange <0000> <ffff> endcodespacerange 101 begincidchar "+b" ".join(b"<%04X> 1" % i for i in range(101))+b" endcidchar","font.cmap-block-count"),
    ]:
        original=ttf()
        font=b"<< /Type /Font /Subtype /Type0 /BaseFont /Rect /DescendantFonts [8 0 R] /Encoding "+(b"9 0 R" if cmap else b"/Identity-H")+b" >>"
        descendant=b"<< /Type /Font /Subtype /CIDFontType2 /BaseFont /Rect /CIDSystemInfo << /Registry (Adobe) /Ordering (Identity) /Supplement 0 >> /FontDescriptor 6 0 R /DW 400 "+(b"/CIDToGIDMap /Identity" if cmap else b"")+b" >>"
        objects=[font,*original[1:],descendant]
        if cmap:objects.append(stream(cmap,b"/CMapName /Fixture /CIDSystemInfo << /Registry (Adobe) /Ordering (Identity) /Supplement 0 >>"))
        code=b"00ff" if name=="codespace" else b"0001"
        yield Case("paint-cmap-"+name,page(b"BT /F 20 Tf 4 4 Td <"+code+b"> Tj ET",objects,b"<< /Font << /F 5 0 R >> >>"),RENDER,
                   ("text-cmap-recovery",),"A reported font/CMap repair paints the exact rectangle; strict rejects it",oracle=oracle,repairs=(tag,),strict_valid=False)


def strict_substitution_cases():
    """Strict ToUnicode rejection also applies to provider-backed fonts."""
    cases = [
        ("simple", b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding /FirstChar 65 /LastChar 65 /Widths [600] /ToUnicode 5 0 R >>",
         [stream(b"1 begincodespacerange <00> <ff> endcodespacerange 1 beginbfchar <41> <D800> endbfchar")], b"A"),
        ("cid", b"<< /Type /Font /Subtype /Type0 /BaseFont /Fixture /DescendantFonts [5 0 R] /Encoding 6 0 R /ToUnicode 7 0 R >>",
         [b"<< /Type /Font /Subtype /CIDFontType2 /BaseFont /Fixture /CIDSystemInfo << /Registry (Adobe) /Ordering (Japan1) /Supplement 0 >> /FontDescriptor 8 0 R /DW 600 >>",
          stream(b"/CMapName /Fixture def 1 begincodespacerange <0000> <ffff> endcodespacerange 1 begincidchar <0041> 1 endcidchar",
                 b"/Type /CMap /CMapName /Fixture /CIDSystemInfo << /Registry (Adobe) /Ordering (Japan1) /Supplement 0 >>"),
          stream(b"1 begincodespacerange <0000> <ffff> endcodespacerange 1 beginbfchar <0041> <D800> endbfchar"),
          b"<< /Type /FontDescriptor /FontName /Fixture /Flags 4 /FontBBox [0 0 1000 1000] /ItalicAngle 0 /Ascent 800 /Descent -200 /CapHeight 700 /StemV 80 >>"], b"\0A"),
    ]
    for name, font, objects, data in cases:
        yield Vector("strict-substitution-"+name,{
            ".pdf":pdf(font,objects), ".input":data, ".out":expected([(65,.6,"A")]),
            ".strict.error":b"font.tounicode-utf16\n",
        },("text-strict-repair-boundary",),
        "Strict rejects malformed Unicode before returning a substitute; Repair keeps its outline, PDF width and fallback Unicode",
        {"driver":"substitution-unicode-repair"})


def extraction_cases():
    """Independent Trm arithmetic and explicitly authored reading-order strings.

    Selection cells use PDF advances and descriptor em bounds; Type 3 uses its
    declared FontBBox through FontMatrix. Layout is policy, not ISO typography.
    """
    import re
    from .paint import page, ttf

    def value(item):
        if isinstance(item, (list, tuple)):
            return b"[" + b" ".join(value(x) for x in item) + b"]"
        return scalar(item)

    def transform(p, m):
        x, y = p
        a, b, c, d, e, f = m
        return a*x+c*y+e, b*x+d*y+f

    def records(text, x, y, *, size=10, horizontal: float = 1, character=0, word=0,
                rise=0, matrix=(1,0,0,1,0,0), advance=.5, height: float = 1,
                mode=0, hidden=False, path="", vertical=False, origin=(.25,.8),
                font_matrix=None, unicode_values=None):
        rows=[]
        for index,c in enumerate(text):
            def point(px,py):
                return transform((x+px*size*horizontal,y+py*size+rise),matrix)
            baseline=point(0,0)
            if vertical:
                corners=[(0,0),(advance,0),(advance,1),(0,1)]
                corners=[(px-origin[0],py-origin[1]) for px,py in corners]
            elif font_matrix:
                corners=[transform(p,font_matrix) for p in [(0,0),(500,0),(500,1000),(0,1000)]]
            else:
                corners=[(0,0),(advance,0),(advance,height),(0,height)]
            quad=[n for p in corners for n in point(*p)]
            end=point(0,-1) if vertical else point(advance,0)
            tip=point(0,-1) if vertical else point(1,0)
            perp=point(1,0) if vertical else point(0,1)
            em=((perp[0]-baseline[0])**2+(perp[1]-baseline[1])**2)**.5
            unicode_value=c if unicode_values is None else unicode_values[index]
            rows.append([unicode_value,*quad,size,vertical,mode,hidden,path,*baseline,
                         tip[0]-baseline[0],tip[1]-baseline[1],em,*end])
            spacing=character+(word if c==' ' else 0)
            if vertical:y+=-size+spacing
            else:x+=(advance*size+spacing)*horizontal
        return rows

    differences=b" ".join(b"%d /uni%04X" % (i,i) for i in range(32,127))
    procs=b" ".join(b"/uni%04X 6 0 R" % i for i in range(32,127))
    font=(b"<< /Type /Font /Subtype /Type3 /FontBBox [0 0 500 1000] /FontMatrix [.001 0 0 .001 0 0] "
          b"/FirstChar 32 /LastChar 126 /Widths ["+b"500 "*95+b"] /Encoding << /Differences ["+differences+
          b"] >> /CharProcs << "+procs+b" >> /Resources << >> >>")
    objects=[font,stream(b"500 0 d0 0 0 500 1000 re f")]
    resources=b"<< /Font << /F 5 0 R >> >>"

    def vector(name, content, rows, text, *, source=None, objs=None, res=None, catalog=None, queries=(), hidden_text=None, marks=None):
        pdf_data=page(content,objects if objs is None else objs,resources if res is None else res,size=240)
        if catalog:
            # Rebuild objects to let the PDF writer recompute every xref offset.
            pdf_data=document([b"<< /Type /Catalog /Pages 2 0 R "+catalog+b" >>",
                b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
                b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 240 240] /Resources "+res+b" /Contents 4 0 R >>",  # pyright: ignore[reportOperatorIssue]
                stream(content),*(objects if objs is None else objs)])
        words=[];lines=[];offset=0
        for line in text.split('\n'):
            if line:
                start_word=len(words)
                words.extend([[len(text[:offset+m.start()].encode()),len(text[:offset+m.end()].encode()),len(lines)] for m in re.finditer(r"\w+",line)])
                lines.append([len(text[:offset].encode()),len(text[:offset+len(line)].encode()),start_word,len(words),text[offset+len(line):].startswith("\n\n")])
            offset+=len(line)+1
        files={".pdf":pdf_data,".glyphs":value(rows)+b"\n",".text":text.encode(),
               ".content":(source if source is not None else ''.join(row[0] if row[0] is not None else '\ufffd' for row in rows if not row[12])).encode(),
               ".lines":value(lines)+b"\n",".words":value(words)+b"\n",".queries":value(queries)+b"\n"}
        if hidden_text is not None:files[".hidden-text"]=hidden_text.encode()
        if marks is not None:files[".marked"]=value(marks)+b"\n"
        return Vector("extract-"+name,files,("text-positioned","text-layout","text-search","text-replacement"),
                      "Independent selection geometry, semantic text, layout boundaries and search",{"driver":"text-page"})

    yield vector("spacing",b"BT /F 10 Tf 80 Tz 1 Tc 2 Tw 3 Ts 1 0 0 1 10 30 Tm (A A) Tj ET",
                 records("A A",10,30,horizontal=.8,character=1,word=2,rise=3),"A A")
    yield vector("tj",b"BT /F 10 Tf 10 60 Td [(AB) -100 (CD) -250 (EF)] TJ ET",
                 records("AB",10,60)+records("CD",21,60)+records("EF",33.5,60),"ABCD EF",
                 queries=[['abcd ef',True,True,True,[[0,7]],[[0,1,2,3,4,5]]],['bc',True,True,True,[],[]]])
    yield vector("simple",b"BT /F 10 Tf 10 20 Td (AA) Tj ET",records("ZZ",10,20,advance=.4,height=.6),"ZZ",
                 objs=[ttf()[0][:-2]+b" /ToUnicode 8 0 R >>",*ttf()[1:],stream(b"1 begincodespacerange <00> <ff> endcodespacerange 1 beginbfchar <41> <005A> endbfchar")])
    unknown=font.replace(b"65 /uni0041",b"65 /unknownOpaque").replace(b"/uni0041 6 0 R",b"/unknownOpaque 6 0 R")
    yield vector("unknown",b"BT /F 10 Tf 10 20 Td (ABA) Tj ET",records("ABA",10,20,unicode_values=[None,'B',None]),"\ufffdB\ufffd",
                 objs=[unknown,objects[1]],queries=[['B',False,False,True,[[3,4]],[[1]]],['\ufffd',False,False,True,[],[]]])
    yield vector("rotated",b"q 0 1 -1 0 200 0 cm BT /F 10 Tf 10 20 Td (AB) Tj ET Q",
                 records("AB",10,20,matrix=(0,1,-1,0,200,0)),"AB")
    yield vector("type3-matrix",b"BT /F 10 Tf 10 20 Td (AB) Tj ET",
                 records("AB",10,20,advance=1,font_matrix=(.002,0,0,.003,0,0)),"AB",
                 objs=[font.replace(b"[.001 0 0 .001 0 0]",b"[.002 0 0 .003 0 0]"),objects[1]])
    yield vector("form",b"/Figure BMC q 2 0 0 2 5 7 cm /X Do Q EMC",
                 records("AB",10,20,matrix=(2,0,0,2,11,15),path="Figure"),"AB",
                 objs=[*objects,stream(b"BT /F 10 Tf 10 20 Td (AB) Tj ET",b"/Type /XObject /Subtype /Form /BBox [0 0 200 200] /Matrix [1 0 0 1 3 4]")],
                 res=b"<< /Font << /F 5 0 R >> /XObject << /X 7 0 R >> >>")
    yield vector("ocr",b"BT /F 10 Tf 3 Tr 10 20 Td (OCR) Tj ET",records("OCR",10,20,mode=3),"OCR")
    yield vector("clipped",b"0 0 1 1 re W n BT /F 10 Tf 7 Tr 10 20 Td (AB) Tj ET",records("AB",10,20,mode=7),"AB")
    yield vector("actual-lines",b"BT /F 10 Tf 10 20 Td /Span << /ActualText (a) >> BDC (A) Tj EMC 0 20 Td /Span << /ActualText (b) >> BDC (B) Tj EMC ET",
                 records("A",10,20,path="Span")+records("B",10,40,path="Span"),"ab",source="ab")
    yield vector("named-properties",b"/Span /Replace BDC BT /F 10 Tf 10 20 Td (A) Tj ET EMC",
                 records("A",10,20,path="Span"),"named",source="named",marks=[["Span",9,"named","alternative",0,1,False]],
                 res=b"<< /Font << /F 5 0 R >> /Properties << /Replace << /MCID 9 /ActualText (named) /Alt (alternative) >> >> >>")
    yield vector("negative",b"BT /F -10 Tf 100 80 Td (ABC) Tj ET",records("ABC",100,80,size=-10),"ABC")
    yield vector("reversed",b"/ReversedChars BMC BT /F 10 Tf 10 20 Td (cba) Tj [(fed) (ihg)] TJ ET EMC",
                 records("cbafedihg",10,20,path="ReversedChars"),"abcdefghi",source="abcdefghi")
    content=b"BT /F 10 Tf 10 20 Td /Span << /ActualText <FEFF007300740072006100DF0065> >> BDC (AB) Tj EMC /Span << /ActualText (Z) >> BDC (C) Tj EMC ET"
    yield vector("actual",content,records("ABC",10,20,path="Span"),"straßeZ",source="straßeZ",
                 queries=[['STRASSE',True,False,True,[[0,7]],[[0,1]]],['s',True,False,True,[[0,1]],[[0,1]]],['S',True,True,True,[],[]]])
    content=b"BT /F 10 Tf 10 20 Td /Span << /ActualText (outer) >> BDC (A) Tj /Span << /ActualText (inner) >> BDC (B) Tj EMC EMC /Span << /ActualText () >> BDC (C) Tj EMC ET"
    yield vector("nested",content,records("A",10,20,path="Span")+records("B",15,20,path="Span/Span")+records("C",20,20,path="Span"),"outer",source="outer")
    yield vector("alt",b"BT /F 10 Tf 10 20 Td /Span << /Alt (star) >> BDC (A) Tj EMC /Span << /Alt (moon) >> BDC (B) Tj EMC ET",
                 records("AB",10,20,path="Span"),"star moon",source="star moon")
    yield vector("empty-replacement",b"/Span << /ActualText <EFBBBF1B656E1BE282AC> >> BDC 10 20 5 10 re f EMC",[],"€",source="€")
    yield vector("graphic-replacement",b"BT /F 10 Tf 10 20 Td (L) Tj ET /Span << /ActualText (middle) >> BDC 15 20 2 4 re f EMC BT /F 10 Tf 15 20 Td (R) Tj ET",
                 records("L",10,20)+records("R",15,20),"LmiddleR",source="LmiddleR")
    oc=b"<< /Font << /F 5 0 R >> /Properties << /Hidden 7 0 R >> /XObject << /X 8 0 R >> >>"
    ocobjs=[*objects,b"<< /Type /OCG /Name (hidden) >>",stream(b"BT /F 10 Tf 10 50 Td (D) Tj ET",b"/Type /XObject /Subtype /Form /BBox [0 0 200 200] /OC 7 0 R")]
    yield vector("optional",b"BT /F 10 Tf 10 20 Td (A) Tj /OC /Hidden BDC (B) Tj EMC (C) Tj ET /X Do",
                 records("A",10,20)+records("B",15,20,hidden=True,path="OC")+records("C",20,20)+records("D",10,50,hidden=True),"A C",source="AC",
                 objs=ocobjs,res=oc,catalog=b"/OCProperties << /OCGs [7 0 R] /D << /BaseState /ON /OFF [7 0 R] >> >>",hidden_text="D\n\nABC")
    yield vector("columns",b"BT /F 10 Tf 130 200 Td (Right) Tj 0 -12 Td (side) Tj 1 0 0 1 10 200 Tm (Left) Tj 0 -12 Td (side) Tj ET",
                 records("Right",130,200)+records("side",130,188)+records("Left",10,200)+records("side",10,188),"Left\nside\n\nRight\nside",
                 queries=[['left side',True,True,True,[[0,9]],[[9,10,11,12,13,14,15,16]]]])
    yield vector("marked-order",b"/P BMC BT /F 10 Tf 10 20 Td (first) Tj 0 12 Td (last) Tj ET EMC",
                 records("first",10,20,path="P")+records("last",10,32,path="P"),"first\nlast")
    yield vector("hyphen",b"BT /F 10 Tf 10 100 Td (hyphen-) Tj 0 -12 Td (ated text) Tj 0 -12 Td (across lines) Tj ET",
                 records("hyphen-",10,100)+records("ated text",10,88)+records("across lines",10,76),"hyphen-\nated text\nacross lines",
                 queries=[['hyphenated',True,True,True,[[0,12]],[[0,1,2,3,4,5,6,7,8,9,10]]],['hyphenated',True,False,False,[],[]],['TEXT ACROSS',True,True,True,[[13,24]],[[12,13,14,15,16,17,18,19,20,21]]]])
    # Embedded CIDFont, widths and vertical origins authored here; Unicode is
    # independent of GIDs. The rectangle sfnt supplies only a paintable program.
    base=ttf()
    for vertical in (False,True):
        cmap=b"1 begincodespacerange <0000> <ffff> endcodespacerange 2 beginbfchar <0001> <0041> <0002> <0042> endbfchar"
        font0=b"<< /Type /Font /Subtype /Type0 /BaseFont /Rect /Encoding /Identity-"+(b"V" if vertical else b"H")+b" /DescendantFonts [8 0 R] /ToUnicode 9 0 R >>"
        descendant=b"<< /Type /Font /Subtype /CIDFontType2 /BaseFont /Rect /CIDSystemInfo << /Registry (Adobe) /Ordering (Identity) /Supplement 0 >> /FontDescriptor 6 0 R /CIDToGIDMap /Identity /DW 500 /DW2 [800 -1000] >>"
        yield vector("vertical" if vertical else "composite",b"BT /F 10 Tf 100 100 Td <00010002> Tj ET",
                     records("AB",100,100,vertical=vertical,height=.6),"AB",objs=[font0,*base[1:],descendant,stream(cmap)])


def structure_cases():
    """Logical order comes from an authored /K graph, never a renderer.

    Quads use the declared .001 FontMatrix, 500-unit advance, ten-point size
    and explicit origins. Paragraph separators are the documented copy policy.
    """
    def val(item):
        if isinstance(item, (list, tuple)):
            return b"[" + b" ".join(val(x) for x in item) + b"]"
        return scalar(item)

    def case(name, elements, runs, *, roles=None, actual=None, alt=None,
             malformed=None, split=False, object_form=False, hidden=False, explicit17=False, object_annotation=False, object_image=False, nulls: bool | str = False, empty_root=False, marked=b"", invalid_mcid=None, untagged=False, annotation_flags=0, annotation_subtype="Link", property_list=None):
        roles, actual, alt = roles or {}, actual or {}, alt or {}
        excluded = hidden or bool(annotation_flags & (2|32)) or (bool(annotation_flags & 1) and annotation_subtype == "Custom")
        ids = {key: 12+i for i,(key,_,_) in enumerate(elements)}
        parent = {kid:key for key,_,kids in elements for kid in kids if isinstance(kid,str)}
        entries = {key:(role,kids) for key,role,kids in elements}
        ancestors = {}
        for key,_,_ in elements:
            ancestors[key] = parent.get(key, "document")
        charprocs = b" ".join(b"/uni%04X 6 0 R" % i for i in range(32,127))
        encoding = b" ".join(b"%d /uni%04X" % (i,i) for i in range(32,127))
        font = (b"<< /Type /Font /Subtype /Type3 /FontBBox [0 0 500 1000] /FontMatrix [.001 0 0 .001 0 0] "
                b"/FirstChar 32 /LastChar 126 /Widths [" + b"500 "*95 + b"] /Encoding << /Differences [" + encoding +
                b"] >> /CharProcs << " + charprocs + b" >> /Resources << >> >>")
        contents = {"page":[], "form":[], "second":[]}
        glyphs=[]; source=""; run_text={}; run_indices={}
        for owner,mcid,text,x,y in runs:
            prefix = b"/Artifact BMC " if mcid is None else b"/P << /MCID %d >> BDC " % mcid
            if mcid == -1: prefix=b""
            if mcid is not None and mcid != -1:
                if invalid_mcid is not None: prefix = b"/P << /MCID " + invalid_mcid + marked + b" >> BDC "
                elif marked: prefix = prefix.replace(b" >> BDC ", marked+b" >> BDC ")
            if invalid_mcid == b"99999 0 R": prefix=b"/P /MCIDProperties BDC "
            if property_list is not None: prefix=b"/P "+(b"/InvalidProperties" if property_list==b"42" else property_list)+b" BDC "
            command = prefix + b"BT /F 10 Tf 1 0 0 1 %d %d Tm (" % (x,y) + text.encode() + b") Tj ET" + (b"" if mcid == -1 else b" EMC")
            contents[owner].append(command)
            container = 8 if owner=="form" else 3
            key=("form" if owner=="form" else "page",(0 if object_form and owner=="form" else mcid))
            run_indices[key]=list(range(len(glyphs),len(glyphs)+len(text)))
            for index,c in enumerate(text):
                gx=x+index*5
                glyphs.append([c, [gx,y,gx+5,y,gx+5,y+10,gx,y+10], container,None if mcid == -1 or invalid_mcid is not None or property_list is not None else mcid])
            source+=text
            run_text[("form" if owner=="form" else "page",(0 if object_form and owner=="form" else mcid))]=text
        # Stream ordering, rather than dictionary/object numbering, determines
        # glyph indices. Forms are invoked between the two page streams.
        first=b"\n".join(contents["page"])
        second=b"\n".join(contents["second"])
        if (contents["form"] or object_form) and not object_annotation and malformed!="appearance": first += b" /X Do"
        if split:
            first=b"/P << /MCID 0 >> BDC BT /F 10 Tf 10 20 Td (A) Tj ET"
            second=b"BT /F 10 Tf 15 20 Td (B) Tj ET EMC"
            glyphs=[[c,[x,20,x+5,20,x+5,30,x,30],3,0] for c,x in [("A",10),("B",15)]]
            source="AB"; run_text[("page",0)]="AB"; run_indices[("page",0)]=[0,1]
        root_kids=[key for key,_,_ in elements if key not in parent]
        page_parents={};form_parents={}
        for key,_,kids in elements:
            for kid in kids:
                if isinstance(kid,tuple):
                    kind,mcid=kid
                    if kind=="m": page_parents[mcid]=ids[key]
                    elif kind=="f":form_parents[mcid]=ids[key]
        def reference(number): return b"%d 0 R" % number
        def parent_array(table):
            return b"["+b" ".join(reference(table[i]) if i in table else b"null" for i in range(max(table,default=-1)+1))+b"]"
        nums=b"0 "+parent_array(page_parents)
        if form_parents:nums+=b" 1 "+parent_array(form_parents)
        if object_form:
            owner=next(ids[k] for k,_,kids in elements if ("o",0) in kids)
            nums+=b" 1 "+reference(owner)
        role_map=b" /RoleMap << "+b" ".join(b"/"+a.encode()+b" /"+b.encode() for a,b in roles.items())+b" >>" if roles else b""
        struct_root=b"<< /Type /StructTreeRoot /K 10 0 R /ParentTree 11 0 R"+role_map+b" >>"
        objects=[b"<< /Type /Catalog /Pages 2 0 R /StructTreeRoot 9 0 R /MarkInfo << /Marked true >> >>",
                 b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
                 b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 120 120] /StructParents 0 /Contents [4 0 R 7 0 R] "
                 b"/Resources << /Font << /F 5 0 R >> /XObject << /X 8 0 R >> >> >>",
                 stream(first), font, stream(b"500 0 d0 0 0 500 1000 re f"),stream(second),
                 stream(b"\n".join(contents["form"]), b"/Type /XObject /Subtype /Form /BBox [0 0 120 120] /Resources << /Font << /F 5 0 R >> >> " +
                        (b"/StructParent 1" if object_form else b"/StructParents 1")),
                 struct_root,b"<< /Type /StructElem /S /Document /P 9 0 R /K ["+b" ".join(reference(ids[k]) for k in root_kids)+b"] >>",
                 b"<< /Nums ["+nums+b"] >>"]
        for key,role,kids in elements:
            def kid_value(kid):
                if isinstance(kid,str):return reference(ids[kid])
                kind,mcid=kid
                if kind=="m":return str(mcid).encode()
                if kind=="f":return b"<< /Type /MCR /Pg 3 0 R /Stm 8 0 R /MCID %d >>" % mcid
                return b"<< /Type /OBJR /Pg 3 0 R /Obj 8 0 R >>"
            properties=b""
            if key in actual:properties+=b" /ActualText "+scalar(actual[key])
            if key in alt:properties+=b" /Alt "+scalar(alt[key])
            objects.append(b"<< /Type /StructElem /S /"+role.encode()+b" /P "+reference(ids[parent[key]] if key in parent else 10)+
                           b" /Pg 3 0 R /K ["+b" ".join(kid_value(k) for k in kids)+b"]"+properties+b" >>")
        if invalid_mcid == b"99999 0 R":
            objects[2]=objects[2].replace(b"/Resources <<",b"/Resources << /Properties << /MCIDProperties << /MCID 99999 0 R >> >>",1)
        if property_list == b"42":
            objects[2]=objects[2].replace(b"/Resources <<",b"/Resources << /Properties << /InvalidProperties 42 >>",1)
        if name=="negative-parent-key":
            objects[2]=objects[2].replace(b"/StructParents 0",b"/StructParents -7")
            objects[10]=objects[10].replace(b"/Nums [0 ",b"/Nums [-7 ")
        if name=="indirect-types":
            n=len(objects)+1
            objects[8]=objects[8].replace(b"/Type /StructTreeRoot",b"/Type "+reference(n))
            objects[9]=objects[9].replace(b"/Type /StructElem",b"/Type "+reference(n+1))
            objects[11]=objects[11].replace(b"/Type /StructElem",b"/Type "+reference(n+1)).replace(b"/K [0]",b"/K [<< /Type "+reference(n+2)+b" /Pg 3 0 R /MCID 0 >>]")
            objects.extend([b"/StructTreeRoot",b"/StructElem",b"/MCR"])
        if malformed=="type":objects[11]=objects[11].replace(b"/Type /StructElem",b"/Type false")
        if nulls or empty_root:
            nil=b"null"
            if nulls=="indirect":
                nil=reference(len(objects)+1);objects.append(b"null")
            objects[8]=objects[8][:-2]+b" /RoleMap "+nil+b" /Namespaces "+nil+b" >>"
            objects[9]=objects[9][:-2]+b" /ActualText "+nil+b" /Alt "+nil+b" /NS "+nil+b" >>"
            if nulls:
                objects[2]=objects[2][:-2]+b" /StructParent "+nil+b" >>"
                objects[11]=objects[11].replace(b"/K [0]",b"/K [<< /Type /MCR /Pg "+nil+b" /Stm "+nil+b" /MCID 0 >>]")
                objects[11]=objects[11][:-2]+b" /NS "+nil+b" /ActualText "+nil+b" /Alt "+nil+b" >>"
                nested=first.replace(b" BDC ",b" BDC /Span << /MCID null /ActualText null /Alt null >> BDC ",1)+b" EMC"
                objects[3]=stream(nested)
            if empty_root:
                objects[8]=objects[8].replace(b"/K 10 0 R /ParentTree 11 0 R",b"/K null /ParentTree null")
                objects[2]=objects[2].replace(b"/StructParents 0",b"/StructParents null")
        if malformed=="crosspage":
            other=len(objects)+1
            objects[1]=b"<< /Type /Pages /Kids [3 0 R "+reference(other)+b"] /Count 2 >>"
            objects[11]=objects[11].replace(b"/K [0]",b"/K [0 << /Type /MCR /Pg "+reference(other)+b" /MCID 0 >>]")
            objects[10]=objects[10].replace(b"] >>",b" 2 [12 0 R]] >>")
            objects.append(b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 120 120] /Resources << /Font << /F 5 0 R >> >> /StructParents 2 /Contents "+reference(other+1)+b" >>")
            objects.append(stream(b"/P << /MCID 0 >> BDC BT /F 10 Tf 10 90 Td (B) Tj ET EMC"))
        if malformed=="appearance":
            annotation=len(objects)+1
            objects[2]=objects[2][:-2]+b" /Annots ["+reference(annotation)+b"] >>"
            objects.append(b"<< /Type /Annot /Subtype /Link /Rect [0 0 120 120] /Border [0 0 0] /P 3 0 R /AP << /N 8 0 R >> >>")
        if name=="role-loop":objects[0]=objects[0].replace(b"/Marked true",b"/Marked false")
        if object_image:
            objects[7]=stream(b"\0",b"/Type /XObject /Subtype /Image /Width 1 /Height 1 /BitsPerComponent 8 /ColorSpace /DeviceGray /StructParent 1")
        if object_annotation:
            objects[7]=b"<< /Type /Annot /Subtype /"+annotation_subtype.encode()+b" /F %d /Rect [0 0 20 20] /Border [0 0 0] /P 3 0 R /StructParent 1 >>" % annotation_flags
            objects[2]=objects[2][:-2]+b" /Annots [8 0 R] >>"
        if hidden:
            ocg=len(objects)+1
            objects[0]=objects[0][:-2]+b" /OCProperties << /OCGs ["+reference(ocg)+b"] /D << /BaseState /ON /OFF ["+reference(ocg)+b"] >> >> >>"
            if object_form:
                subtype=b"/Subtype /Image" if object_image else b"/Subtype /Link" if object_annotation else b"/Subtype /Form"
                objects[7]=objects[7].replace(subtype,subtype+b" /OC "+reference(ocg))
            else:
                first=b"/OC "+reference(ocg)+b" BDC "+first+b" EMC"
                # A direct OCG dictionary is a property list, not a stream reference.
                first=first.replace(b"/OC "+reference(ocg)+b" BDC",b"/OC /Hidden BDC")
                objects[2]=objects[2].replace(b"/Resources <<",b"/Resources << /Properties << /Hidden "+reference(ocg)+b" >>",1)
                objects[3]=stream(first)
            objects.append(b"<< /Type /OCG /Name (Hidden) >>")
        modern={key for key,role,_ in elements if role=="Artifact" or (role.startswith("H") and role[1:].isdigit() and int(role[1:])>6)}
        namespace_keys=modern | ({elements[0][0]} if explicit17 or malformed in ("namespace","rolemapns") else set())
        if namespace_keys:
            ns=len(objects)+1
            objects[8]=objects[8][:-2]+b" /Namespaces ["+reference(ns)+b"] >>"
            for key in namespace_keys:
                objects[ids[key]-1]=objects[ids[key]-1][:-2]+b" /NS "+reference(ns)+b" >>"
            uri="custom" if malformed=="namespace" else "http://iso.org/pdf/ssn" if explicit17 else "http://iso.org/pdf2/ssn"
            objects.append(b"<< /Type /Namespace /NS "+scalar(uri)+(b" /RoleMapNS << >>" if malformed=="rolemapns" else b"")+b" >>")
        if malformed=="root":
            objects[0]=objects[0].replace(b" /StructTreeRoot 9 0 R",b"")
            objects[2]=objects[2].replace(b" /StructParents 0",b"")
        if malformed=="parent":objects[10]=b"<< /Nums [0 [10 0 R]] >>"
        if malformed=="cycle":objects[11]=objects[11].replace(b"/K [0]",b"/K [12 0 R]")

        if untagged:
            objects[0]=objects[0].replace(b" /StructTreeRoot 9 0 R",b"").replace(b" /MarkInfo << /Marked true >>",b"")
            objects[2]=objects[2].replace(b" /StructParents 0",b"")

        def standard(role):
            seen=set()
            # These generated roles either end in a known role or a valid loop.
            if role in roles and not explicit17:role=roles[role]
            while not explicit17 and role in roles and role not in ("P","Span","Sect","Document","Artifact") and role not in seen:
                seen.add(role);role=roles[role]
            return role if role in ("P","Span","Sect","Document","Artifact") or (role.startswith("H") and role[1:].isdigit() and not role[1:].startswith("0")) else None
        def block_for(key):
            while key!="document":
                if standard(entries[key][0])=="P":return key
                key=parent.get(key,"document")
            return None
        # Independently traverse the generated hierarchy and assemble text and
        # element ranges. Paragraph leaf nodes are separated by two LFs.
        order=[]; chunks=[]; output=""; logical_chars=[]
        def emit(value, indices=(), replacement=False, inserted=False):
            nonlocal output
            for offset,c in enumerate(value):
                start=len(output.encode()); output+=c
                source_range=(min(indices),max(indices)+1) if replacement and indices else ((indices[offset],indices[offset]+1) if indices else (0,0))
                logical_chars.append([c,start,len(output.encode()),*source_range,inserted])
        def associated(key):
            result=[]
            for child in entries[key][1]:
                if isinstance(child,str):
                    if standard(entries[child][0])!="Artifact":result.extend(associated(child))
                else:
                    kind,mcid=child
                    result.extend(run_indices.get(("form" if kind in ("f","o") else "page",mcid),[]))
            return result
        def walk(key, parent_index):
            nonlocal output
            role,kids=("Document",root_kids) if key=="document" else entries[key]
            role_type=standard(role)
            if role_type=="Artifact":return
            index=len(order);order.append([10 if key=="document" else ids[key],parent_index,role,role_type,0,0,key in modern])
            if key in actual or key in alt:
                if output and key in alt and not output[-1].isspace():emit(" ",inserted=True)
                start=len(output.encode());emit("" if excluded else actual.get(key,alt.get(key,"")),associated(key),replacement=True);end=len(output.encode());chunks.append(block_for(key))
                order[index][4:6]=[start,end]
                def suppressed(child,p):
                    if isinstance(child,tuple):return
                    r,ks=entries[child]
                    if standard(r)=="Artifact":return
                    n=len(order);order.append([ids[child],p,r,standard(r),start,start,child in modern])
                    for sub in ks:suppressed(sub,n)
                for child in kids:suppressed(child,index)
                return
            starts=[];ends=[]
            for child in kids:
                before=len(order)
                if isinstance(child,str):
                    walk(child,index)
                    if len(order)>before:
                        starts.append(order[before][4]);ends.append(order[before][5])
                else:
                    kind,mcid=child
                    value=run_text.get(("form" if kind in ("f","o") else "page",mcid),"")
                    if value:
                        if output and chunks and chunks[-1]!=block_for(key) and block_for(key) is not None:emit("\n\n",inserted=True)
                        starts.append(len(output.encode()));emit(value,run_indices.get(("form" if kind in ("f","o") else "page",mcid),[]));ends.append(len(output.encode()));chunks.append(block_for(key))
            order[index][4:6]=[min(starts,default=len(output.encode())),max(ends,default=len(output.encode()))]
        walk("document",-1)
        if empty_root:order=[];output="";logical_chars=[]
        # Explicit horizontal selection-cell arithmetic for the page view.
        # Unlike the logical oracle, this includes every artifact glyph.
        visible = [] if hidden or malformed == "appearance" else glyphs
        geometric = ""; previous = None
        for c, quad, container, mcid in sorted(visible, key=lambda g: (-g[1][1],g[1][0])):
            if previous is not None:
                x,y = quad[:2]; px,py = previous
                if y != py: geometric += "\n\n" if abs(y-py)>18 else "\n"
                elif x-px>2: geometric += " "
            geometric += c; previous=(quad[2],quad[1])
        files={".pdf":document(objects).replace(b"%PDF-1.7",b"%PDF-2.0",1),".text":geometric.encode(),".logical":output.encode(),".logical-characters":val(logical_chars)+b"\n",".content":("" if hidden else source).encode(),
               ".structure":val(order)+b"\n",".glyphs":val(glyphs)+b"\n"}
        if malformed:files[".error"]=("cycle" if malformed=="cycle" else "text.structure-namespace" if malformed in ("namespace","rolemapns") else "text.structure-replacement" if malformed in ("replacement","crosspage") else "limit" if malformed=="depth" else "text.structure-content" if malformed=="appearance" else "syntax").encode()
        if malformed:
            files[".fallback"] = geometric.encode()
            files[".repair"] = b"text.structure-fallback"
        if marked or property_list is not None: files[".property-repair"] = b"text.marked-property"
        if untagged: files[".unstructured"] = b""
        if malformed or untagged:
            files[".logical"] = b""
            files[".logical-characters"] = val([])+b"\n"

        return Vector("tagged-"+name,files,("text-structure","text-structure-references","text-structure-replacement","text-structure-fallback","text-logical-view","text-marked-properties","text-annotation-references"),
                      "Independent logical /K graph, ParentTree ownership, replacement text and selection geometry",{"driver":"text-structure"})

    yield case("nested-order",[("section","Sect",["second","first"]),("first","P",[("m",0)]),("second","P",[("m",1)])],
               [("page",0,"First",10,90),("page",1,"Second",10,70)])
    yield case("stream-span",[("p","P",[("m",0)])],[],split=True)
    yield case("form-mcid",[("form","P",[("f",0)]),("page","P",[("m",0)])],
               [("page",0,"Page",10,90),("form",0,"Form",10,50)])
    yield case("object-reference",[("form","P",[("o",0)]),("page","P",[("m",0)])],
               [("page",0,"Page",10,90),("form",-1,"Form",10,50)],object_form=True)
    yield case("role-artifact",[("first","Custom",[("m",1)]),("artifact","Artifact",[("m",2)]),("last","P",[("m",0)])],
               [("page",0,"Last",10,90),("page",1,"First",10,70),("page",2,"Footer",10,10),("page",None,"Decoration",10,5)],roles={"Custom":"Intermediate","Intermediate":"P"})
    yield case("actual-text",[("outer","Sect",["child"]),("child","P",[("m",0)])],
               [("page",0,"Bad",10,90)],actual={"outer":"Correct"},alt={"outer":"ignored"})
    yield case("alt",[("figure","Span",[("o",0)])],[],object_form=True,alt={"figure":"Illustration"})
    yield case("actual-adjacent",[("first","P",[("m",0)]),("last","P",[("m",1)])],
               [("page",0,"XX",10,90),("page",1,"YY",10,10)],actual={"first":"A","last":"B"})
    yield case("role-loop",[("custom","Custom",[("m",0)])],[("page",0,"Text",10,90)],roles={"Custom":"Other","Other":"Custom"})
    yield case("standard-role-remap",[("custom","P",[("m",0)])],[("page",0,"Text",10,90)],roles={"P":"Span"})
    yield case("mixed-kids",[("p","P",[("m",2),"span",("m",0)]),("span","Span",[("m",1)])],
               [("page",0,"A",10,90),("page",1,"B",15,90),("page",2,"C",20,90)])
    yield case("empty-actual",[("p","P",[("m",0)])],[("page",0,"Remove",10,90)],actual={"p":""},alt={"p":"ignored"})
    yield case("hidden-actual",[("p","P",[("m",0)])],[("page",0,"Secret",10,90)],actual={"p":"Secret"},hidden=True)
    yield case("hidden-object-alt",[("figure","Span",[("o",0)])],[],object_form=True,alt={"figure":"Hidden"},hidden=True)
    yield case("unsupported-replacement",[("outer","Sect",["first","last"]),("first","P",[("m",0)]),("middle","P",[("m",1)]),("last","P",[("m",2)])],
               [("page",0,"A",10,90),("page",1,"B",10,70),("page",2,"C",10,50)],actual={"outer":"Combined"},malformed="replacement")
    yield case("heading-seven",[("heading","H7",[("m",0)])],[("page",0,"Heading",10,90)])
    yield case("namespace-pdf17",[("p","P",[("m",0)])],[("page",0,"Text",10,90)],roles={"P":"Span"},explicit17=True)
    yield case("annotation-alt",[("link","Span",[("o",0)])],[],object_form=True,object_annotation=True,alt={"link":"Link"})
    yield case("hidden-annotation-alt",[("link","Span",[("o",0)])],[],object_form=True,object_annotation=True,alt={"link":"Hidden"},hidden=True)
    yield case("malformed-role",[("custom","UnknownCustomRole",[("m",0)])],[("page",0,"Text",10,90)],malformed="role")
    yield case("unsupported-appearance",[("p","P",[("f",0)])],[("form",0,"Text",10,50)],malformed="appearance")
    yield case("unsupported-crosspage-replacement",[("p","P",[("m",0)])],[("page",0,"A",10,90)],actual={"p":"Across"},malformed="crosspage")
    yield case("image-alt",[("figure","Span",[("o",0)])],[],object_form=True,object_image=True,alt={"figure":"Image"})
    yield case("hidden-image-alt",[("figure","Span",[("o",0)])],[],object_form=True,object_image=True,alt={"figure":"Hidden"},hidden=True)
    yield case("null-optionals",[("p","P",[("m",0)])],[("page",0,"Text",10,90)],nulls=True)
    yield case("indirect-null-optionals",[("p","P",[("m",0)])],[("page",0,"Text",10,90)],nulls="indirect")
    yield case("null-kids",[],[],empty_root=True)
    yield case("negative-parent-key",[("p","P",[("m",0)])],[("page",0,"Text",10,90)])
    yield case("indirect-types",[("p","P",[("m",0)])],[("page",0,"Text",10,90)])
    yield case("malformed-type",[("p","P",[("m",0)])],[("page",0,"Text",10,90)],malformed="type")
    deep=[("s%d" % i,"Sect",["s%d" % (i+1)]) for i in range(70)]+[("s70","P",[("m",0)])]
    yield case("deep-tree",deep,[("page",0,"Text",10,90)],malformed="depth")
    for fault in ("parent","cycle","namespace","root"):
        yield case("malformed-"+fault,[("p","P",[("m",0)])],[("page",0,"A",10,20)],malformed=fault)
    yield case("fallback-malformed",[("p","P",[("m",0)])],[("page",0,"Copy",10,90)],malformed="parent")
    yield case("fallback-unsupported",[("p","P",[("m",0)])],[("page",0,"Search",10,90)],malformed="namespace")
    yield case("fallback-rolemapns",[("p","P",[("m",0)])],[("page",0,"Copy",10,90)],malformed="rolemapns")
    yield case("fallback-partial",[("early","P",[("m",0)]),("outer","Sect",["first","last"]),("first","P",[("m",1)]),("middle","P",[("m",2)]),("last","P",[("m",3)])],
               [("page",0,"Before",10,110),("page",1,"A",10,90),("page",2,"B",10,70),("page",3,"C",10,50)],actual={"outer":"Combined"},malformed="replacement")
    yield case("all-artifact",[("artifact","Artifact",[("m",0)])],[("page",0,"Footer",10,90)])
    for name,value in [("actual",b" /ActualText 42"),("alt",b" /Alt false"),("expansion",b" /E /Invalid"),("invalid-string",b" /ActualText <FEFF00>")]:
        yield case("property-"+name,[("p","P",[("m",0)])],[("page",0,"Copy",10,90)],marked=value)
    for name,value in [("name",b"/Invalid"),("negative",b"-1"),("boolean",b"false"),("missing",b"99999 0 R")]:
        yield case("mcid-"+name,[("p","P",[("m",0)])],[("page",0,"Copy",10,90)],invalid_mcid=value,malformed="mcid")
        yield case("mcid-unused-"+name,[("p","P",[("m",0)])],[("page",0,"Copy",10,90)],invalid_mcid=value,untagged=True)
    for name,flags,subtype in [("hidden",2,"Link"),("no-view",32,"Link"),("invisible-custom",1,"Custom"),("invisible-standard",1,"Link")]:
        yield case("annotation-"+name,[("link","Span",[("o",0)])],[],object_form=True,object_annotation=True,annotation_flags=flags,annotation_subtype=subtype,alt={"link":"Link"})
    for name,value in [("integer",b"42"),("missing",b"/UnknownProperties")]:
        yield case("property-list-"+name,[("p","P",[("m",0)])],[("page",0,"Copy",10,90)],untagged=True,property_list=value)
