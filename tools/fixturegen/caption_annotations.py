"""Plain Line captions with independent AFM and straight TrueType glyph areas.

Each fixture supplies its caption baseline and line regions explicitly. Expected
pixels use exact rational clipping of the pinned Arimo glyph contours; neither
product layout nor an engine generates the expectation.
"""
from fractions import Fraction as F
from pathlib import Path

from .ink_annotations import rectangle_union
from .line_annotations import source
from .model import Case, OPEN, RENDER, UNSUPPORTED
from .paint import bundled_contours
from .raster import clipped_area

AREA = 'caption_annotations'
REQUIREMENT = 'content-line-captions'
ROOT = Path(__file__).resolve().parents[2]
# Helvetica AFM: I advance 278, ascent 718, descent -207. At 12 pt,
# a centred I starts at 14.332 with baseline 12.934; clearance is 2.
INLINE = [(4,15.5,12.332,16.5),(19.668,15.5,28,16.5)]
FULL = [(4,15.5,28,16.5)]


def oracle(regions=INLINE, *, rows=(('I',14.332,12.934),), dpi=72, rotate=0,
           color=(0.,0.,0.), alpha=1., text_alpha=None, rect=(0,0,32,32), axis=(1,0)):
    scale = F(dpi,72)
    def point(x,y):
        return (y*scale,(32-x)*scale) if rotate else (x*scale,y*scale)
    def box(bounds):
        a,b = point(*map(lambda v:F(str(v)),bounds[:2])),point(*map(lambda v:F(str(v)),bounds[2:]))
        return min(a[0],b[0]),min(a[1],b[1]),max(a[0],b[0]),max(a[1],b[1])
    clip=box(rect)
    rectangles=[]
    for bounds in regions:
        a,b,c,d=box(bounds)
        rectangles.append((max(a,clip[0]),max(b,clip[1]),min(c,clip[2]),min(d,clip[3])))
    polygons=[]
    for text,x,y in rows:
        x,y=F(str(x)),F(str(y))
        for character in text:
            if character != ' ':
                contours,upem=bundled_contours(ROOT/'data/fonts/Arimo-Regular.ttf',character)
                for contour in contours:
                    polygons.append([point(x+(axis[0]*px-axis[1]*py)*F(12,upem),
                                           y+(axis[1]*px+axis[0]*py)*F(12,upem)) for px,py in contour])
            assert character in 'I ', 'fixture uses independently specified 278-unit advances'
            x+=axis[0]*F(3336,1000);y+=axis[1]*F(3336,1000)
    size=int(32*scale);pixels=bytearray()
    ta=alpha if text_alpha is None else text_alpha
    for y in reversed(range(size)):
        for x in range(size):
            body=rectangle_union(rectangles,x,y)
            glyph=sum(abs(clipped_area(p,x,y)) for p in polygons) if clip[0]<=x and x+1<=clip[2] and clip[1]<=y and y+1<=clip[3] else 0
            body=int(min(1,body)*255+F(1,2))/255*alpha
            glyph=int(min(1,glyph)*255+F(1,2))/255*ta
            for c in color:
                bg=int(255*(1-body+c*body)+.5)
                pixels.append(int(bg*(1-glyph)+255*c*glyph+.5))
    return f'P6\n{size} {size}\n255\n'.encode()+pixels


def cases():
    def rendered(name,contents=b'(I)',line=b'[4 16 28 16]',*,extra=b'',width=1.,regions=INLINE,
                 rows=(('I',14.332,12.934),),dpi=72,rotate=0,color=(0.,0.,0.),alpha=1.,rect=(0,0,32,32),
                 text_alpha=None,axis=(1,0),objects=()):
        return Case(name,source(line,width=width,color=color,alpha=alpha,rect=rect,rotate=rotate,objects=objects,
                                extra=b'/Cap true /Contents '+contents+b' '+extra),RENDER,(REQUIREMENT,),
                    'Literal line regions and independently decoded pinned glyph polygons verify caption placement',
                    dpi=dpi,oracle=oracle(regions,rows=rows,dpi=dpi,rotate=rotate,color=color,alpha=alpha,
                                          text_alpha=text_alpha,rect=rect,axis=axis),
                    information=('font.substituted',) if any(text for text,_,_ in rows) else (),extra={'tolerance':1})
    for dpi in (72,144):
        yield rendered(f'inline-{dpi}',dpi=dpi)
        yield rendered(f'top-{dpi}',extra=b'/CP /Top',regions=FULL,rows=(('I',14.332,20.484),),dpi=dpi)
        yield rendered(f'multiline-{dpi}',contents=b'(I\rI)',rows=(('I',14.332,20.134),('I',14.332,5.734)),dpi=dpi)
    yield rendered('offset',extra=b'/CO [4 0]',regions=[(4,15.5,16.332,16.5),(23.668,15.5,28,16.5)],rows=(('I',18.332,12.934),))
    yield rendered('above',extra=b'/CO [0 8]',regions=FULL,rows=(('I',14.332,20.934),))
    yield rendered('wide',contents=b'(IIIIIIII)',regions=FULL,rows=(('IIIIIIII',2.656,20.484),))
    yield rendered('zero-width',width=0,regions=[])
    yield rendered('opacity',alpha=.5,color=(0.,0.,1.))
    yield rendered('separate-opacity',alpha=.5,extra=b'/ca 0.25',text_alpha=.25)
    yield rendered('clipped',rect=(8,12,24,20))
    yield rendered('rotated',rotate=90,dpi=144)
    # The specified text follows the source line direction, including reversal.
    yield rendered('reversed',line=b'[28 16 4 16]',rows=(('I',17.668,19.066),),axis=(-1,0))
    yield rendered('vertical',line=b'[16 4 16 28]',regions=[(15.5,4,16.5,12.332),(15.5,19.668,16.5,28)],
                   rows=(('I',19.066,14.332),),axis=(0,1))
    yield rendered('leaders',line=b'[4 10 28 10]',extra=b'/LL 4 /LLO 2 /LLE 1',
                   regions=INLINE+[(3.5,12,4.5,17),(27.5,12,28.5,17)])
    yield rendered('indirect',contents=b'6 0 R',extra=b'/CP 7 0 R /CO 8 0 R',objects=(b'(I)',b'/Inline',b'[0 0]'))
    yield rendered('utf16',contents=b'<FEFF0049>')
    yield rendered('utf8',contents=b'<EFBBBF49>')
    yield rendered('crlf',contents=b'(I\r\nI)',rows=(('I',14.332,20.134),('I',14.332,5.734)))
    yield rendered('leading-empty-line',contents=b'(\nI)',rows=(('I',14.332,5.734),))
    yield rendered('trailing-empty-line',contents=b'(I\n)',rows=(('I',14.332,20.134),))
    yield rendered('empty',contents=b'()',rows=(),regions=FULL)
    for name,extra in [('position-type',b'/CP 42'),('offset-type',b'/CO 42'),('offset-count',b'/CO [0]'),
                       ('offset-number',b'/CO [0 /Bad]')]:
        yield Case(name,source(extra=b'/Cap true '+extra),OPEN,(REQUIREMENT,),
                   'Caption-only parameters validate even when the caption is empty',extra={'render_error':'syntax'})
    yield Case('contents-type',source(extra=b'/Cap true /Contents 42'),OPEN,(REQUIREMENT,),
               'Plain caption Contents must be a PDF text string',extra={'render_error':'syntax'})
    yield Case('rich-text',source(extra=b'/Cap true /RC (<p>I</p>)'),RENDER,(REQUIREMENT,),
               'Rich caption uses the independently specified Helvetica baseline and literal line gap',
               oracle=oracle([(4,7,12.332,9),(19.668,7,28,9)],rows=(('I',14.332,4.934),)),
               information=('font.substituted',),extra={'tolerance':1})
    for name,extra,feature in [('position-unknown',b'/CP /Other','annotation.appearance'),
                              ('unmapped',b'/Contents <FEFF4E00>','appearance.font-encoding')]:
        yield Case(name,source(extra=b'/Cap true '+extra),UNSUPPORTED,(REQUIREMENT,),
                   'Unimplemented rich text, positions and font encodings remain explicit',feature=feature,
                   information=('font.substituted',) if name=='unmapped' else ())
