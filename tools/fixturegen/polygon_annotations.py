"""Polygon/PolyLine and PDF 2.0 Path, ISO 32000-2 12.5.6.9/Table 181.

Expectations use exact convex clipping, literal rectangular stroke regions,
and a closed-form quadratic area integral. No product/competitor renderer or
path evaluator is used. The curved fill is y=8+(x-4)^2/48 above y=4 on [4,28].
"""
import math

from .ink_annotations import polygon_area, rectangle_union
from .model import Case, OPEN, RENDER, UNSUPPORTED
from .pdf import document, stream

AREA = 'polygon_annotations'
REQUIREMENT = 'content-polygon-appearances'
TRIANGLE = [(4, 4), (28, 4), (4, 28)]
BOX = [(8, 8), (24, 8), (24, 24), (8, 24)]
FRAME = [(7, 7, 25, 9), (7, 23, 25, 25), (7, 9, 9, 23), (23, 9, 25, 23)]
OPEN_FRAME = [(8, 7, 25, 9), (23, 9, 25, 25), (8, 23, 23, 25)]
CURVE = b'[[4 8] [12 8 20 12 28 20] [28 4] [4 4]]'


def source(vertices: bytes | None=b'[4 4 28 4 4 28]', *, shape='Polygon', path=None, width=0., fill=(1., 0., 0.),
           color=(0., 0., 0.), fill_alpha=1., stroke_alpha=1., rotate=0, extra=b'', objects=(), stored=None):
    style = f'/BS << /W {width} >> /CA {stroke_alpha} /ca {fill_alpha}'.encode()
    style += b' /C [' + b' '.join(str(v).encode() for v in color or ()) + b']'
    if fill is not None:
        style += b' /IC [' + b' '.join(str(v).encode() for v in fill) + b']'
    annotation = b'<< /Type /Annot /Subtype /' + shape.encode() + b' /Rect [0 0 32 32] ' + style + extra
    if vertices is not None:
        annotation += b' /Vertices ' + vertices
    if path is not None:
        annotation += b' /Path ' + path
    if stored is not None:
        annotation += b' /AP << /N 6 0 R >>'
    values = [b'<< /Type /Catalog /Pages 2 0 R >>', b'<< /Type /Pages /Kids [3 0 R] /Count 1 >>',
              b'<< /Type /Page /Parent 2 0 R /MediaBox [0 0 32 32] /Contents 4 0 R /Resources << >> /Annots [5 0 R] '
              + f'/Rotate {rotate}'.encode() + b' >>', stream(b''), annotation + b' >>']
    if stored is not None:
        values.append(stream(stored, b'/Type /XObject /Subtype /Form /BBox [0 0 32 32] /Resources << >>'))
    return document(values + list(objects))


def quadratic_area(x, y, scale):
    """Analytic area below the parabola within one device pixel."""
    left, right = max(4., x / scale), min(28., (x + 1) / scale)
    bottom, top = max(4., y / scale), (y + 1) / scale
    if left >= right or top <= bottom:
        return 0.
    cuts = [left, right]
    for level in (bottom, top):
        if level >= 8:
            root = 4 + math.sqrt(48 * (level - 8))
            if left < root < right:
                cuts.append(root)
    cuts.sort()
    total = 0.
    for a, b in zip(cuts, cuts[1:]):
        height = 8 + (((a + b) / 2) - 4) ** 2 / 48
        if height >= top:
            total += (b - a) * (top - bottom)
        elif height > bottom:
            total += (8 - bottom) * (b - a) + ((b - 4) ** 3 - (a - 4) ** 3) / 144
    return max(0., min(1., total * scale * scale))


def oracle(polygons=(TRIANGLE,), *, regions=(), curve=False, dpi=72, rotate=0, fill=(1., 0., 0.),
           color=(0., 0., 0.), fill_alpha=1., stroke_alpha=1.):
    scale = dpi / 72
    def point(x, y):
        return (y * scale, (32 - x) * scale) if rotate else (x * scale, y * scale)
    vertices = [[point(x, y) for x, y in points] for points in polygons]
    rectangles = []
    for x, y, xx, yy in regions:
        a, b = point(x, y), point(xx, yy)
        rectangles.append((min(a[0], b[0]), min(a[1], b[1]), max(a[0], b[0]), max(a[1], b[1])))
    size, pixels = int(32 * scale), bytearray()
    def mask(area):
        return int(max(0., min(1., area)) * 255 + .5) / 255
    for y in reversed(range(size)):
        for x in range(size):
            filled = quadratic_area(x, y, scale) if curve else sum(polygon_area(points, x, y) for points in vertices)
            bg = mask(filled) * fill_alpha if fill is not None else 0.
            fg = mask(rectangle_union(rectangles, x, y)) * stroke_alpha if color is not None else 0.
            pixels.extend(int(255 * ((1 - fg) * (1 - bg + c * bg) + s * fg) + .5)
                          for c, s in zip(fill or (1., 1., 1.), color or (0., 0., 0.)))
    return f'P6\n{size} {size}\n255\n'.encode() + pixels


def cases():
    def rendered(name, vertices: bytes | None=b'[4 4 28 4 4 28]', *, polygons=(TRIANGLE,), regions=(), curve=False, dpi=72,
                 rotate=0, fill=(1., 0., 0.), color=(0., 0., 0.), fill_alpha=1., stroke_alpha=1., **options):
        return Case(name, source(vertices, rotate=rotate, fill=fill, color=color, fill_alpha=fill_alpha,
                                 stroke_alpha=stroke_alpha, **options), RENDER,
                    (REQUIREMENT, 'content-annotation-generation'),
                    'Independent convex/region coverage and quadratic integrals verify vertices and PDF 2.0 path appearances',
                    dpi=dpi, oracle=oracle(polygons, regions=regions, curve=curve, dpi=dpi, rotate=rotate, fill=fill,
                                           color=color, fill_alpha=fill_alpha, stroke_alpha=stroke_alpha), extra={'tolerance': 1})
    for dpi in (72, 144):
        yield rendered(f'triangle-{dpi}', dpi=dpi)
        yield rendered(f'triangle-stored-{dpi}', dpi=dpi,
                       stored=b'1 0 0 rg 24 0 0 24 4 4 cm 0 0 m 1 0 l 0 1 l h f')
        yield rendered(f'polygon-border-{dpi}', b'[8 8 24 8 24 24 8 24]', width=2, fill=None, regions=FRAME, dpi=dpi)
        yield rendered(f'polyline-{dpi}', b'[8 8 24 8 24 24 8 24]', shape='PolyLine', width=2, fill=None, regions=OPEN_FRAME, dpi=dpi)
        yield rendered(f'curve-fill-{dpi}', None, path=CURVE, curve=True, dpi=dpi)
        yield rendered(f'curve-line-{dpi}', None, path=b'[[4 8] [12 8 20 8 28 8]]', shape='PolyLine',
                       width=2, fill=None, regions=[(4, 7, 28, 9)], dpi=dpi)
    yield rendered('clockwise', b'[4 28 28 4 4 4]')
    yield rendered('rotated', rotate=90, dpi=144)
    yield rendered('separate-opacity', b'[8 8 24 8 24 24 8 24]', width=2, polygons=(BOX,), regions=FRAME,
                   fill=(0., 1., 0.), color=(0., 0., 1.), fill_alpha=.25, stroke_alpha=.5)
    yield rendered('self-crossing', b'[4 4 28 28 4 28 28 4]',
                   polygons=([(4, 4), (16, 16), (28, 4)], [(16, 16), (28, 28), (4, 28)]))
    yield rendered('path-lines', None, path=b'[[4 4] [28 4] [4 28]]')
    yield rendered('path-precedence', b'42', path=b'[[4 4] [28 4] [4 28]]')
    yield rendered('null-path', path=b'null')
    yield rendered('plain-effect', extra=b' /BE << /S /S >> /LE 42')
    yield rendered('null-effect', extra=b' /BE null')
    yield rendered('none-endings', b'[8 8 24 8 24 24 8 24]', shape='PolyLine', width=2, fill=None,
                   regions=OPEN_FRAME, extra=b' /LE [/None /None] /BE 42')
    yield rendered('null-endings', b'[8 8 24 8 24 24 8 24]', shape='PolyLine', width=2, fill=None,
                   regions=OPEN_FRAME, extra=b' /LE null')
    yield rendered('indirect-vertices', b'6 0 R', objects=(b'[7 0 R 4 28 4 4 28]', b'4'))
    yield rendered('indirect-path', None, path=b'6 0 R', curve=True,
                   objects=(b'[7 0 R 8 0 R [28 4] [4 4]]', b'[4 8]', b'[12 8 20 12 9 0 R 20]', b'28'))
    yield rendered('no-mark', width=0, fill=None)
    yield rendered('no-stroke-color', width=2, color=None)
    for name, value in [('missing', None), ('null', b'null'), ('type', b'42'), ('empty', b'[]'),
                        ('odd', b'[4 4 28]'), ('number', b'[4 /Bad]')]:
        yield Case('vertices-' + name, source(value), OPEN, (REQUIREMENT,),
                   'Malformed vertex geometry fails before painting', extra={'render_error': 'syntax'})
    for name, value in [('type', b'42'), ('empty', b'[]'), ('first-curve', b'[[4 4 8 8 12 12]]'),
                        ('operands', b'[[4 4] [8 8 12 12]]'), ('entry', b'[[4 4] 42]'), ('number', b'[[4 /Bad]]')]:
        yield Case('path-' + name, source(None, path=value), OPEN, (REQUIREMENT,),
                   'PDF 2.0 paths require a moveto pair followed by line pairs or cubic operands', extra={'render_error': 'syntax'})
    for name, extra, shape in [('effect', b'/BE 42', 'Polygon'), ('endings-type', b'/LE 42', 'PolyLine'),
                               ('endings-count', b'/LE [/None]', 'PolyLine'), ('ending-name', b'/LE [/None 42]', 'PolyLine')]:
        yield Case(name, source(extra=extra, shape=shape), OPEN, (REQUIREMENT,),
                   'Malformed effect/ending declarations fail explicitly', extra={'render_error': 'syntax'})
    for name, extra, shape in [('cloudy', b'/BE << /S /C >>', 'Polygon'), ('arrow', b'/LE [/None /OpenArrow]', 'PolyLine')]:
        yield Case(name, source(extra=extra, shape=shape), UNSUPPORTED, (REQUIREMENT,),
                   'Cloudy borders and polyline end styles remain explicit gaps', feature='annotation.appearance')
