"""Table 184 stamp names with independent AFM layout and quadratic glyph areas.

The font oracle integrates original TrueType quadratics analytically by Green's
theorem. Stored controls use four filled border regions instead of a stroke.
No product parser, renderer, path flattener or competitor supplies pixels.
"""
from pathlib import Path

from .font_pixels import glyph_segments, signed_area
from .model import Case, OPEN, RENDER, UNSUPPORTED
from .pdf import document, stream

AREA='stamp_annotations'
REQUIREMENT='content-stamp-appearances'
ROOT=Path(__file__).resolve().parents[2]
LABELS={'Approved':'APPROVED','Experimental':'EXPERIMENTAL','NotApproved':'NOT APPROVED','AsIs':'AS IS',
        'Expired':'EXPIRED','NotForPublicRelease':'NOT FOR PUBLIC RELEASE','Confidential':'CONFIDENTIAL',
        'Final':'FINAL','Sold':'SOLD','Departmental':'DEPARTMENTAL','ForComment':'FOR COMMENT',
        'TopSecret':'TOP SECRET','Draft':'DRAFT','ForPublicRelease':'FOR PUBLIC RELEASE'}


def metrics():
    widths={};vertical={}
    for line in (ROOT/'data/afm/Helvetica-Bold.afm').read_text().splitlines():
        if line.startswith('C '):
            fields=dict(part.strip().split(' ',1) for part in line.split(';') if part.strip())
            code=int(fields['C'])
            if 0<=code<128:
                widths[chr(code)]=float(fields['WX'])/1000
        elif line.startswith(('Ascender ','Descender ')):
            name,value=line.split();vertical[name]=float(value)/1000
    return widths,vertical['Ascender'],vertical['Descender']


def layout(label, rect):
    widths,ascent,descent=metrics()
    x0,y0,x1,y1=rect;width=x1-x0;height=y1-y0;short=min(width,height)
    advance=sum(widths[c] for c in label)
    size=min((width-short/4)/advance,(height-short/4)/(ascent-descent))
    return widths,size,(x0+x1-size*advance)/2,(y0+y1-size*(ascent+descent))/2,short/32


def source(name: bytes | None=b'/Draft', *, rect=(4,8,124,40), color=None, alpha=1., extra=b'', rotate=0, flags=0,
           objects=(), stored=None):
    box=' '.join(str(v) for v in rect).encode()
    annotation=b'<< /Type /Annot /Subtype /Stamp /Rect ['+box+f'] /CA {alpha} /F {flags} '.encode()+extra
    if name is not None:
        annotation+=b' /Name '+name
    if color is not None:
        annotation+=b' /C ['+b' '.join(str(v).encode() for v in color)+b']'
    if stored is not None:
        annotation+=b' /AP << /N 6 0 R >>'
    values=[b'<< /Type /Catalog /Pages 2 0 R >>',b'<< /Type /Pages /Kids [3 0 R] /Count 1 >>',
            b'<< /Type /Page /Parent 2 0 R /MediaBox [0 0 128 48] /Resources << >> /Contents 4 0 R /Annots [5 0 R] '
            +f'/Rotate {rotate}'.encode()+b' >>',stream(b''),annotation+b' >>']
    if stored is not None:
        _,size,x,y,w=layout(stored,rect)
        x0,y0,x1,y1=rect
        # Four independent filled regions form the border; no stroke operator.
        frame=f'{x0} {y0} {x1-x0} {w} re {x0} {y1-w} {x1-x0} {w} re '
        frame+=f'{x0} {y0+w} {w} {y1-y0-2*w} re {x1-w} {y0+w} {w} {y1-y0-2*w} re f '
        content=f'1 0 0 rg {frame} BT /Text {size} Tf {x} {y} Td ({stored}) Tj ET'.encode()
        values.append(stream(content,b'/Type /XObject /Subtype /Form /BBox ['+box+b'] /Resources '
            b'<< /Font << /Text << /Type /Font /Subtype /Type1 /BaseFont /Helvetica-Bold /Encoding /WinAnsiEncoding >> >> >>'))
    return document(values+list(objects))


def oracle(label, *, rect=(4,8,124,40), color=(1.,0.,0.), alpha=1., text_alpha=None,dpi=72,rotate=0,visible=True):
    scale=dpi/72
    def point(p):
        x,y=p
        return (y*scale,(128-x)*scale) if rotate else (x*scale,y*scale)
    def box(bounds):
        a,b=point(bounds[:2]),point(bounds[2:])
        return min(a[0],b[0]),min(a[1],b[1]),max(a[0],b[0]),max(a[1],b[1])
    widths,size,x,y,w=layout(label,rect)
    glyphs=[]
    if visible:
        for character in label:
            if character!=' ':
                edges=[tuple(point(p) for p in edge) for edge in glyph_segments(ROOT/'data/fonts/Arimo-Bold.ttf',character,size,(x,y))]
                points=[p for edge in edges for p in edge]
                bbox=(min(p[0] for p in points),min(p[1] for p in points),max(p[0] for p in points),max(p[1] for p in points))
                glyphs.append((bbox,edges))
            x+=widths[character]*size
    outer=box(rect);inner=box((rect[0]+w,rect[1]+w,rect[2]-w,rect[3]-w))
    width,height=(48,128) if rotate else (128,48)
    width,height=int(width*scale),int(height*scale)
    def area(bounds,x,y):
        return max(0.,min(bounds[2],x+1)-max(bounds[0],x))*max(0.,min(bounds[3],y+1)-max(bounds[1],y))
    ta=alpha if text_alpha is None else text_alpha;pixels=bytearray()
    for yy in reversed(range(height)):
        for xx in range(width):
            frame=area(outer,xx,yy)-area(inner,xx,yy) if visible else 0.
            bounds=(max(xx,outer[0]),max(yy,outer[1]),min(xx+1,outer[2]),min(yy+1,outer[3]))
            covered=0.
            if bounds[0]<bounds[2] and bounds[1]<bounds[3]:
                covered=abs(sum(signed_area(edges,bounds) for bbox,edges in glyphs
                            if bbox[0]<xx+1 and bbox[2]>xx and bbox[1]<yy+1 and bbox[3]>yy))
            fg=int(max(0.,min(1.,covered))*255+.5)/255*ta
            bg=int(max(0.,min(1.,frame))*255+.5)/255*alpha
            for c in color:
                base=int(255*(1-bg+c*bg)+.5)
                pixels.append(int(base*(1-fg)+255*c*fg+.5))
    return f'P6\n{width} {height}\n255\n'.encode()+pixels


def cases():
    def rendered(case,name: bytes | None=b'/Draft',*,label='DRAFT',rect=(4,8,124,40),color=None,alpha=1.,text_alpha=None,
                 dpi=72,rotate=0,visible=True,**options):
        return Case(case,source(name,rect=rect,color=color,alpha=alpha,rotate=rotate,**options),RENDER,(REQUIREMENT,),
                    'AFM metrics, analytic original quadratic integrals and literal frame regions independently establish pixels',
                    dpi=dpi,information=('font.substituted',) if visible else (),
                    oracle=oracle(label,rect=rect,color=color or (1.,0.,0.),alpha=alpha,text_alpha=text_alpha,dpi=dpi,
                                  rotate=rotate,visible=visible),extra={'tolerance':1})
    for name,label in LABELS.items():
        yield rendered(name,name=('/'+name).encode(),label=label)
    for name in ('Final','Draft','NotForPublicRelease'):
        yield rendered(name+'-144',name=('/'+name).encode(),label=LABELS[name],dpi=144)
    for dpi in (72,144):
        yield rendered(f'final-stored-{dpi}',name=b'/Final',label='FINAL',stored='FINAL',dpi=dpi)
    yield rendered('default',name=None)
    yield rendered('null-name',name=b'null')
    yield rendered('unknown-name',name=b'/Custom')
    yield rendered('null-intent',extra=b'/IT null')
    yield rendered('indirect',name=b'6 0 R',extra=b'/IT 7 0 R',objects=(b'/Final',b'/Stamp'),label='FINAL')
    yield rendered('color',color=(0.,0.,1.))
    yield rendered('opacity',alpha=.5)
    yield rendered('separate-opacity',alpha=.75,text_alpha=.25,extra=b'/ca 0.25')
    yield rendered('rotated',rotate=90,dpi=144)
    yield rendered('tall',name=b'/Final',label='FINAL',rect=(48,4,80,44))
    yield rendered('hidden',flags=2,visible=False)
    yield rendered('no-view',flags=32,visible=False)
    yield rendered('no-color',color=(),visible=False)
    yield rendered('stored-ignores-fields',name=b'42',extra=b'/IT /StampImage /C 42',label='FINAL',stored='FINAL')
    for name,extra in [('name-type',b'/Name 42'),('intent-type',b'/IT 42')]:
        yield Case(name,source(name=None,color=(),extra=extra),OPEN,(REQUIREMENT,),
                   'Name/IT types validate before empty output',extra={'render_error':'syntax'})
    for intent in ('StampImage','StampSnapshot','Unknown'):
        yield Case('intent-'+intent,source(extra=b'/IT /'+intent.encode()),UNSUPPORTED,(REQUIREMENT,),
                   'Image, snapshot and unrecognized intents require stored artwork',feature='annotation.appearance')
