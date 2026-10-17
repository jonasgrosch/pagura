"""Small independently written font programs and analytic glyph-space oracles.

PDF program constraints are checked against local ISO 32000-2 clause 9.9
and 9.6.5.4. Type 1 chapters 6-8 and TN 5176/5177 define the
charstring and CFF vectors below. OpenType/TrueType layouts await primary review. No engine is used to construct expected outlines.
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


def cff_font(cid=False, cff2=False, hostile=None, variable=False, variant=None, font_set=False, line_count=0, conforming=False):
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
    if variant == "width-seac":
        seac = operands(25,30,200,65,194)+end
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
        subrs = [(number(i+1-107)+b"\x0a")*2+b"\x0b" for i in range(9)]
        subrs.append(operands(1,0)+b"\x05\x0b")
    widths = b"" if cff2 else offset(600)+b"\x14"+offset(100 if variant in ("hints","width-seac") else 0)+b"\x15"
    private_size = len(widths)+len(private_extra)+6
    private = widths+private_extra+offset(private_size)+b"\x13"+index(subrs,cff2)
    custom_strings = index([b"curve", b"Aacute"]+([b"Adobe",b"Identity"] if cid and conforming else []), False)
    charset = b"\x00"+b"".join(u16(n) for n in ([17,42,65,88] if cid else [34,391,392,125]))
    # The acute SID is 125; its StandardEncoding character code is 194.
    enc = b"\x00\x04\x41\x42\xc1\xc2"
    fdselect = b"\x03"+u16(1)+u16(0)+b"\x00"+u16(5)
    def top(chars, chars_set, encoding, private_at, fdarray_at, fdselect_at, vstore_at=0):
        result = operands(393,394,0)+b"\x0c\x1e" if cid and conforming else b""
        result += offset(chars)+b"\x11"
        if not cff2:
            result += offset(chars_set)+b"\x0f"+(b"" if cid and conforming else offset(encoding)+b"\x10")
        if cid and not conforming:
            result += operands(0,0,0)+b"\x0c\x1e"
        if cid or cff2:
            result += offset(fdarray_at)+b"\x0c\x24"+offset(fdselect_at)+b"\x0c\x25"
        else:
            result += offset(private_size)+offset(private_at)+b"\x12"
        if variant == "cid-count":
            result += operands(1)+b"\x0c\x22"
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


def type1_font(pfb=False, hex_eexec=False, len_iv=4, hostile=None, variant=None, program=None, custom_subrs=None, conforming=False):
    def enc_char(data):
        return data if len_iv == -1 else encrypt(bytes(len_iv)+data,4330)
    def char(*values, op):
        return operands(*values)+op
    width = char(0,600,op=b"\x0d")
    subr = (char(100,0,op=b"\x05")+char(0,200,op=b"\x05")
            +char(-100,0,op=b"\x05")+b"\x0b")
    if hostile == "recursive":
        subr = char(5 if conforming else 0,op=b"\x0a")+b"\x0b"
    rect = width+char(0,0,op=b"\x15")+char(5 if conforming else 0,op=b"\x0a")+b"\x09\x0e"
    if variant == "hint-replacement":
        rect = width+char(0,0,op=b"\x15")+char(4,1,3,op=b"\x0c\x10")+b"\x0c\x11\x0a"+char(5,op=b"\x0a")+b"\x0e"
    curve = width+char(0,0,op=b"\x15")+char(0,200,100,0,200,-200,op=b"\x08")+b"\x0e"
    accent = width+char(0,0,op=b"\x15")+char(20,40,op=b"\x05")+b"\x0e"
    seac = width+char(0,30,200,65,194,op=b"\x0c\x06")
    # OtherSubrs 1/2/0 record the reference point and six cubic points.
    flex = width+char(0,0,op=b"\x15")+char(0,1,op=b"\x0c\x10")
    flex_points = [(30,0),(-20,0),(10,10),(10,0),(10,0),(10,-10),(10,0)] if conforming else [(0,0),(10,0),(10,10),(10,-10),(10,0),(10,-10),(10,10)]
    for x,y in flex_points:
        flex += char(x,y,op=b"\x15")+char(0,2,op=b"\x0c\x10")
    flex += char(50,60,0,3,0,op=b"\x0c\x10")+b"\x0c\x11\x0c\x11\x0c\x21\x0e"
    if program is not None:
        rect = program
    if custom_subrs is not None:
        flex = width+b"\x0e"
    if hostile == "stack":
        rect = number(0)*25+b"\x0e"
    if variant == "seac-trailing":
        seac += operands(0)
    if variant == "seac-bearings":
        rect = char(40,600,op=b"\x0d")+(operands(0,0)+b"\x15")+char(5 if conforming else 0,op=b"\x0a")+b"\x0e"
        accent = char(10,600,op=b"\x0d")+(operands(0,0)+b"\x15")+char(20,40,op=b"\x05")+b"\x0e"
        seac = char(40,600,op=b"\x0d")+char(10,30,200,65,194,op=b"\x0c\x06")
    glyphs = [(b".notdef",width+b"\x0e"),(b"A",rect),(b"curve",curve),
              (b"Aacute",seac),(b"acute",accent),(b"flex",flex)]
    if variant == "missing-notdef":
        glyphs = glyphs[1:]
    subrs = [subr]
    if conforming or variant == "hint-replacement":
        subrs = [char(3,0,op=b"\x0c\x10")+b"\x0c\x11\x0c\x11\x0c\x21\x0b",
                 char(0,1,op=b"\x0c\x10")+b"\x0b", char(0,2,op=b"\x0c\x10")+b"\x0b",
                 b"\x0b", char(0,10,op=b"\x01")+b"\x0b", subr]
    if custom_subrs is not None:
        subrs = custom_subrs
    plain = b"/Private 16 dict dup begin\n/BlueShift 11 def\n" if conforming else b""
    if variant != "default-leniv":
        plain += f"/lenIV {len_iv} def\n".encode()
    plain += f"/Subrs {len(subrs)} array\n".encode()
    for i, subroutine in enumerate(subrs):
        encrypted_subr = enc_char(subroutine)
        plain += f"dup {i} {len(encrypted_subr)} RD ".encode()+encrypted_subr+b" NP\n"
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
    if variant == "matrix-procedure":
        clear = clear.replace(b"[0.001 0 0 0.001 0 0]", b"{0.001 0 0 0.001 0 0}")
    if variant == "unencrypted":
        return clear.replace(b"currentfile eexec\n",b"")+plain
    if variant == "missing-fonttype":
        clear = clear.replace(b"/FontType 1 def\n",b"")
    if variant == "missing-matrix":
        clear = clear.replace(b"/FontMatrix [0.001 0 0 0.001 0 0] def\n",b"")
    if variant == "missing-header":
        clear = clear[clear.index(b"\n")+1:]
    if variant == "subrs-incomplete":
        plain = plain.replace(f"/Subrs {len(subrs)} array".encode(), f"/Subrs {len(subrs)+1} array".encode())
    if variant == "dict-count":
        plain = plain.replace(b"/CharStrings 6 dict",b"/CharStrings 1 dict")
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
        data = cff_font(cid, cff2,conforming=True)
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
    yield vector("opentype-cff", truetype(cff=cff_font(conforming=True)), "count 5\nadvance 1 600\noutline 1\n"+RECT, "font-cff", "opentype")
    for hostile in ("recursive", "stack", "mask", "seac-cycle"):
        yield vector("cff-"+hostile, cff_font(conforming=True,hostile=hostile), "outline-error 1\n", "font-bounds", "cff")
    yield vector("cff-index-overflow", b"\x01\x00\x04\x04\xff\xff\x04\xff\xff\xff\xff", "parse-error\n", "font-bounds", "cff")

    for name, options in [("type1-pfa", {}), ("type1-pfb", {"pfb": True}),
                          ("type1-hex", {"hex_eexec": True}), ("type1-leniv", {"len_iv": -1})]:
        expected = "count 6\nmatrix 0.001 0 0 0.001 0 0\nname A 1\nname acute 4\nencoding 65 A\nadvance 1 600\n"
        expected += "outline 1\n"+RECT+"outline 2\n"+QUAD
        expected += "outline 3\n"+RECT+"m 30 200\nl 50 240\nh\n"
        expected += "outline 5\nm 0 0\nc 10 0 20 10 30 10\nc 40 10 50 0 60 0\nh\n"
        yield vector(name,type1_font(conforming=True,**options),expected,"font-type1","type1")
    for hostile in ("recursive", "stack"):
        yield vector("type1-"+hostile,type1_font(conforming=True,hostile=hostile),"outline-error 1\n","font-bounds","type1")
    yield vector("type1-truncated-pfb",type1_font(conforming=True,pfb=True)[:-5],"parse-error\n","font-bounds","type1")

    yield vector("cff2-default-blend",truetype(cff=cff_font(conforming=True,cff2=True,variable=True),cff2=True),
                 "count 5\noutline 1\nm 0 0\nl 130 0\nl 130 200\nl 0 200\nh\n", "font-cff", "opentype")

    for variant in ("hints", "global", "flex", "arithmetic"):
        outline = "m 0 0\nc 10 0 20 10 30 10\nc 40 10 50 0 60 0\nh\n" if variant == "flex" else RECT
        yield vector("cff-"+variant,cff_font(conforming=True,variant=variant),
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
        yield vector("type1-"+variant,type1_font(conforming=True,variant=variant),"count 6\nname A 1\nencoding 65 A\nadvance 1 600\noutline 1\n"+RECT,"font-type1","type1")
    yield vector("truetype-implied-points",truetype(glyph_override=simple([(0,100,False),(100,100,False),(100,0,True),(0,0,False)])),
                 "outline 1\nm 0 50\nc 0 83.33333333333333 16.66666666666667 100 50 100\nc 83.33333333333333 100 100 66.66666666666667 100 0\nc 33.33333333333333 0 0 16.66666666666667 0 50\nh\n", "font-truetype")

    yield vector("cff-cid-fd-array",cff_font(conforming=True,cid=True,variant="fd-array"),
                 "count 5\nis-cid 1\ncid 17 1\nadvance 1 1400\nadvance 2 600\noutline 1\nm 0 0\nl 200 0\nl 200 400\nl 0 400\nh\noutline 2\n"+QUAD,"font-cff","cff")

    yield vector("type1-lexical",type1_font(conforming=True,variant="lexical"),"count 6\nname A 1\noutline 1\n"+RECT,"font-type1","type1")

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
    yield vector("opentype-cff-pdf-minimum",sfnt({b"CFF ":cff_font(conforming=True),b"cmap":cmap(4)},otto=True),
                 "count 5\nadvance 1 600\noutline 1\n"+RECT,"font-cff","opentype")
    yield vector("opentype-cff-missing-cmap",sfnt({b"CFF ":cff_font(conforming=True)},otto=True),"parse-error\n","font-cff","opentype")
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

    yield vector("cff-multiple-fonts",cff_font(conforming=True,font_set=True),"parse-error\n","font-cff","cff")
    yield vector("type1-missing-notdef",type1_font(conforming=True,variant="missing-notdef"),"parse-error\n","font-type1","type1")
    yield vector("type1-paint-type-ignored",type1_font(conforming=True,variant="paint-type"),"outline 1\n"+RECT,"font-type1","type1")
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

    yield vector("cff2-fdselect4",truetype(cff=cff_font(conforming=True,cff2=True,variant="fd4"),cff2=True),
                 "count 5\noutline 1\nm 0 0\nl 200 0\nl 200 400\nl 0 400\nh\noutline 2\n"+QUAD,"font-cff","opentype")
    yield vector("cff2-private-blend",truetype(cff=cff_font(conforming=True,cff2=True,variable=True,variant="private-blend"),cff2=True),
                 "outline 1\nm 0 0\nl 130 0\nl 130 200\nl 0 200\nh\n","font-cff","opentype")
    for hostile in ("fd-index","fd-sentinel","fd-order"):
        yield vector("cff2-"+hostile,truetype(cff=cff_font(conforming=True,cff2=True,variant="fd4",hostile=hostile),cff2=True),"parse-error\n","font-bounds","opentype")
    for hostile in ("dict-underflow","dict-vsindex","dict-stack"):
        yield vector("cff2-"+hostile,truetype(cff=cff_font(conforming=True,cff2=True,variable=True,hostile=hostile),cff2=True),"parse-error\n","font-bounds","opentype")

    for n in (64,128,256):
        for subr in (False,True):
            variant = "linear-subr" if subr else "linear"
            expected = f"budget 1 {(6 if subr else 3)*n+4} {n+2}\noutline 1\nm 0 0\n"
            expected += "".join(f"l {x} 0\n" for x in range(1,n+1))+"h\n"
            yield vector(f"cff-{variant}-{n}",cff_font(conforming=True,variant=variant,line_count=n),expected,"font-bounds","cff")
    yield vector("cff-branching-subrs",cff_font(conforming=True,variant="branching"),"budget-error 1 2000\n","font-bounds","cff")

    yield vector("cff2-fdselect4-wide",truetype(cff=cff_font(conforming=True,cff2=True,variant="fd4-wide"),cff2=True),
                 "count 5\noutline 1\nm 0 0\nl 200 0\nl 200 400\nl 0 400\nh\n","font-cff","opentype")

    yield vector("cff2-private-blend-override",truetype(cff=cff_font(conforming=True,cff2=True,variable=True,variant="private-blend-override"),cff2=True),
                 "outline 1\nm 0 0\nl 130 0\nl 130 200\nl 0 200\nh\n","font-cff","opentype")

    yield vector("cff2-font-dict-matrix",truetype(cff=cff_font(conforming=True,cff2=True,variant="fd-matrix"),cff2=True),
                 "unsupported font.format\n","font-cff","opentype")
    yield from specification_cases()

    yield vector("cff-nonfinite",cff_font(conforming=True,hostile="nonfinite"),"outline-error 1\n","font-bounds","cff")


def metadata_font(charset_kind=0, encoding_kind=0, charset_format=0):
    """CFF metadata vectors from TN 5176 sections 10-13 and appendices A-C."""
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
        enc = b"\x81\x01\x41\x04\x01\x50"+u16(391)
    else:
        enc = b"\x80\x05\x41\x42\x43\x44\x45\x01\x5a"+u16(391)
    def top(chars_at,charset_at,enc_at):
        return offset(chars_at)+b"\x11"+offset(charset_kind if charset_kind in (1,2) else charset_at)+b"\x0f"+offset(encoding_kind if encoding_kind <= 1 else enc_at)+b"\x10"+operands(0,0)+b"\x12"
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


def reviewed_cff(program, subrs=(), global_subrs=(), private=b"", top=b"", omit_private=False, header_size=4):
    """Two-glyph CFF, authored from TN 5176 sections 4-16, not an engine."""
    names, strings, globals_ = index([b"Reviewed"]), index([]), index(global_subrs)
    # FontMatrix/charset/Encoding defaults deliberately omitted.
    def dictionary(chars_at, private_at, size):
        result = top+offset(chars_at)+b"\x11"
        if not omit_private:
            result += offset(size)+offset(private_at)+b"\x12"
        return result
    extra = private
    if subrs:
        extra += offset(len(private)+6)+b"\x13"
    chars = index([b"\x0e", program])
    dummy = dictionary(0,0,len(extra))
    at = header_size+len(names)+len(index([dummy]))+len(strings)+len(globals_)
    return (bytes([1,0,header_size,4])+bytes(header_size-4)+names
            +index([dictionary(at,at+len(chars),len(extra))])+strings+globals_+chars
            +extra+(index(subrs) if subrs else b""))


def specification_cases():
    """Primary-text rules: Type 1 chapters 6-8; TN 5176; TN 5177 §3/4, B/C."""
    end, move = b"\x0e", operands(0,0)+b"\x15"
    line = operands(12,0)+b"\x05"
    def cff(name, program, expected, **options):
        return vector("review-cff-"+name, reviewed_cff(program,**options), expected, "font-cff", "cff")
    def t1(name, program=None, expected="outline 1\nm 0 0\nl 12 0\nh\n", **options):
        return vector("review-type1-"+name, type1_font(program=program,conforming=True,**options), expected, "font-type1", "type1")
    width = operands(0,600)+b"\x0d"
    tmove = width+move
    small = "outline 1\nm 0 0\nl 12 0\nh\n"
    # Width may occur before any of the first stack-clearing operators (§4.1 Note 4).
    for name, first, args in [("rmoveto",21,(0,0)),("hmoveto",22,(0,)),("vmoveto",4,(0,)),
                               ("hstem",1,(0,10)),("vstem",3,(0,10)),("hstemhm",18,(0,10)),("vstemhm",23,(0,10))]:
        program = operands(25,*args)+bytes([first])+(b"" if first in (4,21,22) else move)+line+end
        yield cff("width-"+name,program,"advance 1 125\n"+small,private=operands(100)+b"\x15")
    yield cff("width-endchar",operands(25)+end,"advance 1 125\noutline 1\n",private=operands(100)+b"\x15")
    yield cff("width-default",move+line+end,"advance 1 77\n"+small,private=operands(77)+b"\x14")
    yield cff("dotsection",move+b"\x0c\x00"+line+end,small)
    yield cff("roll-zero",move+operands(0,4)+b"\x0c\x1e"+line+end,small)
    yield cff("index-negative",move+operands(6,-1)+b"\x0c\x1d\x0c\x0a"+operands(0)+b"\x05"+end,small)
    yield cff("index-outside",move+operands(6,1)+b"\x0c\x1d"+end,"outline-error 1\n")
    # Each path operator has independently chosen unit coordinates and analytic cubics.
    draws = [
        ("rlineto",5,(1,2,3,4),"l 1 2\nl 4 6\n"),
        ("hlineto",6,(1,2,3),"l 1 0\nl 1 2\nl 4 2\n"),
        ("vlineto",7,(1,2,3),"l 0 1\nl 2 1\nl 2 4\n"),
        ("rrcurveto",8,(1,2,3,4,5,6),"c 1 2 4 6 9 12\n"),
        ("rcurveline",24,(1,2,3,4,5,6,7,8),"c 1 2 4 6 9 12\nl 16 20\n"),
        ("rlinecurve",25,(1,2,3,4,5,6,7,8),"l 1 2\nc 4 6 9 12 16 20\n"),
        ("vvcurveto",26,(1,2,3,4,5,6,7,8,9),"c 1 2 4 6 4 11\nc 4 17 11 25 11 34\n"),
        ("hhcurveto",27,(1,2,3,4,5,6,7,8,9),"c 2 1 5 5 10 5\nc 16 5 23 13 32 13\n"),
        ("hvcurveto",31,(1,2,3,4,5,6,7,8,9),"c 1 0 3 3 3 7\nc 3 12 9 19 17 28\n"),
        ("vhcurveto",30,(1,2,3,4,5,6,7,8,9),"c 0 1 2 4 6 4\nc 11 4 17 11 26 19\n"),
    ]
    for name, op, args, path in draws:
        yield cff(name,move+operands(*args)+bytes([op])+end,"outline 1\nm 0 0\n"+path+"h\n")
    flexes = [
        ("flex",35,(1,2,3,4,5,6,7,8,9,10,11,12,50),"c 1 2 4 6 9 12\nc 16 20 25 30 36 42\n"),
        ("hflex",34,(1,2,3,4,5,6,7),"c 1 0 3 3 7 3\nc 12 3 18 0 25 0\n"),
        ("hflex1",36,(1,2,3,4,5,6,7,8,9),"c 1 2 4 6 9 6\nc 15 6 22 14 31 0\n"),
        ("flex1-x",37,(1,2,3,4,5,6,7,8,9,10,11),"c 1 2 4 6 9 12\nc 16 20 25 30 0 41\n"),
        ("flex1-y",37,(2,1,4,3,6,5,8,7,10,9,11),"c 2 1 6 4 12 9\nc 20 16 30 25 41 0\n"),
    ]
    for name, op, args, path in flexes:
        yield cff(name,move+operands(*args)+bytes([12,op])+end,"outline 1\nm 0 0\n"+path+"h\n")
    # Arithmetic/storage/conditionals are tested by the resulting line length.
    for name, args, op, result in [
        ("abs",(-12,),9,12),("add",(5,7),10,12),("sub",(19,7),11,12),
        ("div",(25,2),12,12.5),("neg",(-12,),14,12),("mul",(3,4),24,12),
        ("sqrt",(144,),26,12),("and",(2,3),3,1),("or",(0,3),4,1),
        ("not",(0,),5,1),("eq",(3,3),15,1),("ifelse",(12,9,2,3),22,12)]:
        yield cff(name,move+operands(*args)+bytes([12,op])+operands(0)+b"\x05"+end,
                  f"outline 1\nm 0 0\nl {result} 0\nh\n")
    for name, expression in [
        ("drop",operands(12,99)+b"\x0c\x12"),
        ("dup",operands(6)+b"\x0c\x1b\x0c\x0a"),
        ("exch",operands(9,12)+b"\x0c\x1c\x0c\x12"),
        ("roll",operands(7,12,3,3,-1)+b"\x0c\x1e\x0c\x12\x0c\x12"),
        ("transient",operands(12,31)+b"\x0c\x14"+operands(31)+b"\x0c\x15")]:
        yield cff(name,move+expression+operands(0)+b"\x05"+end,small)
    for n in (1239,1240,33899,33900):
        bias = 107 if n<1240 else 1131 if n<33900 else 32768
        for global_ in (False,True):
            subrs = [b"\x0b"]*(n-1)+[line+b"\x0b"]
            program = move+number(n-1-bias)+bytes([29 if global_ else 10])+end
            options = {"global_subrs" if global_ else "subrs":subrs}
            yield cff(f"bias-{'global' if global_ else 'local'}-{n}",program,small,**options)
    for n in (10,11):
        subrs = [number(i+1-107)+b"\x0a\x0b" for i in range(n-1)]+[line+b"\x0b"]
        yield cff(f"nesting-{n}",move+number(-107)+b"\x0a"+end,small if n==10 else "outline-error 1\n",subrs=subrs)
        tsubrs = [number(i+1)+b"\x0a\x0b" for i in range(n-1)]+[line+b"\x0b"]
        yield t1(f"nesting-{n}",tmove+operands(0)+b"\x0a"+end,small if n==10 else "outline-error 1\n",custom_subrs=tsubrs)
    for n in (48,49):
        program=move+operands(*([1]*n))+b"\x06"+end
        path="outline 1\nm 0 0\n"+"".join(f"l {(i+1)//2} {i//2}\n" for i in range(1,n+1))+"h\n"
        yield cff(f"stack-{n}",program,path if n==48 else "outline-error 1\n")
        yield cff(f"dict-stack-{n}",move+line+end,small if n==48 else "parse-error\n",top=operands(*range(n))+b"\x0e")
    for n in (96,97):
        hints=b"".join(operands(10+2*i,1)+b"\x12" for i in range(n))
        mask=bytes([255])*(n//8)+(bytes([128]) if n%8 else b"")
        yield cff(f"stems-{n}",hints+b"\x13"+mask+move+line+end,small if n==96 else "outline-error 1\n")
    for name, prefix in [("mask-padding",operands(0,10)+b"\x12\x13\x81"),
                         ("mask-no-stems",b"\x13"),("late-stems",move+operands(0,10)+b"\x12"),
                         ("hint-order",operands(0,10)+b"\x03"+operands(0,10)+b"\x01")]:
        yield cff(name,prefix+move+line+end,"outline-error 1\n")
    yield cff("mask-implicit-vstem",operands(25,0,10)+b"\x12"+operands(0,10)+b"\x14\xc0\x13\x80"+move+line+end,
              "advance 1 125\n"+small,private=operands(100)+b"\x15")
    for n in (65535,65536):
        # 3 move bytes + 3 line bytes + endchar; pad with two-byte no-ops.
        program=move+line+b"\x0c\x00"*((n-7)//2)+end
        if len(program)<n: program=move+line+operands(0)+b"\x0c\x12"+program[8:]
        assert len(program)==n
        yield cff(f"length-{n}",program,small if n==65535 else "outline-error 1\n")
    yield cff("private-required",move+line+end,"parse-error\n",omit_private=True)
    yield cff("header-extension",move+line+end,small,header_size=8)
    yield cff("reserved-dict",move+line+end,"parse-error\n",top=operands(1)+b"\x16")
    yield cff("wrong-dict-domain",move+line+end,"parse-error\n",top=operands(1)+b"\x14")
    yield cff("blue-order",move+line+end,"parse-error\n",private=operands(0,10)+b"\x07")
    # Type 1 fallback returns arg1 then arg2, and results expire at the next command.
    yield t1("othersubr-fallback",tmove+operands(12,0,2,99)+b"\x0c\x10\x0c\x11\x0c\x11\x05"+end)
    yield t1("othersubr-stale",tmove+operands(12,1,99)+b"\x0c\x10"+operands(3)+b"\x06\x0c\x11"+end,"outline-error 1\n")
    yield t1("div",tmove+operands(25,2)+b"\x0c\x0c"+operands(0)+b"\x05"+end,"outline 1\nm 0 0\nl 12.5 0\nh\n")
    for variant in ("default-leniv","matrix-procedure","unencrypted"):
        yield t1(variant,variant=variant,expected="advance 1 600\noutline 1\n"+RECT)
    # Type 1 6.4: closepath leaves the charstring current point at its old endpoint.
    yield t1("closepoint",tmove+line+b"\x09"+operands(2,3)+b"\x15"+line+end,
             "outline 1\nm 0 0\nl 12 0\nh\nm 14 3\nl 26 3\nh\n")
    yield t1("width-first",move+width+end,"outline-error 1\n")

    for name, op, args, path in [
        ("hlineto",6,(12,),"l 12 0\n"),("vlineto",7,(12,),"l 0 12\n"),
        ("hvcurveto",31,(1,2,3,4),"c 1 0 3 3 3 7\n"),
        ("vhcurveto",30,(1,2,3,4),"c 0 1 2 4 6 4\n")]:
        yield t1(name,tmove+operands(*args)+bytes([op])+end,"outline 1\nm 0 0\n"+path+"h\n")
    for name, op, args, point in [("hmoveto",22,(12,),"12 0"),("vmoveto",4,(12,),"0 12")]:
        x,y=map(int,point.split())
        yield t1(name,width+operands(*args)+bytes([op])+line+end,f"outline 1\nm {point}\nl {x+12} {y}\nh\n")
    yield t1("sbw",operands(3,4,600,0)+b"\x0c\x07"+move+line+end,"advance 1 600\noutline 1\nm 3 4\nl 15 4\nh\n")
    for name, op, args in [("hstem",b"\x01",(0,10)),("vstem",b"\x03",(0,10)),
                            ("hstem3",b"\x0c\x02",(0,10,20,10,40,10)),
                            ("vstem3",b"\x0c\x01",(0,10,20,10,40,10)),("dotsection",b"\x0c\x00",())]:
        yield t1(name,width+operands(*args)+op+move+line+end)
    for n in (24,25):
        args=(12,0)+tuple(0 for _ in range(n-4))
        program=tmove+operands(*args,len(args),99)+b"\x0c\x10\x0c\x11\x0c\x11\x05"+end
        yield t1(f"stack-{n}",program,small if n==24 else "outline-error 1\n")
    for seed in (0,7):
        bits=struct.unpack(">Q",struct.pack(">d",float(seed)))[0]
        state=(bits>>32)^(bits&0xffffffff)
        first=((state*1664525+1013904223)&0xffffffff)
        second=((first*1664525+1013904223)&0xffffffff)
        x,y=(first+1)/2**32,(second+1)/2**32
        yield cff(f"random-{seed}",move+b"\x0c\x17\x0c\x17\x05"+end,
                  f"outline 1\nm 0 0\nl {x} {y}\nh\n",private=operands(seed)+b"\x0c\x13")
    # The fifth argument to endchar is a width; seac component names use
    # StandardEncoding even when the font's own code vector differs (Appendix C).
    yield vector("review-cff-seac-nested",cff_font(hostile="seac-cycle",conforming=True),"outline-error 3\n","font-cff","cff")
    yield cff("trailing-endchar",end+operands(0),"outline-error 1\n")
    yield t1("trailing-endchar",width+end+operands(0),"outline-error 1\n")

    yield t1("seac-bearings",variant="seac-bearings",expected="outline 3\nm 40 0\nl 140 0\nl 140 200\nl 40 200\nh\nm 40 200\nl 60 240\nh\n")
    yield t1("seac-nested",width+operands(0,30,200,65,194)+b"\x0c\x06","outline-error 3\n")
    yield vector("review-cff-cid-format4",cff_font(cid=True,variant="fd4",conforming=True),"parse-error\n","font-cff","cff")
    yield cff("hint-empty",b"\x01"+move+line+end,"outline-error 1\n")
    for name, encoding in [("supplement-unencoded",b"\x80\x01\x41\x01\x5a"+u16(391)),
                           ("empty-encoding-format",b"\x02\x00")]:
        data=metadata_font(3,3)
        original=b"\x80\x05\x41\x42\x43\x44\x45\x01\x5a"+u16(391)
        assert data.endswith(original)
        yield vector("review-cff-"+name,data[:-len(original)]+encoding,"parse-error\n","font-cff","cff")

    for variant in ("missing-fonttype","missing-matrix","missing-header","subrs-incomplete","dict-count"):
        yield t1(variant,variant=variant,expected="parse-error\n")
    # Type 1 6.2 uses signed integers for byte 255, unlike Type 2 16.16.
    yield t1("long-div",tmove+b"\xff"+struct.pack(">i",100000)+operands(100)+b"\x0c\x0c"+operands(0)+b"\x05"+end,
             "outline 1\nm 0 0\nl 1000 0\nh\n")

    yield cff("fixed",move+b"\xff"+struct.pack(">i",98304)+operands(0)+b"\x05"+end,
              "outline 1\nm 0 0\nl 1.5 0\nh\n")
    yield cff("real",move+line+end,"advance 1 0.125\n"+small,private=b"\x1e\x0a\x12\x5f\x14")
    yield cff("real-padding",move+line+end,"parse-error\n",private=b"\x1e\x10\xf0\x14")
    for len_iv in (0,1):
        yield t1(f"leniv-{len_iv}",len_iv=len_iv,expected="advance 1 600\noutline 1\n"+RECT)

    for op,name in [(19,"hintmask"),(20,"cntrmask")]:
        yield cff("width-"+name,operands(25,0,10)+bytes([op,128])+move+line+end,
                  "advance 1 125\n"+small,private=operands(100)+b"\x15")
    yield vector("review-cff-width-seac",cff_font(variant="width-seac",conforming=True),
                 "advance 3 125\noutline 3\n"+RECT+"m 30 200\nl 50 240\nh\n","font-cff","cff")
    yield cff("flex1-equal",move+operands(1,1,1,1,1,1,1,1,1,1,2)+b"\x0c\x25"+end,
              "outline 1\nm 0 0\nc 1 1 2 2 3 3\nc 4 4 5 5 0 7\nh\n")

    yield t1("seac-trailing",variant="seac-trailing",expected="outline-error 3\n")
    for n in (65535,65536):
        padding=n-4-len(tmove+line+end)
        # dotsection changes only hints; one stem declaration handles odd size.
        prefix = operands(0,10)+b"\x01" if padding % 2 else b""
        program=width+prefix+b"\x0c\x00"*((padding-len(prefix))//2)+move+line+end
        assert len(program)+4==n
        yield t1(f"length-{n}",program,small if n==65535 else "parse-error\n")

    yield vector("review-cff-cid-count",cff_font(cid=True,variant="cid-count",conforming=True),
                 "parse-error\n","font-cff","cff")

    yield t1("close-line",tmove+line+b"\x09"+line+end,
             "outline 1\nm 0 0\nl 12 0\nh\nm 12 0\nl 24 0\nh\n")
    yield t1("setcurrentpoint",tmove+line+operands(3,4,2,99)+b"\x0c\x10\x0c\x11\x0c\x11\x0c\x21"+line+end,
             "outline 1\nm 0 0\nl 12 0\nl 15 4\nh\n")
