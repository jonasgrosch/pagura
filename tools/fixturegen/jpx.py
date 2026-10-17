"""Independent JPEG 2000 Part 1 encoder and JP2 box fixtures.

The encoder deliberately writes one layer, default precincts, rectangular code-blocks
per subband, full bit planes and the ordinary MQ coding style. It implements
forward integer 5/3 lifting, RCT, scalar-quantized 9/7 lifting, tag-tree leaves,
and MQ arithmetic encoding, using only the Python standard library.
Expected samples are the input pixels, never the output of a decoder. Lossy
vectors have an explicit tolerance for quantization. This is test tooling,
not a general JPEG 2000 encoder.
"""
import math
import struct

from .model import Vector

AREA = "jpx"

# T.800-dependent MQ state data: pending primary-text/table-locator review.
# Qe, next-MPS, next-LPS, exchange flag; compared with OpenJPEG, not verified here.
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
            self.state, self.mps = list(state), list(mps)

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
    def __init__(self, planes, modes, roi=0):
        self.total, self.modes, self.pass_index, self.first = planes*3-2, modes, 0, 0
        self.roi = roi
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
        self.raw = bool(self.modes & 1 and self.pass_index >= 10+3*self.roi and self.pass_index % 3 != 0)
        self.raw_bits = []
        self.encoder = ArithmeticEncoder(self.state,self.mps)


def encode_block(coefficients, orientation, modes=0, roi=0):
    height, width = len(coefficients), len(coefficients[0])
    maximum = max(abs(v) for row in coefficients for v in row)
    if maximum == 0:
        return 0, []
    planes = maximum.bit_length()
    sig, visited, refined = set(), set(), set()
    arithmetic = PassEncoder(planes,modes,roi)

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
            for x, y in positions:
                if (x, y) in sig and (x, y) not in visited:
                    context = 16 if (x, y) in refined else 14 + int(zc(x, y) != 0)
                    arithmetic.write(context, (abs(coefficients[y][x]) >> plane) & 1)
                    refined.add((x, y))
            if modes & 2:
                arithmetic.reset()
            arithmetic.end_pass()
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
        visited.clear()
    return planes, arithmetic.parts


def forward_line(values, origin, reversible):
    result = list(values)
    if len(result) == 1:
        if origin & 1 and reversible:
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
        result = [v / 1.230174104914001 if i % 2 == low else v * (1.230174104914001 / 2)
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


def packet(bands, maxima, origins, block_size=64, modes=0, markers=False, layer=0, roi=0):
    header, body = PacketBits(), b""
    present = any(any(v for row in band for v in row) for band in bands)
    header.number(int(present))
    if present:
        for orientation_index, (band, maximum, origin) in enumerate(zip(bands, maxima, origins)):
            columns, rows, blocks = partition(band, origin, block_size)
            if not blocks:
                continue
            encoded = [encode_block(block, 0 if len(bands) == 1 else orientation_index+1, modes, roi) for block in blocks]
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
                passes = planes * 3 - 2
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


def packed_markers(code,data,split,start_index=0):
    chunks = [data[i:i+split] for i in range(0,len(data),split)] or [b""]
    assert len(chunks)+start_index <= 256
    return b"".join(marker(code,bytes([index+start_index])+chunk) for index,chunk in enumerate(chunks))


def ppm_markers(headers,split):
    chunks, current = [], b""
    for header in headers:
        if split-len(current) < 4:
            chunks.append(current)
            current = b""
        current += struct.pack(">I",len(header))
        while header:
            if len(current) == split:
                chunks.append(current)
                current = b""
            amount = min(split-len(current),len(header))
            current += header[:amount]
            header = header[amount:]
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
        exponents = ([bits+2] + [bits+2-(levels-r) for r in range(1,levels+1) for _ in range(3)]
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
        result += marker(0x55, b"\0\x40")
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
                bands = [[[[math.trunc(v / (2**(bits-e))) for v in row] for row in b]
                          for b, e in zip(comp,exponents)] for comp in bands]
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
            ppt_index = 0
            for part,group in enumerate(groups):
                body,headers = b"",b""
                for sequence,key in enumerate(group):
                    layer,resolution,c,precinct,_,_ = key
                    crops,origins,block_size,maxima = descriptions[(c,resolution,precinct)]
                    header,data = (packet(crops,maxima,origins,block_size,modes,markers,layer,roi[c])
                                   if layer == layers-1 else (b"\0"+(b"\xff\x92" if markers else b""),b""))
                    headers += header
                    if markers:
                        body += marker(0x91,struct.pack(">H",sequence))
                    body += (b"" if packed else header)+data
                tile_header = marker(0x5f,encode_poc(late_poc)) if late_poc and part == 1 else b""
                if packed == "ppt":
                    tile_header += packed_markers(0x61,headers,packed_split,ppt_index)
                    ppt_index += max(1,(len(headers)+packed_split-1)//packed_split)
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


def layered_codestream(values,modes):
    """Split one code-block's codewords and passes across two real contributions."""
    source = [[v-128 for v in values[y*8:y*8+8]] for y in range(8)]
    planes,pieces = encode_block(source,0,modes)
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


def cases():
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
    yield vector("poc-deferred-declaration",codestream([[30,70,110]],1,3,levels=1,origin=(0,1),precincts=[(0,0),(1,1)],late_poc=[(0,0,1,2,1,3)]),[[30,70,110]],1,3,("jpx-poc","jpx-precincts"))
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
    # PDF 7.4.9 directly names these enumerations; encoding mechanics remain T.800/T.801 pending review.
    forbidden_color = jp2(codestream(rgb,8,8),8,8,3).replace(struct.pack(">I",16),struct.pack(">I",19),1)
    yield Vector("pdf-colour-19",{".jpx":forbidden_color,".error":b"syntax\n"},("jpx-boxes","jpx-pdf-contract"),"PDF 7.4.9 excludes enumerated CIEJab 19",extra={"error":"syntax"})
    cmyk = [gray,gray,gray,gray]
    cmyk_data = jp2(codestream(cmyk,8,8),8,8,4).replace(struct.pack(">I",16),struct.pack(">I",12),1)
    yield vector("pdf-colour-12",cmyk_data,cmyk,8,8,("jpx-boxes","jpx-pdf-contract"),color="cmyk")
    yield Vector("pdf-depth-17",{".jpx":codestream([[65536]*64],8,8,bits=17),".error":b"unsupported\n"},("jpx-part1","jpx-pdf-contract"),"PDF allows 17-bit samples; the current unsigned 16-bit sample API reports Unsupported",extra={"error":"unsupported"})
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
    ppm[ppm.index(b"\xff\x60")+4] = 1
    bad["ppm-index"] = bytes(ppm)
    ppt = bytearray(codestream([gray],8,8,packed="ppt",packed_split=7))
    ppt[ppt.index(b"\xff\x61")+4] = 1
    bad["ppt-index"] = bytes(ppt)
    dual = codestream([gray],8,8,packed="ppt")
    bad["packed-conflict"] = dual[:dual.index(b"\xff\x90")]+marker(0x60,b"\0\0\0\0\0")+dual[dual.index(b"\xff\x90"):]
    valid_ppm = codestream([gray],8,8,packed="ppm")
    first_tile = valid_ppm.index(b"\xff\x90")
    bad["ppm-unused"] = valid_ppm[:first_tile]+marker(0x60,b"\1extra")+valid_ppm[first_tile:]
    # Put the invalid stuffed byte at a fragment end so its source offset differs
    # from the following marker's data. Expected position follows the box layout.
    packed_source = codestream([gray],8,8,packed="ppm")
    ppm_start = packed_source.index(b"\xff\x60")
    tile_start = packed_source.index(b"\xff\x90")
    record = packed_source[ppm_start+5:tile_start]
    first_chunk = record[:4]+b"\xff"
    second_chunk = b"\xff"
    third_chunk = record[6:]
    fragments = marker(0x60,b"\0"+first_chunk)+marker(0x60,b"\1"+second_chunk)+marker(0x60,b"\2"+third_chunk)
    stuffed = packed_source[:ppm_start]+fragments+packed_source[tile_start:]
    bad_position = ppm_start+len(marker(0x60,b"\0"+first_chunk))+5
    yield Vector("packed-stuffing",{".jpx":stuffed,".error":b"syntax\n",".offset":f"{bad_position}\n".encode()},("jpx-hostile","jpx-packed-headers"),"Packet stuffing error reports the original packed-header fragment byte",extra={"error":"syntax"})
    for name, data in bad.items():
        feature_requirements = {"poc-range":("jpx-poc",),"precinct-size":("jpx-precincts",),
                                "ppm-index":("jpx-packed-headers",),"ppt-index":("jpx-packed-headers",),
                                "packed-conflict":("jpx-packed-headers",),"ppm-unused":("jpx-packed-headers",)}
        yield Vector(name,{".jpx": data, ".error": b"syntax\n"},("jpx-hostile",)+feature_requirements.get(name,()),"Malformed JPEG 2000 input",extra={"error": "syntax"})
