"""Text markup: literal polygons, rational intersections and blend equations.

No renderer supplies expectations. Quad/band corners and the twelve-step wave
are stated independently of product geometry. The union oracle integrates the
nonzero rule exactly, including overlapping annotations' quads and BBox clips.
"""
from fractions import Fraction as F

from .model import Case, OPEN, RENDER
from .pdf import document, stream
from .raster import winding_area

AREA = 'markup_annotations'
REQUIREMENT = 'content-text-markup'
QUAD = [(4,8),(28,8),(28,20),(4,20)]
UNDERLINE = [(4,8),(28,8),(28,9),(4,9)]
STRIKEOUT = [(4,13.5),(28,13.5),(28,14.5),(4,14.5)]
WAVE = [(4,8),(6,9),(8,8),(10,9),(12,8),(14,9),(16,8),(18,9),(20,8),(22,9),(24,8),(26,9),(28,8),
        (28,8.5),(26,9.5),(24,8.5),(22,9.5),(20,8.5),(18,9.5),(16,8.5),(14,9.5),(12,8.5),(10,9.5),(8,8.5),(6,9.5),(4,8.5)]
REGIONS = {'Highlight': QUAD, 'Underline': UNDERLINE, 'StrikeOut': STRIKEOUT, 'Squiggly': WAVE}


def source(shape='Highlight', quads=b'[4 8 28 8 28 20 4 20]', *, color=None, alpha=1., extra=b'',
           background=(1.,1.,1.), rect=(0,0,32,32), rotate=0, objects=(), stored=False):
    box=' '.join(str(v) for v in rect).encode()
    annotation=b'<< /Type /Annot /Subtype /'+shape.encode()+b' /Rect ['+box+f'] /CA {alpha} '.encode()+extra
    if color is not None:
        annotation+=b' /C ['+b' '.join(str(v).encode() for v in color)+b']'
    if quads is not None:
        annotation+=b' /QuadPoints '+quads
    if stored:
        annotation+=b' /AP << /N 6 0 R >>'
    page=(' '.join(str(v) for v in background)+' rg 0 0 32 32 re f').encode()
    values=[b'<< /Type /Catalog /Pages 2 0 R >>',b'<< /Type /Pages /Kids [3 0 R] /Count 1 >>',
            b'<< /Type /Page /Parent 2 0 R /MediaBox [0 0 32 32] /Resources << >> /Contents 4 0 R /Annots [5 0 R] '
            +f'/Rotate {rotate}'.encode()+b' >>',stream(page),annotation+b' >>']
    if stored:
        values.append(stream(b'1 1 0 rg 4 8 24 12 re f',b'/Type /XObject /Subtype /Form /BBox ['+box+b'] /Resources << >>'))
    return document(values+list(objects))


def oracle(polygons, *, dpi=72, rotate=0, color=(1.,1.,0.), alpha=1., background=(1.,1.,1.),
           rect=(0,0,32,32), multiply=False):
    scale=F(dpi,72)
    def point(x,y):
        x,y=F(str(x)),F(str(y))
        return (y*scale,(32-x)*scale) if rotate else (x*scale,y*scale)
    contours=[[point(x,y) for x,y in polygon] for polygon in polygons]
    x0,y0,x1,y1=rect
    clip=[point(x0,y0),point(x1,y0),point(x1,y1),point(x0,y1)]
    groups=[('nonzero',contours),('nonzero',[clip])]
    size=int(32*scale);pixels=bytearray()
    for y in reversed(range(size)):
        for x in range(size):
            area=winding_area(groups,x,y) if contours else 0
            coverage=int(area*255+F(1,2))/255*alpha
            for c,bg in zip(color,background):
                blend=bg*c if multiply else c
                pixels.append(int(255*(bg*(1-coverage)+blend*coverage)+.5))
    return f'P6\n{size} {size}\n255\n'.encode()+pixels


def cases():
    def rendered(name,shape='Highlight',quads=b'[4 8 28 8 28 20 4 20]',*,polygons=None,color=None,
                 dpi=72,rotate=0,alpha=1.,fill_alpha=None,background=(1.,1.,1.),rect=(0,0,32,32),repairs=(),**options):
        if polygons is None:
            polygons=[REGIONS[shape]]
        rgb=(1.,1.,0.) if shape=='Highlight' else (0.,0.,0.)
        if color:
            rgb=color if len(color)==3 else color*3
        return Case(name,source(shape,quads,color=color,alpha=alpha,background=background,rect=rect,rotate=rotate,**options),
                    RENDER,(REQUIREMENT,),'Literal polygons, exact winding/clip integration and the blend equation',
                    dpi=dpi,repairs=repairs,oracle=oracle(polygons,dpi=dpi,rotate=rotate,color=rgb,
                    alpha=alpha if fill_alpha is None else fill_alpha,background=background,rect=rect,multiply=shape=='Highlight'),
                    extra={'tolerance':1})
    for shape in REGIONS:
        for dpi in (72,144):
            yield rendered(f'{shape.lower()}-{dpi}',shape,dpi=dpi)
        yield rendered(f'{shape.lower()}-alternate',shape,b'[4 20 28 20 4 8 28 8]',repairs=('annotation.quad-order',))
        yield rendered(f'{shape.lower()}-vertical',shape,b'[24 4 24 28 12 28 12 4]',
                       polygons=[[(32-y,x) for x,y in REGIONS[shape]]])
    yield rendered('multiply-black',background=(0.,0.,0.))
    yield rendered('multiply-cyan',background=(0.,1.,1.))
    yield rendered('multiply-opacity',background=(0.,1.,1.),alpha=.5)
    yield rendered('separate-opacity',alpha=.75,fill_alpha=.25,extra=b'/ca 0.25')
    yield rendered('underline-opacity','Underline',color=(0.,0.,1.),alpha=.5)
    yield rendered('strikeout-color','StrikeOut',color=(1.,0.,0.))
    yield rendered('squiggly-clipped','Squiggly',rect=(10,8,22,10),dpi=144)
    yield rendered('rotated',rotate=90,dpi=144)
    yield rendered('skewed',quads=b'[4 8 24 8 28 20 8 20]',polygons=[[(4,8),(24,8),(28,20),(8,20)]])
    yield rendered('skewed-underline','Underline',b'[4 8 24 8 28 20 8 20]',
                   polygons=[[(4,8),(24,8),(F(73,3),9),(F(13,3),9)]])
    yield rendered('overlap',quads=b'[4 8 20 8 20 20 4 20 12 8 28 8 28 20 12 20]',alpha=.5,
                   polygons=[[(4,8),(20,8),(20,20),(4,20)],[(12,8),(28,8),(28,20),(12,20)]])
    yield rendered('disjoint',quads=b'[4 8 28 8 28 14 4 14 4 18 28 18 28 24 4 24]',
                   polygons=[[(4,8),(28,8),(28,14),(4,14)],[(4,18),(28,18),(28,24),(4,24)]])
    yield rendered('no-color',color=(),polygons=[])
    yield rendered('zero-opacity',alpha=0)
    yield rendered('zero-area',quads=b'[4 8 28 8 28 8 4 8]',polygons=[])
    yield rendered('indirect',quads=b'6 0 R',objects=(b'[7 0 R 8 28 8 28 20 4 20]',b'4'))
    yield rendered('stored-ignores-geometry',quads=b'42',stored=True)
    for name,quads in [('missing',None),('null',b'null'),('type',b'42'),('empty',b'[]'),('short',b'[4 8 28 8 28 20 4]'),
                       ('number',b'[4 /Bad 28 8 28 20 4 20]'),('clockwise',b'[4 8 4 20 28 20 28 8]'),
                       ('concave',b'[4 8 28 8 12 10 4 20]')]:
        yield Case(name,source(quads=quads,color=()),OPEN,(REQUIREMENT,),
                   'Required quad count, numeric types and orientation validate before empty color',extra={'render_error':'syntax'})
