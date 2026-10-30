"""Independent opaque tiling controls, ISO32000-2 8.7.3.1/Table74 and11.6.7.

Exact rational polygon/pixel intersections and compositing were authored outside
Pagura before engine execution. These are59 project-authored controls; upstream
PDFs remain external. RGB and input bytes match the frozen independent control
set; the external set also retains independently derived premultiplied RGBA.
No product renderer/reference, external package or machine-specific path is used.
"""
from fractions import Fraction as F
import math
from .model import Case, OPEN, RENDER, UNSUPPORTED

AREA = 'opaque_tiling'
REQUIREMENT = 'content-tiling'


def number(value):
    value = F(value)
    denominator = value.denominator
    places = 0
    while denominator != 1:
        common = math.gcd(denominator, 10)
        assert common > 1, 'PDF numbers must have finite decimal expansions'
        denominator //= common
        places += 1
    scaled = value * 10 ** places
    assert scaled.denominator == 1
    sign = '-' if scaled < 0 else ''
    digits = str(abs(scaled.numerator)).rjust(places + 1, '0')
    return sign + (digits if not places else digits[:-places] + '.' + digits[-places:])


def stream(content, dictionary=''):
    data = content.encode('ascii')
    return f'<< {dictionary} /Length {len(data)} >>\nstream\n'.encode() + data + b'\nendstream'


def pdf(objects):
    out = bytearray(b'%PDF-1.7\n')
    offsets = [0]
    for index, body in enumerate(objects, 1):
        offsets.append(len(out))
        out.extend(f'{index} 0 obj\n'.encode() + body + b'\nendobj\n')
    start = len(out)
    out.extend(f'xref\n0 {len(offsets)}\n0000000000 65535 f \n'.encode())
    for offset in offsets[1:]:
        out.extend(f'{offset:010} 00000 n \n'.encode())
    out.extend(f'trailer\n<< /Size {len(offsets)} /Root 1 0 R >>\nstartxref\n{start}\n%%EOF\n'.encode())
    return bytes(out)


def polygon_path(points):
    return ' '.join(f'{number(x)} {number(y)} {"m" if i == 0 else "l"}'
                    for i, (x, y) in enumerate(points)) + ' h'


def rectangle_path(rect):
    x0, y0, x1, y1 = map(F, rect)
    return f'{number(x0)} {number(y0)} {number(x1-x0)} {number(y1-y0)} re'


def make_pdf(*, size=16, bbox=(0, 0, 10, 10), step=('0.001', '0.001'),
             matrix=(1, 0, 0, 1, 0, 0), tiling_type=1,
             body='q 0 g 0 0 10 10 re f Q', mark=None, clip=None,
             alpha=1, mask=None, backdrop=None, paint_type=1, repeat=1,
             stroke=False, stencil=False, direct=False, inner_state=None):
    mark = mark or [(0, 0), (size, 0), (size, size), (0, size)]
    content = ''
    if backdrop is not None:
        content += 'q ' + ' '.join(number(F(x, 255)) for x in backdrop) + f' rg 0 0 {size} {size} re f Q\n'
    content += 'q\n'
    if clip is not None:
        content += rectangle_path(clip) + ' W n\n'
    content += '/Outer gs\n'
    if direct:
        content += '0 G\n' if stroke else '0 g\n'
    elif stroke:
        content += '/Pattern CS /P SCN\n'
    else:
        content += '[/Pattern /DeviceGray] cs 0 /P scn\n' if paint_type == 2 else '/Pattern cs /P scn\n'
    if stencil:
        content += 'q 15.5 0 0 15.5 0.25 0.25 cm /Stencil Do Q\n'
    else:
        content += (polygon_path(mark) + (' 2 w S\n' if stroke else ' f\n')) * repeat
    content += 'Q\n'
    mask_dictionary = '/SMask << /S /Alpha /G 8 0 R >>' if mask else ''
    mask_content = '/MaskAlpha gs 0 g ' + rectangle_path(mask['rect']) + ' f' if mask else ''
    pattern = (f'/PatternType 1 /PaintType {paint_type} /TilingType {tiling_type} '
        f'/BBox [{" ".join(map(number, bbox))}] /XStep {number(F(step[0]))} /YStep {number(F(step[1]))} '
        f'/Matrix [{" ".join(map(number, matrix))}] '
        '/Resources << /ExtGState << /Inner 10 0 R >> >>')
    objects = [
        b'<< /Type /Catalog /Pages 2 0 R >>',
        b'<< /Type /Pages /Kids [3 0 R] /Count 1 >>',
        f'<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {size} {size}] /Resources << /Pattern << /P 4 0 R >> /ExtGState << /Outer 6 0 R >> /XObject << /Stencil 11 0 R >> >> /Contents 5 0 R >>'.encode(),
        stream(body, pattern), stream(content),
        f'<< /ca {number(F(alpha))} {mask_dictionary} >>'.encode(),
        b'null',
        stream(mask_content, f'/Type /XObject /Subtype /Form /BBox [0 0 {size} {size}] /Group << /S /Transparency /CS /DeviceRGB /I true >> /Resources << /ExtGState << /MaskAlpha 9 0 R >> >>'),
        f'<< /ca {number(F(mask["alpha"])) if mask else "1"} >>'.encode(),
        inner_state.encode('ascii') if inner_state is not None else b'<< /ca 0.4 /BM /Normal >>',
        stream('00>', '/Type /XObject /Subtype /Image /Width 1 /Height 1 /BitsPerComponent 1 /ImageMask true /Decode [0 1] /Filter /ASCIIHexDecode'),
    ]
    return pdf(objects)


def clip_axis(points, axis, boundary, keep_greater):
    result = []
    if not points:
        return result
    previous = points[-1]
    for current in points:
        old_inside = previous[axis] >= boundary if keep_greater else previous[axis] <= boundary
        inside = current[axis] >= boundary if keep_greater else current[axis] <= boundary
        if old_inside != inside:
            t = (boundary - previous[axis]) / (current[axis] - previous[axis])
            result.append(tuple(a + t * (b - a) for a, b in zip(previous, current)))
        if inside:
            result.append(current)
        previous = current
    return result


def clipped(points, rect):
    for axis, boundary, greater in [(0, rect[0], True), (0, rect[2], False),
                                    (1, rect[1], True), (1, rect[3], False)]:
        points = clip_axis(points, axis, F(boundary), greater)
    return points


def area(points):
    if not points:
        return F(0)
    return abs(sum(a[0] * b[1] - a[1] * b[0]
                   for a, b in zip(points, points[1:] + points[:1]))) / 2


def quantize(value):
    assert 0 <= value <= 1
    value *= 255
    return (2 * value.numerator + value.denominator) // (2 * value.denominator)


def expectations(case, dpi):
    size = case.get('size', 16)
    scale = F(dpi, 72)
    dimensions = int(size * scale)
    assert dimensions == size * scale
    if not any(case.get(k) for k in ['mark', 'clip', 'mask', 'backdrop', 'stroke']) and F(case.get('alpha', 1)) == 1:
        rgba = bytes([0, 0, 0, 255]) * (dimensions * dimensions)
        return dimensions, b'P6\n%d %d\n255\n' % (dimensions, dimensions) + bytes(dimensions * dimensions * 3), rgba, rgba
    mark = case.get('mark') or [(0, 0), (size, 0), (size, size), (0, size)]
    points = [(F(x) * scale, F(y) * scale) for x, y in mark]
    hole = []
    if case.get('stroke'):
        assert mark == [(4, 4), (12, 4), (12, 12), (4, 12)] and not case.get('clip')
        points = [(F(x) * scale, F(y) * scale) for x, y in [(3, 3), (13, 3), (13, 13), (3, 13)]]
        hole = [(F(x) * scale, F(y) * scale) for x, y in [(5, 5), (11, 5), (11, 11), (5, 11)]]
    if case.get('clip'):
        points = clipped(points, [F(x) * scale for x in case['clip']])
    rgb, white, transparent = bytearray(), bytearray(), bytearray()
    for row in range(dimensions):
        for x in range(dimensions):
            y = dimensions - row - 1
            coverage = area(clipped(points, (x, y, x + 1, y + 1)))
            if hole:
                coverage -= area(clipped(hole, (x, y, x + 1, y + 1)))
            assert 0 <= coverage <= 1
            opacity = F(case.get('alpha', 1))
            if case.get('mask'):
                # These mask boundaries are deliberately pixel-aligned; no
                # correlated mark/mask boundary or sampled-mask assumption.
                mask = case['mask']
                box = [F(v) * scale for v in mask['rect']]
                assert all(v.denominator == 1 for v in box)
                opacity *= F(mask['alpha']) if box[0] <= x < box[2] and box[1] <= y < box[3] else 0
            alpha = coverage * opacity
            # All repeated-positive marks are opaque, hence idempotent.
            assert case.get('repeat', 1) == 1 or opacity == 1
            if case.get('backdrop') is None:
                premul = [F(0)] * 3
                result_alpha = alpha
            else:
                premul = [F(v, 255) * (1 - alpha) for v in case['backdrop']]
                result_alpha = F(1)
            on_white = [quantize(v + 1 - result_alpha) for v in premul]
            rgb.extend(on_white)
            white.extend([*on_white, 255])
            transparent.extend([*(quantize(v) for v in premul), quantize(result_alpha)])
    return dimensions, b'P6\n%d %d\n255\n' % (dimensions, dimensions) + rgb, white, transparent


def definitions():
    yield 'full-cell-default-black', {'body': 'q 0 0 10 10 re f Q'}, None
    yield 'full-cell-evenodd', {'body': 'q 0 g 0 0 10 10 re f* Q'}, None
    yield 'offset-ink', {'bbox': (0, 0, 1200, 1200), 'body': 'q 1100 1100 100 100 re f Q'}, None
    for kind in [1, 2, 3]:
        yield f'type-{kind}', {'tiling_type': kind}, None
    for label, matrix in [
        ('shear', (1, '0.25', '0.5', 1, '0.25', '0.5')),
        ('reflection', (-1, 0, 0, 1, 3, 2)),
        ('quarter-turn', (0, 1, -1, 0, 2, 3)),
        ('fractional-origin', (1, 0, 0, 1, '0.125', '0.375')),
        ('large-origin', (1, 0, 0, 1, 1000000, -1000000)),
    ]:
        yield label, {'matrix': matrix}, None
    for x, y in [('-0.001', '0.001'), ('0.001', '-0.001'), ('-0.001', '-0.001')]:
        yield f'negative-step-{x.startswith("-")}-{y.startswith("-")}', {'step': (x, y)}, None
    yield 'local-rectangular-clip', {'body': 'q 3 2 4 6 re W n 0 g 0 0 10 10 re f Q'}, None
    yield 'local-rectangular-evenodd-clip', {'body': 'q 3 2 4 6 re W* n 0 g 0 0 10 10 re f Q'}, None
    yield 'local-reflection', {'body': 'q -1 0 0 1 10 0 cm 0 g 0 0 10 10 re f Q'}, None
    yield 'local-quarter-turn', {'body': 'q 0 1 -1 0 10 0 cm 0 g 0 0 10 10 re f Q'}, None
    yield 'outer-fractional-mark-and-clip', {
        'mark': [('0.25', '0.25'), ('15.75', '0.25'), ('15.75', '15.75'), ('0.25', '15.75')],
        'clip': ('2.5', '1.75', '14.25', '14.5')}, None
    yield 'outer-triangle', {'mark': [(1, 1), (15, 3), (3, 15)]}, None
    yield 'outer-stroke', {'mark': [(4, 4), (12, 4), (12, 12), (4, 12)], 'stroke': True}, None
    yield 'outer-alpha', {'alpha': '0.4'}, None
    yield 'outer-alpha-backdrop', {'alpha': '0.4', 'backdrop': (0, 255, 0)}, None
    yield 'outer-softmask', {'alpha': '0.5', 'mask': {'rect': (0, 0, 8, 16), 'alpha': '0.4'}}, None
    yield 'outer-softmask-zero', {'mask': {'rect': (0, 0, 16, 16), 'alpha': 0}}, None
    yield 'repeated-descendants', {'repeat': 16}, None
    # These deliberately remain outside the narrow single opaque gray-rectangle
    # proof. Their large BBox keeps enumeration over the ordinary instance cap.
    for label, changes in [
        ('sparse-cell-gap', {'body': 'q 0 g 0 0 0.00025 0.00025 re f Q'}),
        ('clipped-gap', {'body': 'q 0 0 0.00025 10 re W n 0 g 0 0 10 10 re f Q'}),
        ('inner-alpha', {'body': 'q /Inner gs 0 g 0 0 10 10 re f Q'}),
        ('stroke-only', {'body': 'q 0 G 0.0001 w 0 0 10 10 re S Q'}),
        ('inner-nonblack', {'body': 'q 0.5 g 0 0 10 10 re f Q'}),
        ('inner-rgb-black', {'body': 'q 0 0 0 rg 0 0 10 10 re f Q'}),
        ('two-cell-fills', {'body': 'q 0 g 0 0 10 10 re f 1 g 1 1 8 8 re f Q'}),
        ('nonrectangular-clip', {'body': 'q 0 0 m 10 0 l 0 10 l h W n 0 g 0 0 10 10 re f Q'}),
        ('uncolored-cell', {'paint_type': 2}),
        ('inherited-stencil', {'stencil': True}),
        ('insufficient-pixel-collar', {'body': 'q 0 g 0 0 0.5 0.5 re f Q'}),
        ('near-singular', {'matrix': (1, 1, 1, '1.0000000000000002', 0, 0)}),
        ('very-small-step', {'step': ('1e-300', '1e-300')}),
        ('huge-origin', {'step': ('0.000001', '0.000001'), 'matrix': (1, 0, 0, 1, '1000000000000', '1000000000000')}),
    ]:
        yield label, changes, 'limit'
    yield 'singular', {'matrix': (1, 0, 0, 0, 0, 0)}, 'unsupported'
    yield 'determinant-underflow', {'matrix': ('1e-200', 0, 0, '1e-200', 0, 0)}, 'unsupported'
    yield 'zero-step', {'step': (0, '0.001')}, 'syntax'


def fixture(identifier, changes, error=None, *, dpi=72):
    name = identifier + (f'-{dpi}dpi' if dpi != 72 else '')
    data = make_pdf(**changes)
    description = ('Single opaque black rectangle has a complete device-pixel interior instance; '
                   'outer geometry, opacity and backdrop are independent exact-rational controls')
    if error is not None:
        description = 'Ineligible cell geometry/state or uncertain numeric placement retains its explicit defensive outcome'
        return Case(name, data, UNSUPPORTED if error == 'unsupported' else OPEN,
                    (REQUIREMENT,), description,
                    feature='pattern.tiling' if error == 'unsupported' else None,
                    extra={} if error == 'unsupported' else {'render_error': error})
    oracle = expectations(changes, dpi)[1]
    tolerance = 1 if any(key in changes for key in ['alpha', 'mask', 'clip', 'mark']) else 0
    return Case(name, data, RENDER, (REQUIREMENT,), description, oracle=oracle, dpi=dpi,
                extra={'tolerance': tolerance})


def cases():
    for identifier, changes, error in definitions():
        yield fixture(identifier, changes, error)
        if error is None and identifier in ['shear', 'outer-triangle', 'outer-fractional-mark-and-clip', 'outer-softmask']:
            yield fixture(identifier, changes, dpi=144)
        if error is None and identifier in ['full-cell-default-black', 'outer-alpha', 'outer-softmask', 'outer-fractional-mark-and-clip']:
            yield fixture('direct-' + identifier, {**changes, 'direct': True})
    for size in [200, 1200]:
        yield fixture(f'direct-public-{size}', {'size': size, 'direct': True})
    function = '<< /FunctionType 2 /Domain [0 1] /C0 [0.4] /C1 [0.4] /N 1 >>'
    for key in ['BG2', 'UCR2']:
        yield fixture(key.lower() + '-selected', {
            'body': 'q /Inner gs 0 g 0 0 10 10 re f Q',
            'inner_state': f'<< /{key} {function} >>'}, 'limit')
    large = {'bbox': (0, 0, 100000000, 100000000),
             'matrix': ('0.000001', 0, 0, '0.000001', 0, 0), 'tiling_type': 2}
    yield fixture('raw-fill-working-range', {**large,
        'body': 'q 0 g 0 0 10000000000000 10000000000000 re f Q'}, 'limit')
    yield fixture('raw-clip-working-range', {**large,
        'body': 'q 0 0 10000000000000 10000000000000 re W n 0 g 0 0 100000000 100000000 re f Q'}, 'limit')
    # BBox validation precedes placement; this is a parser boundary control,
    # unlike the two geometry controls which reach over-cap enumeration.
    yield fixture('raw-bbox-parser-working-range', {**large,
        'bbox': (0, 0, 10000000000000, 10000000000000)}, 'limit')
    yield fixture('offset-local-clip', {'bbox': (0, 0, 1200, 1200),
        'matrix': (1, 0, 0, 1, -1100, -1100), 'tiling_type': 2,
        'body': 'q 1100 1100 100 100 re W n 0 g 0 0 1200 1200 re f Q'})
