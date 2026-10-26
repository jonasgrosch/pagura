"""Independent fill and stroke controls for the documented cloudy artwork.

The standard requires convex lobes but does not mandate their geometry. These
controls integrate fills analytically and bounded normal-offset stroke contours
geometrically. No product flattening, stroke builder or engine is used.
"""
import math
from functools import lru_cache

from .font_pixels import signed_area
from .model import Case, OPEN, RENDER
from .pdf import document, stream

AREA='cloud_annotations'
REQUIREMENT='content-cloudy-borders'


def source(shape='Square',*,intensity=1,width=0,alpha=1.,rotate=0,extra=b'',stored=None,effect=None,stroke=False,dash=None):
    effect=f'<< /S /C /I {intensity} >>'.encode() if effect is None else effect
    border=f'/BS << /W {width}'.encode()+(b' /S /D /D ['+b' '.join(str(v).encode() for v in dash)+b']' if dash else b'')+b' >>'
    style=b'/BE '+effect+b' '+border+f' /CA {alpha} '.encode()
    if shape=='FreeText':style+=b'/DA (/Helvetica 12 Tf) /Contents () /C '+(b'[]' if stroke else b'[1 0 0]')+b' /RD [8 8 8 8] '
    elif shape=='Polygon':style+=b'/Vertices [8 8 24 8 24 24 8 24] '+(b'/C [0] /IC [] ' if stroke else b'/C [] /IC [1 0 0] ')
    else:style+=b'/RD [8 8 8 8] '+(b'/C [0] /IC [] ' if stroke else b'/C [] /IC [1 0 0] ')
    if stored is not None:style+=b'/AP << /N 6 0 R >> '
    values=[b'<< /Type /Catalog /Pages 2 0 R >>',b'<< /Type /Pages /Kids [3 0 R] /Count 1 >>',
            b'<< /Type /Page /Parent 2 0 R /MediaBox [0 0 32 32] /Resources << >> /Contents 4 0 R /Annots [5 0 R] '
            +f'/Rotate {rotate}'.encode()+b' >>',stream(b''),
            b'<< /Type /Annot /Subtype /'+shape.encode()+b' /Rect [0 0 32 32] '+style+extra+b' >>']
    if stored is not None:values.append(stream(stored,b'/Type /XObject /Subtype /Form /BBox [0 0 32 32] /Resources << >>'))
    return document(values)


def curves(shape='Square',intensity=1,width=0):
    # Independently specified knots, and each quadratic's control vertex.
    low,high=(8,24) if shape=='Polygon' else (8+width/2,24-width/2)
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


def arc_fraction(t):
    """Closed-form arclength divided by the lobe chord, starting at t=0."""
    u=2*t-1
    return (u*math.sqrt(1+4*u*u)+math.asinh(2*u)/2+math.sqrt(5)+math.asinh(2)/2)/4


def arc_parameter(distance):
    lo,hi=0.,1.
    for _ in range(56):
        middle=(lo+hi)/2
        if arc_fraction(middle)<distance:lo=middle
        else:hi=middle
    return (lo+hi)/2


@lru_cache(maxsize=12)
def stroke_contours(shape,width,dash):
    """True normal offsets, independent of cubic conversion or renderer flattening.

    A quadratic lobe has |q''|=4L, |q'|>=L and |normal''|<=96.
    Interpolation error is therefore <=(4L+96h)/(8N²). N is chosen so
    the full boundary-strip area bound is <1/8 channel at 144dpi. Dash
    cuts use the closed-form arclength, inverted by bounded bisection.
    """
    edges=curves(shape,width=width);half=width/2
    lengths=[math.dist(edge[0],edge[2])*arc_fraction(1) for edge in edges]
    total=sum(lengths);intervals=[]
    if dash:
        cursor=0.;index=0
        while cursor<total:
            stop=min(total,cursor+dash[index%len(dash)])
            if index%2==0:intervals.append((cursor,stop))
            cursor=stop;index+=1
        assert intervals[-1][1]<total, 'selected dash controls end in a gap at the closed seam'
    else:intervals=[(0.,total)]
    # Normal variation <=4 per lobe, joins use at most the PDF miter limit10.
    perimeter=2*(total+4*half*len(edges))+40*half*len(edges)+4*half*len(intervals)
    second=4*max(math.dist(e[0],e[2]) for e in edges)+96*half
    samples=1
    while second/(8*samples*samples)*perimeter*16*255>=.125:samples*=2
    def point(edge,t):
        a,c,b=edge;u=1-t
        x=u*u*a[0]+2*u*t*c[0]+t*t*b[0];y=u*u*a[1]+2*u*t*c[1]+t*t*b[1]
        dx=2*(u*(c[0]-a[0])+t*(b[0]-c[0]));dy=2*(u*(c[1]-a[1])+t*(b[1]-c[1]));length=math.hypot(dx,dy)
        return (x,y),(dy/length,-dx/length)
    def join(vertex,one,two,side):
        # Cross(normal_before, normal_after) has the tangent turn's sign.
        # Only the exterior offset receives a miter. The inner side returns
        # through the vertex; its overlap is removed by the nonzero union.
        if side*(one[0]*two[1]-one[1]*two[0])<=0:return vertex
        divisor=1+one[0]*two[0]+one[1]*two[1]
        assert divisor>0 and math.sqrt(2/divisor)<=10
        return vertex[0]+side*half*(one[0]+two[0])/divisor,vertex[1]+side*half*(one[1]+two[1])/divisor
    contours=[]
    for begin,end in intervals:
        positive=[];negative=[];cursor=0.;previous=None;first=None
        for edge,length in zip(edges,lengths):
            left,right=max(begin,cursor),min(end,cursor+length)
            if left<right:
                chord=math.dist(edge[0],edge[2])
                lo=0. if left==cursor else arc_parameter((left-cursor)/chord)
                hi=1. if right==cursor+length else arc_parameter((right-cursor)/chord)
                origin,normal=point(edge,lo)
                if first is None:first=(origin,normal)
                if previous is not None:
                    positive.append(join(origin,previous,normal,1));negative.append(join(origin,previous,normal,-1))
                count=max(1,math.ceil((hi-lo)*samples))
                for step in range(count+1):
                    vertex,normal=point(edge,lo+(hi-lo)*step/count)
                    positive.append((vertex[0]+half*normal[0],vertex[1]+half*normal[1]))
                    negative.append((vertex[0]-half*normal[0],vertex[1]-half*normal[1]))
                previous=normal
            cursor+=length
        if not dash:
            assert first is not None and previous is not None
            positive.append(join(first[0],previous,first[1],1));negative.append(join(first[0],previous,first[1],-1))
            contours.extend((positive,list(reversed(negative))))
        else:contours.append(positive+list(reversed(negative)))
    return contours


def polygon_coverage(contours,size):
    """Exact nonzero union of linear contours, including overlapping join edges.

    Sweep vertex/row/intersection heights. Between events each filled interval
    has linear boundaries, so integrating clamp(x - pixel_left, 0, 1) is exact.
    This is essential at concave joins; summed signed areas count overlap twice.
    """
    areas=[[0.]*(size+1) for _ in range(size)];tails=[[0.]*(size+1) for _ in range(size)]
    events={float(y):[] for y in range(size+1)};edges=[]
    for vertices in contours:
        for (ax,ay),(bx,by) in zip(vertices,vertices[1:]+vertices[:1]):
            low,high=max(0.,min(ay,by)),min(float(size),max(ay,by))
            if low>=high:continue
            index=len(edges);edges.append((ax,ay,(bx-ax)/(by-ay),1 if by>ay else -1))
            events.setdefault(low,[]).append((index,True));events.setdefault(high,[]).append((index,False))
    def at(edge,y):return edge[0]+(y-edge[1])*edge[2]
    def primitive(x):return 0. if x<=0 else x*x/2 if x<1 else x-.5
    def boundary(edge,sign,lo,hi):
        y=math.floor((lo+hi)/2);dy=(hi-lo)*sign;one,two=at(edge,lo),at(edge,hi)
        full=max(0,min(size,math.floor(min(one,two))))
        tails[y][0]+=dy;tails[y][full]-=dy
        for x in range(full,min(size,math.ceil(max(one,two)))):
            areas[y][x]+=dy*(max(0.,min(1.,one-x)) if abs(one-two)<1e-10 else (primitive(two-x)-primitive(one-x))/(two-one))
    active=set();heights=sorted(events)
    for lo,hi in zip(heights,heights[1:]):
        for index,add in events[lo]:
            if add:active.add(index)
            else:active.remove(index)
        if not active:continue
        order=sorted(active,key=lambda index:at(edges[index],(lo+hi)/2));cuts={lo,hi}
        if sorted(order,key=lambda index:at(edges[index],lo))!=sorted(order,key=lambda index:at(edges[index],hi)):
            for offset,index in enumerate(order):
                one=edges[index]
                for other in order[offset+1:]:
                    two=edges[other];divisor=one[2]-two[2]
                    if divisor:
                        crossing=lo+(at(two,lo)-at(one,lo))/divisor
                        if lo<crossing<hi:cuts.add(crossing)
        cuts=sorted(cuts)
        for bottom,top in zip(cuts,cuts[1:]):
            winding=0
            for index in sorted(order,key=lambda index:at(edges[index],(bottom+top)/2)):
                edge=edges[index];before=winding;winding+=edge[3]
                if before==0:boundary(edge,-1,bottom,top)
                elif winding==0:boundary(edge,1,bottom,top)
            assert winding==0
    output=[]
    for row,tail in zip(areas,tails):
        running=0.
        for value,change in zip(row[:size],tail[:size]):
            running+=change;area=value+running
            assert -1e-6<=area<=1+1e-6
            output.append(min(1.,max(0.,area)))
    return output


def stroke_oracle(shape='Square',*,width=1.,dash=(),dpi=72,rotate=0,alpha=1.):
    scale=dpi/72;size=int(32*scale)
    def point(p):
        x,y=p
        return (y*scale,(32-x)*scale) if rotate else (x*scale,y*scale)
    contours=[[point(p) for p in contour] for contour in stroke_contours(shape,width,tuple(dash))]
    areas=polygon_coverage(contours,size);pixels=bytearray()
    for y in reversed(range(size)):
        for x in range(size):
            coverage=int(areas[y*size+x]*255+.5)/255
            pixels.extend([int(255*(1-coverage*alpha)+.5)]*3)
    return f'P6\n{size} {size}\n255\n'.encode()+pixels


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
    for shape in ('Square','Circle','Polygon','FreeText'):
        for dpi in (72,144):
            yield Case(f'{shape}-stroke-{dpi}',source(shape,width=1,stroke=True),RENDER,(REQUIREMENT,),
                       'Independent normal offsets and exact nonzero polygon coverage establish the cloud stroke and miter joins',
                       oracle=stroke_oracle('Square' if shape=='FreeText' else shape,dpi=dpi),dpi=dpi,extra={'tolerance':1})
    for width in (1,2):
        for dpi in (72,144):
            options={'width':width,'dash':(3,3)}
            yield Case(f'dashed-{width}-{dpi}',source(stroke=True,**options),RENDER,(REQUIREMENT,),
                       'Closed-form lobe arclength positions dash cuts; true normal offsets and butt caps establish coverage',
                       oracle=stroke_oracle(dpi=dpi,**options),dpi=dpi,extra={'tolerance':1})
    for name,options in [('stroke-opacity',{'alpha':.5}),('stroke-rotated',{'rotate':90})]:
        yield Case(name,source(width=1,stroke=True,**options),RENDER,(REQUIREMENT,),
                   'Independent stroke coverage followed by rotation or source-over opacity establishes pixels',
                   oracle=stroke_oracle(**options),extra={'tolerance':1})
    for name,effect in [('bad-intensity',b'<< /S /C /I /Bad >>'),('negative',b'<< /S /C /I -1 >>'),
                       ('excess',b'<< /S /C /I 3 >>'),('bad-effect',b'42')]:
        yield Case(name,source(effect=effect),OPEN,(REQUIREMENT,),
                   'Malformed cloud declarations fail before no-mark shortcuts',extra={'render_error':'syntax'})
