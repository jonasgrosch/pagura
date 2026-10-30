"""Independently authored annotation rectangle and inert-Popup repair controls."""
from .model import Case, OPEN, RENDER
from .pdf import document, opaque_rectangles, stream

AREA = 'annotation_compat'
REQUIREMENT = 'content-annotation-rectangle'
REPAIR = 'annotation.popup-missing-rect'


def source(entries, *, subtype=b'Popup', objects=()):
    return document([
        b'<< /Type /Catalog /Pages 2 0 R >>',
        b'<< /Type /Pages /Kids [3 0 R] /Count 1 >>',
        b'<< /Type /Page /Parent 2 0 R /MediaBox [0 0 16 16] /Resources << >> '
        b'/Contents 4 0 R /Annots [5 0 R] >>',
        stream(b'0 0 1 rg 0 0 4 16 re f'),
        b'<< /Type /Annot /Subtype /'+subtype+b' '+entries+b' >>',
        *objects,
    ])


def pixels(dpi=72, *, stored=False):
    scale = dpi // 72
    rectangles = [((0, 0, 4*scale, 16*scale), (0, 0, 255))]
    if stored:
        rectangles.append(((6*scale, 6*scale, 4*scale, 4*scale), (255, 0, 0)))
    return opaque_rectangles(16*scale, 16*scale, rectangles)


def cases():
    for name, entries, objects in [
        ('popup-missing-rect', b'', ()),
        ('popup-null-rect', b'/Rect null', ()),
        ('popup-indirect-null-rect', b'/Rect 6 0 R', (b'null',)),
        ('popup-null-ap-missing-rect', b'/AP null', ()),
        ('popup-undefined-rect', b'/Rect 80 0 R', ()),
    ]:
        yield Case(name, source(entries, objects=objects), RENDER, (REQUIREMENT,),
                   'Repair reports skipping an inert popup with no Rect or stored artwork; existing page paint is preserved',
                   oracle=pixels(), repairs=(REPAIR,), strict_valid=False)
    for name, entries, subtype, objects in [
        ('popup-stored-missing-rect', b'/AP << /N 6 0 R >>', b'Popup',
         (stream(b'1 0 0 rg 0 0 4 4 re f', b'/Subtype /Form /BBox [0 0 4 4]'),)),
        ('popup-short-rect', b'/Rect [0 0 4]', b'Popup', ()),
        ('popup-wrong-rect', b'/Rect /Missing', b'Popup', ()),
        ('square-missing-rect', b'/C [1 0 0]', b'Square', ()),
        ('popup-rect-cycle', b'/Rect 6 0 R', b'Popup', (b'7 0 R', b'6 0 R')),
    ]:
        yield Case(name, source(entries, subtype=subtype, objects=objects), OPEN, (REQUIREMENT,),
                   'Missing or malformed geometry, artwork, and failed object resolution remain explicit errors',
                   extra={'render_error': 'limit' if name == 'popup-rect-cycle' else 'syntax'})
    ap = stream(b'1 0 0 rg 0 0 4 4 re f',
                b'/Type /XObject /Subtype /Form /BBox [0 0 4 4] /Resources << >>')
    for dpi in (72, 144):
        for kind, rect, objects in [
            ('direct', b'[6 6 10 10]', (ap,)),
            ('indirect', b'[7 0 R 8 0 R 9 0 R 10 0 R]', (ap, b'6', b'6', b'10', b'10')),
        ]:
            yield Case(f'link-{kind}-rect-{dpi}',
                       source(b'/Rect '+rect+b' /AP << /N 6 0 R >>', subtype=b'Link', objects=objects),
                       RENDER, (REQUIREMENT,),
                       'Rect scalar references resolve as ordinary PDF numbers; the stored red rectangle is independently specified',
                       dpi=dpi, oracle=pixels(dpi, stored=True))
