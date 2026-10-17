"""Small independently written font programs and analytic glyph-space oracles.

PDF program constraints are checked against local ISO 32000-2 clause 9.9
and 9.6.5.4. External binary-layout interpretations remain pending primary-text
review of OpenType and Adobe TN 5176/5177. No engine is used to construct expected outlines.
"""
import struct
from .model import Vector

AREA = "font"


def u16(n):
    return struct.pack(">H", n)


def i16(n):
    return struct.pack(">h", n)


def u32(n):
    return struct.pack(">I", n)


RECT = "m 0 0\nl 100 0\nl 100 200\nl 0 200\nh\n"
QUAD = "m 0 0\nc 0 200 100 200 300 0\nh\n"


def simple(points, contours=None):
    contours = contours or [len(points)-1]
    header = i16(len(contours)) + struct.pack(">hhhh", 0, 0, 300, 300)
    body = b"".join(u16(n) for n in contours) + u16(0)
    body += bytes(int(on) for _, _, on in points)
    for axis in (0, 1):
        previous = 0
        for point in points:
            body += i16(point[axis]-previous)
            previous = point[axis]
    return header + body


def cmap(fmt):
    if fmt == 0:
        sub = u16(0)+u16(262)+u16(0)+bytes(1 if i == 65 else 0 for i in range(256))
        platform, encoding = 1, 0
    elif fmt == 6:
        sub = u16(6)+u16(12)+u16(0)+u16(65)+u16(1)+u16(1)
        platform, encoding = 3, 0
    elif fmt == 4:
        # A..B use idRangeOffset and nonzero idDelta; glyph zero stays zero.
        sub = (u16(4)+u16(36)+u16(0)+u16(4)+u16(4)+u16(1)+u16(0)
               +u16(66)+u16(65535)+u16(0)+u16(65)+u16(65535)
               +i16(-1)+i16(1)+u16(4)+u16(0)+u16(2)+u16(0))
        platform, encoding = 3, 1
    else:
        sub = u16(12)+u16(0)+u32(28)+u32(0)+u32(1)+u32(0x1f600)+u32(0x1f601)+u32(1)
        platform, encoding = 3, 10
    return u16(0)+u16(1)+u16(platform)+u16(encoding)+u32(12)+sub


def sfnt(tables, otto=False):
    offset = 12+16*len(tables)
    directory = b""
    body = b""
    for tag, data in sorted(tables.items()):
        directory += tag+u32(0)+u32(offset)+u32(len(data))
        padded = data+bytes((-len(data)) % 4)
        body += padded
        offset += len(padded)
    return (b"OTTO" if otto else u32(0x10000))+u16(len(tables))+bytes(6)+directory+body


def truetype(fmt=4, short=False, glyph_override=None, cff=None, cff2=False, table_overrides=None, omit_tables=()):
    head = bytearray(54)
    head[:4] = u32(0x10000)
    head[12:16] = u32(0x5f0f3cf5)
    head[18:20] = u16(1000)
    head[50:52] = i16(0 if short else 1)
    hhea = u32(0x10000)+bytes(30)+u16(2)
    names = [b"A", b"curve", b"composite", b"attached"]
    post = u32(0x20000)+bytes(28)+u16(5)+u16(0)+b"".join(u16(258+i) for i in range(4))
    post += b"".join(bytes([len(n)])+n for n in names)
    rect = simple([(0,0,True),(100,0,True),(100,200,True),(0,200,True)])
    curve = simple([(0,0,True),(0,300,False),(300,0,True)])
    composite = i16(-1)+bytes(8)+u16(0x83)+u16(1)+i16(20)+i16(30)+i16(8192)+i16(4096)+i16(0)+i16(8192)
    attached = i16(-1)+bytes(8)+u16(0x23)+u16(1)+i16(0)+i16(0)+u16(1)+u16(1)+u16(2)+u16(0)
    glyphs = [b"", rect, curve, composite, attached]
    if glyph_override is not None:
        glyphs[1] = glyph_override
    if cff is not None:
        glyph_count = 5
        tables = {b"CFF2" if cff2 else b"CFF ": cff}
    else:
        glyph_count = len(glyphs)
        loca = b""
        glyf = b""
        for glyph in glyphs + [b""]:
            loca += u16(len(glyf)//2) if short else u32(len(glyf))
            glyf += glyph+bytes(len(glyph) % 2)
        tables = {b"glyf": glyf, b"loca": loca}
    tables.update({b"head": bytes(head), b"hhea": hhea,
                   b"maxp": u32(0x5000 if cff is not None else 0x10000)+u16(glyph_count)+(b"" if cff is not None else bytes(26)),
                   b"hmtx": u16(500)+i16(0)+u16(600)+i16(0)+bytes((glyph_count-2)*2),
                   b"cmap": cmap(fmt), b"post": post})
    tables.update(table_overrides or {})
    for tag in omit_tables:
        tables.pop(tag,None)
    return sfnt(tables, otto=cff is not None)


def number(n):
    if -107 <= n <= 107:
        return bytes([n+139])
    if 108 <= n <= 1131:
        return bytes([247+(n-108)//256, (n-108) % 256])
    if -1131 <= n <= -108:
        return bytes([251+(-n-108)//256, (-n-108) % 256])
    return b"\x1c"+i16(n)


def operands(*values):
    return b"".join(number(n) for n in values)


def index(items, cff2=False):
    count = u32(len(items)) if cff2 else u16(len(items))
    if not items:
        return count
    # Fixed four-byte offsets simplify the independent writer.
    offsets = [1]
    for item in items:
        offsets.append(offsets[-1]+len(item))
    return count+b"\x04"+b"".join(u32(n) for n in offsets)+b"".join(items)


def offset(n):
    return b"\x1d"+u32(n)


def cff_font(cid=False, cff2=False, hostile=None, variable=False, variant=None, font_set=False, line_count=0):
    end = b"" if cff2 else b"\x0e"
    rect_subr = operands(100,0,0,200,-100,0)+b"\x05"+(b"" if cff2 else b"\x0b")
    rect = operands(0,0)+b"\x15"+number(-107)+b"\x0a"+end
    curve = operands(0,0)+b"\x15"+operands(0,200,100,0,200,-200)+b"\x08"+end
    # A + acute, StandardEncoding codes, with an independently chosen offset.
    accent = operands(0,0)+b"\x15"+operands(20,40)+b"\x05"+end
    seac = operands(30,200,65,194)+end if not cff2 and not cid else rect
    if variant == "hints":
        rect = operands(20,0,10)+b"\x01\x13\x00"+rect
    if variant == "flex":
        rect = operands(0,0)+b"\x15"+operands(10,10,10,10,10,10,10)+b"\x0c\x22"+end
    if variant == "arithmetic":
        rect = operands(0,0)+b"\x15"+operands(20,5)+b"\x0c\x18"+operands(0,0,200,-100,0)+b"\x05"+end
    if variable:
        blend_values = (100,30,1) if variant == "private-blend" else (100,50,30,1)
        selection = operands(0)+b"\x0f" if variant == "private-blend-override" else b""
        rect = selection+operands(0,0)+b"\x15"+operands(*blend_values)+b"\x10"+operands(0,0,200,-130,0)+b"\x05"
    if line_count:
        if variant == "linear-subr":
            rect_subr = b"\x05\x0b"
            rect = operands(0,0)+b"\x15"+(operands(1,0,-107)+b"\x0a")*line_count+end
        else:
            rect = operands(0,0)+b"\x15"+(operands(1,0)+b"\x05")*line_count+end
    if variant == "branching":
        rect = operands(0,0)+b"\x15"+number(-107)+b"\x0a"+end
    glyphs = [end, rect, curve, seac, accent]
    if hostile == "recursive":
        rect_subr = number(-107)+b"\x0a\x0b"
    if hostile == "stack":
        glyphs[1] = number(0)*514+end
    if hostile == "mask":
        glyphs[1] = operands(0,10)+b"\x01\x13"  # Missing one mask byte.
    if hostile == "nonfinite":
        glyphs[1] = operands(1,0)+b"\x0c\x0c"+end
    if hostile == "seac-cycle":
        glyphs[1] = operands(0,0,65,194)+end
    if variant == "global":
        glyphs[1] = operands(0,0)+b"\x15"+number(-107)+b"\x1d"+end
    charstrings = index(glyphs, cff2)
    private_extra = b""
    if variant in ("private-blend","private-blend-override"):
        # Dataset 1 has one active region. Blend 10 + 20*1 = 30,
        # leaving the leading BlueValues operand 5 untouched.
        private_extra = operands(1)+b"\x16"+operands(5,10,20,1)+b"\x17\x06"
        private_extra += operands(10,20,1)+b"\x17\x0a"
    if hostile == "dict-underflow":
        private_extra = operands(1)+b"\x17\x06"
    if hostile == "dict-vsindex":
        private_extra = operands(99)+b"\x16"
    if hostile == "dict-stack":
        private_extra = operands(*([0]*514))+b"\x06"
    subrs = [rect_subr]
    if variant == "branching":
        subrs = [(number(i+1-107)+b"\x0a")*2+b"\x0b" for i in range(23)]
        subrs.append(operands(1,0)+b"\x05\x0b")
    widths = b"" if cff2 else offset(600)+b"\x14"+offset(100 if variant == "hints" else 0)+b"\x15"
    private_size = len(widths)+len(private_extra)+6
    private = widths+private_extra+offset(private_size)+b"\x13"+index(subrs,cff2)
    custom_strings = index([b"curve", b"Aacute"], False)
    charset = b"\x00"+b"".join(u16(n) for n in ([17,42,65,88] if cid else [34,391,392,125]))
    # The acute SID is 125; its StandardEncoding character code is 194.
    enc = b"\x00\x04\x41\x42\xc1\xc2"
    fdselect = b"\x03"+u16(1)+u16(0)+b"\x00"+u16(5)
    def top(chars, chars_set, encoding, private_at, fdarray_at, fdselect_at, vstore_at=0):
        result = offset(chars)+b"\x11"
        if not cff2:
            result += offset(chars_set)+b"\x0f"+offset(encoding)+b"\x10"
        if cid:
            result += operands(0,0,0)+b"\x0c\x1e"
        if cid or cff2:
            result += offset(fdarray_at)+b"\x0c\x24"+offset(fdselect_at)+b"\x0c\x25"
        else:
            result += offset(private_size)+offset(private_at)+b"\x12"
        if variable:
            result += offset(vstore_at)+b"\x18"
        return result
    dummy = top(0,0,0,0,0,0)
    if cff2:
        prefix_len = 5+len(dummy)+len(index([rect_subr] if variant == "global" else [], True))
    else:
        prefix_len = 4+len(index([b"FixtureOne",b"FixtureTwo"] if font_set else [b"Fixture"]))+len(index([dummy,dummy] if font_set else [dummy]))+len(custom_strings)+len(index([rect_subr] if variant == "global" else []))
    chars_at = prefix_len
    charset_at = chars_at+len(charstrings)
    enc_at = charset_at+len(charset)
    private_at = enc_at+len(enc)
    first_private_len = len(private)
    if variant in ("fd-array","fd4","fd4-wide","fd-matrix"):
        second_widths = b"" if cff2 else offset(700)+b"\x14"+offset(0)+b"\x15"
        second_subr = operands(200,0,0,400,-200,0)+b"\x05" if cff2 else rect_subr
        private += second_widths+offset(len(second_widths)+6)+b"\x13"+index([second_subr],cff2)
        fdselect = bytes([0,0,1,0,0,0])
    fdarray_at = private_at+len(private)
    if variant in ("fd4","fd4-wide","fd-matrix"):
        fdselect = b"\x04"+u32(3)+u32(0)+u16(0)+u32(1)+u16(256 if variant == "fd4-wide" else 1)+u32(2)+u16(0)+u32(5)
        if hostile == "fd-index": fdselect = fdselect[:15]+u16(2)+fdselect[17:]
        if hostile == "fd-sentinel": fdselect = fdselect[:-4]+u32(4)
        if hostile == "fd-order": fdselect = fdselect[:11]+u32(0)+fdselect[15:]
    fonts = [offset(private_size)+offset(private_at)+b"\x12"]
    if variant in ("fd-array","fd4","fd4-wide","fd-matrix"):
        fonts.append(offset(6 if cff2 else 18)+offset(private_at+first_private_len)+b"\x12"+(b"" if cff2 and variant != "fd-matrix" else operands(2,0,0,2,0,0)+b"\x0c\x07"))
    if variant == "fd4-wide": fonts = [fonts[0]]*256+[fonts[1]]
    fdarray = index(fonts, cff2)
    select_at = fdarray_at+len(fdarray)
    top_dict = top(chars_at,charset_at,enc_at,private_at,fdarray_at,select_at,select_at+len(fdselect))
    if cff2:
        prefix = b"\x02\x00\x05"+u16(len(top_dict))+top_dict+index([rect_subr] if variant == "global" else [],True)
    else:
        prefix = b"\x01\x00\x04\x04"+index([b"FixtureOne",b"FixtureTwo"] if font_set else [b"Fixture"])+index([top_dict,top_dict] if font_set else [top_dict])+custom_strings+index([rect_subr] if variant == "global" else [])
    # Two supports: one contributes zero at default, the all-zero peak
    # contributes one. The default blend is 100 + 0*50 + 1*30 = 130.
    regions = u16(1)+u16(2)+i16(0)+i16(16384)+i16(16384)+bytes(6)
    item_data = u16(0)+u16(0)+u16(2)+u16(0)+u16(1)
    datasets = [item_data]
    if variant in ("private-blend","private-blend-override"): datasets.append(u16(0)+u16(0)+u16(1)+u16(1))
    region_at = 8+4*len(datasets)
    item_offsets = []
    cursor = region_at+len(regions)
    for item in datasets:
        item_offsets.append(cursor)
        cursor += len(item)
    store = u16(1)+u32(region_at)+u16(len(datasets))+b"".join(u32(o) for o in item_offsets)+regions+b"".join(datasets)
    variation_store = u16(len(store))+store if variable else b""
    return prefix+charstrings+charset+enc+private+fdarray+fdselect+variation_store


def encrypt(data, seed):
    result = bytearray()
    for byte in data:
        cipher = byte ^ (seed >> 8)
        result.append(cipher)
        seed = ((cipher+seed)*52845+22719) & 65535
    return bytes(result)


def type1_font(pfb=False, hex_eexec=False, len_iv=4, hostile=None, variant=None):
    def enc_char(data):
        return data if len_iv == -1 else encrypt(bytes(len_iv)+data,4330)
    def char(*values, op):
        return operands(*values)+op
    width = char(0,600,op=b"\x0d")
    subr = (char(100,0,op=b"\x05")+char(0,200,op=b"\x05")
            +char(-100,0,op=b"\x05")+b"\x0b")
    if hostile == "recursive":
        subr = char(0,op=b"\x0a")+b"\x0b"
    rect = width+char(0,0,op=b"\x15")+char(0,op=b"\x0a")+b"\x09\x0e"
    if variant == "hint-replacement":
        rect = width+char(0,0,op=b"\x15")+char(0,1,3,op=b"\x0c\x10")+b"\x0c\x11\x0a\x0e"
    curve = width+char(0,0,op=b"\x15")+char(0,200,100,0,200,-200,op=b"\x08")+b"\x0e"
    accent = width+char(0,0,op=b"\x15")+char(20,40,op=b"\x05")+b"\x0e"
    seac = width+char(0,30,200,65,194,op=b"\x0c\x06")
    # OtherSubrs 1/2/0 record the reference point and six cubic points.
    flex = width+char(0,0,op=b"\x15")+char(0,1,op=b"\x0c\x10")
    for x,y in [(0,0),(10,0),(10,10),(10,-10),(10,0),(10,-10),(10,10)]:
        flex += char(x,y,op=b"\x15")+char(0,2,op=b"\x0c\x10")
    flex += char(50,60,0,3,0,op=b"\x0c\x10")+b"\x0c\x11\x0c\x11\x0c\x21\x0e"
    if hostile == "stack":
        rect = number(0)*25+b"\x0e"
    glyphs = [(b".notdef",width+b"\x0e"),(b"A",rect),(b"curve",curve),
              (b"Aacute",seac),(b"acute",accent),(b"flex",flex)]
    if variant == "missing-notdef":
        glyphs = glyphs[1:]
    plain = f"/lenIV {len_iv} def\n/Subrs 1 array\n".encode()
    encrypted_subr = enc_char(subr)
    plain += b"dup 0 "+str(len(encrypted_subr)).encode()+b" RD "+encrypted_subr+b" NP\n"
    plain += b"/CharStrings 6 dict dup begin\n"
    for name, program in glyphs:
        program = enc_char(program)
        plain += b"/"+name+b" "+str(len(program)).encode()+b" RD "+program+b" ND\n"
    plain += b"end\ncurrentfile closefile\n"
    clear = b"%!PS-AdobeFont-1.0: Fixture 1.0\n/FontType 1 def\n/FontMatrix [0.001 0 0 0.001 0 0] def\n/Encoding StandardEncoding def\ncurrentfile eexec\n"
    if variant == "custom-encoding":
        clear = clear.replace(b"StandardEncoding", b"256 array 0 1 255 {1 index exch /.notdef put} for dup 65 /A put dup 194 /acute put readonly")
    if variant == "lexical":
        clear = clear.replace(b"/FontMatrix ", b"/Comment { (an escaped brace }) % another }\n } def\n/FontMatrix\x00")
    if variant == "paint-type":
        clear = clear.replace(b"/FontType 1 def",b"/FontType 1 def /PaintType 2 def")
    cipher = encrypt(bytes(4)+plain,55665)
    if hex_eexec:
        cipher = cipher.hex().encode()+b"\n"
    if pfb:
        def segment(kind, data):
            return bytes([128,kind])+struct.pack("<I",len(data))+data
        return segment(1,clear)+segment(2,cipher)+bytes([128,3])
    return clear+cipher


def vector(name, data, expected, requirement, fmt="truetype"):
    return Vector(name, {".font": data, ".expect": expected.encode()}, (requirement,),
                  "Independent font writer and analytic glyph-space expectations", extra={"format": fmt})


def cases():
    for fmt in (0,4,6,12):
        expected = "count 5\nmatrix 0.001 0 0 0.001 0 0\nname A 1\nadvance 1 600\nadvance 4 600\n"
        expected += f"code {('mac' if fmt == 0 else 'symbol' if fmt == 6 else 'full' if fmt == 12 else 'unicode')} {0x1f600 if fmt == 12 else 65} 1\n"
        expected += "outline 1\n"+RECT+"outline 2\n"+QUAD
        expected += "outline 3\nm 20 30\nl 70 55\nl 70 155\nl 20 130\nh\n"
        expected += "outline 4\n"+RECT+"m 100 200\nl 200 200\nl 200 400\nl 100 400\nh\n"
        yield vector(f"truetype-cmap{fmt}", truetype(fmt, short=fmt == 0), expected, "font-truetype")
    recursive = i16(-1)+bytes(8)+u16(3)+u16(1)+i16(0)+i16(0)
    for name, data, expected in [
        ("truncated-directory", truetype()[:25], "parse-error\n"),
        ("recursive-component", truetype(glyph_override=recursive), "outline-error 1\n"),
        ("truncated-glyph", truetype(glyph_override=i16(1)+bytes(8)), "outline-error 1\n"),
        ("oversized-directory", u32(0x10000)+u16(65535)+bytes(6), "parse-error\n"),
    ]:
        yield vector(name, data, expected, "font-bounds")
    for cid, cff2, name in [(False,False,"cff"),(True,False,"cff-cid"),(False,True,"cff2")]:
        data = cff_font(cid, cff2)
        expected = "count 5\nmatrix 0.001 0 0 0.001 0 0\n"
        if cid:
            expected += "cid 17 1\ncid 88 4\nis-cid 1\n"
        elif not cff2:
            expected += "name A 1\nname acute 4\nencoding 65 A\nadvance 1 600\n"
        expected += "outline 1\n"+RECT+"outline 2\n"+QUAD
        if not cid and not cff2:
            expected += "outline 3\n"+RECT+"m 30 200\nl 50 240\nh\n"
        if cff2:
            data = truetype(cff=data, cff2=True)
        yield vector(name, data, expected, "font-cff", "opentype" if cff2 else "cff")
    yield vector("opentype-cff", truetype(cff=cff_font()), "count 5\nadvance 1 600\noutline 1\n"+RECT, "font-cff", "opentype")
    for hostile in ("recursive", "stack", "mask", "seac-cycle"):
        yield vector("cff-"+hostile, cff_font(hostile=hostile), "outline-error 1\n", "font-bounds", "cff")
    yield vector("cff-index-overflow", b"\x01\x00\x04\x04\xff\xff\x04\xff\xff\xff\xff", "parse-error\n", "font-bounds", "cff")

    for name, options in [("type1-pfa", {}), ("type1-pfb", {"pfb": True}),
                          ("type1-hex", {"hex_eexec": True}), ("type1-leniv", {"len_iv": -1})]:
        expected = "count 6\nmatrix 0.001 0 0 0.001 0 0\nname A 1\nname acute 4\nencoding 65 A\nadvance 1 600\n"
        expected += "outline 1\n"+RECT+"outline 2\n"+QUAD
        expected += "outline 3\n"+RECT+"m 30 200\nl 50 240\nh\n"
        expected += "outline 5\nm 0 0\nc 10 0 20 10 30 0\nc 40 0 50 -10 60 0\nh\n"
        yield vector(name,type1_font(**options),expected,"font-type1","type1")
    for hostile in ("recursive", "stack"):
        yield vector("type1-"+hostile,type1_font(hostile=hostile),"outline-error 1\n","font-bounds","type1")
    yield vector("type1-truncated-pfb",type1_font(pfb=True)[:-5],"parse-error\n","font-bounds","type1")

    yield vector("cff2-default-blend",truetype(cff=cff_font(cff2=True,variable=True),cff2=True),
                 "count 5\noutline 1\nm 0 0\nl 130 0\nl 130 200\nl 0 200\nh\n", "font-cff", "opentype")

    for variant in ("hints", "global", "flex", "arithmetic"):
        outline = "m 0 0\nc 10 0 20 10 30 10\nc 40 10 50 0 60 0\nh\n" if variant == "flex" else RECT
        yield vector("cff-"+variant,cff_font(variant=variant),
                     f"count 5\nadvance 1 {120 if variant == 'hints' else 600}\noutline 1\n"+outline,"font-cff","cff")
    compressed = i16(1)+bytes(8)+u16(3)+u16(0)+bytes([0x31,0x33,0x35,0x23,100,100,200])
    repeated = i16(1)+bytes(8)+u16(2)+u16(0)+bytes([0x39,2])
    yield vector("truetype-short-flags",truetype(glyph_override=compressed),"outline 1\n"+RECT,"font-truetype")
    yield vector("truetype-repeat-flags",truetype(glyph_override=repeated),"outline 1\nm 0 0\nl 0 0\nl 0 0\nh\n","font-truetype")
    for name, tag, data in [
        ("invalid-cmap-range",b"cmap",u16(0)+u16(1)+u16(3)+u16(1)+u32(65535)),
        ("descending-loca",b"loca",u32(10)+u32(0)+bytes(16)),
        ("truncated-hmtx",b"hmtx",bytes(3)),
        ("invalid-post",b"post",u32(0x20000)+bytes(28)+u16(5)+u16(65535)),
    ]:
        yield vector(name,truetype(table_overrides={tag:data}),"parse-error\n","font-bounds")

    for variant in ("hint-replacement", "custom-encoding"):
        yield vector("type1-"+variant,type1_font(variant=variant),"count 6\nname A 1\nencoding 65 A\nadvance 1 600\noutline 1\n"+RECT,"font-type1","type1")
    yield vector("truetype-implied-points",truetype(glyph_override=simple([(0,100,False),(100,100,False),(100,0,True),(0,0,False)])),
                 "outline 1\nm 0 50\nc 0 83.33333333333333 16.66666666666667 100 50 100\nc 83.33333333333333 100 100 66.66666666666667 100 0\nc 33.33333333333333 0 0 16.66666666666667 0 50\nh\n", "font-truetype")

    yield vector("cff-cid-fd-array",cff_font(cid=True,variant="fd-array"),
                 "count 5\nis-cid 1\ncid 17 1\nadvance 1 1400\nadvance 2 600\noutline 1\nm 0 0\nl 200 0\nl 200 400\nl 0 400\nh\noutline 2\n"+QUAD,"font-cff","cff")

    yield vector("type1-lexical",type1_font(variant="lexical"),"count 6\nname A 1\noutline 1\n"+RECT,"font-type1","type1")

    # ISO 32000-2, 9.6.5.4 identifies each subtable by both directory IDs.
    subs = []
    for gid in (1,2,3):
        subs.append(u16(6)+u16(12)+u16(0)+u16(65)+u16(1)+u16(gid))
    offsets = [28,40,52]
    cmaps = u16(0)+u16(3)+b"".join(u16(p)+u16(e)+u32(o) for (p,e),o in zip([(3,1),(3,10),(0,3)],offsets))+b"".join(subs)
    yield vector("cmap-platform-identities",truetype(table_overrides={b"cmap":cmaps}),
                 "has unicode 1\nhas full 1\nhas platform0 1\nhas mac 0\ncode unicode 65 1\ncode full 65 2\ncode platform0 65 3\n","font-truetype")
    # Table 124 requires CFF and cmap for OpenType CFF; glyf's tables
    # are a different branch of the table and are not required here.
    yield vector("opentype-cff-pdf-minimum",sfnt({b"CFF ":cff_font(),b"cmap":cmap(4)},otto=True),
                 "count 5\nadvance 1 600\noutline 1\n"+RECT,"font-cff","opentype")
    yield vector("opentype-cff-missing-cmap",sfnt({b"CFF ":cff_font()},otto=True),"parse-error\n","font-cff","opentype")
    for missing in (b"glyf",b"head",b"hhea",b"hmtx",b"loca",b"maxp"):
        # Rebuild the independent writer's table records with one omitted.
        original = truetype()
        count = struct.unpack_from(">H",original,4)[0]
        tables = {}
        for i in range(count):
            tag,_,at,size = struct.unpack_from(">4sIII",original,12+16*i)
            if tag != missing:
                tables[tag] = original[at:at+size]
        yield vector("required-"+missing.decode()+"-missing",sfnt(tables),"parse-error\n","font-truetype")
    yield vector("vertical-tables-ignored",truetype(table_overrides={b"vhea":b"invalid",b"vmtx":b"invalid"}),
                 "advance 1 600\noutline 1\n"+RECT,"font-truetype")

    yield vector("cff-multiple-fonts",cff_font(font_set=True),"parse-error\n","font-cff","cff")
    yield vector("type1-missing-notdef",type1_font(variant="missing-notdef"),"parse-error\n","font-type1","type1")
    yield vector("type1-paint-type-ignored",type1_font(variant="paint-type"),"outline 1\n"+RECT,"font-type1","type1")
    yield vector("truetype-cid-no-cmap",truetype(omit_tables=(b"cmap",)),"has unicode 0\noutline 1\n"+RECT,"font-truetype")

    yield vector("cff-standard-sids",metadata_font(),
                 "count 391\nname .notdef 0\nname zcaron 228\nname exclamsmall 229\nname parenrightinferior 270\nname colonmonetary 300\nname Ydieresissmall 378\nname 001.000 379\nname Semibold 390\nencoding 65 A\n", "font-cff","cff")
    yield vector("cff-expert-charset",metadata_font(1,1),
                 "count 166\nname space 1\nname exclamsmall 2\nname Ydieresissmall 165\nencoding 33 exclamsmall\nencoding 95 hyphensuperior\nencoding 96 Gravesmall\nencoding 97 Asmall\nencoding 255 Ydieresissmall\n", "font-cff","cff")
    yield vector("cff-expert-subset",metadata_font(2,1),
                 "count 87\nname space 1\nname dollaroldstyle 2\nname commainferior 86\nencoding 36 dollaroldstyle\n", "font-cff","cff")
    for fmt in (0,1,2):
        yield vector(f"cff-custom-charset{fmt}",metadata_font(3,2,fmt),
                     "count 6\nname exclamsmall 1\nname Hungarumlautsmall 2\nname dollaroldstyle 3\nname Semibold 4\nname custom 5\nencoding 65 exclamsmall\nencoding 68 Semibold\nencoding 80 custom\n", "font-cff","cff")
    yield vector("cff-custom-encoding0-supplement",metadata_font(3,3),"encoding 65 exclamsmall\nencoding 66 Hungarumlautsmall\nencoding 90 custom\n", "font-cff","cff")
    for fmt, expected in [
        (2,"has symbol 1\ncode symbol 65 1\ncode symbol 33088 2\ncode symbol 33089 0\ncode symbol 129 0\ncode symbol 64 0\ncode symbol 321 0\nabsent symbol 65536\n"),
        (10,"has full 1\nhas unicode 0\ncode full 128512 1\ncode full 128513 0\ncode full 128514 2\nabsent full 128511\nabsent full 128515\n"),
        (13,"has platform0 1\nhas unicode 0\ncode platform0 65 1\ncode platform0 67 1\ncode platform0 128512 2\ncode platform0 1114111 2\nabsent platform0 68\nabsent platform0 1114112\n"),
        (14,"has unicode 1\nhas platform0 0\ncode unicode 65 1\n"),
    ]:
        yield vector(f"truetype-cmap{fmt}",truetype(table_overrides={b"cmap":extended_cmap(fmt)}),expected,"font-truetype")
    for fmt, mal in [(2,"key"),(2,"offset"),(10,"count"),(13,"overlap")]:
        yield vector(f"cmap{fmt}-invalid-{mal}",truetype(table_overrides={b"cmap":extended_cmap(fmt,mal)}),"parse-error\n","font-bounds")

    yield vector("cff2-fdselect4",truetype(cff=cff_font(cff2=True,variant="fd4"),cff2=True),
                 "count 5\noutline 1\nm 0 0\nl 200 0\nl 200 400\nl 0 400\nh\noutline 2\n"+QUAD,"font-cff","opentype")
    yield vector("cff2-private-blend",truetype(cff=cff_font(cff2=True,variable=True,variant="private-blend"),cff2=True),
                 "outline 1\nm 0 0\nl 130 0\nl 130 200\nl 0 200\nh\n","font-cff","opentype")
    for hostile in ("fd-index","fd-sentinel","fd-order"):
        yield vector("cff2-"+hostile,truetype(cff=cff_font(cff2=True,variant="fd4",hostile=hostile),cff2=True),"parse-error\n","font-bounds","opentype")
    for hostile in ("dict-underflow","dict-vsindex","dict-stack"):
        yield vector("cff2-"+hostile,truetype(cff=cff_font(cff2=True,variable=True,hostile=hostile),cff2=True),"parse-error\n","font-bounds","opentype")

    for n in (64,128,256):
        for subr in (False,True):
            variant = "linear-subr" if subr else "linear"
            expected = f"budget 1 {(6 if subr else 3)*n+4} {n+2}\noutline 1\nm 0 0\n"
            expected += "".join(f"l {x} 0\n" for x in range(1,n+1))+"h\n"
            yield vector(f"cff-{variant}-{n}",cff_font(variant=variant,line_count=n),expected,"font-bounds","cff")
    yield vector("cff-branching-subrs",cff_font(variant="branching"),"budget-error 1 2000\n","font-bounds","cff")

    yield vector("cff2-fdselect4-wide",truetype(cff=cff_font(cff2=True,variant="fd4-wide"),cff2=True),
                 "count 5\noutline 1\nm 0 0\nl 200 0\nl 200 400\nl 0 400\nh\n","font-cff","opentype")

    yield vector("cff2-private-blend-override",truetype(cff=cff_font(cff2=True,variable=True,variant="private-blend-override"),cff2=True),
                 "outline 1\nm 0 0\nl 130 0\nl 130 200\nl 0 200\nh\n","font-cff","opentype")

    yield vector("cff2-font-dict-matrix",truetype(cff=cff_font(cff2=True,variant="fd-matrix"),cff2=True),
                 "unsupported font.format\n","font-cff","opentype")
    yield vector("cff-nonfinite",cff_font(hostile="nonfinite"),"outline-error 1\n","font-bounds","cff")


def metadata_font(charset_kind=0, encoding_kind=0, charset_format=0):
    """CFF metadata fixtures; external table definitions await primary review."""
    if charset_kind == 0:
        sids = list(range(391))
    elif charset_kind == 1:
        # The first expert entries and the complete table's final entry.
        sids = None
    elif charset_kind == 2:
        sids = None
    else:
        sids = [0,229,230,231,390,391]
    count = len(sids) if sids is not None else 166 if charset_kind == 1 else 87
    chars = index([b"\x0e"]*count)
    if charset_kind <= 2 and charset_kind != 0:
        charset_data = b""
    elif charset_format == 0:
        charset_data = b"\x00"+b"".join(u16(sid) for sid in sids[1:])
    else:
        left = lambda n: bytes([n]) if charset_format == 1 else u16(n)
        charset_data = bytes([charset_format])+u16(229)+left(2)+u16(390)+left(1)
    if encoding_kind <= 1:
        enc = b""
    elif encoding_kind == 2:
        enc = b"\x81\x01\x41\x03\x01\x50"+u16(391)
    else:
        enc = b"\x80\x02\x41\x42\x01\x5a"+u16(391)
    def top(chars_at,charset_at,enc_at):
        return offset(chars_at)+b"\x11"+offset(charset_kind if charset_kind in (1,2) else charset_at)+b"\x0f"+offset(encoding_kind if encoding_kind <= 1 else enc_at)+b"\x10"
    names = index([b"Metadata"])
    strings = index([b"custom"] if charset_kind == 3 else [])
    at = 4+len(names)+len(index([top(0,0,0)]))+len(strings)+len(index([]))
    dictionary = top(at,at+len(chars),at+len(chars)+len(charset_data))
    return b"\x01\x00\x04\x04"+names+index([dictionary])+strings+index([])+chars+charset_data+enc


def extended_cmap(fmt, malformed=None):
    if fmt == 2:
        keys = [0]*256
        keys[0x81] = 8
        # One-byte A maps to glyph 1; 0x8140 maps to glyph 2. The byte
        # 0x81 alone is a lead byte and must not use the single-byte header.
        headers = u16(65)+u16(1)+i16(-1)+u16(10)
        headers += u16(0x40)+u16(2)+i16(1)+u16(4)
        sub = u16(2)+u16(540)+u16(0)+b"".join(u16(k) for k in keys)+headers+u16(2)+u16(1)+u16(0)
        assert len(sub) == 540
        if malformed == "key":
            sub = sub[:6]+u16(7)+sub[8:]
        elif malformed == "offset":
            sub = sub[:524]+u16(65534)+sub[526:]
        platform, encoding = 3,0
    elif fmt == 10:
        sub = u16(10)+u16(0)+u32(26)+u32(0)+u32(0x1f600)+u32(3)+u16(1)+u16(0)+u16(2)
        if malformed == "count":
            sub = sub[:16]+u32(0xffffffff)+sub[20:]
        platform, encoding = 3,10
    elif fmt == 13:
        groups = [(65,67,1),(0x1f600,0x10ffff,2)]
        if malformed == "overlap":
            groups[1] = (66,0x10ffff,2)
        sub = u16(13)+u16(0)+u32(40)+u32(0)+u32(2)+b"".join(u32(a)+u32(b)+u32(g) for a,b,g in groups)
        platform, encoding = 0,6
    else:
        # UVS format 14 does not select glyphs from a single PDF code;
        # the base (3,1) encoding remains the ordinary glyph selector.
        base = cmap(4)[12:]
        uvs = u16(14)+u32(30)+u32(1)+bytes([0,0xfe,0x0f])+u32(0)+u32(21)+u32(1)+bytes([0,0,65])+u16(2)
        assert len(uvs) == 30
        return u16(0)+u16(2)+u16(3)+u16(1)+u32(20)+u16(0)+u16(5)+u32(20+len(base))+base+uvs
    return u16(0)+u16(1)+u16(platform)+u16(encoding)+u32(12)+sub
