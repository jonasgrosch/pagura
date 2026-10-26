"""Leader lines, ISO 32000-2 Table 178/Figure 80.

Expected regions are specified directly as rectangles and their exact union.
The fixture author does not derive regions by calling the product's layout.
"""
from .ink_annotations import oracle
from .line_annotations import source
from .model import Case, OPEN, RENDER

AREA = 'leader_annotations'
REQUIREMENT = 'content-line-leaders'
POSITIVE = [(8,13.5,24,14.5),(7.5,10,8.5,15),(23.5,10,24.5,15)]
NEGATIVE = [(8,17.5,24,18.5),(7.5,17,8.5,22),(23.5,17,24.5,22)]
VERTICAL = [(17.5,8,18.5,24),(17,7.5,22,8.5),(17,23.5,22,24.5)]


def over_black(base, foreground):
    header, bg = base.split(b'\n255\n',1)
    _, fg = foreground.split(b'\n255\n',1)
    return header+b'\n255\n'+bytes(int(b*f/255+.5) for b,f in zip(bg,fg))


def cases():
    def rendered(name, line=b'[8 8 24 8]', *, entries=b'/LL 4 /LLO 2 /LLE 1', regions=POSITIVE,
                 dpi=72, rotate=0, width=1., color=(0.,0.,0.), alpha=1., rect=(0,0,32,32), objects=(), dash=None):
        return Case(name,source(line,width=width,color=color,alpha=alpha,rotate=rotate,rect=rect,
                                extra=b' '+entries,objects=objects,dash=dash),RENDER,(REQUIREMENT,),
                    'Independent literal rectangle unions verify leader sign, offset, extension and shared stroke opacity',
                    dpi=dpi,oracle=oracle(regions,dpi=dpi,rotate=rotate,color=color or (0.,0.,0.),alpha=alpha,rect=rect),
                    extra={'tolerance':1})
    for dpi in (72,144):
        yield rendered(f'positive-{dpi}',dpi=dpi)
        yield rendered(f'negative-{dpi}',b'[8 24 24 24]',entries=b'/LL -4 /LLO 2 /LLE 1',regions=NEGATIVE,dpi=dpi)
        yield rendered(f'vertical-{dpi}',b'[24 8 24 24]',regions=VERTICAL,dpi=dpi)
    yield rendered('reversed',b'[24 24 8 24]',regions=NEGATIVE)
    yield rendered('rotated',rotate=90,dpi=144)
    yield rendered('opacity',color=(0.,0.,1.),alpha=.5)
    yield rendered('coincident',b'[16 8 16 8]',regions=[(15.5,10,16.5,15)],alpha=.5)
    yield rendered('clipped',rect=(8,11,24,16))
    yield rendered('zero-width',width=0,regions=[])
    yield rendered('no-color',color=None,regions=[])
    yield rendered('offset-absent',entries=b'/LL 4 /LLE 1',regions=[(8,11.5,24,12.5),(7.5,8,8.5,13),(23.5,8,24.5,13)])
    yield rendered('extension-absent',entries=b'/LL 4 /LLO 2',regions=[(8,13.5,24,14.5),(7.5,10,8.5,14),(23.5,10,24.5,14)])
    yield rendered('only-extension',entries=b'/LL 0 /LLE 2',regions=[(8,7.5,24,8.5),(7.5,8,8.5,10),(23.5,8,24.5,10)])
    yield rendered('only-offset',entries=b'/LL 0 /LLO 2',regions=[(8,9.5,24,10.5)])
    yield rendered('offset-without-leader',entries=b'/LLO 2',regions=[(8,7.5,24,8.5)])
    yield rendered('indirect',entries=b'/LL 6 0 R /LLO 7 0 R /LLE 8 0 R',objects=(b'4',b'2',b'1'))
    dashed = [(x,13.5,min(24,x+2),14.5) for x in range(8,24,3)]
    dashed += [(x-.5,y,x+.5,y+2) for x in (8,24) for y in (10,13)]
    yield rendered('dashes',dash=[2,1],regions=dashed)
    yield Case('butt-endings',source(b'[8 8 24 8]',width=1.,extra=b'/LL 4 /LLO 2 /LLE 1 /LE [/Butt /Butt]'),RENDER,
               (REQUIREMENT,'content-line-endings'),'Endings belong to the translated line proper',
               oracle=over_black(oracle(POSITIVE),oracle([(7.5,11,8.5,17),(23.5,11,24.5,17)])),extra={'tolerance':1})
    for name,entries in [('length-type',b'/LL /Bad'),('extension-type',b'/LL 4 /LLE /Bad'),
                          ('offset-type',b'/LL 4 /LLO /Bad'),('negative-extension',b'/LL 4 /LLE -1'),
                          ('negative-offset',b'/LL 4 /LLO -1'),('missing-length',b'/LLE 2')]:
        yield Case(name,source(width=0,extra=entries),OPEN,(REQUIREMENT,),
                   'Leader fields validate before empty output',extra={'render_error':'syntax'})
