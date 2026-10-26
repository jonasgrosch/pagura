"""Independent T.81 Annex D/F/G/H JPEG encoder and coefficient decode model.

The encoder keeps an unbounded integer code base and postpones Annex D's
carry propagation until termination. Interval subdivision, conditional exchange
and renormalization are unchanged. The decoder uses a sixteen-bit sliding
window, independently of the encoder's cumulative integer representation.
Neither model imports or executes the product.
"""
import math
import struct
from typing import Any

# T.81 Table D.3: Qe, next LPS, next MPS, switch MPS.
STATES = [
    (0x5a1d, 1, 1, 1),
    (0x2586, 14, 2, 0),
    (0x1114, 16, 3, 0),
    (0x080b, 18, 4, 0),
    (0x03d8, 20, 5, 0),
    (0x01da, 23, 6, 0),
    (0x00e5, 25, 7, 0),
    (0x006f, 28, 8, 0),
    (0x0036, 30, 9, 0),
    (0x001a, 33, 10, 0),
    (0x000d, 35, 11, 0),
    (0x0006, 9, 12, 0),
    (0x0003, 10, 13, 0),
    (0x0001, 12, 13, 0),
    (0x5a7f, 15, 15, 1),
    (0x3f25, 36, 16, 0),
    (0x2cf2, 38, 17, 0),
    (0x207c, 39, 18, 0),
    (0x17b9, 40, 19, 0),
    (0x1182, 42, 20, 0),
    (0x0cef, 43, 21, 0),
    (0x09a1, 45, 22, 0),
    (0x072f, 46, 23, 0),
    (0x055c, 48, 24, 0),
    (0x0406, 49, 25, 0),
    (0x0303, 51, 26, 0),
    (0x0240, 52, 27, 0),
    (0x01b1, 54, 28, 0),
    (0x0144, 56, 29, 0),
    (0x00f5, 57, 30, 0),
    (0x00b7, 59, 31, 0),
    (0x008a, 60, 32, 0),
    (0x0068, 62, 33, 0),
    (0x004e, 63, 34, 0),
    (0x003b, 32, 35, 0),
    (0x002c, 33, 9, 0),
    (0x5ae1, 37, 37, 1),
    (0x484c, 64, 38, 0),
    (0x3a0d, 65, 39, 0),
    (0x2ef1, 67, 40, 0),
    (0x261f, 68, 41, 0),
    (0x1f33, 69, 42, 0),
    (0x19a8, 70, 43, 0),
    (0x1518, 72, 44, 0),
    (0x1177, 73, 45, 0),
    (0x0e74, 74, 46, 0),
    (0x0bfb, 75, 47, 0),
    (0x09f8, 77, 48, 0),
    (0x0861, 78, 49, 0),
    (0x0706, 79, 50, 0),
    (0x05cd, 48, 51, 0),
    (0x04de, 50, 52, 0),
    (0x040f, 50, 53, 0),
    (0x0363, 51, 54, 0),
    (0x02d4, 52, 55, 0),
    (0x025c, 53, 56, 0),
    (0x01f8, 54, 57, 0),
    (0x01a4, 55, 58, 0),
    (0x0160, 56, 59, 0),
    (0x0125, 57, 60, 0),
    (0x00f6, 58, 61, 0),
    (0x00cb, 59, 62, 0),
    (0x00ab, 61, 63, 0),
    (0x008f, 61, 32, 0),
    (0x5b12, 65, 65, 1),
    (0x4d04, 80, 66, 0),
    (0x412c, 81, 67, 0),
    (0x37d8, 82, 68, 0),
    (0x2fe8, 83, 69, 0),
    (0x293c, 84, 70, 0),
    (0x2379, 86, 71, 0),
    (0x1edf, 87, 72, 0),
    (0x1aa9, 87, 73, 0),
    (0x174e, 72, 74, 0),
    (0x1424, 72, 75, 0),
    (0x119c, 74, 76, 0),
    (0x0f6b, 74, 77, 0),
    (0x0d51, 75, 78, 0),
    (0x0bb6, 77, 79, 0),
    (0x0a40, 77, 48, 0),
    (0x5832, 80, 81, 1),
    (0x4d1c, 88, 82, 0),
    (0x438e, 89, 83, 0),
    (0x3bdd, 90, 84, 0),
    (0x34ee, 91, 85, 0),
    (0x2eae, 92, 86, 0),
    (0x299a, 93, 87, 0),
    (0x2516, 86, 71, 0),
    (0x5570, 88, 89, 1),
    (0x4ca9, 95, 90, 0),
    (0x44d9, 96, 91, 0),
    (0x3e22, 97, 92, 0),
    (0x3824, 99, 93, 0),
    (0x32b4, 99, 94, 0),
    (0x2e17, 93, 86, 0),
    (0x56a8, 95, 96, 1),
    (0x4f46, 101, 97, 0),
    (0x47e5, 102, 98, 0),
    (0x41cf, 103, 99, 0),
    (0x3c3d, 104, 100, 0),
    (0x375e, 99, 93, 0),
    (0x5231, 105, 102, 0),
    (0x4c0f, 106, 103, 0),
    (0x4639, 107, 104, 0),
    (0x415e, 103, 99, 0),
    (0x5627, 105, 106, 1),
    (0x50e7, 108, 107, 0),
    (0x4b85, 109, 103, 0),
    (0x5597, 110, 109, 0),
    (0x504f, 111, 107, 0),
    (0x5a10, 110, 111, 1),
    (0x5522, 112, 109, 0),
    (0x59eb, 112, 111, 1),
]


class Encoder:
    """D.1: normalized intervals with deferred byte output and stuffing."""
    def __init__(self):
        self.a, self.base, self.shifts = 65536, 0, 0
        self.states = {}

    def bit(self, context, decision):
        state, mps = self.states.get(context, (0, 0)) if context is not None else (0, 0)
        q, nl, nm, switch = STATES[state]
        self.a -= q
        lps = decision != mps
        if lps:
            if self.a >= q:
                self.base += self.a
                self.a = q
            state, mps = nl, mps ^ switch
        else:
            if self.a >= 32768:
                return
            if self.a < q:
                self.base += self.a
                self.a = q
            state = nm
        if context is not None:
            self.states[context] = state, mps
        while self.a < 32768:
            self.a *= 2
            self.base *= 2
            self.shifts += 1

    def finish(self):
        # D.1.8/Figure D.14: choose a point inside the final interval with
        # the low sixteen bits cleared, or fifteen bits after half-unit repair.
        # The unbounded register defers carries, so this also resolves every
        # previously pending byte. Marker input supplies the omitted zero suffix.
        final = (self.base+self.a-1) & ~65535
        if final < self.base:
            final += 32768
        assert self.base <= final < self.base+self.a
        bits = self.shifts+16
        result = (final << (-bits % 8)).to_bytes((bits+7)//8, 'big')
        return result.rstrip(b'\0').replace(b'\xff', b'\xff\0')


class Decoder:
    """D.2: a bit window, exchange tests and adaptive conditional estimates."""
    def __init__(self, data):
        raw = data.replace(b'\xff\0', b'\xff')
        self.bits = ''.join(f'{byte:08b}' for byte in raw)
        self.position = 16
        self.value = int((self.bits+'0'*16)[:16], 2)
        self.a, self.states = 65536, {}

    def bit(self, context):
        index, mps = self.states.get(context, (0, 0)) if context is not None else (0, 0)
        q, nl, nm, switch = STATES[index]
        self.a -= q
        if self.value >= self.a:
            self.value -= self.a
            lps = self.a >= q
            self.a = q
        elif self.a >= 32768:
            return mps
        else:
            lps = self.a < q
        decision = mps ^ lps
        if context is not None:
            self.states[context] = (nl, mps ^ switch) if lps else (nm, mps)
        while self.a < 32768:
            incoming = int(self.bits[self.position]) if self.position < len(self.bits) else 0
            self.value = self.value*2 + incoming
            self.a *= 2
            self.position += 1
        return decision


def write_magnitude(coder, key, value, first, x1, x2):
    sz = abs(value)-1
    coder.bit(key(first), int(sz != 0))
    if not sz:
        return
    leading, index = 1, x1
    while sz >= 2*leading:
        coder.bit(key(index), 1)
        leading *= 2
        index = x2 if leading == 2 else index+1
    coder.bit(key(index), 0)
    for shift in range(leading.bit_length()-2, -1, -1):
        coder.bit(key(index+14), (sz >> shift) & 1)


def read_magnitude(coder, key, first, x1, x2):
    if not coder.bit(key(first)):
        return 1
    leading, index = 1, x1
    while coder.bit(key(index)):
        leading *= 2
        assert leading <= 16384
        index = x2 if leading == 2 else index+1
    value = leading
    for shift in range(leading.bit_length()-2, -1, -1):
        value += coder.bit(key(index+14)) << shift
    return value+1


def dc_context(difference, lower, upper):
    magnitude = abs(difference)
    if magnitude <= (0 if lower == 0 else 1 << (lower-1)):
        return 0
    return 4 + 4*int(difference < 0) + 8*int(magnitude > 1 << upper)


def arithmetic_block(coder, block, prior, table, start, end, high, low, dc, ac, encode):
    """F.1.4/F.2.4 and G.1.3/G.2, explicit symbol-level models."""
    key_dc = lambda i: ('dc', table, i)
    key_ac = lambda i: ('ac', table, i)
    decision = coder.bit
    predictor, previous = prior
    def bit(context, value=0):
        if encode:
            decision(context, value)
            return value
        return decision(context)
    if start == 0:
        if high:
            value = bit(None, (block[0] >> low) & 1)
            if not encode:
                block[0] |= value << low
        else:
            difference = (block[0] >> low)-predictor if encode else 0
            context = dc_context(previous, *dc)
            present = bit(key_dc(context), int(difference != 0))
            if present:
                negative = bit(key_dc(context+1), int(difference < 0))
                first = context+2+negative
                if encode:
                    write_magnitude(coder, key_dc, difference, first, 20, 21)
                else:
                    difference = read_magnitude(coder, key_dc, first, 20, 21) * (-1 if negative else 1)
            predictor += difference
            previous = difference
            if not encode:
                block[0] = predictor << low
        if end == 0:
            return predictor, previous
    k = max(1, start)
    if high:
        # In the encoder, values >=2 at this approximation are old nonzeros;
        # in the decoder, old nonzeros are already stored in the block.
        values = [(abs(v) >> low) * (-1 if v < 0 else 1) for v in block] if encode else block
        old_end = next((i+1 for i in range(end, start-1, -1)
                        if abs(values[i]) >= 2), start) if encode else next(
                            (i+1 for i in range(end, start-1, -1) if block[i]), start)
        eob = next((i+1 for i in range(end, start-1, -1) if values[i]), start)
        while k <= end:
            if k >= old_end and bit(key_ac(3*(k-1)), int(k >= eob)):
                break
            while True:
                base = 3*(k-1)
                old = abs(values[k]) >= 2 if encode else block[k] != 0
                if old:
                    value = bit(key_ac(base+2), abs(values[k]) & 1)
                    if not encode and value:
                        block[k] += (1 if block[k] > 0 else -1) << low
                    break
                if bit(key_ac(base+1), int(values[k] != 0)):
                    negative = bit(None, int(values[k] < 0))
                    if not encode:
                        block[k] = (-1 if negative else 1) << low
                    break
                k += 1
                assert k <= end
            k += 1
    else:
        values = [(abs(v) >> low) * (-1 if v < 0 else 1) for v in block] if encode else block
        eob = next((i+1 for i in range(end, k-1, -1) if values[i]), k)
        while k <= end:
            if bit(key_ac(3*(k-1)), int(k >= eob)):
                break
            while not bit(key_ac(3*(k-1)+1), int(values[k] != 0)):
                k += 1
                assert k <= end
            negative = bit(None, int(values[k] < 0))
            first = 3*(k-1)+2
            extended = 189 if k <= ac else 217
            if encode:
                write_magnitude(coder, key_ac, values[k], first, first, extended)
            else:
                block[k] = (read_magnitude(coder, key_ac, first, first, extended)
                            * (-1 if negative else 1)) << low
            k += 1
    return predictor, previous



class HuffmanModel:
    """F.2.2/G.2 reader for the generated canonical five/eight bit tables."""
    def __init__(self, data, ac_symbols):
        self.bits = ''.join(f'{b:08b}' for b in data.replace(b'\xff\0', b'\xff'))
        self.position = 0
        self.ac_symbols = ac_symbols

    def read(self, size):
        value = int(self.bits[self.position:self.position+size] or '0', 2)
        self.position += size
        return value

    def signed(self, size):
        value = self.read(size)
        return value if value >= 1 << (size-1) else value-(1 << size)+1

    def block(self, block, predictor, start, end, high, low):
        if start == 0:
            if high:
                block[0] |= self.read(1) << low
            else:
                size = self.read(5)
                predictor += self.signed(size) if size else 0
                block[0] = predictor << low
            if end == 0:
                return predictor
        k = max(1, start)
        if not high:
            while k <= end:
                symbol = self.ac_symbols[self.read(8)]
                run, size = symbol >> 4, symbol & 15
                if size:
                    k += run
                    block[k] = self.signed(size) << low
                    k += 1
                elif run == 15:
                    k += 16
                else:
                    assert run == 0
                    break
            return predictor
        def correct(index):
            if self.read(1):
                block[index] += (1 if block[index] > 0 else -1) << low
        while k <= end:
            symbol = self.ac_symbols[self.read(8)]
            run, size = symbol >> 4, symbol & 15
            if size == 0 and run != 15:
                assert run == 0
                for index in range(k, end+1):
                    if block[index]:
                        correct(index)
                break
            new = ((1 if self.read(1) else -1) << low) if size else 0
            if not size:
                run = 16
            while k <= end:
                if block[k]:
                    correct(k)
                elif not run:
                    break
                else:
                    run -= 1
                k += 1
                if not new and not run:
                    break
            if new:
                block[k] = new
                k += 1
        return predictor

def reconstruct(blocks, width, height, precision, quant):
    """Annex A inverse transform, rounded and clamped at native precision."""
    order = sorted(range(64), key=lambda p: (p//8+p%8, p%8 if (p//8+p%8)%2 == 0 else p//8))
    result = [0]*(width*height)
    stride = (width+7)//8
    for i, block in enumerate(blocks):
        for y in range(8):
            for x in range(8):
                sx, sy = i%stride*8+x, i//stride*8+y
                if sx >= width or sy >= height:
                    continue
                value = (1 << (precision-1)) + sum(
                    c*q * (1/math.sqrt(2) if p%8 == 0 else 1)
                    * (1/math.sqrt(2) if p//8 == 0 else 1)
                    * math.cos((2*x+1)*(p%8)*math.pi/16)
                    * math.cos((2*y+1)*(p//8)*math.pi/16)/4
                    for c, q, p in zip(block, quant, order))
                native = max(0, min((1 << precision)-1, math.floor(value+0.5)))
                result[sy*width+sx] = native
    return result


def colour_model(planes, precision, convert):
    """PDF Table 13 at native precision; leave byte-domain values unrounded."""
    maximum, centre = (1 << precision)-1, 1 << (precision-1)
    result = []
    for pixel in zip(*planes):
        values = list(pixel)
        if convert and len(values) >= 3:
            y, cb, cr = values[0], values[1]-centre, values[2]-centre
            values[:3] = [y+1.402*cr, y-0.344136*cb-0.714136*cr, y+1.772*cb]
            values[:3] = [max(0, min(maximum, value)) for value in values[:3]]
            if len(values) == 4:
                values[:3] = [maximum-value for value in values[:3]]
        result.extend(value*255/maximum for value in values)
    return result


def jpeg(*, arithmetic=True, precision=8, progressive=False, restart=0, tables=False,
         conditioning=None, quantizer=1, width=24, height=16, sparse=False, dac=True, stress=False, sample_values=None, banded=False, components=1, transform=0, adobe=None, include_exact=False):
    from .image import segment, Wire, magnitude, AC, ac_refinement
    n = 4 if tables else components
    conditioning = conditioning or [(0, 1, 5)]*4
    count = ((width+7)//8)*((height+7)//8)
    dc_values = [-500, -499, -495, -460, -510, 0, 500, 495, 350, -300]
    blocks = []
    for ci in range(n):
        plane = []
        for bi in range(count):
            block = [0]*64
            block[0] = dc_values[(bi+ci)%len(dc_values)] * (16 if precision == 12 else 1)
            if stress:
                block[0] = [-16384, 16383, 0, -8192, 8191, -1][bi % 6]
            if sample_values is not None:
                block[0] = 8*(sample_values[bi % len(sample_values)]-2048)
            if not sparse:
                for j, k in enumerate([1, 2, 5, 6, 16, 17, 31, 32, 63]):
                    block[k] = ([1, -1, 2, -3, 19, -37, 67, -128, 9][(bi+j+ci)%9]
                                * (8 if precision == 12 and k == 31 else 1))
                if stress:
                    for k,value in [(1,16383),(2,-16383),(31,8192),(63,-8192)]:
                        block[k] = value
            plane.append(block)
        blocks.append(plane)
    decoded = [[[0]*64 for _ in range(count)] for _ in range(n)]
    process = (0xca if progressive else 0xc9) if arithmetic else (0xc2 if progressive else 0xc1)
    data = b'\xff\xd8'
    if adobe is not None:
        data += segment(0xee, b'Adobe'+struct.pack('>HHHB',100,0,0,adobe))
    data += segment(0xdb, b'\x10' + struct.pack('>64H', *([quantizer]*64)))
    data += segment(process, bytes([precision])+struct.pack('>HHB',height,width,n)
                    + b''.join(bytes([ci+1,0x11,0]) for ci in range(n)))
    ac_symbols, ac_codes = b'', {}  # Huffman tables, set below when not arithmetic
    if arithmetic and dac:
        data += segment(0xcc, b''.join(bytes([ci, l+16*u, 0x10+ci, k])
                                       for ci, (l,u,k) in enumerate(conditioning)))
    elif not arithmetic:
        # Complete coefficient alphabets for P=12 (F.1.2.1/Table F.1).
        dc_symbols = bytes(range(16))
        ac_symbols = bytes(AC + [r*16+s for r in range(16) for s in range(11,15)])
        data += segment(0xc4, b'\0'+bytes([0,0,0,0,16]+[0]*11)+dc_symbols
                        +b'\x10'+bytes([0]*7+[len(ac_symbols)]+[0]*8)+ac_symbols)
        ac_codes = {v:i for i,v in enumerate(ac_symbols)}
    if restart:
        data += segment(0xdd, struct.pack('>H', restart))
    scans = [(0,0,0,1), (1,63,0,1), (0,0,1,0), (1,63,1,0)] if progressive else [(0,63,0,0)]
    if banded:
        assert arithmetic
        scans = [(0,0,0,2),(1,8,0,2),(9,63,0,2),
                 (0,0,2,1),(1,8,2,1),(9,63,2,1),
                 (0,0,1,0),(1,8,1,0),(9,63,1,0)]
    for start, end, high, low in scans:
        selected = list(range(n)) if start == 0 else [0]
        # Four component DC/sequential scans use all four table destinations;
        # progressive AC scans must each carry exactly one component.
        groups = [selected] if start == 0 else [[ci] for ci in range(n)]
        for group in groups:
            header = bytes([len(group)]) + b''.join(bytes([ci+1, ci*17 if tables else 0]) for ci in group)
            data += segment(0xda, header+bytes([start,end,high*16+low]))
            sequence = 0
            for first in range(0, count, restart or count):
                last = min(count, first+(restart or count))
                coder: Any = Encoder() if arithmetic else Wire()
                prior = [(0,0)]*n
                for bi in range(first,last):
                    for ci in group:
                        block = blocks[ci][bi]
                        if arithmetic:
                            prior[ci] = arithmetic_block(coder,block,prior[ci],ci if tables else 0,
                                start,end,high,low,conditioning[ci if tables else 0][:2],
                                conditioning[ci if tables else 0][2],True)
                        elif start == 0:
                            if high:
                                coder.put((block[0] >> low) & 1,1)
                            else:
                                value = block[0] >> low
                                diff = value-prior[ci][0]
                                coder.put(abs(diff).bit_length(),5)
                                magnitude(coder,diff)
                                prior[ci] = value,0
                            if end == 0:
                                continue
                        if not arithmetic and end != 0:
                            if high:
                                ac_refinement(coder, block)
                                continue
                            run = 0
                            for k in range(max(1,start),end+1):
                                v = (abs(block[k]) >> low) * (-1 if block[k]<0 else 1)
                                if not v:
                                    run += 1
                                else:
                                    while run >= 16:
                                        coder.put(ac_codes[240],8); run -= 16
                                    coder.put(ac_codes[run*16+abs(v).bit_length()],8)
                                    magnitude(coder,v); run = 0
                            if run:
                                coder.put(ac_codes[0],8)
                encoded = coder.finish() if arithmetic else coder.bytes(True)
                data += encoded
                if arithmetic:
                    model = Decoder(encoded)
                    prior = [(0,0)]*n
                    for bi in range(first,last):
                        for ci in group:
                            prior[ci] = arithmetic_block(model,decoded[ci][bi],prior[ci],
                                ci if tables else 0,start,end,high,low,
                                conditioning[ci if tables else 0][:2],conditioning[ci if tables else 0][2],False)
                else:
                    model = HuffmanModel(encoded, ac_symbols)
                    predictors = [0]*n
                    for bi in range(first,last):
                        for ci in group:
                            predictors[ci] = model.block(decoded[ci][bi], predictors[ci], start, end, high, low)
                if last < count:
                    data += bytes([255,0xd0+sequence]); sequence = (sequence+1)%8
    assert decoded == blocks
    planes = [reconstruct(plane,width,height,precision,[quantizer]*64) for plane in decoded]
    convert = transform == 1 if adobe is None else adobe in (1, 2)
    exact = colour_model(planes, precision, convert)
    expected = bytes(max(0,min(255,math.floor(value+0.5))) for value in exact)
    if include_exact:
        return data+b'\xff\xd9', expected, exact
    return data+b'\xff\xd9', expected




def verify_qm():
    # T.81 K.4.1 is a separate published known answer for the bit coder.
    plain = bytes.fromhex('00020051 000000c0 0352872a aaaaaaaa 82c02000 fcd79ef6 74eaabf7 697ee74c')
    encoded = bytes.fromhex('655b5144f7969d517855bfff00fc5184c7cef93900287d46708ecbc0f6')
    decoder = Decoder(encoded)
    encoder = Encoder()
    decisions = [int(b) for value in plain for b in f'{value:08b}']
    for value in decisions:
        assert decoder.bit(0) == value
        encoder.bit(0, value)
    assert encoder.finish() == encoded
    model = Decoder(encoder.finish())
    assert [model.bit(0) for _ in decisions] == decisions

def lossless_jpeg(*, precision=8, predictor=1, point=0, components=1, restart_rows=0,
                  arithmetic=False, sampling=None, separate=False, tables=False, transform=0, adobe=None):
    """Independent T.81 H encoder; the original sample grid is the oracle.

    Differences use modulo 2**16; neither overflow predictions nor negative
    half-differences are clipped. High-precision display reduction is the same
    explicitly declared full-range policy as the twelve-bit DCT fixtures.
    """
    from .image import Wire, segment
    width, height = 5, 4
    maximum = (1 << precision) - 1
    seeds = [0, 255, 128, 63, 192, 255, 0, 129, 65, 191,
             17, 240, 3, 252, 127, 221, 34, 189, 66, 128]
    sampling = sampling or [(1, 1)] * components
    max_h, max_v = max(h for h, _ in sampling), max(v for _, v in sampling)
    columns, rows = math.ceil(width / max_h), math.ceil(height / max_v)
    geometry, planes = [], []
    for c, (h, v) in enumerate(sampling):
        cw, ch = math.ceil(width * h / max_h), math.ceil(height * v / max_v)
        stride, lines = columns * h, rows * v
        geometry.append((cw, ch, stride, lines))
        planes.append([[((seeds[(min(y, ch - 1) * cw + min(x, cw - 1) + c * 7) % len(seeds)]
                         * maximum + 127) // 255) >> point for x in range(stride)] for y in range(lines)])
    data = b'\xff\xd8'
    if adobe is not None:
        data += segment(0xee, b'Adobe' + struct.pack('>HHHB', 100, 0, 0, adobe))
    selectors = [c if tables else 0 for c in range(components)]
    symbols = [[(size + table) % 17 for size in range(17)] for table in range(4)]
    for table in sorted(set(selectors)):
        if arithmetic:
            data += segment(0xcc, bytes([table, 16]))  # H.1.2.3.3: L=0, U=1.
        else:
            data += segment(0xc4, bytes([table]) + bytes([0] * 4 + [17] + [0] * 11) + bytes(symbols[table]))
    data += segment(0xcb if arithmetic else 0xc3, bytes([precision]) + struct.pack('>HHB', height, width, components)
                    + b''.join(bytes([c + 1, h * 16 + v, 0]) for c, (h, v) in enumerate(sampling)))
    for group in ([[c] for c in range(components)] if separate else [list(range(components))]):
        interleaved = len(group) > 1
        scan_cols, scan_rows = (columns, rows) if interleaved else geometry[group[0]][:2]
        interval = restart_rows * scan_cols
        if interval:
            data += segment(0xdd, struct.pack('>H', interval))
        data += segment(0xda, bytes([len(group)]) + b''.join(bytes([c + 1, selectors[c] << 4]) for c in group)
                        + bytes([predictor, 0, point]))
        wire, coder = Wire(), Encoder()
        differences = [[[0] * stride for _ in range(lines)] for _, _, stride, lines in geometry]
        start, sequence = 0, 0
        for mcu in range(scan_cols * scan_rows):
            if interval and mcu and mcu % interval == 0:
                data += coder.finish() if arithmetic else wire.bytes(jpeg=True)
                data += bytes([255, 208 + sequence % 8])
                sequence += 1
                start, wire, coder = mcu, Wire(), Encoder()
            my, mx = divmod(mcu, scan_cols)
            for c in group:
                h, v = sampling[c] if interleaved else (1, 1)
                plane, history = planes[c], differences[c]
                first_row = start // scan_cols * v
                for y in range(my * v, (my + 1) * v):
                    for x in range(mx * h, (mx + 1) * h):
                        if x == 0 and y == first_row:
                            predicted = 1 << (precision - point - 1)
                        elif y == first_row:
                            predicted = plane[y][x - 1]
                        elif x == 0:
                            predicted = plane[y - 1][x]
                        else:
                            a, b, corner = plane[y][x - 1], plane[y - 1][x], plane[y - 1][x - 1]
                            predicted = [a, b, corner, a + b - corner, a + ((b - corner) // 2),
                                         b + ((a - corner) // 2), (a + b) // 2][predictor - 1]
                        difference = (plane[y][x] - predicted + 32768) % 65536 - 32768
                        if arithmetic:
                            left = history[y][x - 1] if x else 0
                            above = history[y - 1][x] if y > first_row else 0
                            context = 5 * dc_context(left, 0, 1) + dc_context(above, 0, 1)
                            key = lambda index: ('lossless', selectors[c], index)
                            coder.bit(key(context), int(difference != 0))
                            if difference:
                                negative = int(difference < 0)
                                coder.bit(key(context + 1), negative)
                                first = 129 if dc_context(above, 0, 1) >= 12 else 100
                                write_magnitude(coder, key, difference, context + 2 + negative, first, first + 1)
                        else:
                            size = abs(difference).bit_length()
                            wire.put(symbols[selectors[c]].index(size), 5)
                            if size != 16:
                                wire.put(difference if difference >= 0 else difference + (1 << size) - 1, size)
                        history[y][x] = difference
        data += coder.finish() if arithmetic else wire.bytes(jpeg=True)
    expected = bytearray()
    convert = adobe in (1, 2) if adobe is not None else transform == 1
    for y in range(height):
        for x in range(width):
            pixel = [planes[c][y * v // max_v][x * h // max_h] << point for c, (h, v) in enumerate(sampling)]
            if convert and components >= 3:
                luminance, cb, cr = pixel[0], pixel[1] - (1 << (precision - 1)), pixel[2] - (1 << (precision - 1))
                rgb = [luminance + 1.402 * cr, luminance - 0.344136 * cb - 0.714136 * cr, luminance + 1.772 * cb]
                rgb = [min(maximum, max(0, value)) for value in rgb]
                pixel = [maximum - value if components == 4 else value for value in rgb] + pixel[3:]
            expected.extend(math.floor(value * 255 / maximum + 0.5) for value in pixel)
    return data + b'\xff\xd9', bytes(expected), width, height


def lossless_vectors():
    """T.81 H values, layout boundaries and malformed control fields."""
    from .image import Wire, segment, vector
    settings: list[tuple[str, dict[str, Any]]] = [(f'precision-{precision}', {'precision': precision, 'predictor': precision % 7 + 1})
                for precision in range(2, 17)]
    settings += [(f'predictor-{predictor}-{precision}', {'precision': precision, 'predictor': predictor})
                 for precision in (8, 16) for predictor in range(1, 8)]
    settings += [(f'point-{precision}', {'precision': precision, 'point': precision - 1, 'predictor': 5})
                 for precision in (2, 8, 16)]
    settings += [
        ('restart', {'predictor': 7, 'restart_rows': 1}),
        ('rgb-planar', {'components': 3, 'separate': True, 'tables': True, 'predictor': 6, 'restart_rows': 2}),
        ('cmyk-planar', {'components': 4, 'separate': True, 'tables': True, 'predictor': 4}),
        ('subsampled', {'components': 3, 'sampling': [(2, 2), (1, 1), (1, 1)], 'predictor': 5, 'restart_rows': 1}),
        ('fractional', {'components': 3, 'sampling': [(3, 1), (2, 1), (1, 1)], 'predictor': 6}),
        ('subsampled-planar', {'components': 3, 'sampling': [(2, 2), (1, 1), (1, 1)], 'predictor': 3, 'separate': True}),
        ('ycbcr', {'components': 3, 'transform': 1}),
        ('ycck', {'components': 4, 'transform': 1}),
        ('adobe-rgb', {'components': 3, 'transform': 1, 'adobe': 0}),
        ('adobe-ycbcr', {'components': 3, 'adobe': 1}),
        ('16bit-ycck', {'components': 4, 'precision': 16, 'adobe': 2}),
    ]
    for suffix, options in settings:
        encoded, expected, _, _ = lossless_jpeg(**options)
        yield vector('jpeg-lossless-vector-' + suffix, 'dct', encoded, expected,
                     requirement='image-jpeg-lossless', tolerance=0, transform=options.get('transform', 0))
    # Literal codewords independently pin H.1.2.1/H.1.2.2; none is obtained
    # from lossless_jpeg's predictor/difference encoder.
    for suffix, precision, predictor, dimensions, words, expected in [
        ('category16', 16, 1, (1, 1), ['10000'], bytes([0])),
        # Native 2x2 grid [3,0;1,2]: predictor 5 gives 1+((-3)>>1)=-1.
        ('negative-half', 2, 5, (2, 2), ['00001'+'1', '00010'+'00', '00010'+'01', '00010'+'11'], bytes([255, 0, 85, 170])),
        # Native grid [65535,0;0,65535]: last prediction is -65535;
        # modulo difference 131070 becomes -2, never clamped to zero.
        ('overflow', 16, 4, (2, 2), ['01111'+'1'*15, '00001'+'1', '00001'+'1', '00010'+'01'], bytes([255, 0, 0, 255])),
    ]:
        wire = Wire()
        for word in words:
            wire.code(word)
        width, height = dimensions
        encoded = (b'\xff\xd8' + segment(0xc4, bytes([0]) + bytes([0] * 4 + [17] + [0] * 11) + bytes(range(17)))
                   + segment(0xc3, bytes([precision]) + struct.pack('>HHB', height, width, 1) + bytes([1, 17, 0]))
                   + segment(0xda, bytes([1, 1, 0, predictor, 0, 0])) + wire.bytes(jpeg=True) + b'\xff\xd9')
        yield vector('jpeg-lossless-known-' + suffix, 'dct', encoded, expected,
                     requirement='image-jpeg-lossless', tolerance=0, transform=0)
    plain, _, _, _ = lossless_jpeg()
    frame, scan, table = (plain.index(marker) for marker in (b'\xff\xc3', b'\xff\xda', b'\xff\xc4'))
    malformed = [
        ('precision-low', frame + 4, 1), ('precision-high', frame + 4, 17),
        ('quantizer', frame + 12, 1), ('predictor-zero', scan + 7, 0),
        ('predictor-eight', scan + 7, 8), ('spectral-end', scan + 8, 1),
        ('approximation-high', scan + 9, 16), ('point-range', scan + 9, 8),
        ('ac-selector', scan + 6, 1), ('missing-table', scan + 6, 0x30),
        ('category', table + 4 + 1 + 16 + 8, 17), ('sample-range', frame + 4, 7),
    ]
    for suffix, offset, value in malformed:
        damaged = plain[:offset] + bytes([value]) + plain[offset + 1:]
        yield vector('jpeg-lossless-bad-' + suffix, 'dct', damaged,
                     requirement='image-jpeg-lossless', outcome='syntax')
    for suffix, damaged in [
        ('missing-scan', plain[:scan] + plain[-2:]),
        ('missing-huffman', plain[:table] + plain[table + 2 + int.from_bytes(plain[table + 2:table + 4], 'big'):]),
        ('duplicate-scan', plain[:-2] + plain[scan:]),
        ('truncated-entropy', plain[:scan + 11] + plain[-2:]),
    ]:
        yield vector('jpeg-lossless-bad-' + suffix, 'dct', damaged,
                     requirement='image-jpeg-lossless', outcome='syntax')
    restarted, _, _, _ = lossless_jpeg(restart_rows=1)
    interval, restart = restarted.index(b'\xff\xdd'), restarted.index(b'\xff\xd0')
    for suffix, offset, value in [('restart-row', interval + 5, 4), ('restart-sequence', restart + 1, 0xd7)]:
        yield vector('jpeg-lossless-bad-' + suffix, 'dct', restarted[:offset] + bytes([value]) + restarted[offset + 1:],
                     requirement='image-jpeg-lossless', outcome='syntax')
    arithmetic, _, _, _ = lossless_jpeg(arithmetic=True)
    yield vector('jpeg-lossless-arithmetic-boundary', 'dct', arithmetic,
                 requirement='image-jpeg-lossless', outcome='unsupported', feature='jpeg.arithmetic')
    yield vector('jpeg-lossless-hierarchical-boundary', 'dct', plain[:frame + 1] + b'\xc7' + plain[frame + 2:],
                 requirement='image-jpeg-lossless', outcome='unsupported', feature='jpeg.process')


def lossless_cases():
    """Compatibility controls: T.81 H is beyond PDF 7.4.8's required processes."""
    from dataclasses import replace
    from .paint import image_case
    for suffix, options in [
        ('gray', {}),
        ('restart', {'predictor': 7, 'restart_rows': 1}),
        ('rgb', {'predictor': 4, 'components': 3}),
        ('16bit', {'precision': 16}),
        ('16bit-point', {'precision': 16, 'predictor': 5, 'point': 3}),
        ('half-predictor', {'predictor': 6}),
        ('rgb-planar', {'components': 3, 'separate': True, 'tables': True, 'predictor': 6}),
        ('rgb-subsampled', {'components': 3, 'sampling': [(2, 2), (1, 1), (1, 1)], 'predictor': 5, 'restart_rows': 1}),
        ('rgb-fractional', {'components': 3, 'sampling': [(3, 1), (2, 1), (1, 1)], 'predictor': 6}),
        ('2bit-point', {'precision': 2, 'point': 1, 'predictor': 7}),
        ('12bit', {'precision': 12, 'predictor': 3}),
        ('ycbcr', {'components': 3, 'transform': 1}),
    ]:
        encoded, expected, width, height = lossless_jpeg(**options)
        components = options.get('components', 1)
        colors = [tuple(expected[i:i + 3]) if components == 3 else (expected[i],) * 3
                  for i in range(0, len(expected), components)]
        name = f'jpeg-lossless-huffman-{suffix}'
        settings = {'matrix': (width, 0, 0, height, 2, 2), 'colors': colors, 'requirement': 'image-jpeg-lossless'}
        space = b'/DeviceRGB' if components == 3 else b'/DeviceGray'
        dictionary = (b'/BitsPerComponent 8 /ColorSpace ' + space + b'/Filter /DCTDecode /DecodeParms << /ColorTransform '
                      + str(options.get('transform', 0)).encode() + b' >>')
        yield image_case(name, encoded, dictionary, [], width, height, **settings)
        control = image_case(name + '-rgb-control', bytes(v for color in colors for v in color),
                             b'/BitsPerComponent 8 /ColorSpace /DeviceRGB', [], width, height, **settings)
        yield replace(control, extra={**control.extra, 'control_of': 'image/' + name})


def cases():
    verify_qm()
    from .image import vector, image_pdf_vector
    def encoded_vector(name, requirement, encoded, expected, exact, **options):
        case = vector(name, 'dct', encoded, expected, tolerance=1,
                      requirement=requirement, transform=options.get('transform', 0))
        if options.get('precision', 8) == 12:
            case.files['.exact'] = struct.pack('<'+'d'*len(exact), *exact)
        return case

    settings = [
        ('sequential', {}),
        ('default-conditioning', dict(dac=False)),
        ('progressive', dict(progressive=True)),
        ('progressive-bands', dict(progressive=True,banded=True,restart=2)),
        ('rgb-shared-tables',dict(components=3,transform=1,progressive=True)),
        ('restart', dict(restart=1, width=80)),
        ('progressive-restart', dict(progressive=True,restart=1,width=80)),
        ('four-tables', dict(tables=True,conditioning=[(0,0,1),(0,1,5),(3,5,16),(15,15,63)])),
        ('progressive-four-tables', dict(progressive=True,tables=True,restart=2,
            conditioning=[(0,0,1),(0,1,5),(3,5,16),(15,15,63)])),
        ('12bit', dict(precision=12)),
        ('12bit-ranges', dict(precision=12,stress=True)),
        ('12bit-bands',dict(precision=12,progressive=True,banded=True)),
        ('12bit-rgb',dict(precision=12,components=3,transform=1)),
        ('12bit-ycck',dict(precision=12,components=4,transform=1)),
        ('12bit-adobe-rgb',dict(precision=12,components=3,adobe=1)),
        ('12bit-adobe-ycck',dict(precision=12,components=4,adobe=2)),
        ('12bit-adobe-cmyk',dict(precision=12,components=4,transform=1,adobe=0)),
        ('12bit-four-tables', dict(precision=12,tables=True,progressive=True,restart=2)),
        ('12bit-progressive', dict(precision=12,progressive=True,restart=1)),
        ('wide-quantizer', dict(quantizer=65535)),
        ('zero-suffix-termination', dict(sparse=True)),
        ('empty-termination', dict(sparse=True,width=8,height=8,precision=12,sample_values=[2048])),
    ]
    for name, options in settings:
        encoded, expected, exact = jpeg(include_exact=True, **options)
        yield encoded_vector('jpeg-arithmetic-'+name, 'image-jpeg-arithmetic',
                             encoded, expected, exact, **options)
        if name in ('sequential','progressive','12bit'):
            width,height = options.get('width',24),options.get('height',16)
            dictionary = b'/BitsPerComponent 8 /ColorSpace /DeviceGray /Filter /DCTDecode'
            yield image_pdf_vector('jpeg-arithmetic-pdf-'+name, encoded, expected, dictionary,
                requirement='image-jpeg-arithmetic',width=width,height=height,space='DeviceGray',tolerance=1)
    for name, options in [('sequential', {}), ('progressive', dict(progressive=True)),
                          ('restart',dict(restart=1,width=80)),
                          ('progressive-restart',dict(progressive=True,restart=1,width=80)),
                          ('wide-quantizer',dict(quantizer=65535)),
                          ('ranges',dict(stress=True)),
                          ('rgb',dict(components=3,transform=1)),
                          ('progressive-rgb',dict(components=3,transform=1,progressive=True)),
                          ('ycck',dict(components=4,transform=1)),
                          ('adobe-rgb',dict(components=3,adobe=1)),
                          ('adobe-ycck',dict(components=4,adobe=2)),
                          ('adobe-cmyk',dict(components=4,transform=1,adobe=0)),
                          ('progressive-ranges',dict(stress=True,progressive=True)),
                          ('sample-reduction',dict(width=80,height=8,sparse=True,
                              sample_values=[0,1,15,16,17,2047,2048,4094,4095,4080]))]:
        encoded, expected, exact = jpeg(arithmetic=False,precision=12,include_exact=True,**options)
        yield encoded_vector('jpeg-12bit-'+name, 'image-jpeg-12bit',
                             encoded, expected, exact, precision=12, **options)
        if name in ('sequential','progressive'):
            dictionary = b'/BitsPerComponent 8 /ColorSpace /DeviceGray /Filter /DCTDecode'
            yield image_pdf_vector('jpeg-12bit-pdf-'+name,encoded,expected,dictionary,
                requirement='image-jpeg-12bit',width=24,height=16,space='DeviceGray',tolerance=1)
    for arithmetic in (False, True):
        encoded, expected, exact = jpeg(include_exact=True, arithmetic=arithmetic, width=128, height=64)
        yield encoded_vector('jpeg-budget-'+('arithmetic' if arithmetic else 'huffman'),
                             'image-jpeg-arithmetic', encoded, expected, exact)
    # Malformed DAC, wrong restart sequence, absent terminator and scan order.
    normal, _ = jpeg(restart=1)
    dac = normal.index(b'\xff\xcc')
    from .image import segment
    length = int.from_bytes(normal[dac+2:dac+4],'big')
    for name,payload in [('odd',b'\0'),('selector',b'\x04\x10'),('class',b'\x20\x10'),
                         ('dc-bounds',b'\0\x12'),('ac-zero',b'\x10\0'),('ac-large',b'\x10\x40')]:
        bad = normal[:dac]+segment(0xcc,payload)+normal[dac+2+length:]
        yield vector('jpeg-arithmetic-bad-dac-'+name,'dct',bad,outcome='syntax',requirement='image-jpeg-arithmetic')
    for name,bad in [('restart',normal.replace(b'\xff\xd0',b'\xff\xd1',1)),
                     ('truncated',normal[:-3]),('missing-eoi',normal[:-2]),('marker',normal[:-1])]:
        yield vector('jpeg-arithmetic-bad-'+name,'dct',bad,outcome='syntax',requirement='image-jpeg-arithmetic')

    # Explicit invalid coefficient trees (F.2.4) are encoded by Annex D,
    # so malformed entropy exercises the coefficient model rather than headers.
    ordinary, _ = jpeg(width=8,height=8)
    sos = ordinary.index(b'\xff\xda')
    prefix = ordinary[:sos+10]
    key_dc = lambda i: ('dc', 0, i)
    key_ac = lambda i: ('ac', 0, i)
    for name in ['zero-run','dc-overflow','ac-overflow']:
        coder = Encoder()
        if name == 'dc-overflow':
            coder.bit(key_dc(0),1)
            coder.bit(key_dc(1),0)
            coder.bit(key_dc(2),1)
            for i in range(20,35):
                coder.bit(key_dc(i),1)
        else:
            coder.bit(key_dc(0),0)
            coder.bit(key_ac(0),0)
            if name == 'zero-run':
                for k in range(1,64):
                    coder.bit(key_ac(3*(k-1)+1),0)
            else:
                coder.bit(key_ac(1),1)
                coder.bit(None,0)
                coder.bit(key_ac(2),1)
                coder.bit(key_ac(2),1)
                for i in range(189,203):
                    coder.bit(key_ac(i),1)
        yield vector('jpeg-arithmetic-bad-'+name,'dct',prefix+coder.finish()+b'\xff\xd9',
                     outcome='syntax',requirement='image-jpeg-arithmetic')
    progressive, _ = jpeg(progressive=True)
    at = progressive.index(b'\xff\xda')
    for name,header in [('refinement-order',bytes([1,1,0,0,0,0x10])),
                        ('ac-before-dc',bytes([1,1,0,1,63,0])),
                        ('selector',bytes([1,1,0x44,0,0,0]))]:
        yield vector('jpeg-arithmetic-bad-'+name,'dct',progressive[:at]+segment(0xda,header)+progressive[at+10:],
                     outcome='syntax',requirement='image-jpeg-arithmetic')
    yield vector('jpeg-arithmetic-bad-stuffing','dct',ordinary[:-2]+b'\xff\xff\0\xff\xd9',
                 outcome='syntax',requirement='image-jpeg-arithmetic')
    # DAC entries can be supplied in separate segments and replaced again.
    at = ordinary.index(b'\xff\xcc')
    size = int.from_bytes(ordinary[at+2:at+4],'big')
    entries = ordinary[at+4:at+2+size]
    split = b''.join(segment(0xcc,entries[i:i+2]) for i in range(0,len(entries),2))
    _,expected = jpeg(width=8,height=8)
    yield vector('jpeg-arithmetic-dac-redefined','dct',ordinary[:at]+split+segment(0xcc,entries)+ordinary[at+2+size:],
                 expected,tolerance=1,requirement='image-jpeg-arithmetic')
    # Baseline precision remains restricted to eight bits (Table B.2).
    encoded, _ = jpeg(arithmetic=False,precision=12)
    yield vector('jpeg-baseline-12bit-rejected','dct',encoded.replace(b'\xff\xc1',b'\xff\xc0',1),
        outcome='unsupported',feature='jpeg.precision',requirement='image-jpeg-12bit')
    encoded, _ = jpeg(arithmetic=False,precision=12)
    yield image_pdf_vector('jpeg-12bit-pdf-bpc-mismatch',encoded,[],
        b'/BitsPerComponent 16 /ColorSpace /DeviceGray /Filter /DCTDecode',
        requirement='image-jpeg-12bit',width=24,height=16,outcome='syntax')
    # Complete one-page images exercise painting and independent byte oracles.
    from .model import Case, RENDER
    from .pdf import document, stream
    for name,options in [('arithmetic',{}),('arithmetic-progressive',dict(progressive=True,restart=1)),
                         ('12bit',dict(arithmetic=False,precision=12)),
                         ('12bit-progressive',dict(arithmetic=False,precision=12,progressive=True))]:
        encoded, expected = jpeg(**options)
        pdf = document([
            b'<< /Type /Catalog /Pages 2 0 R >>',
            b'<< /Type /Pages /Kids [3 0 R] /Count 1 >>',
            b'<< /Type /Page /Parent 2 0 R /MediaBox [0 0 24 16] /Resources << /XObject << /Im 5 0 R >> >> /Contents 4 0 R >>',
            stream(b'q 24 0 0 16 0 0 cm /Im Do Q'),
            stream(encoded,b'/Type /XObject /Subtype /Image /Width 24 /Height 16 /BitsPerComponent 8 /ColorSpace /DeviceGray /Filter /DCTDecode'),
        ])
        oracle = b'P6\n24 16\n255\n' + bytes(v for value in expected for v in [value]*3)
        yield Case('jpeg-render-'+name,pdf,RENDER,
                   ('image-jpeg-arithmetic' if options.get('arithmetic',True) else 'image-jpeg-12bit',),
                   'JPEG native-precision reconstruction painted on a pixel-aligned grid',
                   oracle=oracle,extra={'tolerance':1})
    yield from lossless_vectors()
    yield from lossless_cases()
