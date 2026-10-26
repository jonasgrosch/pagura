"""Rounded annotation rectangles with independent conic/rectangle areas.

Circular-corner strokes use exact parallel-set outer/inner radii. Unequal-axis
corners have fill controls; their stroke/dash accuracy remains a separate gate.
Stored controls use independently constructed sixteen-arc paths.
"""
import math

from .circle_annotations import rectangle_area
from .model import Case, OPEN, RENDER, UNSUPPORTED
from .pdf import document, stream

AREA='rounded_annotations'
REQUIREMENT='content-rounded-borders'


def ellipse_region(bounds, region):
    x0,y0,x1,y1=bounds;rx=(x1-x0)/2;ry=(y1-y0)/2;cx=(x0+x1)/2;cy=(y0+y1)/2
    if min(rx,ry)<=0 or region[0]>=region[2] or region[1]>=region[3]:
        return 0.
    def primitive(t):
        return (t*math.sqrt(max(0.,1-t*t))+math.asin(t))/2
    def quadrant(dx,dy):
        a,b=min(1.,abs(dx)),min(1.,abs(dy));cut=math.sqrt(max(0.,1-b*b))
        return math.copysign(1.,dx)*math.copysign(1.,dy)*(a*b if a<=cut else cut*b+primitive(a)-primitive(cut))
    a,b=(region[0]-cx)/rx,(region[2]-cx)/rx;c,d=(region[1]-cy)/ry,(region[3]-cy)/ry
    return max(0.,(quadrant(b,d)-quadrant(a,d)-quadrant(b,c)+quadrant(a,c))*rx*ry)


def area(bounds,radii,x,y):
    x0,y0,x1,y1=bounds;rx,ry=radii
    if x1<=x0 or y1<=y0:
        return 0.
    if min(rx,ry)<=0:
        return rectangle_area(bounds,x,y)
    result=rectangle_area(bounds,x,y)
    for cx,cy,sx,sy in [(x0+rx,y0+ry,-1,-1),(x1-rx,y0+ry,1,-1),
                       (x1-rx,y1-ry,1,1),(x0+rx,y1-ry,-1,1)]:
        corner=(cx-rx if sx<0 else cx,cy-ry if sy<0 else cy,cx if sx<0 else cx+rx,cy if sy<0 else cy+ry)
        region=(max(x,corner[0]),max(y,corner[1]),min(x+1,corner[2]),min(y+1,corner[3]))
        result-=rectangle_area(corner,x,y)-ellipse_region((cx-rx,cy-ry,cx+rx,cy+ry),region)
    return max(0.,min(1.,result))


def oracle(*,bounds=(4,4,28,28),radii=(6.,6.),width=2.,fill=None,color=(0.,0.,0.),alpha=1.,fill_alpha=1.,dpi=72,rotate=0,visible=True):
    scale=dpi/72;x0,y0,x1,y1=bounds;w=width/2
    center=(x0+w,y0+w,x1-w,y1-w);rx=min(radii[0],(x1-x0)/2-w);ry=min(radii[1],(y1-y0)/2-w)
    if rx==0 or ry==0: rx=ry=0.
    assert not(width and color is not None) or rx==ry, 'unequal-axis strokes need a parallel-ellipse oracle'
    def box(r):
        a,b,c,d=r
        return tuple(v*scale for v in ((b,32-c,d,32-a) if rotate else r))
    radius=(ry*scale,rx*scale) if rotate else (rx*scale,ry*scale)
    center=box(center);outer=box(bounds);inner=box((x0+width,y0+width,x1-width,y1-width))
    outer_r=(radius[0]+w*scale,radius[1]+w*scale) if rx else (0.,0.)
    inner_r=(max(0.,radius[0]-w*scale),max(0.,radius[1]-w*scale)) if rx else (0.,0.)
    size=int(32*scale);pixels=bytearray()
    def mask(amount):return int(max(0.,min(1.,amount))*255+.5)/255
    for y in reversed(range(size)):
        for x in range(size):
            bg=mask(area(center,radius,x,y))*fill_alpha if visible and fill is not None else 0.
            fg=mask(area(outer,outer_r,x,y)-area(inner,inner_r,x,y))*alpha if visible and color is not None and width else 0.
            pixels.extend(int(255*((1-fg)*(1-bg+c*bg)+f*fg)+.5) for c,f in zip(fill or (1.,1.,1.),color or (0.,0.,0.)))
    return f'P6\n{size} {size}\n255\n'.encode()+pixels


def stored_path(rect,radii,width):
    x0,y0,x1,y1=rect;half=width/2;x0+=half;y0+=half;x1-=half;y1-=half
    rx=min(radii[0],(x1-x0)/2);ry=min(radii[1],(y1-y0)/2)
    program=[]
    for index,(cx,cy,start) in enumerate([(x1-rx,y0+ry,-math.pi/2),(x1-rx,y1-ry,0),
                                         (x0+rx,y1-ry,math.pi/2),(x0+rx,y0+ry,math.pi)]):
        program.append(f'{cx+rx*math.cos(start):.15g} {cy+ry*math.sin(start):.15g} '+('m' if index==0 else 'l'))
        for step in range(4):
            a=start+step*math.pi/8;b=a+math.pi/8;k=4/3*math.tan((b-a)/4)
            points=(cx+rx*(math.cos(a)-k*math.sin(a)),cy+ry*(math.sin(a)+k*math.cos(a)),
                    cx+rx*(math.cos(b)+k*math.sin(b)),cy+ry*(math.sin(b)-k*math.cos(b)),cx+rx*math.cos(b),cy+ry*math.sin(b))
            program.append(' '.join(f'{v:.15g}' for v in points)+' c')
    return '\n'.join(program)+'\nh\n'


def source(*,shape='Square',rect=(4,4,28,28),radii=(6.,6.),width=2.,fill=None,color=(0.,0.,0.),alpha=1.,fill_alpha=1.,
           border=None,bs=None,extra=b'',rotate=0,flags=0,objects=(),stored=False):
    box=' '.join(str(v) for v in rect).encode();border=border or f'[{radii[0]} {radii[1]} {width}]'.encode()
    style=b'/Border '+border+f' /CA {alpha} /ca {fill_alpha} /F {flags} '.encode()
    if bs is not None:style+=b' /BS '+bs
    if shape=='FreeText':style+=b' /DA (/Helvetica 12 Tf) /Contents ()'
    elif shape=='Widget':style+=b' /FT /Tx /MK << /BC [0] >>'
    else:style+=b' /C ['+b' '.join(str(v).encode() for v in color or ())+b']'
    if fill is not None:style+=b' /IC ['+b' '.join(str(v).encode() for v in fill)+b']'
    if stored:style+=b' /AP << /N 6 0 R >>'
    values=[b'<< /Type /Catalog /Pages 2 0 R >>',b'<< /Type /Pages /Kids [3 0 R] /Count 1 >>',
            b'<< /Type /Page /Parent 2 0 R /MediaBox [0 0 32 32] /Resources << >> /Contents 4 0 R /Annots [5 0 R] '
            +f'/Rotate {rotate}'.encode()+b' >>',stream(b''),b'<< /Type /Annot /Subtype /'+shape.encode()+b' /Rect ['+box+b'] '+style+extra+b' >>']
    if stored:
        path=stored_path(rect,radii,width);program=''
        if fill is not None:program+=' '.join(str(v) for v in fill)+' rg\n'+path+'f\n'
        if width and color is not None:program+=' '.join(str(v) for v in color)+f' RG\n{width} w\n'+path+'S\n'
        values.append(stream(program.encode(),b'/Type /XObject /Subtype /Form /BBox ['+box+b'] /Resources << >>'))
    return document(values+list(objects))


def cases():
    def rendered(name,*,bounds=None,dpi=72,visible=True,**options):
        expected={k:options[k] for k in ('radii','width','fill','color','alpha','fill_alpha','rotate') if k in options}
        return Case(name,source(**options),RENDER,(REQUIREMENT,),'Closed-form quarter-ellipse and rectangle areas independently establish corner pixels',
                    dpi=dpi,oracle=oracle(bounds=bounds or options.get('rect',(4,4,28,28)),dpi=dpi,visible=visible,**expected),extra={'tolerance':1})
    for dpi in (72,144):
        for shape in ('Link','Square','Widget','FreeText'):
            yield rendered(f'{shape}-{dpi}',shape=shape,dpi=dpi)
        yield rendered(f'filled-{dpi}',fill=(1.,0.,0.),color=(0.,0.,1.),dpi=dpi)
        yield rendered(f'stored-{dpi}',fill=(1.,0.,0.),color=(0.,0.,1.),dpi=dpi,stored=True)
        yield rendered(f'ellipse-fill-{dpi}',radii=(8.,4.),width=0.,fill=(1.,0.,0.),color=None,dpi=dpi)
    yield rendered('rotated',fill=(1.,0.,0.),color=(0.,0.,1.),rotate=90,dpi=144)
    yield rendered('opacity',fill=(1.,0.,0.),alpha=.5,fill_alpha=.25)
    yield rendered('clamped-radii',radii=(100.,100.))
    yield rendered('zero-horizontal',radii=(0.,6.))
    yield rendered('zero-vertical',radii=(6.,0.))
    yield rendered('no-marks',color=None)
    yield rendered('rd',radii=(4.,4.),extra=b'/RD [2 4 6 2]',bounds=(6,6,22,24))
    yield rendered('null-bs',bs=b'null')
    yield rendered('link-null-bs',shape='Link',bs=b'null')
    yield rendered('stored-ignores-fields',stored=True,fill=(1.,0.,0.),color=(0.,0.,1.),border=b'42',bs=b'42')
    yield rendered('bs-precedence',radii=(0.,0.),border=b'999 0 R',bs=b'<< /W 2 >>')
    yield rendered('indirect-radii',border=b'[6 0 R 7 0 R 8 0 R]',objects=(b'6',b'6',b'2'))
    yield rendered('hidden',flags=2,visible=False)
    yield rendered('no-view',flags=32,visible=False)
    for name,border in [('radius-type',b'[/X 6 2]'),('negative-radius',b'[-1 6 2]'),('width-type',b'[6 6 /X]')]:
        yield Case(name,source(border=border,color=None),OPEN,(REQUIREMENT,),'Invalid radii/width validate before transparent output',extra={'render_error':'syntax'})
    yield Case('nonrectangular',source(shape='Ink',extra=b'/InkList [[4 4 28 28]]'),UNSUPPORTED,(REQUIREMENT,),
               'Legacy radii on non-rectangular path artwork remain explicit',feature='annotation.appearance')
