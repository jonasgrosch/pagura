"""Rounded annotation rectangles with independent conic and normal-offset areas.

Circular-corner strokes use exact parallel-set radii. Elliptical-corner strokes
use true conic normals; circular dashes use closed-form arc lengths and butt caps.
Stored controls use independently constructed sixteen-arc paths.
"""
import math
from decimal import Decimal, localcontext

from .circle_annotations import rectangle_area
from .cloud_annotations import polygon_coverage
from .model import Case, OPEN, RENDER, UNSUPPORTED
from .pdf import document, stream

AREA='rounded_annotations'
REQUIREMENT='content-rounded-borders'


def stroke_contours(rect=(4,4,28,28),radii=(8.,4.),width=2.,dash=()):
    """Convex parallel curves, independent of the generated cubic appearance.

    For q=(a cos(t),b sin(t)), |q''|<=a and the normal's second derivative
    is bounded by 2ab(a²-b²)/b⁴+(a/b)², taking a>=b. Chord interpolation
    error is at most max|offset''|*dt²/8. A conservative full-boundary strip
    bound stays below 1/8 channel at 144dpi. Interior offsets here have no cusps.
    """
    half=width/2;x0,y0,x1,y1=rect;x0+=half;y0+=half;x1-=half;y1-=half
    rx=min(radii[0],(x1-x0)/2);ry=min(radii[1],(y1-y0)/2);a,b=max(rx,ry),min(rx,ry)
    assert 0<half<b*b/a, 'selected controls require regular convex offsets'
    assert not dash or rx==ry, 'elliptical arc-length dashes need a separate bound'
    centers=[(x1-rx,y0+ry),(x1-rx,y1-ry),(x0+rx,y1-ry),(x0+rx,y0+ry)]
    def point(index,t):
        theta=(index-1)*math.pi/2+t*math.pi/2;cx,cy=centers[index]
        c,s=math.cos(theta),math.sin(theta);n=math.hypot(ry*c,rx*s)
        return (cx+rx*c,cy+ry*s),(ry*c/n,rx*s/n)
    segments=[]
    for index in range(4):
        # Length is used only for circular dashes. Solid elliptical controls
        # consume every whole segment and do not approximate ellipse arclength.
        segments.append((index,rx*math.pi/2))
        end,_=point(index,1);start,_=point((index+1)%4,0)
        segments.append((index+4,math.dist(end,start)))
    total=sum(length for _,length in segments);intervals=[]
    if dash:
        cursor=0.;index=0
        while cursor<total:
            stop=min(total,cursor+dash[index%len(dash)])
            if index%2==0:intervals.append((cursor,stop))
            cursor=stop;index+=1
    else:intervals=[(0.,total)]
    second=a+half*(2*a*b*(a*a-b*b)/b**4+(a/b)**2)
    perimeter=4*(x1-x0+y1-y0)+4*half*len(intervals)
    samples=1
    while second*(math.pi/(2*samples))**2/8*perimeter*4*255>=.125:samples*=2
    contours=[]
    for begin,end in intervals:
        outer=[];inner=[];cursor=0.
        for index,length in segments:
            left,right=max(begin,cursor),min(end,cursor+length)
            if left<right:
                lo,hi=(left-cursor)/length,(right-cursor)/length
                count=max(1,math.ceil((hi-lo)*samples)) if index<4 else 1
                for step in range(count+1):
                    t=lo+(hi-lo)*step/count
                    if index<4:vertex,normal=point(index,t)
                    else:
                        one,normal=point(index-4,1);two,_=point((index-3)%4,0)
                        vertex=(one[0]+t*(two[0]-one[0]),one[1]+t*(two[1]-one[1]))
                    outer.append((vertex[0]+half*normal[0],vertex[1]+half*normal[1]))
                    inner.append((vertex[0]-half*normal[0],vertex[1]-half*normal[1]))
            cursor+=length
        if dash:contours.append(outer+list(reversed(inner)))
        else:contours.extend((outer,list(reversed(inner))))
    return contours


def stroke_oracle(*,rect=(4,4,28,28),radii=(8.,4.),width=2.,dash=(),dpi=72,rotate=0,alpha=1.):
    scale=dpi/72;size=int(32*scale)
    def point(p):
        x,y=p
        return (y*scale,(32-x)*scale) if rotate else (x*scale,y*scale)
    contours=[[point(p) for p in contour] for contour in stroke_contours(rect,radii,width,dash)]
    areas=polygon_coverage(contours,size);pixels=bytearray()
    for y in reversed(range(size)):
        for x in range(size):
            coverage=int(areas[y*size+x]*255+.5)/255
            pixels.extend([int(255*(1-coverage*alpha)+.5)]*3)
    return f'P6\n{size} {size}\n255\n'.encode()+pixels


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
    # Decimal half-angle identities make the independently authored 16-arc
    # control identical across platforms; binary libm sin/cos last bits differ.
    with localcontext() as context:
        context.prec=60
        zero,one=Decimal(0),Decimal(1)
        x0,y0,x1,y1=map(lambda value:Decimal(str(value)),rect)
        half=Decimal(str(width))/2;x0+=half;y0+=half;x1-=half;y1-=half
        rx=min(Decimal(str(radii[0])),(x1-x0)/2);ry=min(Decimal(str(radii[1])),(y1-y0)/2)
        diagonal=(one/2).sqrt();c=((one+diagonal)/2).sqrt();s=((one-diagonal)/2).sqrt()
        c16=((one+c)/2).sqrt();k=4*((one-c16)/(one+c16)).sqrt()/3
        quarter=[(one,zero),(c,s),(diagonal,diagonal),(s,c),(zero,one)]
        program=[]
        for index,(cx,cy) in enumerate([(x1-rx,y0+ry),(x1-rx,y1-ry),(x0+rx,y1-ry),(x0+rx,y0+ry)]):
            directions=[(y,-x) if index==0 else (x,y) if index==1 else (-y,x) if index==2 else (-x,-y) for x,y in quarter]
            def coordinates(values):
                return ' '.join(format(value.quantize(Decimal('0.000000000001')),'f') for value in values)
            ax,ay=directions[0]
            program.append(coordinates((cx+rx*ax,cy+ry*ay))+(' m' if index==0 else ' l'))
            for (ax,ay),(bx,by) in zip(directions,directions[1:]):
                points=(cx+rx*(ax-k*ay),cy+ry*(ay+k*ax),cx+rx*(bx+k*by),cy+ry*(by-k*bx),cx+rx*bx,cy+ry*by)
                program.append(coordinates(points)+' c')
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


def stroke_cases():
    for dpi in (72,144):
        for shape in ('Square','Widget'):
            yield Case(f'ellipse-stroke-{shape}-{dpi}',source(shape=shape,radii=(8.,4.)),RENDER,(REQUIREMENT,),
                       'True ellipse normal offsets establish unequal-axis corner strokes',dpi=dpi,
                       oracle=stroke_oracle(dpi=dpi),extra={'tolerance':1})
        yield Case(f'ellipse-stroke-stored-{dpi}',source(radii=(8.,4.),stored=True),RENDER,(REQUIREMENT,),
                   'An independently authored finer cubic appearance meets the same conic-normal oracle',dpi=dpi,
                   oracle=stroke_oracle(dpi=dpi),extra={'tolerance':1})
        for dash in ((3,3),(7,2)):
            name=f'circular-dash-{dash[0]}-{dpi}'
            border=f'[6 6 2 [{dash[0]} {dash[1]}]]'.encode()
            yield Case(name,source(border=border),RENDER,(REQUIREMENT,),
                       'Exact straight and circular arclengths position dash cuts with true normals and butt caps',dpi=dpi,
                       oracle=stroke_oracle(radii=(6.,6.),dash=dash,dpi=dpi),extra={'tolerance':1})
    for name,options in [('ellipse-stroke-opacity',{'alpha':.5}),('ellipse-stroke-rotated',{'rotate':90})]:
        yield Case(name,source(radii=(8.,4.),**options),RENDER,(REQUIREMENT,),
                   'Conic-normal stroke coverage followed by opacity or page rotation establishes pixels',
                   oracle=stroke_oracle(**options),extra={'tolerance':1})


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
    yield from stroke_cases()
