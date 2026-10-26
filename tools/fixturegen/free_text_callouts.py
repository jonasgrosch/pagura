"""FreeText callouts: literal stroke regions and independent ending/glyph areas."""
import math

from .circle_annotations import ellipse_area
from .ending_annotations import STYLES, regions
from .free_text_annotations import oracle as text_oracle, source
from .ink_annotations import polygon_area
from .model import Case, OPEN, RENDER, UNSUPPORTED

AREA='free_text_callouts'
REQUIREMENT='content-free-text-callouts'
RECT=(4,4,124,44)
RD=(40,0,0,20)
# Authored unit-width miter/butt stroke boundaries, not a stroker.
BODIES={
    'elbow':[(12,11.5),(48.5,11.5),(48.5,32),(47.5,32),(47.5,12.5),(12,12.5)],
    'straight':[(12,31.5),(44,31.5),(44,32.5),(12,32.5)],
    'vertical':[(11.5,12),(12.5,12),(12.5,31.5),(48,31.5),(48,32.5),(11.5,32.5)],
}
POINTS={'elbow':[(12,12),(48,12),(48,32)],'straight':[(12,32),(44,32)],
        'vertical':[(12,12),(12,32),(48,32)]}


def oracle(style='None', *, body='elbow',dpi=72,rotate=0,alpha=1.,fill_alpha=1.,background=(1.,1.,0.),
           width=1.,rows=('HI',),visible=True):
    scale=dpi/72
    def point(p):
        x,y=p
        return (y*scale,(128-x)*scale) if rotate else (x*scale,y*scale)
    body_region=[point(p) for p in BODIES[body]]
    start=POINTS[body][0];adjacent=POINTS[body][1]
    dx,dy=start[0]-adjacent[0],start[1]-adjacent[1];length=math.hypot(dx,dy);a,b=dx/length,dy/length
    fill,outer,inner,radius=regions(style,width)
    def mapped(points):
        return [point((start[0]+a*x-b*y,start[1]+b*x+a*y)) for x,y in points]
    fill,outer,inner=mapped(fill),mapped(outer),mapped(inner);cx,cy=point(start)
    corner_a,corner_b=point(RECT[:2]),point(RECT[2:]);clip=(min(corner_a[0],corner_b[0]),min(corner_a[1],corner_b[1]),max(corner_a[0],corner_b[0]),max(corner_a[1],corner_b[1]))
    def underlay(x,y):
        if not visible or not(clip[0]<=x and x+1<=clip[2] and clip[1]<=y and y+1<=clip[3]):
            return 255,255,255
        if radius is None:
            filled=polygon_area(fill,x,y);stroked=polygon_area(outer,x,y)-polygon_area(inner,x,y)
        else:
            def disk(r):
                return ellipse_area((cx-r*scale,cy-r*scale,cx+r*scale,cy+r*scale),x,y)
            filled=disk(radius);stroked=disk(radius+width/2)-disk(radius-width/2)
        values=[255.,255.,255.]
        for amount,paint,opacity in [(polygon_area(body_region,x,y) if width else 0.,(0.,0.,0.),alpha),
                                    (filled,background,fill_alpha),(stroked if width else 0.,(0.,0.,0.),alpha)]:
            if paint is not None:
                mask=int(max(0.,min(1.,amount))*255+.5)/255*opacity
                values=[int(v*(1-mask)+255*c*mask+.5) for v,c in zip(values,paint)]
        return values
    return text_oracle(rows,rect=RECT,rd=RD,border=width,background=background,alpha=alpha,text_alpha=fill_alpha,
                       dpi=dpi,rotate=rotate,visible=visible,underlay=underlay)


def cases():
    def rendered(name,*,style='None',body='elbow',dpi=72,rotate=0,alpha=1.,fill_alpha=1.,background=(1.,1.,0.),
                 width=1.,rows=('HI',),contents=b'(HI)',visible=True,callout=None,ending=None,extra=b'',objects=(),flags=0):
        cl=callout if callout is not None else b'['+b' '.join(str(v).encode() for p in POINTS[body] for v in p)+b']'
        le=ending if ending is not None else ('/'+style).encode()
        entries=b'/IT /FreeTextCallout /RD [40 0 0 20] /CL '+cl+b' /LE '+le+f' /ca {fill_alpha} '.encode()+extra
        return Case(name,source(contents,rect=RECT,border=width,background=background,alpha=alpha,extra=entries,
                                objects=objects,rotate=rotate,flags=flags),RENDER,(REQUIREMENT,),
                    'Literal callout stroke polygons, analytic ending regions and independently placed glyph integrals establish pixels',
                    dpi=dpi,information=('font.substituted',) if visible and any(rows) else (),
                    oracle=oracle(style,body=body,dpi=dpi,rotate=rotate,alpha=alpha,fill_alpha=fill_alpha,
                                  background=background,width=width,rows=rows,visible=visible),extra={'tolerance':1})
    for style in ('None',)+STYLES:
        yield rendered(style,style=style)
    for style in ('OpenArrow','ClosedArrow','Circle'):
        yield rendered(style+'-144',style=style,dpi=144)
    yield rendered('null-ending',ending=b'null')
    yield rendered('straight',body='straight',style='ClosedArrow')
    yield rendered('vertical',body='vertical',style='OpenArrow')
    yield rendered('rotated',style='ClosedArrow',rotate=90,dpi=144)
    yield rendered('opacity',style='Square',alpha=.5,fill_alpha=.5)
    yield rendered('separate-opacity',style='ClosedArrow',alpha=.75,fill_alpha=.25)
    yield rendered('transparent-background',style='ClosedArrow',background=None)
    yield rendered('zero-width-filled-ending',style='Square',width=0.)
    yield rendered('no-text',style='Circle',rows=(),contents=b'()')
    yield rendered('hidden',style='OpenArrow',flags=2,visible=False)
    yield rendered('indirect',style='ClosedArrow',callout=b'6 0 R',ending=b'7 0 R',
                   objects=(b'[12 12 48 12 48 32]',b'/ClosedArrow'))
    for name,entries in [('cl-type',b'/CL 42'),('cl-short',b'/CL [12 12]'),('cl-odd',b'/CL [12 12 48 12 48]'),
                         ('cl-long',b'/CL [12 12 48 12 48 32 50 32]'),('cl-value',b'/CL [12 12 /X 12]'),
                         ('le-type',b'/CL [12 12 48 12] /LE [/None /None]')]:
        yield Case(name,source(b'()',extra=b'/IT /FreeTextCallout '+entries),OPEN,(REQUIREMENT,),
                   'Cardinality and operand types validate before zero-width or empty-text suppression',extra={'render_error':'syntax'})
    yield Case('unknown-ending',source(extra=b'/IT /FreeTextCallout /CL [12 12 48 12] /LE /Unknown'),
               UNSUPPORTED,(REQUIREMENT,),'Unknown endings retain an explicit capability refusal',feature='annotation.appearance')
