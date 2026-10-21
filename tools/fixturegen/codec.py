"""Independent ISO 32000-2 7.4/RFC 1950/1951 stream-filter oracles.

No Pagura execution/imports. Python's zlib/base64/binascii are independent
encoders; the bitstream, LZW, run-length and predictor encoders below are
written from the formats. .params is ASCII key=value, one entry per line.
"""
import base64
import binascii
import random
import struct
import zlib

from .model import Vector

AREA = "codec"


def vector(name, encoded, decoded, requirement, filter="FlateDecode", *,
           outcome="valid", repair="none", cap=None, **params):
    options = dict(filter=filter, outcome=outcome, repair=repair,
                   cap=len(decoded) if cap is None else cap, **params)
    return Vector(name, {".in": encoded, ".out": decoded,
                        ".params": "".join(f"{k}={v}\n" for k, v in options.items()).encode()},
                  (requirement,), name.replace("-", " "),
                  extra={"oracle_source": "independent Python standard-library/format encoder"})


class Bits:
    """RFC 1951 least-significant-bit-first fields."""
    def __init__(self):
        self.values = []

    def put(self, value, count):
        self.values.extend((value >> i) & 1 for i in range(count))

    def bytes(self):
        out = bytearray((len(self.values) + 7) // 8)
        for i, bit in enumerate(self.values):
            out[i // 8] |= bit << (i % 8)
        return bytes(out)


def codes(lengths):
    """Canonical codes as (wire value, bit count), RFC 1951 3.2.2."""
    counts = [lengths.count(i) for i in range(16)]
    counts[0] = 0
    next_code = [0] * 16
    for i in range(1, 16):
        next_code[i] = (next_code[i - 1] + counts[i - 1]) << 1
    result = {}
    for symbol, length in enumerate(lengths):
        if length:
            code = next_code[length]
            next_code[length] += 1
            reversed_code = int(f"{code:0{length}b}"[::-1], 2)
            result[symbol] = reversed_code, length
    return result


def fixed(symbols):
    bits = Bits()
    bits.put(3, 3)  # final, fixed Huffman
    literal = codes([8] * 144 + [9] * 112 + [7] * 24 + [8] * 8)
    distance = codes([5] * 32)
    for item in symbols:
        if isinstance(item, int):
            bits.put(*literal[item])
        else:
            # Explicit (length code, length extra, extra bits, distance code,
            # distance extra, extra bits), independent edge constructions.
            lc, le, lb, dc, de, db = item
            bits.put(*literal[lc]); bits.put(le, lb)
            bits.put(*distance[dc]); bits.put(de, db)
    bits.put(*literal[256])
    return bits.bytes()


def dynamic(lengths, distances, payload, *, code_lengths=None, runs=None):
    bits = Bits()
    bits.put(5, 3)  # final, dynamic
    bits.put(len(lengths) - 257, 5)
    bits.put(len(distances) - 1, 5)
    bits.put(15, 4)  # all 19 code lengths
    order = [16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15]
    if code_lengths is None:
        code_lengths = [4] * 16 + [0] * 3  # complete code-length alphabet
    for symbol in order:
        bits.put(code_lengths[symbol], 3)
    table = codes(code_lengths)
    for value in lengths + distances if runs is None else runs:
        if isinstance(value, int):
            bits.put(*table[value])
        else:
            symbol, extra, width = value
            bits.put(*table[symbol]); bits.put(extra, width)
    literal = codes(lengths)
    distance = codes(distances)
    for item in payload:
        if isinstance(item, int):
            bits.put(*literal[item])
        else:
            lc, le, lb, dc, de, db = item
            bits.put(*literal[lc]); bits.put(le, lb)
            bits.put(*distance[dc]); bits.put(de, db)
    return bits.bytes()


def zlib_stream(raw, plain, *, cinfo=7, level=2, dictionary=None):
    """RFC 1950 2.2 framing; checksum from independently weighted byte sums."""
    cmf = cinfo * 16 + 8
    flg = level * 64 + (32 if dictionary is not None else 0)
    flg += (-(cmf * 256 + flg)) % 31
    size = len(plain)
    s1 = (1 + sum(plain)) % 65521
    s2 = (size + sum((size - i) * byte for i, byte in enumerate(plain))) % 65521
    return (bytes([cmf, flg]) + (b"" if dictionary is None else struct.pack(">I", dictionary))
            + raw + struct.pack(">I", s2 * 65536 + s1))


def external_format_cases():
    # RFC 1950 2.2/2.3: every defined window and advisory FLEVEL, valid FCHECK.
    for cinfo in range(8):
        for level in range(4):
            encoded = zlib_stream(fixed([65]), b"A", cinfo=cinfo, level=level)
            assert zlib.decompress(encoded) == b"A"
            yield vector(f"zlib-rfc1950-cinfo{cinfo}-level{level}", encoded, b"A", "filter-flate")
    yield vector("zlib-rfc1950-invalid-cinfo", zlib_stream(fixed([]), b"", cinfo=8), b"",
                 "filter-flate", outcome="syntax")
    yield vector("zlib-rfc1950-fdict", zlib_stream(fixed([]), b"", dictionary=0x12345678), b"",
                 "filter-flate", outcome="syntax")
    # The CINFO=0 window is 256, even when more decoded history is available.
    for distance in (256, 257):
        prefix_length = distance - 1
        raw = fixed([65, (284, prefix_length - 227, 5, 0, 0, 0),
                     (257, 0, 0, 15 if distance == 256 else 16,
                      63 if distance == 256 else 0, 6 if distance == 256 else 7)])
        plain = b"A" * (distance + 3)
        encoded = zlib_stream(raw, plain, cinfo=0)
        yield vector(f"zlib-rfc1950-window-distance{distance}", encoded,
                     plain if distance == 256 else b"", "filter-flate",
                     outcome="valid" if distance == 256 else "syntax", cap=len(plain))
    # RFC 1951 3.2.6 literal boundaries, including the two literal bit widths.
    plain = bytes([0, 143, 144, 255])
    yield vector("flate-rfc1951-fixed-literal-boundaries", fixed(list(plain)), plain,
                 "filter-flate", raw=1)
    # RFC 1951 3.2.7 permits a one-bit distance alphabet with one unused branch.
    lengths = [0] * 258
    lengths[65], lengths[256], lengths[257] = 1, 2, 2
    encoded = dynamic(lengths, [1], [65, (257, 0, 0, 0, 0, 0), 256])
    assert zlib.decompress(encoded, -15) == b"AAAA"
    yield vector("flate-rfc1951-single-distance", encoded, b"AAAA", "filter-flate", raw=1)
    # One repeat-16 run spans the literal/distance boundary: EOB, length-3,
    # and two distance codes all have length one (3.2.7's single sequence).
    lengths = [0] * 256 + [1, 1]
    code_lengths = [0] * 19
    code_lengths[1], code_lengths[16], code_lengths[18] = 1, 2, 2
    encoded = dynamic(lengths, [1, 1], [256], code_lengths=code_lengths,
                      runs=[(18, 127, 7), (18, 107, 7), 1, (16, 0, 2)])
    assert zlib.decompress(encoded, -15) == b""
    yield vector("flate-rfc1951-repeat16-across-alphabets", encoded, b"", "filter-flate", raw=1)
    # Exercise zero-repeat 17 alongside 18, rather than just zero literals.
    code_lengths = [0] * 19
    for symbol in (0, 1, 17, 18): code_lengths[symbol] = 2
    encoded = dynamic([0] * 256 + [1], [0], [256], code_lengths=code_lengths,
                      runs=[(18, 127, 7), (18, 104, 7), (17, 0, 3), 1, 0])
    assert zlib.decompress(encoded, -15) == b""
    yield vector("flate-rfc1951-repeat17-zero", encoded, b"", "filter-flate", raw=1)
    # PNG Second Edition 9.3: floor(509/2)=254, without eight-bit sum overflow.
    # Clause 9.4: equal nearest distances choose left before corner and above
    # before corner. These hand-derived rows do not use the predictor encoder.
    for label, raw, plain in [
        ("average-overflow", bytes([0, 0, 254, 3, 255, 2]), bytes([0, 254, 255, 0])),
        ("paeth-tie-left", bytes([0, 1, 0, 4, 2, 0]), bytes([1, 0, 3, 3])),
        ("paeth-tie-above", bytes([0, 1, 3, 4, 255, 0]), bytes([1, 3, 0, 3])),
        ("paeth-wide-sum", bytes([0, 0, 255, 4, 255, 1]), bytes([0, 255, 255, 0])),
    ]:
        # Adler-32 covers raw bytes including tags, before predictor reversal.
        encoded = zlib_stream(fixed(list(raw)), raw)
        assert zlib.decompress(encoded) == raw
        yield vector("png-w3c-" + label, encoded, plain, "predictor-png", predictor=15, columns=2)
    # TIFF 6.0 section 14 pp.64-65: sample-wise modular differences, stride
    # SamplesPerPixel, reset each row. PDF 7.4.4.4 supplies MSB-first packing.
    for depth in (1, 2, 4, 8, 16):
        maximum = (1 << depth) - 1
        raw = pack_samples([maximum, 1, 1, maximum], depth) * 2
        plain = pack_samples([maximum, 1, 0, 0], depth) * 2
        yield vector(f"tiff6-predictor2-wrap-b{depth}", zlib_stream(fixed(list(raw)), raw),
                     plain, "predictor-tiff", predictor=2, colors=2, bits=depth, columns=2)


def lzw(data, early, reset_at=4096):
    """Phrase encoder with independent decoder-width accounting (7.4.4.2).

    Dictionary insertion leads the decoder by one entry, except at EOD.
    Width follows codes the decoder has consumed, not encoder table size.
    """
    dictionary = {bytes([i]): i for i in range(256)}
    next_code = 258
    output = [(256, 9)]
    width = 9
    decoder_next = 258
    has_previous = False

    def emit(code):
        nonlocal width, decoder_next, has_previous
        output.append((code, width))
        if code == 256:
            width, decoder_next, has_previous = 9, 258, False
        elif code != 257:
            if has_previous and decoder_next < 4096:
                decoder_next += 1
                if width < 12 and decoder_next == (1 << width) - early:
                    width += 1
            has_previous = True

    phrase = b""
    for byte in data:
        symbol = bytes([byte])
        combined = phrase + symbol
        if combined in dictionary:
            phrase = combined
            continue
        emit(dictionary[phrase])
        if next_code < 4096:
            dictionary[combined] = next_code
            next_code += 1
        if next_code == reset_at:
            emit(256)
            dictionary = {bytes([i]): i for i in range(256)}
            next_code = 258
        phrase = symbol
    if phrase:
        emit(dictionary[phrase])
    emit(257)
    bit_string = "".join(f"{code:0{size}b}" for code, size in output)
    bit_string += "0" * (-len(bit_string) % 8)
    return bytes(int(bit_string[i:i + 8], 2) for i in range(0, len(bit_string), 8))


def lzw_literal_codes(count, early, clear=False):
    """Literal-only boundary stream, ISO 32000-2 7.4.4.2 and Table 8.

    Width follows the ordinal of the literal: the encoder makes entry 511
    after literal 254; EarlyChange=0 delays the next-width code once.
    Unlike the phrase encoder, this construction has no adaptive dictionary.
    """
    fields = [(256, 9)]
    for ordinal in range(count):
        width = min(12, max(9, (257 + ordinal + early).bit_length()))
        fields.append((65, width))
    width = min(12, max(9, (257 + count + early).bit_length()))
    if clear:
        fields.extend([(256, width), (66, 9), (257, 9)])
    else:
        fields.append((257, width))
    bits = "".join(f"{code:0{width}b}" for code, width in fields)
    bits += "0" * (-len(bits) % 8)
    return bytes(int(bits[i:i + 8], 2) for i in range(0, len(bits), 8))


def run_length(data):
    # Literal packets are sufficient as an independent general encoder;
    # explicit repeat packets exercise all repeat lengths separately.
    return b"".join(bytes([len(data[i:i + 128]) - 1]) + data[i:i + 128]
                    for i in range(0, len(data), 128)) + b"\x80"


def pack_samples(samples, bits):
    value = "".join(f"{sample:0{bits}b}" for sample in samples)
    value += "0" * (-len(value) % 8)
    return bytes(int(value[i:i + 8], 2) for i in range(0, len(value), 8))


def paeth(a, b, c):
    p = a + b - c
    return min([(abs(p - a), 0, a), (abs(p - b), 1, b), (abs(p - c), 2, c)])[2]


def predictors(bits, colors, columns, png):
    samples = [[(i * 29 + row * 43 + (i // colors) * 7) % (1 << bits)
                for i in range(colors * columns)] for row in range(6)]
    rows = [pack_samples(row, bits) for row in samples]
    if not png:
        encoded = []
        for row in samples:
            deltas = [(value - (row[i - colors] if i >= colors else 0)) % (1 << bits)
                      for i, value in enumerate(row)]
            encoded.append(pack_samples(deltas, bits))
        return b"".join(encoded), b"".join(rows)
    encoded = bytearray()
    previous = bytes(len(rows[0]))
    stride = (bits * colors + 7) // 8
    for row_index, row in enumerate(rows):
        tag = row_index % 5
        encoded.append(tag)
        for i, byte in enumerate(row):
            left = row[i - stride] if i >= stride else 0
            above = previous[i]
            upper_left = previous[i - stride] if i >= stride else 0
            prediction = [0, left, above, (left + above) // 2, paeth(left, above, upper_left)][tag]
            encoded.append((byte - prediction) & 255)
        previous = row
    return bytes(encoded), b"".join(rows)


def cases():
    yield from encoding_cases()
    yield from external_format_cases()
    rng = random.Random(0x50414755)
    data = bytes(rng.randrange(256) for _ in range(4096)) + b"ABC\0" * 2048
    strategies = [zlib.Z_DEFAULT_STRATEGY, zlib.Z_FILTERED, zlib.Z_HUFFMAN_ONLY, zlib.Z_RLE, zlib.Z_FIXED]
    for level in range(10):
        for strategy in strategies:
            for raw in (False, True):
                encoder = zlib.compressobj(level, zlib.DEFLATED, -15 if raw else 15, 8, strategy)
                encoded = encoder.compress(data) + encoder.flush()
                yield vector(f"flate-l{level}-s{strategy}-raw{int(raw)}", encoded, data, "filter-flate",
                             repair="filter.raw-deflate" if raw else "none")
    for length in (0, 65535):
        plain = bytes(i % 251 for i in range(length))
        encoded = b"\x01" + struct.pack("<HH", length, length ^ 65535) + plain
        yield vector(f"flate-stored-{length}", encoded, plain, "filter-flate", raw=1)
    for padding in (0x08, 0x78, 0x88, 0xf8):
        encoded = bytes([padding]) + b"\0\0\xff\xff" + fixed([])
        assert zlib.decompress(encoded, -15) == b""
        yield vector(f"flate-raw-stored-padding-{padding}", encoded, b"", "filter-flate", repair="filter.raw-deflate")
    # The first two bytes are also a valid zlib header. An explicitly raw
    # container must bypass automatic detection; no speculative fallback.
    encoded = b"\x08\x1d\0\xe2\xff" + b"A" * 29 + fixed([])
    assert zlib.decompress(encoded, -15) == b"A" * 29
    yield vector("flate-raw-zlib-header-collision", encoded, b"A" * 29, "filter-flate", raw=1)
    yield vector("flate-raw-repair-fixed", fixed([65, (285, 0, 0, 0, 0, 0)]), b"A" * 259,
                 "filter-repairs", repair="filter.raw-deflate")
    yield vector("flate-empty-fixed", fixed([]), b"", "filter-flate", raw=1)
    yield vector("flate-length258-overlap", fixed([65, (285, 0, 0, 0, 0, 0)]), b"A" * 259, "filter-flate", raw=1)
    window = bytes(rng.randrange(256) for _ in range(32768))
    encoded = b"\0" + struct.pack("<HH", 32768, 32767) + window + fixed([(285, 0, 0, 29, 8191, 13)])
    yield vector("flate-distance32768", encoded, window + window[:258], "filter-flate", raw=1)
    lengths = [0] * 258
    for i in range(14):
        lengths[65 + i] = i + 1
    lengths[256] = lengths[257] = 15
    encoded = dynamic(lengths, [0], [65, 78, 65, 256])
    assert zlib.decompress(encoded, -15) == b"ANA"
    yield vector("flate-dynamic-length15", encoded, b"ANA", "filter-flate", raw=1)
    lengths = [0] * 257
    lengths[256] = 1
    encoded = dynamic(lengths, [0], [256])
    assert zlib.decompress(encoded, -15) == b""
    yield vector("flate-dynamic-single-empty-distance", encoded, b"", "filter-flate", raw=1)
    for label, lit, dist, payload in [
        ("literal-incomplete", {65: 2, 256: 2}, [0], []),
        ("literal-oversubscribed", {65: 1, 66: 1, 256: 1}, [0], []),
        ("missing-eob", {65: 1, 66: 1}, [0], []),
        ("distance-incomplete", {256: 1}, [2], []),
        ("distance-oversubscribed", {256: 1}, [1, 1, 1], []),
        ("empty-distance-used", {256: 1, 257: 1}, [0], [257]),
    ]:
        lengths = [0] * (max(256, *lit) + 1)
        for symbol, length in lit.items(): lengths[symbol] = length
        yield vector("flate-invalid-" + label, dynamic(lengths, dist, payload), b"", "filter-flate", outcome="syntax", cap=4096, raw=1)
    for label, symbol in [("repeat-without-previous", 16), ("repeat-overflow", 18)]:
        bits = Bits(); bits.put(5, 3); bits.put(0, 5); bits.put(0, 5); bits.put(0, 4)
        for value in ([1, 0, 0, 1] if symbol == 16 else [0, 0, 1, 1]): bits.put(value, 3)
        bits.put(1, 1); bits.put(0 if symbol == 16 else 127, 2 if symbol == 16 else 7)
        if symbol == 18: bits.put(1, 1); bits.put(127, 7)
        yield vector("flate-invalid-" + label, bits.bytes(), b"", "filter-flate", outcome="syntax", cap=4096, raw=1)
    ring = bytes(i % 251 for i in range(32868))
    encoded = b"\0" + struct.pack("<HH", len(ring), len(ring) ^ 65535) + ring + fixed([(285, 0, 0, 15, 7, 6)])
    plain = ring + (ring[-200:] * 2)[:258]
    assert zlib.decompress(encoded, -15) == plain
    yield vector("flate-ring-wrap-overlap", encoded, plain, "filter-flate", raw=1)
    # Predictor reversal must not corrupt the circular pre-predictor history.
    raw_plain = plain
    predicted = bytes(value for a, b in zip(raw_plain[::2], raw_plain[1::2])
                      for value in (a, (a + b) & 255))
    wrapped = b"\x78\x9c" + encoded + struct.pack(">I", zlib.adler32(raw_plain))
    assert zlib.decompress(wrapped) == raw_plain
    yield vector("predictor-tiff-ring-wrap", wrapped, predicted, "predictor-tiff", predictor=2, columns=2)
    # Fifteen decoded one-byte PNG rows have thirty pre-predictor bytes.
    # A valid distance 24 exceeds the final cap of 15, exposing a cap-sized
    # raw-history window that forgets tags too soon.
    prefix = b"".join(bytes([0, value]) for value in range(12))
    raw_plain = prefix + prefix[:6]
    raw_encoded = b"\0" + struct.pack("<HH", len(prefix), len(prefix) ^ 65535) + prefix + fixed([(260, 0, 0, 8, 7, 3)])
    wrapped = b"\x78\x9c" + raw_encoded + struct.pack(">I", zlib.adler32(raw_plain))
    assert zlib.decompress(wrapped) == raw_plain
    yield vector("predictor-png-distance-beyond-cap", wrapped, bytes(range(12)) + bytes(range(3)),
                 "predictor-png", predictor=15, columns=1)
    # Every length-code minimum/maximum and every distance-code maximum.
    length_bases = [3,4,5,6,7,8,9,10,11,13,15,17,19,23,27,31,35,43,51,59,67,83,99,115,131,163,195,227,258]
    length_extras = [0]*8 + [1]*4 + [2]*4 + [3]*4 + [4]*4 + [5]*4 + [0]
    for index, (base, extra) in enumerate(zip(length_bases, length_extras)):
        maximum = (1 << extra) - 1
        # Exclude code 284's extra=31: the RFC table assigns it 227..257.
        # Code 285 independently exercises length 258.
        for value in sorted({0, maximum - int(index == 27)}):
            encoded = fixed([90, (257 + index, value, extra, 0, 0, 0)])
            plain = b"Z" * (1 + base + value)
            assert zlib.decompress(encoded, -15) == plain
            yield vector(f"flate-length-code-{257 + index}-{value}", encoded, plain, "filter-flate", raw=1)
    distance_bases = [1,2,3,4,5,7,9,13,17,25,33,49,65,97,129,193,257,385,513,769,1025,1537,2049,3073,4097,6145,8193,12289,16385,24577]
    distance_extras = [0]*4 + [i for i in range(1,14) for _ in range(2)]
    symbols = []
    plain = bytearray(window)
    for index, (base, extra) in enumerate(zip(distance_bases, distance_extras)):
        value = (1 << extra) - 1
        distance = base + value
        symbols.append((285, 0, 0, index, value, extra))
        for _ in range(258): plain.append(plain[-distance])
    encoded = b"\0" + struct.pack("<HH", 32768, 32767) + window + fixed(symbols)
    assert zlib.decompress(encoded, -15) == plain
    yield vector("flate-all-distance-codes", encoded, bytes(plain), "filter-flate", raw=1)
    good = zlib.compress(b"hello")
    for name, encoded, decoded, tag in [
        ("checksum", good[:-1] + bytes([good[-1] ^ 1]), b"hello", "filter.checksum-mismatch"),
        ("truncated-checksum", good[:-2], b"hello", "filter.truncated"),
        ("truncated-block", b"\x01\x05\0\xfa\xffhe", b"he", "filter.truncated"),
        ("trailing", good + b"junk", b"hello", "filter.trailing-data"),
        ("truncated-header", b"\x78", b"", "filter.truncated"),
        ("truncated-empty", b"", b"", "filter.truncated"),
    ]:
        yield vector("flate-" + name, encoded, decoded, "filter-repairs", repair=tag, raw=int(name in ("truncated-block", "truncated-empty")))
    for name, encoded in [
        ("block-type", b"\x07"), ("bad-fcheck", bytes([good[0], good[1] ^ 1]) + good[2:]),
        ("bad-stored-length", b"\x01\x01\0\0\0x"),
        ("bad-distance", fixed([(257, 0, 0, 0, 0, 0)])),
        ("length284-overflow", fixed([65, (284, 31, 5, 0, 0, 0)])),
        ("reserved-distance", fixed([65, (257, 0, 0, 30, 0, 0)])),
        ("reserved-literal", fixed([286])),
        ("preset-dictionary", b"\x78\x20"),
    ]:
        yield vector("flate-invalid-" + name, encoded, b"", "filter-flate", outcome="syntax", cap=4096, raw=int(name not in ("bad-fcheck", "preset-dictionary")))
    for name, code_lengths in [("oversubscribed", [1, 1, 1, 0]), ("incomplete", [1, 0, 0, 0])]:
        bits = Bits(); bits.put(5, 3); bits.put(0, 5); bits.put(0, 5); bits.put(0, 4)
        for value in code_lengths: bits.put(value, 3)
        yield vector("flate-invalid-" + name, bits.bytes(), b"", "filter-flate", outcome="syntax", cap=4096, raw=1)
    yield vector("flate-bomb", zlib.compress(b"A" * 1_000_000), b"", "decoder-output-cap", outcome="limit", cap=31)
    for early in (0, 1):
        for label, plain, reset in [("empty", b"", 4096), ("special", b"ABABABA" * 200, 4096),
                                    ("widths-reset", data * 4, 4096), ("early-reset", data, 1024),
                                    ("frozen-table", data * 4, 4097)]:
            # 7.4.4.2 requires clear on table exhaustion; a frozen
            # dictionary that continues producing data is malformed.
            yield vector(f"lzw-ec{early}-{label}", lzw(plain, early, reset), plain,
                         "filter-lzw", "LZWDecode", early=early,
                         repair="filter.lzw-full-table" if label == "frozen-table" else "none")
    yield vector("lzw-truncated", lzw(b"hello", 1)[:-2], b"hell", "filter-repairs", "LZWDecode", repair="filter.truncated")
    yield vector("lzw-trailing", lzw(b"hello", 1) + b"\0", b"hello", "filter-repairs", "LZWDecode", repair="filter.trailing-data")
    yield vector("lzw-invalid-code", bytes.fromhex("804b40"), b"", "filter-lzw", "LZWDecode", outcome="syntax", cap=4096)
    for length in range(10):
        plain = data[:length]
        yield vector(f"a85-partial-{length}", base64.a85encode(plain) + b"~>", plain, "filter-ascii85", "ASCII85Decode")
    yield vector("a85-z-whitespace", b"z\x00\t\r\n\x0c ! ! ! ! !~> \n", b"\0" * 8, "filter-ascii85", "ASCII85Decode")
    yield vector("a85-missing-end", b"BOu!rDZ", b"hello", "filter-repairs", "ASCII85Decode", repair="filter.truncated")
    for i, encoded in enumerate([b"!z~>", b"v~>", b"!~>", b"uuuuu~>", b"~~>", b"<~z~>"]):
        yield vector(f"a85-invalid-{i}", encoded, b"", "filter-ascii85", "ASCII85Decode", outcome="syntax", cap=4096)
    yield vector("hex-binascii", binascii.hexlify(data) + b">", data, "filter-asciihex", "ASCIIHexDecode")
    yield vector("hex-odd-whitespace", b" a\x00B\tC\rD\nE\x0c>", bytes.fromhex("abcde0"), "filter-asciihex", "ASCIIHexDecode")
    yield vector("hex-missing-end", b"123", b"\x12\x30", "filter-repairs", "ASCIIHexDecode", repair="filter.truncated")
    for label, filter_name, encoded, plain in [
        ("hex", "ASCIIHexDecode", b"61>junk", b"a"),
        ("a85", "ASCII85Decode", b"z~>x", b"\0" * 4),
        ("run", "RunLengthDecode", b"\0x\x80\0", b"x"),
    ]:
        yield vector(label + "-trailing-data", encoded, plain, "filter-repairs", filter_name, repair="filter.trailing-data")
    yield vector("hex-invalid", b"gg>", b"", "filter-asciihex", "ASCIIHexDecode", outcome="syntax", cap=4096)
    yield vector("run-literals", run_length(data), data, "filter-runlength", "RunLengthDecode")
    yield vector("run-repeats", b"\x81A\xffB\x80", b"A" * 128 + b"BB", "filter-runlength", "RunLengthDecode")
    yield vector("run-truncated", b"\x04ab", b"ab", "filter-repairs", "RunLengthDecode", repair="filter.truncated")
    yield vector("run-repeat-truncated", b"\x81", b"", "filter-repairs", "RunLengthDecode", repair="filter.truncated")
    for bits in (1, 2, 4, 8, 16):
        for colors in (1, 3, 5, 255):
            for columns in (1, 7):
                for png in (False, True):
                    encoded, plain = predictors(bits, colors, columns, png)
                    yield vector(f"predictor-b{bits}-c{colors}-n{columns}-p{int(png)}", zlib.compress(encoded), plain,
                                 "predictor-png" if png else "predictor-tiff", predictor=15 if png else 2,
                                 bits=bits, colors=colors, columns=columns)
    for early in (0, 1):
        for bits in (1, 2, 4, 8, 16):
            for png in (False, True):
                encoded, plain = predictors(bits, 3, 7, png)
                yield vector(f"lzw-predictor-ec{early}-b{bits}-p{int(png)}", lzw(encoded, early), plain,
                             "predictor-png" if png else "predictor-tiff", "LZWDecode", early=early,
                             predictor=15 if png else 2, bits=bits, colors=3, columns=7)
    # Samples [3, 0, 3] give differences [3, 1, 3] modulo four;
    # preserve the unused two low padding bits (11) without prediction.
    yield vector("tiff-padding-preserved", zlib.compress(b"\xdf"), b"\xcf", "predictor-tiff", predictor=2, bits=2, columns=3)
    for name, plain in [("invalid-tag", b"\x05x"), ("partial-row", b"\0x"), ("tag-only", b"\0")]:
        yield vector("predictor-" + name, zlib.compress(plain), b"", "predictor-png", outcome="syntax", cap=4096, predictor=15, columns=2)

    # ISO 32000-2 7.4.4.2 requires zero unused bits after EOD.
    encoded = lzw(b"", 1)
    yield vector("lzw-eod-nonzero-padding", encoded[:-1] + bytes([encoded[-1] | 1]), b"",
                 "filter-lzw", "LZWDecode", repair="filter.lzw-padding", cap=1)
    for early in (0, 1):
        for count in (254, 255, 766, 767, 1790, 1791, 3838):
            yield vector(f"lzw-iso-ec{early}-literal-boundary-{count}", lzw_literal_codes(count, early),
                         b"A" * count, "filter-lzw", "LZWDecode", early=early)
        yield vector(f"lzw-iso-ec{early}-full-clear", lzw_literal_codes(3838, early, clear=True),
                     b"A" * 3838 + b"B", "filter-lzw", "LZWDecode", early=early)
        yield vector(f"lzw-iso-ec{early}-missing-full-clear", lzw_literal_codes(3839, early),
                     b"A" * 3839, "filter-lzw", "LZWDecode", early=early, repair="filter.lzw-full-table")
    # 7.3.7/7.3.9 treat null dictionary entries as absent; Table 8 defaults apply.
    yield vector("flate-null-predictor-parameters", zlib.compress(b"x"), b"x", "filter-flate",
                 predictor="null", colors="null", bits="null", columns="null")
    yield vector("lzw-null-predictor-parameters", lzw(b"x", 1), b"x", "filter-lzw", "LZWDecode",
                 predictor="null", colors="null", bits="null", columns="null", early="null")
    # 7.4.3's partial-group construction: zero-fill binary, convert, then trim.
    for count in (1, 2, 3, 4):
        plain = b"\xff" * count
        number = int.from_bytes(plain + bytes(4 - count), "big")
        digits = []
        for _ in range(5):
            number, digit = divmod(number, 85)
            digits.append(digit + 33)
        encoded = bytes(reversed(digits))[:count + 1] + b"~>"
        yield vector(f"a85-iso-max-partial-{count}", encoded, plain, "filter-ascii85", "ASCII85Decode")
    # 7.4.4.4 permits any PNG row tag for every Predictor value 10..15.
    for predictor in range(10, 16):
        yield vector(f"png-iso-row-tag-predictor-{predictor}", zlib.compress(b"\x02\x07\x02\x03"),
                     bytes([7, 10]), "predictor-png", predictor=predictor)
    # 7.4.1 / Table 5: filters run in array order with corresponding parameters.
    yield vector("filter-chain-iso-order", b"ff4180>", b"AA", "filter-chain",
                 "ASCIIHexDecode,RunLengthDecode", cap=3)
    # 7.4.10 / Table 14: Crypt is first, optional Type and Name are checked.
    yield vector("crypt-iso-default-null", b"41>", b"A", "filter-crypt-boundary", "Crypt,ASCIIHexDecode",
                 crypt_type="null", crypt_name="null", take_crypt=1)
    yield vector("crypt-iso-correct-type", b"41>", b"A", "filter-crypt-boundary", "Crypt,ASCIIHexDecode",
                 crypt_type="/CryptFilterDecodeParms", take_crypt=1)
    for name, filter_name, parameters in [
        ("not-first", "ASCIIHexDecode,Crypt", {}),
        ("wrong-type", "Crypt", {"crypt_type": "/Other"}),
        ("wrong-name-type", "Crypt", {"crypt_name": 1}),
    ]:
        yield vector("crypt-iso-invalid-" + name, b"", b"", "filter-crypt-boundary", filter_name,
                     outcome="syntax", parameter_error=1, **parameters)

    # 7.4.3 ignores every Table 1 whitespace byte while recognizing EOD.
    yield vector("a85-eod-whitespace", b"z~\x00\t\n\x0c\r >", bytes(4),
                 "filter-ascii85", "ASCII85Decode")

    encoded = lzw(b"x", 1)
    yield vector("lzw-eod-nonzero-padding-literal", encoded[:-1] + bytes([encoded[-1] | 1]), b"x",
                 "filter-lzw", "LZWDecode", repair="filter.lzw-padding")
    # Each literal creates AA entries. Entry 4095 therefore means AA as well;
    # repeated use must preserve its value after freezing. Two full-table
    # episodes separated by clear still require only one stream diagnostic.
    for early in (0, 1):
        fields = [(256, 9)]
        for _ in range(2):
            for ordinal in range(3838):
                width = min(12, max(9, (257 + ordinal + early).bit_length()))
                fields.append((65, width))
            fields.extend([(4095, 12), (4095, 12), (256, 12)])
        fields.extend([(66, 9), (257, 9)])
        bits = "".join(f"{code:0{width}b}" for code, width in fields)
        bits += "0" * (-len(bits) % 8)
        encoded = bytes(int(bits[i:i + 8], 2) for i in range(0, len(bits), 8))
        yield vector(f"lzw-iso-ec{early}-frozen-entry4095-clear", encoded, b"A" * (3842 * 2) + b"B",
                     "filter-lzw", "LZWDecode", early=early, repair="filter.lzw-full-table")


def png_filter_candidates(row, previous, channels):
    """PNG 9.2-9.4/12.8: independent forward filters and signed-byte scores."""
    candidates = [bytearray() for _ in range(5)]
    for i, value in enumerate(row):
        a = row[i-channels] if i >= channels else 0
        b = previous[i] if previous else 0
        c = previous[i-channels] if previous and i >= channels else 0
        for target, prediction in zip(candidates, [0, a, b, (a+b)//2, paeth(a,b,c)]):
            target.append((value - prediction) % 256)
    scores = [sum(abs(value if value < 128 else value - 256) for value in row)
              for row in candidates]
    tag = min(range(5), key=lambda n: (scores[n], n))
    return tag, bytes(candidates[tag])


def encoding_cases():
    rng = random.Random(0x5246431951)
    for label, plain, block_type in [
        ("empty", b"", 1), ("tiny", b"hello", 1),
        ("all-bytes", bytes(range(256)) * 4, None),
        ("incompressible", rng.randbytes(8192), 0),
        ("dynamic", b"A" * 8192, 2),
        ("multiblock", rng.randbytes(65535) + b"ABACABADABACABA" * 10000, None),
        ("window-wrap", bytes(range(251)) * 800, None),
        ("lazy", b"abcdefgabcdeabcdefgabcdef" * 4000, None),
    ]:
        extra = {} if block_type is None else dict(block_type=block_type)
        yield vector("compress-" + label, plain, plain, "encode-flate", "encode-flate", **extra)
    # Construct two-row examples selecting every filter, with the choice
    # established by independent signed scores, never by the Rust encoder.
    for channels, layout in [(1,"gray"),(3,"rgb"),(4,"rgba")]:
        for tag in range(5):
            for _ in range(256):
                previous = rng.randbytes(17 * channels)
                row = bytearray()
                for i, above in enumerate(previous):
                    left = row[i-channels] if i >= channels else 0
                    corner = previous[i-channels] if i >= channels else 0
                    prediction = [0,left,above,(left+above)//2,paeth(left,above,corner)][tag]
                    delta = (rng.randrange(256) if tag == 0 else
                             rng.randrange(1,256) if tag == 1 and i < channels else
                             rng.randrange(-1,2) if tag == 4 else 0)
                    row.append((prediction + delta) % 256)
                if png_filter_candidates(row, previous, channels)[0] == tag:
                    break
            else:
                raise AssertionError("could not construct filter example")
            plain = previous + row
            yield vector(f"png-encode-{layout}-filter{tag}", plain, plain,
                         "encode-png", "encode-png", width=17, height=2, channels=channels)
        # PNG 6.2 preserves all colours even when alpha is zero or partial.
        plain = bytes([0, 255, 128, 1][:channels])
        yield vector(f"png-encode-{layout}-one-pixel", plain, plain,
                     "encode-png", "encode-png", width=1, height=1, channels=channels)
    plain = rng.randbytes(257 * 257 * 4)
    yield vector("png-encode-rgba-multiple-idat", plain, plain, "encode-png", "encode-png",
                 width=257, height=257, channels=4)
    yield vector("png-encode-straight-alpha", bytes([255,12,90,0, 255,128,64,128]),
                 bytes([255,12,90,0, 255,128,64,128]), "encode-png", "encode-png", width=2, height=1, channels=4)


def decode_png(encoded):
    """Independent PNG 5/9/11.2 decoder for encoder tests; never invokes Pagura."""
    assert encoded[:8] == b"\x89PNG\r\n\x1a\n"
    pos, chunks, compressed = 8, [], bytearray()
    width = height = channels = 0
    while pos < len(encoded):
        size = int.from_bytes(encoded[pos:pos+4], "big")
        assert size <= 0x7fffffff and pos + 12 + size <= len(encoded)
        name = encoded[pos+4:pos+8]
        data = encoded[pos+8:pos+8+size]
        crc = int.from_bytes(encoded[pos+8+size:pos+12+size], "big")
        assert binascii.crc32(name + data) == crc
        chunks.append(name)
        if name == b"IHDR":
            assert len(chunks) == 1 and len(data) == 13
            width, height, depth, color, compression, method, interlace = struct.unpack(">IIBBBBB", data)
            assert 0 < width <= 0x7fffffff and 0 < height <= 0x7fffffff
            assert depth == 8 and color in (0,2,6) and (compression,method,interlace) == (0,0,0)
            channels = {0:1,2:3,6:4}[color]
        elif name == b"IDAT":
            assert len(chunks) >= 2 and all(c == b"IDAT" for c in chunks[1:])
            compressed.extend(data)
        else:
            assert name == b"IEND" and not data and pos + 12 == len(encoded)
        pos += size + 12
    assert chunks[0] == b"IHDR" and chunks[-1] == b"IEND" and b"IDAT" in chunks
    raw = inflate_exact(compressed)
    stride = width * channels
    assert len(raw) == height * (stride + 1)
    pixels, previous = bytearray(), b""
    tags = []
    for start in range(0, len(raw), stride+1):
        tag = raw[start]
        assert 0 <= tag <= 4
        row = bytearray()
        for i, delta in enumerate(raw[start+1:start+1+stride]):
            a = row[i-channels] if i >= channels else 0
            b = previous[i] if previous else 0
            c = previous[i-channels] if previous and i >= channels else 0
            prediction = [0,a,b,(a+b)//2,paeth(a,b,c)][tag]
            row.append((delta + prediction) % 256)
        # Check the optional encoder heuristic too, including deterministic ties.
        chosen, filtered = png_filter_candidates(row, previous, channels)
        assert chosen == tag and filtered == raw[start+1:start+1+stride]
        pixels.extend(row)
        tags.append(tag)
        previous = row
    return width, height, channels, bytes(pixels), tags


def inflate_exact(encoded):
    decoder = zlib.decompressobj()
    plain = decoder.decompress(encoded) + decoder.flush()
    assert decoder.eof and not decoder.unused_data and not decoder.unconsumed_tail
    return plain


def verify_encoded_batch(payload):
    """Validate Rust-produced bytes passed by the Rust tests, not generated here.

    Frames are >BIIIII (kind, encoded size, pixel size, width, height, channels),
    followed by encoded and expected bytes. All expected bytes are independent
    fixture data; this module never imports, runs or calls the product.
    """
    count, pos = 0, 0
    while pos < len(payload):
        kind, size, plain_size, width, height, channels = struct.unpack_from(">BIIIII", payload, pos)
        pos += 21
        encoded, expected = payload[pos:pos+size], payload[pos+size:pos+size+plain_size]
        pos += size + plain_size
        assert len(expected) == plain_size
        if kind == 0:
            assert inflate_exact(encoded) == expected
        else:
            assert kind == 1
            w, h, c, pixels, _ = decode_png(encoded)
            assert (w,h,c,pixels) == (width,height,channels,expected)
        count += 1
    assert pos == len(payload)
    return count


if __name__ == "__main__":
    import sys
    assert sys.argv[1:] == ["verify"]
    print(f"Independent Python oracle verified {verify_encoded_batch(sys.stdin.buffer.read())} encoded streams/images")
