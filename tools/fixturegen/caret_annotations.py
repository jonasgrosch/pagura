"""Original Caret artwork and independently integrated literal page polygons.

Table 183 defines symbols and RD, not artwork pixels. Expectations use rational
polygon/clip areas; stored controls use direct page-space polygons, independently
of the generator's normalized design-space transform.
"""
from .markup_annotations import oracle
from .model import Case, OPEN, RENDER, UNSUPPORTED
from .pdf import document, stream

AREA = 'caret_annotations'
REQUIREMENT = 'content-caret-appearances'
CARET = [(8,8),(16,24),(24,8),(16,20)]
PARAGRAPH = [(19,23),(20,24),(24,24),(24,16),(23,16),(23,23),(22,23),(22,16),
             (21,16),(21,20),(20,20),(19,21)]


def source(symbol=None, *, rect=(8,8,24,24), color=None, alpha=1., rotate=0, flags=0,
           extra=b'',objects=(),stored=False):
    box=' '.join(str(v) for v in rect).encode()
    annotation=b'<< /Type /Annot /Subtype /Caret /Rect ['+box+f'] /CA {alpha} /F {flags} '.encode()+extra
    if symbol is not None:
        annotation+=b' /Sy '+symbol
    if color is not None:
        annotation+=b' /C ['+b' '.join(str(v).encode() for v in color)+b']'
    if stored:
        annotation+=b' /AP << /N 6 0 R >>'
    values=[b'<< /Type /Catalog /Pages 2 0 R >>',b'<< /Type /Pages /Kids [3 0 R] /Count 1 >>',
            b'<< /Type /Page /Parent 2 0 R /MediaBox [0 0 32 32] /Resources << >> /Contents 4 0 R /Annots [5 0 R] '
            +f'/Rotate {rotate}'.encode()+b' >>',stream(b''),annotation+b' >>']
    if stored:
        values.append(stream(b'0 g 8 8 m 16 24 l 24 8 l 16 20 l h '
            b'19 23 m 20 24 l 24 24 l 24 16 l 23 16 l 23 23 l 22 23 l 22 16 l '
            b'21 16 l 21 20 l 20 20 l 19 21 l h f',
            b'/Type /XObject /Subtype /Form /BBox ['+box+b'] /Resources << >>'))
    return document(values+list(objects))


def cases():
    def rendered(name,symbol=None,*,polygons=(CARET,),rect=(8,8,24,24),color=None,alpha=1.,
                 fill_alpha=None,dpi=72,rotate=0,**options):
        return Case(name,source(symbol,rect=rect,color=color,alpha=alpha,rotate=rotate,**options),RENDER,
                    (REQUIREMENT,),'Independent literal page-space polygons, rational coverage and opacity',
                    dpi=dpi,oracle=oracle(polygons,rect=rect,color=color or (0.,0.,0.),
                    alpha=alpha if fill_alpha is None else fill_alpha,dpi=dpi,rotate=rotate),extra={'tolerance':1})
    for dpi in (72,144):
        yield rendered(f'caret-{dpi}',dpi=dpi)
        yield rendered(f'paragraph-{dpi}',b'/P',polygons=(CARET,PARAGRAPH),dpi=dpi)
        yield rendered(f'paragraph-stored-{dpi}',b'/P',polygons=(CARET,PARAGRAPH),dpi=dpi,stored=True)
    yield rendered('none',b'/None')
    yield rendered('null',b'null',extra=b'/RD null')
    yield rendered('rotated',b'/P',polygons=(CARET,PARAGRAPH),rotate=90,dpi=144)
    yield rendered('inset',b'/P',polygons=(CARET,PARAGRAPH),rect=(4,4,28,28),extra=b'/RD [4 4 4 4]')
    yield rendered('asymmetric',rect=(4,4,28,28),extra=b'/RD [2 4 6 8]',polygons=([(6,12),(14,24),(22,12),(14,21)],))
    yield rendered('unequal',rect=(4,8,28,24),polygons=([(4,8),(16,24),(28,8),(16,20)],),dpi=144)
    yield rendered('opacity',b'/P',polygons=(CARET,PARAGRAPH),alpha=.5,color=(0.,0.,1.))
    yield rendered('separate-opacity',b'/P',polygons=(CARET,PARAGRAPH),alpha=.75,fill_alpha=.25,extra=b'/ca 0.25')
    yield rendered('no-color',color=(),polygons=())
    yield rendered('zero-opacity',alpha=0)
    yield rendered('hidden',flags=2,polygons=())
    yield rendered('no-view',flags=32,polygons=())
    yield rendered('indirect',b'6 0 R',rect=(4,4,28,28),extra=b'/RD 7 0 R',polygons=(CARET,PARAGRAPH),
                   objects=(b'/P',b'[8 0 R 4 4 4]',b'4'))
    yield rendered('unrelated-fields',extra=b'/BE 42 /BS 42 /IC 42')
    yield rendered('stored-ignores-geometry',b'42',extra=b'/RD 42',stored=True,polygons=(CARET,PARAGRAPH))
    for name,extra in [('rd-type',b'/RD 42'),('rd-empty',b'/RD []'),('rd-short',b'/RD [0 0 0]'),
                       ('rd-long',b'/RD [0 0 0 0 0]'),('rd-number',b'/RD [0 /Bad 0 0]'),
                       ('rd-negative',b'/RD [-1 0 0 0]'),('rd-wide',b'/RD [8 0 8 0]'),
                       ('rd-tall',b'/RD [0 8 0 8]'),('symbol-type',b'/Sy 42')]:
        yield Case(name,source(color=(),extra=extra),OPEN,(REQUIREMENT,),
                   'RD cardinality, numeric types and signs and Sy type validate before empty output',extra={'render_error':'syntax'})
    yield Case('symbol-unknown',source(b'/Other'),UNSUPPORTED,(REQUIREMENT,),
               'Unrecognized symbols refuse explicitly',feature='annotation.appearance')
