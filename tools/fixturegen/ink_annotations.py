"""InkList controls, ISO 32000-2 12.5.6.13/Table 185.

Independent expectations use literal stroke regions, exact rectangle-union
areas and convex-polygon clipping. No engine or product path code supplies the
oracle. Stored appearances fill those regions rather than stroking InkList.
"""
import math

from .circle_annotations import oracle as circle_oracle
from .model import Case, OPEN, RENDER
from .pdf import document, stream

AREA = 'ink_annotations'
REQUIREMENT = 'content-ink-appearances'
HORIZONTAL = [(4, 7, 28, 9)]
CORNER = [(4, 7, 21, 9), (19, 9, 21, 24)]
CROSS = [(4, 15, 28, 17), (15, 4, 17, 28)]
DIAGONAL = [(3.5, 4.5), (19.5, 20.5), (20.5, 19.5), (4.5, 3.5)]


def source(ink=b'[[4 8 28 8]]', *, width=2., dash=None, color=(0., 0., 0.), alpha=1.,
           rect=(0, 0, 32, 32), rotate=0, extra=b'', objects=(), stored=None):
    box = ' '.join(str(v) for v in rect).encode()
    style = f'/BS << /W {width}'.encode()
    if dash is not None:
        style += b' /S /D /D [' + b' '.join(str(v).encode() for v in dash) + b']'
    style += b' >> /C [' + b' '.join(str(v).encode() for v in color or ()) + f'] /CA {alpha}'.encode()
    annotation = b'<< /Type /Annot /Subtype /Ink /Rect [' + box + b'] ' + style + extra
    if ink is not None:
        annotation += b' /InkList ' + ink
    if stored is not None:
        annotation += b' /AP << /N 6 0 R >>'
    values = [b'<< /Type /Catalog /Pages 2 0 R >>', b'<< /Type /Pages /Kids [3 0 R] /Count 1 >>',
              b'<< /Type /Page /Parent 2 0 R /MediaBox [0 0 32 32] /Contents 4 0 R /Resources << >> /Annots [5 0 R] '
              + f'/Rotate {rotate}'.encode() + b' >>', stream(b''), annotation + b' >>']
    if stored is not None:
        values.append(stream(stored, b'/Type /XObject /Subtype /Form /BBox [' + box + b'] /Resources << >>'))
    return document(values + list(objects))


def polygon_area(polygon, x, y):
    """Clip the independently specified convex region to one pixel."""
    for axis, limit, lower in [(0, x, True), (0, x + 1, False), (1, y, True), (1, y + 1, False)]:
        output = []
        for a, b in zip(polygon, polygon[1:] + polygon[:1]):
            inside_a = a[axis] >= limit if lower else a[axis] <= limit
            inside_b = b[axis] >= limit if lower else b[axis] <= limit
            if inside_a:
                output.append(a)
            if inside_a != inside_b:
                t = (limit - a[axis]) / (b[axis] - a[axis])
                output.append((a[0] + t * (b[0] - a[0]), a[1] + t * (b[1] - a[1])))
        polygon = output
    return abs(sum(a[0] * b[1] - b[0] * a[1] for a, b in zip(polygon, polygon[1:] + polygon[:1]))) / 2


def rectangle_union(rectangles, x, y):
    clipped = [(max(x, a), max(y, b), min(x + 1, c), min(y + 1, d)) for a, b, c, d in rectangles
               if c > x and a < x + 1 and d > y and b < y + 1]
    xs = sorted({v for a, _, c, _ in clipped for v in (a, c)})
    ys = sorted({v for _, b, _, d in clipped for v in (b, d)})
    return sum((right - left) * (top - bottom)
               for left, right in zip(xs, xs[1:]) for bottom, top in zip(ys, ys[1:])
               if any(a <= (left + right) / 2 <= c and b <= (bottom + top) / 2 <= d for a, b, c, d in clipped))


def oracle(regions=(), *, polygon=(), dpi=72, rotate=0, color=(0., 0., 0.), alpha=1., rect=(0, 0, 32, 32)):
    scale = dpi / 72
    def point(x, y):
        return (y * scale, (32 - x) * scale) if rotate else (x * scale, y * scale)
    def box(bounds):
        a, b = point(*bounds[:2]), point(*bounds[2:])
        return min(a[0], b[0]), min(a[1], b[1]), max(a[0], b[0]), max(a[1], b[1])
    clip = box(rect)
    rectangles = []
    for bounds in regions:
        a, b, c, d = box(bounds)
        rectangles.append((max(a, clip[0]), max(b, clip[1]), min(c, clip[2]), min(d, clip[3])))
    vertices = [point(x, y) for x, y in polygon]
    size, pixels = int(32 * scale), bytearray()
    for y in reversed(range(size)):
        for x in range(size):
            coverage = polygon_area(vertices, x, y) if vertices else rectangle_union(rectangles, x, y)
            coverage = int(max(0., min(1., coverage)) * 255 + .5) / 255 * alpha
            pixels.extend(int(255 * (1 - coverage + c * coverage) + .5) for c in color)
    return f'P6\n{size} {size}\n255\n'.encode() + pixels


def appearance(regions):
    return b'0 g ' + b' '.join(f'{a} {b} {c-a} {d-b} re'.encode() for a, b, c, d in regions) + b' f'


def circle_path():
    """Separately authored tangent cubics; expected coverage is an exact disk."""
    commands = [b'[27 16]']
    for index in range(8):
        a, b = index * math.pi / 4, (index + 1) * math.pi / 4
        k = 4 / 3 * math.tan((b - a) / 4)
        xy = (16 + 11 * (math.cos(a) - k * math.sin(a)), 16 + 11 * (math.sin(a) + k * math.cos(a)),
              16 + 11 * (math.cos(b) + k * math.sin(b)), 16 + 11 * (math.sin(b) - k * math.cos(b)),
              16 + 11 * math.cos(b), 16 + 11 * math.sin(b))
        commands.append(b'[' + ' '.join(format(v, '.15g') for v in xy).encode() + b']')
    return b'[' + b' '.join(commands) + b']'


def source_cases():
    def rendered(name, ink=b'[[4 8 28 8]]', *, regions=HORIZONTAL, polygon=(), dpi=72, rotate=0, color=(0., 0., 0.),
                 alpha=1., rect=(0, 0, 32, 32), **options):
        return Case(name, source(ink, rotate=rotate, color=color, alpha=alpha, rect=rect, **options), RENDER,
                    (REQUIREMENT, 'content-annotation-generation'),
                    'Exact union/convex-region coverage independently checks InkList stroke geometry and opacity',
                    dpi=dpi, oracle=oracle(regions, polygon=polygon, dpi=dpi, rotate=rotate,
                                           color=color or (0., 0., 0.), alpha=alpha, rect=rect), extra={'tolerance': 1})
    for name, ink, regions in [('line', b'[[4 8 28 8]]', HORIZONTAL), ('corner', b'[[4 8 20 8 20 24]]', CORNER),
                               ('cross', b'[[4 16 28 16] [16 4 16 28]]', CROSS)]:
        for dpi in (72, 144):
            yield rendered(f'{name}-{dpi}', ink, regions=regions, dpi=dpi)
            yield rendered(f'{name}-stored-{dpi}', ink, regions=regions, dpi=dpi, stored=appearance(regions))
    for dpi in (72, 144):
        yield rendered(f'dash-{dpi}', dash=[3, 1], regions=[(x, 7, x + 3, 9) for x in range(4, 28, 4)], dpi=dpi)
        yield rendered(f'diagonal-{dpi}', b'[[4 4 20 20]]', width=math.sqrt(2), polygon=DIAGONAL, dpi=dpi)
    yield rendered('rotated', b'[[4 8 20 8 20 24]]', regions=CORNER, rotate=90, dpi=144)
    yield rendered('cross-opacity', b'[[4 16 28 16] [16 4 16 28]]', regions=CROSS, alpha=.5, color=(1., 0., 0.))
    yield rendered('clipped', b'[[4 16 28 16]]', regions=[(4, 15, 28, 17)], rect=(8, 8, 24, 24))
    yield rendered('zero-width', width=0, regions=[])
    yield rendered('no-color', color=None, regions=[])
    yield rendered('single-point', b'[[16 16]]', regions=[])
    yield rendered('null-path', extra=b' /Path null')
    yield rendered('indirect', b'6 0 R', objects=(b'[7 0 R 8 0 R]', b'[9 0 R 8 20 8]', b'[20 12 20 24]', b'4'),
                   regions=[(4, 7, 20, 9), (19, 12, 21, 24)])
    for name, ink in [('missing', None), ('null', b'null'), ('wrong-type', b'42'), ('empty', b'[]'),
                      ('empty-path', b'[[]]'), ('path-type', b'[42]'), ('odd-path', b'[[4 8 20]]'),
                      ('coordinate-type', b'[[4 /Bad 20 8]]')]:
        yield Case(name, source(ink), OPEN, (REQUIREMENT,),
                   'Ink geometry requires nonempty arrays of finite coordinate pairs', extra={'render_error': 'syntax'})
    yield rendered('pdf2-path', extra=b' /Path [[4 8] [28 8]]')
    yield rendered('stored-ignores-geometry', b'42', stored=appearance(HORIZONTAL))
    yield rendered('pdf2-precedence', b'[[16 4 16 28]]', extra=b' /Path [[4 8] [28 8]]')
    yield rendered('pdf2-cubic-line', extra=b' /Path [[4 8] [12 8 20 8 28 8]]')
    yield rendered('pdf2-mixed', extra=b' /Path [[4 8] [12 8] [16 8 24 8 28 8]]')
    yield rendered('pdf2-indirect', extra=b' /Path 6 0 R',
                   objects=(b'[7 0 R 8 0 R]', b'[4 8]', b'[12 8 20 8 9 0 R 8]', b'28'))
    for dpi, alpha in ((72, 1.), (144, 1.), (72, .5)):
        yield Case(f'pdf2-circle-{dpi}-{alpha}', source(alpha=alpha, extra=b' /Path ' + circle_path()), RENDER,
                   (REQUIREMENT,), 'Exact concentric disk areas independently check non-linear PDF 2.0 Ink path strokes',
                   dpi=dpi, oracle=circle_oracle(width=2., dpi=dpi, stroke_alpha=alpha), extra={'tolerance': 1})
    for name, path in [('type', b'42'), ('empty', b'[]'), ('first-curve', b'[[4 8 12 8 28 8]]'),
                       ('operands', b'[[4 8] [12 8 20 8]]'), ('number', b'[[4 /Bad]]')]:
        yield Case('pdf2-' + name, source(width=0, extra=b' /Path ' + path), OPEN, (REQUIREMENT,),
                   'Path validates before zero-width suppression', extra={'render_error': 'syntax'})
    for name, ink in [('missing', None), ('null', b'null'), ('invalid', b'42')]:
        yield rendered('pdf2-inklist-' + name, ink, extra=b' /Path [[4 8] [28 8]]')
    yield rendered('pdf2-ignored-coordinate', b'[[4 /Unused]]', extra=b' /Path [[4 8] [28 8]]')
    yield rendered('pdf2-ignored-cycle', b'6 0 R', extra=b' /Path [[4 8] [28 8]]', objects=(b'6 0 R',))


def cases():
    from .annotation_scope import refused
    for case in source_cases():
        if case.name.startswith('pdf2-circle-'):
            yield from refused(case)
        else:
            yield case
