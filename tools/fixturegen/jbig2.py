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
    data=segment(1,48,1,page_info(13,3,4))+segment(2,36,1,region(rows,arithmetic(rows,2)))
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


def dictionary(glyphs,template,selection=None,imported=(),retain=False,reuse_contexts=None,return_contexts=False):
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
                refinements=None,refine_template=0,alphabet_count=None,complete_last_strip=False):
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
        for gy,row in enumerate(glyph):
            for gx,b in enumerate(row):
                if 0<=x+gx<width and 0<=y+gy<height:
                    a=rows[y+gy][x+gx]
                    rows[y+gy][x+gx]=(a|b,a&b,a^b,1^a^b)[operator]
        previous_width=len(glyph[0])
        last=index+1==len(placements)
        if (last and complete_last_strip) or (not last and placements[index+1][1]//strips!=y//strips): integer(encoder,"advance",None)
    return rows


def text_region(symbols,placements,width,height,strips=1,offset=0,operator=0,default=0,refinements=None,refine_template=0):
    encoder=ArithmeticEncoder()
    rows=text_pixels(encoder,symbols,placements,width,height,strips,offset,operator,default,refinements,refine_template)
    flags=0x10+((strips.bit_length()-1)<<2)+(operator<<7)+(default<<9)+((offset&31)<<10)
    if refinements is not None: flags+=2+(refine_template<<15)
    header=struct.pack(">IIIIBH",width,height,0,0,0,flags)
    if refinements is not None and refine_template==0: header+=bytes([255,255,255,255])
    return header+struct.pack(">I",len(placements))+encoder.finish(),rows


# Generic and symbol vectors share the fixture driver entry point.

def cases():
    yield from pdf_mapping_cases()
    yield from refinement_cases()
    yield from refined_symbol_cases()
    yield from refinement_hostile_cases()
    for case in generic_cases():
        if case.name=="unsupported-refinement-region":
            data=case.files[".jb2"]
            yield vector("refinement-truncated",data,outcome="syntax",requirement="jbig2-hostile-input")
        elif case.name=="unsupported-symbol-dictionary":
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
        yield vector(name+"-truncated",page+segment(2,0,1,struct.pack(">H",flags)),outcome="syntax",requirement="jbig2-hostile-input")
    for name,flags in (("text-refinement",0x12),("text-layout",0)):
        coded=struct.pack(">IIIIBH",17,6,0,0,0,flags)
        if name=="text-refinement":
            yield vector(name+"-truncated",page+segment(2,6,1,coded),outcome="syntax",requirement="jbig2-hostile-input")
        else:
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


def pdf_mapping_cases():
    """PDF mapping rules derived from ISO 32000-2:2020 EC3, 7.4.7/Table 12.

    JBIG2 coding details used to represent these small bitmaps remain subject
    to primary-text review of ISO/IEC 14492 and T.88.
    """
    rows=[[1,0,1],[0,1,0]]
    page=segment(10,48,1,page_info(3,2))
    ordinary=segment(11,38,1,region(rows,arithmetic(rows,2)))
    # Increasing segment numbers are recommended, not required by PDF.
    yield vector("pdf-sequential-nonmonotonic",page+segment(2,38,1,region(rows,arithmetic(rows,2))),rows,requirement="jbig2-embedded-segments")
    header=bytes([151,74,66,50,13,10,26,10,1,0,0,0,1])
    yield vector("pdf-file-header-forbidden",header+page+ordinary,outcome="syntax",requirement="jbig2-embedded-segments")
    yield vector("pdf-region-colour-extension-forbidden",page+segment(11,38,1,region(rows,arithmetic(rows,2),operator=8)),outcome="syntax",requirement="jbig2-explicit-unsupported")
    body,symbols=dictionary([rows],2)
    globals=segment(0,0,0,body)
    coded,expected=text_region(symbols,[(0,0,0)],3,2)
    data=segment(1,48,1,page_info(3,2))+segment(2,6,1,coded,refs=(0,))
    yield vector("pdf-globals-single-image",data,expected,globals=globals,requirement="jbig2-embedded-segments")
    yield vector("pdf-global-in-page-stream",globals+data,outcome="syntax",requirement="jbig2-embedded-segments")
    yield vector("pdf-page-in-globals",data,outcome="syntax",globals=segment(0,0,1,body),requirement="jbig2-embedded-segments")
    yield vector("pdf-page-two-in-globals",data,outcome="syntax",globals=segment(0,0,2,body),requirement="jbig2-embedded-segments")
    yield vector("pdf-global-missing",data,outcome="syntax",requirement="jbig2-embedded-segments")


def refinement_pixels(encoder, rows, reference, template, dx=0, dy=0, typical=False,
                      adaptive=((-1,-1),(-1,-1)), namespace="refinement"):
    """Geometric refinement neighbourhoods, in descending bit significance.

    The T.88 locator is pending primary-text review. Encoded vectors require
    exact external bitmap agreement before being accepted as evidence.
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
            page=segment(1,48,1,page_info(19,13))
            intermediate=segment(2,36,1,region(reference,arithmetic(reference,2)))
            payload=refinement_payload(target,reference,template,typical=typical)
            coded=segment(3,42,1,region(target,payload,x=3,y=2),refs=(2,))
            expected=[[0]*19 for _ in range(13)]
            for y,row in enumerate(target): expected[y+2][3:16]=row
            yield vector(f"refinement-t{template}-prediction-{int(typical)}",page+intermediate+coded,expected,requirement="jbig2-refinement")
            # A refinement without a referral uses the current page as its
            # reference, including the region's nonzero page origin.
            page_reference=[[0]*19 for _ in range(13)]
            for y,row in enumerate(reference): page_reference[y+2][3:16]=row
            payload=refinement_payload(target,page_reference,template,dx=-3,dy=-2,typical=typical)
            initial=segment(2,38,1,region(reference,arithmetic(reference,2),x=3,y=2))
            coded=segment(3,43,1,region(target,payload,x=3,y=2,operator=4))
            yield vector(f"refinement-page-t{template}-prediction-{int(typical)}",page+initial+coded,expected,requirement="jbig2-refinement")
    adaptive=((-3,-2),(2,1))
    payload=refinement_payload(target,reference,0,adaptive=adaptive)
    page=segment(1,48,1,page_info(13,9))
    initial=segment(2,36,1,region(reference,arithmetic(reference,2)))
    refined=segment(3,40,1,region(target,payload),refs=(2,))
    final=segment(4,42,1,region(reference,refinement_payload(reference,target,1)),refs=(3,))
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
    globals=segment(0,0,0,body)
    grown=[[int(x==y or x+y==4) for x in range(5)]for y in range(5)]
    small=[[1,0],[0,1]]
    for template in (0,1):
        for dx,dy in ((0,0),(-1,1),(2,-2)):
            coded,rows=text_region(symbols,[(1,1,0),(7,1,1),(12,1,0),(1,8,1)],19,13,
                refinements=[(grown,dx,dy),None,(small,-dx,-dy),(grown,0,0)],refine_template=template)
            data=segment(1,48,1,page_info(19,13))+segment(2,6,1,coded,refs=(0,))
            yield vector(f"text-refinement-t{template}-offset-{dx+1}-{dy+2}",data,rows,globals=globals,requirement="jbig2-refinement")
        for dx,dy in ((0,0),(-2,1),(2,-1)):
            aggregate,selected,_=aggregate_dictionary(symbols,[("single",grown,0,dx,dy)],template)
            global_aggregate=globals+segment(1,0,0,aggregate,refs=(0,))
            coded,rows=text_region(selected,[(1,1,2),(8,1,0)],17,8)
            data=segment(2,48,1,page_info(17,8))+segment(3,6,1,coded,refs=(1,))
            yield vector(f"symbol-single-refinement-t{template}-offset-{dx+2}-{dy+1}",data,rows,globals=global_aggregate,requirement="jbig2-symbol-aggregation")
        aggregate_rows=[[0]*13 for _ in range(5)]
        for gx,gy,glyph in ((0,0,symbols[0]),(6,0,grown)):
            for y,row in enumerate(glyph):
                for x,value in enumerate(row):aggregate_rows[gy+y][gx+x]|=value
        definitions=[("aggregate",aggregate_rows,[(0,0,0),(6,0,1)],[None,(grown,1,-1)])]
        aggregate,selected,_=aggregate_dictionary(symbols,definitions,template)
        global_aggregate=globals+segment(1,0,0,aggregate,refs=(0,))
        coded,rows=text_region(selected,[(1,1,2)],17,8)
        data=segment(2,48,1,page_info(17,8))+segment(3,6,1,coded,refs=(1,))
        yield vector(f"symbol-multiple-aggregation-t{template}",data,rows,globals=global_aggregate,requirement="jbig2-symbol-aggregation")
    # Carry bitmap probabilities across dictionary boundaries, while integer
    # coding contexts start afresh. Dense shapes distinguish reuse from reset.
    glyphs_a=[[[((x*3+y*7+x*y)^y)>>2&1 for x in range(11)] for y in range(7)]]
    glyphs_b=[[[int(x%3==y%3) for x in range(11)]for y in range(7)]]
    for template in range(4):
        first,initial,contexts=dictionary(glyphs_a,template,retain=True,return_contexts=True)
        second,selected=dictionary(glyphs_b,template,imported=initial,reuse_contexts=contexts)
        global_retained=segment(0,0,0,first)+segment(1,0,0,second,refs=(0,))
        coded,rows=text_region(selected,[(1,1,0),(14,1,1)],27,10)
        data=segment(2,48,1,page_info(27,10))+segment(3,6,1,coded,refs=(1,))
        yield vector(f"retained-generic-contexts-t{template}",data,rows,globals=global_retained,requirement="jbig2-retained-contexts")
    for template in (0,1):
        first,initial,contexts=aggregate_dictionary(symbols,[("single",grown,0,1,-1)],template,retain=True)
        second_target=[[int((x+y)%3==0)for x in range(5)]for y in range(5)]
        second,selected,_=aggregate_dictionary(initial,[("single",second_target,2,-1,1)],template,reuse_contexts=contexts)
        global_retained=globals+segment(1,0,0,first,refs=(0,))+segment(2,0,0,second,refs=(1,))
        coded,rows=text_region(selected,[(1,1,2),(8,1,3)],17,8)
        data=segment(3,48,1,page_info(17,8))+segment(4,6,1,coded,refs=(2,))
        yield vector(f"retained-refinement-contexts-t{template}",data,rows,globals=global_retained,requirement="jbig2-retained-contexts")


def refinement_hostile_cases():
    rows=[[1,0,1],[0,1,0],[1,0,1]]
    page=segment(1,48,1,page_info(9,7))
    prior=segment(2,36,1,region(rows,arithmetic(rows,2)))
    cases={
        "refinement-adaptive-noncausal":(region(rows,b"\x00\x00\x00\xff\xff\xff\xac"),(2,)),
        "refinement-reserved-flags":(region(rows,b"\x04"),(2,)),
        "refinement-two-references":(region(rows,refinement_payload(rows,rows,1)),(2,2)),
        "refinement-wrong-reference":(region(rows,refinement_payload(rows,rows,1)),(1,)),
        "refinement-arithmetic-truncated":(region(rows,refinement_payload(rows,rows,1)[:-1]),(2,)),
    }
    for name,(body,refs) in cases.items():
        yield vector(name,page+prior+segment(3,42,1,body,refs=refs),outcome="syntax",requirement="jbig2-hostile-input")
    yield vector("reserved-generic-type-32",page+segment(2,32,1,b""),outcome="unsupported",feature="jbig2.segment-type",requirement="jbig2-explicit-unsupported")
    body,symbols=dictionary([rows],2)
    globals=segment(0,0,0,body)
    for name,value,outcome in (("text-refinement-zero-width",-3,"syntax"),("text-refinement-pixel-limit",30_000_000,"limit")):
        encoder=ArithmeticEncoder()
        integer(encoder,"strip",0);integer(encoder,"strip",1);integer(encoder,"first",1)
        identifier(encoder,1,0);integer(encoder,"refine-indicator",1)
        for family,n in (("refine-width",value),("refine-height",0),("refine-x",0),("refine-y",0)):integer(encoder,family,n)
        coded=struct.pack(">IIIIBHI",9,7,0,0,0,0x8012,1)+encoder.finish()
        yield vector(name,page+segment(2,6,1,coded,refs=(0,)),outcome=outcome,globals=globals,requirement="jbig2-hostile-input")
    encoder=ArithmeticEncoder()
    integer(encoder,"strip",0);integer(encoder,"strip",1);integer(encoder,"first",1)
    identifier(encoder,1,0);integer(encoder,"refine-indicator",2)
    coded=struct.pack(">IIIIBHI",9,7,0,0,0,0x8012,1)+encoder.finish()
    yield vector("text-refinement-invalid-indicator",page+segment(2,6,1,coded,refs=(0,)),outcome="syntax",globals=globals,requirement="jbig2-hostile-input")
    second,_=dictionary([rows],2,imported=symbols,reuse_contexts={})
    yield vector("retained-contexts-missing",page,outcome="syntax",globals=globals+segment(1,0,0,second,refs=(0,)),requirement="jbig2-hostile-input")
    first,initial,contexts=dictionary([rows],0,retain=True,return_contexts=True)
    second,_=dictionary([rows],1,imported=initial,reuse_contexts=contexts)
    yield vector("retained-contexts-incompatible",page,outcome="syntax",globals=segment(0,0,0,first)+segment(1,0,0,second,refs=(0,)),requirement="jbig2-hostile-input")
    first,initial,contexts=dictionary([rows],2,retain=True,return_contexts=True)
    second,_,_=aggregate_dictionary(initial,[("single",rows,0,0,0)],1,reuse_contexts=contexts)
    yield vector("retained-refinement-contexts-missing",page,outcome="syntax",globals=segment(0,0,0,first)+segment(1,0,0,second,refs=(0,)),requirement="jbig2-hostile-input")
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
        aggregate_globals=globals+segment(1,0,0,body,refs=(0,))
        coded,expected=text_region(selected,[(1,1,1),(5,1,2),(1,8,3)],17,15)
        data=segment(2,48,1,page_info(17,15))+segment(3,6,1,coded,refs=(1,))
        yield vector(f"symbol-mixed-aggregation-state-t{template}",data,expected,globals=aggregate_globals,requirement="jbig2-symbol-aggregation")

    for template in (0,1):
        for bit in (0,1):
            reference=[[bit]*25 for _ in range(11)]
            target=[[bit]*17 for _ in range(5)]
            page=segment(1,48,1,page_info(17,5))
            prior=segment(2,36,1,region(reference,arithmetic(reference,2)))
            refined=segment(3,42,1,region(target,refinement_payload(target,reference,template,typical=True)),refs=(2,))
            yield vector(f"refinement-prediction-uniform-{bit}-t{template}",page+prior+refined,target,requirement="jbig2-refinement")
