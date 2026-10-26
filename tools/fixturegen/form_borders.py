"""Filled-form borders with conic/trapezoid areas and literal Type 3 glyphs.

These vectors are exercised through filled_scene, save/reopen and the real WASI
editing client. Expected pixels never interpret a generated appearance stream.
"""
from .circle_annotations import rectangle_area
from .document import Builder, navigation_value
from .document_forms import name, ref
from .forms import base
from .model import Vector
from .pdf import stream
from .relief_annotations import oracle as relief_oracle
from .rounded_annotations import area

AREA='form_borders'
REQUIREMENT='forms-appearance-style'


def oracle(*,style='B',width=2.,color=(0.,0.,0.),background=None,kind='Tx',text='AB',dpi=72,
           rotate=0,stored=False,visible=True):
    scale=dpi/72;size=int(32*scale)
    def point(x,y):return (y*scale,(32-x)*scale) if rotate else (x*scale,y*scale)
    def box(bounds):
        a,b=point(*bounds[:2]),point(*bounds[2:])
        return min(a[0],b[0]),min(a[1],b[1]),max(a[0],b[0]),max(a[1],b[1])
    def mask(v):return int(max(0.,min(1.,v))*255+.5)/255
    if style in ('B','I') and not stored:
        pixels=bytearray(relief_oracle(style=style,width=width,color=color,fill=background,background=True,
                                       dpi=dpi,rotate=rotate,visible=visible).split(b'\n',3)[3])
    else:
        pixels=bytearray();h=width/2;radius=min(6.,12-h) if style=='rounded' else 0.
        outer=box((4,4,28,28));inner=box((4+width,4+width,28-width,28-width))
        for y in reversed(range(size)):
            for x in range(size):
                values=[1.,1.,1.]
                if visible:
                    fill=(.75,.75,.75) if stored else background
                    if fill is not None:
                        a=mask(rectangle_area(outer,x,y));values=[1-a+c*a for c in fill]
                    if not stored and width and color is not None:
                        ro=(radius+h)*scale if radius else 0.;ri=max(0.,radius-h)*scale if radius else 0.
                        a=mask(area(outer,(ro,ro),x,y)-area(inner,(ri,ri),x,y))
                        values=[v*(1-a)+c*a for v,c in zip(values,color)]
                pixels.extend(int(v*255+.5) for v in values)
    if visible and kind!='Btn' and text and width<10:
        # Type 3 glyph metrics: advance 6, black rectangle 5 by 7 at size 10.
        # Centre vertically in the 24-unit widget, baseline at page y=13.
        inset=width+2 if color is not None else 2
        rectangles=[box((4+inset+6*i,13,9+inset+6*i,20)) for i in range(len(text))]
        for y in reversed(range(size)):
            for x in range(size):
                coverage=mask(sum(rectangle_area(r,x,y) for r in rectangles))
                offset=((size-1-y)*size+x)*3
                for c in range(3):pixels[offset+c]=int(pixels[offset+c]*(1-coverage)+.5)
    return f'P6\n{size} {size}\n255\n'.encode()+pixels


def source(*,style='B',width=2.,color=(0.,0.,0.),background=None,kind='Tx',rotate=0,stored=False,flags=0,extra=None):
    objects=base(kind,1<<17 if kind=='Ch' else 0)
    objects[3]['MediaBox']=[0,0,32,32];objects[3]['Rotate']=rotate
    widget=objects[11];widget['Rect']=[4,4,28,28];widget['F']=flags
    widget['MK']={'BC':list(color or ())}
    if background is not None:widget['MK']['BG']=list(background)
    if style=='rounded':widget['Border']=[6,6,width]
    else:widget['BS']={'W':width,'S':name(style)}
    if kind=='Ch':widget['Opt']=['AA','BB']
    if stored:
        widget['AP']={'N':ref(80)}
        objects[80]=stream(b'0.75 g 0 0 24 24 re f /Tx BMC BT /F1 10 Tf (OLD) Tj ET EMC',
                           b'/Type /XObject /Subtype /Form /BBox [0 0 24 24] /Resources << /Font << /F1 70 0 R >> >>')
    if extra:widget.update(extra)
    builder=Builder()
    for number,body in sorted(objects.items()):
        builder.obj(number,body if isinstance(body,bytes) else navigation_value(body))
    return builder.classic()[0]


def cases():
    def rendered(identifier,*,dpi=72,text='AB',visible=True,expected_style=None,**options):
        expected={k:options[k] for k in ('style','width','color','background','kind','rotate','stored') if k in options}
        if expected_style is not None:expected['style']=expected_style
        return Vector(identifier,{'.pdf':source(**options),'.ppm':oracle(dpi=dpi,text=text,visible=visible,**expected)},
                      (REQUIREMENT,'forms-render-filled'),
                      'Independent conic/trapezoid area and literal glyph rectangles verify filled, saved and reopened form borders',
                      extra={'dpi':dpi,'field_kind':options.get('kind','Tx'),'text':text,'tolerance':1})
    for style in ('rounded','B','I'):
        for dpi in (72,144):
            for kind in ('Tx','Ch','Btn'):
                yield rendered(f'{kind}-{style}-{dpi}',kind=kind,style=style,dpi=dpi)
        yield rendered(f'background-{style}',style=style,background=(1.,0.,0.))
        yield rendered(f'rotated-{style}',style=style,rotate=90,dpi=144)
        yield rendered(f'stored-{style}',style=style,stored=True)
    yield rendered('unknown-style',style='Custom',expected_style='S')
    yield rendered('null-bs',style='rounded',extra={'BS':None})
    yield rendered('bs-precedence',style='B',extra={'Border':42})
    yield rendered('zero-width',style='B',width=0.)
    yield rendered('no-border-color',style='I',color=None)
    yield rendered('wide-off',style='B',width=100.,kind='Btn',text='')
    yield rendered('hidden',flags=2,visible=False)
    yield rendered('no-view',flags=32,visible=False)
