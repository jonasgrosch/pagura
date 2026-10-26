"""Independent quadratic-area controls for the documented cloudy artwork.

The standard requires convex lobes but does not mandate their geometry. These
controls integrate the stated parabolas analytically; they do not flatten curves
or execute the engine. BS width/dash and limit contracts have public API checks.
"""
import math

from .font_pixels import signed_area
from .model import Case, OPEN, RENDER
from .pdf import document, stream

AREA='cloud_annotations'
REQUIREMENT='content-cloudy-borders'


def source(shape='Square',*,intensity=1,width=0,alpha=1.,rotate=0,extra=b'',stored=None,effect=None):
    effect=f'<< /S /C /I {intensity} >>'.encode() if effect is None else effect
    style=b'/BE '+effect+f' /BS << /W {width} >> /CA {alpha} '.encode()
    if shape=='FreeText':style+=b'/DA (/Helvetica 12 Tf) /Contents () /C [1 0 0] /RD [8 8 8 8] '
    elif shape=='Polygon':style+=b'/Vertices [8 8 24 8 24 24 8 24] /C [] /IC [1 0 0] '
    else:style+=b'/RD [8 8 8 8] /C [] /IC [1 0 0] '
    if stored is not None:style+=b'/AP << /N 6 0 R >> '
    values=[b'<< /Type /Catalog /Pages 2 0 R >>',b'<< /Type /Pages /Kids [3 0 R] /Count 1 >>',
            b'<< /Type /Page /Parent 2 0 R /MediaBox [0 0 32 32] /Resources << >> /Contents 4 0 R /Annots [5 0 R] '
            +f'/Rotate {rotate}'.encode()+b' >>',stream(b''),
            b'<< /Type /Annot /Subtype /'+shape.encode()+b' /Rect [0 0 32 32] '+style+extra+b' >>']
    if stored is not None:values.append(stream(stored,b'/Type /XObject /Subtype /Form /BBox [0 0 32 32] /Resources << >>'))
    return document(values)


def curves(shape='Square',intensity=1,width=0):
    # Independently specified knots, and each quadratic's control vertex.
    low,high=8+width/2,24-width/2
    if shape=='Circle':
        r=(high-low)/2;d=math.sqrt(.5)
        points=[(16+r*x,16+r*y) for x,y in [(1,0),(d,d),(0,1),(-d,d),(-1,0),(-d,-d),(0,-1),(d,-d)]]
    else:points=[(low,low),(high,low),(high,high),(low,high)]
    output=[]
    for start,end in zip(points,points[1:]+points[:1]):
        dx,dy=end[0]-start[0],end[1]-start[1]
        n=math.ceil(math.hypot(dx,dy)/((4+4*intensity)*max(width,1)))
        for k in range(n):
            a=(start[0]+dx*k/n,start[1]+dy*k/n);b=(start[0]+dx*(k+1)/n,start[1]+dy*(k+1)/n)
            control=((a[0]+b[0])/2+(b[1]-a[1]),(a[1]+b[1])/2-(b[0]-a[0]))
            output.append((a,control,b))
    return output


def oracle(shape='Square',*,intensity=1,width=0,alpha=1.,dpi=72,rotate=0):
    scale=dpi/72
    def point(p):
        x,y=p
        return (y*scale,(32-x)*scale) if rotate else (x*scale,y*scale)
    edges=[tuple(point(p) for p in curve) for curve in curves(shape,intensity,width)]
    size=int(32*scale);pixels=bytearray()
    for y in reversed(range(size)):
        for x in range(size):
            area=abs(signed_area(edges,(x,y,x+1,y+1)))
            coverage=int(min(1.,max(0.,area))*255+.5)/255
            c=int(255*(1-coverage*alpha)+.5)
            pixels.extend((255,c,c))
    return f'P6\n{size} {size}\n255\n'.encode()+pixels


def cases():
    for shape in ('Square','Circle','Polygon','FreeText'):
        for dpi in (72,144):
            yield Case(f'{shape}-{dpi}',source(shape),RENDER,(REQUIREMENT,),
                       'Exact quadratic contour integrals establish the documented convex cloud lobes',
                       oracle=oracle(shape,dpi=dpi),dpi=dpi,extra={'tolerance':1})
    for name,options in [('strong',{'intensity':2}),('fractional',{'intensity':.5}),
                         ('opacity',{'alpha':.5}),('rotated',{'rotate':90}),('width-affects-lobes',{'width':2})]:
        yield Case(name,source(**options),RENDER,(REQUIREMENT,),
                   'Independent quadratic areas, annotation clipping and source-over establish pixels',
                   oracle=oracle(**options),extra={'tolerance':1})
    for name,effect in [('bad-intensity',b'<< /S /C /I /Bad >>'),('negative',b'<< /S /C /I -1 >>'),
                       ('excess',b'<< /S /C /I 3 >>'),('bad-effect',b'42')]:
        yield Case(name,source(effect=effect),OPEN,(REQUIREMENT,),
                   'Malformed cloud declarations fail before no-mark shortcuts',extra={'render_error':'syntax'})
