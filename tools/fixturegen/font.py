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
    private_extra = operands(-210,10)+b"\x07"+operands(-10,10)+b"\x06" if variant == "blue-order" else b""
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
    yield from recovery_cases()
    yield from static_font_cases()
    yield from metadata_recovery_cases()
    yield from descriptor_flag_cases()
    yield from glyph_name_cases()

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


def reviewed_cff(program, subrs=(), global_subrs=(), private=b"", top=b"", omit_private=False, header_size=4, encoding=None):
    """Two-glyph CFF, authored from TN 5176 sections 4-16, not an engine."""
    names, strings, globals_ = index([b"Reviewed"]), index([]), index(global_subrs)
    # FontMatrix/charset/Encoding defaults deliberately omitted.
    def dictionary(chars_at, private_at, size):
        result = top+offset(chars_at)+b"\x11"
        if not omit_private:
            result += offset(size)+offset(private_at)+b"\x12"
        if encoding is not None:
            result += offset(private_at+size+len(index(subrs) if subrs else b""))+b"\x10"
        return result
    extra = private
    if subrs:
        extra += offset(len(private)+6)+b"\x13"
    chars = index([b"\x0e", program])
    dummy = dictionary(0,0,len(extra))
    at = header_size+len(names)+len(index([dummy]))+len(strings)+len(globals_)
    return (bytes([1,0,header_size,4])+bytes(header_size-4)+names
            +index([dictionary(at,at+len(chars),len(extra))])+strings+globals_+chars
            +extra+(index(subrs) if subrs else b"")+(encoding or b""))


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


def recovery_vector(name, data, expected, tag, glyph=None, fmt="truetype", requirement="font-recovery"):
    """ADR 0003 policy oracles: strict tag and exact recovered glyph/mapping."""
    phase = "parse" if glyph is None else f"outline {glyph}"
    tags = [tag]
    if name in ("loca-truncated", "glyf-truncated"):
        tags.append("font.glyph-data")
    return Vector("repair-"+name,
                  {".font": data, ".expect": expected.encode(),
                   ".strict": f"{phase} {tag}\n".encode(),
                   ".repairs": ("\n".join(tags)+"\n").encode()},
                  (requirement,), "Reported font recovery with exact retained geometry",
                  extra={"format": fmt, "repairs": tags})


def font_tables(data):
    """Decode only this writer's sfnt directory for independent mutations."""
    result = {}
    for at in range(12, 12+16*int.from_bytes(data[4:6], "big"), 16):
        start, length = struct.unpack(">II", data[at+8:at+16])
        result[data[at:at+4]] = data[start:start+length]
    return result


def recovery_cases():
    base = truetype()
    tables = font_tables(base)
    result = "count 5\nadvance 1 600\noutline 1\n"+RECT
    yield vector("sfnt-legacy-true", b"true"+base[4:], result, "font-truetype")
    yield recovery_vector("sfnt-version", b"????"+base[4:], result, "font.sfnt-version")
    data = bytearray(base)
    # Shift the whole table body by one byte; offsets are still exact.
    end = 12+16*len(tables)
    data[end:end] = b"\x00"
    for at in range(12, end, 16):
        start = int.from_bytes(data[at+8:at+12], "big")
        data[at+8:at+12] = u32(start+1)
    yield recovery_vector("table-alignment", bytes(data), result, "font.table-alignment")
    # An optional final table overstates its extent, with all required data intact.
    data = bytearray(sfnt({**tables, b"zzzz": b"\x00\x00\x00\x00"}))
    at = 12+16*len(tables)
    data[at+12:at+16] = u32(100)
    yield recovery_vector("table-bounds", bytes(data), result, "font.table-bounds")
    # A required table may also overstate its extent if all consumed fields exist.
    reduced = {k:v for k,v in tables.items() if k < b"maxp"}
    reduced[b"maxp"] = tables[b"maxp"]
    data = bytearray(sfnt(reduced))
    at = 12+16*(len(reduced)-1)
    data[at+12:at+16] = u32(100)
    yield recovery_vector("required-table-bounds", bytes(data), result, "font.table-bounds")
    yield recovery_vector("metrics-bearings", truetype(table_overrides={b"hmtx":tables[b"hmtx"][:8]}),
                          result+"advance 4 600\n", "font.table-bounds")
    # loca retains complete entries; unavailable glyphs cannot consume neighbours.
    loca = tables[b"loca"][:-4]
    yield recovery_vector("loca-truncated", truetype(table_overrides={b"loca":loca}),
                          result+"outline 4\n", "font.table-bounds")
    # A complete earlier glyph remains available when glyf loses its tail.
    glyf = tables[b"glyf"][:-8]
    yield recovery_vector("glyf-truncated", truetype(table_overrides={b"glyf":glyf}),
                          result+"outline 4\n", "font.table-bounds")
    yield recovery_vector("glyph-truncated", truetype(glyph_override=b"\x00"),
                          "outline 1\noutline 2\n"+QUAD, "font.glyph-data", 1)
    # Put the component in glyph 3 instead to avoid a recursive reference.
    glyf = bytearray(tables[b"glyf"])
    start = int.from_bytes(tables[b"loca"][12:16], "big")
    glyf[start+10:start+12] = u16(0x93)  # 0x83 matrix + reserved bit
    yield recovery_vector("component-matrix", truetype(table_overrides={b"glyf":bytes(glyf)}),
                          result+"outline 3\nm 20 30\nl 70 55\nl 70 155\nl 20 130\nh\n",
                          "font.component-flags", 3)
    # Format 6: one intact mapping, one absent mapping, original declared count.
    sub = u16(6)+u16(14)+u16(0)+u16(65)+u16(2)+u16(1)
    cm = u16(0)+u16(1)+u16(3)+u16(0)+u32(12)+sub
    yield recovery_vector("cmap-truncated", truetype(table_overrides={b"cmap":cm}),
                          "code symbol 65 1\nabsent symbol 66\n"+result, "font.cmap-truncated")
    # Format 12: retain one complete group, not a fabricated missing second group.
    cm = bytearray(cmap(12)); cm[16:20] = u32(40); cm[24:28] = u32(2)
    yield recovery_vector("cmap-groups", truetype(table_overrides={b"cmap":bytes(cm)}),
                          "code full 128512 1\ncode full 128513 2\nabsent full 128514\n"+result,
                          "font.cmap-truncated")
    cm = bytearray(cmap(4)); cm[42:44] = u16(65535)
    yield recovery_vector("cmap-sentinel", truetype(table_overrides={b"cmap":bytes(cm)}),
                          "code unicode 65 1\ncode unicode 65535 0\n"+result, "font.cmap-alignment")
    # Odd idRangeOffset is measured in bytes, so insert a pad before its glyphs.
    cm = bytearray(cmap(4)); cm[14:16] = u16(37); cm[40:42] = u16(5); cm[44:44] = b"\x00"
    yield recovery_vector("cmap-byte-alignment", truetype(table_overrides={b"cmap":bytes(cm)}),
                          "code unicode 65 1\ncode unicode 66 0\n"+result, "font.cmap-alignment")
    # Bad segment order is unusable, but a second subtable still maps correctly.
    good = cmap(6)[12:]; bad = bytearray(cmap(4)[12:]); bad[20:22] = u16(67)
    cm = u16(0)+u16(2)+u16(3)+u16(1)+u32(20)+u16(3)+u16(0)+u32(20+len(bad))+bad+good
    yield recovery_vector("cmap-subtable", truetype(table_overrides={b"cmap":bytes(cm)}),
                          "absent unicode 65\ncode symbol 65 1\n"+result, "font.cmap-data")
    # Both formats leave undefined escapes without execution semantics: stop here, keep prefix,
    # and never execute the syntactically valid suffix after the unknown opcode.
    move = operands(0,0)+b"\x15"; line = operands(12,0)+b"\x05"
    prefix = move+line; reserved = b"\x0c\xff"; suffix = operands(80,0)+b"\x05\x0e"
    expected = "outline 1\nm 0 0\nl 12 0\nh\noutline 0\n"
    yield recovery_vector("cff-escape", reviewed_cff(prefix+reserved+suffix), expected,
                          "font.charstring-operator", 1, "cff")
    yield recovery_vector("cff-subr-escape", reviewed_cff(move+number(-107)+b"\x0a"+suffix,
                          subrs=[line+reserved+b"\x0b"]), expected, "font.charstring-operator", 1, "cff")
    width = operands(0,600)+b"\x0d"
    for name, program, subrs in [("type1-escape", width+prefix+reserved+suffix, None),
                                 ("type1-subr-escape", width+move+operands(0)+b"\x0a"+suffix,
                                  [line+reserved+b"\x0b"])]:
        yield recovery_vector(name, type1_font(conforming=True, program=program, custom_subrs=subrs),
                              expected, "font.charstring-operator", 1, "type1")

    data = bytearray(truetype())
    for at in range(12, 12+16*len(tables), 16):
        if data[at:at+4] == b"post":
            data[at+8:at+12] = u32((len(data)+3)//4*4)
    yield recovery_vector("optional-table-missing", bytes(data), result, "font.table-bounds")
    # Completely unusable optional cmap header cannot invalidate glyph geometry.
    yield recovery_vector("cmap-header", truetype(table_overrides={b"cmap":b"\xff\xff"}),
                          "has unicode 0\n"+result, "font.cmap-data")

    yield recovery_vector("glyph-missing", base, "outline 999\n"+result, "font.glyph-data", 999)

    # Every supported array/group layout keeps its intact prefix independently.
    for fmt, original, shortened, mappings in [
        (0, cmap(0), 12+6+66, "code mac 65 1\nabsent mac 66\n"),
        (2, extended_cmap(2), 12+538, "code symbol 65 1\ncode symbol 33088 2\nabsent symbol 33089\n"),
        (4, cmap(4), 12+34, "code unicode 65 1\nabsent unicode 66\ncode unicode 65535 0\n"),
        (10, extended_cmap(10), 12+24, "code full 128512 1\ncode full 128513 0\nabsent full 128514\n"),
        (13, extended_cmap(13), 12+28, "code platform0 65 1\ncode platform0 67 1\nabsent platform0 128512\n"),
    ]:
        yield recovery_vector(f"cmap{fmt}-prefix", truetype(table_overrides={b"cmap":original[:shortened]}),
                              mappings+result, "font.cmap-truncated")
    yield recovery_vector("cff-global-escape", reviewed_cff(move+number(-107)+b"\x1d"+suffix,
                          global_subrs=[line+reserved+b"\x0b"]), expected, "font.charstring-operator", 1, "cff")


def static_type1(aliases=(b"-|", b"|-", b"|"), subrs=1, clear_definitions=False,
                 qualifier=b"executeonly", short=False, comments=False, computed=None, duplicate=None, glyph_name=b"A", notdef_last=False):
    """Type 1 2.4/7.3/10.3: explicit procedures and length-delimited cipher bytes."""
    reader, define, put = aliases
    bodies = [b"string currentfile exch readstring pop", b"def" if short else b"noaccess def",
              b"put" if short else b"noaccess put"]
    if comments:
        bodies = [body.replace(b" ", b" % ignored } { ( text\n", 1) for body in bodies]
    if computed is not None:
        index, body = computed
        bodies[index] = body
    declarations = b"".join(b"/"+name+b" {"+body+b"} "+qualifier+b" def\n"
                            for name,body in zip(aliases,bodies))
    clear = (b"%!PS-AdobeFont-1.0: Static 1.0\n/FontType 1 def\n"
             b"/FontMatrix [0.001 0 0 0.001 0 0] def\n/Encoding StandardEncoding def\n")
    if clear_definitions:
        clear += b"userdict begin\n"+declarations+b"end\n"
    clear += b"currentfile eexec\n"
    private = b"dup /Private 8 dict dup begin\n"
    if not clear_definitions:
        private += declarations
    private += b"/lenIV 4 def\n/Subrs "+str(subrs).encode()+b" array\n"
    line = (operands(100,0)+b"\x05"+operands(0,200)+b"\x05"+operands(-100,0)+b"\x05\x0b")
    for i in range(subrs):
        cipher = encrypt(bytes(4)+(line if i==0 else b"\x0b"),4330)
        private += b"dup "+str(i).encode()+b" "+str(len(cipher)).encode()+b" "+reader+b" "+cipher+b" "+put+b"\n"
    private += define+b"\n2 index /CharStrings "+(b"4" if duplicate==b"" else b"3" if duplicate is not None else b"2")+b" dict dup begin\n"
    width = operands(0,600)+b"\x0d"
    glyphs = [(b".notdef",width+b"\x0e"),
              (glyph_name,width+operands(0,0)+b"\x15"+operands(0)+b"\x0a\x0e")]
    if notdef_last:
        glyphs.reverse()
    if duplicate is not None:
        if duplicate==b"":
            glyphs.append((b"",width+b"\x0e"))
        glyphs.append((duplicate,operands(0,800)+b"\x0d\x0e"))
    for name, program in glyphs:
        cipher = encrypt(bytes(4)+program,4330)
        private += b"/"+name+b" "+str(len(cipher)).encode()+b" "+reader+b" "+cipher+b" "+define+b"\n"
    private += b"end end readonly put noaccess put\ncurrentfile closefile\n"
    return clear+encrypt(bytes(4)+private,55665)


def static_font_cases():
    expected = "count 2\nadvance 1 600\nname A 1\noutline 0\noutline 1\n"+RECT
    for name, options in [
        ("symbols", {}),
        ("canonical", {"aliases":(b"RD",b"ND",b"NP")}),
        ("custom", {"aliases":(b"readBytes",b"defineGlyph",b"storeSubr"), "qualifier":b"bind executeonly"}),
        ("clear", {"clear_definitions":True}),
        ("short", {"short":True, "qualifier":b"readonly"}),
        ("comments", {"comments":True}),
    ]:
        yield vector("type1-static-"+name, static_type1(**options), expected, "font-type1-aliases", "type1")
    for name, options in [
        ("reader", {"computed":(0,b"currentfile exch readstring pop")}),
        ("put", {"computed":(2,b"exch pop put")}),
        ("nested", {"computed":(2,b"{pop} noaccess put")}),
        ("custom", {"aliases":(b"RD",b"ND",b"customPut"), "computed":(2,b"exch pop put")}),
    ]:
        yield vector("type1-computed-"+name, static_type1(**options), "unsupported font.type1-postscript\n", "font-type1-aliases", "type1")

    rect = operands(0,0)+b"\x15"+operands(100,0,0,200,-100,0)+b"\x05\x0e"
    expected = "count 2\nadvance 1 600\noutline 1\n"+RECT
    width = operands(600)+b"\x14"
    for name, private in [
        ("operators", operands(-210,10)+b"\x07"+operands(-10,10)+b"\x06"),
        ("family-operators", operands(-210,10)+b"\x09"+operands(-10,10)+b"\x08"),
        ("values", operands(10,-5)+b"\x06"),
        ("other", operands(-10,10)+b"\x06"+operands(10,-5)+b"\x07"),
        ("family", operands(10,-5)+b"\x08"),
        ("family-other", operands(-10,10)+b"\x08"+operands(10,-5)+b"\x09"),
    ]:
        data = reviewed_cff(rect, private=private+width)
        yield recovery_vector("cff-blue-"+name, data, expected, "font.cff-blue-order", fmt="cff", requirement="font-cff-blue-order")
    # Type 1 5.3 explicitly permits these non-globally-sorted top-zone pairs.
    zones = operands(-15,15,700,15,-168,12)+b"\x06"
    yield vector("cff-blue-zone-order", reviewed_cff(rect, private=zones+width), expected, "font-cff-blue-order", "cff")
    data = reviewed_cff(rect, private=operands(-210,10)+b"\x07"+width)
    yield recovery_vector("cff-blue-opentype", sfnt({b"CFF ":data,b"cmap":cmap(4)},otto=True),
                          expected, "font.cff-blue-order", fmt="opentype", requirement="font-cff-blue-order")
    yield recovery_vector("cff-blue-cid", cff_font(cid=True,conforming=True,variant="blue-order"),
                          "count 5\nis-cid 1\nadvance 1 600\noutline 1\n"+RECT,
                          "font.cff-blue-order", fmt="cff", requirement="font-cff-blue-order")
    yield recovery_vector("cff-blue-cff2", truetype(cff=cff_font(cff2=True,conforming=True,variant="blue-order"),cff2=True),
                          "count 5\noutline 1\n"+RECT, "font.cff-blue-order", fmt="opentype", requirement="font-cff-blue-order")
    for name, data, fmt in [
        ("truetype", truetype(), "truetype"),
        ("cff", cff_font(conforming=True), "cff"),
        ("cff-cid", cff_font(cid=True,conforming=True,variant="fd-array"), "cff"),
        ("cff2", truetype(cff=cff_font(cff2=True,conforming=True,variable=True),cff2=True), "opentype"),
        ("type1-small", static_type1(subrs=1), "type1"),
        ("type1-large", static_type1(subrs=65), "type1"),
    ]:
        yield vector("memory-"+name, data, "outline 1\n"+("m 0 0\nl 130 0\nl 130 200\nl 0 200\nh\n" if name=="cff2" else "m 0 0\nl 200 0\nl 200 400\nl 0 400\nh\n" if name=="cff-cid" else RECT), "font-retained-bytes", fmt)


def metadata_recovery_cases():
    """TN 5176 4/12/15 and Type 1 6/10; reported, bounded admission policy."""
    rect = operands(0,0)+b"\x15"+operands(100,0,0,200,-100,0)+b"\x05\x0e"
    width = operands(600)+b"\x14"
    expected = "count 2\nadvance 1 600\noutline 0\noutline 1\n"+RECT
    for name, private in [
        ("empty-blue-values", b"\x06"),
        ("empty-other-blues", operands(-10,10)+b"\x06\x07"),
        ("empty-family-blues", b"\x08"),
        ("empty-family-other-blues", operands(-10,10)+b"\x08\x09"),
        ("empty-snap-h", b"\x0c\x0c"),
        ("empty-snap-v", b"\x0c\x0d"),
        ("scalar-hint-arity", operands(12,34)+b"\x0a"),
    ]:
        yield recovery_vector("cff-"+name, reviewed_cff(rect,private=private+width),
                              expected,"font.cff-dict-operands",fmt="cff",requirement="font-cff-dict-recovery")
    for name, options in [
        ("reserved-private", {"private":operands(11,22)+b"\x0c\x0f"+width}),
        ("reserved-top", {"top":operands(11,22)+b"\x16", "private":width}),
        ("reserved-repeated", {"private":(operands(11)+b"\x0c\xff")*64+width}),
    ]:
        yield recovery_vector("cff-"+name,reviewed_cff(rect,**options),expected,
                              "font.cff-dict-operator",fmt="cff",requirement="font-cff-dict-recovery")
    for name, options in [
        ("width-arity", {"private":operands(600,700)+b"\x14"}),
        ("subrs-arity", {"private":b"\x13"+width}),
        ("seed-arity", {"private":operands(1,2)+b"\x0c\x13"+width}),
        ("matrix-arity", {"top":operands(1,0,0,1,0)+b"\x0c\x07", "private":width}),
        ("reserved-number-31", {"private":b"\x1f"+width}),
        ("reserved-number-255", {"private":b"\xff"+width}),
    ]:
        yield vector("cff-dict-"+name,reviewed_cff(rect,**options),"parse-error\n",
                     "font-cff-dict-recovery","cff")
    for name, duplicate in [("name",b"A"),("notdef",b".notdef"),("empty",b"")]:
        result = expected.replace("count 2", "count 3" if duplicate==b"" else "count 2")
        result += "name A 1\nadvance 0 600\n"
        if duplicate==b"":
            result += "advance 2 600\noutline 2\n"
        yield recovery_vector("type1-duplicate-"+name,static_type1(duplicate=duplicate),result,
                              "font.type1-duplicate-glyph",fmt="type1",requirement="font-type1-duplicate-glyph")
    # A supplement may add another code only for a main-encoded glyph.
    # GID 1 has ISOAdobe SID 1 (space); no engine supplies these mappings.
    for name, encoding, mappings in [
        ("unencoded",b"\x80\x00\x01"+bytes([67])+u16(1),"encoding-none 67\n"),
        ("missing-sid",b"\x80\x01\x41\x02"+bytes([67])+u16(65000)+bytes([66])+u16(1),
         "encoding 65 space\nencoding-none 67\nencoding 66 space\n"),
        ("conflict",b"\x80\x01\x41\x02"+bytes([65])+u16(1)+bytes([66])+u16(1),
         "encoding 65 space\nencoding 66 space\n"),
        ("truncated",b"\x80\x01\x41\x02"+bytes([66])+u16(1)+bytes([67]),
         "encoding 65 space\nencoding 66 space\nencoding-none 67\n"),
        ("missing-count",b"\x80\x01\x41","encoding 65 space\n"),
    ]:
        yield recovery_vector("cff-supplement-"+name,reviewed_cff(rect,private=width,encoding=encoding),
                              expected+mappings,"font.cff-encoding-supplement",fmt="cff",requirement="font-cff-encoding-supplement")


def descriptor_flag_cases():
    """PDF 9.8.2/Table 121, 9.6.5.2/4: flag validity and explicit selection routes."""
    from .pdf import document, stream
    def vector_case(name, program, flags, *, encoding=b"/Encoding /WinAnsiEncoding",
                    subtype=b"TrueType", code=65, text="A", repaired=True, symbolic=False, glyph=1, decoded_text=None, unicode_entry=b"", objects=(), base_font=b"Rect"):
        font = (b"<< /Type /Font /Subtype /"+subtype+b" /BaseFont /"+base_font+b" /FontDescriptor 6 0 R "
                b"/FirstChar "+str(code).encode()+b" /LastChar "+str(code).encode()+b" /Widths [600] "+encoding+(b" "+unicode_entry if unicode_entry else b"")+b" >>")
        descriptor = (b"<< /Type /FontDescriptor /FontName /Rect /Flags "+str(flags).encode()+
                      b" /FontBBox [0 0 100 200] /ItalicAngle 0 /Ascent 200 /Descent 0 /CapHeight 200 /StemV 50 ")
        if subtype==b"Type1":
            descriptor += b"/FontFile 7 0 R >>"
            marker = program.index(b"currentfile eexec\n")+len(b"currentfile eexec\n")
            extra = b"/Length1 %d /Length2 %d /Length3 0" % (marker,len(program)-marker)
        else:
            descriptor += b"/FontFile2 7 0 R >>"
            extra = b"/Length1 "+str(len(program)).encode()
        pdf = document([b"<< /Type /Catalog /Pages 2 0 R >>",
                        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
                        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 16 16] /Resources << /Font << /F 5 0 R >> >> /Contents 4 0 R >>",
                        stream(b"BT /F 20 Tf 4 4 Td <"+bytes([code]).hex().encode()+b"> Tj ET"),
                        font,descriptor,stream(program,extra),*objects])
        # The writer's 100x200 glyph at 20 points is [4,6] x [4,8].
        pixels = bytes(v for y in range(16) for x in range(16)
                       for v in ([0,0,0] if glyph and 4<=x<6 and 8<=y<12 else [255,255,255]))
        expect = f"glyph {glyph}\ntext {text}\nadvance 0.6\nsymbolic {int(symbolic)}\n".encode()
        if decoded_text is not None:
            expect += f"decoded-text {decoded_text}\n".encode()
        files={".pdf":pdf,".ppm":b"P6\n16 16\n255\n"+pixels,".expect":expect,
               ".strict":b"font.descriptor-flags\n" if repaired else b"valid\n"}
        return Vector("descriptor-"+name,files,("font-descriptor-flags","font-glyph-name-unicode"),
                      "Descriptor flags select explicit glyphs, advances, extraction and exact pixels",
                      extra={"repairs":["font.descriptor-flags"] if repaired else []})
    # 9.10.2 reader fallback recovers the selected glyph's program name only
    # for extraction. Both flags and glyph selection stay on the raw route.
    for name,flags in [("both-post",36),("symbolic-latin-post",4)]:
        yield vector_case(name,truetype(),flags,text="A",decoded_text="",symbolic=True,
                          repaired=flags==36)
    compound = named_post([b".notdef",b"uni00410042_u1F600.alt",b"curve",b"composite",b"attached"])
    yield vector_case("post-agl-components",truetype(table_overrides={b"post":compound}),4,
                      text="AB😀",decoded_text="",symbolic=True,repaired=False)
    tounicode = stream(b"begincmap 1 begincodespacerange <00> <FF> endcodespacerange "
                       b"1 beginbfchar <41> <005A> endbfchar endcmap")
    yield vector_case("post-tounicode-priority",truetype(),4,text="Z",symbolic=True,repaired=False,
                      unicode_entry=b"/ToUnicode 8 0 R",objects=(tounicode,))
    # Name B selects glyph 1 through Unicode, whose post name is A. The
    # encoding's B is authoritative for text and must not become A.
    sub = u16(6)+u16(14)+u16(0)+u16(65)+u16(2)+u16(1)+u16(1)
    cm = u16(0)+u16(1)+u16(3)+u16(1)+u32(12)+sub
    yield vector_case("post-encoding-priority",truetype(table_overrides={b"cmap":cm}),32,
                      encoding=b"/Encoding << /Differences [65 /B] >>",text="B",repaired=False)
    unknown = named_post([b".notdef",b"unknownGlyph",b"curve",b"composite",b"attached"])
    yield vector_case("post-unknown-name",truetype(table_overrides={b"post":unknown}),4,
                      text="",symbolic=True,repaired=False)
    # Annex D.6 names are separate from AGL: a12 is U+261E and a19 is U+2713
    # only in the Dingbats family. Glyph IDs, widths and pixels stay fixed.
    for name,glyph_name,base,text in [
        ("post-nondingbats-a12",b"a12",b"Rect",""),
        ("post-nondingbats-a19",b"a19",b"Rect",""),
        ("post-zapfdingbats-a12",b"a12",b"ZapfDingbats","☞"),
        ("post-zapfdingbats-a19",b"a19",b"ABCDEF+ZapfDingbats-Bold","✓"),
        ("post-dingbats-a19",b"a19",b"Dingbats","✓"),
    ]:
        post = named_post([b".notdef",glyph_name,b"curve",b"composite",b"attached"])
        yield vector_case(name,truetype(table_overrides={b"post":post}),4,
                          base_font=base,text=text,decoded_text="",symbolic=True,repaired=False)
    post = named_post([b".notdef",b"a12",b"curve",b"composite",b"attached"])
    yield vector_case("post-nondingbats-a12-encoding",truetype(table_overrides={b"post":post}),32,
                      encoding=b"/Encoding << /Differences [65 /a12] >>",text="",repaired=False)
    # Without post names, these programs exercise only the cmap selection routes.
    unicode = truetype(omit_tables=(b"post",))
    symbol = truetype(fmt=6,omit_tables=(b"post",))
    # 9.8.2 uses Symbolic as primary: both stays raw, neither stays named.
    yield vector_case("both",unicode,36,text="",symbolic=True)
    yield vector_case("neither",unicode,0)
    # A valid pair is metadata, not evidence for overriding the 9.6.5 route.
    yield vector_case("symbolic-latin",unicode,4,text="",symbolic=True,repaired=False)
    yield vector_case("nonsymbolic-symbol",symbol,32,encoding=b"",text="",repaired=False)
    # The named route may use the symbol fallback only after its required
    # cmap/post mappings fail (9.6.5.4 final paragraph). This needs no repair.
    yield vector_case("nonsymbolic-symbol-encoding",symbol,32,repaired=False)
    differences = b"/Encoding << /BaseEncoding /WinAnsiEncoding /Differences [65 /B] >>"
    # The same Differences selects no B on the named route, but is ignored
    # on the raw Symbolic route, where byte 65 still selects rectangle A.
    yield vector_case("symbolic-latin-differences",unicode,4,encoding=differences,
                      text="",symbolic=True,repaired=False)
    yield vector_case("nonsymbolic-latin-differences",unicode,32,encoding=differences,
                      text="B",glyph=0,repaired=False)
    yield vector_case("symbol-post-named",truetype(fmt=6),32,repaired=False)
    yield vector_case("valid-latin",unicode,32,repaired=False)
    yield vector_case("valid-symbol",symbol,4,encoding=b"",text="",symbolic=True,repaired=False)
    yield vector_case("ignored-encoding-cycle",symbol,4,encoding=b"/Encoding 8 0 R",
                      text="",symbolic=True,repaired=False,objects=(b"8 0 R",))
    # A dual (3,0)/(3,1) cmap: Symbolic still takes the raw route and ignores PDF Differences (9.6.5.4).
    c0,c1 = cmap(6)[12:],cmap(4)[12:]
    dual = u16(0)+u16(2)+u16(3)+u16(0)+u32(20)+u16(3)+u16(1)+u32(20+len(c0))+c0+c1
    dual_font = truetype(omit_tables=(b"post",),table_overrides={b"cmap":dual})
    yield vector_case("both-ambiguous",dual_font,36,text="",symbolic=True)
    yield vector_case("neither-ambiguous",dual_font,0,symbolic=False)
    yield vector_case("ambiguous-symbol",dual_font,4,
                      encoding=b"/Encoding << /BaseEncoding /WinAnsiEncoding /Differences [65 /B] >>",
                      text="",symbolic=True,repaired=False)
    # Literal bytes stay the route without Encoding, even with Unicode cmaps.
    yield vector_case("unicode-no-encoding",unicode,4,encoding=b"",text="",symbolic=True,repaired=False)
    for name,glyph,flags,sym,text in [("type1-symbolic-latin",b"A",4,True,"A"),
                                      ("type1-nonsymbolic-symbol",b"alpha",32,False,"α")]:
        program=static_type1(glyph_name=glyph)
        program=program.replace(b"/Encoding StandardEncoding def",b"/Encoding 256 array 0 1 255 {1 index exch /.notdef put} for dup 65 /"+glyph+b" put readonly def")
        yield vector_case(name,program,flags,encoding=b"",subtype=b"Type1",symbolic=sym,text=text,repaired=False)
    # Type 1 Encoding overrides the built-in encoding regardless of flags.
    yield vector_case("type1-nonsymbolic-override",static_type1(glyph_name=b"alpha"),32,
                      encoding=b"/Encoding << /Differences [65 /alpha] >>",
                      subtype=b"Type1",text="α",repaired=False)


def named_post(names):
    """Independent post-2 name indices, including aliases at separate glyphs."""
    custom = list(dict.fromkeys(name for name in names if name != b".notdef"))
    indices = [0 if name==b".notdef" else 258+custom.index(name) for name in names]
    return (u32(0x20000)+bytes(28)+u16(len(names))+b"".join(u16(i) for i in indices)
            +b"".join(bytes([len(name)])+name for name in custom))


def glyph_name_cases():
    req = "font-program-glyph-names"
    aliases = named_post([b".notdef",b"A",b"A",b"composite",b"attached"])
    yield vector("glyph-names-post-alias",truetype(table_overrides={b"post":aliases}),
                 "count 5\nname A 1\nglyph-name 0 .notdef\nglyph-name 1 A\nglyph-name 2 A\n"
                 "glyph-name-none 5\nglyph-name-none 4294967295\noutline 1\n"+RECT+"outline 2\n"+QUAD,req)
    standard = u32(0x10000)+bytes(28)
    yield vector("glyph-names-post-standard",truetype(table_overrides={b"post":standard}),
                 "count 5\nglyph-name 0 .notdef\nglyph-name 3 space\n",req)
    yield vector("glyph-names-type1-reordered",static_type1(notdef_last=True),
                 "count 2\nname A 1\nglyph-name 0 .notdef\nglyph-name 1 A\noutline 1\n"+RECT,req,"type1")
    yield vector("glyph-names-cff",cff_font(conforming=True),
                 "count 5\nglyph-name 0 .notdef\nglyph-name 1 A\nglyph-name 4 acute\n",req,"cff")
    yield vector("glyph-names-cid",cff_font(cid=True,conforming=True),
                 "count 5\nglyph-name-none 0\nglyph-name-none 1\n",req,"cff")
    yield vector("glyph-names-cff2",cff_font(cff2=True,conforming=True),
                 "count 5\nglyph-name-none 0\nglyph-name-none 1\n",req,"cff")
