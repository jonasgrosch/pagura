"""Independent JBIG2 generic-region encoders (T.88 6.2, 7.2, Annex E).

Expected bitmaps are constructed geometrically before encoding. The arithmetic
encoder uses the MQ interval machine; the MMR encoder emits horizontal runs
or explicit reference-line modes. No Pagura code is imported or executed.
"""
from __future__ import annotations

import struct
from .model import Vector

AREA = "jbig2"
# T.88 Table E.1: Qe, MPS successor, LPS successor, MPS exchange.
MQ = (
    (0x5601,1,1,1),(0x3401,2,6,0),(0x1801,3,9,0),(0x0ac1,4,12,0),
    (0x0521,5,29,0),(0x0221,38,33,0),(0x5601,7,6,1),(0x5401,8,14,0),
    (0x4801,9,14,0),(0x3801,10,14,0),(0x3001,11,17,0),(0x2401,12,18,0),
    (0x1c01,13,20,0),(0x1601,29,21,0),(0x5601,15,14,1),(0x5401,16,14,0),
    (0x5101,17,15,0),(0x4801,18,16,0),(0x3801,19,17,0),(0x3401,20,18,0),
    (0x3001,21,19,0),(0x2801,22,19,0),(0x2401,23,20,0),(0x2201,24,21,0),
    (0x1c01,25,22,0),(0x1801,26,23,0),(0x1601,27,24,0),(0x1401,28,25,0),
    (0x1201,29,26,0),(0x1101,30,27,0),(0x0ac1,31,28,0),(0x09c1,32,29,0),
    (0x08a1,33,30,0),(0x0521,34,31,0),(0x0441,35,32,0),(0x02a1,36,33,0),
    (0x0221,37,34,0),(0x0141,38,35,0),(0x0111,39,36,0),(0x0085,40,37,0),
    (0x0049,41,38,0),(0x0025,42,39,0),(0x0015,43,40,0),(0x0009,44,41,0),
    (0x0005,45,42,0),(0x0001,45,43,0),(0x5601,46,46,0),
)


class ArithmeticEncoder:
    """MQ encoder with delayed carry propagation and seven-bit FF stuffing."""
    def __init__(self):
        self.a, self.c, self.remaining = 0x8000, 0, 12
        self.output = [0]  # Carry sink; not part of the encoded stream.
        self.contexts = {}

    def byte_out(self):
        if self.output[-1] != 255 and self.c >= 0x8000000:
            self.output[-1] += 1
            self.c &= 0x7ffffff
        if self.output[-1] == 255:
            shift, self.remaining = 20, 7
        else:
            shift, self.remaining = 19, 8
        self.output.append(self.c >> shift)
        self.c &= (1 << shift) - 1

    def encode(self, context, value):
        state, mps = self.contexts.get(context, (0, 0))
        qe, more, less, exchange = MQ[state]
        self.a -= qe
        if value == mps:
            if self.a >= 0x8000:
                self.c += qe
                return
            if self.a < qe:
                self.a = qe
            else:
                self.c += qe
            state = more
        else:
            if self.a < qe:
                self.c += qe
            else:
                self.a = qe
            state, mps = less, mps ^ exchange
        self.contexts[context] = state, mps
        while self.a < 0x8000:
            self.a *= 2
            self.c *= 2
            self.remaining -= 1
            if self.remaining == 0:
                self.byte_out()

    def finish(self):
        top = self.c + self.a
        self.c |= 0xffff
        if self.c >= top:
            self.c -= 0x8000
        self.c <<= self.remaining
        self.byte_out()
        self.c <<= self.remaining
        self.byte_out()
        output = self.output[1:]
        if output and output[-1] == 255:
            output.pop()
        return bytes(output) + b"\xff\xac"


# Geometric neighbourhoods in decreasing context-bit significance.
NEIGHBOURS = (
    [(x,-2) for x in range(-2,3)] + [(x,-1) for x in range(-3,4)] + [(x,0) for x in range(-4,0)],
    [(x,-2) for x in range(-1,3)] + [(x,-1) for x in range(-2,4)] + [(x,0) for x in range(-3,0)],
    [(x,-2) for x in range(-1,2)] + [(x,-1) for x in range(-2,3)] + [(x,0) for x in range(-2,0)],
    [(x,-1) for x in range(-3,3)] + [(x,0) for x in range(-4,0)],
)
DEFAULT_AT = (((3,-1),(-3,-1),(2,-2),(-2,-2)), ((3,-1),), ((2,-1),), ((2,-1),))


def arithmetic(rows, template, typical=False, adaptive=None):
    adaptive = adaptive or DEFAULT_AT[template]
    positions = list(NEIGHBOURS[template])
    for old, new in zip(DEFAULT_AT[template], adaptive):
        positions[NEIGHBOURS[template].index(old)] = new
    height, width = len(rows), len(rows[0])
    def pixel(x, y):
        return rows[y][x] if 0 <= y < height and 0 <= x < width else 0
    encoder = ArithmeticEncoder()
    ltp = 0
    for y, row in enumerate(rows):
        if typical:
            next_ltp = int(row == (rows[y-1] if y else [0]*width))
            encoder.encode((0x9b25,0x795,0xe5,0x195)[template], next_ltp ^ ltp)
            ltp = next_ltp
        if ltp:
            continue
        for x, value in enumerate(row):
            context = 0
            for dx, dy in positions:
                context = context * 2 + pixel(x+dx,y+dy)
            encoder.encode(context, value)
    flags = (template << 1) | (8 if typical else 0)
    return bytes([flags]) + b"".join(struct.pack("bb", *p) for p in adaptive) + encoder.finish()


WHITE = """00110101 000111 0111 1000 1011 1100 1110 1111 10011 10100 00111 01000 001000 000011 110100 110101
101010 101011 0100111 0001100 0001000 0010111 0000011 0000100 0101000 0101011 0010011 0100100 0011000 00000010 00000011 00011010
00011011 00010010 00010011 00010100 00010101 00010110 00010111 00101000 00101001 00101010 00101011 00101100 00101101 00000100 00000101 00001010
00001011 01010010 01010011 01010100 01010101 00100100 00100101 01011000 01011001 01011010 01011011 01001010 01001011 00110010 00110011 00110100""".split()
BLACK = """0000110111 010 11 10 011 0011 0010 00011 000101 000100 0000100 0000101 0000111 00000100 00000111 000011000
0000010111 0000011000 0000001000 00001100111 00001101000 00001101100 00000110111 00000101000 00000010111 00000011000 000011001010 000011001011 000011001100 000011001101 000001101000 000001101001
000001101010 000001101011 000011010010 000011010011 000011010100 000011010101 000011010110 000011010111 000001101100 000001101101 000011011010 000011011011 000001010100 000001010101 000001010110 000001010111
000001100100 000001100101 000001010010 000001010011 000000100100 000000110111 000000111000 000000100111 000000101000 000001011000 000001011001 000000101011 000000101100 000001011010 000001100110 000001100111""".split()
WHITE_MAKEUP = """11011 10010 010111 0110111 00110110 00110111 01100100 01100101 01101000 01100111 011001100 011001101 011010010 011010011 011010100 011010101 011010110 011010111 011011000 011011001 011011010 011011011 010011000 010011001 010011010 011000 010011011""".split()
BLACK_MAKEUP = """0000001111 000011001000 000011001001 000001011011 000000110011 000000110100 000000110101 0000001101100 0000001101101 0000001001010 0000001001011 0000001001100 0000001001101 0000001110010 0000001110011 0000001110100 0000001110101 0000001110110 0000001110111 0000001010010 0000001010011 0000001010100 0000001010101 0000001011010 0000001011011 0000001100100 0000001100101""".split()
COMMON = """00000001000 00000001100 00000001101 000000010010 000000010011 000000010100 000000010101 000000010110 000000010111 000000011100 000000011101 000000011110 000000011111""".split()


def run_code(length, black):
    output = ""
    while length >= 64:
        multiple = min(length // 64, 40)
        output += COMMON[multiple-28] if multiple >= 28 else (BLACK_MAKEUP if black else WHITE_MAKEUP)[multiple-1]
        length -= multiple * 64
    return output + (BLACK if black else WHITE)[length]


def bits_bytes(bits):
    bits += "0" * (-len(bits) % 8)
    return bytes(int(bits[i:i+8],2) for i in range(0,len(bits),8))


def mmr(rows):
    output = ""
    for row in rows:
        x, black = 0, 0
        while x < len(row):
            runs = []
            for color in (black, 1-black):
                start = x
                while x < len(row) and row[x] == color:
                    x += 1
                runs.append(x-start)
            output += "001" + run_code(runs[0],black) + run_code(runs[1],1-black)
    return b"\x01" + bits_bytes(output + "000000000001"*2)


def segment(number, kind, page, body, refs=(), long_page=False, extended=False):
    count = len(refs)
    header = struct.pack(">IB", number, kind | (64 if long_page else 0))
    if count > 4 or extended:
        header += struct.pack(">I", 0xe0000000 | count) + bytes((count+8)//8)
    else:
        header += bytes([count << 5])
    size = 1 if number <= 256 else 2 if number <= 65536 else 4
    header += b"".join(n.to_bytes(size,"big") for n in refs)
    header += page.to_bytes(4 if long_page else 1,"big")
    return header + struct.pack(">I",len(body)) + body


def page_info(width, height, flags=64, stripe=0):
    return struct.pack(">IIIIBH",width,height,0,0,flags,stripe)


def region(rows, coding, x=0, y=0, operator=0):
    return struct.pack(">IIIIB",len(rows[0]),len(rows),x,y,operator) + coding


def packed(rows):
    return b"".join(bits_bytes("".join(str(b) for b in row)) for row in rows)


def vector(name, data, rows=None, outcome="valid", requirement="jbig2-generic-arithmetic", globals=b"", feature=None, width=0, height=0):
    if rows:
        width,height=len(rows[0]),len(rows)
    params = f"outcome={outcome}\nwidth={width}\nheight={height}\n"
    if feature:
        params += f"feature={feature}\n"
    return Vector(name, {".jb2":data,".globals":globals,".out":packed(rows) if rows else b"", ".params":params.encode()},
                  (requirement,), "Independent T.88 encoding and geometric bitmap" if rows else "Malformed or explicitly unsupported embedded stream")


def generic_cases():
    patterns = {
        "white": [[0]*19 for _ in range(9)],
        "black": [[1]*19 for _ in range(9)],
        "checker": [[(x+y)%2 for x in range(19)] for y in range(9)],
        "geometry": [[int((x-9)**2+(y-4)**2 < 20 or x == y) for x in range(19)] for y in range(9)],
        "repeated": [[int(x%5 < 2 and 2 <= y < 8) for x in range(19)] for y in range(9)],
        "noise": [[((x*73+y*29+x*y*17) ^ (x*11+y*y*9)) >> ((x+y)%7) & 1 for x in range(47)] for y in range(31)],
    }
    for template in range(4):
        for typical in (False,True):
            for name,rows in patterns.items():
                encoded=arithmetic(rows,template,typical)
                data=segment(1,48,1,page_info(len(rows[0]),len(rows)))+segment(2,38,1,region(rows,encoded))
                yield vector(f"arithmetic-t{template}-{'typical-' if typical else ''}{name}",data,rows)
        rows=patterns["noise"]
        adaptive=((-6,-1),(5,-2),(-4,-3),(3,-4)) if template==0 else ((-5,-3),)
        data=segment(1,48,1,page_info(47,31))+segment(2,39,1,region(rows,arithmetic(rows,template,True,adaptive)))
        yield vector(f"arithmetic-t{template}-adaptive",data,rows)
    for name,rows in patterns.items():
        data=segment(1,48,1,page_info(len(rows[0]),len(rows)))+segment(2,38,1,region(rows,mmr(rows)))
        yield vector(f"mmr-{name}",data,rows,requirement="jbig2-generic-mmr")
    # Every T.4 terminating code and both colors of every makeup code.
    for name,lengths in (("terminating", range(64)),("makeup",range(64,2561,64))):
        rows=[]
        width=max(lengths)+4
        for length in lengths:
            rows += [[0]*length+[1]*(width-length), [1]*length+[0]*(width-length)]
        data=segment(1,48,1,page_info(width,len(rows)))+segment(2,38,1,region(rows,mmr(rows)))
        yield vector(f"mmr-{name}",data,rows,requirement="jbig2-generic-mmr")
    # Hand-constructed T.6 vertical offsets and pass mode, independently
    # describing the two rows rather than deriving modes with the decoder.
    for delta,code in ((-3,"0000010"),(-2,"000010"),(-1,"010"),(0,"1"),(1,"011"),(2,"000011"),(3,"0000011")):
        rows=[[int(8<=x<16) for x in range(24)], [int(8+delta<=x<16+delta) for x in range(24)]]
        first="001"+run_code(8,False)+run_code(8,True)+"1"
        coded=b"\x01"+bits_bytes(first+code+code+"1"+"000000000001"*2)
        data=segment(1,48,1,page_info(24,2))+segment(2,38,1,region(rows,coded))
        yield vector(f"mmr-vertical-{delta+3}",data,rows,requirement="jbig2-generic-mmr")
    rows=[[int(4<=x<8 or 12<=x<16) for x in range(24)], [0]*24]
    first="001"+run_code(4,False)+run_code(4,True)+"001"+run_code(4,False)+run_code(4,True)+"1"
    coded=b"\x01"+bits_bytes(first+"0001"*2+"1"+"000000000001"*2)
    yield vector("mmr-pass",segment(1,48,1,page_info(24,2))+segment(2,38,1,region(rows,coded)),rows,requirement="jbig2-generic-mmr")
    for operator in range(5):
        rows=[[int((x+y)%3==0) for x in range(11)] for y in range(7)]
        small=[[int(x!=y) for x in range(9)] for y in range(6)]
        data=segment(1,48,1,page_info(11,7))+segment(2,38,1,region(rows,arithmetic(rows,0)))
        data+=segment(3,38,1,region(small,arithmetic(small,3),x=5,y=4,operator=operator))
        result=[r[:] for r in rows]
        for y in range(3):
            for x in range(6):
                a,b=result[y+4][x+5],small[y][x]
                result[y+4][x+5]=(a|b,a&b,a^b,1^a^b,b)[operator]
        yield vector(f"combine-clipped-{operator}",data,result,requirement="jbig2-page-combination")
    rows=patterns["geometry"]
    data=segment(1,48,1,page_info(19,0xffffffff,64,0x8005))
    data+=segment(2,38,1,region(rows[:5],arithmetic(rows[:5],1)))+segment(3,50,1,struct.pack(">I",4))
    data+=segment(4,39,1,region(rows[5:],arithmetic(rows[5:],2),y=5))+segment(5,50,1,struct.pack(">I",8))
    yield vector("striped-unknown-height",data,rows,requirement="jbig2-page-combination")
    rows=[[1]*13 for _ in range(3)]
    data=segment(1,48,1,page_info(13,3,4))+segment(2,32,1,region(rows,arithmetic(rows,2)))
    yield vector("intermediate-default-black",data,rows,requirement="jbig2-page-combination")
    rows=[[0]*13 for _ in range(3)]
    data=segment(1,48,1,page_info(13,3,8))+segment(2,38,1,region([[1]*13 for _ in range(3)],arithmetic([[1]*13 for _ in range(3)],2),operator=4))
    yield vector("page-default-operator",data,outcome="unsupported",feature="jbig2.page-combination",requirement="jbig2-page-combination")
    # Both header widths: no dependence on any other coding implementation.
    for number in (255,256,257,65536,65537):
        data=segment(number,48,1,page_info(13,3),long_page=True)
        yield vector(f"header-number-{number}",data,rows,requirement="jbig2-embedded-segments")
    page=segment(1,48,1,page_info(19,9))
    good=page+segment(2,38,1,region(patterns["geometry"],arithmetic(patterns["geometry"],0)))
    hostile={
        "missing-page": b"",
        "truncated-header": good[:7],
        "truncated-body":good[:-1],
        "arithmetic-truncated":page+segment(2,38,1,region(patterns["geometry"],arithmetic(patterns["geometry"],0)[:-2])),
        "bad-adaptive":page+segment(2,38,1,region(patterns["geometry"],b"\x02\x00\x00\xff\xac")),
        "page-two":segment(1,48,2,page_info(19,9)),
        "page-zero":segment(1,48,0,page_info(19,9)),
        "duplicate-page":page+segment(2,48,1,page_info(19,9)),
        "duplicate-number":page+page,
        "zero-width":segment(1,48,1,page_info(0,9)),
        "reserved-page-flag":segment(1,48,1,page_info(19,9,128)),
        "end-page-forbidden":page+segment(2,49,1,b""),
        "end-file-forbidden":page+segment(2,51,1,b""),
        "forward-referral":page+segment(2,38,1,b"",refs=(3,)),
        "missing-referral":page+segment(3,38,1,b"",refs=(0,)),
        "unexpected-referral":page+segment(2,38,1,b"",refs=(1,)),
        "extended-referrals":page+segment(3,38,1,b"",refs=(1,)*5),
        "noncanonical-referrals":page+segment(3,38,1,b"",refs=(1,),extended=True),
        "reserved-referrals":b"\x00\x00\x00\x01\x30\xa0",
        "bad-stripe":page+segment(2,50,1,struct.pack(">I",9)),
        "unfinished-stripe":segment(1,48,1,page_info(19,0xffffffff,64,0x800a)),
        "mmr-truncated":page+segment(2,38,1,region(patterns["geometry"],b"\x01\x00")),
        "mmr-nonprogressing":page+segment(2,38,1,region(patterns["geometry"],b"\x01"+bits_bytes("001"+WHITE[0]+BLACK[0]))),
        "palette-forbidden":page+segment(2,54,1,b""),
    }
    for name,data in hostile.items():
        yield vector(name,data,outcome="syntax",requirement="jbig2-hostile-input")
    yield vector("cyclic-referral",page+segment(2,38,1,b"",refs=(2,)),outcome="cycle",requirement="jbig2-hostile-input")
    yield vector("huge-dimensions",segment(1,48,1,page_info(0xfffffffe,0xfffffffe)),outcome="limit",requirement="jbig2-hostile-input")
    for kind,feature in ((0,"symbol-dictionary"),(4,"text-region"),(16,"pattern-dictionary"),(20,"halftone-region"),(40,"refinement-region"),(53,"huffman-table"),(62,"extension"),(63,"segment-type")):
        yield vector(f"unsupported-{feature}",page+segment(2,kind,1,b""),outcome="unsupported",feature=f"jbig2.{feature}",requirement="jbig2-explicit-unsupported")
    yield vector("unsupported-unknown-length",page+struct.pack(">IBBBI",2,38,0,1,0xffffffff),outcome="unsupported",feature="jbig2.unknown-length",requirement="jbig2-explicit-unsupported")


def integer(encoder, family, value):
    """T.88 Annex A sign, magnitude category and binary suffix."""
    sign = int(value is None or value < 0)
    magnitude = 0 if value is None else abs(value)
    for category,(offset,size) in enumerate(((0,2),(4,4),(20,6),(84,8),(340,12),(4436,32))):
        if magnitude < offset+(1 << size):
            break
    suffix=magnitude-offset
    prefix="1"*category + ("0" if category < 5 else "")
    bits=str(sign)+prefix+f"{suffix:0{size}b}"
    history=1
    for bit in map(int,bits):
        encoder.encode((family,history),bit)
        history=(history*2+bit) if history < 256 else ((history*2+bit)&511)|256


def identifier(encoder,count,value):
    width=(count-1).bit_length()
    history=1
    for shift in range(width-1,-1,-1):
        bit=(value >> shift)&1
        encoder.encode(("identifier",history),bit)
        history=history*2+bit


def dictionary(glyphs,template,selection=None,imported=()):
    encoder=ArithmeticEncoder()
    previous_height=0
    ordered=sorted(glyphs,key=len)
    for height in sorted(set(map(len,ordered))):
        integer(encoder,"height",height-previous_height)
        previous_height=height
        previous_width=0
        for rows in (r for r in ordered if len(r)==height):
            width=len(rows[0])
            integer(encoder,"width",width-previous_width)
            previous_width=width
            for y,row in enumerate(rows):
                for x,value in enumerate(row):
                    context=0
                    for dx,dy in NEIGHBOURS[template]:
                        px,py=x+dx,y+dy
                        context=context*2+(rows[py][px] if 0<=px<width and 0<=py<height else 0)
                    encoder.encode(("pixels",context),value)
        integer(encoder,"width",None)
    all_symbols=list(imported)+ordered
    selection=selection if selection is not None else [True]*len(all_symbols)
    index=0
    export=False
    while index<len(all_symbols):
        end=index
        while end<len(all_symbols) and selection[end]==export:
            end+=1
        integer(encoder,"export",end-index)
        export=not export
        index=end
    body=struct.pack(">H",template<<10)+b"".join(struct.pack("bb",*p) for p in DEFAULT_AT[template])
    body+=struct.pack(">II",sum(selection),len(ordered))+encoder.finish()
    return body,[glyph for glyph,keep in zip(all_symbols,selection) if keep]


def text_region(symbols,placements,width,height,strips=1,offset=0,operator=0,default=0):
    encoder=ArithmeticEncoder()
    integer(encoder,"strip",0)
    previous_strip=previous_first=0
    rows=[[default]*width for _ in range(height)]
    for index,(x,y,glyph_id) in enumerate(placements):
        new_strip=index==0 or placements[index-1][1]//strips!=y//strips
        if new_strip:
            strip=y//strips*strips
            integer(encoder,"strip",(strip-previous_strip)//strips)
            previous_strip=strip
            integer(encoder,"first",x-previous_first)
            previous_first=x
        else:
            px,py,pid=placements[index-1]
            integer(encoder,"advance",x-(px+len(symbols[pid][0])-1)-offset)
        if strips>1:
            integer(encoder,"within",y%strips)
        identifier(encoder,len(symbols),glyph_id)
        glyph=symbols[glyph_id]
        for gy,row in enumerate(glyph):
            for gx,b in enumerate(row):
                if 0<=x+gx<width and 0<=y+gy<height:
                    a=rows[y+gy][x+gx]
                    rows[y+gy][x+gx]=(a|b,a&b,a^b,1^a^b)[operator]
        last=index+1==len(placements)
        if not last and placements[index+1][1]//strips!=y//strips:
            integer(encoder,"advance",None)
    flags=0x10+((strips.bit_length()-1)<<2)+(operator<<7)+(default<<9)+((offset&31)<<10)
    return struct.pack(">IIIIBHI",width,height,0,0,0,flags,len(placements))+encoder.finish(),rows


# Generic and symbol vectors share the fixture driver entry point.

def cases():
    for case in generic_cases():
        if case.name=="unsupported-symbol-dictionary":
            page=segment(1,48,1,page_info(19,9))
            yield vector(case.name,page+segment(2,0,1,b"\x00\x01"),outcome="unsupported",feature="jbig2.symbol-dictionary",requirement="jbig2-explicit-unsupported")
        elif case.name=="unsupported-text-region":
            page=segment(1,48,1,page_info(19,9))
            body=struct.pack(">IIIIBH",19,9,0,0,0,1)
            yield vector(case.name,page+segment(2,4,1,body),outcome="unsupported",feature="jbig2.text-region",requirement="jbig2-explicit-unsupported")
        else:
            yield case
    glyphs=[[[1,0,1],[0,1,0],[1,0,1]],[[1,1],[1,0],[1,1]],[[1,0,0,1],[1,1,1,1]],[[1]],[[0,1,0],[1,1,1]]]
    for template in range(4):
        body,symbols=dictionary(glyphs,template)
        globals=segment(0,0,0,body)
        placements=[(1,1,0),(5,1,1),(10,1,2),(0,6,3),(6,6,4),(12,6,1)]
        coded,rows=text_region(symbols,placements,19,12)
        data=segment(1,48,1,page_info(19,12))+segment(2,6,1,coded,refs=(0,))
        yield vector(f"symbols-t{template}-text",data,rows,globals=globals,requirement="jbig2-arithmetic-symbols")
    body,symbols=dictionary(glyphs,2,[True,False,True,False,True])
    globals=segment(0,0,0,body)
    for strips in (1,2,4,8):
        for offset in (-16,-1,0,15):
            placements=[(-1,0,0),(3,0,1),(9,0,2),(1,strips+1,2),(7,strips+1,1)]
            coded,rows=text_region(symbols,placements,17,14,strips,offset)
            data=segment(1,48,1,page_info(17,14))+segment(2,7,1,coded,refs=(0,))
            yield vector(f"text-strips-{strips}-offset-{offset+16}",data,rows,globals=globals,requirement="jbig2-arithmetic-text")
    # Every arithmetic integer category, signs, and the 32-bit tail, with
    # deliberately off-image text coordinates whose expected pixels are empty.
    for value in (0,3,4,19,20,83,84,339,340,4435,4436,65535,0xffffffff,0x100001152):
        for sign in (-1,1):
            coded,rows=text_region(symbols,[(sign*value,1,0)],17,5)
            data=segment(1,48,1,page_info(17,5))+segment(2,6,1,coded,refs=(0,))
            yield vector(f"integer-coordinate-{value}-{'negative' if sign<0 else 'positive'}",data,rows,globals=globals,requirement="jbig2-arithmetic-text")
    for op in range(4):
        coded,rows=text_region(symbols,[(1,1,0),(1,1,1),(3,1,2)],13,7,operator=op,default=1)
        data=segment(1,48,1,page_info(13,7))+segment(2,6,1,coded,refs=(0,))
        yield vector(f"text-combination-{op}",data,rows,globals=globals,requirement="jbig2-arithmetic-text")
    # Imported and new symbols are filtered through export runs in a second
    # global dictionary. The image refers only to the second dictionary.
    body2,selected=dictionary([[[1,1,0],[0,1,1]]],1,[False,True,False,True],imported=symbols)
    globals2=globals+segment(1,0,0,body2,refs=(0,))
    coded,rows=text_region(selected,[(1,1,0),(8,1,1)],17,6)
    data=segment(2,48,1,page_info(17,6))+segment(3,6,1,coded,refs=(1,))
    yield vector("symbol-import-and-export",data,rows,globals=globals2,requirement="jbig2-arithmetic-symbols")
    page=segment(1,48,1,page_info(17,6))
    for name,flags,feature in (("symbol-refinement",2,"symbol-refinement"),("symbol-context",256,"symbol-context")):
        yield vector(name,page+segment(2,0,1,struct.pack(">H",flags)),outcome="unsupported",feature="jbig2."+feature,requirement="jbig2-explicit-unsupported")
    for name,flags in (("text-refinement",0x12),("text-layout",0)):
        coded=struct.pack(">IIIIBH",17,6,0,0,0,flags)
        yield vector(name,page+segment(2,6,1,coded),outcome="unsupported",feature="jbig2."+name,requirement="jbig2-explicit-unsupported")

    for number in (256,257,65536,65537,70002):
        body,one=dictionary([[[1,0,1],[0,1,0]]],2)
        globals_wide=segment(number-2,0,0,body)
        coded,rows=text_region(one,[(1,1,0)],7,5)
        data=segment(number-1,48,1,page_info(7,5),long_page=True)+segment(number,6,1,coded,refs=(number-2,),long_page=True)
        yield vector(f"symbol-reference-width-{number}",data,rows,globals=globals_wide,requirement="jbig2-embedded-segments")
    globals_extended=b""
    imported=[]
    for number in range(5):
        body,glyph=dictionary([[[int(x==number%3) for x in range(3)]]],2)
        globals_extended+=segment(number,0,0,body)
        imported+=glyph
    body,exported=dictionary([],2,imported=imported)
    globals_extended+=segment(6,0,0,body,refs=tuple(range(5)))
    coded,rows=text_region(exported,[(1+3*i,1,i) for i in range(5)],17,4)
    data=segment(7,48,1,page_info(17,4))+segment(8,6,1,coded,refs=(6,))
    yield vector("extended-referrals-valid",data,rows,globals=globals_extended,requirement="jbig2-embedded-segments")
    rows=[[0]*7 for _ in range(3)]
    for template in range(4):
        long_rows=[[0]*16384]
        data=segment(1,48,1,page_info(16384,1))+segment(2,38,1,region(long_rows,arithmetic(long_rows,template)))
        yield vector(f"arithmetic-long-tail-{template}",data,long_rows)
    page=segment(1,48,1,page_info(7,3))
    yield vector("unsupported-extended-template",page+segment(2,38,1,region(rows,b"\x10")),outcome="unsupported",feature="jbig2.extended-template",requirement="jbig2-explicit-unsupported")
    yield vector("unsupported-mmr-extension",page+segment(2,38,1,region(rows,b"\x01"+bits_bytes("0000001"))),outcome="unsupported",feature="jbig2.mmr-extension",requirement="jbig2-explicit-unsupported")
    encoder=ArithmeticEncoder()
    integer(encoder,"height",None)
    body=struct.pack(">HbbII",2<<10,2,-1,1,1)+encoder.finish()
    yield vector("symbol-out-of-band-height",page+segment(2,0,1,body),outcome="syntax",requirement="jbig2-hostile-input")
    body=struct.pack(">HbbII",2<<10,2,-1,1,0xffffffff)+b"\xff\xac"
    yield vector("symbol-count-overflow",page+segment(2,0,1,body),outcome="limit",requirement="jbig2-hostile-input")
    # An unused identifier in a non-power-of-two alphabet is malformed.
    body,symbols=dictionary([[[1]],[[1,0]],[[0,1,0]]],2)
    globals_bad=segment(0,0,0,body)
    encoder=ArithmeticEncoder()
    integer(encoder,"strip",0); integer(encoder,"strip",1); integer(encoder,"first",1)
    identifier(encoder,3,3)
    coded=struct.pack(">IIIIBHI",7,3,0,0,0,0x10,1)+encoder.finish()
    yield vector("text-unused-identifier",page+segment(2,6,1,coded,refs=(0,)),outcome="syntax",globals=globals_bad,requirement="jbig2-hostile-input")
    coded,_=text_region(symbols,[(1,1,0)],7,3)
    yield vector("text-wrong-referral-type",page+segment(2,6,1,coded,refs=(1,)),outcome="syntax",requirement="jbig2-hostile-input")

    rows=[[0]*7 for _ in range(3)]
    payload=arithmetic(rows,2)
    bad=payload[:-2]+b"\xff\x90\xff\xac"
    yield vector("arithmetic-invalid-stuffing",page+segment(2,38,1,region(rows,bad)),outcome="syntax",requirement="jbig2-hostile-input")
