"""Independent JPEG 2000 Part 1 encoder and JP2 box fixtures.

The encoder deliberately writes one layer, default precincts, rectangular code-blocks
per subband, full bit planes and the ordinary MQ coding style. It implements
forward integer 5/3 lifting, RCT, scalar-quantized 9/7 lifting, tag-tree leaves,
and Annex C arithmetic encoding, using only the Python standard library.
Expected samples are the input pixels, never the output of a decoder. Lossy
vectors have an explicit tolerance for quantization. This is test tooling,
not a general JPEG 2000 encoder.
"""
import math
import struct

from .model import Vector

AREA = "jpx"

# ISO/IEC 15444-1 Table C.2: Qe, next-MPS, next-LPS, exchange flag.
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
    def __init__(self):
        self.a, self.c, self.ct = 0x8000, 0, 12
        self.output = [0]  # Carry propagation sentinel, not part of the stream.
        self.reset()

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

    def finish(self):
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


def encode_block(coefficients, orientation, reset=False, segmentation=False):
    height, width = len(coefficients), len(coefficients[0])
    maximum = max(abs(v) for row in coefficients for v in row)
    if maximum == 0:
        return 0, b""
    planes = maximum.bit_length()
    sig, visited, refined = set(), set(), set()
    arithmetic = ArithmeticEncoder()

    def neighbours(x, y):
        h = [(xx, y) for xx in (x - 1, x + 1) if (xx, y) in sig]
        v = [(x, yy) for yy in (y - 1, y + 1) if (x, yy) in sig]
        d = sum((xx, yy) in sig for xx in (x - 1, x + 1) for yy in (y - 1, y + 1))
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
            arithmetic.write(context, int(coefficients[y][x] < 0) ^ prediction)
            sig.add((x, y))

    positions = [(x, y) for stripe in range(0, height, 4) for x in range(width)
                 for y in range(stripe, min(stripe + 4, height))]
    for plane in reversed(range(planes)):
        if plane != planes - 1:
            for x, y in positions:
                if (x, y) not in sig and zc(x, y) != 0:
                    visited.add((x, y))
                    significant(x, y, plane)
            if reset:
                arithmetic.reset()
            for x, y in positions:
                if (x, y) in sig and (x, y) not in visited:
                    context = 16 if (x, y) in refined else 14 + int(zc(x, y) != 0)
                    arithmetic.write(context, (abs(coefficients[y][x]) >> plane) & 1)
                    refined.add((x, y))
            if reset:
                arithmetic.reset()
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
        if segmentation:
            for bit in (1, 0, 1, 0):
                arithmetic.write(18, bit)
        if reset:
            arithmetic.reset()
        visited.clear()
    return planes, arithmetic.finish()


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
    if width == 0 or height == 0:
        return 0, 0, []
    xs = [x0] + list(range((x0//block_size+1)*block_size, x0+width, block_size)) + [x0+width]
    ys = [y0] + list(range((y0//block_size+1)*block_size, y0+height, block_size)) + [y0+height]
    blocks = [[row[a-x0:b-x0] for row in band[c-y0:d-y0]]
              for c, d in zip(ys, ys[1:]) for a, b in zip(xs, xs[1:])]
    return len(xs)-1, len(ys)-1, blocks


def packet(bands, maxima, origins, block_size=64, reset=False, segmentation=False, markers=False, layer=0):
    header, body = PacketBits(), b""
    present = any(any(v for row in band for v in row) for band in bands)
    header.number(int(present))
    if present:
        for orientation_index, (band, maximum, origin) in enumerate(zip(bands, maxima, origins)):
            columns, rows, blocks = partition(band, origin, block_size)
            if not blocks:
                continue
            encoded = [encode_block(block, 0 if len(bands) == 1 else orientation_index+1,
                                    reset=reset, segmentation=segmentation) for block in blocks]
            inclusion = TagTree(columns, rows, [layer if planes else layer+1 for planes, _ in encoded])
            zeros = TagTree(columns, rows, [maximum-planes if planes else maximum for planes, _ in encoded])
            for index, (planes, data) in enumerate(encoded):
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
                exponent = passes.bit_length()-1
                extra = max(0, len(data).bit_length()-3-exponent)
                header.number((1 << extra)-1, extra)
                header.number(0)
                header.number(len(data), 3+exponent+extra)
                body += data
    return header.finish() + (b"\xff\x92" if markers else b"") + body


def marker(code, payload=b""):
    return struct.pack(">HH", 0xff00 | code, len(payload)+2) + payload


def codestream(planes, width, height, *, levels=0, tile=None, progression=0,
               bits=8, signed=False, mct=False, lossy=False, origin=(0, 0),
               tile_parts=False, markers=False, modes=0, extra_markers=False, block_exp=6, layers=1, derived_quant=False):
    x0, y0 = origin
    tile = tile or (width, height)
    count = len(planes)
    siz = struct.pack(">H8IH", 0, width+x0, height+y0, x0, y0, tile[0], tile[1], x0, y0, count)
    siz += bytes([bits-1 + 128*int(signed), 1, 1])*count
    coding = bytes([6 if markers else 0, progression]) + struct.pack(">H", layers) + bytes([int(mct), levels, block_exp-2, block_exp-2, modes, int(not lossy)])
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
            keys = [(l,r,c) for c in range(count) for r in range(levels+1) for l in range(layers)]
            keys.sort(key=lambda v: (v[0],v[1],v[2]) if progression == 0 else
                      (v[1],v[0],v[2]) if progression == 1 else
                      (v[1],v[2],v[0]) if progression == 2 else (v[2],v[1],v[0]))
            packets = []
            for layer, resolution, component in keys:
                scale = 1 << (levels-resolution)
                if ((tx+tw+scale-1)//scale == (tx+scale-1)//scale or
                    (ty+th+scale-1)//scale == (ty+scale-1)//scale):
                    continue
                start = 0 if resolution == 0 else 1 + 3*(resolution-1)
                n = 1 if resolution == 0 else 3
                divisor = 1 << (levels-resolution)
                rx0, ry0 = (tx+divisor-1)//divisor, (ty+divisor-1)//divisor
                origins = [(rx0, ry0)] if resolution == 0 else [
                    (rx0//2 if o & 1 else (rx0+1)//2,
                     ry0//2 if o & 2 else (ry0+1)//2) for o in (1, 2, 3)]
                data = (packet(bands[component][start:start+n], [e+1 for e in exponents[start:start+n]],
                              origins, 1 << block_exp, reset=bool(modes & 2), segmentation=bool(modes & 32), markers=markers, layer=layer)
                        if layer == layers-1 else b"\0"+(b"\xff\x92" if markers else b""))
                packets.append(data)
            parts = [[p] for p in packets] if tile_parts else [packets]
            for part, group in enumerate(parts):
                payload = b""
                for seq, p in enumerate(group):
                    if markers:
                        payload += marker(0x91, struct.pack(">H", seq))
                    payload += p
                result += marker(0x90, struct.pack(">HIBB", index, 14+len(payload), part, len(parts)))
                result += b"\xff\x93" + payload
            index += 1
    return result + b"\xff\xd9"


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
    for modes in (2,32,34):
        yield vector(f"modes-{modes}",codestream([gray],8,8,modes=modes),[gray],8,8)
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
    for name, data in bad.items():
        yield Vector(name,{".jpx": data, ".error": b"syntax\n"},("jpx-hostile",),"Malformed JPEG 2000 input",extra={"error": "syntax"})
