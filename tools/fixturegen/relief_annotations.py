"""Beveled/inset borders: literal trapezoids, analytic areas and source-over.

The two facets each comprise two disjoint trapezoids. Their union coverage is
quantized once per PDF fill, then composited in source order, independently of
the generated six-vertex paths. Reader artwork is policy, not mandated pixels.
"""
from .circle_annotations import rectangle_area
from .ink_annotations import polygon_area
from .model import Case, OPEN, RENDER, UNSUPPORTED
from .pdf import document, stream

AREA='relief_annotations'
REQUIREMENT='content-relief-borders'


def oracle(*,bounds=(4,4,28,28),width=2.,style='B',color=(0.,0.,0.),fill=None,
           alpha=1.,fill_alpha=1.,background=False,dpi=72,rotate=0,visible=True):
    x0,y0,x1,y1=bounds;w=min(width,(x1-x0)/2,(y1-y0)/2);scale=dpi/72
    left=[(x0,y0),(x0+w,y0+w),(x0+w,y1-w),(x0,y1)]
    top=[(x0,y1),(x0+w,y1-w),(x1-w,y1-w),(x1,y1)]
    bottom=[(x0,y0),(x1,y0),(x1-w,y0+w),(x0+w,y0+w)]
    right=[(x1,y0),(x1,y1),(x1-w,y1-w),(x1-w,y0+w)]
    def point(x,y):return (y*scale,(32-x)*scale) if rotate else (x*scale,y*scale)
    regions=[[point(*p) for p in polygon] for polygon in (left,top,bottom,right)]
    h=0. if background else min(width/2,(x1-x0)/2,(y1-y0)/2)
    a,b=point(x0+h,y0+h),point(x1-h,y1-h)
    inside=min(a[0],b[0]),min(a[1],b[1]),max(a[0],b[0]),max(a[1],b[1])
    base=tuple(max(0.,min(1.,c)) for c in (color or (0.,0.,0.)))
    if len(base)==1:base=base*3
    light=tuple((c+1)/2 for c in base);dark=tuple(c/2 for c in base)
    shades=(light,dark) if style=='B' else (dark,light)
    size=int(32*scale);pixels=bytearray()
    def mask(value):return int(max(0.,min(1.,value))*255+.5)/255
    for y in reversed(range(size)):
        for x in range(size):
            values=[1.,1.,1.]
            if visible:
                if fill is not None:
                    a=mask(rectangle_area(inside,x,y))*fill_alpha
                    values=[v*(1-a)+c*a for v,c in zip(values,fill)]
                if width and color is not None:
                    for polygons,shade in zip((regions[:2],regions[2:]),shades):
                        a=mask(sum(polygon_area(p,x,y) for p in polygons))*alpha
                        values=[v*(1-a)+c*a for v,c in zip(values,shade)]
            pixels.extend(int(v*255+.5) for v in values)
    return f'P6\n{size} {size}\n255\n'.encode()+pixels


def source(*,shape='Square',rect=(4,4,28,28),width=2.,style='B',color=(0.,0.,0.),fill=None,
           alpha=1.,fill_alpha=1.,rotate=0,flags=0,bs=None,extra=b'',objects=(),stored=False):
    def array(values):return b'['+b' '.join(str(v).encode() for v in values or ())+b']'
    border=bs if bs is not None else f'<< /W {width} /S /{style} >>'.encode()
    entries=b'/BS '+border+f' /CA {alpha} /ca {fill_alpha} /F {flags} '.encode()
    if shape=='Widget':
        entries+=b'/FT /Tx /MK << /BC '+array(color)
        if fill is not None:entries+=b' /BG '+array(fill)
        entries+=b' >>'
    elif shape=='FreeText':
        entries+=b'/DA (/Helvetica 12 Tf) /Contents ()'
        if fill is not None:entries+=b' /C '+array(fill)
    else:
        entries+=b'/C '+array(color)
        if fill is not None:entries+=b' /IC '+array(fill)
    if stored:entries+=b' /AP << /N 6 0 R >>'
    values=[b'<< /Type /Catalog /Pages 2 0 R >>',b'<< /Type /Pages /Kids [3 0 R] /Count 1 >>',
            b'<< /Type /Page /Parent 2 0 R /MediaBox [0 0 32 32] /Resources << >> /Contents 4 0 R /Annots [5 0 R] '
            +f'/Rotate {rotate}'.encode()+b' >>',stream(b''),
            b'<< /Type /Annot /Subtype /'+shape.encode()+b' /Rect '+array(rect)+b' '+entries+extra+b' >>']
    if stored:
        # Literal independent four-trapezoid control, joined in two fills.
        first=b'4 4 m 6 6 l 6 26 l 4 28 l h 4 28 m 6 26 l 26 26 l 28 28 l h f\n'
        second=b'4 4 m 28 4 l 26 6 l 6 6 l h 28 4 m 28 28 l 26 26 l 26 6 l h f\n'
        shades=(b'0.5 g\n',b'0 g\n') if style=='B' else (b'0 g\n',b'0.5 g\n')
        values.append(stream(shades[0]+first+shades[1]+second,
                             b'/Type /XObject /Subtype /Form /BBox [4 4 28 28] /Resources << >>'))
    return document(values+list(objects))


def cases():
    def rendered(name,*,dpi=72,bounds=None,visible=True,**options):
        expected={k:options[k] for k in ('width','style','color','fill','alpha','fill_alpha','rotate') if k in options}
        background=options.get('shape') in ('Widget','FreeText')
        return Case(name,source(**options),RENDER,(REQUIREMENT,),
                    'Independent trapezoid areas and per-fill coverage establish relief shading, paint order and opacity',
                    dpi=dpi,oracle=oracle(bounds=bounds or options.get('rect',(4,4,28,28)),dpi=dpi,
                                          visible=visible,background=background,**expected),extra={'tolerance':1})
    for style in ('B','I'):
        for dpi in (72,144):
            for shape in ('Link','Square','Widget','FreeText'):
                yield rendered(f'{shape}-{style}-{dpi}',shape=shape,style=style,dpi=dpi)
            yield rendered(f'stored-{style}-{dpi}',style=style,dpi=dpi,stored=True)
        yield rendered(f'filled-{style}',style=style,fill=(1.,0.,0.),color=(0.,0.,1.))
        yield rendered(f'gray-{style}',style=style,color=(.5,))
    for shape in ('Square','Widget','FreeText'):
        yield rendered(f'separate-opacity-{shape}',shape=shape,fill=(1.,0.,0.),alpha=.5,fill_alpha=.25)
        yield rendered(f'opaque-border-{shape}',shape=shape,fill=(1.,0.,0.),alpha=1.,fill_alpha=.25)
    yield rendered('fractional',rect=(4.25,4.5,27.75,27.5),width=1.5,alpha=.5)
    yield rendered('rotated',style='I',rect=(4,6,26,28),color=(0.,0.,1.),rotate=90,dpi=144)
    yield rendered('rd',extra=b'/RD [2 4 6 2]',bounds=(6,6,22,24))
    yield rendered('wide',width=100.)
    yield rendered('zero-width',width=0.)
    yield rendered('no-color',color=None)
    yield rendered('hidden',flags=2,visible=False)
    yield rendered('no-view',flags=32,visible=False)
    yield rendered('clamped-color',color=(-1.,2.,.5))
    yield rendered('bs-precedence',extra=b'/Border 999 0 R',bs=b'<< /W 2 /S /B /D 998 0 R >>')
    yield rendered('indirect',bs=b'6 0 R',objects=(b'<< /W 7 0 R /S 8 0 R >>',b'2',b'/B'))
    yield rendered('stored-ignores-style',stored=True,bs=b'42',extra=b'/Border 999 0 R')
    for name,bs in [('negative-width',b'<< /W -1 /S /B >>'),('width-type',b'<< /W /X /S /I >>')]:
        yield Case(name,source(bs=bs,color=None),OPEN,(REQUIREMENT,),
                   'Malformed widths validate before invisible output',extra={'render_error':'syntax'})
    yield Case('nonrectangular',source(shape='Ink',extra=b'/InkList [[4 4 28 28]]'),UNSUPPORTED,(REQUIREMENT,),
               'Relief shading for nonrectangular paths requires explicit artwork',feature='annotation.appearance')
