"""Portable pre-code literal-bit/Fraction controls for the optional mask index.

Each expected word, prefix and range visits logical bits directly. Expected
areas visit every bit's Fraction overlap. No product/reference implementation
or grouped/indexed area evaluator contributes expected values. Nonzero source
row padding, word boundaries, orientations and reverse Decode are explicit.
"""
from fractions import Fraction as F
import json
import struct

from .model import Vector


def packed(bits):
    width = len(bits[0])
    stride = (width + 7) // 8
    result = bytearray()
    for row in bits:
        line = bytearray([255] * stride)
        for x, value in enumerate(row):
            if value == 0:
                line[x // 8] &= ~(1 << (7 - x % 8))
        result.extend(line)
    return bytes(result)


def integral(bits, rectangle, reverse):
    """Visit every present logical bit and its exact cell overlap directly."""
    x0, y0, x1, y1 = rectangle
    height, width = len(bits), len(bits[0])
    lowx, lowy = max(F(0), x0), max(F(0), y0)
    highx, highy = min(F(width), x1), min(F(height), y1)
    if highx <= lowx or highy <= lowy:
        return F(0)
    total = F(0)
    for y, row in enumerate(bits):
        dy = max(F(0), min(F(y + 1), highy) - max(F(y), lowy))
        for x, bit in enumerate(row):
            value = 1 - bit if reverse else bit
            dx = max(F(0), min(F(x + 1), highx) - max(F(x), lowx))
            total += value * dx * dy
    return total / ((highx - lowx) * (highy - lowy))


def nearest(value):
    return (2 * value.numerator + value.denominator) // (2 * value.denominator)


def queries(width, height):
    rows = [('full', [F(0), F(0), F(width), F(height)]),
            ('fractional', [F(1, 7), F(1, 11), F(width) - F(1, 5), F(height) - F(1, 3)]),
            ('single-cell', [F(1, 7), F(1, 11), F(4, 7), F(5, 11)]),
            ('clipped-border', [F(-2, 3), F(-1, 7), F(3, 5), F(4, 9)]),
            ('outside', [F(-2), F(-2), F(-1), F(-1)]),
            ('tiny-finite-reciprocal', [F(0), F(0), F(1, 2**1020), F(1, 2**1020)]),
            ('last-logical-cell', [F(width) - F(3, 4), F(height) - F(2, 3), F(width) + F(1, 3), F(height) + F(1, 5)])]
    if width > 64:
        rows.append(('word-boundary', [F(63) + F(1, 4), F(1, 7), min(F(width), F(65) - F(1, 5)), F(height) - F(1, 9)]))
    return rows


def vector(name, bits):
    height, width = len(bits), len(bits[0])
    values, rois = [], bytearray()
    for label, roi in queries(width, height):
        for reverse in (False, True):
            alpha = integral(bits, roi, reverse)
            assert 0 <= alpha <= 1
            a, c = nearest(255 * alpha), nearest(255 * (1 - alpha))
            values.append({'label': label, 'rectangle': list(map(str, roi)),
                           'decode': [1, 0] if reverse else [0, 1], 'alpha': str(alpha),
                           'transparent_red_rgba': [a, 0, 0, a], 'white_red_rgba': [255, c, c, 255]})
            rois.extend(struct.pack('>6d', *map(float, roi), float(alpha), float(reverse)))
    prefixes = [[sum(row[:min(width, k * 64)]) for k in range((width + 63) // 64 + 1)] for row in bits]
    metadata = {'name': name, 'width': width, 'height': height, 'literal_bits': bits,
                'stride': (width + 7) // 8, 'padbits': 'all ones, outside logical samples',
                'direct_per_bit_prefix_population': prefixes, 'queries': values,
                'oracle_method': 'per-logical-bit Fraction clipped-area integral; reverse decoded per bit; no grouped sum or product/reference engine'}
    blocks = (height + 63) // 64
    words = [sum(bits[y][x] << (y % 64) for y in range(b * 64, min((b + 1) * 64, height))) for b in range(blocks) for x in range(width)]
    vertical = [sum(bits[y][x] for y in range(min(b * 64, height))) for b in range(blocks + 1) for x in range(width)]
    marks = sorted({0, height, *[min(height, n) for n in (1, 2, 31, 32, 63, 64, 65, 127, 128, 129)]})
    ranges = b''.join(struct.pack('>4I', x, a, b, sum(bits[y][x] for y in range(a, b))) for a in marks for b in marks if a <= b for x in range(width))
    return Vector('index-' + name, {'.in': packed(bits), '.params': (json.dumps(metadata, indent=2) + '\n').encode(),
                  '.words.u64be': b''.join(struct.pack('>Q', n) for n in words),
                  '.prefix.u32be': b''.join(struct.pack('>I', n) for n in vertical),
                  '.ranges': ranges, '.rois': bytes(rois)}, ('image-packed-tiles',),
                  'Pre-code literal bits, exact vertical populations and per-bit Fraction areas; binary integers/f64 are big endian',
                  extra={'width': width, 'height': height, 'range_queries': len(ranges) // 16, 'fraction_queries': len(values)})


def index_vectors():
    dimensions = [(w, 7) for w in (1, 63, 64, 65, 127, 128, 129)] + [(65, 1), (2, 1), (1, 1)]
    for width, height in dimensions:
        patterns = {'wave': [[int((7 * x + 11 * y) % 13 < 5) for x in range(width)] for y in range(height)],
                    'last-bit': [[int(x == width - 1 and y == height - 1) for x in range(width)] for y in range(height)],
                    'ones': [[1] * width for _ in range(height)]}
        for pattern, bits in patterns.items():
            for orientation in ('rows', 'transposed', 'mirrored'):
                grid = bits if orientation == 'rows' else [list(row) for row in zip(*bits)] if orientation == 'transposed' else [list(reversed(row)) for row in bits]
                yield vector(f'w{width}-h{height}-{pattern}-{orientation}', grid)
