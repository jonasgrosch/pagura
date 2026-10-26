"""Straight line annotations, ISO 32000-2 12.5.6.7/Table 178.

Independent expected pixels reuse literal stroke-region and convex-clipping
geometry from the Ink fixtures, never a product or competitor renderer.
"""
import math

from .ink_annotations import HORIZONTAL, DIAGONAL, oracle
from .model import Case, OPEN, RENDER, UNSUPPORTED
from .pdf import document, stream

AREA = 'line_annotations'
REQUIREMENT = 'content-line-appearances'


def source(line: bytes | None=b'[4 8 28 8]', *, width=2., dash=None, color=(0., 0., 0.), alpha=1.,
           rect=(0, 0, 32, 32), rotate=0, extra=b'', objects=(), stored=False):
    box = ' '.join(str(v) for v in rect).encode()
    style = f'/BS << /W {width}'.encode()
    if dash is not None:
        style += b' /S /D /D [' + b' '.join(str(v).encode() for v in dash) + b']'
    style += b' >> /C [' + b' '.join(str(v).encode() for v in color or ()) + f'] /CA {alpha}'.encode()
    annotation = b'<< /Type /Annot /Subtype /Line /Rect [' + box + b'] ' + style + extra
    if line is not None:
        annotation += b' /L ' + line
    if stored:
        annotation += b' /AP << /N 6 0 R >>'
    values = [b'<< /Type /Catalog /Pages 2 0 R >>', b'<< /Type /Pages /Kids [3 0 R] /Count 1 >>',
              b'<< /Type /Page /Parent 2 0 R /MediaBox [0 0 32 32] /Contents 4 0 R /Resources << >> /Annots [5 0 R] '
              + f'/Rotate {rotate}'.encode() + b' >>', stream(b''), annotation + b' >>']
    if stored:
        values.append(stream(b'0 g 4 7 24 2 re f', b'/Type /XObject /Subtype /Form /BBox [' + box + b'] /Resources << >>'))
    return document(values + list(objects))


def cases():
    def rendered(name, line: bytes | None=b'[4 8 28 8]', *, regions=HORIZONTAL, polygon=(), dpi=72, rotate=0,
                 color=(0., 0., 0.), alpha=1., rect=(0, 0, 32, 32), **options):
        return Case(name, source(line, rotate=rotate, color=color, alpha=alpha, rect=rect, **options), RENDER,
                    (REQUIREMENT, 'content-annotation-generation'),
                    'Exact literal regions and convex clipping verify line coordinates, stroke and opacity',
                    dpi=dpi, oracle=oracle(regions, polygon=polygon, dpi=dpi, rotate=rotate,
                                           color=color or (0., 0., 0.), alpha=alpha, rect=rect), extra={'tolerance': 1})
    for dpi in (72, 144):
        yield rendered(f'horizontal-{dpi}', dpi=dpi)
        yield rendered(f'stored-{dpi}', dpi=dpi, stored=True)
        yield rendered(f'vertical-{dpi}', b'[16 4 16 28]', regions=[(15, 4, 17, 28)], dpi=dpi)
        yield rendered(f'diagonal-{dpi}', b'[4 4 20 20]', width=math.sqrt(2), polygon=DIAGONAL, dpi=dpi)
        yield rendered(f'dash-{dpi}', dash=[3, 1], regions=[(x, 7, x + 3, 9) for x in range(4, 28, 4)], dpi=dpi)
    yield rendered('reversed', b'[28 8 4 8]')
    yield rendered('rotated', rotate=90, dpi=144)
    yield rendered('opacity', color=(0., 0., 1.), alpha=.25, extra=b' /ca /Unused')
    yield rendered('clipped', b'[4 16 28 16]', regions=[(4, 15, 28, 17)], rect=(8, 8, 24, 24))
    yield rendered('zero-width', width=0, regions=[])
    yield rendered('no-color', color=None, regions=[])
    yield rendered('point', b'[16 16 16 16]', regions=[])
    yield rendered('none-endings', extra=b' /LE [/None /None]')
    yield rendered('null-optionals', extra=b' /LE null /LL null /LLE null /LLO null /Cap null')
    yield rendered('zero-leaders', extra=b' /LL 0 /LLE 0 /LLO 0 /Cap false /CO /Unused /CP 42')
    yield rendered('indirect', b'6 0 R', extra=b' /LE 8 0 R /Cap 10 0 R',
                   objects=(b'[7 0 R 8 28 8]', b'4', b'[/None 9 0 R]', b'/None', b'false'))
    yield rendered('stored-ignores-geometry', b'42', stored=True, extra=b' /Cap /Unused /LL /Unused')
    for name, line in [('missing', None), ('null', b'null'), ('type', b'42'), ('short', b'[4 8 28]'),
                       ('long', b'[4 8 28 8 1]'), ('number', b'[4 /Bad 28 8]')]:
        yield Case('l-' + name, source(line), OPEN, (REQUIREMENT,),
                   'Line geometry requires exactly four finite numbers', extra={'render_error': 'syntax'})
    for name, extra in [('extension-without-leader', b'/LLE 0'), ('negative-extension', b'/LL 0 /LLE -1'),
                        ('negative-offset', b'/LLO -1'), ('leader-type', b'/LL /Bad'), ('caption-type', b'/Cap 42'),
                        ('endings-type', b'/LE 42'), ('endings-count', b'/LE [/None]'), ('ending-type', b'/LE [/None 42]')]:
        yield Case(name, source(extra=extra), OPEN, (REQUIREMENT,),
                   'Line parameter types, cardinality and sign restrictions are validated', extra={'render_error': 'syntax'})
    for name, extra in [('leader', b'/LL 4'), ('extension', b'/LL 0 /LLE 2'), ('offset', b'/LLO 2'),
                        ('caption', b'/Cap true'), ('arrow', b'/LE [/None /OpenArrow]')]:
        yield Case(name, source(extra=extra), UNSUPPORTED, (REQUIREMENT,),
                   'Leader, caption and non-None ending synthesis remain explicit', feature='annotation.appearance')
