"""Square/Circle geometry, ISO 32000-2 12.5.6.8/Table 180.

Independent pixel areas use closed-form ellipse integrals and exact rectangle
intersections, never the product's cubic paths or a PDF renderer. Circle stroke
oracles are differences of concentric disk areas. Ellipse fill controls cover
unequal axes; dash/stroke integration also uses the existing path machinery.
"""
import math

from .model import Case, OPEN, RENDER, UNSUPPORTED
from .pdf import document, stream

AREA = 'circle_annotations'
REQUIREMENT = 'content-circle-appearances'


def source(*, shape='Circle', rect=(4, 4, 28, 28), extra=b'', rotate=0, objects=(), stored=False):
    box = ' '.join(str(v) for v in rect).encode()
    ap = b' /AP << /N 6 0 R >>' if stored else b''
    values = [b'<< /Type /Catalog /Pages 2 0 R >>', b'<< /Type /Pages /Kids [3 0 R] /Count 1 >>',
              b'<< /Type /Page /Parent 2 0 R /MediaBox [0 0 32 32] /Contents 4 0 R /Resources << >> /Annots [5 0 R] '
              + f'/Rotate {rotate}'.encode() + b' >>', stream(b''),
              b'<< /Type /Annot /Subtype /' + shape.encode() + b' /Rect [' + box + b'] ' + extra + ap + b' >>']
    if stored:
        # Separately authored stored appearance; the oracle below evaluates an
        # exact conic, independently of this tangent-cubic approximation.
        program = ['1 0 0 rg', '28 16 m']
        for index in range(8):
            a, b = index * math.pi / 4, (index + 1) * math.pi / 4
            k = 4 / 3 * math.tan((b - a) / 4)
            xy = (16 + 12 * (math.cos(a) - k * math.sin(a)), 16 + 12 * (math.sin(a) + k * math.cos(a)),
                  16 + 12 * (math.cos(b) + k * math.sin(b)), 16 + 12 * (math.sin(b) - k * math.cos(b)),
                  16 + 12 * math.cos(b), 16 + 12 * math.sin(b))
            program.append(' '.join(format(v, '.15g') for v in xy) + ' c')
        program.append('h f')
        values.append(stream('\n'.join(program).encode(), b'/Type /XObject /Subtype /Form /BBox [4 4 28 28] /Resources << >>'))
    values.extend(objects)
    return document(values)


def ellipse_area(bounds, x, y):
    """Area of an axis-aligned ellipse intersected with one unit pixel.

    Integrate min(y, sqrt(1-x*x)) in each unit-circle quadrant, then use
    inclusion/exclusion and the ellipse's affine area scale.
    """
    x0, y0, x1, y1 = bounds
    rx, ry = (x1 - x0) / 2, (y1 - y0) / 2
    if min(rx, ry) <= 0:
        return 0.
    cx, cy = (x0 + x1) / 2, (y0 + y1) / 2
    def primitive(t):
        return (t * math.sqrt(max(0., 1 - t * t)) + math.asin(t)) / 2
    def quadrant(dx, dy):
        a, b = min(1., abs(dx)), min(1., abs(dy))
        cut = math.sqrt(max(0., 1 - b * b))
        area = a * b if a <= cut else cut * b + primitive(a) - primitive(cut)
        return math.copysign(1., dx) * math.copysign(1., dy) * area
    a, b = (x - cx) / rx, (x + 1 - cx) / rx
    c, d = (y - cy) / ry, (y + 1 - cy) / ry
    return max(0., min(1., (quadrant(b, d) - quadrant(a, d) - quadrant(b, c) + quadrant(a, c)) * rx * ry))


def rectangle_area(bounds, x, y):
    x0, y0, x1, y1 = bounds
    return max(0., min(x + 1, x1) - max(x, x0)) * max(0., min(y + 1, y1) - max(y, y0))


def oracle(*, shape='Circle', bounds=(4, 4, 28, 28), width=1., fill=None, stroke=(0., 0., 0.),
           fill_alpha=1., stroke_alpha=1., dpi=72, rotate=0):
    scale = dpi / 72
    x0, y0, x1, y1 = bounds
    if rotate:
        x0, y0, x1, y1 = y0, 32 - x1, y1, 32 - x0
    outer = tuple(v * scale for v in (x0, y0, x1, y1))
    w = width * scale
    interior = (outer[0] + w / 2, outer[1] + w / 2, outer[2] - w / 2, outer[3] - w / 2)
    inner = (outer[0] + w, outer[1] + w, outer[2] - w, outer[3] - w)
    area = rectangle_area if shape == 'Square' else ellipse_area
    if shape == 'Circle' and width and stroke is not None:
        assert x1 - x0 == y1 - y0, 'disk-difference stroke oracle requires a circle'
    size = int(32 * scale)
    pixels = bytearray()
    def mask(value):
        return int(max(0., min(1., value)) * 255 + .5) / 255
    for y in reversed(range(size)):
        for x in range(size):
            bg = mask(area(interior, x, y)) * fill_alpha if fill is not None else 0.
            fg = mask(area(outer, x, y) - area(inner, x, y)) * stroke_alpha if width and stroke is not None else 0.
            pixels.extend(int(255 * ((1 - fg) * (1 - bg + color * bg) + foreground * fg) + .5)
                          for color, foreground in zip(fill or (1., 1., 1.), stroke or (0., 0., 0.)))
    return f'P6\n{size} {size}\n255\n'.encode() + pixels


def cases():
    def render(name, *, shape='Circle', rect=(4, 4, 28, 28), bounds=None, width=1., fill=None,
               stroke=(0., 0., 0.), fill_alpha=1., stroke_alpha=1., dpi=72, rotate=0, extra=b'', objects=(), stored=False):
        style = f'/BS << /W {width} >> /CA {stroke_alpha} /ca {fill_alpha}'.encode()
        style += b' /C [' + b' '.join(str(c).encode() for c in stroke or ()) + b']'
        if fill is not None:
            style += b' /IC [' + b' '.join(str(c).encode() for c in fill) + b']'
        return Case(name, source(shape=shape, rect=rect, extra=style + extra, rotate=rotate, objects=objects, stored=stored),
                    RENDER, (REQUIREMENT, 'content-annotation-generation'),
                    'Closed-form conic/rectangle pixel areas validate generated shape bounds, interior and border opacity',
                    dpi=dpi, oracle=oracle(shape=shape, bounds=bounds or rect, width=width, fill=fill, stroke=stroke,
                                           fill_alpha=fill_alpha, stroke_alpha=stroke_alpha, dpi=dpi, rotate=rotate),
                    extra={'tolerance': 1})
    for dpi in (72, 144):
        yield render(f'circle-outline-{dpi}', dpi=dpi)
        yield render(f'circle-fill-{dpi}', width=0, fill=(1., 0., 0.), dpi=dpi)
        yield render(f'circle-stored-{dpi}', width=0, fill=(1., 0., 0.), dpi=dpi, stored=True)
        yield render(f'circle-border-fill-{dpi}', width=2, fill=(0., 1., 0.), stroke=(0., 0., 1.), dpi=dpi)
        yield render(f'ellipse-fill-{dpi}', rect=(4, 8, 28, 24), width=0, fill=(1., 0., 0.), dpi=dpi)
        yield render(f'ellipse-rotated-{dpi}', rect=(4, 8, 28, 24), width=0, fill=(1., 0., 0.), dpi=dpi, rotate=90)
    yield render('circle-opacity', width=2, fill=(1., 0., 0.), fill_alpha=.25, stroke_alpha=.5)
    yield render('no-mark', width=0)
    yield render('transparent-border', fill=(0., 1., 0.), stroke=None)
    for shape in ('Circle', 'Square'):
        yield render(shape.lower() + '-rd', shape=shape, bounds=(6, 4, 22, 24), width=0,
                     fill=(1., 0., 0.), extra=b' /RD [2 4 6 0] /BE << /S /S >>')
        yield render(shape.lower() + '-null', shape=shape, width=0, fill=(1., 0., 0.), extra=b' /RD null /BE null')
        yield render(shape.lower() + '-empty-effect', shape=shape, width=0, fill=(1., 0., 0.), extra=b' /BE << >>')
    yield render('indirect-rd', bounds=(6, 4, 22, 24), width=0, fill=(1., 0., 0.),
                 extra=b' /RD 6 0 R', objects=(b'[7 0 R 4 6 0]', b'2'))
    yield render('reversed-rectangle', rect=(28, 28, 4, 4), bounds=(4, 4, 28, 28), width=0, fill=(1., 0., 0.))
    for name, extra in [('rd-type', b'/RD 42'), ('rd-short', b'/RD [1 2 3]'), ('rd-negative', b'/RD [-1 0 0 0]'),
                        ('rd-width', b'/RD [12 0 12 0]'), ('rd-height', b'/RD [0 12 0 12]'),
                        ('rd-entry', b'/RD [0 /Bad 0 0]'), ('effect-type', b'/BE 42'), ('effect-style', b'/BE << /S 42 >>')]:
        yield Case(name, source(extra=extra), OPEN, (REQUIREMENT,),
                   'Malformed rectangle differences and effect types refuse generation', extra={'render_error': 'syntax'})
    yield Case('cloudy', source(extra=b'/BE << /S /C /I 1 >>'), UNSUPPORTED, (REQUIREMENT,),
               'Cloudy border synthesis remains an explicit gap', feature='annotation.appearance')
