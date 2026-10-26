"""Independent ISO 32000-2 image samples and T.81/T.4/T.6 encoders.

Python standard library only. JPEG expected pixels are evaluated from the
quantized coefficients using the T.81 inverse transform, independent of the
Rust implementation. One 8-bit level is allowed for IDCT/colour rounding.
"""
import math
import struct
import zlib
from .model import Vector
from .pdf import document, stream

AREA = "image"
ORDER = sorted(range(64), key=lambda p: (p//8+p%8, p%8 if (p//8+p%8)%2 == 0 else p//8))
COS = [[math.cos((2*x+1)*u*math.pi/16) for u in range(8)] for x in range(8)]


def vector(name, kind, encoded, expected=b"", *, requirement=None, outcome="valid", **options):
    params = dict(kind=kind, outcome=outcome, **options)
    return Vector(name, {".in": encoded, ".out": expected,
                        ".params": "".join(f"{k}={v}\n" for k, v in params.items()).encode()},
                  (requirement or f"image-{kind}",), name.replace("-", " "))


class Wire:
    def __init__(self):
        self.bits = ""

    def put(self, value, size):
        if size:
            self.bits += f"{value:0{size}b}"

    def code(self, value):
        self.bits += value

    def bytes(self, jpeg=False):
        bits = self.bits + ("1" if jpeg else "0") * (-len(self.bits) % 8)
        data = bytes(int(bits[i:i+8], 2) for i in range(0, len(bits), 8))
        return data.replace(b"\xff", b"\xff\0") if jpeg else data

    def align(self):
        self.bits += "0" * (-len(self.bits) % 8)


def segment(marker, data):
    return bytes([255, marker]) + struct.pack(">H", len(data)+2) + data


DC = list(range(12))
AC = [r*16+s for r in range(16) for s in range(1, 11)] + [r*16 for r in range(16)]
AC_CODES = {s: (i, 8) for i, s in enumerate(AC)}


def huff(wire, symbol, dc=False):
    wire.put(symbol if dc else AC_CODES[symbol][0], 4 if dc else 8)


def magnitude(wire, value):
    size = abs(value).bit_length()
    wire.put(value if value >= 0 else value + (1 << size) - 1, size)
    return size


def dc_value(wire, value):
    huff(wire, abs(value).bit_length(), True)
    magnitude(wire, value)


def ac_values(wire, block, low):
    run = 0
    for v in block[1:]:
        value = (abs(v) >> low) * (1 if v >= 0 else -1)
        if not value:
            run += 1
            continue
        while run >= 16:
            huff(wire, 0xf0)
            run -= 16
        huff(wire, run*16+abs(value).bit_length())
        magnitude(wire, value)
        run = 0
    if run:
        huff(wire, 0)


def ac_refinement(wire, block):
    k = 1
    while k < 64:
        target = next((i for i in range(k,64) if abs(block[i]) == 1), 64)
        if target == 64:
            huff(wire, 0)
            for i in range(k,64):
                if abs(block[i]) >= 2:
                    wire.put(abs(block[i]) & 1, 1)
            break
        zeros = sum(block[i] == 0 for i in range(k,target))
        if zeros >= 16:
            huff(wire, 0xf0)
            consumed = 0
            while consumed < 16:
                if abs(block[k]) >= 2:
                    wire.put(abs(block[k]) & 1, 1)
                elif block[k] == 0:
                    consumed += 1
                k += 1
        else:
            huff(wire, zeros*16+1)
            wire.put(int(block[target] > 0), 1)
            for i in range(k,target):
                if abs(block[i]) >= 2:
                    wire.put(abs(block[i]) & 1, 1)
            k = target+1


def coefficients(plane, width, height):
    result = []
    for by in range((height+7)//8):
        for bx in range((width+7)//8):
            block = [[plane[min(by*8+y, height-1)*width+min(bx*8+x, width-1)]-128
                      for x in range(8)] for y in range(8)]
            natural = []
            for v in range(8):
                for u in range(8):
                    value = sum(block[y][x] * COS[x][u]
                                * COS[y][v] for y in range(8) for x in range(8))
                    value *= (1/math.sqrt(2) if u == 0 else 1) * (1/math.sqrt(2) if v == 0 else 1) / 4
                    natural.append(round(value))
            result.append([natural[i] for i in ORDER])
    return result


def inverse(blocks, width, height):
    out = [0] * (width*height)
    stride = (width+7)//8
    for i, block in enumerate(blocks):
        for y in range(8):
            for x in range(8):
                sx, sy = i%stride*8+x, i//stride*8+y
                if sx >= width or sy >= height:
                    continue
                value = 128 + sum(c * (1/math.sqrt(2) if p%8 == 0 else 1)
                                  * (1/math.sqrt(2) if p//8 == 0 else 1)
                                  * COS[x][p%8]
                                  * COS[y][p//8]/4
                                  for p, c in zip(ORDER, block))
                out[sy*width+sx] = max(0, min(255, math.floor(value+0.5)))
    return bytes(out)


def jpeg(width=13, height=9, n=1, progressive=False, restart=0, adobe=None, transform=None, sampling=None, extended=False, interleaved=False, combined_refinement=False):
    sampling = sampling or [(1, 1)]*n
    mh, mv = max(h for h, v in sampling), max(v for h, v in sampling)
    shapes = [((width*h+mh-1)//mh, (height*v+mv-1)//mv) for h, v in sampling]
    planes = [bytes((40+ci*39+x*5+y*3+(x*y)%7) % 220 for y in range(ch) for x in range(cw))
              for ci, (cw, ch) in enumerate(shapes)]
    blocks = [coefficients(p, cw, ch) for p, (cw, ch) in zip(planes, shapes)]
    result = b"\xff\xd8"
    if adobe is not None:
        result += segment(0xee, b"Adobe"+struct.pack(">HHH", 100, 0, 0)+bytes([adobe]))
    result += segment(0xdb, b"\0"+bytes([1])*64)
    frame = bytes([8])+struct.pack(">HHB", height, width, n)
    frame += b"".join(bytes([i+1, h*16+v, 0]) for i, (h, v) in enumerate(sampling))
    result += segment(0xc2 if progressive else (0xc1 if extended else 0xc0), frame)
    result += segment(0xc4, b"\0"+bytes([0, 0, 0, len(DC)]+[0]*12)+bytes(DC)
                      +b"\x10"+bytes([0]*7+[len(AC)]+[0]*8)+bytes(AC))
    if restart:
        result += segment(0xdd, struct.pack(">H", restart))

    def scan(ci, start, end, high=0, low=0):
        nonlocal result
        result += segment(0xda, bytes([1, ci+1, 0, start, end, high*16+low]))
        wire, predictor, sequence = Wire(), 0, 0
        for bi, block in enumerate(blocks[ci]):
            if restart and bi and bi % restart == 0:
                result += wire.bytes(True)+bytes([255, 0xd0+sequence])
                wire, predictor, sequence = Wire(), 0, (sequence+1)%8
            if start == 0:
                if high:
                    wire.put(block[0] & 1, 1)
                else:
                    value = block[0] >> low
                    dc_value(wire, value-predictor)
                    predictor = value
                if not progressive:
                    ac_values(wire, block, 0)
            elif high and combined_refinement:
                ac_refinement(wire, block)
            elif high:
                value = block[start]
                if abs(value) >= 2:
                    huff(wire, 0)
                    wire.put(abs(value) & 1, 1)
                elif value:
                    huff(wire, 1)
                    wire.put(int(value > 0), 1)
                else:
                    huff(wire, 0)
            else:
                ac_values(wire, block, low)
        result += wire.bytes(True)

    def interleaved_scan(high=0, low=0):
        nonlocal result
        end = 0 if progressive else 63
        result += segment(0xda, bytes([n])+b"".join(bytes([ci+1, 0]) for ci in range(n))+bytes([0, end, high*16+low]))
        cols, rows = (width+8*mh-1)//(8*mh), (height+8*mv-1)//(8*mv)
        wire, predictors, sequence = Wire(), [0]*n, 0
        for mcu in range(cols*rows):
            if restart and mcu and mcu%restart == 0:
                result += wire.bytes(True)+bytes([255, 0xd0+sequence])
                wire, predictors, sequence = Wire(), [0]*n, (sequence+1)%8
            for ci, (h,v) in enumerate(sampling):
                cw, ch = shapes[ci]
                stride = (cw+7)//8
                for yy in range(v):
                    for xx in range(h):
                        bx, by = mcu%cols*h+xx, mcu//cols*v+yy
                        block = blocks[ci][by*stride+bx] if bx < stride and by < (ch+7)//8 else [0]*64
                        if high:
                            wire.put(block[0] & 1, 1)
                        else:
                            value = block[0] >> low
                            dc_value(wire, value-predictors[ci])
                            predictors[ci] = value
                            if not progressive:
                                ac_values(wire, block, 0)
        result += wire.bytes(True)

    if interleaved:
        interleaved_scan(low=int(progressive))
    for ci in range(n):
        if progressive:
            if not interleaved:
                scan(ci, 0, 0, low=1)
            scan(ci, 1, 63, low=1)
            if not interleaved:
                scan(ci, 0, 0, high=1)
            if combined_refinement:
                scan(ci, 1, 63, high=1)
            else:
                for k in range(1, 64):
                    scan(ci, k, k, high=1)
        elif not interleaved:
            scan(ci, 0, 63)
    if interleaved and progressive:
        interleaved_scan(high=1)
    result += b"\xff\xd9"
    reconstructed = [inverse(b, cw, ch) for b, (cw, ch) in zip(blocks, shapes)]
    use_transform = (adobe in (1, 2)) if adobe is not None else (n == 3 if transform is None else bool(transform))
    expected = bytearray()
    for y in range(height):
        for x in range(width):
            pixel = [p[(y*v//mv)*cw+(x*h//mh)] for p, (cw, _), (h, v) in zip(reconstructed, shapes, sampling)]
            if use_transform and n >= 3:
                yy, cb, cr = pixel[0], pixel[1]-128, pixel[2]-128
                converted = [yy+1.402*cr, yy-0.344136*cb-0.714136*cr, yy+1.772*cb]
                pixel[:3] = [max(0, min(255, math.floor(v+0.5))) for v in converted]
                if n == 4:
                    pixel[:3] = [255-v for v in pixel[:3]]
            expected.extend(pixel)
    return result, bytes(expected)


# T.4 terminal codes needed by the independent fixture encoder (0..15).
WHITE = "00110101 000111 0111 1000 1011 1100 1110 1111 10011 10100 00111 01000 001000 000011 110100 110101".split()
BLACK = "0000110111 010 11 10 011 0011 0010 00011 000101 000100 0000100 0000101 0000111 00000100 00000111 000011000".split()


WHITE += "101010 101011 0100111 0001100 0001000 0010111 0000011 0000100 0101000 0101011 0010011 0100100 0011000 00000010 00000011 00011010 00011011 00010010 00010011 00010100 00010101 00010110 00010111 00101000 00101001 00101010 00101011 00101100 00101101 00000100 00000101 00001010 00001011 01010010 01010011 01010100 01010101 00100100 00100101 01011000 01011001 01011010 01011011 01001010 01001011 00110010 00110011 00110100".split()
BLACK += "0000010111 0000011000 0000001000 00001100111 00001101000 00001101100 00000110111 00000101000 00000010111 00000011000 000011001010 000011001011 000011001100 000011001101 000001101000 000001101001 000001101010 000001101011 000011010010 000011010011 000011010100 000011010101 000011010110 000011010111 000001101100 000001101101 000011011010 000011011011 000001010100 000001010101 000001010110 000001010111 000001100100 000001100101 000001010010 000001010011 000000100100 000000110111 000000111000 000000100111 000000101000 000001011000 000001011001 000000101011 000000101100 000001011010 000001100110 000001100111".split()
WHITE_MAKEUP = "11011 10010 010111 0110111 00110110 00110111 01100100 01100101 01101000 01100111 011001100 011001101 011010010 011010011 011010100 011010101 011010110 011010111 011011000 011011001 011011010 011011011 010011000 010011001 010011010 011000 010011011".split()
BLACK_MAKEUP = "0000001111 000011001000 000011001001 000001011011 000000110011 000000110100 000000110101 0000001101100 0000001101101 0000001001010 0000001001011 0000001001100 0000001001101 0000001110010 0000001110011 0000001110100 0000001110101 0000001110110 0000001110111 0000001010010 0000001010011 0000001010100 0000001010101 0000001011010 0000001011011 0000001100100 0000001100101".split()
COMMON = "00000001000 00000001100 00000001101 000000010010 000000010011 000000010100 000000010101 000000010110 000000010111 000000011100 000000011101 000000011110 000000011111".split()


def run(wire, length, black):
    while length >= 64:
        size = min(length // 64 * 64, 2560)
        table = BLACK_MAKEUP if black else WHITE_MAKEUP
        wire.code(table[size//64-1] if size < 1792 else COMMON[(size-1792)//64])
        length -= size
    wire.code((BLACK if black else WHITE)[length])


def row2d(wire, row, reference):
    width = len(row)
    a0, color, first = 0, 0, True
    transitions = lambda line: [x for x in range(width) if line[x] != (line[x-1] if x else 0)]
    current, previous = transitions(row), transitions(reference)
    vertical = {-3: "0000010", -2: "000010", -1: "010", 0: "1", 1: "011", 2: "000011", 3: "0000011"}
    while a0 < width:
        a1 = next((x for i, x in enumerate(current) if i % 2 == color and (x > a0 or first and x == a0)), width)
        a2 = next((x for x in current if x > a1), width)
        b1 = next((x for i, x in enumerate(previous) if i % 2 == color and (x > a0 or first and x == a0)), width)
        b2 = next((x for x in previous if x > b1), width)
        if b2 < a1:
            wire.code("0001")
            a0 = b2
        elif a1-b1 in vertical:
            wire.code(vertical[a1-b1])
            a0, color = a1, 1-color
        else:
            wire.code("001")
            run(wire, a1-a0, color)
            run(wire, a2-a1, 1-color)
            a0 = a2
        first = False


def ccitt(rows, k=0, eol=False, align=False, eob=True, black_is_1=False):
    width = len(rows[0])
    wire = Wire()
    for y, row in enumerate(rows):
        if align:
            wire.align()
        if eol or k > 0:
            wire.code("000000000001")
        one_d = k == 0 or (k > 0 and y % 3 == 0)
        if k > 0:
            wire.put(int(one_d), 1)
        if not one_d:
            row2d(wire, row, rows[y-1] if y else [0]*width)
            continue
        runs, color, size = [], 0, 0
        for bit in row:
            if bit == color:
                size += 1
            else:
                runs.append(size)
                color, size = bit, 1
        runs.append(size)
        for i, size in enumerate(runs):
            run(wire, size, i%2)
    if align:
        wire.align()
    if eob:
        wire.code("000000000001"*(2 if k < 0 else 6))
    expected = Wire()
    for row in rows:
        expected.code("".join(str(v if black_is_1 else 1-v) for v in row))
        expected.align()
    return wire.bytes(), expected.bytes()


def raw_cases():
    for bits in (1, 2, 4, 8, 16):
        for n in (1, 3, 4):
            width, height = 7, 3
            maximum = (1 << bits)-1
            samples = [(i*37+i//7) & maximum for i in range(width*height*n)]
            wire = Wire()
            for y in range(height):
                for v in samples[y*width*n:(y+1)*width*n]:
                    wire.put(v, bits)
                wire.align()
            expected = bytes(samples) if bits <= 8 else b"".join(struct.pack(">H", v) for v in samples)
            space = {1: b"DeviceGray", 3: b"DeviceRGB", 4: b"DeviceCMYK"}[n]
            for flate in (False, True):
                encoded = zlib.compress(wire.bytes()) if flate else wire.bytes()
                dictionary = b"/Type /XObject /Subtype /Image /Width 7 /Height 3 /BitsPerComponent %d /ColorSpace /%s" % (bits, space)
                if flate:
                    dictionary += b" /Filter /FlateDecode"
                pdf = document([b"<< /Type /Catalog /Pages 2 0 R >>", b"<< /Type /Pages /Kids [] /Count 0 >>", stream(encoded, dictionary)])
                name = f"samples-{bits}-{n}-{'flate' if flate else 'raw'}"
                yield Vector(name, {".pdf": pdf, ".out": expected,
                                   ".params": f"kind=samples\nwidth=7\nheight=3\nbits={bits}\nn={n}\noutcome=valid\n".encode()},
                             ("image-samples", "image-dictionaries"), "MSB rows, row padding and exact unpacked samples")


def dictionary_cases():
    base = [b"<< /Type /Catalog /Pages 2 0 R >>", b"<< /Type /Pages /Kids [] /Count 0 >>"]
    common = b"/Type /XObject /Subtype /Image /Width 2 /Height 1 /BitsPerComponent 8 /ColorSpace /DeviceGray"
    stencil = stream(b"\x40", b"/Type /XObject /Subtype /Image /Width 2 /Height 1 /ImageMask true /Decode [1 0]")
    soft = stream(bytes([64, 192]), common)
    for name, extra, mask, parameters, outcome in [
        ("explicit-mask", b" /Mask 4 0 R", stencil, "mask=stencil\n", "valid"),
        ("soft-mask", b" /SMask 4 0 R", soft, "mask=soft\n", "valid"),
        ("soft-overrides-key", b" /SMask 4 0 R /Mask [250 255]", soft, "mask=soft\n", "valid"),
        ("invalid-explicit-mask", b" /Mask 4 0 R", soft, "", "syntax"),
        ("invalid-soft-mask", b" /SMask 4 0 R", stencil, "", "syntax"),
        ("colour-key", b" /Mask [12 26]", soft, "mask=key\n", "valid"),
        ("reversed-key", b" /Mask [26 12]", soft, "", "syntax"),
        ("short-decode", b" /Decode [0]", soft, "", "syntax"),
        ("huge-dictionary", b" /Width 9223372036854775807", soft, "", "limit"),
    ]:
        # A single value for every dictionary key; enormous dimensions are
        # valid integers beyond the public image width representation.
        image_common = common.replace(b"/Width 2", b"/Width 9223372036854775807") if name == "huge-dictionary" else common
        image_extra = b"" if name == "huge-dictionary" else extra
        pdf = document(base+[stream(bytes([12, 26]), image_common+image_extra), mask])
        yield Vector(name, {".pdf": pdf, ".out": bytes([12, 26]),
                           ".params": f"kind=dictionary\noutcome={outcome}\n{parameters}".encode()},
                     ("image-dictionaries",), name.replace("-", " "))
    pdf = document(base+[stream(bytes([12,26]), b"/Type /XObject /Subtype /Image /Width 4 0 R /Height 5 0 R /BitsPerComponent 6 0 R /ColorSpace 7 0 R /Interpolate 8 0 R /Decode [1 0]"),
                         b"2", b"1", b"8", b"/DeviceGray", b"true"])
    yield Vector("indirect-entries", {".pdf": pdf, ".out": bytes([12,26]),
                  ".params": b"kind=dictionary\noutcome=valid\ninterpolate=1\n"}, ("image-dictionaries",), "Indirect image scalar entries")
    for name, dictionary, encoded, expected in [
        ("inline-abbreviated", b"<< /W 3 /H 2 /BPC 2 /CS /G /D [1 0] /I true >>", bytes([0x18,0xe4]), bytes([0,1,2,3,2,1])),
        ("inline-indexed", b"<< /W 3 /H 1 /BPC 2 /CS [/I /RGB 3 <000000FF000000FF000000FF>] >>", bytes([0x18]), bytes([0,1,2])),
        ("inline-flate", b"<< /W 4 /H 1 /BPC 8 /CS /G /F /Fl >>", zlib.compress(b" EIQ"), b" EIQ"),
        ("inline-asciihex", b"<< /W 2 /H 1 /BPC 8 /CS /G /F /AHx >>", b"0c1a>", bytes([12,26])),
        ("inline-runlength", b"<< /W 3 /H 1 /BPC 8 /CS /G /F /RL >>", bytes([2])+b" EI"+bytes([128]), b" EI"),
    ]:
        yield Vector(name, {".dict": dictionary, ".in": encoded, ".out": expected,
                           ".params": b"kind=inline\noutcome=valid\n"}, ("image-inline",), name.replace("-", " "))

    rows = [[int((x+y)%3 == 0) for x in range(13)] for y in range(6)]
    encoded, _ = ccitt(rows, k=-1)
    yield Vector("inline-ccitt", {".dict": b"<< /W 13 /H 6 /BPC 1 /CS /G /F /CCF /DP << /K -1 /Columns 13 >> >>", ".in": encoded,
                 ".out": bytes(1-v for row in rows for v in row), ".params": b"kind=inline\noutcome=valid\n"}, ("image-inline", "image-ccitt"), "Legacy inline CCITT boundary with no Length")


def eob_jpeg(run_count=4, restart=0):
    data = b"\xff\xd8"+segment(0xdb, b"\0"+bytes([1])*64)
    data += segment(0xc2, bytes([8])+struct.pack(">HHB", 8, 32, 1)+bytes([1, 0x11, 0]))
    data += segment(0xc4, b"\0"+bytes([0,0,0,len(DC)]+[0]*12)+bytes(DC)
                    +b"\x10"+bytes([0]*7+[len(AC)]+[0]*8)+bytes(AC))
    wire = Wire()
    for _ in range(4):
        dc_value(wire, 0)
    data += segment(0xda, bytes([1,1,0,0,0,0]))+wire.bytes(True)
    if restart:
        data += segment(0xdd, struct.pack(">H", restart))
    wire = Wire()
    r = run_count.bit_length()-1
    huff(wire, r*16)
    wire.put(run_count-(1<<r), r)
    data += segment(0xda, bytes([1,1,0,1,63,1]))+wire.bytes(True)
    # AC refinement EOB run spans the same four blocks.
    wire = Wire(); huff(wire, 0x20); wire.put(0,2)
    return data+segment(0xda,bytes([1,1,0,1,63,0x10]))+wire.bytes(True)+b"\xff\xd9"


def cases():
    yield from dnl_cases()
    from .jpx import marker_render_cases
    yield from marker_render_cases()
    from . import jpeg as jpeg_models
    yield from jpeg_models.cases()
    yield from uncompressed_fax_cases()
    yield from uncompressed_fax_render_cases()
    from .jbig2 import extended_template_render_cases
    yield from extended_template_render_cases()
    yield from raw_cases()
    yield from dictionary_cases()
    yield from iso_review_cases()
    yield from image_codec_cases()
    yield from tolerance_cases()
    yield from dictionary_repair_cases()
    yield from fax_grid_cases()
    yield from inline_filter_cases()
    yield from jpx_options_cases()
    for name, options in [
        ("jpeg-baseline-gray", {}), ("jpeg-extended", dict(extended=True)),
        ("jpeg-baseline-rgb", dict(n=3)), ("jpeg-rgb-direct", dict(n=3, transform=0)),
        ("jpeg-cmyk", dict(n=4)), ("jpeg-ycck", dict(n=4, adobe=2)),
        ("jpeg-adobe-overrides", dict(n=3, adobe=0, transform=1)),
        ("jpeg-420", dict(n=3, sampling=[(2, 2), (1, 1), (1, 1)])),
        ("jpeg-progressive", dict(progressive=True)),
        ("jpeg-progressive-rgb", dict(progressive=True, n=3)),
        ("jpeg-progressive-restart", dict(progressive=True, restart=1)),
        ("jpeg-progressive-band-refinement", dict(progressive=True, combined_refinement=True)),
        ("jpeg-interleaved-420", dict(width=17, height=17, n=3, sampling=[(2,2),(1,1),(1,1)], interleaved=True)),
        ("jpeg-interleaved-progressive", dict(width=17, height=17, n=3, sampling=[(2,2),(1,1),(1,1)], interleaved=True, progressive=True, combined_refinement=True, restart=1)),
        ("jpeg-restart", dict(restart=1)),
    ]:
        encoded, expected = jpeg(**options)
        yield vector(name, "dct", encoded, expected, tolerance=1, transform=options.get("transform", -1))
    encoded, _ = jpeg()
    yield vector("jpeg-truncated", "dct", encoded[:-8], outcome="syntax")
    yield vector("jpeg-missing-scan", "dct", encoded[:encoded.index(b"\xff\xda")]+b"\xff\xd9", outcome="syntax")
    huge = encoded.replace(b"\x00\x09\x00\x0d\x01", b"\xff\xff\xff\xff\x01", 1)
    yield vector("jpeg-huge", "dct", huge, outcome="limit", requirement="image-limits")
    at = encoded.index(b"\xff\xc4")
    bad = encoded[:at]+segment(0xc4, b"\0"+bytes([3]+[0]*15)+b"\0\1\2")+encoded[at:]
    yield vector("jpeg-bad-huffman", "dct", bad, outcome="syntax")
    rows = [[(x//3+y)%2 for x in range(13)] for y in range(6)]
    for k in (0, 3, -1):
        for align in (False, True):
            for black in (False, True):
                for eob in (False, True):
                    encoded, expected = ccitt(rows, k=k, eol=k>=0, align=align, eob=eob, black_is_1=black)
                    yield vector(f"fax-{k}-{int(align)}-{int(black)}-{int(eob)}", "ccitt", encoded, expected,
                                 k=k, align=int(align), black=int(black), eob=int(eob), eol=int(k>=0), width=13, height=6)
    for name, code, expected in [
        ("fax-g4-white", "1", b"\xff\xf8"),
        ("fax-g4-black", "001"+WHITE[0]+BLACK[13], b"\0\0"),
        ("fax-g4-vertical", "001"+WHITE[4]+BLACK[4]+"1"+"011"+"010"+"1", b"\xf0\xf8\xf9\xf8"),
        ("fax-g4-pass", "001"+WHITE[4]+BLACK[4]+"1"+"0001"+"1", b"\xf0\xf8\xff\xf8"),
    ]:
        wire = Wire(); wire.code(code+"000000000001"*2)
        yield vector(name, "ccitt", wire.bytes(), expected, k=-1, width=13, height=len(expected)//2, eob=1)
    encoded, _ = ccitt(rows, k=-1)
    yield vector("fax-truncated", "ccitt", encoded[:-4], outcome="syntax", k=-1, width=13, height=6, eob=1)
    long_rows = [[0]*64+[1]*13, [1]*64+[0]*13]
    encoded, expected = ccitt(long_rows)
    yield vector("fax-makeup", "ccitt", encoded, expected, k=0, width=77, height=2, eob=1)

    for length in list(range(1,64))+list(range(64,2561,64))+[5504]:
        for black in (False, True):
            row = [int(black)]*length
            encoded, expected = ccitt([row])
            yield vector(f"fax-run-{length}-{int(black)}", "ccitt", encoded, expected, k=0, width=length, height=1, eob=1)
    for offset in range(-3, 4):
        rows = [[int(5 <= x < 12) for x in range(23)],
                [int(5+offset <= x < 12+offset) for x in range(23)]]
        encoded, expected = ccitt(rows, k=-1)
        yield vector(f"fax-vertical-{offset}", "ccitt", encoded, expected, k=-1, width=23, height=2, eob=1)
    for k in (-1, 4):
        rows = [[int((x*x+3*y*y+x*y)%19 < 8) for x in range(73)] for y in range(17)]
        encoded, expected = ccitt(rows, k=k)
        yield vector(f"fax-complex-{k}", "ccitt", encoded, expected, k=k, width=73, height=17, eob=1)

    yield vector("jpeg-progressive-eob-run", "dct", eob_jpeg(), bytes([128])*256)
    yield vector("jpeg-eob-overrun", "dct", eob_jpeg(5), outcome="syntax")
    yield vector("jpeg-eob-cross-restart", "dct", eob_jpeg(restart=2), outcome="syntax")
    for name in ("jpeg-baseline-gray", "jpeg-progressive"):
        encoded, _ = jpeg(progressive=name == "jpeg-progressive")
        at = encoded.index(b"\xff\xc0" if name == "jpeg-baseline-gray" else b"\xff\xc2")
        yield vector(name+"-precision-9", "dct", encoded[:at+4]+bytes([9])+encoded[at+5:], outcome="unsupported", feature="jpeg.precision")
    encoded, _ = jpeg(restart=1)
    yield vector("jpeg-bad-restart", "dct", encoded.replace(b"\xff\xd0", b"\xff\xd1", 1), outcome="syntax")

    wire = Wire()
    good = WHITE[4]+BLACK[4]
    eol = "000000000001"
    for row in (good, WHITE[15], WHITE[15], good):
        wire.code(eol+row)
    wire.code(eol*6)
    yield vector("fax-damaged-rows", "ccitt", wire.bytes(), bytes([0xf0,0xf0,0xff,0xf0]),
                 k=0, width=8, height=4, eob=1, eol=1, damaged=2, outcome="repair")
    rows = [[int((x+y)%5 == 0) for x in range(13)] for y in range(6)]
    encoded, expected = ccitt(rows, eob=False)
    yield vector("fax-unknown-rows", "ccitt", encoded, expected, k=0, width=13, height=0, eob=0)
    encoded, expected = ccitt(rows, eol=True)
    yield vector("fax-rows-overridden", "ccitt", encoded, expected, k=0, width=13, height=1, eob=1, eol=1)
    wire = Wire(); wire.code("00000000"+eol+WHITE[8]+eol*6)
    yield vector("fax-optional-eol-fill", "ccitt", wire.bytes(), b"\xff", k=0, width=8, height=1, eob=1, eol=0)


def iso_review_cases():
    """PDF wrapper expectations derived from the local ISO text, not codecs."""
    base = [b"<< /Type /Catalog /Pages 2 0 R >>", b"<< /Type /Pages /Kids [] /Count 0 >>"]
    common = b"/Type /XObject /Subtype /Image /Width 2 /Height 1 /ColorSpace /DeviceGray"
    for name, dictionary, data, outcome in [
        ("iso-runlength-eight-bit", common+b" /BitsPerComponent 8 /Filter /RunLengthDecode", b"\x01\x0c\x1a\x80", "valid"),
        ("iso-runlength-one-bit-rejected", common+b" /BitsPerComponent 1 /Filter /RunLengthDecode", b"\x00\x40\x80", "syntax"),
        ("iso-xobject-subtype-required", common.replace(b"/Subtype /Image", b"")+b" /BitsPerComponent 8", bytes([12,26]), "syntax"),
    ]:
        yield Vector(name, {".pdf": document(base+[stream(data,dictionary)]), ".out": bytes([12,26]),
                     ".params": f"kind=dictionary\noutcome={outcome}\n".encode()},
                     ("image-dictionaries",), "ISO 32000-2 Table 87/8.9.7 filter and image type consistency")
    for name, dictionary, data, expected, outcome, resources in [
        ("iso-inline-direct-lab-rejected", b"<< /W 1 /H 1 /BPC 8 /CS [/Lab << /WhitePoint [1 1 1] >>] >>", b"\0\0\0", b"", "syntax", None),
        ("iso-inline-resource-lab", b"<< /W 1 /H 1 /BPC 8 /CS /L1 /L 3 >>", bytes([1,2,3]), bytes([1,2,3]), "valid", b"<< /ColorSpace << /L1 [/Lab << /WhitePoint [1 1 1] >>] >> >>"),
        ("iso-inline-indexed-stream-lookup-rejected", b"<< /W 1 /H 1 /BPC 8 /CS [/I /RGB 0 3 0 R] >>", b"\0", b"", "syntax", None),
        ("iso-inline-runlength-one-bit-rejected", b"<< /W 2 /H 1 /BPC 1 /CS /G /F /RL >>", b"\0\x40\x80", b"", "syntax", None),
        ("iso-inline-ascii-length-whitespace", b"<< /W 1 /H 1 /BPC 8 /CS /G /F /AHx /L 3 >>", b" \n0F>", bytes([15]), "valid", None),
        ("iso-inline-unknown-entry-ignored", b"<< /W 2 /H 1 /BPC 8 /CS /G /Unknown [1 2 3] >>", bytes([12,26]), bytes([12,26]), "valid", None),
    ]:
        files = {".dict": dictionary, ".in": data, ".out": expected,
                 ".params": f"kind=inline\noutcome={outcome}\n".encode()}
        if resources:
            files[".resources"] = resources
        yield Vector(name, files, ("image-inline",), "ISO 32000-2 8.9.7 direct/resource colour-space rules")
    for name, options, outcome in [
        ("iso-jpeg-gray-transform-ignored", dict(transform=99), "valid"),
        ("iso-jpeg-adobe-transform-ignored", dict(n=3, adobe=0, transform=99), "valid"),
        ("iso-jpeg-invalid-transform", dict(n=3, transform=99), "syntax"),
    ]:
        encoded, expected = jpeg(**options)  # pyright: ignore[reportArgumentType]
        yield vector(name, "dct", encoded, expected, outcome=outcome, tolerance=1, transform=99)
    rows = [[int((x+y)%3 == 0) for x in range(13)] for y in range(6)]
    encoded, expected = ccitt(rows, k=3)
    yield vector("iso-fax-positive-k-equivalence", "ccitt", encoded, expected,
                 k=1, width=13, height=6, eob=1)


def image_pdf_vector(name, encoded, expected, dictionary, *, requirement, width, height, components=1, bits=8, globals=None, alpha=None, extras=(), trailer=b"", outcome="valid", **options):
    """Complete PDF wrapper with a directly calculated sample oracle."""
    objects = [b"<< /Type /Catalog /Pages 2 0 R >>", b"<< /Type /Pages /Kids [] /Count 0 >>",
               stream(encoded, b"/Type /XObject /Subtype /Image /Width %d /Height %d " % (width,height)+dictionary)]
    if globals is not None:
        # Globals is an ordinary PDF stream, decoded before the image codec.
        objects.append(stream(zlib.compress(globals), b"/Filter /FlateDecode"))
    objects.extend(extras)
    if b"4 0 R" in dictionary and globals is None and not extras:
        objects.append(b"4 0 R")  # Deliberate ignored-value cycle.
    params = dict(kind="pdf-image", outcome=outcome, width=width, height=height, components=components, bits=bits, **options)
    files = {".pdf": document(objects, trailer), ".out": " ".join(str(v) for v in expected).encode(),
             ".params": "".join(f"{k}={v}\n" for k,v in params.items()).encode()}
    if alpha is not None:
        files[".alpha"] = " ".join(str(v) for v in alpha).encode()
    return Vector(name, files, (requirement,), name.replace("-"," "))


def image_codec_cases():
    # Reuse independent Python format encoders, never the product decoders.
    from .jpx import codestream, jp2, box
    from . import jbig2
    width, height = 8, 4
    gray = [(x*19+y*7)%256 for y in range(height) for x in range(width)]
    raw = codestream([gray],width,height)
    wrapped = jp2(raw,width,height,1)
    for name, dictionary, outcome, space, decode in [
        ("jpx-container-gray", b"/Filter /JPXDecode", "valid", "DeviceGray", "none"),
        ("jpx-dictionary-depth-ignored", b"/Filter /JPXDecode /BitsPerComponent 4 0 R", "valid", "DeviceGray", "none"),
        ("jpx-decode-ignored", b"/Filter /JPXDecode /Decode 4 0 R", "valid", "DeviceGray", "none"),
        ("jpx-explicit-decode", b"/Filter /JPXDecode /ColorSpace /DeviceGray /BitsPerComponent 1 /Decode [1 0]", "valid", "DeviceGray", "1,0"),
        ("jpx-colour-components-mismatch", b"/Filter /JPXDecode /ColorSpace /DeviceRGB", "syntax", "", ""),
        ("jpx-alpha-missing", b"/Filter /JPXDecode /SMaskInData 1", "syntax", "", ""),
        ("jpx-alpha-invalid-mode", b"/Filter /JPXDecode /SMaskInData 3", "syntax", "", ""),
        ("jpx-dimensions-mismatch", b"/Filter /JPXDecode", "syntax", "", ""),
    ]:
        yield image_pdf_vector(name,wrapped,gray,dictionary,requirement="image-jpx",width=width,height=height+(name == "jpx-dimensions-mismatch"),outcome=outcome,space=space,decode=decode)
    yield image_pdf_vector("jpx-raw-codestream-rejected",raw,[],b"/Filter /JPXDecode /ColorSpace /DeviceGray",requirement="image-jpx",width=width,height=height,outcome="syntax")
    icc = jp2(raw,width,height,1,method=2)
    yield image_pdf_vector("jpx-container-icc",icc,gray,b"/Filter /JPXDecode",requirement="image-jpx",width=width,height=height,space="icc")
    rgb = [[(i*13+c*31)%256 for i in range(width*height)] for c in range(3)]
    rgb_bytes = [v for pixel in zip(*rgb) for v in pixel]
    rgb_stream = jp2(codestream(rgb,width,height),width,height,3)
    for colour,space,planes in [(12,"DeviceCMYK",rgb+[gray]),(18,"sYCC",rgb),(19,"forbidden",rgb)]:
        payload = jp2(codestream(planes,width,height),width,height,len(planes))
        payload = payload.replace(box(b"colr",b"\1\0\0"+struct.pack(">I",16)),box(b"colr",b"\1\0\0"+struct.pack(">I",colour)),1)
        yield image_pdf_vector(f"jpx-container-colour-{colour}",payload,[v for pixel in zip(*planes) for v in pixel],b"/Filter /JPXDecode",requirement="image-jpx",width=width,height=height,components=len(planes),space=space,outcome="syntax" if colour == 19 else "valid")
    # Same number of components: the PDF colour space overrides the JP2 colr.
    yield image_pdf_vector("jpx-dictionary-colour-override",rgb_stream,rgb_bytes,b"/Filter /JPXDecode /ColorSpace [/Lab << /WhitePoint [1 1 1] >>]",requirement="image-jpx",width=width,height=height,components=3,space="Lab")
    alpha = [(i*7)%256 for i in range(width*height)]
    definitions = struct.pack(">H",4)+b"".join(struct.pack(">HHH",*v) for v in [(0,0,1),(1,0,2),(2,0,3),(3,1,0)])
    alpha_stream = jp2(codestream(rgb+[alpha],width,height),width,height,4,extra=box(b"cdef",definitions))
    for mode in (0,1):
        yield image_pdf_vector(f"jpx-alpha-mode-{mode}",alpha_stream,rgb_bytes,b"/Filter /JPXDecode /SMaskInData %d" % mode,
                               requirement="image-jpx",width=width,height=height,components=3,space="DeviceRGB",mask="none" if mode == 0 else "embedded",alpha=alpha if mode else None,alpha_bits=8)
    yield image_pdf_vector("jpx-alpha-overrides-mask",alpha_stream,rgb_bytes,b"/Filter /JPXDecode /SMaskInData 1 /Mask [0]",requirement="image-jpx",width=width,height=height,components=3,space="DeviceRGB",mask="embedded",alpha=alpha,alpha_bits=8)
    yield image_pdf_vector("jpx-alpha-conflicts-smask",alpha_stream,[],b"/Filter /JPXDecode /SMaskInData 1 /SMask 4 0 R",requirement="image-jpx",width=width,height=height,components=3,outcome="syntax")
    # Palette components can have distinct precisions even when the index
    # codestream has one common precision (7.4.9 and 8.9.5.2).
    palette = [(0,3,1023),(1,7,513)]
    pclr = struct.pack(">HB",2,3)+bytes([0,2,9])+b"".join(bytes([a,b])+struct.pack(">H",c) for a,b,c in palette)
    cmap = b"".join(struct.pack(">HBB",0,1,c) for c in range(3))
    indices = [i%2 for i in range(width*height)]
    mixed = jp2(codestream([indices],width,height),width,height,1,extra=box(b"pclr",pclr)+box(b"cmap",cmap))
    mixed = mixed.replace(box(b"colr", b"\1\0\0"+struct.pack(">I",17)), box(b"colr", b"\1\0\0"+struct.pack(">I",16)), 1)
    yield image_pdf_vector("jpx-mixed-precision",mixed,[v for i in indices for v in palette[i]],b"/Filter /JPXDecode",requirement="image-jpx",width=width,height=height,components=3,bits=0,component_bits="1,3,10",space="DeviceRGB")
    words = [(i*3001)%65536 for i in range(width*height)]
    word_stream = jp2(codestream([words],width,height,bits=16),width,height,1,bits=16)
    yield image_pdf_vector("jpx-sixteen-bit",word_stream,words,b"/Filter /JPXDecode /BitsPerComponent false",requirement="image-jpx",width=width,height=height,bits=16,space="DeviceGray")
    # JP2's colr is still part of the file; dictionary ColorSpace overrides it
    # only for non-stencil images. A stencil needs no colour interpretation.
    stencil = [i%2 for i in range(width*height)]
    mask_stream = jp2(codestream([stencil],width,height,bits=1),width,height,1,bits=1)
    yield image_pdf_vector("jpx-stencil",mask_stream,stencil,b"/Filter /JPXDecode /ImageMask true /Decode [1 0]",requirement="image-jpx",width=width,height=height,bits=1,space="stencil",decode="1,0")
    for black in (False,True):
        rows = [[int(black) for _ in range(width)] for _ in range(height)]
        data = jbig2.segment(1,48,1,jbig2.page_info(width,height,flags=64+4*int(black)))
        yield image_pdf_vector(f"jbig2-default-{'black' if black else 'white'}",data,[1-v for row in rows for v in row],b"/Filter /JBIG2Decode /BitsPerComponent 1 /ColorSpace /DeviceGray",requirement="image-jbig2",width=width,height=height,bits=1,space="DeviceGray")
    glyph = [[1,0,1],[0,1,0],[1,0,1]]
    body,symbols = jbig2.dictionary([glyph],2)
    globals = jbig2.segment(0,0,0,body)
    region,rows = jbig2.text_region(symbols,[(1,0,0),(4,1,0)],width,height)
    data = jbig2.segment(1,48,1,jbig2.page_info(width,height))+jbig2.segment(2,6,1,region,refs=(0,))
    for filtered in (False,True):
        encoded = data.hex().encode()+b">" if filtered else data
        dictionary = (b"/Filter [/ASCIIHexDecode /JBIG2Decode] /DecodeParms [null << /JBIG2Globals 4 0 R >>]" if filtered else b"/Filter /JBIG2Decode /DecodeParms << /JBIG2Globals 4 0 R >>")+b" /BitsPerComponent 1 /ColorSpace /DeviceGray"
        yield image_pdf_vector(f"jbig2-globals-{'chain' if filtered else 'direct'}",encoded,[1-v for row in rows for v in row],dictionary,requirement="image-jbig2",width=width,height=height,bits=1,space="DeviceGray",globals=globals)
    for codec in (b"JBIG2Decode",b"JPXDecode",b"Crypt"):
        yield Vector("inline-forbidden-"+codec.decode(), {".dict": b"<< /W 1 /H 1 /BPC 8 /CS /G /F /"+codec+b" >>", ".in": b"", ".out": b"", ".params": b"kind=inline\noutcome=syntax\n"}, ("image-inline",), "ISO 32000-2 8.9.7 forbids this inline filter")


# T.81 B.2.4.2/C: a one-symbol DC=0 table and AC EOB table encode
# constant level-shifted zero coefficients. The inverse DCT is exactly 128.
def constant_jpeg(n=1, padding=1, restart=False, progressive=False, extension=b'', adobe=False):
    width = 16 if restart else 1
    result = b'\xff\xd8'
    if adobe:
        result += segment(0xee, b'Adobe'+struct.pack('>HHH',100,0,0)+b'\0'+extension)
    result += segment(0xdb, b'\0'+bytes([1])*64)
    result += segment(0xc2 if progressive else 0xc0, bytes([8])+struct.pack('>HHB',1,width,n)+b''.join(bytes([i+1,17,0]) for i in range(n)))
    table = bytes([1]+[0]*15)+b'\0'
    result += segment(0xc4,b'\0'+table+b'\x10'+table)
    if restart:
        result += segment(0xdd,b'\0\1')
    scans = [(0,0),(1,63)] if progressive else [(0,63)]
    for start,end in scans:
        result += segment(0xda,bytes([n])+b''.join(bytes([i+1,0]) for i in range(n))+bytes([start,end,0]))
        entropy = '0'*(n if progressive else 2*n)
        byte = int(entropy+str(padding)*(-len(entropy)%8),2)
        result += bytes([byte])
        if restart:
            result += b'\xff\xd0'+bytes([byte])
    return result+b'\xff\xd9', [128]*(width*n)


def tolerance_cases():
    # ISO 8.9.3 defines byte-padded rows; 8.9.5.1 requires consistent data.
    # Zero-byte fill and discarding the suffix are reported reader policies.
    for bits in (1, 2, 4, 8, 16):
        width, height = 3, 2
        wire = Wire()
        for y in range(height):
            for x in range(width):
                wire.put(((y*width+x)*37+11) % (1 << bits), bits)
            wire.align()
        original = wire.bytes()
        stride = (width*bits+7)//8
        for suffix, data, repair in [('short', original[:-1], 'image.data-short'),
                                     ('excess', original+b'\xaa\xff', 'image.data-excess')]:
            padded = (data[:stride*height]+bytes(stride*height))[:stride*height]
            expected = [((int.from_bytes(padded[y*stride:(y+1)*stride], 'big') >>
                         (8*stride-(x+1)*bits)) & ((1 << bits)-1))
                        for y in range(height) for x in range(width)]
            yield image_pdf_vector(f'image-data-{suffix}-{bits}', data, expected,
                b'/ColorSpace /DeviceGray /BitsPerComponent %d' % bits,
                requirement='image-length-repairs', width=width, height=height,
                bits=bits, outcome='repair', repair=repair, space='DeviceGray')
    yield image_pdf_vector('image-data-empty', b'', [0]*6,
        b'/ColorSpace /DeviceGray /BitsPerComponent 8', requirement='image-length-repairs',
        width=3, height=2, outcome='repair', repair='image.data-short', space='DeviceGray')

    for suffix, options in [('scan',{}),('restart',dict(restart=True)),('progressive',dict(progressive=True))]:
        encoded, expected = constant_jpeg(padding=0,**options)
        yield image_pdf_vector('jpeg-padding-'+suffix,encoded,expected,
            b'/Filter /DCTDecode /ColorSpace /DeviceGray /BitsPerComponent 8',
            requirement='image-jpeg-repairs',width=16 if options.get('restart') else 1,height=1,
            outcome='repair',repair='jpeg.entropy-padding',space='DeviceGray')
    encoded, expected = constant_jpeg(n=3,adobe=True,extension=b'\x03\0\x15\x04')
    yield image_pdf_vector('jpeg-adobe-extension',encoded,expected,
        b'/Filter /DCTDecode /ColorSpace /DeviceRGB /BitsPerComponent 8 /DecodeParms << /ColorTransform 99 >>',
        requirement='image-jpeg-repairs',width=1,height=1,components=3,
        outcome='repair',repair='jpeg.adobe-marker',space='DeviceRGB')
    repeated = encoded[:2]+segment(0xee,b'Adobe'+struct.pack('>HHH',100,0,0)+b'\0extra')*8+encoded[2:]
    yield image_pdf_vector('jpeg-adobe-extensions-repeated',repeated,expected,
        b'/Filter /DCTDecode /ColorSpace /DeviceRGB /BitsPerComponent 8',
        requirement='image-jpeg-repairs',width=1,height=1,components=3,
        outcome='repair',repair='jpeg.adobe-marker',space='DeviceRGB')
    for suffix, marker in [('short',b'Adobe\0'),('transform',b'Adobe'+bytes(6)+b'\3')]:
        normal, _ = constant_jpeg()
        yield image_pdf_vector('jpeg-adobe-invalid-'+suffix,normal[:2]+segment(0xee,marker)+normal[2:],[],
            b'/Filter /DCTDecode /ColorSpace /DeviceGray /BitsPerComponent 8',
            requirement='image-jpeg-repairs',width=1,height=1,outcome='syntax')

    # T.6 2.4.1.1-2 places zero octet fill after EOFB. These encoders
    # instead put seven zeros between the sole V(0) white row and EOFB.
    yield image_pdf_vector('fax-padding-before-eofb',bytes.fromhex('80001001'),[1],
        b'/Filter /CCITTFaxDecode /ColorSpace /DeviceGray /BitsPerComponent 1 /DecodeParms << /Columns 1 /K -1 >>',
        requirement='image-fax-repairs',width=1,height=1,bits=1,outcome='repair',
        repair='ccitt.eob-padding',space='DeviceGray')
    wire = Wire()
    for row in ('00001111',None,None,'00001111'):
        wire.code('000000000001')
        if row is None:
            wire.code('0000000000000')
        else:
            run(wire,4,False); run(wire,4,True)
    wire.code('000000000001'*6)
    expected = [int(v) for row in ['11110000','11110000','11111111','11110000'] for v in row]
    yield image_pdf_vector('fax-damaged-rows-pdf',wire.bytes(),expected,
        b'/Filter /CCITTFaxDecode /ColorSpace /DeviceGray /BitsPerComponent 1 /DecodeParms << /Columns 8 /K 0 /EndOfLine true /DamagedRowsBeforeError 2 >>',
        requirement='image-fax-repairs',width=8,height=4,bits=1,outcome='repair',
        repair='ccitt.damaged-row',reports=2,space='DeviceGray')

    # Inline filters must send diagnostics to the same document sink.
    for name,dictionary,data,expected,repair in [
        ('inline-short',b'<< /W 3 /H 1 /BPC 8 /CS /G /L 2 >>',b'\x0c\x1a',bytes([12,26,0]),'image.data-short'),
        ('inline-checksum',b'<< /W 2 /H 1 /BPC 8 /CS /G /F /Fl /L 10 >>',zlib.compress(bytes([12,26]))[:-1]+bytes([0]),bytes([12,26]),'filter.checksum-mismatch'),
    ]:
        yield Vector(name,{'.dict':dictionary,'.in':data,'.out':expected,
            '.params':f'kind=inline\noutcome=repair\nrepair={repair}\n'.encode()},
            ('image-inline',),name.replace('-',' '))


def jpx_options_cases():
    from .jpx import codestream, jp2, box, marker
    width,height = 3,2
    planes = [[10+c*40+i for i in range(width*height)] for c in range(3)]
    expected = [v for pixel in zip(*planes) for v in pixel]
    base = jp2(codestream(planes,width,height),width,height,3)
    original = box(b'colr',b'\1\0\0'+struct.pack('>I',16))
    for name, replacement, dictionary in [
        ('jpx-overrides-vendor-colour',box(b'colr',b'\4\0\0vend'),b'/ColorSpace /DeviceRGB'),
        ('jpx-overrides-ciejab',box(b'colr',b'\1\0\0'+struct.pack('>I',19)),b'/ColorSpace /DeviceRGB'),
        ('jpx-overrides-colour-priority',box(b'colr',b'\1\x80\x7f'+struct.pack('>I',17)),b'/ColorSpace /DeviceRGB'),
        # T.800 I.5.3.3 ignores PREC/APPROX in JP2. JPX ranking needs
        # the distinct Part 2 container contract and is not asserted here.
        ('jpx-jp2-ignores-priority',box(b'colr',b'\1\x80\x7f'+struct.pack('>I',16)),b''),
    ]:
        yield image_pdf_vector(name,base.replace(original,replacement,1),expected,
            b'/Filter /JPXDecode '+dictionary,requirement='image-jpx',width=width,height=height,
            components=3,space='DeviceRGB')

    alpha = [0,32,64,128,192,255]
    shuffled = [alpha,planes[2],planes[0],planes[1]]
    defs = struct.pack('>H',4)+b''.join(struct.pack('>HHH',*r) for r in [(0,2,0),(1,0,3),(2,0,1),(3,0,2)])
    for mode in (0,2):
        encoded = jp2(codestream(shuffled,width,height),width,height,4,extra=box(b'cdef',defs))
        yield image_pdf_vector(f'jpx-premultiplied-mode-{mode}',encoded,expected,
            b'/Filter /JPXDecode /ColorSpace /DeviceRGB /SMaskInData %d' % mode,
            requirement='image-jpx',width=width,height=height,components=3,space='DeviceRGB',
            mask='embedded' if mode else 'none',alpha=alpha if mode else None,alpha_bits=8,premultiplied=int(mode==2))
    defs = struct.pack('>H',5)+b''.join(struct.pack('>HHH',*r) for r in [(0,0,1),(1,0,2),(2,0,3),(3,1,1),(4,1,2)])
    encoded = jp2(codestream(planes+[alpha,alpha],width,height),width,height,5,extra=box(b'cdef',defs))
    yield image_pdf_vector('jpx-ignored-per-colour-opacity',encoded,expected,
        b'/Filter /JPXDecode /ColorSpace /DeviceRGB /SMaskInData 0',requirement='image-jpx',
        width=width,height=height,components=3,space='DeviceRGB')

    # T.800 B.2 equations B-1/B-2 and G.4 give each component's grid.
    # Expansion uses a constant sample on its step-wide rectangle, with
    # border replication before the first sample. This is a resampling choice.
    origin, extent = (3,5),(11,12)
    steps = [(1,1),(2,3),(3,2),(2,2)]
    payloads, components, geometries = [],[],[]
    for c,(dx,dy) in enumerate(steps):
        start = tuple((v+d-1)//d for v,d in zip(origin,(dx,dy)))
        end = tuple((v+d-1)//d for v,d in zip(extent,(dx,dy)))
        w,h = (end[i]-start[i] for i in range(2))
        values = [(c*37+i*13)%256 for i in range(w*h)]
        raw = codestream([values],w,h,origin=start)
        payloads.append(raw[raw.index(b'\xff\x93')+2:-2])
        components.append(values)
        geometries.append((dx,dy,start[0]*dx,start[1]*dy,w,h))
    siz = struct.pack('>H8IH',0,*extent,*origin,8,7,*origin,4)+b''.join(bytes([7,dx,dy]) for dx,dy in steps)
    header_source = codestream([[128]],1,1)
    coding = header_source[header_source.index(b'\xff\x52'):header_source.index(b'\xff\x90')]
    payload = b''.join(payloads)
    raw = b'\xff\x4f'+marker(0x51,siz)+coding+marker(0x90,struct.pack('>HIBB',0,14+len(payload),0,1))+b'\xff\x93'+payload+b'\xff\xd9'
    defs = struct.pack('>H',4)+b''.join(struct.pack('>HHH',*r) for r in [(0,0,1),(1,0,2),(2,0,3),(3,1,0)])
    encoded = jp2(raw,8,7,4,extra=box(b'cdef',defs))
    expanded = []
    for values,(dx,dy,x0,y0,w,h) in zip(components,geometries):
        expanded.append([values[min(h-1,max(0,(origin[1]+y-y0)//dy))*w+min(w-1,max(0,(origin[0]+x-x0)//dx))] for y in range(7) for x in range(8)])
    yield image_pdf_vector('jpx-reference-grid-alpha',encoded,[v for pixel in zip(*expanded[:3]) for v in pixel],
        b'/Filter /JPXDecode /SMaskInData 1',requirement='image-jpx-placement',width=8,height=7,
        components=3,space='DeviceRGB',mask='embedded',alpha=expanded[3],alpha_bits=8)


def dictionary_repair_cases():
    """ISO Table 5/87/92 and T.81 B.2.1, with direct sample expectations."""
    import base64
    from .codec import lzw
    from .jpx import codestream, jp2

    gray = b'/ColorSpace /DeviceGray /BitsPerComponent 8'
    plain = bytes([12, 26])
    for short, encoded in [
        ('AHx', plain.hex().encode()+b'>'),
        ('A85', base64.a85encode(plain)+b'~>'),
        ('LZW', lzw(plain, 1)),
        ('Fl', zlib.compress(plain)),
        ('RL', b'\x01'+plain+b'\x80'),
    ]:
        name = 'iso-xobject-filter-abbreviation' if short == 'RL' else 'xobject-filter-'+short.lower()
        yield image_pdf_vector(name, encoded, list(plain), gray+b'/Filter /'+short.encode(),
            requirement='image-filter-abbreviation', width=2, height=1,
            outcome='repair', repair='image.filter-abbreviation', space='DeviceGray')
    encoded, expected = constant_jpeg()
    yield image_pdf_vector('xobject-filter-dct', encoded, expected, gray+b'/Filter /DCT',
        requirement='image-filter-abbreviation', width=1, height=1,
        outcome='repair', repair='image.filter-abbreviation', space='DeviceGray')
    encoded, _ = ccitt([[0]], k=-1)
    yield image_pdf_vector('xobject-filter-ccf', encoded, [1],
        b'/ColorSpace /DeviceGray /BitsPerComponent 1 /Filter /CCF /DecodeParms << /K -1 /Columns 1 >>',
        requirement='image-filter-abbreviation', width=1, height=1, bits=1,
        outcome='repair', repair='image.filter-abbreviation', space='DeviceGray')
    yield image_pdf_vector('xobject-filter-alias-chain', zlib.compress(plain).hex().encode()+b'>', list(plain),
        gray+b'/Filter [/AHx /Fl] /DecodeParms [null null]',
        requirement='image-filter-abbreviation', width=2, height=1,
        outcome='repair', repair='image.filter-abbreviation', space='DeviceGray')
    yield image_pdf_vector('xobject-filter-unknown', plain, [], gray+b'/Filter /Unknown',
        requirement='image-filter-abbreviation', width=2, height=1,
        outcome='unsupported', feature='filter.unknown', reject_repair=1)

    # Stencil meaning comes from ImageMask and Decode, independent of the
    # discarded colour entry. No colour lookup is needed, even for a cycle.
    stencil = b'/ImageMask true /BitsPerComponent 1 '
    for suffix, space, decode in [
        ('gray', b'/DeviceGray', b'[0 1]'),
        ('rgb-inverted', b'/DeviceRGB', b'[1 0]'),
        ('wrong-type', b'42', b'[0 1]'),
        ('cycle', b'4 0 R', b'[0 1]'),
    ]:
        yield image_pdf_vector('stencil-color-space-'+suffix, b'\x40', [0, 1],
            stencil+b'/ColorSpace '+space+b' /Decode '+decode,
            requirement='image-stencil-color-space', width=2, height=1, bits=1,
            outcome='repair', repair='image.stencil-color-space', space='stencil',
            inverted=int(decode == b'[1 0]'), decode='1,0' if decode == b'[1 0]' else '0,1')
    for suffix, extra in [
        ('precision', b'/BitsPerComponent 2'), ('mask', b'/Mask [0 1]'),
        ('soft-mask', b'/SMask 4 0 R'), ('decode', b'/Decode [0 0]'),
    ]:
        yield image_pdf_vector('stencil-invalid-'+suffix, b'\x40', [],
            b'/ImageMask true /ColorSpace /DeviceRGB '+extra,
            requirement='image-stencil-color-space', width=2, height=1, bits=1,
            outcome='syntax', reject_repair=1)
    yield image_pdf_vector('stencil-null-color-space', b'\x40', [0, 1],
        stencil+b'/ColorSpace 4 0 R', extras=(b'null',),
        requirement='image-stencil-color-space', width=2, height=1, bits=1,
        space='stencil', inverted=0)
    yield Vector('inline-stencil-color-space', {'.dict': b'<< /W 2 /H 1 /IM true /CS /RGB /D [1 0] >>',
        '.in': b'\x40', '.out': bytes([0, 1]),
        '.params': b'kind=inline\noutcome=repair\nrepair=image.stencil-color-space\nstencil=1\n'},
        ('image-stencil-color-space',), 'Inline stencil discards an extraneous colour space')

    # True null is normative absence (7.3.9 and Table 5), with no repair.
    wrapped = jp2(codestream([list(plain)], 2, 1), 2, 1, 1)
    for suffix, params, extras in [
        ('null', b'null', ()), ('array-null', b'[null]', ()),
        ('indirect-null', b'4 0 R', (b'null',)),
        ('array-indirect-null', b'[4 0 R]', (b'null',)),
        ('dictionary', b'<< /Quality 60 >>', ()),
    ]:
        yield image_pdf_vector('jpx-params-'+suffix, wrapped, list(plain),
            b'/Filter /JPXDecode /DecodeParms '+params, extras=extras,
            requirement='image-decode-params', width=2, height=1, space='DeviceGray')
    for suffix, params in [('boolean', b'true'), ('integer', b'42'), ('name', b'/None'),
                           ('string-null', b'(null)'), ('array-wrong-type', b'[false]')]:
        yield image_pdf_vector('jpx-params-'+suffix, wrapped, list(plain),
            b'/Filter /JPXDecode /DecodeParms '+params,
            requirement='image-decode-params', width=2, height=1,
            outcome='repair', repair='image.decode-params', space='DeviceGray')
    # The preceding filter's nondefault predictor must survive normalization.
    difference = bytes((v-(wrapped[i-1] if i else 0)) % 256 for i, v in enumerate(wrapped))
    yield image_pdf_vector('jpx-params-preserve-predictor', zlib.compress(b'\1'+difference), list(plain),
        b'/Filter [/FlateDecode /JPXDecode] /DecodeParms [<< /Predictor 15 /Columns %d >> 42]' % len(wrapped),
        requirement='image-decode-params', width=2, height=1,
        outcome='repair', repair='image.decode-params', space='DeviceGray')
    for suffix, params in [('wrong-count', b'[]'), ('cyclic', b'4 0 R')]:
        yield image_pdf_vector('jpx-params-'+suffix, wrapped, [],
            b'/Filter /JPXDecode /DecodeParms '+params,
            requirement='image-decode-params', width=2, height=1,
            outcome='limit' if suffix == 'cyclic' else 'syntax', reject_repair=1)
    yield image_pdf_vector('jpx-params-crypt-not-bypassed', wrapped, list(plain),
        b'/Filter [/Crypt /JPXDecode] /DecodeParms [null false]',
        requirement='image-decode-params', width=2, height=1, outcome='repair',
        repair='image.decode-params', space='DeviceGray')
    # Both ordinary and corrected parameters retain document-layer decryption.
    from .crypto import parameters, encrypt, encryption_dictionary, ID, hex_string
    encryption, key = parameters(2, b'', b'owner')
    for suffix, params, outcome in [('null', b'null', 'valid'), ('wrong-type', b'42', 'repair')]:
        yield image_pdf_vector('jpx-params-encrypted-'+suffix, encrypt(key, 'V2', 3, wrapped), list(plain),
            b'/Filter /JPXDecode /DecodeParms '+params,
            extras=(encryption_dictionary(encryption, 'V2', False),),
            trailer=b'/Encrypt 4 0 R /ID ['+hex_string(ID)+b' '+hex_string(ID)+b']',
            requirement='image-decode-params', width=2, height=1, outcome=outcome,
            space='DeviceGray', reject_repair=0,
            **({'repair': 'image.decode-params'} if outcome == 'repair' else {}))  # pyright: ignore[reportArgumentType]

    # Correcting a literal string requires decrypting that string too. The
    # samples are chosen directly; encryption uses the independent PDF oracle.
    for revision, method, explicit in [(2, 'V2', False), (4, 'AESV2', True)]:
        encryption, key = parameters(revision, b'', b'owner')
        null = hex_string(encrypt(key, method, 3, b'null'))
        # Nondefault PNG Sub must survive alongside the corrected JPX entry.
        encoded = zlib.compress(b'\1'+difference)
        filters = b'[/Crypt /FlateDecode /JPXDecode]' if explicit else b'[/FlateDecode /JPXDecode]'
        crypt = b'<< /Type /CryptFilterDecodeParms /Name /StdCF >> ' if explicit else b''
        params = b'['+crypt+b'<< /Predictor 15 /Columns %d >> ' % len(wrapped)+null+b']'
        security = encryption_dictionary(encryption, method, False)
        if explicit:
            # The explicit cipher must override Identity, not merely agree
            # with the default. Strings still use StdCF, including (null).
            security = security.replace(b'/StmF /StdCF', b'/StmF /Identity')
        yield image_pdf_vector('params-'+('crypt' if explicit else 'encrypted')+'-string-null',
            encrypt(key, method, 3, encoded), list(plain),
            b'/Filter '+filters+b' /DecodeParms '+params,
            extras=(security,),
            trailer=b'/Encrypt 4 0 R /ID ['+hex_string(ID)+b' '+hex_string(ID)+b']',
            requirement='image-parameter-override', width=2, height=1,
            outcome='repair', repair='image.decode-params', space='DeviceGray')
    rows = [[0, 1, 1, 0, 1]]
    fax, _ = ccitt(rows, k=-1)
    for suffix, first, extras, outcome in [
        ('null', b'null', (), 'valid'),
        ('string-null', b'(null)', (), 'repair'),
        ('indirect-string-null', b'4 0 R', (b'(null)',), 'repair'),
        ('wrong-integer', b'42', (), 'syntax'),
        ('unknown-string', b'(default)', (), 'syntax'),
    ]:
        yield image_pdf_vector('image-params-'+suffix, zlib.compress(fax), [1, 0, 0, 1, 0],
            b'/ImageMask true /Filter [/FlateDecode /CCITTFaxDecode] /DecodeParms ['+first+b' << /K -1 /Columns 5 >>]',
            extras=extras, requirement='image-decode-params', width=5, height=1, bits=1,
            outcome=outcome, repair='image.decode-params', space='stencil', reject_repair=int(outcome == 'syntax'))

    # One DC=0/EOB block reconstructs exactly 128. Complete encoded bytes
    # beyond that block are discarded only before EOI, never before a scan.
    for suffix, options, trailer in [
        ('one-byte', {}, b'\x3f'), ('stuffed', {}, b'\xff\x00\x12'),
        ('progressive', dict(progressive=True), b'\0\x55'),
        ('restart', dict(restart=True), b'\x3f'),
    ]:
        encoded, expected = constant_jpeg(**options)
        yield image_pdf_vector('jpeg-entropy-excess-'+suffix, encoded[:-2]+trailer+encoded[-2:], expected,
            gray+b'/Filter /DCTDecode', requirement='image-jpeg-entropy-excess',
            width=16 if options.get('restart') else 1, height=1,
            outcome='repair', repair='jpeg.entropy-excess', space='DeviceGray')
    encoded, _ = constant_jpeg(progressive=True)
    second_scan = encoded.find(b'\xff\xda', encoded.index(b'\xff\xda')+2)
    normal, _ = constant_jpeg()
    for suffix, bad in [
        ('before-scan', encoded[:second_scan]+b'\x55'+encoded[second_scan:]),
        ('before-restart', normal[:-2]+b'\x55\xff\xd0'+normal[-2:]),
        ('no-eoi', normal[:-2]+b'\x55'),
        ('stuffing', normal[:-2]+b'\x55\xff\xff\x00'+normal[-2:]),
    ]:
        yield image_pdf_vector('jpeg-entropy-excess-invalid-'+suffix, bad, [],
            gray+b'/Filter /DCTDecode', requirement='image-jpeg-entropy-excess',
            width=1, height=1, outcome='syntax', reject_repair=1)


def fax_grid_cases():
    """Table 11 owns encoded rows; Table 87 owns the painted sample grid.

    Right-side extension with fax white is an explicit recovery policy.
    Unused source byte-alignment bits have no pixel meaning.
    """
    for columns, width in [(5, 3), (5, 7), (9, 13), (13, 9)]:
        rows = [[int((x+2*y) % 4 < 2) for x in range(columns)] for y in range(3)]
        for k in [0, -1]:
            for black in [False, True]:
                encoded, _ = ccitt(rows, k=k, align=k == 0, black_is_1=black)
                expected = []
                for row in rows:
                    logical = row[:width]+[0]*max(0, width-columns)
                    expected.extend(v if black else 1-v for v in logical)
                params = b'<< /K %d /Columns %d /EncodedByteAlign %s /BlackIs1 %s >>' % (
                    k, columns, b'true' if k == 0 else b'false', b'true' if black else b'false')
                yield image_pdf_vector(f'fax-columns-{columns}-width-{width}-g{4 if k < 0 else 3}-black{int(black)}',
                    encoded, expected, b'/Filter /CCITTFaxDecode /ColorSpace /DeviceGray /BitsPerComponent 1 /DecodeParms '+params,
                    requirement='image-ccitt-columns', width=width, height=3, bits=1,
                    outcome='repair', repair='image.ccitt-columns', space='DeviceGray')

    rows = [[int(x in (0, 8, 1727)) for x in range(1728)] for _ in range(2)]
    encoded, _ = ccitt(rows, k=-1)
    yield image_pdf_vector('fax-columns-default', encoded, [1-v for row in rows for v in row[:9]],
        b'/Filter /CCITTFaxDecode /ColorSpace /DeviceGray /BitsPerComponent 1 /DecodeParms << /K -1 >>',
        requirement='image-ccitt-columns', width=9, height=2, bits=1,
        outcome='repair', repair='image.ccitt-columns', space='DeviceGray')

    rows = [[0, 1, 1, 0, 1], [1, 0, 0, 1, 0]]
    encoded, _ = ccitt(rows, k=-1, black_is_1=True)
    yield image_pdf_vector('fax-columns-stencil-inverted', encoded, [v for row in rows for v in row+[0, 0]],
        b'/Filter /CCITTFaxDecode /ImageMask true /Decode [1 0] /DecodeParms << /K -1 /Columns 5 /BlackIs1 true >>',
        requirement='image-ccitt-columns', width=7, height=2, bits=1, decode='1,0', inverted=1,
        outcome='repair', repair='image.ccitt-columns', space='stencil')
    for height, suffix, length_tag in [(1, 'excess-rows', 'image.data-excess'), (3, 'missing-row', 'image.data-short')]:
        expected = [v for row in rows[:height] for v in row+[0, 0]]
        if height > len(rows):
            # Existing short packed-data policy fills source bytes with zero;
            # the extension beyond Columns remains fax white independently.
            expected += [0]*7
        yield image_pdf_vector('fax-columns-'+suffix, encoded, expected,
            b'/Filter /CCITTFaxDecode /ColorSpace /DeviceGray /BitsPerComponent 1 /DecodeParms << /K -1 /Columns 5 /BlackIs1 true >>',
            requirement='image-ccitt-columns', width=7, height=height, bits=1,
            outcome='repair', repair='image.ccitt-columns', reports=2,
            report_tags='image.ccitt-columns,'+length_tag, space='DeviceGray')
    for explicit in [False, True]:
        dictionary = b'<< /W 7 /H 2 /BPC 1 /CS /G /F /CCF /DP << /K -1 /Columns 5 /BlackIs1 true >>'
        if explicit:
            dictionary += b' /L %d' % len(encoded)
        yield Vector('inline-fax-columns-'+('length' if explicit else 'legacy'), {
            '.dict': dictionary+b' >>', '.in': encoded,
            '.out': bytes(v for row in rows for v in row+[0, 0]),
            '.params': b'kind=inline\noutcome=repair\nrepair=image.ccitt-columns\n'},
            ('image-ccitt-columns',), 'Fax source rows framed with Columns and adapted to Width')


def inline_filter_cases():
    """7.3.8.1 Table 5 permits zero filters; 8.9.7 has no exception."""
    for suffix, dictionary, encoded, expected in [
        ('gray', b'<< /W 3 /H 2 /BPC 2 /CS /G /F [] >>', bytes([0x18, 0xe4]), bytes([0, 1, 2, 3, 2, 1])),
        ('cmyk', b'<< /W 2 /H 1 /BPC 8 /CS /CMYK /F [] >>', bytes(7)+b'\xff', bytes(7)+b'\xff'),
        ('null-params', b'<< /W 2 /H 1 /BPC 8 /CS /G /Filter [] /DP null >>', bytes([12, 26]), bytes([12, 26])),
        ('stencil', b'<< /W 2 /H 1 /IM true /F [] >>', b'\x40', bytes([0, 1])),
        ('embedded-ei', b'<< /W 4 /H 1 /BPC 8 /CS /G /F [] >>', b' EI ', b' EI '),
    ]:
        yield Vector('inline-empty-filter-'+suffix, {'.dict': dictionary, '.in': encoded, '.out': expected,
            '.params': b'kind=inline\noutcome=valid\n'}, ('image-inline-filter',),
            'Empty inline filter array denotes unencoded samples in both recovery modes')
    for suffix, value in [('boolean', b'false'), ('integer', b'0'), ('string', b'(Fl)'),
                          ('dictionary', b'<< >>'), ('null-element', b'[null]'), ('nested-array', b'[[]]'),
                          ('unknown-name', b'/Unknown')]:
        outcome = 'unsupported' if suffix == 'unknown-name' else 'syntax'
        yield Vector('inline-invalid-filter-'+suffix, {
            '.dict': b'<< /W 1 /H 1 /BPC 8 /CS /G /F '+value+b' >>', '.in': b'\0', '.out': b'',
            '.params': f'kind=inline\noutcome={outcome}\nfeature=filter.unknown\nboundary_error=1\n'.encode()},
            ('image-inline-filter',), 'Malformed or unknown inline filters are never inferred from image bytes')


def uncompressed_fax_cases():
    """Literal T.4 Table5/T.6 Table4 words with separately stated bitmaps."""
    from .jbig2 import bits_bytes, packed
    enter = {0:'000000001111', -1:'0000001111'}
    end = {0:'000000000001'*6, -1:'000000000001'*2}
    # (wire code, original black/white pixels), including the five-white word
    # whose final1 does not represent a black pixel.
    words = [('1','1'),('01','01'),('001','001'),('0001','0001'),
             ('00001','00001'),('000001','00000')]
    def fixture(name, code, pixels, *, k=-1, width=None, **options):
        width = width or len(pixels)
        rows = [[int(v) for v in pixels[i:i+width]] for i in range(0,len(pixels),width)]
        expected = packed([[1-v for v in row] for row in rows])
        return vector('fax-uncompressed-'+name,'ccitt',bits_bytes(code+end[k]),expected,
                      requirement='image-ccitt-uncompressed',k=k,width=width,height=len(rows),eob=1,**options)
    for k in (0,-1):
        for index,(code,pixels) in enumerate(words):
            yield fixture(f'word-{k}-{index}',enter[k]+code+'00000010',pixels,k=k)
        # Exit words append zero through four white pixels, then the tag gives
        # the next run colour. Both following run colours remain observable.
        for zeros in range(5):
            for tag in (0,1):
                exit_code = '0'*(6+zeros)+'1'+str(tag)
                suffix = (BLACK if tag else WHITE)[4]
                if k < 0:
                    suffix = '001'+suffix+(WHITE if tag else BLACK)[0]
                yield fixture(f'exit-{k}-{zeros}-{tag}',enter[k]+'1'+exit_code+suffix,
                              '1'+'0'*zeros+str(tag)*4,k=k)
    all_code = ''.join(code for code,_ in words)
    all_pixels = ''.join(pixels for _,pixels in words)
    # T.6 2.3.1 concatenates row-end/start pixels. Narrow widths force a
    # single literal word to cross several rows; no row marker or padding.
    for width in (1,2,3,5,7):
        yield fixture(f'continuation-{width}',enter[-1]+all_code*width+'00000010',
                      all_pixels*width,width=width)
    # PDF Table11 preserves raw polarity and lets EOFB override Rows. With
    # EndOfBlock false, either the stated row count or exhausted input ends it.
    rows = [[int(v) for v in (all_pixels*3)[i:i+3]] for i in range(0,60,3)]
    continuous = enter[-1]+all_code*3+'00000010'
    yield vector('fax-uncompressed-black-is-one','ccitt',bits_bytes(continuous+end[-1]),packed(rows),
                 requirement='image-ccitt-uncompressed',k=-1,width=3,height=1,eob=1,black=1)
    for declared in (0,20):
        yield vector(f'fax-uncompressed-no-eob-{declared}','ccitt',bits_bytes(continuous),
                     packed([[1-v for v in row] for row in rows]),
                     requirement='image-ccitt-uncompressed',k=-1,width=3,height=declared,eob=0)
    for k in (0,-1):
        wire = Wire()
        for _ in range(2):
            wire.align()
            wire.code(enter[k]+'1111'+'00000010')
        wire.align()
        wire.code(end[k])
        yield vector(f'fax-uncompressed-aligned-{k}','ccitt',wire.bytes(),b'\x00\x00',
                     requirement='image-ccitt-uncompressed',k=k,width=4,height=2,eob=1,align=1)
    # The first coded row has black pixels3..6. The next row leaves raw mode
    # at column3; VR(1) ends its black run at8, then V(0) reaches column12.
    first = '001'+WHITE[3]+BLACK[4]+'1'
    yield fixture('resume-vertical',first+enter[-1]+'001'+'00000011'+'0111',
                  '000111100000'+'001111110000',width=12)
    # Exit after one black pixel into a white run; pass skips the reference
    # row's black island, then V(0) reaches the final boundary.
    yield fixture('resume-pass',first+enter[-1]+'1'+'00000010'+'00011',
                  '000111100000'+'100000000000',width=12)
    # T.4 mixed rows: first1-D, then2-D, each leaving raw mode before EOL.
    pixels = '10000100'
    raw = '1'+'00001'+'0000000010'
    mixed = '0000000000011'+enter[0]+raw+'0000000000010'+enter[-1]+raw
    mixed += ('0000000000011')*6
    yield vector('fax-uncompressed-mixed','ccitt',bits_bytes(mixed),b'\x7b\x7b',
                 requirement='image-ccitt-uncompressed',k=2,width=8,height=2,eob=1,eol=1)
    for k in (0,-1):
        prefix = '000000001' if k == 0 else '0000001'
        for selector in range(7):
            yield vector(f'fax-uncompressed-undefined-{k}-{selector}','ccitt',
                         bits_bytes(prefix+f'{selector:03b}'),requirement='image-ccitt-uncompressed',
                         outcome='unsupported',feature='ccitt.extension',k=k,width=8,height=1,eob=1)
    for name, code, k, width in [
        ('invalid-word',enter[-1]+'0'*11+'1',-1,8),
        ('missing-exit',enter[-1]+'1111'+end[-1],-1,4),
        ('forbidden-1d-prefix',WHITE[3]+enter[0]+'111'+'00000010',0,6),
    ]:
        yield vector('fax-uncompressed-'+name,'ccitt',bits_bytes(code),
                     requirement='image-ccitt-uncompressed',outcome='syntax',k=k,width=width,height=1,eob=1)
    for name,k,options in [('g3',0,{}),('aligned',-1,{'align':1}),('eol',-1,{'eol':1})]:
        prefix = '000000000001' if options.get('eol') else ''
        # A single five-white code cannot be interrupted by an EOL or byte
        # fill. T.6 continuation is valid only in the unbroken bit stream.
        code = prefix+enter[k]+'000001'+'00000010'+end[k]
        yield vector('fax-uncompressed-crosses-framed-'+name,'ccitt',bits_bytes(code),
                     requirement='image-ccitt-uncompressed',outcome='syntax',k=k,width=2,height=2,eob=1,**options)


def uncompressed_fax_render_cases():
    from dataclasses import replace
    from .paint import image_case
    selected = {'fax-uncompressed-continuation-3','fax-uncompressed-resume-vertical',
                'fax-uncompressed-resume-pass','fax-uncompressed-mixed','fax-uncompressed-exit-0-4-1'}
    for item in uncompressed_fax_cases():
        if item.name not in selected:
            continue
        params = dict(line.split('=') for line in item.files['.params'].decode().splitlines())
        width, height = int(params['width']), int(params['height'])
        stride = (width+7)//8
        pixels = item.files['.out']
        colors = [(255*((pixels[y*stride+x//8] >> (7-x%8)) & 1),)*3
                  for y in range(height) for x in range(width)]
        settings = {'matrix':(width,0,0,height,2,2),'colors':colors,'requirement':'image-ccitt-uncompressed'}
        dictionary = (b'/BitsPerComponent 1 /ColorSpace /DeviceGray /Filter /CCITTFaxDecode /DecodeParms << /K '
                      + params['k'].encode()+b' /Columns '+str(width).encode()+b' /Rows '+str(height).encode()+b' >>')
        name = 'ccitt-'+item.name.removeprefix('fax-')
        yield image_case(name,item.files['.in'],dictionary,[],width,height,**settings)
        control = image_case(name+'-rgb-control',bytes(v for c in colors for v in c),
                             b'/BitsPerComponent 8 /ColorSpace /DeviceRGB',[],width,height,**settings)
        yield replace(control,extra={**control.extra,'control_of':'image/'+name})


def dnl_frame(progressive=False):
    """One literal DC-zero/EOB MCU, advertised as an 8x8 sample grid."""
    raw, _ = constant_jpeg(progressive=progressive)
    data = bytearray(raw)
    sof = data.index(b"\xff\xc2" if progressive else b"\xff\xc0")
    data[sof + 5:sof + 9] = struct.pack(">HH", 8, 8)
    return bytes(data), sof


def dnl_streams():
    baseline, sof = dnl_frame()
    for name, initial in [("deferred", 0), ("same-height", 8), ("redefined", 16)]:
        data = bytearray(baseline)
        data[sof + 5:sof + 7] = struct.pack(">H", initial)
        data[-2:-2] = segment(0xdc, struct.pack(">H", 8))
        yield "jpeg-dnl-" + name, bytes(data)
    progressive, sof = dnl_frame(progressive=True)
    data = bytearray(progressive)
    data[sof + 5:sof + 7] = b"\0\0"
    second_scan = data.index(b"\xff\xda", data.index(b"\xff\xda") + 2)
    data[second_scan:second_scan] = segment(0xdc, struct.pack(">H", 8))
    yield "jpeg-dnl-progressive", bytes(data)


def dnl_cases():
    # T.81 B.2.5/Table B.10: DNL follows the first complete MCU row. The
    # literal zero coefficient yields 128, independently of either decoder.
    from .paint import image_case
    streams = list(dnl_streams())
    samples = [128] * 64
    for name, data in streams:
        yield vector(name.replace("jpeg-dnl-", "jpeg-dnl-vector-"), "dct", data, bytes(samples), width=8, height=8, components=1)
    controls = [("jpeg-dnl-baseline-control", dnl_frame()[0]),
                ("jpeg-dnl-progressive-control", dnl_frame(progressive=True)[0])]
    for name, data in streams + controls:
        yield image_case(name, data, b"/ColorSpace /DeviceGray /BitsPerComponent 8 /Filter /DCTDecode",
                         samples, 8, 8, matrix=(8, 0, 0, 8, 2, 2), requirement="image-dct")
    yield image_case("jpeg-dnl-rgb-control", bytes([128]) * 192,
                     b"/ColorSpace /DeviceRGB /BitsPerComponent 8", samples, 8, 8,
                     matrix=(8, 0, 0, 8, 2, 2), requirement="image-dct")
    data = streams[0][1]
    # B.1.1.4 and Table B.10 allow Ld > 4; extension bytes are not pixels.
    yield vector("jpeg-dnl-extension", "dct", data[:-8] + segment(0xdc, b"\0\x08extra") + data[-2:],
                 bytes(samples), width=8, height=8, components=1)
    baseline, sof = dnl_frame()
    absent = bytearray(baseline); absent[sof + 5:sof + 7] = b"\0\0"
    progressive, _ = dnl_frame(progressive=True)
    first_scan = baseline.index(b"\xff\xda")
    dnl = segment(0xdc, b"\0\x08")
    invalid = {
        "missing": bytes(absent),
        "zero": data[:-8] + segment(0xdc, b"\0\0") + data[-2:],
        "short": data[:-8] + segment(0xdc, b"\x08") + data[-2:],
        "excess-rows": data[:-8] + segment(0xdc, b"\0\x10") + data[-2:],
        "before-scan": baseline[:first_scan] + dnl + baseline[first_scan:],
        "duplicate": data[:-2] + dnl + data[-2:],
        "after-second-scan": progressive[:-2] + dnl + progressive[-2:],
    }
    for name, encoded in invalid.items():
        yield vector("jpeg-dnl-" + name, "dct", encoded, outcome="syntax")
    # A marker-looking comment payload cannot replace the actual first-scan DNL.
    commented = data[:first_scan] + segment(0xfe, b"x\xff\xdc\0\4\0\x10") + data[first_scan:]
    yield vector("jpeg-dnl-comment", "dct", commented, bytes(samples), width=8, height=8, components=1)
    restarted, expected = constant_jpeg(restart=True)
    restart_frame = bytearray(restarted)
    sof = restart_frame.index(b"\xff\xc0")
    restart_frame[sof + 5:sof + 7] = b"\0\0"
    restart_frame[-2:-2] = segment(0xdc, b"\0\1")
    yield vector("jpeg-dnl-restart", "dct", bytes(restart_frame), bytes(expected),
                 width=16, height=1, components=1)
    from .jpeg import lossless_jpeg
    lossless, expected, width, height = lossless_jpeg(restart_rows=1)
    lossless_frame = bytearray(lossless)
    sof = lossless_frame.index(b"\xff\xc3")
    lossless_frame[sof + 5:sof + 7] = b"\0\0"
    lossless_frame[-2:-2] = segment(0xdc, struct.pack(">H", height))
    yield vector("jpeg-dnl-lossless", "dct", bytes(lossless_frame), expected,
                 width=width, height=height, components=1)
    yield Vector("inline-jpeg-dnl", {".dict": b"<< /W 8 /H 8 /BPC 8 /CS /G /F /DCT >>",
                 ".in": data, ".out": bytes(samples), ".params": b"kind=inline\noutcome=valid\n"},
                 ("image-inline", "image-dct"), "DNL marker frames an inline JPEG before its EOI")
