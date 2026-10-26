"""Rich annotation controls with literal rectangles from an original Type3 font.

Every row baseline, run position, size and color below is authored independently
of the product parser and layout. The stored controls contain literal artwork.
"""
from .forms import base
from .document import Builder, navigation_value
from .document_forms import name, ref
from .model import Case, OPEN, RENDER, UNSUPPORTED
from .pdf import stream

AREA='rich_annotations'
REQUIREMENT='content-rich-annotation-text'
BLACK=(0.,0.,0.)
BLUE=(0.,0.,1.)


def source(xml: bytes | int | None=b'<body><p>AB</p></body>',*,ds=None,contents=b'AB',rect=(4,4,124,60),extra=None,rotate=0,alpha=1.,stored=None,streamed=False,shape='FreeText'):
    objects=base()
    objects[10]['Fields']=[]
    objects[3]['Rotate']=rotate
    annotation={'Type':name('Annot'),'Subtype':name(shape),'Rect':list(rect),'BS':{'W':0},'DA':b'/F1 10 Tf 0 g','Contents':contents,'CA':alpha}
    if shape=='Line':annotation|={'L':[10,20,110,20],'Cap':True,'C':[0,0,0]}
    if xml is not None:annotation['RC']=ref(90) if streamed else xml
    if streamed:
        assert isinstance(xml,bytes)
        objects[90]=stream(xml)
    if ds is not None:annotation['DS']=ds
    if extra:annotation.update(extra)
    if stored is not None:
        annotation['AP']={'N':ref(91)}
        objects[91]=stream(stored,b'/Type /XObject /Subtype /Form /BBox [4 4 124 60] /Resources << >>')
    objects[11]=annotation
    builder=Builder()
    for number,body in sorted(objects.items()):builder.obj(number,body if isinstance(body,bytes) else navigation_value(body))
    return builder.classic()[0]


def oracle(regions,*,alpha=1.,dpi=72,rotate=0,clip=(6,6,122,58)):
    scale=dpi/72
    def box(rect):
        a,b,c,d=rect
        return tuple(v*scale for v in ((b,128-c,d,128-a) if rotate else rect))
    regions=[(box(rect),color) for rect,color in regions];clip=box(clip)
    width,height=(64,128) if rotate else (128,64);width,height=int(width*scale),int(height*scale)
    pixels=bytearray()
    for y in reversed(range(height)):
        for x in range(width):
            pixel=[255,255,255]
            for (a,b,c,d),color in regions:
                coverage=max(0.,min(x+1,c,clip[2])-max(x,a,clip[0]))*max(0.,min(y+1,d,clip[3])-max(y,b,clip[1]))
                opacity=int(min(1.,coverage)*255+.5)/255*alpha
                pixel=[int(old*(1-opacity)+255*value*opacity+.5) for old,value in zip(pixel,color)]
            pixels.extend(pixel)
    return f'P6\n{width} {height}\n255\n'.encode()+pixels


def cases():
    ordinary=[((6,50,11,57),BLACK),((12,50,17,57),BLACK)]
    def rendered(label,xml: bytes | None=b'<body><p>AB</p></body>',*,regions=ordinary,dpi=72,clip=(6,6,122,58),**options):
        return Case(label,source(xml,**options),RENDER,(REQUIREMENT,),
                    'Literal original Type3 glyph rectangles and independently authored layout establish exact pixels',
                    oracle=oracle(regions,alpha=options.get('alpha',1.),rotate=options.get('rotate',0),dpi=dpi,clip=clip),dpi=dpi,extra={'tolerance':1})
    for dpi in (72,144):
        yield rendered(f'plain-{dpi}',dpi=dpi)
        yield rendered(f'stored-{dpi}',dpi=dpi,stored=b'0 g 6 50 5 7 re f 12 50 5 7 re f')
    yield rendered('span-color',b'<body><p>A<span style="color:#0000ff">B</span></p></body>',regions=[ordinary[0],(ordinary[1][0],BLUE)])
    yield rendered('inheritance',b'<body style="color:#0000ff"><p>AB</p></body>',regions=[(r,BLUE) for r,_ in ordinary])
    yield rendered('default-style',None,ds=b'font:10pt F1;color:rgb(0,0,255)',regions=[(r,BLUE) for r,_ in ordinary])
    yield rendered('mixed-size',b'<body><p>A<span style="font-size:20pt">B</span></p></body>',regions=[((6,42,11,49),BLACK),((12,42,22,56),BLACK)])
    yield rendered('line-break',b'<body><p>A<br/>B</p></body>',regions=[ordinary[0],((6,38,11,45),BLACK)])
    yield rendered('paragraphs',b'<body><p>A</p><p>B</p></body>',regions=[ordinary[0],((6,38,11,45),BLACK)])
    yield rendered('empty-paragraph',b'<body><p>A</p><p/><p>B</p></body>',regions=[ordinary[0],((6,26,11,33),BLACK)])
    yield rendered('leading-break',b'<body><p><br/>A</p></body>',regions=[((6,38,11,45),BLACK)])
    yield rendered('paragraph-spacing',b'<body><p style="margin-bottom:4pt">A</p><p>B</p></body>',regions=[ordinary[0],((6,34,11,41),BLACK)])
    yield rendered('center',b'<body><p style="text-align:center">AB</p></body>',regions=[((58,50,63,57),BLACK),((64,50,69,57),BLACK)])
    yield rendered('right',b'<body><p style="text-align:right">AB</p></body>',regions=[((110,50,115,57),BLACK),((116,50,121,57),BLACK)])
    yield rendered('margins',b'<body><p style="margin:2pt 3pt 4pt 5pt">AB</p></body>',regions=[((11,48,16,55),BLACK),((17,48,22,55),BLACK)])
    yield rendered('spacing',b'<body><p style="letter-spacing:1pt">AB</p></body>',regions=[ordinary[0],((13,50,18,57),BLACK)])
    yield rendered('wrap',b'<body><p>AB CD</p></body>',rect=(4,4,24,60),clip=(6,6,22,58),regions=ordinary+[((6,38,11,45),BLACK),((12,38,17,45),BLACK)])
    yield rendered('entities',b'<body><p>A&#32;B</p></body>',regions=[ordinary[0],((18,50,23,57),BLACK)])
    yield rendered('whitespace',b'<body>\n<p>  A\n\t B  </p>\n</body>',regions=[ordinary[0],((18,50,23,57),BLACK)])
    yield rendered('scaled',b'<body><p style="xfa-font-horizontal-scale:150%;xfa-font-vertical-scale:50%">AB</p></body>',regions=[((6,54,13.5,57.5),BLACK),((15,54,22.5,57.5),BLACK)])
    yield rendered('raised',b'<body><p>A<span style="vertical-align:3pt">B</span></p></body>',regions=[((6,47,11,54),BLACK),((12,50,17,57),BLACK)])
    yield rendered('underline',b'<body><p style="text-decoration:underline">AB</p></body>',regions=ordinary+[((6,49,18,49.5),BLACK)])
    yield rendered('default-scale',extra={'DA':b'/F1 10 Tf 150 Tz'},regions=[((6,50,13.5,57),BLACK),((15,50,22.5,57),BLACK)])
    yield rendered('default-matrix',extra={'DA':b'/F1 10 Tf 1.5 0 0 .5 0 0 Tm'},regions=[((6,54,13.5,57.5),BLACK),((15,54,22.5,57.5),BLACK)])
    yield rendered('default-word-spacing',b'<body><p>A B</p></body>',extra={'DA':b'/F1 10 Tf 2 Tw'},regions=[ordinary[0],((20,50,25,57),BLACK)])
    yield rendered('caption',b'<p style="font:10pt F1">AB</p>',shape='Line',regions=[((54,17,59,24),BLACK),((60,17,65,24),BLACK)])
    yield rendered('caption-color',b'<p style="font:10pt F1;color:#0000ff">AB</p>',shape='Line',ds=42,regions=[((54,17,59,24),BLUE),((60,17,65,24),BLUE)])
    yield rendered('caption-trailing-break',b'<p style="font:10pt F1">AB<br/></p>',shape='Line',regions=[((54,23,59,30),BLACK),((60,23,65,30),BLACK)])
    yield rendered('caption-overlapping-lines',b'<p style="font:10pt F1;line-height:0pt">AB<br/><span style="color:#0000ff">C</span></p>',shape='Line',regions=[((54,17,59,24),BLACK),((60,17,65,24),BLACK),((57,17,62,24),BLUE)])
    yield rendered('opacity',alpha=.25)
    yield rendered('rotation',rotate=90,dpi=144)
    yield rendered('text-stream',streamed=True)
    for suffix,encoding,bom in [('big','utf-16-be',b'\xfe\xff'),('little','utf-16-le',b'\xff\xfe')]:
        yield rendered(f'text-stream-utf16-{suffix}',bom+'<body><p>AB</p></body>'.encode(encoding),streamed=True)
    yield rendered('rc-precedence',contents=42)
    yield rendered('ignored-elements',b'<body><p>A<unknown>B</unknown>B</p></body>')
    yield rendered('inert-link',b'<body><p><a href="https://example.invalid/">AB</a></p></body>')
    yield rendered('empty',b'<body/>',regions=[])
    yield rendered('hidden',b'invalid',extra={'F':2},regions=[])
    yield rendered('stored-precedence',b'invalid',extra={'DA':42,'DS':42},stored=b'0 g 6 50 5 7 re f 12 50 5 7 re f')
    for label,xml,extra in [('mismatched',b'<p>AB</b>',None),('undefined-entity',b'<p>&unknown;</p>',None),
                           ('nested-p',b'<p><p>A</p></p>',None),('text-outside-root',b'AB',None),
                           ('invalid-size',b'<p style="font-size:-1pt">AB</p>',None),('rc-type',42,None),
                           ('ds-type',b'<p>AB</p>',{'DS':42})]:
        yield Case(label,source(xml,extra=extra),OPEN,(REQUIREMENT,),'Invalid selected rich text fails explicitly',extra={'render_error':'syntax'})
    for label,xml in [('external-entity',b'<!DOCTYPE p><p>AB</p>'),('embedded-object',b'<p><span xfa:embed="other"/></p>'),
                       ('rtl',b'<p dir="rtl">AB</p>'),('unsupported-layout',b'<p style="tab-stops:20pt">AB</p>')]:
        yield Case(label,source(xml),UNSUPPORTED,(REQUIREMENT,),'Unsupported semantics are never silently omitted',feature='annotation.appearance')
    yield Case('automatic-size',source(extra={'DA':b'/F1 0 Tf'}),UNSUPPORTED,(REQUIREMENT,),
               'Unimplemented rich automatic sizing fails explicitly rather than selecting an arbitrary size',feature='annotation.appearance')
    yield Case('invalid-stream-utf16',source(b'\xff\xfe\x00\xd8',streamed=True),OPEN,(REQUIREMENT,),
               'XML stream encoding errors fail without text-string repair or substitution',extra={'render_error':'syntax'})
