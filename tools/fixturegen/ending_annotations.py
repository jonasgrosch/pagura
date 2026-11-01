"""Line ending pixels from analytic regions, ISO 32000-2 Table 179.

Expected stroke regions below are literal offset polygons/disk differences,
not stroking the paths emitted by the implementation or another engine.
Artwork dimensions are Pagura reader policy; alternate artwork is not a
competitor defect. Body, ending interiors and ending borders are separate marks.
"""
import math

from .circle_annotations import ellipse_area
from .ink_annotations import polygon_area
from .model import Case, OPEN, RENDER, UNSUPPORTED
from .pdf import document, stream

AREA = 'ending_annotations'
REQUIREMENT = 'content-line-endings'
STYLES = ('Square', 'Circle', 'Diamond', 'OpenArrow', 'ClosedArrow', 'Butt', 'ROpenArrow', 'RClosedArrow', 'Slash')


def regions(style, width):
    """Interior, outer stroke boundary, inner stroke boundary, disk radius."""
    r, h = 3 * max(width, 1), width / 2
    def box(a):
        return [(-a, -a), (a, -a), (a, a), (-a, a)]
    def diamond(a):
        return [(-a, 0), (0, -a), (a, 0), (0, a)]
    if style == 'None':
        return [], [], [], None
    if style == 'Circle':
        return [], [], [], r
    if style == 'Square':
        return box(r), box(r + h), box(r - h), None
    if style == 'Diamond':
        return diamond(r), diamond(r + math.sqrt(2) * h), diamond(r - math.sqrt(2) * h), None
    if style == 'Butt':
        return [], [(-h, -r), (h, -r), (h, r), (-h, r)], [], None
    if style == 'Slash':
        a, b, nx, ny = r / 2, r * math.sqrt(3) / 2, -h * math.sqrt(3) / 2, h / 2
        return [], [(-a + nx, -b + ny), (a + nx, b + ny), (a - nx, b - ny), (-a - nx, -b - ny)], [], None
    k = h / math.sqrt(5)
    if style.endswith('ClosedArrow'):
        fill = [(-2*r, r), (0, 0), (-2*r, -r)]
        outer = [(-2*r-h, r+h/2+5*k/2), (5*k, 0), (-2*r-h, -r-h/2-5*k/2)]
        inner = [(-2*r+h, r-h/2-5*k/2), (-5*k, 0), (-2*r+h, -r+h/2+5*k/2)]
    else:
        fill, inner = [], []
        outer = [(-2*r+k, r+2*k), (5*k, 0), (-2*r+k, -r-2*k),
                 (-2*r-k, -r+2*k), (-5*k, 0), (-2*r-k, r-2*k)]
    if style.startswith('R'):
        fill, outer, inner = [[(-x, y) for x, y in points] for points in (fill, outer, inner)]
    return fill, outer, inner, None


def oracle(styles=('Square', 'Square'), *, width=1., fill=(1., 0., 0.), color=(0., 0., 0.),
           alpha=1., fill_alpha=1., dpi=72, rotate=0, line=((8., 16.), (24., 16.)), rect=(0, 0, 32, 32)):
    scale = dpi / 72
    def point(x, y):
        return (y*scale, (32-x)*scale) if rotate else (x*scale, y*scale)
    (x0, y0), (x1, y1) = line
    dx, dy = x1-x0, y1-y0
    length = math.hypot(dx, dy)
    dx, dy = (dx/length, dy/length) if length else (1., 0.)
    nx, ny = -dy*width/2, dx*width/2
    body = [point(x0+nx, y0+ny), point(x1+nx, y1+ny), point(x1-nx, y1-ny), point(x0-nx, y0-ny)]
    ending = []
    for i, (style, (cx, cy)) in enumerate(zip(styles, line)):
        fill_region, outer, inner, radius = regions(style, width)
        a, b = ((-dx, -dy) if i == 0 else (dx, dy)) if length else (1., 0.)
        def mapped(points):
            return [point(cx+a*x-b*y, cy+b*x+a*y) for x, y in points]
        ending.append((mapped(fill_region), mapped(outer), mapped(inner), point(cx, cy), radius))
    clip_a, clip_b = point(*rect[:2]), point(*rect[2:])
    clip = min(clip_a[0],clip_b[0]), min(clip_a[1],clip_b[1]), max(clip_a[0],clip_b[0]), max(clip_a[1],clip_b[1])
    def mask(value):
        return int(max(0., min(1., value)) * 255 + .5) / 255
    size, pixels = int(32*scale), bytearray()
    for y in reversed(range(size)):
        for x in range(size):
            if not (clip[0] <= x and x+1 <= clip[2] and clip[1] <= y and y+1 <= clip[3]):
                pixels.extend((255,255,255)); continue
            filled, stroked = 0., 0.
            for interior, outer, inner, (cx,cy), radius in ending:
                if radius is None:
                    filled += polygon_area(interior, x, y)
                    stroked += polygon_area(outer, x, y) - polygon_area(inner, x, y)
                else:
                    def disk(r):
                        return ellipse_area((cx-r*scale,cy-r*scale,cx+r*scale,cy+r*scale),x,y)
                    filled += disk(radius)
                    stroked += disk(radius+width/2) - disk(radius-width/2)
            values = [1.,1.,1.]
            for amount, paint, opacity in [(polygon_area(body,x,y) if width else 0., color, alpha),
                                            (filled, fill, fill_alpha), (stroked if width else 0., color, alpha)]:
                if paint is not None:
                    a = mask(amount)*opacity
                    values = [v*(1-a)+c*a for v,c in zip(values,paint)]
            pixels.extend(int(255*v+.5) for v in values)
    return f'P6\n{size} {size}\n255\n'.encode()+pixels


def source(styles=('Square', 'Square'), *, shape='Line', width=1., fill: tuple[float,...] | bytes | None=(1.,0.,0.), color=(0.,0.,0.), alpha=1.,
           fill_alpha=1., rotate=0, line=((8.,16.),(24.,16.)), rect=(0,0,32,32), path=None, endings=None, objects=(), extra=b''):
    xy = b' '.join(str(v).encode() for point in line for v in point)
    geometry = b' /'+(b'L' if shape=='Line' else b'Vertices')+b' ['+xy+b']'
    if path is not None:
        geometry += b' /Path '+path
    if endings is None:
        endings = b'['+b' '.join(b'/'+name.encode() for name in styles)+b']'
    style = f' /BS << /W {width} >> /CA {alpha} /ca {fill_alpha}'.encode()
    for key,value in [(b'C',color),(b'IC',fill)]:
        encoded = value if isinstance(value,bytes) else b'['+b' '.join(str(v).encode() for v in value or ())+b']'
        style += b' /'+key+b' '+encoded
    box = b' '.join(str(v).encode() for v in rect)
    return document([b'<< /Type /Catalog /Pages 2 0 R >>',b'<< /Type /Pages /Kids [3 0 R] /Count 1 >>',
                     b'<< /Type /Page /Parent 2 0 R /MediaBox [0 0 32 32] /Contents 4 0 R /Resources << >> /Annots [5 0 R] '
                     + f'/Rotate {rotate}'.encode()+b' >>',stream(b''),
                     b'<< /Type /Annot /Subtype /'+shape.encode()+b' /Rect ['+box+b']'+geometry+style+b' /LE '+endings+extra+b' >>']+list(objects))


def cases():
    def rendered(name, styles=('Square','Square'), *, shape='Line', dpi=72, path=None, endings=None, objects=(), **options):
        return Case(name,source(styles,shape=shape,path=path,endings=endings,objects=objects,**options),RENDER,
                    (REQUIREMENT,), 'Analytic offset polygons and exact disk areas verify ending artwork and paint order',
                    dpi=dpi,oracle=oracle(styles,dpi=dpi,**options),extra={'tolerance':1})
    for shape in ('Line','PolyLine'):
        for style in STYLES:
            for dpi in (72,144):
                yield rendered(f'{shape.lower()}-{style}-{dpi}',(style,style),shape=shape,dpi=dpi)
    yield rendered('mixed',('Diamond','Circle'))
    yield rendered('vertical',('Square','ClosedArrow'),line=((16.,8.),(16.,24.)))
    yield rendered('rotated',('Circle','OpenArrow'),rotate=90,dpi=144)
    yield rendered('opacity',('Square','Diamond'),alpha=.5,fill_alpha=.25,color=(0.,0.,1.),fill=(0.,1.,0.))
    yield rendered('transparent-interior',('ClosedArrow','Square'),fill=None)
    yield rendered('transparent-border',('Diamond','Circle'),color=None)
    yield rendered('zero-width',('Square','Circle'),width=0)
    yield rendered('clipped',('Square','Circle'),rect=(8,8,24,24))
    yield rendered('indirect',('Butt','Circle'),endings=b'6 0 R',objects=(b'[7 0 R /Circle]',b'/Butt'))
    yield rendered('path-cubic',('Square','ClosedArrow'),shape='PolyLine',path=b'[[8 16] [8 16 24 16 24 16]]')
    yield rendered('path-repeats',('Square','ClosedArrow'),shape='PolyLine',path=b'[[8 16] [8 16] [24 16] [24 16]]')
    for shape in ('Line','PolyLine'):
        yield Case(shape.lower()+'-unknown',source(('None','Unknown'),shape=shape),UNSUPPORTED,(REQUIREMENT,),
                   'Unknown endpoint names remain explicit',feature='annotation.appearance')
        yield Case(shape.lower()+'-interior-type',source(('Circle','None'),shape=shape,fill=b'42'),OPEN,(REQUIREMENT,),
                   'A painted closed ending validates its interior color',extra={'render_error':'syntax'})
