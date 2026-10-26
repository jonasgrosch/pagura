"""Plain FreeText: authored line placement and analytic original glyph areas.

Expected line breaks and sizes are supplied per case, independently of the
product's greedy wrapper. Pixel areas integrate original font quadratics; border
regions, clipping, background and alpha are independently composed.
"""
from pathlib import Path

from .font_pixels import glyph_segments, signed_area
from .model import Case, OPEN, RENDER, UNSUPPORTED
from .pdf import document, stream

AREA='free_text_annotations'
REQUIREMENT='content-free-text-appearances'
ROOT=Path(__file__).resolve().parents[2]


def metrics(bold=False):
    widths={};vertical={}
    for line in (ROOT/('data/afm/Helvetica-Bold.afm' if bold else 'data/afm/Helvetica.afm')).read_text().splitlines():
        if line.startswith('C '):
            fields=dict(part.strip().split(' ',1) for part in line.split(';') if part.strip())
            code=int(fields['C'])
            if 0<=code<128:
                widths[chr(code)]=float(fields['WX'])/1000
        elif line.startswith(('Ascender ','Descender ')):
            name,value=line.split();vertical[name]=float(value)/1000
    return widths,vertical['Ascender'],vertical['Descender']


def source(contents: bytes | None=b'(HI)', *, da: bytes | None=b'(/Helvetica 12 Tf 0 g)',rect=(4,4,124,44),q: int | str=0,border=0,
           background=None,alpha=1.,extra=b'',objects=(),catalog=b'',rotate=0,flags=0,stored=False):
    box=' '.join(str(v) for v in rect).encode()
    annotation=b'<< /Type /Annot /Subtype /FreeText /Rect ['+box+b']'
    if contents is not None:
        annotation+=b' /Contents '+contents
    if da is not None:
        annotation+=b' /DA '+da
    if background is not None:
        annotation+=b' /C ['+b' '.join(str(v).encode() for v in background)+b']'
    annotation+=f' /Q {q} /BS << /W {border} >> /CA {alpha} /F {flags} '.encode()+extra
    if stored:
        annotation+=b' /AP << /N 6 0 R >>'
    values=[b'<< /Type /Catalog /Pages 2 0 R '+catalog+b' >>',b'<< /Type /Pages /Kids [3 0 R] /Count 1 >>',
            b'<< /Type /Page /Parent 2 0 R /MediaBox [0 0 128 48] /Resources << >> /Contents 4 0 R /Annots [5 0 R] '
            +f'/Rotate {rotate}'.encode()+b' >>',stream(b''),annotation+b' >>']
    if stored:
        values.append(stream(b'BT /Text 12 Tf 6 33.384 Td (HI) Tj ET',
            b'/Type /XObject /Subtype /Form /BBox [4 4 124 44] /Resources << /Font << /Text '
            b'<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >> >> >>'))
    return document(values+list(objects))


def oracle(rows: tuple[str,...]=('HI',), *, size=12.,rect=(4,4,124,44),rd=(0,0,0,0),q=0,border=0.,background=None,
           color=(0.,0.,0.),alpha=1.,text_alpha=None,dpi=72,rotate=0,visible=True,bold=False,leading=None):
    widths,ascent,_=metrics(bold);scale=dpi/72
    x0,y0,x1,y1=rect;left,top,right,bottom=rd
    bounds=(x0+left,y0+bottom,x1-right,y1-top)
    padding=border+2;area=(bounds[0]+padding,bounds[1]+padding,bounds[2]-padding,bounds[3]-padding)
    leading=1.2*size if leading is None else leading
    def point(p):
        x,y=p
        return (y*scale,(128-x)*scale) if rotate else (x*scale,y*scale)
    def box(r):
        a,b=point(r[:2]),point(r[2:])
        return min(a[0],b[0]),min(a[1],b[1]),max(a[0],b[0]),max(a[1],b[1])
    glyphs=[]
    for row,text in enumerate(rows if visible else ()):
        advance=sum(widths[c] for c in text)*size
        x=area[0]+(area[2]-area[0]-advance)*(q/2)
        y=area[3]-ascent*size-row*leading
        for character in text:
            if character!=' ':
                path=ROOT/('data/fonts/Arimo-Bold.ttf' if bold else 'data/fonts/Arimo-Regular.ttf')
                edges=[tuple(point(p) for p in edge) for edge in glyph_segments(path,character,size,(x,y))]
                points=[p for edge in edges for p in edge]
                bbox=(min(p[0] for p in points),min(p[1] for p in points),max(p[0] for p in points),max(p[1] for p in points))
                glyphs.append((bbox,edges))
            x+=widths[character]*size
    outer=box(bounds);inner=box((bounds[0]+border,bounds[1]+border,bounds[2]-border,bounds[3]-border));clip=box(area)
    width,height=(48,128) if rotate else (128,48);width,height=int(width*scale),int(height*scale)
    def coverage(r,x,y):
        return max(0.,min(r[2],x+1)-max(r[0],x))*max(0.,min(r[3],y+1)-max(r[1],y))
    def quantized(value):
        return int(max(0.,min(1.,value))*255+.5)/255
    ta=alpha if text_alpha is None else text_alpha;pixels=bytearray()
    for yy in reversed(range(height)):
        for xx in range(width):
            bg=quantized(coverage(outer,xx,yy))*ta if visible and background else 0.
            frame=quantized(coverage(outer,xx,yy)-coverage(inner,xx,yy))*alpha if visible and border else 0.
            region=(max(xx,clip[0]),max(yy,clip[1]),min(xx+1,clip[2]),min(yy+1,clip[3]))
            covered=0.
            if region[0]<region[2] and region[1]<region[3]:
                covered=abs(sum(signed_area(edges,region) for bbox,edges in glyphs
                            if bbox[0]<xx+1 and bbox[2]>xx and bbox[1]<yy+1 and bbox[3]>yy))
            fg=quantized(covered)*ta
            for b,c in zip(background or (1.,1.,1.),color):
                base=int(255*(1-bg+b*bg)+.5)
                base=int(base*(1-frame)+.5)
                pixels.append(int(base*(1-fg)+255*c*fg+.5))
    return f'P6\n{width} {height}\n255\n'.encode()+pixels


def cases():
    def rendered(name,contents: bytes | None=b'(HI)',*,rows: tuple[str,...]=('HI',),size=12.,rd=(0,0,0,0),color=(0.,0.,0.),text_alpha=None,
                 dpi=72,visible=True,bold=False,leading=None,**options):
        expected={key:options[key] for key in ('rect','q','border','background','alpha','rotate') if key in options}
        return Case(name,source(contents,**options),RENDER,(REQUIREMENT,),
                    'Authored line breaks/AFM placement, analytic original glyph areas and literal border regions establish pixels',
                    dpi=dpi,information=('font.substituted',) if visible and any(rows) else (),
                    oracle=oracle(rows,size=size,rd=rd,color=color,text_alpha=text_alpha,dpi=dpi,visible=visible,
                                  bold=bold,leading=leading,**expected),extra={'tolerance':1})
    for q in range(3):
        for dpi in (72,144):
            yield rendered(f'quadding-{q}-{dpi}',q=q,dpi=dpi)
    yield rendered('curved-glyphs',contents=b'(ROUND)',rows=('ROUND',),dpi=144)
    yield rendered('explicit-lines',contents=b'(HI\\rHI)',rows=('HI','HI'))
    yield rendered('crlf-lines',contents=b'(HI\\r\\nHI)',rows=('HI','HI'))
    yield rendered('wrap-spaces',contents=b'(HI HI HI)',rows=('HI HI','HI'),rect=(4,4,42,44))
    yield rendered('wrap-long-word',contents=b'(HHHHH)',rows=('HHH','HH'),rect=(4,4,36,44))
    yield rendered('clip-overflow',contents=b'(HI\\rHI\\rHI\\rHI)',rows=('HI','HI','HI','HI'))
    yield rendered('auto-size-cap',da=b'(/Helvetica 0 Tf 0 g)')
    _,ascent,descent=metrics()
    yield rendered('auto-size-height',da=b'(/Helvetica 0 Tf 0 g)',rect=(4,4,124,18),size=10/(ascent-descent))
    yield rendered('rd',extra=b'/RD [4 6 8 2]',rd=(4,6,8,2))
    yield rendered('border-background',border=2,background=(1.,1.,0.))
    yield rendered('background-empty-text',contents=b'()',rows=(),background=(1.,1.,0.))
    yield rendered('text-color',da=b'(/Helvetica 12 Tf 0 0 1 rg)',color=(0.,0.,1.))
    yield rendered('opacity',border=2,background=(1.,1.,0.),alpha=.5)
    yield rendered('separate-opacity',border=2,background=(1.,1.,0.),alpha=.75,text_alpha=.25,extra=b'/ca 0.25')
    yield rendered('leading',contents=b'(HI\\rHI)',rows=('HI','HI'),da=b'(/Helvetica 12 Tf 18 TL)',leading=18)
    yield rendered('rotated',rotate=90,dpi=144)
    yield rendered('typewriter-ignores-callout',extra=b'/IT /FreeTextTypeWriter /CL 999 0 R /LE 42')
    yield rendered('plain-ignores-callout',extra=b'/CL 999 0 R /LE 42')
    yield rendered('utf16',contents=b'<FEFF00480049>')
    yield rendered('null-optionals',extra=b'/RD null /IT null /RC null /DS null')
    yield rendered('indirect',contents=b'6 0 R',da=b'7 0 R',objects=(b'(HI)',b'(/Helvetica 12 Tf 0 g)'))
    for dpi in (72,144):
        yield rendered(f'stored-{dpi}',stored=True,dpi=dpi,da=b'42',extra=b'/IT /FreeTextCallout /RC 42')
    for name,flags in (('hidden',2),('no-view',32)):
        yield rendered(name,flags=flags,visible=False)
    yield rendered('absent-contents',contents=None,rows=())
    yield rendered('null-contents',contents=b'null',rows=())
    catalog=b'/AcroForm << /Fields [] /DR << /Font << /Body 6 0 R /Unused 999 0 R >> >> >>'
    font=b'<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica-Bold /Encoding /WinAnsiEncoding >>'
    yield rendered('dr-font',da=b'(/Body 12 Tf)',catalog=catalog,objects=(font,),bold=True)
    for name,options in [('missing-da',{'da':None}),('da-type',{'da':b'42'}),('contents-type',{'contents':b'42'}),
                         ('quadding-type',{'q':'/Left'}),('quadding-range',{'q':3}),('intent-type',{'extra':b'/IT 42'}),
                         ('rd-cardinality',{'extra':b'/RD [1 2 3]'}),('unknown-font',{'da':b'(/Mystery 12 Tf)'}),
                         ('present-bad-font',{'da':b'(/Body 12 Tf)','catalog':catalog,'objects':(b'42',)})]:
        yield Case(name,source(**options),OPEN,(REQUIREMENT,),'Malformed selected values fail explicitly',extra={'render_error':'syntax'})
    for name,options,feature in [('callout',{'extra':b'/IT /FreeTextCallout /CL [1 1 4 4]'},'annotation.appearance'),
                                ('rich-text',{'extra':b'/RC (rich)'},'annotation.appearance'),
                                ('default-style',{'extra':b'/DS (font: Helvetica)'},'annotation.appearance'),
                                ('cloudy',{'extra':b'/BE << /S /C >>'},'annotation.appearance'),
                                ('complex-text',{'contents':b'<FEFF03A9>'},'appearance.font-encoding')]:
        yield Case(name,source(contents=options.get('contents',b'(HI)'),extra=options.get('extra',b'')),UNSUPPORTED,(REQUIREMENT,),
                   'Remaining richer appearance surfaces retain explicit capability refusals',feature=feature,
                   information=('font.substituted',) if name=='complex-text' else ())
