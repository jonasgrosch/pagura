"""Small independently written font programs and analytic glyph-space oracles.

Binary layouts follow OpenType tables and Adobe TN 5176/5177, referenced by
ISO 32000-2 clause 9.9. No engine is used to construct expected outlines.
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


def truetype(fmt=4, short=False, glyph_override=None, cff=None, cff2=False, table_overrides=None):
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


def cff_font(cid=False, cff2=False, hostile=None, variable=False, variant=None):
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
        rect = operands(0,0)+b"\x15"+operands(100,50,30,1)+b"\x10"+operands(0,0,200,-130,0)+b"\x05"
    glyphs = [end, rect, curve, seac, accent]
    if hostile == "recursive":
        rect_subr = number(-107)+b"\x0a\x0b"
    if hostile == "stack":
        glyphs[1] = number(0)*514+end
    if hostile == "mask":
        glyphs[1] = operands(0,10)+b"\x01\x13"  # Missing one mask byte.
    if hostile == "seac-cycle":
        glyphs[1] = operands(0,0,65,194)+end
    if variant == "global":
        glyphs[1] = operands(0,0)+b"\x15"+number(-107)+b"\x1d"+end
    charstrings = index(glyphs, cff2)
    private = offset(600)+b"\x14"+offset(100 if variant == "hints" else 0)+b"\x15"+offset(18)+b"\x13"
    assert len(private) == 18
    private += index([rect_subr], cff2)
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
            result += offset(18)+offset(private_at)+b"\x12"
        if variable:
            result += offset(vstore_at)+b"\x18"
        return result
    dummy = top(0,0,0,0,0,0)
    if cff2:
        prefix_len = 5+len(dummy)+len(index([rect_subr] if variant == "global" else [], True))
    else:
        prefix_len = 4+len(index([b"Fixture"]))+len(index([dummy]))+len(custom_strings)+len(index([rect_subr] if variant == "global" else []))
    chars_at = prefix_len
    charset_at = chars_at+len(charstrings)
    enc_at = charset_at+len(charset)
    private_at = enc_at+len(enc)
    first_private_len = len(private)
    if variant == "fd-array":
        private += offset(700)+b"\x14"+offset(0)+b"\x15"+offset(18)+b"\x13"+index([rect_subr],cff2)
        fdselect = bytes([0,0,1,0,0,0])
    fdarray_at = private_at+len(private)
    fonts = [offset(18)+offset(private_at)+b"\x12"]
    if variant == "fd-array":
        fonts.append(offset(18)+offset(private_at+first_private_len)+b"\x12"+operands(2,0,0,2,0,0)+b"\x0c\x07")
    fdarray = index(fonts, cff2)
    select_at = fdarray_at+len(fdarray)
    top_dict = top(chars_at,charset_at,enc_at,private_at,fdarray_at,select_at,select_at+len(fdselect))
    if cff2:
        prefix = b"\x02\x00\x05"+u16(len(top_dict))+top_dict+index([rect_subr] if variant == "global" else [],True)
    else:
        prefix = b"\x01\x00\x04\x04"+index([b"Fixture"])+index([top_dict])+custom_strings+index([rect_subr] if variant == "global" else [])
    # Two supports: one contributes zero at default, the all-zero peak
    # contributes one. The default blend is 100 + 0*50 + 1*30 = 130.
    regions = u16(1)+u16(2)+i16(0)+i16(16384)+i16(16384)+bytes(6)
    item_data = u16(0)+u16(0)+u16(2)+u16(0)+u16(1)
    store = u16(1)+u32(12)+u16(1)+u32(12+len(regions))+regions+item_data
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
        expected += f"code {('mac' if fmt == 0 else 'symbol' if fmt == 6 else 'unicode')} {0x1f600 if fmt == 12 else 65} 1\n"
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
