"""Independent JPEG 2000 Part 1 encoder and JP2 box fixtures.

The encoder writes coding-pass prefixes, rectangular code-blocks, precincts, layers,
progression volumes, packed headers, ROI and all Part 1 block styles. It implements
forward integer 5/3 lifting, RCT, scalar-quantized 9/7 lifting, tag-tree leaves,
and MQ arithmetic encoding, using only the Python standard library.
Expected samples are input pixels or closed-form coefficient reconstructions,
never the output of a decoder. Lossy
vectors have an explicit tolerance for quantization. This is test tooling,
not a general JPEG 2000 encoder.
"""
import math
import struct

from .model import Vector

AREA = "jpx"

# Probability estimates and transitions checked against T.800 Table C.2.
# Qe, next-MPS, next-LPS, exchange flag; Table C.2 is the primary source.
PROBABILITIES = [
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
]


class ArithmeticEncoder:
    def __init__(self, state=None, mps=None):
        self.a, self.c, self.ct = 0x8000, 0, 12
        self.output = [0]  # Carry propagation sentinel, not part of the stream.
        self.reset()
        if state is not None:
            self.state, self.mps = list(state), list(mps)  # pyright: ignore[reportArgumentType]

    def reset(self):
        self.state = [0] * 19
        self.state[0], self.state[17], self.state[18] = 4, 3, 46
        self.mps = [0] * 19

    def emit_byte(self):
        if self.output[-1] != 255 and self.c & 0x8000000:
            self.output[-1] += 1
        shift = 20 if self.output[-1] == 255 else 19
        self.output.append((self.c >> shift) & 255)
        self.c &= (1 << shift) - 1
        self.ct = 7 if shift == 20 else 8

    def write(self, context, value):
        qe, nmps, nlps, switch = PROBABILITIES[self.state[context]]
        self.a -= qe
        if value == self.mps[context]:
            if self.a >= 0x8000:
                self.c += qe
                return
            if self.a < qe:
                self.a = qe
            else:
                self.c += qe
            self.state[context] = nmps
        else:
            if self.a < qe:
                self.c += qe
            else:
                self.a = qe
            self.mps[context] ^= switch
            self.state[context] = nlps
        while self.a < 0x8000:
            self.a <<= 1
            self.c <<= 1
            self.ct -= 1
            if self.ct == 0:
                self.emit_byte()

    def finish(self, predictable=False):
        if predictable:
            remaining = 12-self.ct
            while remaining > 0:
                self.c <<= self.ct
                self.emit_byte()
                remaining -= self.ct
            if self.output[-1] != 255:
                self.emit_byte()
            # The last byte is the arithmetic coder's pending carry byte.
            return bytes(self.output[1:-1])
        upper = self.c + self.a
        self.c |= 65535
        if self.c >= upper:
            self.c -= 0x8000
        for _ in range(2):
            self.c <<= self.ct
            self.emit_byte()
        if self.output[-1] == 255:
            self.output.pop()
        return bytes(self.output[1:])


class PassEncoder:
    def __init__(self, planes, modes, roi=0, max_passes=None):
        self.total, self.modes, self.pass_index, self.first = planes*3-2, modes, 0, 0
        if max_passes is not None:
            assert 1 <= max_passes <= self.total
            self.total = max_passes
        self.encoder = ArithmeticEncoder()
        self.state, self.mps = self.encoder.state, self.encoder.mps
        self.raw_bits, self.parts, self.raw = [], [], False

    def write(self, context, value):
        if self.raw:
            self.raw_bits.append(value)
        else:
            self.encoder.write(context, value)

    def reset(self):
        fresh = ArithmeticEncoder()
        self.state, self.mps = fresh.state, fresh.mps
        if not self.raw:
            self.encoder.state, self.encoder.mps = self.state, self.mps

    def end_pass(self):
        # D.4/D.6: segment boundaries depend on coding-pass count, not ROI shift.
        self.pass_index += 1
        boundary = (self.pass_index == self.total or self.modes & 4 or
                    (self.modes & 1 and self.pass_index >= 10 and self.pass_index % 3 != 2))
        if not boundary:
            return
        if self.raw:
            result, position, count = bytearray(), 0, 8
            while position < len(self.raw_bits):
                group = self.raw_bits[position:position+count]
                padding = [i % 2 for i in range(count-len(group))]
                value = sum(bit << (count-1-i) for i, bit in enumerate(group+padding))
                result.append(value)
                position += count
                count = 7 if value == 255 else 8
            if result and result[-1] == 255:
                result.append(0x2a)
            data = bytes(result)
        else:
            data = self.encoder.finish(bool(self.modes & 16))
            self.state, self.mps = self.encoder.state, self.encoder.mps
        self.parts.append((self.pass_index-self.first, data))
        self.first = self.pass_index
        self.raw = bool(self.modes & 1 and self.pass_index >= 10 and self.pass_index % 3 != 0)
        self.raw_bits = []
        self.encoder = ArithmeticEncoder(self.state,self.mps)


def encode_block(coefficients, orientation, modes=0, roi=0, max_passes=None):
    height, width = len(coefficients), len(coefficients[0])
    maximum = max(abs(v) for row in coefficients for v in row)
    if maximum == 0:
        return 0, []
    planes = maximum.bit_length()
    sig, visited, refined = set(), set(), set()
    arithmetic = PassEncoder(planes,modes,roi,max_passes)

    def neighbours(x, y):
        h = [(xx, y) for xx in (x - 1, x + 1) if (xx, y) in sig]
        vertical_rows = (y-1,) if modes & 8 and y % 4 == 3 else (y-1,y+1)
        v = [(x, yy) for yy in vertical_rows if (x, yy) in sig]
        d = sum((xx, yy) in sig for xx in (x - 1, x + 1) for yy in vertical_rows)
        sign = lambda points: max(-1, min(1, sum(1 if coefficients[yy][xx] > 0 else -1 for xx, yy in points)))
        return len(h), len(v), d, sign(h), sign(v)

    def zc(x, y):
        h, v, d, _, _ = neighbours(x, y)
        if orientation == 1:
            h, v = v, h
        if orientation == 3:
            return ((0, 1, 2)[min(h + v, 2)] if d == 0 else
                    (3, 4, 5)[min(h + v, 2)] if d == 1 else
                    6 + int(h + v > 0) if d == 2 else 8)
        if h == 0:
            return min(d, 2) if v == 0 else 3 if v == 1 else 4
        return (5 if d == 0 else 6) if h == 1 and v == 0 else 7 if h == 1 else 8

    def significant(x, y, plane, implicit=False):
        value = (abs(coefficients[y][x]) >> plane) & 1
        if not implicit:
            arithmetic.write(zc(x, y), value)
        if value:
            _, _, _, h, v = neighbours(x, y)
            prediction = int(h < 0 or (h == 0 and v < 0))
            if prediction:
                h, v = -h, -v
            context = (9 if v == 0 else 10) if h == 0 else 11 if v < 0 else 12 if v == 0 else 13
            arithmetic.write(context, int(coefficients[y][x] < 0) ^ (0 if arithmetic.raw else prediction))
            sig.add((x, y))

    positions = [(x, y) for stripe in range(0, height, 4) for x in range(width)
                 for y in range(stripe, min(stripe + 4, height))]
    for plane in reversed(range(planes)):
        if plane != planes - 1:
            for x, y in positions:
                if (x, y) not in sig and zc(x, y) != 0:
                    visited.add((x, y))
                    significant(x, y, plane)
            if modes & 2:
                arithmetic.reset()
            arithmetic.end_pass()
            if arithmetic.pass_index == arithmetic.total:
                return planes, arithmetic.parts
            for x, y in positions:
                if (x, y) in sig and (x, y) not in visited:
                    context = 16 if (x, y) in refined else 14 + int(zc(x, y) != 0)
                    arithmetic.write(context, (abs(coefficients[y][x]) >> plane) & 1)
                    refined.add((x, y))
            if modes & 2:
                arithmetic.reset()
            arithmetic.end_pass()
            if arithmetic.pass_index == arithmetic.total:
                return planes, arithmetic.parts
        for stripe in range(0, height, 4):
            for x in range(width):
                end = min(height, stripe + 4)
                start = stripe
                run_mode = end - stripe == 4 and all((x, y) not in sig and (x, y) not in visited
                    and zc(x, y) == 0 for y in range(stripe, end))
                if run_mode:
                    run = next((y - stripe for y in range(stripe, end)
                                if (abs(coefficients[y][x]) >> plane) & 1), 4)
                    arithmetic.write(17, int(run != 4))
                    if run == 4:
                        continue
                    arithmetic.write(18, run >> 1)
                    arithmetic.write(18, run & 1)
                    significant(x, stripe + run, plane, implicit=True)
                    start = stripe + run + 1
                for y in range(start, end):
                    if (x, y) not in sig and (x, y) not in visited:
                        significant(x, y, plane)
        if modes & 32:
            for bit in (1, 0, 1, 0):
                arithmetic.write(18, bit)
        if modes & 2:
            arithmetic.reset()
        arithmetic.end_pass()
        if arithmetic.pass_index == arithmetic.total:
            return planes, arithmetic.parts
        visited.clear()
    return planes, arithmetic.parts


def forward_line(values, origin, reversible):
    result = list(values)
    if len(result) == 1:
        if origin & 1:
            result[0] *= 2
        return result
    low = origin & 1
    def lift(parity, function):
        for i in range(parity, len(result), 2):
            left = i - 1 if i else 1
            right = i + 1 if i + 1 < len(result) else i - 1
            result[i] += function(result[left] + result[right])
    if reversible:
        lift(1-low, lambda s: -(s // 2))
        lift(low, lambda s: (s + 2) // 4)
    else:
        for parity, factor in ((1-low, -1.586134342059924), (low, -.052980118572961),
                               (1-low, .882911075530934), (low, .443506852043971)):
            lift(parity, lambda s, factor=factor: s * factor)
        result = [v / 1.230174104914001 if i % 2 == low else v * 1.230174104914001
                  for i, v in enumerate(result)]
    return result


def transform(plane, x0, y0, levels, reversible):
    if levels == 0:
        return [plane]
    columns = [forward_line(list(col), y0, reversible) for col in zip(*plane)]
    rows = list(map(list, zip(*columns)))
    values = [forward_line(row, x0, reversible) for row in rows]
    bands = []
    for orientation in range(4):
        parity_x = (x0 & 1) ^ (orientation & 1)
        parity_y = (y0 & 1) ^ (orientation >> 1)
        bands.append([row[parity_x::2] for row in values[parity_y::2]])
    return transform(bands[0], (x0+1)//2, (y0+1)//2, levels-1, reversible) + bands[1:]


class PacketBits:
    def __init__(self):
        self.bits = []

    def number(self, value, count=1):
        self.bits.extend((value >> bit) & 1 for bit in reversed(range(count)))

    def finish(self):
        result, position, count = bytearray(), 0, 8
        while position < len(self.bits):
            group = self.bits[position:position+count]
            value = sum(bit << (count - 1 - i) for i, bit in enumerate(group))
            result.append(value)
            position += count
            count = 7 if value == 255 else 8
        if result[-1] == 255:
            result.append(0)
        return bytes(result)


class TagTree:
    """Encode shared minimum values along a rectangular leaf-to-root tree."""
    def __init__(self, width, height, leaves):
        self.levels = [(width, height, list(leaves))]
        while width > 1 or height > 1:
            previous = self.levels[-1][2]
            parent = [min(previous[y*width+x]
                          for y in range(py*2, min(py*2+2, height))
                          for x in range(px*2, min(px*2+2, width)))
                      for py in range((height+1)//2) for px in range((width+1)//2)]
            width, height = (width+1)//2, (height+1)//2
            self.levels.append((width, height, parent))
        self.low = [[0]*len(values) for _, _, values in self.levels]
        self.known = [set() for _ in self.levels]

    def write(self, bits, x, y, threshold):
        inherited = 0
        for level in reversed(range(len(self.levels))):
            width, _, values = self.levels[level]
            index = (y >> level)*width+(x >> level)
            self.low[level][index] = max(self.low[level][index], inherited)
            while self.low[level][index] < threshold and index not in self.known[level]:
                known = values[index] == self.low[level][index]
                bits.number(int(known))
                if known:
                    self.known[level].add(index)
                else:
                    self.low[level][index] += 1
            inherited = values[index] if index in self.known[level] else self.low[level][index]


def partition(band, origin, block_size):
    height, width = len(band), len(band[0]) if band else 0
    x0, y0 = origin
    block_width, block_height = (block_size,block_size) if isinstance(block_size,int) else block_size
    if width == 0 or height == 0:
        return 0, 0, []
    xs = [x0] + list(range((x0//block_width+1)*block_width, x0+width, block_width)) + [x0+width]
    ys = [y0] + list(range((y0//block_height+1)*block_height, y0+height, block_height)) + [y0+height]
    blocks = [[row[a-x0:b-x0] for row in band[c-y0:d-y0]]
              for c, d in zip(ys, ys[1:]) for a, b in zip(xs, xs[1:])]
    return len(xs)-1, len(ys)-1, blocks


def packet(bands, maxima, origins, block_size=64, modes=0, markers=False, layer=0, roi=0, empty_passes=False, max_passes=None):
    header, body = PacketBits(), b""
    present = any(any(v for row in band for v in row) for band in bands)
    header.number(int(present))
    if present:
        for orientation_index, (band, maximum, origin) in enumerate(zip(bands, maxima, origins)):
            columns, rows, blocks = partition(band, origin, block_size)
            if not blocks:
                continue
            encoded = [encode_block(block, 0 if len(bands) == 1 else orientation_index+1, modes, roi, max_passes) for block in blocks]
            if empty_passes:
                # D.4.1 allows zero-byte ordinary segments when the pass has no
                # decisions. In this singleton all samples are already significant.
                assert len(blocks) == 1 and len(blocks[0]) == len(blocks[0][0]) == 1 and modes == 4
                encoded = [(n,[(passes,b"" if i and i % 3 != 2 else data) for i,(passes,data) in enumerate(pieces)]) for n,pieces in encoded]
            inclusion = TagTree(columns, rows, [layer if planes else layer+1 for planes, _ in encoded])
            zeros = TagTree(columns, rows, [maximum-planes if planes else maximum for planes, _ in encoded])
            for index, (planes, pieces) in enumerate(encoded):
                x, y = index % columns, index // columns
                inclusion.write(header, x, y, layer+1)
                if planes == 0:
                    continue
                zero_planes = maximum - planes
                assert zero_planes >= 0
                zeros.write(header, x, y, zero_planes+1)
                passes = sum(n for n,_ in pieces)
                if passes == 1:
                    header.number(0)
                elif passes == 2:
                    header.number(2, 2)
                elif passes <= 5:
                    header.number(12 + passes - 3, 4)
                elif passes <= 36:
                    header.number(0b1111, 4)
                    header.number(passes - 6, 5)
                else:
                    header.number(0b111111111, 9)
                    header.number(passes - 37, 7)
                extra = max([0]+[len(data).bit_length()-3-(n.bit_length()-1) for n,data in pieces])
                header.number((1 << extra)-1, extra)
                header.number(0)
                for n,data in pieces:
                    header.number(len(data),3+(n.bit_length()-1)+extra)
                    body += data
    return header.finish() + (b"\xff\x92" if markers else b""), body


def marker(code, payload=b""):
    return struct.pack(">HH", 0xff00 | code, len(payload)+2) + payload


def subband_bounds(bounds,orientation):
    if orientation == 0:
        return bounds
    x0,y0,x1,y1 = bounds
    x = lambda v: v//2 if orientation & 1 else (v+1)//2
    y = lambda v: v//2 if orientation & 2 else (v+1)//2
    return x(x0),y(y0),x(x1),y(y1)


def progression_keys(keys,default,changes):
    def ordering(key,order):
        l,r,c,p,x,y = key
        return ((l,r,c,p) if order == 0 else (r,l,c,p) if order == 1 else
                (r,y,x,c,l) if order == 2 else (y,x,c,r,l) if order == 3 else (c,y,x,r,l))
    emitted, result = set(), []
    for r0,c0,l1,r1,c1,order in changes or [(0,0,65535,33,65535,default)]:
        selected = [key for key in keys if key not in emitted and key[0] < l1
                    and r0 <= key[1] < r1 and c0 <= key[2] < c1]
        result += sorted(selected,key=lambda key:ordering(key,order))
        emitted.update(selected)
    return result


def encode_poc(changes):
    return b"".join(struct.pack(">BBHBBB",r0,c0,l1,r1,c1,order) for r0,c0,l1,r1,c1,order in changes)


def packed_markers(code,headers,split):
    # A.7.5: a marker ends after a complete header, never inside it.
    chunks, current = [], b""
    for header in headers:
        if current and len(current)+len(header) > split:
            chunks.append(current)
            current = b""
        current += header
    if current:
        chunks.append(current)
    assert len(chunks) <= 256
    return b"".join(marker(code,bytes([index])+chunk) for index,chunk in enumerate(chunks))


def ppm_markers(parts,split):
    # A.7.4: Nppm stays whole and every fragment ends at a packet boundary.
    chunks, current = [], b""
    for headers in parts:
        size = struct.pack(">I",sum(map(len,headers)))
        for index,header in enumerate(headers):
            item = (size if index == 0 else b"")+header
            if current and len(current)+len(item) > split:
                chunks.append(current)
                current = b""
            current += item
    if current:
        chunks.append(current)
    assert len(chunks) <= 256
    return b"".join(marker(0x60,bytes([index])+chunk) for index,chunk in enumerate(chunks))


def codestream(planes, width, height, *, levels=0, tile=None, progression=0,
               bits=8, signed=False, mct=False, lossy=False, origin=(0, 0),
               tile_parts=False, markers=False, modes=0, extra_markers=False, block_exp=6, layers=1, derived_quant=False, precincts=None, poc=(), late_poc=(), packed=None, packed_split=60000, roi=0):
    x0, y0 = origin
    tile = tile or (width, height)
    count = len(planes)
    siz = struct.pack(">H8IH", 0, width+x0, height+y0, x0, y0, tile[0], tile[1], x0, y0, count)
    siz += bytes([bits-1 + 128*int(signed), 1, 1])*count
    precinct_exponents = [precincts]*(levels+1) if isinstance(precincts,tuple) else precincts or [(15,15)]*(levels+1)
    roi = [roi]*count if isinstance(roi,int) else roi
    coding = bytes([(6 if markers else 0) | int(precincts is not None), progression]) + struct.pack(">H", layers) + bytes([int(mct), levels, block_exp-2, block_exp-2, modes, int(not lossy)])
    if precincts is not None:
        coding += bytes(x+(y << 4) for x,y in precinct_exponents)
    if lossy:
        # Delta=1/4 for every subband (Rb=bits, exponent=bits+2, mantissa=0).
        exponents = ([bits+2] + [bits+2-(r-1) for r in range(1,levels+1) for _ in range(3)]
                     if derived_quant else [bits+2]*(1+levels*3))
        qcd = (bytes([0x41])+struct.pack(">H", (bits+2) << 11) if derived_quant else
               bytes([0x42])+b"".join(struct.pack(">H", e << 11) for e in exponents))
    else:
        exponents = [bits] + [bits + gain for _ in range(levels) for gain in (1, 1, 2)]
        qcd = bytes([0x40]) + bytes(exp << 3 for exp in exponents)
    result = b"\xff\x4f" + marker(0x51, siz) + marker(0x52, coding) + marker(0x5c, qcd)
    if poc:
        result += marker(0x5f,encode_poc(poc))
    for component,shift in enumerate(roi):
        if shift:
            result += marker(0x5e,bytes([component,0,shift]))
    encoded_parts, part_headers = [], []
    if extra_markers:
        result += marker(0x53, b"\0\0"+coding[5:]) + marker(0x5d, b"\0"+qcd)
        result += marker(0x5e, b"\0\0\0") + marker(0x64, b"\0\1independent fixture")

    index = 0
    for ty in range(y0, height+y0, tile[1]):
        for tx in range(x0, width+x0, tile[0]):
            tw, th = min(tile[0], width+x0-tx), min(tile[1], height+y0-ty)
            components = [[[planes[c][(y-y0)*width+x-x0] - (1 << (bits-1))
                            for x in range(tx, tx+tw)] for y in range(ty, ty+th)] for c in range(count)]
            if mct:
                for y in range(th):
                    for x in range(tw):
                        r, g, b = [p[y][x] for p in components[:3]]
                        if lossy:
                            values = (.299*r+.587*g+.114*b, -.16875*r-.33126*g+.5*b,
                                      .5*r-.41869*g-.08131*b)
                        else:
                            values = ((r+2*g+b)//4, b-g, r-g)
                        for p, value in zip(components, values):
                            p[y][x] = value
            bands = [transform(p, tx, ty, levels, not lossy) for p in components]
            if lossy:
                bands = [[[[math.trunc(v / (2**(bits+(0 if index == 0 else (1,1,2)[(index-1)%3])-e))) for v in row] for row in b]
                          for index, (b, e) in enumerate(zip(comp,exponents))] for comp in bands]
            for c,component_bands in enumerate(bands):
                if roi[c]:
                    for b,band in enumerate(component_bands):
                        for y,row in enumerate(band):
                            for x,value in enumerate(row):
                                if (x+2*y+b) % 3 == 0:
                                    row[x] = value << roi[c]
            descriptions, keys = {}, []
            for resolution in range(levels+1):
                scale = 1 << (levels-resolution)
                rx0,ry0,rx1,ry1 = [(v+scale-1)//scale for v in (tx,ty,tx+tw,ty+th)]
                if rx0 == rx1 or ry0 == ry1:
                    continue
                ppx,ppy = precinct_exponents[resolution]
                pw,ph = 1 << ppx,1 << ppy
                cells = [(px,py) for py in range(ry0//ph,(ry1+ph-1)//ph)
                         for px in range(rx0//pw,(rx1+pw-1)//pw)]
                first_band = 0 if resolution == 0 else 1+3*(resolution-1)
                orientations = [0] if resolution == 0 else [1,2,3]
                for p,(px,py) in enumerate(cells):
                    bounds = (max(rx0,px*pw),max(ry0,py*ph),min(rx1,(px+1)*pw),min(ry1,(py+1)*ph))
                    x = tx if px == rx0//pw and rx0 % pw else px*pw*scale
                    y = ty if py == ry0//ph and ry0 % ph else py*ph*scale
                    for c in range(count):
                        crops,origins = [],[]
                        for b,orientation in enumerate(orientations):
                            bx0,by0,bx1,by1 = subband_bounds((rx0,ry0,rx1,ry1),orientation)
                            sx,sy,ex,ey = subband_bounds(bounds,orientation)
                            band = bands[c][first_band+b]
                            crops.append([row[sx-bx0:ex-bx0] for row in band[sy-by0:ey-by0]])
                            origins.append((sx,sy))
                        reduction = int(resolution != 0)
                        block_size = (1 << min(block_exp,ppx-reduction),1 << min(block_exp,ppy-reduction))
                        maxima = [e+1+roi[c] for e in exponents[first_band:first_band+len(orientations)]]
                        descriptions[(c,resolution,p)] = (crops,origins,block_size,maxima)
                        keys += [(layer,resolution,c,p,x,y) for layer in range(layers)]
            ordered = progression_keys(keys,progression,late_poc or poc)
            if late_poc:
                groups = [ordered[:1],ordered[1:]]
            else:
                groups = [[key] for key in ordered] if tile_parts else [ordered]
            sequence_number = 0
            for part,group in enumerate(groups):
                body,headers = b"",[]
                for sequence,key in enumerate(group):
                    layer,resolution,c,precinct,_,_ = key
                    crops,origins,block_size,maxima = descriptions[(c,resolution,precinct)]
                    header,data = (packet(crops,maxima,origins,block_size,modes,markers,layer,roi[c])
                                   if layer == layers-1 else (b"\0"+(b"\xff\x92" if markers else b""),b""))
                    headers.append(header)
                    if markers:
                        body += marker(0x91,struct.pack(">H",sequence_number % 65536))
                    body += (b"" if packed else header)+data
                    sequence_number += 1
                tile_header = marker(0x5f,encode_poc(late_poc)) if late_poc and part == 0 else b""
                if packed == "ppt":
                    tile_header += packed_markers(0x61,headers,packed_split)
                encoded_parts.append(marker(0x90,struct.pack(">HIBB",index,14+len(tile_header)+len(body),part,len(groups)))+tile_header+b"\xff\x93"+body)
                part_headers.append(headers)
            index += 1
    if packed == "ppm":
        result += ppm_markers(part_headers,packed_split)
    return result + b"".join(encoded_parts) + b"\xff\xd9"


def pass_count_bits(header,passes):
    if passes == 1:
        header.number(0)
    elif passes == 2:
        header.number(2,2)
    elif passes <= 5:
        header.number(12+passes-3,4)
    elif passes <= 36:
        header.number(15,4)
        header.number(passes-6,5)
    else:
        header.number(511,9)
        header.number(passes-37,7)


def layered_codestream(values,modes,max_passes=None):
    """Split one code-block's codewords and passes across two real contributions."""
    source = [[v-128 for v in values[y*8:y*8+8]] for y in range(8)]
    planes,pieces = encode_block(source,0,modes,max_passes=max_passes)
    cutoff,done = 11,0
    first,second = [],[]
    for n,data in pieces:
        if done+n <= cutoff:
            first.append((n,data))
        elif done >= cutoff:
            second.append((n,data))
        else:
            # A nonterminated segment is assembled before full-quality decoding.
            split = len(data)//2
            first.append((cutoff-done,data[:split]))
            second.append((done+n-cutoff,data[split:]))
        done += n
    lblock,payload = 3,b""
    for layer,group in enumerate((first,second)):
        header = PacketBits()
        header.number(1)
        header.number(1)
        if layer == 0:
            header.number(0,9-planes)
            header.number(1)
        pass_count_bits(header,sum(n for n,_ in group))
        extra = max([0]+[len(data).bit_length()-lblock-(n.bit_length()-1) for n,data in group])
        header.number((1 << extra)-1,extra)
        header.number(0)
        lblock += extra
        for n,data in group:
            header.number(len(data),lblock+n.bit_length()-1)
        payload += header.finish()+b"".join(data for _,data in group)
    prefix = codestream([values],8,8,modes=modes,layers=2)
    prefix = prefix[:prefix.index(b"\xff\x90")]
    return prefix+marker(0x90,struct.pack(">HIBB",0,14+len(payload),0,1))+b"\xff\x93"+payload+b"\xff\xd9"


def tile_header_marker(stream, code, payload):
    start = stream.index(b"\xff\x90")
    length = struct.unpack_from(">I",stream,start+6)[0]
    addition = marker(code,payload)
    return (stream[:start+6]+struct.pack(">I",length+len(addition))+stream[start+10:start+12]
            +addition+stream[start+12:])


def box(kind, body, extended=False):
    return (struct.pack(">I4sQ", 1, kind, len(body)+16) if extended else
            struct.pack(">I4s", len(body)+8, kind)) + body


def icc_profile():
    # A project-authored monochrome ICC v2 input profile: D50 white, unit TRC.
    illuminant = tuple(round(value*65536) for value in (.9642, 1, .8249))
    description = b"Linear grayscale\0"
    tags = [
        (b"desc", b"desc"+bytes(4)+struct.pack(">I",len(description))+description+bytes(4+4+2+1+67)),
        (b"cprt", b"text"+bytes(4)+b"Public domain\0"),
        (b"wtpt", b"XYZ "+bytes(4)+struct.pack(">3i",*illuminant)),
        (b"kTRC", b"curv"+bytes(4)+struct.pack(">IH",1,256)),
    ]
    table, payload = b"", b""
    start = 128+4+len(tags)*12
    for signature, value in tags:
        table += signature+struct.pack(">II",start+len(payload),len(value))
        payload += value+bytes((-len(value)) % 4)
    result = bytearray(128)
    struct.pack_into(">I",result,0,start+len(payload))
    result[8:12] = bytes([2,0x10,0,0])
    result[12:24] = b"scnrGRAYXYZ "
    struct.pack_into(">6H",result,24,2026,10,17,0,0,0)
    result[36:40] = b"acsp"
    struct.pack_into(">3i",result,68,*illuminant)
    result[80:84] = b"pgra"
    return bytes(result)+struct.pack(">I",len(tags))+table+payload


def jp2(stream, width, height, components, *, bits=8, method=1, extra=b"", extended=False):
    color = (struct.pack(">I", 17 if components == 1 else 16) if method == 1 else icc_profile())
    header = box(b"ihdr", struct.pack(">IIH4B", height, width, components, bits-1, 7, 0, 0))
    header += box(b"colr", bytes([method, 0, 0])+color)+extra
    return box(b"jP  ", b"\r\n\x87\n") + box(b"ftyp", b"jp2 \0\0\0\0jp2 ") + box(b"jp2h", header) + box(b"jp2c", stream, extended)


def vector(name, data, planes, width, height, requirements=("jpx-part1",), tolerance=0, color="unspecified", alpha=None, bits=8, signed=False):
    # A simple text oracle keeps dimensions, precision and planar samples explicit.
    text = f"{width} {height} {len(planes)} {bits} {int(signed)} {tolerance}\n"
    text += "".join(" ".join(map(str, plane))+"\n" for plane in planes)
    return Vector(name, {".jpx": data, ".samples": text.encode()}, requirements,
                  "Independently encoded input pixels", extra={"color": color, "alpha": alpha})


def coefficient_stream(bands, width, height, *, lossy=False, exponents=None, modes=0, empty_passes=False, max_passes=None, roi=0):
    """Encode explicitly chosen coefficients, without invoking the forward DWT.

    This separates E.1/F.3 known-answer expectations from the transform encoder.
    Supports no decomposition or a single decomposition, one block per band.
    """
    levels = int(len(bands) == 4)
    exponents = exponents or [8]+[9,9,10]*levels
    base = codestream([[128]*(width*height)],width,height,levels=levels,lossy=lossy,modes=modes)
    qstart = base.index(b"\xff\x5c")
    quant = (b"\x42"+b"".join(struct.pack(">H",e << 11) for e in exponents) if lossy
             else b"\x40"+bytes(e << 3 for e in exponents))
    result = base[:qstart]+marker(0x5c,quant)
    if roi:
        result += marker(0x5e,bytes([0,0,roi]))
    body = b""
    for r in range(levels+1):
        selected = bands[:1] if r == 0 else bands[1:]
        eps = exponents[:1] if r == 0 else exponents[1:]
        header,data = packet(selected,[e+1+roi for e in eps],[(0,0)]*len(selected),modes=modes,empty_passes=empty_passes,max_passes=max_passes,roi=roi)
        body += header+data
    return result+marker(0x90,struct.pack(">HIBB",0,14+len(body),0,1))+b"\xff\x93"+body+b"\xff\xd9"


def rejection(name, data, requirements, rule):
    return Vector(name,{".jpx":data,".error":b"syntax\n"},requirements,rule,extra={"error":"syntax"})


def cases():
    yield from marker_cases()
    width, height = 8, 8
    gray = [(x*37+y*13) % 256 for y in range(height) for x in range(width)]
    rgb = [gray, [(v+71) % 256 for v in gray], [255-v for v in gray]]
    yield vector("gray", codestream([gray],width,height), [gray],width,height)
    yield vector("zero", codestream([[128]*64],8,8), [[128]*64],8,8)
    for levels in (1,2,3):
        yield vector(f"reversible-{levels}",codestream([gray],width,height,levels=levels),[gray],width,height, ("jpx-wavelet",))
    for progression in range(5):
        yield vector(f"rct-progression-{progression}",codestream(rgb,width,height,levels=2,mct=True,progression=progression),rgb,width,height,("jpx-progression", "jpx-rct"))
    yield vector("tiles",codestream(rgb,width,height,tile=(4,4),levels=1,mct=True),rgb,width,height,("jpx-tiles",))
    yield vector("tile-parts",codestream(rgb,width,height,levels=2,mct=True,tile_parts=True),rgb,width,height,("jpx-tiles",))
    yield vector("layers",codestream(rgb,8,8,levels=2,mct=True,layers=3,tile_parts=True),rgb,8,8,("jpx-progression",))
    yield vector("offset",codestream([gray],width,height,levels=2,origin=(3,5)),[gray],width,height,("jpx-wavelet",))
    odd = [(x*9+y*17) % 256 for y in range(7) for x in range(9)]
    yield vector("odd",codestream([odd],9,7,levels=2),[odd],9,7,("jpx-wavelet",))
    yield vector("sixteen-bit",codestream([[v*257 for v in gray]],8,8,bits=16),[[v*257 for v in gray]],8,8,bits=16)
    yield vector("signed",codestream([gray],8,8,signed=True),[gray],8,8,signed=True)
    for modes in (1,2,4,8,16,32,34,3,5,9,17,6,12,20,24,36,63):
        yield vector(f"modes-{modes}",codestream([gray],8,8,modes=modes),[gray],8,8,("jpx-part1","jpx-codeblock-modes"))
    yield vector("packet-markers",codestream([gray],8,8,levels=2,markers=True),[gray],8,8)
    yield vector("code-blocks",codestream([odd],9,7,levels=1,block_exp=2),[odd],9,7,("jpx-part1",))
    yield vector("markers",codestream([gray],8,8,extra_markers=True),[gray],8,8)
    yield vector("irreversible",codestream(rgb,8,8,levels=2,mct=True,lossy=True),rgb,8,8,("jpx-irreversible",),tolerance=3)
    yield vector("derived-quantization",codestream(rgb,8,8,levels=2,mct=True,lossy=True,derived_quant=True),rgb,8,8,("jpx-irreversible",),tolerance=3)
    for w,h,origin in [(1,7,(0,0)),(7,1,(0,0)),(1,7,(1,0)),(7,1,(0,1)),(1,1,(1,1))]:
        values = [100+13*i for i in range(w*h)]
        for lossy in [False,True]:
            yield vector(f"singleton-{w}-{h}-{origin[0]}-{origin[1]}-{int(lossy)}",
                         codestream([values],w,h,levels=1,origin=origin,lossy=lossy),[values],w,h,
                         ("jpx-wavelet",),tolerance=1 if lossy else 0)
    for order in range(5):
        yield vector(f"precincts-{order}",codestream(rgb,8,8,levels=2,mct=True,precincts=(2,2),progression=order,layers=3),rgb,8,8,("jpx-progression","jpx-precincts"))
    yield vector("precincts-offset",codestream([odd],9,7,levels=2,origin=(3,5),precincts=[(1,2),(2,1),(2,3)]),[odd],9,7,("jpx-progression","jpx-precincts"))
    yield vector("precincts-tiles",codestream(rgb,8,8,levels=1,tile=(4,4),mct=True,precincts=(1,1)),rgb,8,8,("jpx-progression","jpx-tiles","jpx-precincts"))
    changes = [(0,0,1,1,3,4),(1,0,1,3,3,2)]
    yield vector("poc",codestream(rgb,8,8,levels=2,mct=True,precincts=(2,2),poc=changes),rgb,8,8,("jpx-progression","jpx-poc"))
    yield vector("poc-overlap",codestream(rgb,8,8,levels=2,mct=True,precincts=(2,2),layers=3,poc=[(0,0,1,3,3,4),(0,0,3,3,3,2)]),rgb,8,8,("jpx-progression","jpx-poc"))
    yield vector("poc-tile-part",codestream(rgb,8,8,levels=2,mct=True,precincts=(2,2),late_poc=[(0,0,1,3,3,4)]),rgb,8,8,("jpx-progression","jpx-poc"))
    for shift in (4,12):
        # Shift 4 uses small-amplitude data so background coefficients fit below 2**shift.
        source = [[128+(v % 7)-3 for v in gray]] if shift == 4 else rgb
        yield vector(f"roi-{shift}",codestream(source,8,8,levels=1,roi=shift,precincts=(2,2)),source,8,8,("jpx-part1","jpx-roi"))
    for method in ("ppm","ppt"):
        yield vector(f"packed-{method}",codestream(rgb,8,8,levels=2,mct=True,precincts=(2,2),packed=method,packed_split=7,tile_parts=True,markers=True),rgb,8,8,("jpx-progression","jpx-packed-headers"))
    yield vector("combined-lossy",codestream(rgb,8,8,levels=2,mct=True,lossy=True,precincts=(2,2),packed="ppt",packed_split=7,modes=63),rgb,8,8,("jpx-irreversible","jpx-progression","jpx-codeblock-modes","jpx-packed-headers","jpx-precincts"),tolerance=3)
    for mode in (0,1,4,63):
        yield vector(f"layer-contributions-{mode}",layered_codestream(gray,mode),[gray],8,8,("jpx-part1","jpx-codeblock-modes"))
    for method in ("ppm","ppt"):
        yield vector(f"packed-zero-{method}",codestream([[128]*64],8,8,levels=2,precincts=(1,1),packed=method,packed_split=7,tile_parts=True,markers=True),[[128]*64],8,8,("jpx-packed-headers",))
    yield vector("precincts-default-multiple",codestream([[i%256 for i in range(32769)]],32769,1),[[i%256 for i in range(32769)]],32769,1,("jpx-precincts",))
    yield vector("combined-reversible",codestream([odd],9,7,levels=2,origin=(3,5),precincts=[(1,1),(2,2),(2,3)],modes=63,roi=12,packed="ppm",packed_split=13,late_poc=[(0,0,1,3,1,3)]),[odd],9,7,("jpx-codeblock-modes","jpx-precincts","jpx-packed-headers","jpx-roi","jpx-poc"))
    yield vector("spatial-offset-boundary",codestream([[30,70,110]],1,3,levels=1,origin=(0,1),precincts=[(0,0),(1,1)],progression=3),[[30,70,110]],1,3,("jpx-precincts","jpx-progression"))
    for order in range(5):
        yield vector(f"spatial-offset-{order}",codestream([odd],9,7,levels=2,origin=(3,5),precincts=[(1,1),(2,2),(2,3)],progression=order),[odd],9,7,("jpx-precincts","jpx-progression"))
    yield vector("poc-forward-declaration",codestream([[30,70,110]],1,3,levels=1,origin=(0,1),precincts=[(0,0),(1,1)],late_poc=[(0,0,1,2,1,3)]),[[30,70,110]],1,3,("jpx-poc","jpx-precincts"))
    yield Vector("roi-precision-limit",{".jpx":codestream([[128]*64],8,8,roi=37),".error":b"unsupported\n"},("jpx-roi","jpx-part1"),"ROI coding precision beyond the 30-bit coefficient representation is explicitly unsupported",extra={"error":"unsupported"})
    raw = codestream([gray],8,8)
    packet_size = len(raw)-raw.index(b"\xff\x93")-4
    variable_length = [packet_size & 127]
    packet_size >>= 7
    while packet_size:
        variable_length.insert(0,128 | (packet_size & 127))
        packet_size >>= 7
    yield vector("packet-lengths",tile_header_marker(raw,0x58,b"\0"+bytes(variable_length)),[gray],8,8)
    yield vector("tile-coding",tile_header_marker(raw,0x52,bytes([0,0,0,1,0,0,4,4,0,1])),[gray],8,8,("jpx-tiles",))
    final_part = bytearray(raw)
    sot = final_part.index(b"\xff\x90")
    final_part[sot+6:sot+10] = bytes(4)
    yield vector("final-tile-part",bytes(final_part),[gray],8,8,("jpx-tiles",))
    wrapped = jp2(raw,8,8,1)
    jp2c = wrapped.index(b"jp2c")-4
    yield vector("final-box",wrapped[:jp2c]+box(b"free",b"metadata")+bytes(4)+b"jp2c"+raw,[gray],8,8,("jpx-boxes",),color="gray")
    yield vector("bit-depth-box",jp2(raw,8,8,1,bits=256,extra=box(b"bpcc",bytes([7]))),[gray],8,8,("jpx-boxes",),color="gray")
    for method in (1,2,3):
        yield vector(f"color-{method}",jp2(raw,8,8,1,method=method),[gray],8,8,("jpx-boxes",),color="gray" if method == 1 else "icc")
    res = box(b"res ",box(b"resc",struct.pack(">4H2b",72,1,72,1,0,0))+box(b"resd",struct.pack(">4H2b",96,1,96,1,0,0)))
    yield vector("resolution-extended",jp2(raw,8,8,1,extra=res,extended=True),[gray],8,8,("jpx-boxes",),color="gray")
    indices = [i % 4 for i in range(64)]
    palette = [(5,10,15),(255,0,1),(0,255,2),(40,50,60)]
    pclr = struct.pack(">HB",4,3)+bytes([7]*3)+bytes(v for row in palette for v in row)
    cmap = b"".join(struct.pack(">HBB",0,1,c) for c in range(3))
    pal_stream = jp2(codestream([indices],8,8),8,8,1,extra=box(b"pclr",pclr)+box(b"cmap",cmap))
    # The palette expands the single codestream component to RGB.
    pal_stream = pal_stream.replace(struct.pack(">I",17),struct.pack(">I",16),1)
    yield vector("palette",pal_stream,[[palette[i][c] for i in indices] for c in range(3)],8,8,("jpx-palette",),color="rgb")
    alpha = [(i*5) % 256 for i in range(64)]
    definitions = struct.pack(">H",4)+b"".join(struct.pack(">HHH",*v) for v in ((0,1,0),(1,0,3),(2,0,1),(3,0,2)))
    yield vector("channel-alpha",jp2(codestream([alpha,rgb[2],rgb[0],rgb[1]],8,8),8,8,4,extra=box(b"cdef",definitions)),rgb+[alpha],8,8,("jpx-boxes",),color="rgb",alpha=3)
    # PDF 7.4.9 names the additional enumerations; their Part 2 definitions need T.801.
    forbidden_color = jp2(codestream(rgb,8,8),8,8,3).replace(struct.pack(">I",16),struct.pack(">I",19),1)
    yield Vector("pdf-colour-19",{".jpx":forbidden_color,".error":b"syntax\n"},("jpx-boxes","jpx-pdf-contract"),"PDF 7.4.9 excludes enumerated CIEJab 19",extra={"error":"syntax"})
    cmyk = [gray,gray,gray,gray]
    cmyk_data = jp2(codestream(cmyk,8,8),8,8,4).replace(struct.pack(">I",16),struct.pack(">I",12),1)
    yield vector("pdf-colour-12",cmyk_data,cmyk,8,8,("jpx-boxes","jpx-pdf-contract"),color="cmyk")
    yield Vector("pdf-depth-17",{".jpx":codestream([[65536]*64],8,8,bits=17),".error":b"unsupported\n"},("jpx-part1","jpx-pdf-contract"),"PDF allows 17-bit samples; the current unsigned 16-bit sample API reports Unsupported",extra={"error":"unsupported"})
    padded = bytearray(codestream([[128]],1,1))
    padded[padded.index(b"\xff\x93")+2] = 127
    yield vector("packet-unused-bits",bytes(padded),[[128]],1,1,("jpx-progression",))
    # Table A.32 includes the CEpoc zero alias for 256 in the wide field too.
    wide = codestream([[128] for _ in range(257)],1,1)
    stop = wide.index(b"\xff\x90")
    volumes = struct.pack(">BHHBHB",0,0,1,1,0,0)+struct.pack(">BHHBHB",0,256,1,1,257,0)
    yield vector("poc-wide-zero-end",wide[:stop]+marker(0x5f,volumes)+wide[stop:],[[128] for _ in range(257)],1,1,("jpx-poc",))
    # D.3.5 Table D.6 gives this column directly, without any wavelet transform.
    column = [[10],[1],[3],[-7]]
    yield vector("model-column",coefficient_stream([column],1,4),[[138,129,131,121]],1,4,("jpx-part1",))
    # F.3.8.1: a 2x2 block with LL=0, HL=4, LH=HH=0 synthesizes [-2,2] on each row.
    chosen = [[[0]],[[4]],[[0]],[[0]]]
    yield vector("lifting-known-answer",coefficient_stream(chosen,2,2),[[126,130,126,130]],2,2,("jpx-wavelet",))
    # E-3/E-4 and F-7: HL=8.5, Delta=2; scaling of the high branch is 1/K.
    # For a two-sample line, F-7 reduces to opposite values +/-HL/2.
    # Thus the reconstructed rows are [119.5,136.5], rounded by the documented policy.
    yield vector("quant-gain-known-answer",coefficient_stream([[[0]],[[8]],[[0]],[[0]]],2,2,lossy=True,exponents=[8]*4),[[120,137,120,137]],2,2,("jpx-irreversible","jpx-wavelet"),tolerance=1)
    yield vector("derived-three-levels",codestream([gray],8,8,levels=3,lossy=True,derived_quant=True),[gray],8,8,("jpx-irreversible",),tolerance=3)
    yield vector("roi-bypass",codestream([[128+(i%7)-3 for i in range(64)]],8,8,modes=17,roi=12),[[128+(i%7)-3 for i in range(64)]],8,8,("jpx-roi","jpx-codeblock-modes","jpx-termination"))
    # A hand calculation of two decisions (contexts 0 then 9) using C.2 and
    # D.4.2 yields one zero byte; this assertion is independent of any decoder.
    assert encode_block([[1]],0,16) == (1,[(1,b"\0")])
    term = coefficient_stream([[[1]]],1,1,modes=16)
    yield vector("ordinary-empty-segments",coefficient_stream([[[63]]],1,1,modes=4,empty_passes=True),[[191]],1,1,("jpx-termination",))
    yield vector("termination-known-answer",term,[[129]],1,1,("jpx-termination",))
    # D.4.2 defines the exact tail when predictable termination is selected.
    broken = bytearray(term)
    broken[-3] ^= 1
    yield rejection("termination-mq-tail",bytes(broken),("jpx-termination",),"D.4.2 predictable MQ tail differs by one bit")
    # D.6 raw padding: the first raw refinement for this singleton is one bit,
    # followed by alternating padding. Corrupt a padding bit, leaving data alone.
    raw_value, raw_modes = 63, 17
    raw_term = coefficient_stream([[[raw_value]]],1,1,modes=raw_modes)
    _,parts = encode_block([[raw_value]],0,raw_modes)
    offset = raw_term.index(b"\xff\x93")+2
    hdr,_ = packet([[[raw_value]]],[9],[(0,0)],modes=raw_modes)
    offset += len(hdr)+len(parts[0][1])
    changed = bytearray(raw_term)
    changed[offset] ^= 1
    yield rejection("termination-raw-padding",bytes(changed),("jpx-termination",),"D.6 raw predictable padding starts at zero and alternates")
    # I.5.3.6: cdef identifies stored premultiplied values; no division is part of decoding.
    premultiplied = [[i//2 for i in plane] for plane in rgb]
    predef = struct.pack(">H",4)+b"".join(struct.pack(">HHH",*v) for v in ((0,2,0),(1,0,3),(2,0,1),(3,0,2)))
    yield vector("channel-premultiplied",jp2(codestream([alpha,premultiplied[2],premultiplied[0],premultiplied[1]],8,8),8,8,4,extra=box(b"cdef",predef)),premultiplied+[alpha],8,8,("jpx-options","jpx-boxes"),alpha=3)
    # Opacity provenance refers to the original cmap channel and palette column.
    opacity_palette = [(v[0],i*70) for i,v in enumerate(palette)]
    pbox = box(b"pclr",struct.pack(">HB",4,2)+b"\x07\x07"+bytes(v for row in opacity_palette for v in row))
    mbox = box(b"cmap",struct.pack(">HBBHBB",0,1,1,0,1,0))
    cbox = box(b"cdef",struct.pack(">H6H",2,0,1,0,1,0,1))
    yield vector("palette-opacity-origin",jp2(codestream([indices],8,8),8,8,1,extra=pbox+mbox+cbox),[[opacity_palette[i][c] for i in indices] for c in range(2)],8,8,("jpx-options","jpx-palette"),alpha=1)
    # I.3.3: ordinary JP2 readers use only the first colr specification.
    yield vector("colour-first-method",jp2(raw,8,8,1,extra=box(b"colr",b"\x01\0\0"+struct.pack(">I",16))),[gray],8,8,("jpx-boxes",))
    unusual = bytearray(jp2(raw,8,8,1))
    colr = unusual.index(b"colr")+4
    unusual[colr+1:colr+3] = b"\x80\x7f"
    yield vector("colour-ignored-fields",bytes(unusual),[gray],8,8,("jpx-boxes",))
    yield vector("additional-codestream",wrapped+box(b"jp2c",codestream([[0]*64],8,8)),[gray],8,8,("jpx-boxes",))
    # A.1.3 allows COC/QCC before their defaults. Values are identical here.
    cod_start,qcd_start,sot = (raw.index(bytes([255,m])) for m in (0x52,0x5c,0x90))
    coc = marker(0x53,b"\0\0"+raw[cod_start+9:qcd_start])
    qcc = marker(0x5d,b"\0"+raw[qcd_start+4:sot])
    yield vector("marker-order",raw[:cod_start]+coc+qcc+raw[cod_start:],[gray],8,8,("jpx-marker-syntax",))
    yield vector("reserved-markers",raw[:cod_start]+b"\xff\x30\xff\x3f"+raw[cod_start:],[gray],8,8,("jpx-marker-syntax",))
    optional_sop = bytearray(raw)
    optional_sop[cod_start+4] |= 2
    yield vector("optional-sop",bytes(optional_sop),[gray],8,8,("jpx-marker-syntax",))
    # B.2/G.4: combine standalone component packets on different grids.
    steps,ref_origin,extent = [(1,1),(2,3),(3,2)],(3,5),(11,12)
    streams, placements = [],[]
    for c,(dx,dy) in enumerate(steps):
        cx0,cy0 = (ref_origin[0]+dx-1)//dx,(ref_origin[1]+dy-1)//dy
        cx1,cy1 = (extent[0]+dx-1)//dx,(extent[1]+dy-1)//dy
        w,h = cx1-cx0,cy1-cy0
        values = [(20*c+17*i)%256 for i in range(w*h)]
        stream = codestream([values],w,h,origin=(cx0,cy0))
        streams.append(stream[stream.index(b"\xff\x93")+2:-2])
        placements.append((dx,dy,cx0*dx,cy0*dy,w,h,values))
    siz = struct.pack(">H8IH",0,*extent,*ref_origin,8,7,*ref_origin,3)+b"".join(bytes([7,dx,dy]) for dx,dy in steps)
    header = b"\xff\x4f"+marker(0x51,siz)+raw[cod_start:sot]
    payload = b"".join(streams)
    sampling = header+marker(0x90,struct.pack(">HIBB",0,14+len(payload),0,1))+b"\xff\x93"+payload+b"\xff\xd9"
    placement = "8 7 3 5 3\n"+"".join(" ".join(map(str,(*item[:6],*item[6])))+"\n" for item in placements)
    yield Vector("component-placement",{".jpx":sampling,".placement":placement.encode()},("jpx-subsampling",),"B-1/B-2 locate samples at integer multiples of each component's step")
    # A.6.6/B.12.3: moving the sole tile POC after its first packet is invalid.
    deferred = bytearray(codestream([gray],8,8,levels=1,late_poc=[(0,0,1,2,1,3)]))
    first_sot,po = deferred.index(b"\xff\x90"),deferred.index(b"\xff\x5f")
    po_size = 2+struct.unpack_from(">H",deferred,po+2)[0]
    po_bytes = deferred[po:po+po_size]
    del deferred[po:po+po_size]
    struct.pack_into(">I",deferred,first_sot+6,struct.unpack_from(">I",deferred,first_sot+6)[0]-po_size)
    second_sot = deferred.index(b"\xff\x90",first_sot+2)
    struct.pack_into(">I",deferred,second_sot+6,struct.unpack_from(">I",deferred,second_sot+6)[0]+po_size)
    deferred[second_sot+12:second_sot+12] = po_bytes
    yield rejection("poc-deferred-declaration",bytes(deferred),("jpx-poc",),"A.6.6/B.12.3 require a tile POC in its first tile-part")
    # A.7.4 reassembles by increasing index, independently of physical marker order.
    ordered_ppm = codestream([gray],8,8,levels=1,packed="ppm",packed_split=7)
    start,stop = ordered_ppm.index(b"\xff\x60"),ordered_ppm.index(b"\xff\x90")
    pos,fragments = start,[]
    while pos < stop:
        size = 2+struct.unpack_from(">H",ordered_ppm,pos+2)[0]
        fragments.append(ordered_ppm[pos:pos+size])
        pos += size
    yield vector("packed-index-order",ordered_ppm[:start]+b"".join(reversed(fragments))+ordered_ppm[stop:],[gray],8,8,("jpx-packed-headers",))
    nested_zero = jp2(raw,8,8,1,extra=bytes(4)+b"free"+b"metadata")
    yield rejection("nested-zero-box",nested_zero,("jpx-boxes",),"I.4 permits a zero-length child only in a zero-length parent")
    mismatch = bytearray(raw)
    mismatch[mismatch.index(b"\xff\x52")+13] = 0
    yield rejection("quant-transform-mismatch",bytes(mismatch),("jpx-marker-syntax",),"A.6.4 quantization representation must match the wavelet transform")
    # B.12.3: keep the original first-resolution prefix. With zero high
    # bands, F.3.8.1 reduces to exact integer linear interpolation of LL:
    # even coordinates keep LL; odd coordinates average adjacent LL samples.
    partial = codestream([gray],8,8,levels=1,tile_parts=True)
    first = partial.index(b"\xff\x90")
    end = first+struct.unpack_from(">I",partial,first+6)[0]
    partial = bytearray(partial[:end]+b"\xff\xd9")
    partial[first+11] = 0
    ll = transform([[v-128 for v in gray[y*8:y*8+8]] for y in range(8)],0,0,1,True)[0]
    horizontal = [[row[x//2] if x%2 == 0 else (row[x//2]+row[min(x//2+1,3)])//2
                   for x in range(8)] for row in ll]
    expected = [max(0,min(255,128+(horizontal[y//2][x] if y%2 == 0 else
                (horizontal[y//2][x]+horizontal[min(y//2+1,3)][x])//2)))
                for y in range(8) for x in range(8)]
    yield vector("incomplete-progression",bytes(partial),[expected],8,8,
                 ("jpx-progression","jpx-partial-quality","jpx-wavelet"))
    # A minimal independent F.3 oracle: LL=10 and zero details give four tens.
    partial = coefficient_stream([[[10]],[[0]],[[0]],[[0]]],2,2)
    first = partial.index(b"\xff\x90")
    low_header,low_body = packet([[[10]]],[9],[(0,0)])
    payload = low_header+low_body
    partial = partial[:first]+marker(0x90,struct.pack(">HIBB",0,14+len(payload),0,1))+b"\xff\x93"+payload+b"\xff\xd9"
    yield vector("partial-resolution",partial,[[138]*4],2,2,
                 ("jpx-progression","jpx-partial-quality","jpx-wavelet"))
    # D.3/Table D.6, E-7/E-8 with r=1/2. These prefix values are specified
    # independently of the entropy encoder, including within-bit-plane stops.
    column = [[10],[1],[3],[-7]]
    prefixes = [(12,0,0,0),(12,0,0,0),(10,0,0,0),(10,0,0,-6),
                (10,0,3,-6),(11,0,3,-7),(11,0,3,-7),(11,1,3,-7),
                (10,1,3,-7),(10,1,3,-7)]
    for modes in (0,1,2,4,8,16,32,63):
        for passes,expected in enumerate(prefixes,1):
            stream = coefficient_stream([column],1,4,modes=modes,max_passes=passes)
            yield vector(f"partial-column-{modes}-{passes}",stream,
                         [[128+v for v in expected]],1,4,
                         ("jpx-partial-quality","jpx-codeblock-modes"))
    # Once all coefficients are significant, only refinement changes Nb.
    # Zero low bits make the E-8 midpoint a closed-form oracle in bypass planes.
    expanded = [[160],[16],[48],[-112]]
    for modes in (1,17,63):
        for passes in (11,12,14,15,17,18,20,21):
            missing = 7-passes//3
            offset = (1 << (missing-1)) if missing else 0
            expected = [max(0,min(255,128+(abs(v)+offset)*(1 if v>0 else -1))) for row in expanded for v in row]
            stream = coefficient_stream([expanded],1,4,modes=modes,max_passes=passes)
            yield vector(f"partial-bypass-{modes}-{passes}",stream,[expected],1,4,
                         ("jpx-partial-quality","jpx-codeblock-modes","jpx-termination"))
    # A nonempty raw significance pass ends before its paired refinement:
    # the newly significant unit is exact; the older samples retain midpoints.
    raw_prefix = coefficient_stream([[[160],[1],[48],[-112]]],1,4,modes=17,max_passes=20)
    yield vector("partial-bypass-significance",raw_prefix,[[255,129,177,15]],1,4,
                 ("jpx-partial-quality","jpx-codeblock-modes","jpx-termination"))
    # A small, smooth RGB tile has sparse entropy data but several synthesis
    # levels. Its exact constant samples also test fused irreversible scaling.
    flat = [[128]*(64*64)]*3
    yield vector("wavelet-budget",codestream(flat,64,64,levels=5,mct=True,lossy=True),
                 flat,64,64,("jpx-wavelet","jpx-hostile"))
    # Remove the final layer's packets but keep its COD declaration. The
    # earlier packet is a real contribution, rather than an empty layer.
    layer = bytearray(coefficient_stream([column],1,4,max_passes=5))
    cod = layer.index(b"\xff\x52")
    struct.pack_into(">H",layer,cod+6,2)
    yield vector("partial-layer",bytes(layer),[[138,128,131,122]],1,4,
                 ("jpx-partial-quality","jpx-progression"))
    # Two nonempty quality layers, ending after the cleanup of plane three.
    # The arithmetic codeword may continue across the layer boundary.
    prefix_samples = []
    for v in gray:
        q = v-128
        magnitude = (abs(q)//8)*8
        prefix_samples.append(max(0,min(255,128+((magnitude+4)*(1 if q>0 else -1) if magnitude else 0))))
    for modes in (0,1,4,63):
        yield vector(f"partial-layer-contributions-{modes}",layered_codestream(gray,modes,max_passes=13),
                     [prefix_samples],8,8,("jpx-partial-quality","jpx-progression","jpx-codeblock-modes"))
    # E-6, Delta=1, r=1/2: unlike E-7, full irreversible magnitudes retain
    # the half-unit offset. Round coefficients away from zero at a tie, then
    # apply the G.1.2 level shift (the decoder's documented output choice).
    for passes,expected in enumerate(prefixes,1):
        values = [128+(v if passes < 9 or v == 0 else v+(1 if v>0 else -1)) for v in expected]
        if passes == 8:
            values[1] = 130  # First significance at the last plane: 1 + 1/2.
        stream = coefficient_stream([column],1,4,lossy=True,max_passes=passes)
        yield vector(f"partial-irreversible-{passes}",stream,[values],1,4,
                     ("jpx-partial-quality","jpx-irreversible"))
    # H.1: the three ROI coefficients have four extra coding planes, while
    # the background coefficient is not shifted. The initial prefixes agree
    # with the same column; background significance arrives in the last plane.
    shifted = [[160],[1],[48],[-112]]
    for passes,expected in enumerate(prefixes[:9],1):
        stream = coefficient_stream([shifted],1,4,roi=4,max_passes=passes)
        values = [128+v for v in expected]
        values[1] = 128
        yield vector(f"partial-roi-{passes}",stream,[values],1,4,
                     ("jpx-partial-quality","jpx-roi"))
    # H.1 background realignment retains one missing bit until refinement;
    # ROI coefficients already have all their original bits at these stops.
    for passes in (17,18,19,20,21):
        stream = coefficient_stream([[[160],[2],[48],[-112]]],1,4,roi=4,max_passes=passes)
        yield vector(f"partial-roi-background-{passes}",stream,
                     [[138,131 if passes < 21 else 130,131,121]],1,4,
                     ("jpx-partial-quality","jpx-roi"))
    bad_count = bytearray(codestream([gray],8,8,levels=1,tile_parts=True))
    pos = bad_count.index(b"\xff\x90")
    while bad_count[pos:pos+2] == b"\xff\x90":
        bad_count[pos+11] = 1  # Two actual parts, a declared count of one.
        pos += struct.unpack_from(">I",bad_count,pos+6)[0]
    yield rejection("tile-part-count",bytes(bad_count),("jpx-tiles","jpx-marker-syntax"),
                    "A.4.2 counts tile-parts, rather than recording the last index")
    # A.7.5 permits PPT to precede its data in an earlier tile-part; report the
    # unsupported organization explicitly, rather than calling valid data malformed.
    early = bytearray(codestream([gray],8,8,levels=1,packed="ppt",tile_parts=True))
    first,second = early.index(b"\xff\x90"),early.index(b"\xff\x90",early.index(b"\xff\x90")+2)
    first_ppt,second_ppt = early.index(b"\xff\x61"),early.index(b"\xff\x61",early.index(b"\xff\x61")+2)
    size = 2+struct.unpack_from(">H",early,second_ppt+2)[0]
    moved = bytearray(early[second_ppt:second_ppt+size]); moved[4] = 1
    struct.pack_into(">I",early,second+6,struct.unpack_from(">I",early,second+6)[0]-size)
    del early[second_ppt:second_ppt+size]
    struct.pack_into(">I",early,first+6,struct.unpack_from(">I",early,first+6)[0]+size)
    early[first_ppt:first_ppt] = moved
    yield Vector("ppt-early-headers",{".jpx":bytes(early),".error":b"unsupported\n"},("jpx-packed-headers",),"A.7.5 permits earlier-part headers; this organization is explicitly unsupported",extra={"error":"unsupported"})
    # Inline headers cannot be mixed with PPT headers within one tile.
    mixed = bytearray(codestream([gray],8,8,levels=1,packed="ppt",tile_parts=True))
    first = mixed.index(b"\xff\x90")
    head = mixed.index(b"\xff\x61")
    size = 2+struct.unpack_from(">H",mixed,head+2)[0]
    inline = mixed[head+5:head+size]
    struct.pack_into(">I",mixed,first+6,struct.unpack_from(">I",mixed,first+6)[0]-5)
    del mixed[head:head+size]
    sod = mixed.index(b"\xff\x93")+2
    mixed[sod:sod] = inline
    yield rejection("packed-mixed-parts",bytes(mixed),("jpx-packed-headers",),"A.7.4/A.7.5 forbid mixed inline and PPT headers for one tile")
    # I.5.3.4 palette sizes are bounded independently of address-space limits.
    yield rejection("palette-entry-limit",jp2(raw,8,8,1,extra=box(b"pclr",struct.pack(">HB",1025,1)+b"\x07"+bytes(1025))+box(b"cmap",b"\0\0\1\0")),("jpx-palette",),"I.5.3.4 limits palettes to 1024 rows")
    ignored_bits = bytearray(raw)
    ignored_bits[cod_start+10] |= 0x80
    ignored_bits[qcd_start+5] |= 7
    yield vector("ignored-marker-bits",bytes(ignored_bits),[gray],8,8,("jpx-marker-syntax",))
    extra_quant = bytearray(raw)
    qlen = struct.unpack_from(">H",extra_quant,qcd_start+2)[0]
    struct.pack_into(">H",extra_quant,qcd_start+2,qlen+3)
    extra_quant[qcd_start+2+qlen:qcd_start+2+qlen] = b"\x40\x40\x40"
    yield vector("quant-extra-steps",bytes(extra_quant),[gray],8,8,("jpx-marker-syntax",))
    yield rejection("tlm-empty",raw[:sot]+marker(0x55,b"\0\x40")+raw[sot:],("jpx-marker-syntax",),"A.7.1 requires a nonempty list of tile lengths")
    yield rejection("comment-registration",raw[:sot]+marker(0x64,b"\0\2x")+raw[sot:],("jpx-marker-syntax",),"A.9.2 reserves comment registrations other than zero and one")
    invalid_part = bytearray(raw)
    invalid_part[sot+10:sot+12] = b"\xff\0"
    yield rejection("tile-part-index-255",bytes(invalid_part),("jpx-tiles","jpx-marker-syntax"),"A.4.2 limits the tile-part index to 254")
    precision = bytearray(codestream(rgb,8,8,mct=True))
    precision[45] = 6
    yield rejection("transform-precision",bytes(precision),("jpx-rct",),"G.2 requires equal input precision for the first three transform components")
    spatial = bytearray(sampling)
    spatial[spatial.index(b"\xff\x52")+5] = 2
    yield rejection("spatial-sampling",bytes(spatial),("jpx-precincts","jpx-subsampling"),"B.12.1.3 requires power-of-two component separations")
    final_ff = bytearray(raw)
    final_ff[-3] = 255
    yield rejection("contribution-final-ff",bytes(final_ff),("jpx-hostile",),"B.10.7 forbids a code-block packet contribution ending in FF")
    direct = box(b"cmap",b"\0\0\0\0")
    yield rejection("mapping-without-palette",jp2(raw,8,8,1,extra=direct),("jpx-palette",),"I.5.3.4 requires palette and component mapping boxes together")
    single_palette = box(b"pclr",b"\0\1\1\7\0")
    yield vector("palette-direct-map",jp2(raw,8,8,1,extra=single_palette+direct),[gray],8,8,("jpx-palette",))
    bad_pad = box(b"pclr",b"\0\1\1\3\x80")
    yield rejection("palette-padding",jp2(raw,8,8,1,extra=bad_pad+box(b"cmap",b"\0\0\1\0")),("jpx-palette",),"I.5.3.4 requires zeros above the palette sample precision")
    yield rejection("resolution-empty",jp2(raw,8,8,1,extra=box(b"res ",b"")),("jpx-boxes",),"I.5.3.7 requires a capture or display resolution child")
    yield rejection("resolution-repeat",jp2(raw,8,8,1,extra=res+res),("jpx-boxes",),"I.5.3.7 permits at most one resolution superbox")
    signed_alpha = jp2(codestream([gray,gray],8,8,signed=True),8,8,2,bits=136,extra=box(b"cdef",struct.pack(">H6H",2,0,0,1,1,1,0)))
    yield rejection("opacity-signed",signed_alpha,("jpx-options",),"I.5.3.6 requires unsigned opacity components")
    yield Vector("grid-high-origin",{".jpx":codestream([[128]],1,1,origin=(1 << 31,0)),".error":b"unsupported\n"},("jpx-subsampling",),"A.5.1 permits 32-bit grids; the current arithmetic explicitly supports 31 bits",extra={"error":"unsupported"})
    whole = codestream(rgb,8,8,packed="ppm")
    start,stop = whole.index(b"\xff\x60"),whole.index(b"\xff\x90")
    contents = whole[start+5:stop]
    split_header = whole[:start]+marker(0x60,b"\0"+contents[:5])+marker(0x60,b"\1"+contents[5:])+whole[stop:]
    yield rejection("packed-header-split",split_header,("jpx-packed-headers",),"A.7.4 requires complete packet headers at marker boundaries")
    corrupt_tree = PacketBits()
    corrupt_tree.number(1)  # Nonempty packet.
    corrupt_tree.number(1)  # Included code-block.
    corrupt_tree.number(0, 33)  # Unterminated zero bit-plane tree.
    corrupt_packet = corrupt_tree.finish()
    bad_tree = raw[:raw.index(b"\xff\x90")]+marker(0x90,struct.pack(">HIBB",0,14+len(corrupt_packet),0,1))+b"\xff\x93"+corrupt_packet+b"\xff\xd9"
    bad = {
        "truncated": raw[:-3],
        "bad-tag-tree": bad_tree,
        "absurd-tiles": raw[:8]+struct.pack(">II",65535,2)+raw[16:24]+struct.pack(">II",1,1)+raw[32:],
        "bad-box-length": box(b"jP  ",b"\r\n\x87\n")+struct.pack(">I4s",7,b"ftyp"),
        "missing-tile": raw[:raw.index(b"\xff\x90")]+b"\xff\xd9",
        "trailing": raw+b"\0",
        "duplicate-coding": raw[:45]+marker(0x52,bytes([0,0,0,1,0,0,4,4,0,1]))+raw[45:],
        "bad-tile-lengths": raw[:raw.index(b"\xff\x90")]+marker(0x55,b"\0\x30")+raw[raw.index(b"\xff\x90"):],
        "bad-packet-lengths": tile_header_marker(raw,0x58,b"\0\x80"),
    }
    bad["poc-range"] = raw[:raw.index(b"\xff\x90")]+marker(0x5f,encode_poc([(0,0,0,1,1,0)]))+raw[raw.index(b"\xff\x90"):]
    invalid_precinct = bytearray(codestream([gray],8,8,levels=1,precincts=(2,2)))
    invalid_precinct[invalid_precinct.index(b"\xff\x52")+15] = 0
    bad["precinct-size"] = bytes(invalid_precinct)
    ppm = bytearray(codestream([gray],8,8,packed="ppm",packed_split=7))
    start = ppm.index(b"\xff\x60")
    size = 2+struct.unpack_from(">H",ppm,start+2)[0]
    ppm[start:start] = ppm[start:start+size]
    bad["ppm-index"] = bytes(ppm)
    ppt = bytearray(codestream([gray],8,8,packed="ppt",packed_split=7))
    start = ppt.index(b"\xff\x61")
    size = 2+struct.unpack_from(">H",ppt,start+2)[0]
    tile_start = ppt.index(b"\xff\x90")
    struct.pack_into(">I",ppt,tile_start+6,struct.unpack_from(">I",ppt,tile_start+6)[0]+size)
    ppt[start:start] = ppt[start:start+size]
    bad["ppt-index"] = bytes(ppt)
    dual = codestream([gray],8,8,packed="ppt")
    bad["packed-conflict"] = dual[:dual.index(b"\xff\x90")]+marker(0x60,b"\0\0\0\0\0")+dual[dual.index(b"\xff\x90"):]
    valid_ppm = codestream([gray],8,8,packed="ppm")
    first_tile = valid_ppm.index(b"\xff\x90")
    bad["ppm-unused"] = valid_ppm[:first_tile]+marker(0x60,b"\1extra")+valid_ppm[first_tile:]
    # Put the invalid stuffed byte at a fragment end so its source offset differs
    # from the following marker's data. Expected position follows the box layout.
    packed_source = codestream(rgb,8,8,packed="ppm")
    ppm_start = packed_source.index(b"\xff\x60")
    tile_start = packed_source.index(b"\xff\x90")
    record = packed_source[ppm_start+5:tile_start]
    first_chunk = record[:4]+b"\xff"
    second_chunk = b"\xff"+record[6:9]
    third_chunk = record[9:]
    fragments = marker(0x60,b"\0"+first_chunk)+marker(0x60,b"\1"+second_chunk)+marker(0x60,b"\2"+third_chunk)
    stuffed = packed_source[:ppm_start]+fragments+packed_source[tile_start:]
    bad_position = ppm_start+len(marker(0x60,b"\0"+first_chunk))+5
    yield Vector("packed-stuffing",{".jpx":stuffed,".error":b"syntax\n",".offset":f"{bad_position}\n".encode()},("jpx-hostile","jpx-packed-headers"),"Packet stuffing error reports the original packed-header fragment byte",extra={"error":"syntax"})
    for name, data in bad.items():
        feature_requirements = {"poc-range":("jpx-poc",),"precinct-size":("jpx-precincts",),
                                "ppm-index":("jpx-packed-headers",),"ppt-index":("jpx-packed-headers",),
                                "packed-conflict":("jpx-packed-headers",),"ppm-unused":("jpx-packed-headers",)}
        yield Vector(name,{".jpx": data, ".error": b"syntax\n"},("jpx-hostile",)+feature_requirements.get(name,()),"Malformed JPEG 2000 input",extra={"error": "syntax"})


def packet_length(value):
    result = [value & 127]
    value >>= 7
    while value:
        result.insert(0, 128 | (value & 127))
        value >>= 7
    return bytes(result)


def baseline_container(raw, *, compatibility=b"jpx jpxb", requirements=None, extra=b""):
    # T.801 M.8/M.9.2, corrected by Cor.4: MinV=1; feature5 is the
    # unrestricted Part1 codestream. DCM/FUAM both require its one mask bit.
    if requirements is None:
        requirements = struct.pack(">BBBHHBH", 1, 1, 1, 1, 5, 1, 0)
    signature = box(b"jP  ", b"\r\n\x87\n")
    header = box(b"ihdr", struct.pack(">IIH4B", 8, 8, 1, 7, 7, 0, 0))
    header += box(b"colr", b"\x01\x00\x01" + struct.pack(">I", 17))
    return (signature + box(b"ftyp", b"jpx " + struct.pack(">I", 1) + compatibility)
            + box(b"rreq", requirements) + box(b"jp2h", header) + extra + box(b"jp2c", raw))


def marker_streams():
    samples = [0, 64, 128, 255, 255, 128, 64, 0] * 8
    raw = codestream([samples], 8, 8)
    sot = raw.index(b"\xff\x90")
    sod = raw.index(b"\xff\x93")
    length = packet_length(len(raw) - sod - 4)
    plm = marker(0x57, b"\0" + bytes([len(length)]) + length)
    crg = marker(0x63, struct.pack(">HH", 0, 0))
    for name, markers in [("plm", plm), ("crg-zero", crg), ("plm-crg-zero", plm + crg)]:
        yield "jpx-" + name, jp2(raw[:sot] + markers + raw[sot:], 8, 8, 1), samples
    yield "jpx-marker-codec-control", jp2(raw, 8, 8, 1), samples
    yield "jpxb-only", baseline_container(raw), samples
    yield "jpxb-jp2-control", baseline_container(raw, compatibility=b"jpx jpxbjp2 "), samples


def marker_render_cases():
    from .paint import image_case
    for name, data, samples in marker_streams():
        yield image_case(name, data, b"/ColorSpace /DeviceGray /BitsPerComponent 8 /Filter /JPXDecode",
                         samples, 8, 8, matrix=(8, 0, 0, 8, 2, 2),
                         requirement="jpx-boxes" if name.startswith("jpxb-") else "jpx-marker-syntax")
    samples = [0, 64, 128, 255, 255, 128, 64, 0] * 8
    yield image_case("jpx-marker-rgb-control", bytes(v for sample in samples for v in (sample,) * 3),
                     b"/ColorSpace /DeviceRGB /BitsPerComponent 8", samples, 8, 8,
                     matrix=(8, 0, 0, 8, 2, 2), requirement="jpx-marker-syntax")


def marker_cases():
    for name, data, samples in marker_streams():
        yield vector(name, data, [samples], 8, 8, ("jpx-boxes", "jpx-marker-syntax"))
    samples = [0, 64, 128, 255, 255, 128, 64, 0] * 8
    raw = codestream([samples], 8, 8, layers=2)
    sot, sod = raw.index(b"\xff\x90"), raw.index(b"\xff\x93")
    # Layer0 is one empty packet (byte 00); layer1 carries the real packet.
    length = packet_length(len(raw) - sod - 5)
    first = marker(0x57, bytes([0, 1 + len(length), 1]))
    last = marker(0x57, b"\1" + length)
    for name, markers in [("plm-continuation", first + last),
                          ("plm-physical-order", last + first)]:
        encoded = raw[:sot] + markers + raw[sot:]
        encoded = tile_header_marker(encoded, 0x58, b"\0\1" + length)
        yield vector(name, encoded, [samples], 8, 8, ("jpx-marker-syntax",))
    invalid_markers = {
        "plm-duplicate-index": first + first + last,
        "plm-incomplete-list": first,
        "plm-truncated-length": marker(0x57, b"\0\1\x80"),
        "plm-split-length": marker(0x57, b"\0\2\x81") + marker(0x57, b"\1\0"),
        "plm-overflow-length": marker(0x57, b"\0\5\xff\xff\xff\xff\x7f"),
        "plm-empty": marker(0x57, b"\0"),
        "crg-short": marker(0x63, b"\0\0"),
        "crg-extra": marker(0x63, bytes(8)),
        "crg-duplicate": marker(0x63, bytes(4)) * 2,
    }
    for name, addition in invalid_markers.items():
        yield rejection(name, raw[:sot] + addition + raw[sot:], ("jpx-marker-syntax",),
                        "T.800 A.7.2/A.9.1 field extent, index and marker-boundary constraints")
    for code in (0x57, 0x63):
        yield rejection(f"main-marker-in-tile-{code}", tile_header_marker(raw, code, bytes(4)),
                        ("jpx-marker-syntax",), "PLM and CRG belong only to the main header")
    # A nonzero registration is defined, but needs a separately evidenced
    # rendering policy; it is not silently treated as an identity offset.
    yield Vector("crg-nonzero", {".jpx": raw[:sot] + marker(0x63, b"\0\1\0\0") + raw[sot:],
                 ".error": b"unsupported\n"}, ("jpx-marker-syntax",),
                 "T.800 A.9.1 nonzero rendering registration remains explicit", extra={"error": "unsupported"})
    raw = codestream([samples], 8, 8)
    for name, requirements in [
        ("jpxb-alternative-feature", struct.pack(">BBBHHBHBH", 1, 1, 1, 2, 5, 1, 65000, 1, 0)),
        ("jpxb-nondisplay-feature", struct.pack(">BBBHHBHBH", 1, 3, 1, 2, 5, 1, 65000, 2, 0)),
    ]:
        yield vector(name, baseline_container(raw, requirements=requirements), [samples], 8, 8, ("jpx-boxes",))
    for size in (2, 4, 8):
        mask = b"\x80" + bytes(size - 1)
        requirements = bytes([size]) + mask * 2 + b"\0\1\0\5" + mask + b"\0\0"
        yield vector(f"jpxb-mask-{size}", baseline_container(raw, requirements=requirements),
                     [samples], 8, 8, ("jpx-boxes",))
    unknown = struct.pack(">BBBHHBH", 1, 1, 1, 1, 65000, 1, 0)
    yield vector("jpxb-legacy-jp2-unknown", baseline_container(raw, requirements=unknown,
                 compatibility=b"jpx jpxbjp2 "), [samples], 8, 8, ("jpx-boxes",))
    for name, requirements in [
        ("jpxb-required-unknown", struct.pack(">BBBHHBH", 1, 1, 1, 1, 65000, 1, 0)),
        ("jpxb-required-conjunction", struct.pack(">BBBHHBHBH", 1, 3, 3, 2, 5, 1, 65000, 2, 0)),
        ("jpxb-required-vendor", bytes([1, 1, 1]) + struct.pack(">HH", 0, 1) + bytes(16) + b"\1"),
        ("jpxb-deprecated-no-grant", struct.pack(">BBBHHBH", 1, 1, 1, 1, 1, 1, 0)),
    ]:
        yield Vector(name, {".jpx": baseline_container(raw, requirements=requirements), ".error": b"unsupported\n"},
                     ("jpx-boxes",), "Required display expressions cannot be granted by unknown/deprecated features",
                     extra={"error": "unsupported"})
    container = baseline_container(raw)
    rreq = container.index(b"rreq") - 4
    rreq_end = rreq + struct.unpack_from(">I", container, rreq)[0]
    for name, data in [
        ("jpxb-requirements-missing", container[:rreq] + container[rreq_end:]),
        ("jpxb-requirements-duplicate", container[:rreq_end] + container[rreq:rreq_end] + container[rreq_end:]),
        ("jpxb-requirements-short", baseline_container(raw, requirements=b"\1\1")),
        ("jpxb-requirements-mask-length", baseline_container(raw, requirements=bytes([3]) + bytes(10))),
        ("jpxb-requirements-extra", baseline_container(raw, requirements=bytes([1, 0, 0, 0, 0, 0, 0, 0]))),
    ]:
        yield rejection(name, data, ("jpx-boxes",), "T.801 M.11.1 exact box position and field extent")
    # T.801 M.11.6/.7: empty first headers inherit the complete JP2 defaults.
    # Keep the original input bytes and test their independently encoded samples.
    for kind in (b"jpch", b"jplh"):
        yield vector("jpxb-layout-" + kind.decode(), baseline_container(raw, extra=box(kind, b"")),
                     [samples], 8, 8, ("jpx-boxes",), color="gray")
    for kind in (b"ftbl", b"cref"):
        yield Vector("jpxb-layout-" + kind.decode(), {".jpx": baseline_container(raw, extra=box(kind, b"")),
                     ".error": b"unsupported\n"}, ("jpx-boxes",),
                     "A baseline brand and mask do not bypass first-image metadata semantics", extra={"error": "unsupported"})
