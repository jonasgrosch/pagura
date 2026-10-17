"""Clause 9 font dictionaries and text vectors, independently authored.

All expectations derive from explicit code/name/width tuples below and the
ISO 32000-2 mapping rules. No product executable is imported or invoked.
"""
from __future__ import annotations

import struct

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
    yield from annex_cases()
    yield from primary_regression_cases()
    yield from program_cases()
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


def embedded_document(program, *, subtype=b"TrueType", fontfile=b"/FontFile2", font_extra=b"", stream_extra=b"", encoding=b"/WinAnsiEncoding", flags=32, first=32, widths=None, cmap=None, cmap_extra=b"", unicode_map=None, cid_map=None, descendant_extra=b"", cid_subtype=b"CIDFontType2"):
    """PDF-level bindings; the independent font writer supplies program bytes."""
    widths = widths if widths is not None else [600] * 192
    encoding_entry = b" /Encoding " + encoding if encoding else b""
    if subtype == b"Type0":
        root = b"<< /Type /Font /Subtype /Type0 /BaseFont /Fixture /DescendantFonts [7 0 R]" + encoding_entry
    else:
        root = b"<< /Type /Font /Subtype /" + subtype + b" /BaseFont /Fixture /FontDescriptor 5 0 R" + encoding_entry
        root += b" /FirstChar %d /LastChar %d /Widths [" % (first,first+len(widths)-1) + b" ".join(str(w).encode() for w in widths) + b"]"
    root += (b" /ToUnicode 9 0 R" if unicode_map is not None else b"") + b" " + font_extra + b" >>"
    descriptor = b"<< /Type /FontDescriptor /FontName /Fixture /Flags %d /FontBBox [0 0 300 300] /ItalicAngle 0 /Ascent 800 /Descent -200 /CapHeight 700 /StemV 80 " % flags + fontfile + b" 6 0 R >>"
    descendant = b"<< /Type /Font /Subtype /" + cid_subtype + b" /BaseFont /Fixture /CIDSystemInfo << /Registry (Adobe) /Ordering (Identity) /Supplement 0 >> /FontDescriptor 5 0 R " + descendant_extra
    if cid_subtype == b"CIDFontType2":
        descendant += b" /CIDToGIDMap 10 0 R" if cid_map is not None else b" /CIDToGIDMap /Identity"
    descendant += b" >>"
    return pdf(root, [descriptor, stream(program,stream_extra), descendant,
                     stream(cmap,b"/Type /CMap /CMapName /Fixture /CIDSystemInfo << /Registry (Adobe) /Ordering (Identity) /Supplement 0 >> " + cmap_extra) if cmap is not None else b"null",
                     stream(unicode_map) if unicode_map is not None else b"null",
                     stream(cid_map) if cid_map is not None else b"null"])


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
    yield tt("tt-symbol-present-missing",[(3,0,{66:1}),(1,0,{65:2})],rows=[row(65,0)],flags=4,encoding=None)
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


def bundled_cases():
    import json
    from pathlib import Path
    from .font import truetype
    data = Path(__file__).resolve().parents[2]/"data"
    lock = json.loads((data/"lock.json").read_text())
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
    for base, item in lock["standard_14"].items():
        props,glyphs = bundled_afm(data/item["metrics"])
        widths = {name:width for _,name,width in glyphs}
        encoding = {code:name for code,name,_ in glyphs if code >= 0}
        # These sample names have explicit known Unicode values; the complete
        # AFM name/width/code set is separately checked without assuming AGL.
        sample_unicode = {"space":" ","A":"A","a":"a","fi":"ﬁ","Alpha":"Α","alpha":"α"}
        samples = [32,65,97,174,0]
        outlines = item["outline"]
        rows = [(code,widths.get(encoding.get(code),0)/1000,
                 sample_unicode.get(encoding.get(code)),
                 bundled_tt_glyph(data/outlines,ord(sample_unicode[encoding[code]]))
                 if outlines and encoding.get(code) in sample_unicode else 0)
                for code in samples]
        variants = [(base,base),(base+"-subset","ABCDEF+"+base)]
        variants += [(base+"-alias-%d" % i,name) for i,name in enumerate(aliases.get(base,[]))]
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
            yield Vector(fixture,files,("text-standard-fonts","text-provider"),"AFM-derived standard font widths, encoding, descriptors and substitute glyphs",{"driver":"bundled-standard"})
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
