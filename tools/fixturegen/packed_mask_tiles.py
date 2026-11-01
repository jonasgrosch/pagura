"""Bounded packed-mask surfaces: independent literal bits and rational areas.

ISO 32000-2 8.9.3-8.9.4 and 11.6.5.2/Table143; resource admission is
explicit ADR0010/0016 policy rather than an ISO numeric allowance. Pad bits
are deliberately nonzero. Expected pixels use Fraction rectangle overlaps,
never any PDF renderer or product sampler. The original authored bytes were
frozen independently before product implementation.
"""
from fractions import Fraction as F
import json
import zlib

from .packed_mask_index_oracles import index_vectors
from .model import Case, OPEN, RENDER, RefusedPage, RenderPage, Vector

AREA = "packed_mask_tiles"
REQUIREMENT = "image-packed-tiles"

def pdf(objects, header=b'%PDF-1.7\n%\xe2\xe3\xcf\xd3\n'):
    out = bytearray(header)
    offsets = [0]
    for i, obj in enumerate(objects, 1):
        offsets.append(len(out))
        out.extend(f'{i} 0 obj\n'.encode() + obj + b'\nendobj\n')
    xref = len(out)
    out.extend(f'xref\n0 {len(objects) + 1}\n0000000000 65535 f \n'.encode())
    for at in offsets[1:]:
        out.extend(f'{at:010} 00000 n \n'.encode())
    out.extend(f'trailer\n<< /Size {len(objects) + 1} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n'.encode())
    return bytes(out)

def stream(dictionary, data):
    return b'<< ' + dictionary + f' /Length {len(data)} >>\nstream\n'.encode() + data + b'\nendstream'

def packed(w, h, rect):
    x0, y0, x1, y1 = rect
    stride = (w + 7) // 8
    pad = stride * 8 - w
    outside = ((1 << pad) - 1).to_bytes(stride, 'big')
    inside = ((1 << x1 - x0) - 1 << stride * 8 - x1 | (1 << pad) - 1).to_bytes(stride, 'big')
    data = outside * y0 + inside * (y1 - y0) + outside * (h - y1)
    assert len(data) == stride * h
    if pad:
        assert all((data[(row + 1) * stride - 1] & (1 << pad) - 1 for row in range(h)))
    return data

def coverage(a, b, lo, hi):
    return max(F(0), min(b, hi) - max(a, lo)) / (b - a)

def nearest_byte(value):
    assert 0 <= value <= 255
    return (value.numerator * 2 + value.denominator) // (value.denominator * 2)

def oracle(w, h, rect, orientation, inverse, size=8):
    x0, y0, x1, y1 = rect
    x0, x1 = (F(x0, w), F(x1, w))
    y0, y1 = (F(y0, h), F(y1, h))
    if orientation == 'quarter-turn':
        x0, y0, x1, y1 = (y0, 1 - x1, y1, 1 - x0)
    elif orientation == 'mirror':
        x0, x1 = (1 - x1, 1 - x0)
    rgb = bytearray()
    rgba = bytearray()
    white = bytearray()
    alphas = []
    for row in range(size):
        for col in range(size):
            alpha = coverage(F(col, size), F(col + 1, size), x0, x1) * coverage(F(row, size), F(row + 1, size), y0, y1)
            if inverse:
                alpha = 1 - alpha
            a = nearest_byte(255 * alpha)
            c = nearest_byte(255 * (1 - alpha))
            rgb.extend([255, c, c])
            rgba.extend([a, 0, 0, a])
            white.extend([255, c, c, 255])
            alphas.append(str(alpha))
    return (b'P6\n' + f'{size} {size}\n255\n'.encode() + rgb, bytes(rgba), bytes(white), alphas)

def document(w, h, rect, orientation='normal', inverse=False, filtered=True, encoded=None):
    if encoded is None:
        raw = packed(w, h, rect)
        encoded = zlib.compress(raw, 9) if filtered else raw
        mask = (f'/Type /XObject /Subtype /Image /Width {w} /Height {h} /ColorSpace /DeviceGray /BitsPerComponent 1 /Decode [{1 if inverse else 0} {0 if inverse else 1}]'.encode()
                + (b' /Filter /FlateDecode' if filtered else b''))
    else:
        mask = f'/Type /XObject /Subtype /Image /Width {w} /Height {h} /ColorSpace /DeviceGray /BitsPerComponent 1 /Filter /FlateDecode'.encode()
    cm = {'normal': '8 0 0 8 0 0', 'quarter-turn': '0 8 -8 0 8 0', 'mirror': '-8 0 0 8 8 0'}[orientation]
    return pdf([
        b'<< /Type /Catalog /Pages 2 0 R >>',
        b'<< /Type /Pages /Kids [3 0 R] /Count 1 >>',
        b'<< /Type /Page /Parent 2 0 R /MediaBox [0 0 8 8] /Resources << /XObject << /Im 5 0 R >> >> /Contents 4 0 R >>',
        stream(b'', f'q {cm} cm /Im Do Q\n'.encode()),
        stream(b'/Type /XObject /Subtype /Image /Width 2 /Height 2 /ColorSpace /DeviceRGB /BitsPerComponent 8 /SMask 6 0 R', b'\xff\0\0' * 4),
        stream(mask, encoded),
    ])


def cases():
    yield from bit_vectors()
    specs = [
        ('small-rectangle', 10, 7, (2, 1, 8, 6), 'normal', False, False),
        ('small-flate', 10, 7, (2, 1, 8, 6), 'normal', False, True),
        ('small-inverse', 10, 7, (2, 1, 8, 6), 'normal', True, True),
        ('small-quarter-turn', 10, 7, (2, 1, 8, 6), 'quarter-turn', False, True),
        ('small-mirror', 10, 7, (2, 1, 8, 6), 'mirror', False, False),
        ('default-large', 34862, 4332, (11620, 1083, 23241, 3249), 'normal', False, True),
        ('default-large-quarter-turn', 34862, 4332, (11620, 1083, 23241, 3249), 'quarter-turn', False, True),
    ]
    for name, width, height, rect, orientation, inverse, filtered in specs:
        ppm, transparent, white, alphas = oracle(width, height, rect, orientation, inverse)
        yield Case(name, document(width, height, rect, orientation, inverse, filtered),
                   RENDER, (REQUIREMENT,), 'Independent bit rectangle and rational mask-footprint area; row padding is nonzero',
                   oracle=ppm, extra={'tolerance': 1})
        yield Vector(name + '-rgba', {'.transparent.rgba': transparent, '.white.rgba': white,
                     '.opacity.json': (json.dumps(alphas, indent=2) + '\n').encode()},
                     (REQUIREMENT,), 'Independent rational premultiplied transparent and white RGBA channels',
                     extra={'width': 8, 'height': 8, 'tolerance': 1, 'pdf_case': AREA + '/' + name})
    width, height, rect = 34862, 4332, (11620, 1083, 23241, 3249)
    raw = packed(width, height, rect)
    valid = zlib.compress(raw, 9)
    for name, encoded in [
        ('flate-short-checksum', valid[:-1]),
        ('flate-checksum-mismatch', valid[:-1] + bytes([valid[-1] ^ 1])),
        ('flate-trailing-byte', valid + b'\0'),
        ('flate-short-rows', zlib.compress(raw[:-1], 9)),
        ('flate-excess-rows', zlib.compress(raw + b'\0', 9)),
    ]:
        yield Case(name, document(width, height, rect, encoded=encoded), OPEN, (REQUIREMENT,),
                   'Incomplete, corrupt or excess mask data cannot certify a tiled source',
                   strict_valid=name not in ('flate-short-checksum', 'flate-checksum-mismatch', 'flate-trailing-byte'),
                   extra={'render_error': 'syntax'})

    yield from shared_sources()
    yield from bilinear_cases()
    yield from generation_and_retile()
    yield from scene_sources()
    yield from late_owner_failures()
    yield from indexed_thresholds()
    yield from index_vectors()


def bit_vectors():
    patterns = [
        ('rectangle-pad6', [[int(2 <= x < 8 and 1 <= y < 6) for x in range(10)] for y in range(7)]),
        ('last-logical-bit-pad7', [[int(x == 8 and y == 8) for x in range(9)] for y in range(9)]),
        ('nonrectangular-pad3', [[int((x + 3 * y) % 5 < 2) for x in range(13)] for y in range(11)]),
    ]
    for name, bits in patterns:
        height, width = len(bits), len(bits[0])
        stride, out_stride = (width + 7) // 8, (height + 7) // 8
        raw = b''.join(int(''.join(map(str, row)) + '1' * (stride * 8 - width), 2).to_bytes(stride, 'big') for row in bits)
        transposed = b''.join(int(''.join(str(bits[y][x]) for y in range(height)) + '0' * (out_stride * 8 - height), 2).to_bytes(out_stride, 'big') for x in range(width))
        params = {'width': width, 'height': height, 'bits_per_component': 1,
                  'input_stride': stride, 'output_stride': out_stride, 'logical_bits': bits,
                  'source_padbits': 'all nonzero', 'new_output_padbits': 'zero',
                  'custom_pixel_cap': max(width, height), 'expected_complete_row_tiles': height,
                  'expected_transposed_complete_row_tiles': width}
        yield Vector('bits-' + name, {'.in': raw, '.transposed': transposed,
                     '.params': (json.dumps(params, indent=2) + '\n').encode()}, (REQUIREMENT,),
                     'Literal logical bits independently construct original and transposed rows; padding never becomes samples')


def shared_sources():
    """One immutable XObject cannot change the whole-image admission policy."""
    width, height, rect = 34862, 4332, (11620, 1083, 23241, 3249)
    encoded = zlib.compress(packed(width, height, rect), 9)
    mask = stream(f'/Type /XObject /Subtype /Image /Width {width} /Height {height} /ColorSpace /DeviceGray /BitsPerComponent 1 /Filter /FlateDecode'.encode(), encoded)
    soft = stream(b'/Type /XObject /Subtype /Image /Width 2 /Height 2 /ColorSpace /DeviceRGB /BitsPerComponent 8 /SMask 7 0 R', b'\xff\0\0' * 4)
    stencil = stream(b'/Type /XObject /Subtype /Image /Width 2 /Height 2 /ColorSpace /DeviceRGB /BitsPerComponent 8 /Mask 7 0 R', b'\xff\0\0' * 4)
    resources = b'/Resources << /XObject << /Soft 5 0 R /Stencil 6 0 R /Ordinary 7 0 R >> >>'
    def mark(name):
        return b'q 8 0 0 8 0 0 cm /' + name + b' Do Q\n'
    for name, marks, kind in [
        ('soft-then-main', [b'Soft', b'Ordinary'], 'limit'),
        ('main-then-soft', [b'Ordinary', b'Soft'], 'limit'),
        ('soft-then-invalid-stencil', [b'Soft', b'Stencil'], 'syntax'),
        ('invalid-stencil-then-soft', [b'Stencil', b'Soft'], 'syntax'),
    ]:
        objects = [b'<< /Type /Catalog /Pages 2 0 R >>', b'<< /Type /Pages /Kids [3 0 R] /Count 1 >>',
            b'<< /Type /Page /Parent 2 0 R /MediaBox [0 0 8 8] ' + resources + b' /Contents 4 0 R >>',
            stream(b'', b''.join(mark(role) for role in marks)), soft, stencil, mask]
        yield Case(name, pdf(objects), OPEN, (REQUIREMENT,), 'Shared mask used as ordinary image or invalid stencil retains refusal in either source order',
            extra={'render_error': kind, 'shared_object': 7, 'purpose_order': [role.decode() for role in marks]})
    for name, marks in [('soft-page-then-main', [b'Soft', b'Ordinary']), ('main-page-then-soft', [b'Ordinary', b'Soft'])]:
        objects = [b'<< /Type /Catalog /Pages 2 0 R >>', b'<< /Type /Pages /Kids [3 0 R 8 0 R] /Count 2 >>',
            b'<< /Type /Page /Parent 2 0 R /MediaBox [0 0 8 8] ' + resources + b' /Contents 4 0 R >>',
            stream(b'', mark(marks[0])), soft, stencil, mask,
            b'<< /Type /Page /Parent 2 0 R /MediaBox [0 0 8 8] ' + resources + b' /Contents 9 0 R >>', stream(b'', mark(marks[1]))]
        extra = {'shared_object': 7, 'purpose_order': [role.decode() for role in marks], 'shared_store_orders': [[0, 1, 0], [1, 0, 1]], 'tolerance': 1}
        ppm, _, _, _ = oracle(width, height, rect, 'normal', False)
        outcomes = tuple(RenderPage(page, ppm, 1) if role == b'Soft' else
                         RefusedPage(page, 'limit', 'image sample budget exceeded')
                         for page, role in enumerate(marks))
        yield Case(name, pdf(objects), OPEN, (REQUIREMENT,), 'Shared mask across two pages retains purpose and original source limits under both repeated page orders',
            pages=2, page_outcomes=outcomes, extra=extra)


def bilinear_cases():
    """Rational samples cross tile rows on a virtual canvas; caps apply per output."""
    for prefix, rect in [('bilinear', (2, 1, 8, 6)), ('asymmetric-bilinear', (1, 1, 6, 4))]:
        width, height, size = 10, 7, 20
        raw = packed(width, height, rect)
        for orientation, inverse in [('normal', False), ('mirror', False), ('quarter-turn', False), ('normal', True)]:
            name = prefix + '-' + orientation + ('-inverse' if inverse else '')
            matrix = {'normal': '8 0 0 8 0 0', 'mirror': '-8 0 0 8 8 0', 'quarter-turn': '0 8 -8 0 8 0'}[orientation]
            mask = (f'/Type /XObject /Subtype /Image /Width {width} /Height {height} '
                f'/ColorSpace /DeviceGray /BitsPerComponent 1 /Interpolate true '
                f'/Decode [{1 if inverse else 0} {0 if inverse else 1}] /Filter /FlateDecode').encode()
            data = pdf([b'<< /Type /Catalog /Pages 2 0 R >>', b'<< /Type /Pages /Kids [3 0 R] /Count 1 >>',
                b'<< /Type /Page /Parent 2 0 R /MediaBox [0 0 8 8] /Resources << /XObject << /Image 5 0 R >> >> /Contents 4 0 R >>',
                stream(b'', f'q {matrix} cm /Image Do Q\n'.encode()),
                stream(b'/Type /XObject /Subtype /Image /Width 2 /Height 2 /ColorSpace /DeviceRGB /BitsPerComponent 8 /SMask 6 0 R', b'\xff\0\0' * 4),
                stream(mask, zlib.compress(raw, 9))])
            rgb, transparent, white, exact = bytearray(), bytearray(), bytearray(), []
            for row in range(size):
                for column in range(size):
                    u, v = F(2 * column + 1, 2 * size), F(2 * row + 1, 2 * size)
                    if orientation == 'mirror':
                        u = 1 - u
                    elif orientation == 'quarter-turn':
                        u, v = 1 - v, u
                    x, y = u * width - F(1, 2), v * height - F(1, 2)
                    ix, iy = x.numerator // x.denominator, y.numerator // y.denominator
                    dx, dy = x - ix, y - iy
                    alpha = F(0)
                    for sy, wy in [(iy, 1 - dy), (iy + 1, dy)]:
                        for sx, wx in [(ix, 1 - dx), (ix + 1, dx)]:
                            code = int(rect[0] <= min(max(sx, 0), width - 1) < rect[2] and rect[1] <= min(max(sy, 0), height - 1) < rect[3])
                            alpha += (1 - code if inverse else code) * wx * wy
                    a, c = nearest_byte(255 * alpha), nearest_byte(255 * (1 - alpha))
                    rgb.extend([255, c, c])
                    transparent.extend([a, 0, 0, a])
                    white.extend([255, c, c, 255])
                    exact.append(str(alpha))
            yield Case(name, data, RENDER, (REQUIREMENT,), 'Independent rational bilinear source-bit reconstruction on the complete logical grid',
                dpi=180, oracle=b'P6\n20 20\n255\n' + rgb,
                extra={'tolerance': 1, 'custom_document_pixels': 10, 'region_width': 10, 'region_height': 1,
                       'full_400_pixel_output_error': 'limit', 'oracle_freeze': 'Before candidate sampler pixel observations; asymmetric strengthening followed prototype source.'})
            yield Vector(name + '-rgba', {'.transparent.rgba': bytes(transparent), '.white.rgba': bytes(white),
                '.opacity.json': (json.dumps(exact, indent=2) + '\n').encode()}, (REQUIREMENT,),
                'Independent rational bilinear premultiplied transparent and white RGBA',
                extra={'width': 20, 'height': 20, 'tolerance': 1, 'pdf_case': AREA + '/' + name})


def generation_and_retile():
    """Generation is identity only; lower valid caps select smaller complete tiles."""
    objects = [b'<< /Type /Catalog /Pages 2 0 R >>', b'<< /Type /Pages /Kids [3 0 R] /Count 1 >>',
        b'<< /Type /Page /Parent 2 0 R /MediaBox [0 0 8 8] /Resources << /XObject << /Image 5 0 R >> >> /Contents 4 0 R >>',
        stream(b'', b'q 8 0 0 8 0 0 cm /Image Do Q\n'),
        stream(b'/Type /XObject /Subtype /Image /Width 2 /Height 2 /ColorSpace /DeviceRGB /BitsPerComponent 8 /SMask 6 1 R', b'\xff\0\0' * 4),
        stream(b'/Type /XObject /Subtype /Image /Width 10 /Height 7 /ColorSpace /DeviceGray /BitsPerComponent 1 /Filter /FlateDecode', zlib.compress(packed(10, 7, (2, 1, 8, 6)), 9))]
    output = bytearray(b'%PDF-1.7\n%\xe2\xe3\xcf\xd3\n')
    offsets = [0]
    for number, data in enumerate(objects, 1):
        offsets.append(len(output))
        output.extend(f'{number} {int(number == 6)} obj\n'.encode() + data + b'\nendobj\n')
    start = len(output)
    output.extend(f'xref\n0 {len(objects) + 1}\n0000000000 65535 f \n'.encode())
    for number, offset in enumerate(offsets[1:], 1):
        output.extend(f'{offset:010} {int(number == 6):05} n \n'.encode())
    output.extend(f'trailer\n<< /Size {len(objects) + 1} /Root 1 0 R >>\nstartxref\n{start}\n%%EOF\n'.encode())
    ppm, transparent, white, alphas = oracle(10, 7, (2, 1, 8, 6), 'normal', False)
    yield Case('small-generation-1', bytes(output), RENDER, (REQUIREMENT,), 'Same present sample bits and rational pixels with soft-mask source generation1',
        oracle=ppm, extra={'tolerance': 1})
    yield Vector('small-generation-1-rgba', {'.transparent.rgba': transparent, '.white.rgba': white,
        '.opacity.json': (json.dumps(alphas, indent=2) + '\n').encode()}, (REQUIREMENT,),
        'Generation-independent rational premultiplied opacity pixels',
        extra={'width': 8, 'height': 8, 'tolerance': 1, 'pdf_case': AREA + '/small-generation-1'})
    params = {'pdf_case': AREA + '/small-flate', 'original_pixels': 30,
        'tiled_samples': 70, 'tiled_work': 1000, 'decoded_bytes': 65536, 'decode_bytes': 65536,
        'requested_pixel_orders': [[30, 20, 10, 9, 20, 30], [10, 20, 30, 9, 10]],
        'tile_heights': {'30': 3, '20': 2, '10': 1, '9': 'limit'}, 'dpi': 9,
        'store_capacities': [0, 67108864], 'opacity_exact': '3/7',
        'scope': 'Already recorded source, original/requested limit intersection; failed tiled admission leaves Store unchanged before parent decode.'}
    yield Vector('retile-one-pixel', {'.ppm': b'P6\n1 1\n255\n' + bytes([255, 146, 146]),
        '.transparent.rgba': bytes([109, 0, 0, 109]), '.white.rgba': bytes([255, 146, 146, 255]),
        '.params': (json.dumps(params, indent=2) + '\n').encode()}, (REQUIREMENT,),
        'Independent70-bit rational area oracle for lower-cap retile/cache-order API requests')


def scene_sources():
    """Distinct held roots pay separately; forty aliases keep every mark charge."""
    width, height = 34862, 4332
    mask = stream(f'/Type /XObject /Subtype /Image /Width {width} /Height {height} /ColorSpace /DeviceGray /BitsPerComponent 1 /Filter /FlateDecode'.encode(),
                  zlib.compress(packed(width, height, (11620, 1083, 23241, 3249)), 9))
    ppm = b'P6\n8 8\n255\n' + b''.join(bytes([0 if 2 <= y < 6 else 255]) * 24 for y in range(8))
    rgba = b''.join(bytes([0, 0, 0, 255]) if 2 <= y < 6 else bytes(4) for y in range(8) for _ in range(8))
    white = b''.join(bytes([0, 0, 0, 255]) if 2 <= y < 6 else bytes([255] * 4) for y in range(8) for _ in range(8))
    specs = [
        ('scene-forty-shared', 1, 1, None, 1),
        ('scene-forty-shared-mask-different-parents', 40, 1, None, 1),
        ('scene-forty-distinct-mask-roots', 40, 40, None, 1),
        ('scene-forty-shared-then-main', 1, 1, 'after', 1),
        ('scene-main-then-forty-shared', 1, 1, 'before', 1),
        ('scene-forty-shared-two-pages', 1, 1, None, 2),
    ]
    for name, count, mask_count, ordinary, pages in specs:
        first_mask, images, resources = 5 + count, [], []
        for i in range(count):
            mid = first_mask + i if mask_count > 1 else first_mask
            images.append(stream(f'/Type /XObject /Subtype /Image /Width 2 /Height 2 /ColorSpace /DeviceRGB /BitsPerComponent 8 /SMask {mid} 0 R'.encode(), bytes(12)))
            resources.append(f'/X{i} {5 + i} 0 R'.encode())
        resources.append(f'/Ordinary {first_mask} 0 R'.encode())
        marks = [b'q 3200 0 0 8 -1600 0 cm /X' + str(i if count > 1 else 0).encode() + b' Do Q\n' for i in range(40)]
        ordinary_mark = b'q 8 0 0 8 0 0 cm /Ordinary Do Q\n'
        if ordinary == 'before':
            marks.insert(0, ordinary_mark)
        if ordinary == 'after':
            marks.append(ordinary_mark)
        content = stream(b'', b''.join(marks))
        page = b'<< /Type /Page /Parent 2 0 R /MediaBox [0 0 8 8] /Resources << /XObject << ' + b' '.join(resources) + b' >> >> /Contents 4 0 R >>'
        objects = [b'<< /Type /Catalog /Pages 2 0 R >>', b'', page, content, *images, *([mask] * mask_count)]
        kids = b'3 0 R'
        if pages == 2:
            next_page = len(objects) + 1
            objects.extend([page.replace(b'/Contents 4 0 R', f'/Contents {next_page + 1} 0 R'.encode()), content])
            kids += f' {next_page} 0 R'.encode()
        objects[1] = b'<< /Type /Pages /Kids [' + kids + b'] /Count ' + str(pages).encode() + b' >>'
        expected = RENDER if mask_count == 1 and ordinary is None else OPEN
        extra = {'tolerance': 0, 'image_occurrences': len(marks), 'distinct_source_roots': mask_count}
        if expected == OPEN:
            extra['render_error'] = 'limit'
        yield Case(name, pdf(objects), expected, (REQUIREMENT,),
                   'Held tiled source roots pay once per scene; distinct roots and ordinary-purpose refusals retain all budgets',
                   pages=pages, oracle=ppm if expected == RENDER else None, extra=extra)
        if expected == RENDER:
            yield Vector(name + '-rgba', {'.transparent.rgba': rgba, '.white.rgba': white},
                         (REQUIREMENT,), 'Independent four opaque black middle rows; wide horizontal image transform excludes mask edges',
                         extra={'width': 8, 'height': 8, 'tolerance': 0, 'pdf_case': AREA + '/' + name})


def late_owner_failures():
    """Same object ID with different bits, complete frame-less JPEG and exact areas."""
    mask = stream(b'/Type /XObject /Subtype /Image /Width 10 /Height 7 /ColorSpace /DeviceGray /BitsPerComponent 1 /Filter /FlateDecode',
                  zlib.compress(packed(10, 7, (2, 2, 5, 4)), 9))
    base = [b'<< /Type /Catalog /Pages 2 0 R >>', b'<< /Type /Pages /Kids [3 0 R] /Count 1 >>',
            b'<< /Type /Page /Parent 2 0 R /MediaBox [0 0 8 8] /Resources << /XObject << /Image 5 0 R >> >> /Contents 4 0 R >>',
            stream(b'', b'q 8 0 0 8 0 0 cm /Image Do Q\n')]
    for name, parent, expected in [
        ('same-id-six-present-samples', stream(b'/Type /XObject /Subtype /Image /Width 2 /Height 2 /ColorSpace /DeviceRGB /BitsPerComponent 8 /SMask 6 0 R', bytes([255, 0, 0]) * 4), RENDER),
        ('late-parent-no-jpeg-frame', stream(b'/Type /XObject /Subtype /Image /Width 2 /Height 2 /ColorSpace /DeviceRGB /BitsPerComponent 8 /Filter /DCTDecode /SMask 6 0 R', bytes.fromhex('ffd8ffd9')), OPEN),
    ]:
        extra: dict[str, object] = {'tolerance': 0}
        if expected == OPEN:
            extra['render_error'] = 'syntax'
        yield Case(name, pdf([*base, parent, mask]), expected, (REQUIREMENT,),
                   'Different present mask bits with identical object IDs, or complete parent JPEG without a required frame',
                   dpi=9, oracle=b'P6\n1 1\n255\n' + bytes([255, 233, 233]) if expected == RENDER else None, extra=extra)
    for name, alpha in [('same-id-six-present-samples', F(6, 70)), ('forty-six-sample-marks', 1 - F(64, 70) ** 40)]:
        a, c = nearest_byte(255 * alpha), nearest_byte(255 * (1 - alpha))
        yield Vector(name + '-rgba', {'.ppm': b'P6\n1 1\n255\n' + bytes([255, c, c]),
                     '.transparent.rgba': bytes([a, 0, 0, a]), '.white.rgba': bytes([255, c, c, 255])},
                     (REQUIREMENT,), 'Independent Fraction source-grid coverage and repeated source-over alpha; no engine-derived samples',
                     extra={'width': 1, 'height': 1, 'tolerance': 0})


def threshold_oracle(page, source, rect, matrix, repeats, clip):
    """Literal bit rectangle projected using Fraction, then exact source-over."""
    width, height = source
    x0, y0, x1, y1 = rect
    a, b, c, d, e, f = map(F, matrix)
    points = [(a * x + c * y + e, b * x + d * y + f)
              for x in (F(x0, width), F(x1, width))
              for y in (1 - F(y0, height), 1 - F(y1, height))]
    left, right = min(x for x, y in points), max(x for x, y in points)
    bottom, top = min(y for x, y in points), max(y for x, y in points)
    rgba, white, rgb = bytearray(), bytearray(), bytearray()
    for row in range(page[1]):
        py0, py1 = F(page[1] - row - 1), F(page[1] - row)
        for col in range(page[0]):
            dx = max(F(0), min(F(col + 1), right, F(clip[2])) - max(F(col), left, F(clip[0])))
            dy = max(F(0), min(py1, top, F(clip[3])) - max(py0, bottom, F(clip[1])))
            alpha = 1 - (1 - dx * dy) ** repeats
            assert 0 <= alpha <= 1
            aa, cc = nearest_byte(255 * alpha), nearest_byte(255 * (1 - alpha))
            rgba.extend((aa, 0, 0, aa))
            white.extend((255, cc, cc, 255))
            rgb.extend((255, cc, cc))
    return b'P6\n' + f'{page[0]} {page[1]}\n255\n'.encode() + rgb, bytes(rgba), bytes(white)


def indexed_thresholds():
    """Pre-code P10 source/oracles, with small-footprint old-route controls."""
    specs = [
        ('minified-forty-rows', (144, 8), (34862, 4332), (11620, 1083, 23241, 3249), (144, 0, 0, 18, 0, 0), 40, (0, 0, 144, 8)),
        ('minified-forty-mirror', (144, 8), (34862, 4332), (11620, 1083, 23241, 3249), (-144, 0, 0, 18, 144, 0), 40, (0, 0, 144, 8)),
        ('minified-forty-quarter', (144, 8), (34862, 4332), (11620, 1083, 23241, 3249), (0, 18, -144, 0, 144, 0), 40, (0, 0, 144, 8)),
        ('minified-forty-256canvas', (256, 256), (34862, 4332), (11620, 1083, 23241, 3249), (144, 0, 0, 18, 0, 0), 40, (0, 0, 144, 8)),
        ('highwidth-short-crop', (8, 8), (8388608, 9), (2796202, 2, 5592405, 7), (131072, 0, 0, 8, -65536, 0), 1, (0, 0, 8, 8)),
    ]
    for name, page, size, rect, matrix, count, clip in specs:
        width, height = size
        encoded = zlib.compress(packed(width, height, rect), 9)
        cm = ' '.join(map(str, matrix)).encode()
        content = f'q {clip[0]} {clip[1]} {clip[2] - clip[0]} {clip[3] - clip[1]} re W n\n'.encode()
        content += (b'q ' + cm + b' cm /Im Do Q\n') * count + b'Q\n'
        objects = [
            b'<< /Type /Catalog /Pages 2 0 R >>',
            b'<< /Type /Pages /Kids [3 0 R] /Count 1 >>',
            f'<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {page[0]} {page[1]}] /Resources << /XObject << /Im 5 0 R >> >> /Contents 4 0 R >>'.encode(),
            stream(b'', content),
            stream(b'/Type /XObject /Subtype /Image /Width 2 /Height 2 /ColorSpace /DeviceRGB /BitsPerComponent 8 /SMask 6 0 R', b'\xff\0\0' * 4),
            stream(f'/Type /XObject /Subtype /Image /Width {width} /Height {height} /ColorSpace /DeviceGray /BitsPerComponent 1 /Filter /FlateDecode'.encode(), encoded),
        ]
        ppm, transparent, white = threshold_oracle(page, size, rect, matrix, count, clip)
        yield Case(name, pdf(objects, header=b'%PDF-1.7\n'), RENDER, (REQUIREMENT,),
                   'Pre-code literal source rectangle, Fraction projection and forty exact source-over operations; no sampled oracle',
                   oracle=ppm, extra={'tolerance': 1})
        yield Vector(name + '-rgba', {'.transparent.rgba': transparent, '.white.rgba': white},
                     (REQUIREMENT,), 'Pre-code independent full Fraction premultiplied RGBA bytes',
                     extra={'width': page[0], 'height': page[1], 'tolerance': 1, 'pdf_case': AREA + '/' + name})
