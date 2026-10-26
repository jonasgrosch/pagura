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
# T.88 (08/2018) Figure 3(b), Table 5: these replace twelve of the same
# sixteen neighbourhood positions. Their identity does not change when moved.
EXTENDED_AT = ((-2,0),(0,-2),(-2,-1),(-1,-2),(1,-2),(2,-1),
               (-3,0),(-4,0),(2,-2),(3,-1),(-2,-2),(-3,-1))


def arithmetic(rows, template, typical=False, adaptive=None, *, extended=False):
    nominal = EXTENDED_AT if extended else DEFAULT_AT[template]
    adaptive = adaptive or nominal
    positions = list(NEIGHBOURS[template])
    for old, new in zip(nominal, adaptive):
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
    flags = (template << 1) | (8 if typical else 0) | (16 if extended else 0)
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


def mmr(rows, eofb=True):
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
    return b"\x01" + bits_bytes(output + ("000000000001"*2 if eofb else ""))


def _segment(number, kind, page, body, refs=(), long_page=False, extended=False, retain=True):
    count = len(refs)
    header = struct.pack(">IB", number, kind | (64 if long_page else 0))
    if count > 4 or extended:
        header += struct.pack(">I", 0xe0000000 | count) + (((1 << (count+1))-1) if retain else 0).to_bytes((count+8)//8,"little")
    else:
        header += bytes([(count << 5) | (((1 << (count+1))-1) if retain else 0)])
    size = 1 if number <= 256 else 2 if number <= 65536 else 4
    header += b"".join(n.to_bytes(size,"big") for n in refs)
    header += page.to_bytes(4 if long_page else 1,"big")
    return header + struct.pack(">I",len(body)) + body


def _page_info(width, height, flags=98, stripe=0):
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
    requirements=(requirement,)
    if requirement=="jbig2-halftone": requirements+=("jbig2-pattern-dictionaries","jbig2-gray-scale")
    return Vector(name, {".jb2":data,".globals":globals,".out":packed(rows) if rows else b"", ".params":params.encode()},
                  requirements, "Independent T.88 encoding and geometric bitmap" if rows else "Malformed or explicitly unsupported embedded stream")


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
                data=_segment(1,48,1,_page_info(len(rows[0]),len(rows)))+_segment(2,38,1,region(rows,encoded))
                yield vector(f"arithmetic-t{template}-{'typical-' if typical else ''}{name}",data,rows)
        rows=patterns["noise"]
        adaptive=((-6,-1),(5,-2),(-4,-3),(3,-4)) if template==0 else ((-5,-3),)
        data=_segment(1,48,1,_page_info(47,31))+_segment(2,39,1,region(rows,arithmetic(rows,template,True,adaptive)))
        yield vector(f"arithmetic-t{template}-adaptive",data,rows)
    for name,rows in patterns.items():
        data=_segment(1,48,1,_page_info(len(rows[0]),len(rows)))+_segment(2,38,1,region(rows,mmr(rows)))
        yield vector(f"mmr-{name}",data,rows,requirement="jbig2-generic-mmr")
    # Every T.4 terminating code and both colors of every makeup code.
    for name,lengths in (("terminating", range(64)),("makeup",range(64,2561,64))):
        rows=[]
        width=max(lengths)+4
        for length in lengths:
            rows += [[0]*length+[1]*(width-length), [1]*length+[0]*(width-length)]
        data=_segment(1,48,1,_page_info(width,len(rows)))+_segment(2,38,1,region(rows,mmr(rows)))
        yield vector(f"mmr-{name}",data,rows,requirement="jbig2-generic-mmr")
    # Hand-constructed T.6 vertical offsets and pass mode, independently
    # describing the two rows rather than deriving modes with the decoder.
    for delta,code in ((-3,"0000010"),(-2,"000010"),(-1,"010"),(0,"1"),(1,"011"),(2,"000011"),(3,"0000011")):
        rows=[[int(8<=x<16) for x in range(24)], [int(8+delta<=x<16+delta) for x in range(24)]]
        first="001"+run_code(8,False)+run_code(8,True)+"1"
        coded=b"\x01"+bits_bytes(first+code+code+"1"+"000000000001"*2)
        data=_segment(1,48,1,_page_info(24,2))+_segment(2,38,1,region(rows,coded))
        yield vector(f"mmr-vertical-{delta+3}",data,rows,requirement="jbig2-generic-mmr")
    rows=[[int(4<=x<8 or 12<=x<16) for x in range(24)], [0]*24]
    first="001"+run_code(4,False)+run_code(4,True)+"001"+run_code(4,False)+run_code(4,True)+"1"
    coded=b"\x01"+bits_bytes(first+"0001"*2+"1"+"000000000001"*2)
    yield vector("mmr-pass",_segment(1,48,1,_page_info(24,2))+_segment(2,38,1,region(rows,coded)),rows,requirement="jbig2-generic-mmr")
    for operator in range(5):
        rows=[[int((x+y)%3==0) for x in range(11)] for y in range(7)]
        small=[[int(x!=y) for x in range(9)] for y in range(6)]
        data=_segment(1,48,1,_page_info(11,7))+_segment(2,38,1,region(rows,arithmetic(rows,0)))
        data+=_segment(3,38,1,region(small,arithmetic(small,3),x=5,y=4,operator=operator))
        result=[r[:] for r in rows]
        for y in range(3):
            for x in range(6):
                a,b=result[y+4][x+5],small[y][x]
                result[y+4][x+5]=(a|b,a&b,a^b,1^a^b,b)[operator]
        yield vector(f"combine-clipped-{operator}",data,result,requirement="jbig2-page-combination")
    rows=patterns["geometry"]
    data=_segment(1,48,1,_page_info(19,0xffffffff,64,0x8005))
    data+=_segment(2,38,1,region(rows[:5],arithmetic(rows[:5],1)))+_segment(3,50,1,struct.pack(">I",4))
    data+=_segment(4,39,1,region(rows[5:],arithmetic(rows[5:],2),y=5))+_segment(5,50,1,struct.pack(">I",8))
    yield vector("striped-unknown-height",data,rows,requirement="jbig2-page-combination")
    rows=[[1]*13 for _ in range(3)]
    data=_segment(1,48,1,_page_info(13,3,36))+_segment(2,36,1,region(rows,arithmetic(rows,2)))
    yield vector("intermediate-default-black",data,rows,requirement="jbig2-page-combination")
    rows=[[0]*13 for _ in range(3)]
    data=_segment(1,48,1,_page_info(13,3,8))+_segment(2,38,1,region([[1]*13 for _ in range(3)],arithmetic([[1]*13 for _ in range(3)],2),operator=4))
    yield vector("page-default-operator",data,outcome="syntax",requirement="jbig2-page-combination")
    # Both header widths: no dependence on any other coding implementation.
    for number in (255,256,257,65536,65537):
        data=_segment(number,48,1,_page_info(13,3),long_page=True)
        yield vector(f"header-number-{number}",data,rows,requirement="jbig2-embedded-segments")
    page=_segment(1,48,1,_page_info(19,9))
    good=page+_segment(2,38,1,region(patterns["geometry"],arithmetic(patterns["geometry"],0)))
    hostile={
        "missing-page": b"",
        "truncated-header": good[:7],
        "truncated-body":good[:-1],
        "arithmetic-truncated":page+_segment(2,38,1,region(patterns["geometry"],arithmetic(patterns["geometry"],0)[:-2])),
        "bad-adaptive":page+_segment(2,38,1,region(patterns["geometry"],b"\x02\x00\x00\xff\xac")),
        "page-two":_segment(1,48,2,_page_info(19,9)),
        "page-zero":_segment(1,48,0,_page_info(19,9)),
        "duplicate-page":page+_segment(2,48,1,_page_info(19,9)),
        "duplicate-number":page+page,
        "zero-width":_segment(1,48,1,_page_info(0,9)),
        "reserved-page-flag":_segment(1,48,1,_page_info(19,9,128)),
        "end-page-forbidden":page+_segment(2,49,1,b""),
        "end-file-forbidden":page+_segment(2,51,1,b""),
        "forward-referral":page+_segment(2,38,1,b"",refs=(3,)),
        "missing-referral":page+_segment(3,38,1,b"",refs=(0,)),
        "unexpected-referral":page+_segment(2,38,1,b"",refs=(1,)),
        "extended-referrals":page+_segment(3,38,1,b"",refs=(1,)*5),
        "noncanonical-referrals":page+_segment(3,38,1,b"",refs=(1,),extended=True),
        "reserved-referrals":b"\x00\x00\x00\x01\x30\xa0",
        "bad-stripe":page+_segment(2,50,1,struct.pack(">I",9)),
        "unfinished-stripe":_segment(1,48,1,_page_info(19,0xffffffff,64,0x800a)),
        "mmr-truncated":page+_segment(2,38,1,region(patterns["geometry"],b"\x01\x00")),
        "mmr-nonprogressing":page+_segment(2,38,1,region(patterns["geometry"],b"\x01"+bits_bytes("001"+WHITE[0]+BLACK[0]))),
        "palette-forbidden":page+_segment(2,54,1,b""),
    }
    for name,data in hostile.items():
        yield vector(name,data,outcome="syntax",requirement="jbig2-hostile-input")
    yield vector("cyclic-referral",page+_segment(2,38,1,b"",refs=(2,)),outcome="cycle",requirement="jbig2-hostile-input")
    yield vector("huge-dimensions",_segment(1,48,1,_page_info(0xfffffffe,0xfffffffe)),outcome="limit",requirement="jbig2-hostile-input")
    for kind,feature in ((0,"symbol-dictionary"),(4,"text-region"),(16,"pattern-dictionary"),(20,"halftone-region"),(40,"refinement-region"),(53,"huffman-table"),(62,"extension"),(63,"segment-type")):
        yield vector(f"unsupported-{feature}",page+_segment(2,kind,1,b""),outcome="unsupported",feature=f"jbig2.{feature}",requirement="jbig2-explicit-unsupported")
    yield vector("unknown-length-truncated",page+struct.pack(">IBBBI",2,38,0,1,0xffffffff),outcome="syntax",requirement="jbig2-hostile-input")


def integer(encoder, family, value):
    """T.88 Annex A sign, magnitude category and binary suffix."""
    sign = int(value is None or value < 0)
    magnitude = 0 if value is None else abs(value)
    for category,(offset,size) in enumerate(((0,2),(4,4),(20,6),(84,8),(340,12),(4436,32))):
        if magnitude < offset+(1 << size):
            break
    else:
        raise ValueError("integer outside the T.88 A.2 range")
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


def dictionary(glyphs,template,selection=None,imported=(),retain=False,reuse_contexts=None,return_contexts=False) -> tuple:
    encoder=ArithmeticEncoder()
    if reuse_contexts is not None: encoder.contexts.update(reuse_contexts)
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
    body=struct.pack(">H",(template<<10)+(512 if retain else 0)+(256 if reuse_contexts is not None else 0))+b"".join(struct.pack("bb",*p) for p in DEFAULT_AT[template])
    body+=struct.pack(">II",sum(selection),len(ordered))+encoder.finish()
    selected=[glyph for glyph,keep in zip(all_symbols,selection) if keep]
    contexts={k:v for k,v in encoder.contexts.items() if isinstance(k,tuple) and k[0] in ("pixels","refinement")}
    return (body,selected,contexts) if return_contexts else (body,selected)


def text_pixels(encoder,symbols,placements,width,height,strips=1,offset=0,operator=0,default=0,
                refinements=None,refine_template=0,alphabet_count=None,complete_last_strip=False,transposed=False,corner=1):
    integer(encoder,"strip",0)
    previous_strip=previous_first=0
    rows=[[default]*width for _ in range(height)]
    previous_width=0
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
            integer(encoder,"advance",x-(px+previous_width-1)-offset)
        if strips>1: integer(encoder,"within",y%strips)
        identifier(encoder,alphabet_count or len(symbols),glyph_id)
        glyph=symbols[glyph_id]
        if refinements is not None:
            refinement=refinements[index]
            integer(encoder,"refine-indicator",int(refinement is not None))
            if refinement is not None:
                refined,dx,dy=refinement
                dw,dh=len(refined[0])-len(glyph[0]),len(refined)-len(glyph)
                integer(encoder,"refine-width",dw);integer(encoder,"refine-height",dh)
                integer(encoder,"refine-x",dx);integer(encoder,"refine-y",dy)
                refinement_pixels(encoder,refined,glyph,refine_template,dw//2+dx,dh//2+dy)
                glyph=refined
        draw_x,draw_y=reference_origin(x,y,glyph,transposed,corner)
        for gy,row in enumerate(glyph):
            for gx,b in enumerate(row):
                if 0<=draw_x+gx<width and 0<=draw_y+gy<height:
                    a=rows[draw_y+gy][draw_x+gx]
                    rows[draw_y+gy][draw_x+gx]=(a|b,a&b,a^b,1^a^b)[operator]
        previous_width=len(glyph) if transposed else len(glyph[0])
        last=index+1==len(placements)
        if (last and complete_last_strip) or (not last and placements[index+1][1]//strips!=y//strips): integer(encoder,"advance",None)
    return rows


def _text_region(symbols,placements,width,height,strips=1,offset=0,operator=0,default=0,refinements=None,refine_template=0,transposed=False,corner=1,complete_last_strip=True):
    encoder=ArithmeticEncoder()
    rows=text_pixels(encoder,symbols,placements,width,height,strips,offset,operator,default,refinements,refine_template,transposed=transposed,corner=corner,complete_last_strip=complete_last_strip)
    flags=(corner<<4)+(64 if transposed else 0)+((strips.bit_length()-1)<<2)+(operator<<7)+(default<<9)+((offset&31)<<10)
    if refinements is not None: flags+=2+(refine_template<<15)
    header=struct.pack(">IIIIBH",width,height,0,0,0,flags)
    if refinements is not None and refine_template==0: header+=bytes([255,255,255,255])
    return header+struct.pack(">I",len(placements))+encoder.finish(),rows


# Generic and symbol vectors share the fixture driver entry point.

def cases():
    yield from extended_template_cases()
    yield from unknown_length_edge_cases()
    yield from text_layout_edge_cases()
    yield from stripe_identity_cases()
    yield from profiles_grammar_cases()
    yield from empty_text_cases()
    yield from mmr_reference_cases()
    yield from page_reference_crop_cases()
    yield from separate_stream_compatibility_cases()
    yield from header_and_table_edge_cases()
    yield from primary_rejection_cases()
    yield from metadata_cases()
    yield from arithmetic_layout_cases()
    yield from arithmetic_known_answer()
    yield from huffman_refinement_cases()
    yield from unknown_length_cases()
    yield from table_cases()
    yield from halftone_cases()
    yield from halftone_edge_cases()
    yield from specification_cases()
    yield from pdf_mapping_cases()
    yield from refinement_cases()
    yield from refined_symbol_cases()
    yield from refinement_hostile_cases()
    for case in generic_cases():
        if case.name in ("unsupported-pattern-dictionary","unsupported-halftone-region","unsupported-huffman-table","unsupported-segment-type","unsupported-symbol-dictionary","unsupported-text-region"):
            yield vector(case.name.replace("unsupported-","truncated-"),case.files[".jb2"],outcome="syntax",requirement="jbig2-hostile-input")
        elif case.name=="unsupported-extension":
            page=_segment(1,48,1,_page_info(19,9))
            yield vector(case.name,page+_segment(2,62,1,b"\xa0\x00\x00\x01"),outcome="unsupported",feature="jbig2.extension",requirement="jbig2-explicit-unsupported")
        elif case.name=="unsupported-refinement-region":
            data=case.files[".jb2"]
            yield vector("refinement-truncated",data,outcome="syntax",requirement="jbig2-hostile-input")
        else:
            yield case
    glyphs=[[[1,0,1],[0,1,0],[1,0,1]],[[1,1],[1,0],[1,1]],[[1,0,0,1],[1,1,1,1]],[[1]],[[0,1,0],[1,1,1]]]
    for template in range(4):
        body,symbols=dictionary(glyphs,template)
        globals=_segment(0,0,0,body)
        placements=[(1,1,0),(5,1,1),(10,1,2),(0,6,3),(6,6,4),(12,6,1)]
        coded,rows=_text_region(symbols,placements,19,12)
        data=_segment(1,48,1,_page_info(19,12))+_segment(2,6,1,coded,refs=(0,))
        yield vector(f"symbols-t{template}-text",data,rows,globals=globals,requirement="jbig2-arithmetic-symbols")
    body,symbols=dictionary(glyphs,2,[True,False,True,False,True])
    globals=_segment(0,0,0,body)
    for strips in (1,2,4,8):
        for offset in (-16,-1,0,15):
            placements=[(-1,0,0),(3,0,1),(9,0,2),(1,strips+1,2),(7,strips+1,1)]
            coded,rows=_text_region(symbols,placements,17,14,strips,offset)
            data=_segment(1,48,1,_page_info(17,14))+_segment(2,7,1,coded,refs=(0,))
            yield vector(f"text-strips-{strips}-offset-{offset+16}",data,rows,globals=globals,requirement="jbig2-arithmetic-text")
    # Every arithmetic integer category, signs, and the 32-bit tail, with
    # deliberately off-image text coordinates whose expected pixels are empty.
    for value in (0,3,4,19,20,83,84,339,340,4435,4436,65535,0xffffffff,0x100001152):
        for sign in (-1,1):
            coded,rows=_text_region(symbols,[(sign*value,1,0)],17,5)
            data=_segment(1,48,1,_page_info(17,5))+_segment(2,6,1,coded,refs=(0,))
            yield vector(f"integer-coordinate-{value}-{'negative' if sign<0 else 'positive'}",data,rows,globals=globals,requirement="jbig2-arithmetic-text")
    for op in range(4):
        coded,rows=_text_region(symbols,[(1,1,0),(1,1,1),(3,1,2)],13,7,operator=op,default=1)
        data=_segment(1,48,1,_page_info(13,7))+_segment(2,6,1,coded,refs=(0,))
        yield vector(f"text-combination-{op}",data,rows,globals=globals,requirement="jbig2-arithmetic-text")
    # Imported and new symbols are filtered through export runs in a second
    # global dictionary. The image refers only to the second dictionary.
    body2,selected=dictionary([[[1,1,0],[0,1,1]]],1,[False,True,False,True],imported=symbols)
    globals2=globals+_segment(1,0,0,body2,refs=(0,))
    coded,rows=_text_region(selected,[(1,1,0),(8,1,1)],17,6)
    data=_segment(2,48,1,_page_info(17,6))+_segment(3,6,1,coded,refs=(1,))
    yield vector("symbol-import-and-export",data,rows,globals=globals2,requirement="jbig2-arithmetic-symbols")
    page=_segment(1,48,1,_page_info(17,6))
    for name,flags,feature in (("symbol-refinement",2,"symbol-refinement"),("symbol-context",256,"symbol-context")):
        yield vector(name+"-truncated",page+_segment(2,0,1,struct.pack(">H",flags)),outcome="syntax",requirement="jbig2-hostile-input")
    for name,flags in (("text-refinement",0x12),("text-layout",0)):
        coded=struct.pack(">IIIIBH",17,6,0,0,0,flags)
        if name=="text-refinement":
            yield vector(name+"-truncated",page+_segment(2,6,1,coded),outcome="syntax",requirement="jbig2-hostile-input")
        else:
            yield vector(name+"-truncated",page+_segment(2,6,1,coded),outcome="syntax",requirement="jbig2-hostile-input")

    for number in (256,257,65536,65537,70002):
        body,one=dictionary([[[1,0,1],[0,1,0]]],2)
        globals_wide=_segment(number-2,0,0,body)
        coded,rows=_text_region(one,[(1,1,0)],7,5)
        data=_segment(number-1,48,1,_page_info(7,5),long_page=True)+_segment(number,6,1,coded,refs=(number-2,),long_page=True)
        yield vector(f"symbol-reference-width-{number}",data,rows,globals=globals_wide,requirement="jbig2-embedded-segments")
    globals_extended=b""
    imported=[]
    for number in range(5):
        body,glyph=dictionary([[[int(x==number%3) for x in range(3)]]],2)
        globals_extended+=_segment(number,0,0,body)
        imported+=glyph
    body,exported=dictionary([],2,imported=imported)
    globals_extended+=_segment(6,0,0,body,refs=tuple(range(5)))
    coded,rows=_text_region(exported,[(1+3*i,1,i) for i in range(5)],17,4)
    data=_segment(7,48,1,_page_info(17,4))+_segment(8,6,1,coded,refs=(6,))
    yield vector("extended-referrals-valid",data,rows,globals=globals_extended,requirement="jbig2-embedded-segments")
    rows=[[0]*7 for _ in range(3)]
    for template in range(4):
        long_rows=[[0]*16384]
        data=_segment(1,48,1,_page_info(16384,1))+_segment(2,38,1,region(long_rows,arithmetic(long_rows,template)))
        yield vector(f"arithmetic-long-tail-{template}",data,long_rows)
    page=_segment(1,48,1,_page_info(7,3))
    yield vector("unsupported-mmr-extension",page+_segment(2,38,1,region(rows,b"\x01"+bits_bytes("0000001"))),outcome="syntax",requirement="jbig2-explicit-unsupported")
    encoder=ArithmeticEncoder()
    integer(encoder,"height",None)
    body=struct.pack(">HbbII",2<<10,2,-1,1,1)+encoder.finish()
    yield vector("symbol-out-of-band-height",page+_segment(2,0,1,body),outcome="syntax",requirement="jbig2-hostile-input")
    body=struct.pack(">HbbII",2<<10,2,-1,1,0xffffffff)+b"\xff\xac"
    yield vector("symbol-count-overflow",page+_segment(2,0,1,body),outcome="limit",requirement="jbig2-hostile-input")
    # An unused identifier in a non-power-of-two alphabet is malformed.
    body,symbols=dictionary([[[1]],[[1,0]],[[0,1,0]]],2)
    globals_bad=_segment(0,0,0,body)
    encoder=ArithmeticEncoder()
    integer(encoder,"strip",0); integer(encoder,"strip",1); integer(encoder,"first",1)
    identifier(encoder,3,3)
    coded=struct.pack(">IIIIBHI",7,3,0,0,0,0x10,1)+encoder.finish()
    yield vector("text-unused-identifier",page+_segment(2,6,1,coded,refs=(0,)),outcome="syntax",globals=globals_bad,requirement="jbig2-hostile-input")
    coded,_=_text_region(symbols,[(1,1,0)],7,3)
    yield vector("text-wrong-referral-type",page+_segment(2,6,1,coded,refs=(1,)),outcome="syntax",requirement="jbig2-hostile-input")

    rows=[[0]*7 for _ in range(3)]
    payload=arithmetic(rows,2)
    bad=payload[:-2]+b"\xff\x90\xff\xac"
    yield vector("arithmetic-invalid-stuffing",page+_segment(2,38,1,region(rows,bad)),outcome="syntax",requirement="jbig2-hostile-input")


def pdf_mapping_cases():
    """PDF mapping rules derived from ISO 32000-2:2020 EC3, 7.4.7/Table 12.

    Segment grammar and coding details follow T.88 (02/2000), 7.2 and D.3.
    """
    rows=[[1,0,1],[0,1,0]]
    page=_segment(10,48,1,_page_info(3,2))
    ordinary=_segment(11,38,1,region(rows,arithmetic(rows,2)))
    # Increasing segment numbers are recommended, not required by PDF.
    yield vector("pdf-sequential-nonmonotonic",page+_segment(2,38,1,region(rows,arithmetic(rows,2))),rows,requirement="jbig2-embedded-segments")
    header=bytes([151,74,66,50,13,10,26,10,1,0,0,0,1])
    yield vector("pdf-file-header-forbidden",header+page+ordinary,outcome="syntax",requirement="jbig2-embedded-segments")
    yield vector("pdf-region-colour-extension-forbidden",page+_segment(11,38,1,region(rows,arithmetic(rows,2),operator=8)),outcome="syntax",requirement="jbig2-explicit-unsupported")
    body,symbols=dictionary([rows],2)
    globals=_segment(0,0,0,body)
    coded,expected=_text_region(symbols,[(0,0,0)],3,2)
    data=_segment(1,48,1,_page_info(3,2))+_segment(2,6,1,coded,refs=(0,))
    yield vector("pdf-globals-single-image",data,expected,globals=globals,requirement="jbig2-embedded-segments")
    yield vector("pdf-global-in-page-stream",globals+data,outcome="syntax",requirement="jbig2-embedded-segments")
    yield vector("pdf-page-in-globals",data,outcome="syntax",globals=_segment(0,0,1,body),requirement="jbig2-embedded-segments")
    yield vector("pdf-page-two-in-globals",data,outcome="syntax",globals=_segment(0,0,2,body),requirement="jbig2-embedded-segments")
    yield vector("pdf-global-missing",data,outcome="syntax",requirement="jbig2-embedded-segments")


def refinement_pixels(encoder, rows, reference, template, dx=0, dy=0, typical=False,
                      adaptive=((-1,-1),(-1,-1)), namespace="refinement"):
    """Geometric refinement neighbourhoods, in descending bit significance.

    Neighbourhoods and reference alignment follow T.88 6.3.5.3–6.3.5.6.
    Exact external bitmaps provide additional differential evidence.
    """
    height,width=len(rows),len(rows[0])
    rh,rw=len(reference),len(reference[0])
    def old(x,y):
        return reference[y][x] if 0<=x<rw and 0<=y<rh else 0
    def new(x,y):
        return rows[y][x] if 0<=x<width and 0<=y<height else 0
    output=([adaptive[0],(0,-1),(1,-1),(-1,0)] if template==0 else [(-1,-1),(0,-1),(1,-1),(-1,0)])
    prior=([adaptive[1],(0,-1),(1,-1),(-1,0),(0,0),(1,0),(-1,1),(0,1),(1,1)] if template==0
           else [(0,-1),(-1,0),(0,0),(1,0),(0,1),(1,1)])
    ltp=0
    for y,row in enumerate(rows):
        implicit=[]
        for x in range(width):
            values=[old(x-dx+ox,y-dy+oy) for oy in (-1,0,1) for ox in (-1,0,1)]
            implicit.append(values[0] if min(values)==max(values) else None)
        if typical:
            next_ltp=int(all(value is None or value==row[x] for x,value in enumerate(implicit)))
            encoder.encode((namespace,0x10 if template==0 else 8),ltp^next_ltp)
            ltp=next_ltp
        for x,value in enumerate(row):
            if ltp and implicit[x] is not None: continue
            context=0
            for ox,oy in prior: context=context*2+old(x-dx+ox,y-dy+oy)
            for ox,oy in output: context=context*2+new(x+ox,y+oy)
            encoder.encode((namespace,context),value)


def refinement_payload(rows, reference, template, dx=0, dy=0, typical=False, adaptive=((-1,-1),(-1,-1))):
    encoder=ArithmeticEncoder()
    refinement_pixels(encoder,rows,reference,template,dx,dy,typical,adaptive)
    flags=template+(2 if typical else 0)
    return bytes([flags])+(b"".join(struct.pack("bb",*p) for p in adaptive) if template==0 else b"")+encoder.finish()


def refinement_cases():
    reference=[[int(2<=x<9 and 2<=y<7 or x==y) for x in range(13)] for y in range(9)]
    target=[[int(1<=x<10 and 2<=y<6 or x+y==9) for x in range(13)] for y in range(9)]
    for template in (0,1):
        for typical in (False,True):
            page=_segment(1,48,1,_page_info(19,13))
            intermediate=_segment(2,36,1,region(reference,arithmetic(reference,2),x=3,y=2))
            payload=refinement_payload(target,reference,template,typical=typical)
            coded=_segment(3,42,1,region(target,payload,x=3,y=2),refs=(2,))
            expected=[[0]*19 for _ in range(13)]
            for y,row in enumerate(target): expected[y+2][3:16]=row
            yield vector(f"refinement-t{template}-prediction-{int(typical)}",page+intermediate+coded,expected,requirement="jbig2-refinement")
            # A refinement without a referral uses the current page as its
            # reference, including the region's nonzero page origin.
            page_reference=[[0]*19 for _ in range(13)]
            for y,row in enumerate(reference): page_reference[y+2][3:16]=row
            payload=refinement_payload(target,page_reference,template,dx=-3,dy=-2,typical=typical)
            initial=_segment(2,38,1,region(reference,arithmetic(reference,2),x=3,y=2))
            coded=_segment(3,43,1,region(target,payload,x=3,y=2,operator=4))
            yield vector(f"refinement-page-t{template}-prediction-{int(typical)}",page+initial+coded,expected,requirement="jbig2-refinement")
    adaptive=((-3,-2),(2,1))
    payload=refinement_payload(target,reference,0,adaptive=adaptive)
    page=_segment(1,48,1,_page_info(13,9))
    initial=_segment(2,36,1,region(reference,arithmetic(reference,2)))
    refined=_segment(3,40,1,region(target,payload),refs=(2,))
    final=_segment(4,42,1,region(reference,refinement_payload(reference,target,1)),refs=(3,))
    yield vector("refinement-intermediate-chain-adaptive",page+initial+refined+final,reference,requirement="jbig2-refinement")


def aggregate_dictionary(imported,definitions,template,retain=False,reuse_contexts=None):
    """Refinement/aggregate symbols; bitmap expectations precede encoding."""
    encoder=ArithmeticEncoder()
    if reuse_contexts is not None: encoder.contexts.update(reuse_contexts)
    symbols=list(imported)
    count=len(imported)+len(definitions)
    previous_height=0
    for height in sorted(set(len(d[1]) for d in definitions)):
        integer(encoder,"height",height-previous_height)
        previous_height=height
        previous_width=0
        for definition in (d for d in definitions if len(d[1])==height):
            kind,rows,*args=definition
            width=len(rows[0])
            integer(encoder,"width",width-previous_width)
            previous_width=width
            if kind=="single":
                symbol_id,dx,dy=args
                integer(encoder,"instances",1)
                identifier(encoder,count,symbol_id)
                integer(encoder,"refine-x",dx);integer(encoder,"refine-y",dy)
                refinement_pixels(encoder,rows,symbols[symbol_id],template,dx,dy)
            else:
                placements,refinements=args
                integer(encoder,"instances",len(placements))
                expected=text_pixels(encoder,symbols,placements,width,height,refinements=refinements,refine_template=template,alphabet_count=count,complete_last_strip=True)
                assert expected==rows
            symbols.append(rows)
        integer(encoder,"width",None)
    integer(encoder,"export",0);integer(encoder,"export",count)
    flags=2+(2<<10)+(template<<12)+(512 if retain else 0)+(256 if reuse_contexts is not None else 0)
    header=struct.pack(">Hbb",flags,2,-1)
    if template==0: header+=bytes([255,255,255,255])
    header+=struct.pack(">II",count,len(definitions))
    contexts={k:v for k,v in encoder.contexts.items() if isinstance(k,tuple) and k[0] in ("pixels","refinement")}
    return header+encoder.finish(),symbols,contexts


def refined_symbol_cases():
    glyphs=[[[1,0,1],[0,1,0],[1,0,1]],[[1,1,1],[1,0,0],[1,1,1]]]
    body,symbols=dictionary(glyphs,2)
    globals=_segment(0,0,0,body)
    grown=[[int(x==y or x+y==4) for x in range(5)]for y in range(5)]
    small=[[1,0],[0,1]]
    for template in (0,1):
        for dx,dy in ((0,0),(-1,1),(2,-2)):
            coded,rows=_text_region(symbols,[(1,1,0),(7,1,1),(12,1,0),(1,8,1)],19,13,
                refinements=[(grown,dx,dy),None,(small,-dx,-dy),(grown,0,0)],refine_template=template)
            data=_segment(1,48,1,_page_info(19,13))+_segment(2,6,1,coded,refs=(0,))
            yield vector(f"text-refinement-t{template}-offset-{dx+1}-{dy+2}",data,rows,globals=globals,requirement="jbig2-refinement")
        for dx,dy in ((0,0),(-2,1),(2,-1)):
            aggregate,selected,_=aggregate_dictionary(symbols,[("single",grown,0,dx,dy)],template)
            global_aggregate=globals+_segment(1,0,0,aggregate,refs=(0,))
            coded,rows=_text_region(selected,[(1,1,2),(8,1,0)],17,8)
            data=_segment(2,48,1,_page_info(17,8))+_segment(3,6,1,coded,refs=(1,))
            yield vector(f"symbol-single-refinement-t{template}-offset-{dx+2}-{dy+1}",data,rows,globals=global_aggregate,requirement="jbig2-symbol-aggregation")
        aggregate_rows=[[0]*13 for _ in range(5)]
        for gx,gy,glyph in ((0,0,symbols[0]),(6,0,grown)):
            for y,row in enumerate(glyph):
                for x,value in enumerate(row):aggregate_rows[gy+y][gx+x]|=value
        definitions=[("aggregate",aggregate_rows,[(0,0,0),(6,0,1)],[None,(grown,1,-1)])]
        aggregate,selected,_=aggregate_dictionary(symbols,definitions,template)
        global_aggregate=globals+_segment(1,0,0,aggregate,refs=(0,))
        coded,rows=_text_region(selected,[(1,1,2)],17,8)
        data=_segment(2,48,1,_page_info(17,8))+_segment(3,6,1,coded,refs=(1,))
        yield vector(f"symbol-multiple-aggregation-t{template}",data,rows,globals=global_aggregate,requirement="jbig2-symbol-aggregation")
    # Carry bitmap probabilities across dictionary boundaries, while integer
    # coding contexts start afresh. Dense shapes distinguish reuse from reset.
    glyphs_a=[[[((x*3+y*7+x*y)^y)>>2&1 for x in range(11)] for y in range(7)]]
    glyphs_b=[[[int(x%3==y%3) for x in range(11)]for y in range(7)]]
    for template in range(4):
        first,initial,contexts=dictionary(glyphs_a,template,retain=True,return_contexts=True)
        second,selected=dictionary(glyphs_b,template,imported=initial,reuse_contexts=contexts)
        global_retained=_segment(0,0,0,first)+_segment(1,0,0,second,refs=(0,))
        coded,rows=_text_region(selected,[(1,1,0),(14,1,1)],27,10)
        data=_segment(2,48,1,_page_info(27,10))+_segment(3,6,1,coded,refs=(1,))
        yield vector(f"retained-generic-contexts-t{template}",data,rows,globals=global_retained,requirement="jbig2-retained-contexts")
    for template in (0,1):
        first,initial,contexts=aggregate_dictionary(symbols,[("single",grown,0,1,-1)],template,retain=True)
        second_target=[[int((x+y)%3==0)for x in range(5)]for y in range(5)]
        second,selected,_=aggregate_dictionary(initial,[("single",second_target,2,-1,1)],template,reuse_contexts=contexts)
        global_retained=globals+_segment(1,0,0,first,refs=(0,))+_segment(2,0,0,second,refs=(1,))
        coded,rows=_text_region(selected,[(1,1,2),(8,1,3)],17,8)
        data=_segment(3,48,1,_page_info(17,8))+_segment(4,6,1,coded,refs=(2,))
        yield vector(f"retained-refinement-contexts-t{template}",data,rows,globals=global_retained,requirement="jbig2-retained-contexts")


def refinement_hostile_cases():
    rows=[[1,0,1],[0,1,0],[1,0,1]]
    page=_segment(1,48,1,_page_info(9,7))
    prior=_segment(2,36,1,region(rows,arithmetic(rows,2)))
    cases={
        "refinement-adaptive-noncausal":(region(rows,b"\x00\x00\x00\xff\xff\xff\xac"),(2,)),
        "refinement-reserved-flags":(region(rows,b"\x04"),(2,)),
        "refinement-two-references":(region(rows,refinement_payload(rows,rows,1)),(2,2)),
        "refinement-wrong-reference":(region(rows,refinement_payload(rows,rows,1)),(1,)),
        "refinement-arithmetic-truncated":(region(rows,refinement_payload(rows,rows,1)[:-1]),(2,)),
    }
    for name,(body,refs) in cases.items():
        yield vector(name,page+prior+_segment(3,42,1,body,refs=refs),outcome="syntax",requirement="jbig2-hostile-input")
    yield vector("reserved-generic-type-32",page+_segment(2,32,1,b""),outcome="syntax",requirement="jbig2-explicit-unsupported")
    body,symbols=dictionary([rows],2)
    globals=_segment(0,0,0,body)
    for name,value,outcome in (("text-refinement-zero-width",-3,"syntax"),("text-refinement-pixel-limit",30_000_000,"limit")):
        encoder=ArithmeticEncoder()
        integer(encoder,"strip",0);integer(encoder,"strip",1);integer(encoder,"first",1)
        identifier(encoder,1,0);integer(encoder,"refine-indicator",1)
        for family,n in (("refine-width",value),("refine-height",0),("refine-x",0),("refine-y",0)):integer(encoder,family,n)
        coded=struct.pack(">IIIIBHI",9,7,0,0,0,0x8012,1)+encoder.finish()
        yield vector(name,page+_segment(2,6,1,coded,refs=(0,)),outcome=outcome,globals=globals,requirement="jbig2-hostile-input")
    encoder=ArithmeticEncoder()
    integer(encoder,"strip",0);integer(encoder,"strip",1);integer(encoder,"first",1)
    identifier(encoder,1,0);integer(encoder,"refine-indicator",2)
    coded=struct.pack(">IIIIBHI",9,7,0,0,0,0x8012,1)+encoder.finish()
    yield vector("text-refinement-invalid-indicator",page+_segment(2,6,1,coded,refs=(0,)),outcome="syntax",globals=globals,requirement="jbig2-hostile-input")
    second,_=dictionary([rows],2,imported=symbols,reuse_contexts={})
    yield vector("retained-contexts-missing",page,outcome="syntax",globals=globals+_segment(1,0,0,second,refs=(0,)),requirement="jbig2-hostile-input")
    first,initial,contexts=dictionary([rows],0,retain=True,return_contexts=True)
    second,_=dictionary([rows],1,imported=initial,reuse_contexts=contexts)
    yield vector("retained-contexts-incompatible",page,outcome="syntax",globals=_segment(0,0,0,first)+_segment(1,0,0,second,refs=(0,)),requirement="jbig2-hostile-input")
    first,initial,contexts=dictionary([rows],2,retain=True,return_contexts=True)
    second,_,_=aggregate_dictionary(initial,[("single",rows,0,0,0)],1,reuse_contexts=contexts)
    yield vector("retained-refinement-contexts-missing",page,outcome="syntax",globals=_segment(0,0,0,first)+_segment(1,0,0,second,refs=(0,)),requirement="jbig2-hostile-input")
    for template in (0,1):
        small=[[1,0],[0,1]]
        aggregate=[[0]*8 for _ in range(5)]
        for ox,oy,glyph in ((0,0,rows),(4,1,small)):
            for y,row in enumerate(glyph):
                for x,value in enumerate(row):aggregate[oy+y][ox+x]|=value
        grown=[[int(x+y==4 or x==y) for x in range(5)]for y in range(5)]
        # One direct refinement, a two-instance aggregate containing it,
        # and a refinement of that aggregate share coding state.
        definitions=[("single",small,0,1,-1),("aggregate",aggregate,[(0,0,0),(4,1,1)],[None,None]),("single",grown,2,-1,1)]
        body,selected,_=aggregate_dictionary(symbols,definitions,template)
        aggregate_globals=globals+_segment(1,0,0,body,refs=(0,))
        coded,expected=_text_region(selected,[(1,1,1),(5,1,2),(1,8,3)],17,15)
        data=_segment(2,48,1,_page_info(17,15))+_segment(3,6,1,coded,refs=(1,))
        yield vector(f"symbol-mixed-aggregation-state-t{template}",data,expected,globals=aggregate_globals,requirement="jbig2-symbol-aggregation")

    for template in (0,1):
        for bit in (0,1):
            reference=[[bit]*17 for _ in range(5)]
            target=[[bit]*17 for _ in range(5)]
            page=_segment(1,48,1,_page_info(17,5))
            prior=_segment(2,36,1,region(reference,arithmetic(reference,2)))
            refined=_segment(3,42,1,region(target,refinement_payload(target,reference,template,typical=True)),refs=(2,))
            yield vector(f"refinement-prediction-uniform-{bit}-t{template}",page+prior+refined,target,requirement="jbig2-refinement")

# Published prefix words and range parameters, T.88 (02/2000), B.5.
HUFFMAN_ROWS = [
    [('0', 4, 0, 0), ('10', 8, 16, 0), ('110', 16, 272, 0), ('111', 32, 65808, 0)],
    [('0', 0, 0, 0), ('10', 0, 1, 0), ('110', 0, 2, 0), ('1110', 3, 3, 0), ('11110', 6, 11, 0), ('111110', 32, 75, 0), ('111111', 0, 0, 2)],
    [('11111110', 8, -256, 0), ('0', 0, 0, 0), ('10', 0, 1, 0), ('110', 0, 2, 0), ('1110', 3, 3, 0), ('11110', 6, 11, 0), ('11111111', 32, -257, 1), ('1111110', 32, 75, 0), ('111110', 0, 0, 2)],
    [('0', 0, 1, 0), ('10', 0, 2, 0), ('110', 0, 3, 0), ('1110', 3, 4, 0), ('11110', 6, 12, 0), ('11111', 32, 76, 0)],
    [('1111110', 8, -255, 0), ('0', 0, 1, 0), ('10', 0, 2, 0), ('110', 0, 3, 0), ('1110', 3, 4, 0), ('11110', 6, 12, 0), ('1111111', 32, -256, 1), ('111110', 32, 76, 0)],
    [('11100', 10, -2048, 0), ('1000', 9, -1024, 0), ('1001', 8, -512, 0), ('1010', 7, -256, 0), ('11101', 6, -128, 0), ('11110', 5, -64, 0), ('1011', 5, -32, 0), ('00', 7, 0, 0), ('010', 7, 128, 0), ('011', 8, 256, 0), ('1100', 9, 512, 0), ('1101', 10, 1024, 0), ('111110', 32, -2049, 1), ('111111', 32, 2048, 0)],
    [('1000', 9, -1024, 0), ('000', 8, -512, 0), ('1001', 7, -256, 0), ('11010', 6, -128, 0), ('11011', 5, -64, 0), ('1010', 5, -32, 0), ('1011', 5, 0, 0), ('11100', 5, 32, 0), ('11101', 6, 64, 0), ('1100', 7, 128, 0), ('001', 8, 256, 0), ('010', 9, 512, 0), ('011', 10, 1024, 0), ('11110', 32, -1025, 1), ('11111', 32, 2048, 0)],
    [('11111100', 3, -15, 0), ('111111100', 1, -7, 0), ('11111101', 1, -5, 0), ('111111101', 0, -3, 0), ('1111100', 0, -2, 0), ('1010', 0, -1, 0), ('00', 1, 0, 0), ('11010', 0, 2, 0), ('111010', 0, 3, 0), ('100', 4, 4, 0), ('111011', 1, 20, 0), ('1011', 4, 22, 0), ('1100', 5, 38, 0), ('11011', 6, 70, 0), ('11100', 7, 134, 0), ('111100', 7, 262, 0), ('1111101', 8, 390, 0), ('111101', 10, 646, 0), ('111111110', 32, -16, 1), ('111111111', 32, 1670, 0), ('01', 0, 0, 2)],
    [('11111100', 4, -31, 0), ('111111100', 2, -15, 0), ('11111101', 2, -11, 0), ('111111101', 1, -7, 0), ('1111100', 1, -5, 0), ('1010', 1, -3, 0), ('010', 1, -1, 0), ('011', 1, 1, 0), ('11010', 1, 3, 0), ('111010', 1, 5, 0), ('100', 5, 7, 0), ('111011', 2, 39, 0), ('1011', 5, 43, 0), ('1100', 6, 75, 0), ('11011', 7, 139, 0), ('11100', 8, 267, 0), ('111100', 8, 523, 0), ('1111101', 9, 779, 0), ('111101', 11, 1291, 0), ('111111110', 32, -32, 1), ('111111111', 32, 3339, 0), ('00', 0, 0, 2)],
    [('1111010', 4, -21, 0), ('11111100', 0, -5, 0), ('1111011', 0, -4, 0), ('11000', 0, -3, 0), ('00', 2, -2, 0), ('11001', 0, 2, 0), ('110110', 0, 3, 0), ('1111100', 0, 4, 0), ('11111101', 0, 5, 0), ('01', 6, 6, 0), ('11010', 5, 70, 0), ('110111', 5, 102, 0), ('111000', 6, 134, 0), ('111001', 7, 198, 0), ('111010', 8, 326, 0), ('111011', 9, 582, 0), ('111100', 10, 1094, 0), ('1111101', 11, 2118, 0), ('11111110', 32, -22, 1), ('11111111', 32, 4166, 0), ('10', 0, 0, 2)],
    [('0', 0, 1, 0), ('10', 1, 2, 0), ('1100', 0, 4, 0), ('1101', 1, 5, 0), ('11100', 1, 7, 0), ('11101', 2, 9, 0), ('111100', 2, 13, 0), ('1111010', 2, 17, 0), ('1111011', 3, 21, 0), ('1111100', 4, 29, 0), ('1111101', 5, 45, 0), ('1111110', 6, 77, 0), ('1111111', 32, 141, 0)],
    [('0', 0, 1, 0), ('10', 0, 2, 0), ('110', 1, 3, 0), ('11100', 0, 5, 0), ('11101', 1, 6, 0), ('111100', 1, 8, 0), ('1111010', 0, 10, 0), ('1111011', 1, 11, 0), ('1111100', 2, 13, 0), ('1111101', 3, 17, 0), ('1111110', 4, 25, 0), ('11111110', 5, 41, 0), ('11111111', 32, 73, 0)],
    [('0', 0, 1, 0), ('100', 0, 2, 0), ('1100', 0, 3, 0), ('11100', 0, 4, 0), ('1101', 1, 5, 0), ('101', 3, 7, 0), ('111010', 1, 15, 0), ('111011', 2, 17, 0), ('111100', 3, 21, 0), ('111101', 4, 29, 0), ('111110', 5, 45, 0), ('1111110', 6, 77, 0), ('1111111', 32, 141, 0)],
    [('100', 0, -2, 0), ('101', 0, -1, 0), ('0', 0, 0, 0), ('110', 0, 1, 0), ('111', 0, 2, 0)],
    [('1111100', 4, -24, 0), ('111100', 2, -8, 0), ('11100', 1, -4, 0), ('1100', 0, -2, 0), ('100', 0, -1, 0), ('0', 0, 0, 0), ('101', 0, 1, 0), ('1101', 0, 2, 0), ('11101', 1, 3, 0), ('111101', 2, 5, 0), ('1111101', 4, 9, 0), ('1111110', 32, -25, 1), ('1111111', 32, 25, 0)],
]


def huffman_word(table, value):
    for prefix, size, base, kind in HUFFMAN_ROWS[table-1]:
        if kind == 2:
            if value is None: return prefix
            continue
        if value is None: continue
        offset = base-value if kind == 1 else value-base
        if 0 <= offset < (1 << size): return prefix + (format(offset, f"0{size}b") if size else "")
    raise ValueError((table,value))


def huffman_dictionary(glyphs, compressed: bool | str = False, dh=4, dw=2, custom=False):
    symbols=sorted(glyphs,key=lambda r:(len(r),len(r[0])))
    output=b""; pending=""; old_height=0
    for height in sorted(set(map(len,symbols))):
        pending+=huffman_word(dh,height-old_height);old_height=height
        width=0; group=[s for s in symbols if len(s)==height]
        for glyph in group:
            delta=len(glyph[0])-width
            pending+=format(delta,"05b") if custom else huffman_word(dw,delta)
            width=len(glyph[0])
        pending+="01" if custom else huffman_word(dw,None)
        collective=[sum((g[y] for g in group),[]) for y in range(height)]
        payload=mmr(collective,eofb=compressed=="eofb")[1:] if compressed else packed(collective)
        pending+=huffman_word(1,len(payload) if compressed else 0)
        output+=bits_bytes(pending)+payload;pending=""
    output+=bits_bytes(huffman_word(1,0)+huffman_word(1,len(symbols)))
    flags=1+((dh-4)<<2)+((3 if custom else dw-2)<<4)
    return struct.pack(">HII",flags,len(symbols),len(symbols))+output,symbols


def huffman_text(symbols,width=19,height=12,fs=6,ds=8,dt=11,corner=1,transposed=False,strips=1):
    id_size=max(1,(len(symbols)-1).bit_length())
    # The length alphabet has one code: every identifier uses id_size bits.
    lengths="".join(format(int(n==id_size),"04b") for n in range(35))
    ids=bits_bytes(lengths+"0"*len(symbols))
    bits=huffman_word(dt,1)+huffman_word(dt,1)+huffman_word(fs,1)
    rows=[[0]*width for _ in range(height)]
    s=1
    for index,glyph in enumerate(symbols):
        if index: bits+=huffman_word(ds,3);s+=3
        within=1 if strips>1 else 0
        if strips>1:bits+=format(within,f"0{strips.bit_length()-1}b")
        bits+=format(index,f"0{id_size}b")
        # S advances on the coding axis; T anchors the selected corner.
        x,y=reference_origin(s,within,glyph,transposed,corner)
        for gy,row in enumerate(glyph):
            for gx,value in enumerate(row):
                if 0<=x+gx<width and 0<=y+gy<height:rows[y+gy][x+gx]|=value
        s+=(len(glyph) if transposed else len(glyph[0]))-1
    bits+=huffman_word(ds,None)
    flags=1+(corner<<4)+(64 if transposed else 0)+((strips.bit_length()-1)<<2)
    selections=(3 if fs==1 else fs-6)+((ds-8)<<2)+((dt-11)<<4)
    return struct.pack(">IIIIBHHI",width,height,0,0,0,flags,selections,len(symbols))+ids+bits_bytes(bits),rows


def table_cases():
    for number,table in enumerate(HUFFMAN_ROWS,1):
        lines=[]
        for index,(prefix,size,base,kind) in enumerate(table):
            offsets=(0,) if kind==2 or size==0 else (0,min((1<<size)-1,base+0x80000000 if kind==1 else 0x7fffffff-base))
            for offset in offsets:
                word=prefix+(format(offset,f"0{size}b") if size and kind!=2 else "")
                value="OOB" if kind==2 else str(base-offset if kind==1 else base+offset)
                lines.append(f"{index} {word} {value}")
        yield Vector(f"huffman-table-{number:02}",{'.codes':("\n".join(lines)+"\n").encode()},("jbig2-huffman-tables",),"Published prefix words and range endpoints from T.88 Annex B")
    glyphs=[[[1,0],[0,1],[1,1]],[[1,1,0],[1,0,1],[0,1,1]],[[0,1,0,1],[1,0,1,0],[1,1,0,0]]]
    for compressed in (False,True,"eofb"):
        for dh in (4,5):
            for dw in (2,3):
                body,symbols=huffman_dictionary(glyphs,compressed,dh,dw)
                globals=_segment(0,0,0,body)
                coded,rows=_text_region(symbols,[(1,1,0),(5,1,1),(10,1,2)],19,7)
                data=_segment(1,48,1,_page_info(19,7))+_segment(2,6,1,coded,refs=(0,))
                yield vector(f"huffman-symbols-{'mmr-eofb' if compressed=='eofb' else 'mmr' if compressed else 'raw'}-{dh}-{dw}",data,rows,globals=globals,requirement="jbig2-huffman-symbols")
    body,symbols=huffman_dictionary(glyphs)
    globals=_segment(0,0,0,body)
    for fs in (6,7):
        for ds in (8,9,10):
            for dt in (11,12,13):
                coded,rows=huffman_text(symbols,fs=fs,ds=ds,dt=dt)
                data=_segment(1,48,1,_page_info(19,12))+_segment(2,6,1,coded,refs=(0,))
                yield vector(f"huffman-text-{fs}-{ds}-{dt}",data,rows,globals=globals,requirement="jbig2-huffman-text")
    for transposed in (False,True):
        for corner in range(4):
            coded,rows=huffman_text(symbols,corner=corner,transposed=transposed,strips=4)
            data=_segment(1,48,1,_page_info(19,12))+_segment(2,7,1,coded,refs=(0,))
            yield vector(f"huffman-layout-{int(transposed)}-{corner}",data,rows,globals=globals,requirement="jbig2-huffman-text")
    # B.2 custom width table: values 0..7 have prefix 00, OOB is 01;
    # the unused lower and upper escape rows both have zero prefix length.
    table=bytes([0x23])+struct.pack(">ii",0,8)+bits_bytes("10"+"011"+"00"+"00"+"10")
    body,symbols=huffman_dictionary(glyphs,custom=True)
    globals=_segment(0,53,0,table)+_segment(1,0,0,body,refs=(0,))
    coded,rows=huffman_text(symbols)
    data=_segment(2,48,1,_page_info(19,12))+_segment(3,6,1,coded,refs=(1,))
    yield vector("huffman-user-width-table",data,rows,globals=globals,requirement="jbig2-huffman-tables")


def generic_plane(encoder,rows,template,first=None,skip=None):
    positions=list(NEIGHBOURS[template])
    if first is not None:positions[NEIGHBOURS[template].index(DEFAULT_AT[template][0])]=(first,0)
    for y,row in enumerate(rows):
        for x,value in enumerate(row):
            if skip and skip[y][x]:continue
            context=0
            for dx,dy in positions:
                px,py=x+dx,y+dy
                context=context*2+(rows[py][px] if 0<=py<len(rows) and 0<=px<len(row) else 0)
            encoder.encode(context,value)


def halftone_cases():
    patterns=[[[0,0,0],[0,0,0]],[[1,0,0],[0,1,0]],[[1,0,1],[0,1,0]],[[1,1,1],[1,1,1]]]
    collective=[sum((p[y] for p in patterns),[]) for y in range(2)]
    indices=[[((n*3+m*5)^m)%4 for n in range(7)] for m in range(5)]
    for mmr_coding in (False,True):
        for template in (range(4) if not mmr_coding else (0,)):
            encoder=ArithmeticEncoder();generic_plane(encoder,collective,template,-3)
            payload=mmr(collective)[1:] if mmr_coding else encoder.finish()
            dictionary_body=struct.pack(">BBBI",1 if mmr_coding else template<<1,3,2,3)+payload
            globals=_segment(0,16,0,dictionary_body)
            for skip_enabled in ((False,True) if not mmr_coding else (False,)):
                for rotation in (0,128):
                    width,height=23,17
                    origin=(-256,-128);rx,ry=3*256,rotation
                    def position(n,m):return ((origin[0]+m*ry+n*rx)//256,(origin[1]+m*rx-n*ry)//256)
                    skip=[]
                    for m,row in enumerate(indices):
                        skip.append([])
                        for n in range(len(row)):
                            x,y=position(n,m);skip[-1].append(x+3<=0 or x>=width or y+2<=0 or y>=height)
                    encoder=ArithmeticEncoder();data=b""
                    for bit in (1,0):
                        plane=[[(value>>bit ^ value>>(bit+1))&1 for value in row] for row in indices]
                        if skip_enabled:
                            plane=[[0 if skip[m][n] else v for n,v in enumerate(row)] for m,row in enumerate(plane)]
                        if mmr_coding:data+=mmr(plane)[1:]
                        else:generic_plane(encoder,plane,template,skip=skip if skip_enabled else None)
                    if not mmr_coding:data=encoder.finish()
                    for operator in range(5):
                        rows=[[1]*width for _ in range(height)]
                        for m,row in enumerate(indices):
                            for n,value in enumerate(row):
                                if skip_enabled and skip[m][n]:value=0
                                x,y=position(n,m)
                                for py,prow in enumerate(patterns[value]):
                                    for px,b in enumerate(prow):
                                        if 0<=x+px<width and 0<=y+py<height:
                                            a=rows[y+py][x+px];rows[y+py][x+px]=(a|b,a&b,a^b,1^a^b,b)[operator]
                        flags=(1 if mmr_coding else template<<1)+(8 if skip_enabled else 0)+(operator<<4)+128
                        header=struct.pack(">IIIIBBIIiiHH",width,height,0,0,0,flags,7,5,*origin,rx,ry)
                        coded=_segment(1,48,1,_page_info(width,height))+_segment(2,23 if rotation else 22,1,header+data,refs=(0,))
                        yield vector(f"halftone-{'mmr' if mmr_coding else 'arithmetic'}-t{template}-skip{int(skip_enabled)}-r{rotation}-op{operator}",coded,rows,globals=globals,requirement="jbig2-halftone")


def specification_cases():
    rows=[[1,0,1,0,1,0,1],[0,1,0,1,0,1,0]]
    page=_segment(1,48,1,_page_info(7,2))
    # Omitted EOFB and nonzero ignored byte-alignment bits, 6.2.6.
    bits=""
    for row in rows:
        x=0
        while x<len(row):
            runs=[]
            for color in (0,1):
                start=x
                while x<len(row) and row[x]==color:x+=1
                runs.append(x-start)
            bits+="001"+run_code(runs[0],False)+run_code(runs[1],True)
    for padding in ("0","1"):
        payload=bits_bytes(bits+padding*((-len(bits))%8))
        yield vector(f"mmr-no-eofb-padding-{padding}",page+_segment(2,38,1,region(rows,b"\x01"+payload)),rows,requirement="jbig2-generic-mmr")
    yield vector("header-number-maximum",_segment(0xffffffff,48,1,_page_info(7,2)),[[0]*7 for _ in range(2)],requirement="jbig2-embedded-segments")
    prior=bytearray(_segment(2,36,1,region(rows,arithmetic(rows,2))))
    prior[5]=0
    coded=_segment(3,42,1,region(rows,refinement_payload(rows,rows,1)),refs=(2,))
    yield vector("retention-reference-after-release",page+prior+coded,outcome="syntax",requirement="jbig2-embedded-segments")
    prior=_segment(2,36,1,region(rows,arithmetic(rows,2)))
    changed=_segment(3,42,1,region(rows,refinement_payload(rows,rows,1),x=1),refs=(2,))
    yield vector("refinement-reference-location-mismatch",page+prior+changed,outcome="syntax",requirement="jbig2-refinement")
    yield vector("refinement-intermediate-missing-reference",page+_segment(2,40,1,region(rows,refinement_payload(rows,rows,1))),outcome="syntax",requirement="jbig2-refinement")


def huffman_refinement_cases():
    base=[[1,0,1],[0,1,0],[1,1,1]]
    target=[[int(x==y or x+y==3) for x in range(4)] for y in range(4)]
    body,symbols=huffman_dictionary([base]);original=_segment(0,0,0,body)
    for template in (0,1):
        for delta_table in (14,15):
            encoder=ArithmeticEncoder()
            refinement_pixels(encoder,target,base,template)
            payload=encoder.finish()
            ids=bits_bytes(''.join(format(int(n==1),'04b') for n in range(35))+'0')
            bits=huffman_word(11,1)+huffman_word(11,2)+huffman_word(6,1)+'0'+'1'
            for value in (1,1,0,0):bits+=huffman_word(delta_table,value)
            bits+=huffman_word(1,len(payload))
            coded=bits_bytes(bits)+payload+bits_bytes(huffman_word(8,None))
            flags=1+2+0x10+(template<<15)
            selections=0 if delta_table==14 else sum(1<<shift for shift in (6,8,10,12))
            header=struct.pack('>IIIIBHH',9,7,0,0,0,flags,selections)
            if template==0:header+=bytes([255]*4)
            header+=struct.pack('>I',1)
            expected=[[0]*9 for _ in range(7)]
            for y,row in enumerate(target):expected[y+1][1:5]=row
            data=_segment(1,48,1,_page_info(9,7))+_segment(2,6,1,header+ids+coded,refs=(0,))
            yield vector(f'huffman-text-refinement-t{template}-table{delta_table}',data,expected,globals=original,requirement='jbig2-huffman-text')
        # A single refined symbol followed by a two-instance aggregate.
        encoder=ArithmeticEncoder();refinement_pixels(encoder,target,base,template)
        payload=encoder.finish()
        bits=huffman_word(4,4)+huffman_word(2,4)+huffman_word(1,1)+'00'+huffman_word(15,0)*2+huffman_word(1,len(payload))
        coded=bits_bytes(bits)+payload
        coded+=bits_bytes(huffman_word(2,None)+huffman_word(4,1)+huffman_word(2,8)+huffman_word(1,2)+huffman_word(11,1)+huffman_word(11,1)+huffman_word(6,0)+'00'+'0'+huffman_word(8,2)+'01'+'0'+huffman_word(8,None)+huffman_word(2,None)+huffman_word(1,0)+huffman_word(1,3))
        flags=1+2+512+(template<<12)
        header=struct.pack('>H',flags)+(bytes([255]*4) if template==0 else b'')+struct.pack('>II',3,2)
        globals=original+_segment(1,0,0,header+coded,refs=(0,))
        retained_globals=globals
        aggregate=[[0]*8 for _ in range(5)]
        for x0,glyph in ((0,base),(4,target)):
            for y,row in enumerate(glyph):
                for x,value in enumerate(row):aggregate[y][x+x0]|=value
        selected=[base,target,aggregate]
        text,rows=_text_region(selected,[(0,0,0),(4,0,1),(0,5,2)],13,11)
        data=_segment(2,48,1,_page_info(13,11))+_segment(3,6,1,text,refs=(1,))
        yield vector(f'huffman-symbol-aggregation-t{template}',data,rows,globals=globals,requirement='jbig2-huffman-symbols')
        # Restore the retained GR states in a new Huffman dictionary.
        next_target=[[int(x==2 or y==1) for x in range(4)] for y in range(4)]
        following=ArithmeticEncoder();following.contexts.update(encoder.contexts)
        refinement_pixels(following,next_target,target,template)
        payload=following.finish()
        bits=huffman_word(4,4)+huffman_word(2,4)+huffman_word(1,1)+'01'+huffman_word(15,0)*2+huffman_word(1,len(payload))
        coded=bits_bytes(bits)+payload+bits_bytes(huffman_word(2,None)+huffman_word(1,0)+huffman_word(1,4))
        header=struct.pack('>H',1+2+256+(template<<12))+(bytes([255]*4) if template==0 else b'')+struct.pack('>II',4,1)
        globals+=_segment(2,0,0,header+coded,refs=(1,))
        text,rows=_text_region(selected+[next_target],[(0,0,3),(5,1,1)],13,7)
        data=_segment(3,48,1,_page_info(13,7))+_segment(4,6,1,text,refs=(2,))
        yield vector(f'huffman-retained-refinement-t{template}',data,rows,globals=globals,requirement='jbig2-retained-contexts')
        # A table after the dictionary referral must not change which saved
        # bitmap statistics are selected (7.4.2.2 step 3).
        table=bytes.fromhex('42 00 00 00 00 00 01 01 10 49 23 81 80')
        user_header=struct.pack('>H',struct.unpack('>H',header[:2])[0]|64)+header[2:]
        mixed_globals=retained_globals+_segment(2,53,0,table)+_segment(3,0,0,user_header+coded,refs=(1,2))
        data=_segment(4,48,1,_page_info(13,7))+_segment(5,6,1,text,refs=(3,))
        yield vector(f'huffman-retained-table-last-t{template}',data,rows,globals=mixed_globals,requirement='jbig2-retained-contexts')


def unknown_length_cases():
    rows=[[1,0,1],[0,1,0],[1,1,1]]
    page=_segment(1,48,1,_page_info(3,5))
    for coding in ('mmr','arithmetic'):
        payload=mmr(rows) if coding=='mmr' else arithmetic(rows,2)
        body=struct.pack('>IIIIB',3,5,0,0,0)+payload+(b'\x00\x00' if coding=='mmr' else b'')+struct.pack('>I',3)
        data=_segment(2,38,1,body)
        data=data[:7]+b'\xff'*4+data[11:]
        yield vector(f'unknown-length-{coding}',page+data,rows+[[0]*3 for _ in range(2)],requirement='jbig2-embedded-segments')


def arithmetic_known_answer():
    decisions=bytes.fromhex('00 02 00 51 00 00 00 C0 03 52 87 2A AA AA AA AA 82 C0 20 00 FC D7 9E F6 BF 7F ED 90 4F 46 A3 BF')
    published=bytes.fromhex('84 C7 3B FC E1 A1 43 04 02 20 00 00 41 0D BB 86 F4 31 7F FF 88 FF 37 47 1A DB 6A DF FF AC')
    encoder=ArithmeticEncoder()
    for byte in decisions:
        for bit in range(7,-1,-1):encoder.encode(0,(byte>>bit)&1)
    assert encoder.finish()==published
    yield Vector('arithmetic-h2-known-answer',{'.mq':published,'.decisions':decisions},('jbig2-arithmetic-coder',),'T.88 H.2 arithmetic known-answer bytes with a single adaptive context')


def primary_rejection_cases():
    rows=[[1,0,1],[0,1,0]]
    page=_segment(1,48,1,_page_info(3,4))
    prior=_segment(2,36,1,region(rows,arithmetic(rows,2)))
    refinement=region(rows,refinement_payload(rows,rows,1))
    for name,data in (
        ('retention-unused-short-bit',page[:5]+bytes([3])+page[6:]),
        ('refinement-page-without-replace',page+_segment(2,42,1,refinement)),
        ('refinement-reference-size-mismatch',page+prior+_segment(3,42,1,region([[1]],refinement_payload([[1]],rows,1)),refs=(2,))),
        ('refinement-reference-operator-mismatch',page+prior+_segment(3,42,1,region(rows,refinement_payload(rows,rows,1),operator=4),refs=(2,))),
        ('refinement-reference-used-twice',page+prior+_segment(3,42,1,refinement,refs=(2,))+_segment(4,42,1,refinement,refs=(2,))),
        ('page-disallows-refinement',_segment(1,48,1,_page_info(3,4,64))+_segment(2,42,1,region(rows,refinement_payload(rows,rows,1),operator=4))),
        ('page-fixed-operator-intermediate-mismatch',_segment(1,48,1,_page_info(3,4,34))+_segment(2,36,1,region(rows,arithmetic(rows,2),operator=1))),
        ('page-disallows-auxiliary',_segment(1,48,1,_page_info(3,4,64))+prior),
        ('stripe-drawing-below-end',_segment(1,48,1,_page_info(3,4,98,0x8004))+_segment(2,38,1,region(rows,arithmetic(rows,2),y=2))+_segment(3,50,1,struct.pack('>I',2))),
        ('stripe-drawing-above-previous-end',_segment(1,48,1,_page_info(3,4,98,0x8004))+_segment(2,50,1,struct.pack('>I',1))+_segment(3,38,1,region(rows,arithmetic(rows,2)))),
        ('huffman-table-reserved-flags',page+_segment(2,53,1,b'\x80')),
        ('huffman-table-invalid-bounds',page+_segment(2,53,1,b'\x00'+struct.pack('>ii',1,1))),
        ('huffman-table-prefix-overflow',page+_segment(2,53,1,b'\x0e'+struct.pack('>ii',0,1)+bytes([33,0,0,0]))),
        ('halftone-referral-missing',page+_segment(2,22,1,region(rows,b''))),
    ):
        yield vector(name,data,outcome='syntax',requirement='jbig2-hostile-input')
    # Reusing a context with the same number of states but different AT pixels
    # violates 7.4.2.2, even though both adaptive pixels are causal.
    body,symbols,contexts=dictionary([rows],2,retain=True,return_contexts=True)
    body2,selected=dictionary([rows],2,imported=symbols,reuse_contexts=contexts)
    body2=body2[:2]+bytes([1,255])+body2[4:]
    globals=_segment(0,0,0,body)+_segment(1,0,0,body2,refs=(0,))
    yield vector('retained-context-adaptive-mismatch',_segment(2,48,1,_page_info(3,4)),outcome='syntax',globals=globals,requirement='jbig2-retained-contexts')


def metadata_cases():
    rows=[[1,0,1],[0,1,0]]
    profiles=_segment(0,52,0,struct.pack('>II',1,1))
    data=_segment(1,48,1,_page_info(3,2))+_segment(2,52,1,struct.pack('>II',1,1))+_segment(3,62,1,struct.pack('>I',0x20000000)+b'title\x00sample\x00\x00')+_segment(4,38,1,region(rows,arithmetic(rows,2)))
    yield vector('profiles-and-optional-comment',data,rows,globals=profiles,requirement='jbig2-profiles')
    yield vector('profiles-first-must-be-global',_segment(1,48,1,_page_info(3,2))+_segment(2,52,1,struct.pack('>II',1,1)),outcome='syntax',requirement='jbig2-profiles')
    yield vector('extension-invalid-necessary-bit',_segment(1,48,1,_page_info(3,2))+_segment(2,62,1,struct.pack('>I',0x80000000)),outcome='syntax',requirement='jbig2-hostile-input')


def arithmetic_layout_cases():
    body,symbols=dictionary([[[1,0],[0,1],[1,1]],[[1,1,0],[0,1,1]]],2)
    globals=_segment(0,0,0,body)
    for transposed in (False,True):
        for corner in range(4):
            coded,rows=_text_region(symbols,[(1,2,0),(5,2,1),(1,6,1),(6,6,0)],13,12,strips=4,transposed=transposed,corner=corner)
            data=_segment(1,48,1,_page_info(13,12))+_segment(2,6,1,coded,refs=(0,))
            yield vector(f'arithmetic-layout-{int(transposed)}-{corner}',data,rows,globals=globals,requirement='jbig2-arithmetic-text')


def halftone_edge_cases():
    # A one-pattern dictionary needs zero gray planes and consumes no gray data.
    pattern=[[1,0],[0,1]]
    globals=_segment(0,16,0,struct.pack('>BBBI',1,2,2,0)+mmr(pattern)[1:])
    rows=[[int((x+y)%2==0) for x in range(6)] for y in range(6)]
    header=struct.pack('>IIIIBBIIiiHH',6,6,0,0,0,0x40,3,3,0,0,512,0)
    for kind in (22,23):
        data=_segment(1,48,1,_page_info(6,6))+_segment(2,kind,1,header,refs=(0,))
        yield vector(f'halftone-one-pattern-type{kind}',data,rows,globals=globals,requirement='jbig2-halftone')
    data=_segment(1,48,1,_page_info(6,6))+_segment(2,20,1,header,refs=(0,))+_segment(3,42,1,region(rows,refinement_payload(rows,rows,1)),refs=(2,))
    yield vector('halftone-intermediate-refinement',data,rows,globals=globals,requirement='jbig2-halftone')


def header_and_table_edge_cases():
    rows=[[1,0,1]]
    page=_segment(1,48,1,_page_info(3,0xffffffff,98,0x8000))
    data=page+_segment(2,38,1,region(rows,arithmetic(rows,2)))+_segment(3,50,1,struct.pack('>I',0))
    yield vector('stripe-zero-maximum-first-row',data,rows,requirement='jbig2-page-combination')
    glyph=[[1,0],[0,1]]
    table=bytes.fromhex('42 00 00 00 00 00 01 01 10 49 23 81 80')
    body,symbols=huffman_dictionary([glyph])
    globals=_segment(0,53,0,table)+_segment(1,0,0,body)
    coded,expected=huffman_text(symbols,fs=1)
    data=_segment(2,48,1,_page_info(19,12))+_segment(3,6,1,coded,refs=(1,0))
    yield vector('huffman-user-text-first-position',data,expected,globals=globals,requirement='jbig2-huffman-text')
    page=_segment(1,48,1,_page_info(3,1))
    for marker in (b'\xff\xaa',b'\xff\xab'):
        yield vector(f'pdf-marker-{marker.hex()}-forbidden',marker+page,outcome='syntax',requirement='jbig2-embedded-segments')
    malformed=bytes([3])+struct.pack('>ii',0,2)+bits_bytes('010010000001')
    yield vector('huffman-user-table-oversubscribed',page+_segment(2,53,1,malformed),outcome='syntax',requirement='jbig2-huffman-tables')
    # A deferred segment can be referred to only by its contiguous attached extensions.
    prior=bytearray(_segment(2,36,1,region(rows,arithmetic(rows,2))));prior[4]|=128
    yield vector('deferred-non-extension-reference',page+prior+_segment(3,42,1,region(rows,refinement_payload(rows,rows,1)),refs=(2,)),outcome='syntax',requirement='jbig2-embedded-segments')
    data=page+prior+_segment(3,62,1,struct.pack('>I',0x20000001),refs=(2,))
    yield vector('deferred-attached-extension',data,[[0]*3],requirement='jbig2-embedded-segments')


# Stable helpers also consumed by the PDF image fixture generator. Callers can
# explicitly declare retention and complete strips when constructing new data.
def segment(number, kind, page, body, refs=(), long_page=False, extended=False, retain=False):
    return _segment(number,kind,page,body,refs,long_page,extended,retain)


def page_info(width,height,flags=64,stripe=0):
    return _page_info(width,height,flags,stripe)


def text_region(symbols,placements,width,height,strips=1,offset=0,operator=0,default=0,refinements=None,refine_template=0,complete_last_strip=False):
    return _text_region(symbols,placements,width,height,strips,offset,operator,default,refinements,refine_template,complete_last_strip=complete_last_strip)


def separate_stream_compatibility_cases():
    glyph=[[1,0,1],[0,1,0]]
    body,symbols=dictionary([glyph],2)
    for complete,retained in ((True,False),(False,True)):
        # The expected pixels depend only on the declared instances. These
        # compatibility cases make the limits of syntax validation explicit.
        globals=segment(0,0,0,body,retain=retained)
        coded,rows=_text_region(symbols,[(1,1,0)],7,4,complete_last_strip=complete)
        data=_segment(1,48,1,_page_info(7,4))+_segment(2,6,1,coded,refs=(0,))
        name='pdf-globals-cross-stream-lifetime' if complete else 'arithmetic-text-terminal-count'
        case=vector(name,data,rows,globals=globals,requirement='jbig2-embedded-segments' if complete else 'jbig2-arithmetic-text')
        yield Vector(case.name,case.files,case.requirements,'Declared-instance bitmap with explicit compatibility semantics',{'strict_valid':False})


def page_reference_crop_cases():
    page_rows=[[1]*13 for _ in range(9)]
    crop=[[1]*5 for _ in range(3)]
    target=[[int(x==y or x+y==4) for x in range(5)] for y in range(3)]
    for template in (0,1):
        for prediction in (False,True):
            # Samples outside this reference rectangle are zero even though
            # the adjacent page pixels are black (7.4.7.4 and 6.3.5.2).
            payload=refinement_payload(target,crop,template,typical=prediction)
            data=_segment(1,48,1,_page_info(13,9))+_segment(2,38,1,region(page_rows,arithmetic(page_rows,2)))+_segment(3,42,1,region(target,payload,x=4,y=3,operator=4))
            rows=[row[:] for row in page_rows]
            for y,row in enumerate(target):rows[y+3][4:9]=row
            yield vector(f'refinement-page-crop-t{template}-prediction{int(prediction)}',data,rows,requirement='jbig2-refinement')


def mmr_reference_cases():
    width=8192
    rows=[[0]*(width//2)+[1]*(width//2),[x%2 if x<width//2 else 0 for x in range(width)]]
    data=_segment(1,48,1,_page_info(width,2))+_segment(2,38,1,region(rows,mmr(rows)))
    yield vector('mmr-sparse-reference-dense-row',data,rows,requirement='jbig2-generic-mmr')


def empty_text_cases():
    body,symbols=huffman_dictionary([[[1]]])
    globals=_segment(0,0,0,body)
    for default in (0,1):
        arithmetic_body,rows=_text_region(symbols,[],7,4,default=default)
        page=_segment(1,48,1,_page_info(7,4))
        yield vector(f'arithmetic-empty-text-default{default}',page+_segment(2,6,1,arithmetic_body,refs=(0,)),rows,globals=globals,requirement='jbig2-arithmetic-text')
        ids=bits_bytes(''.join(format(int(n==1),'04b') for n in range(35))+'0')
        header=struct.pack('>IIIIBHHI',7,4,0,0,0,0x11+(default<<9),0,0)
        coded=header+ids+bits_bytes(huffman_word(11,1))
        yield vector(f'huffman-empty-text-default{default}',page+_segment(2,6,1,coded,refs=(0,)),rows,globals=globals,requirement='jbig2-huffman-text')


def reference_origin(s,t,glyph,transposed,corner):
    """Literal reference-corner steps of 6.4.5, before geometric clipping."""
    width,height=len(glyph[0]),len(glyph)
    right,bottom=corner in (2,3),corner in (0,2)
    # Step vi updates the S coordinate before selecting the reference corner.
    if not transposed and right: s+=width-1
    if transposed and bottom: s+=height-1
    anchor_x,anchor_y=(t,s) if transposed else (s,t)
    # Step viii anchors the selected pixel of the unrotated symbol bitmap.
    return anchor_x-(width-1 if right else 0),anchor_y-(height-1 if bottom else 0)


def unknown_segment(number,rows,payload,declared=None,x=0,y=0):
    declared=len(rows) if declared is None else declared
    body=struct.pack('>IIIIB',len(rows[0]),declared,x,y,0)+payload
    if payload[0]&1: body+=bytes(2)
    body+=struct.pack('>I',len(rows))
    header=_segment(number,38,1,b'')
    return header[:-4]+bytes([255])*4+body


def unknown_length_edge_cases():
    # A following segment proves that framing includes the four-byte count,
    # but excludes bytes belonging to the next header and coded bitmap.
    rows=[[1,0,1,0,0],[1,0,1,0,0],[0]*5]
    tail=[[0,1,0,1,1]]
    for template in range(4):
        for typical in (False,True):
            payload=arithmetic(rows,template,typical)
            data=_segment(1,48,1,_page_info(5,5))+unknown_segment(2,rows,payload,0xffffffff,y=1)
            data+=_segment(3,38,1,region(tail,mmr(tail),y=4))
            expected=[[0]*5]+rows+tail
            yield vector(f'unknown-length-arithmetic-t{template}-prediction{int(typical)}-followed',data,expected,requirement='jbig2-unknown-length')
    # FF AC is also a legal causal AT pair; it is not coded data or its end.
    payload=arithmetic(rows,2,adaptive=((-1,-84),))
    data=_segment(1,48,1,_page_info(5,3))+unknown_segment(2,rows,payload)
    yield vector('unknown-length-arithmetic-marker-in-adaptive-header',data,rows,requirement='jbig2-unknown-length')
    for width in range(1,9):
        pixels=[[int((x+y)%3==0) for x in range(width)] for y in range(3)]
        data=_segment(1,48,1,_page_info(width,4))+unknown_segment(2,pixels,mmr(pixels),4)
        tail=[[1]*width]
        data+=_segment(3,38,1,region(tail,arithmetic(tail,3),y=3))
        yield vector(f'unknown-length-mmr-width{width}-followed',data,pixels+tail,requirement='jbig2-unknown-length')
    page=_segment(1,48,1,_page_info(5,3))
    arithmetic_data=unknown_segment(2,rows,arithmetic(rows,2))
    mmr_data=unknown_segment(2,rows,mmr(rows))
    for name,data in (
        ('row-count-exceeds-height',unknown_segment(2,rows,arithmetic(rows,2),2)),
        ('mmr-missing-eofb',unknown_segment(2,rows,mmr(rows,eofb=False))),
        ('mmr-data-after-eofb',unknown_segment(2,rows,mmr(rows)+b'\x80')),
        ('arithmetic-missing-marker',arithmetic_data[:-6]),
        ('mmr-missing-marker',mmr_data[:-6]),
        ('intermediate-forbidden',arithmetic_data[:4]+bytes([36])+arithmetic_data[5:]),
        ('lossless-forbidden',arithmetic_data[:4]+bytes([39])+arithmetic_data[5:]),
    ):
        yield vector('unknown-length-'+name,page+data,outcome='syntax',requirement='jbig2-unknown-length')
    for coding,data in (('arithmetic',arithmetic_data),('mmr',mmr_data)):
        for count_bytes in range(4):
            yield vector(f'unknown-length-{coding}-row-count-truncated{count_bytes}',page+data[:-4]+data[-4:][:count_bytes],outcome='syntax',requirement='jbig2-unknown-length')


def text_layout_edge_cases():
    body,symbols=dictionary([[[1,0],[0,1],[1,1]],[[1,1,0],[0,1,1]]],2)
    globals=_segment(0,0,0,body)
    placements=[(-1,-1,0),(4,-1,1),(-2,5,1),(6,5,0),(1,1,0)]
    target=[[1,0,0,1],[0,1,1,0]]
    refinements=[None,(target,0,0),None,(target,0,0),None]
    for transposed in (False,True):
        for corner in range(4):
            coded,rows=_text_region(symbols,placements,10,8,strips=4,offset=-2,refinements=refinements,refine_template=1,transposed=transposed,corner=corner)
            page=_segment(1,48,1,_page_info(10,8))
            yield vector(f'arithmetic-layout-clipped-refined-{int(transposed)}-{corner}',page+_segment(2,6,1,coded,refs=(0,)),rows,globals=globals,requirement='jbig2-text-layout')
            # Each layout starts at region row zero while only modifying rows
            # below the end of stripe. This mirrors shared boundary padding.
            positions=[(1,3,0),(5,3,1)]
            coded,rows=_text_region(symbols,positions,10,8,transposed=transposed,corner=corner)
            striped=_segment(1,48,1,_page_info(10,8,98,0x8008))+_segment(2,50,1,struct.pack('>I',0))
            yield vector(f'arithmetic-layout-striped-{int(transposed)}-{corner}',striped+_segment(3,6,1,coded,refs=(0,)),rows,globals=globals,requirement='jbig2-text-layout')
            # Negative DFS/DS and multiple strips use Tables B.6/B.8/B.12.
            ids=bits_bytes(''.join(format(int(n==1),'04b') for n in range(35))+'00')
            bits=huffman_word(12,1)
            rows=[[0]*10 for _ in range(8)]
            huffman_placements=[(-1,1,0),(4,1,1),(2,1,0),(-2,5,1),(6,5,0)]
            previous_t,previous_s=-4,0
            for index,(s,t,glyph_id) in enumerate(huffman_placements):
                strip=t//4*4
                new=index==0 or huffman_placements[index-1][1]//4!=t//4
                if new:
                    bits+=huffman_word(12,(strip-previous_t)//4)+huffman_word(6,s-previous_s)
                    previous_t,previous_s=strip,s
                else:
                    ps,_,pid=huffman_placements[index-1]
                    advance=len(symbols[pid]) if transposed else len(symbols[pid][0])
                    bits+=huffman_word(8,s-ps-advance+1+2)
                bits+=format(t%4,'02b')+str(glyph_id)
                glyph=symbols[glyph_id]
                ox,oy=reference_origin(s,t,glyph,transposed,corner)
                for gy,row in enumerate(glyph):
                    for gx,b in enumerate(row):
                        if 0<=ox+gx<10 and 0<=oy+gy<8:rows[oy+gy][ox+gx]|=b
                if index+1==len(huffman_placements) or huffman_placements[index+1][1]//4!=t//4:bits+=huffman_word(8,None)
            flags=1+(corner<<4)+(64 if transposed else 0)+8+(30<<10)
            coded=struct.pack('>IIIIBHHI',10,8,0,0,0,flags,16,len(huffman_placements))+ids+bits_bytes(bits)
            yield vector(f'huffman-layout-clipped-{int(transposed)}-{corner}',page+_segment(2,6,1,coded,refs=(0,)),rows,globals=globals,requirement='jbig2-text-layout')


def stripe_identity_cases():
    for op in range(4):
        neutral=int(op in (1,3))
        for after in (False,True):
            rows=[[neutral]*5 for _ in range(4)]
            rows[2 if after else 0][2]=1-neutral
            data=_segment(1,48,1,_page_info(5,4,98+(4 if op==1 else 0),0x8004))
            drawing=_segment(3 if after else 2,38,1,region(rows,arithmetic(rows,2),operator=op))
            stripe=_segment(2 if after else 3,50,1,struct.pack('>I',1))
            data+=stripe+drawing if after else drawing+stripe
            initial=int(op==1)
            expected=[[(initial|b,initial&b,initial^b,1^initial^b)[op] for b in row] for row in rows]
            yield vector(f'stripe-identity-{op}-'+('after' if after else 'before'),data,expected,requirement='jbig2-page-combination')
    # REPLACE has no identity source value, even a white source writes pixels.
    rows=[[0]*5 for _ in range(4)]
    page=_segment(1,48,1,_page_info(5,4,98,0x8004))
    drawing=_segment(3,38,1,region(rows,arithmetic(rows,2),operator=4))
    yield vector('stripe-replace-padding-forbidden',page+_segment(2,50,1,struct.pack('>I',1))+drawing,outcome='syntax',requirement='jbig2-page-combination')


def profiles_grammar_cases():
    def profile(n,ids,page=0):return _segment(n,52,page,struct.pack('>I',len(ids))+b''.join(struct.pack('>I',i) for i in ids))
    rows=[[1,0],[0,1]]
    global_profile=profile(0,[1])
    page=_segment(2,48,1,_page_info(2,2))
    drawing=_segment(4,38,1,region(rows,arithmetic(rows,2)))
    yield vector('profiles-page-superset',page+profile(3,[1,2],1)+drawing,rows,globals=global_profile,requirement='jbig2-profiles')
    for name,globals,data in (
        ('global-must-be-first',_segment(0,53,0,bytes.fromhex('42 00 00 00 00 00 01 01 10 49 23 81 80'))+profile(1,[1]),page),
        ('second-global-forbidden',global_profile+profile(1,[1]),page),
        ('duplicate-page-forbidden',global_profile,page+profile(3,[1],1)+profile(4,[1],1)),
        ('page-omits-global',global_profile,page+profile(3,[2],1)),
        ('truncated-count',_segment(0,52,0,b'\0\0\0'),page),
        ('truncated-identifier',_segment(0,52,0,struct.pack('>I',1)+b'\0\0\0'),page),
        ('trailing-data',_segment(0,52,0,struct.pack('>I',0)+b'\0'),page),
    ):
        yield vector('profiles-'+name,data,outcome='syntax',globals=globals,requirement='jbig2-profiles')


def extended_template_cases():
    """T.88:2018 6.2.5/7.4.6: independent original bitmaps and field layout.

    Figure 50 and the explicit byte0..23 list define twelve signed pairs.
    The adjacent '32-byte' sentence conflicts with both; no extra bytes are
    invented or skipped. The requirement records this primary-text conflict.
    """
    rows = [[((x*73+y*29+x*y*17) ^ (x*11+y*y*9)) >> ((x+y)%7) & 1
             for x in range(23)] for y in range(17)]
    moved = ((-6,0),(3,-3),(-4,-2),(-2,-4),(5,-3),(4,-1),
             (-7,0),(-8,0),(4,-2),(6,-1),(-5,-2),(-6,-1))
    settings = [('nominal', EXTENDED_AT), ('all-moved', moved),
                ('overlapping', ((-1,0),)*12),
                ('coordinate-edges', ((-128,-128),(127,-1),(-1,0))*4)]
    for index, point in enumerate(moved):
        changed = list(EXTENDED_AT)
        changed[index] = point
        settings.append((f'moved-{index+1}', tuple(changed)))
    for name, adaptive in settings:
        encoded = arithmetic(rows, 0, adaptive=adaptive, extended=True)
        data = _segment(1,48,1,_page_info(23,17)) + _segment(2,38,1,region(rows,encoded))
        yield vector('extended-template-'+name,data,rows,requirement='jbig2-extended-template')
    repeated = [[0]*23] + [rows[0]]*3 + [rows[1]]*2 + [[0]*23]*2
    encoded = arithmetic(repeated,0,True,moved,extended=True)
    data = _segment(1,48,1,_page_info(23,len(repeated))) + _segment(2,39,1,region(repeated,encoded))
    yield vector('extended-template-typical',data,repeated,requirement='jbig2-extended-template')
    small = [[1,0,1,0,0],[1,0,1,0,0],[0]*5]
    tail = [[0,1,0,1,1]]
    for name, adaptive in [('nominal', EXTENDED_AT), ('marker-in-header', EXTENDED_AT[:11]+((-1,-84),))]:
        encoded = arithmetic(small,0,True,adaptive,extended=True)
        data = _segment(1,48,1,_page_info(5,5)) + unknown_segment(2,small,encoded,0xffffffff,y=1)
        data += _segment(3,38,1,region(tail,mmr(tail),y=4))
        yield vector('extended-template-unknown-'+name,data,[[0]*5]+small+tail,requirement='jbig2-extended-template')
    # EXTTEMPLATE is unused for templates1..3 (Table2 note e), so each still
    # has one adaptive pair, including in unknown-length segment framing.
    for template in (1,2,3):
        coded = arithmetic(small,template)
        coded = bytes([coded[0] | 16]) + coded[1:]
        data = _segment(1,48,1,_page_info(5,3)) + unknown_segment(2,small,coded)
        yield vector(f'extended-flag-unused-t{template}',data,small,requirement='jbig2-extended-template')
    # Only GBTEMPLATE is required zero for MMR by 7.4.6.2. TPGDON and
    # EXTTEMPLATE are unused procedure inputs, rather than reserved flag bits.
    coded = bytes([0x19]) + mmr(small)[1:]
    data = _segment(1,48,1,_page_info(5,3)) + _segment(2,38,1,region(small,coded))
    yield vector('extended-flag-unused-mmr',data,small,requirement='jbig2-extended-template')
    page = _segment(1,48,1,_page_info(5,3))
    encoded = arithmetic(small,0,extended=True)
    for length in range(24):
        data = page + _segment(2,38,1,region(small,encoded[:1+length]))
        yield vector(f'extended-template-truncated-at-{length}',data,outcome='syntax',requirement='jbig2-extended-template')
    for index in range(12):
        damaged = bytearray(encoded)
        damaged[1+index*2:3+index*2] = bytes(2)
        data = page + _segment(2,38,1,region(small,damaged))
        yield vector(f'extended-template-noncausal-{index+1}',data,outcome='syntax',requirement='jbig2-extended-template')
    unknown = unknown_segment(2,small,encoded,0xffffffff)
    for count in range(4):
        yield vector(f'extended-template-truncated-count-{count}',page+unknown[:-4]+unknown[-4:][:count],
                     outcome='syntax',requirement='jbig2-extended-template')
    for name,body in [
        ('missing-marker',unknown_segment(2,small,encoded[:-2])),
        ('count-exceeds-height',unknown_segment(2,small,encoded,2)),
        ('mmr-template',unknown_segment(2,small,b'\x03'+mmr(small)[1:])),
    ]:
        yield vector('extended-template-'+name,page+body,outcome='syntax',requirement='jbig2-extended-template')


def extended_template_render_cases():
    from dataclasses import replace
    from .paint import image_case
    selected = {'extended-template-all-moved','extended-template-typical',
                'extended-template-unknown-marker-in-header'}
    for item in extended_template_cases():
        if item.name not in selected:
            continue
        params = dict(line.split('=') for line in item.files['.params'].decode().splitlines())
        width, height = int(params['width']), int(params['height'])
        packed_pixels = item.files['.out']
        stride = (width+7)//8
        colors = [(255*(1-((packed_pixels[y*stride+x//8] >> (7-x%8)) & 1)),)*3
                  for y in range(height) for x in range(width)]
        settings = {'matrix':(width,0,0,height,2,2),'colors':colors,'requirement':'jbig2-extended-template'}
        name = 'jbig2-'+item.name
        yield image_case(name,item.files['.jb2'],b'/BitsPerComponent 1 /ColorSpace /DeviceGray /Filter /JBIG2Decode',[],width,height,**settings)
        control = image_case(name+'-rgb-control',bytes(v for c in colors for v in c),
                             b'/BitsPerComponent 8 /ColorSpace /DeviceRGB',[],width,height,**settings)
        yield replace(control,extra={**control.extra,'control_of':'image/'+name})
