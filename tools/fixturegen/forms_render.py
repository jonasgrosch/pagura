"""Filled-widget pixel oracles from rational polygons, ISO 32000-2 12.5.5/12.7.

Rectangular Type 3 glyphs have a 600-unit advance and a 500 by 700 outline.
The explicit button provider instead uses a 100 by 200 TrueType rectangle.
Coordinates below are derived from field metrics and annotation transforms,
without interpreting appearance programs or importing a product executable.
"""
from copy import deepcopy
from fractions import Fraction as F
from .model import Vector
from .document import Builder, navigation_value
from .document_forms import name, ref
from .pdf import stream
from .raster import clipped_area

BLACK = (F(0),)*3
GRAY = (F(9,10),)*3
SELECT = (F(3,5), F(3,4), F(9,10))
IDENTITY = (1,0,0,1,0,0)


def glyphs(text='AB', x: F | int = 2, y: F | int = 7, size=10, color=BLACK, outline=(F(1,2),F(7,10))):
    return [((F(x)+i*F(3,5)*size,F(y),outline[0]*size,outline[1]*size), color)
            for i,c in enumerate(text) if c != ' ']


def pixels(marks, *, transform=IDENTITY, size=(128,64), scale=1):
    """Exact polygon/pixel coverage, source-over and one final nearest rounding."""
    a,b,c,d,e,f = map(F,transform)
    polygons=[]
    for (x,y,w,h), color in marks:
        points=[(F(x),F(y)),(F(x+w),F(y)),(F(x+w),F(y+h)),(F(x),F(y+h))]
        polygon=[((a*x+c*y+e)*scale,(b*x+d*y+f)*scale) for x,y in points]
        polygons.append((polygon, color))
    width,height=(n*scale for n in size)
    output=bytearray()
    for y in range(height):
        for x in range(width):
            rgb=[F(1)]*3
            for polygon,color in polygons:
                if not (min(p[0] for p in polygon)<x+1 and max(p[0] for p in polygon)>x
                        and min(p[1] for p in polygon)<height-y and max(p[1] for p in polygon)>height-y-1):
                    continue
                coverage=clipped_area(polygon,x,height-y-1)
                rgb=[s*coverage+t*(1-coverage) for s,t in zip(color,rgb)]
            output.extend(int(v*255+F(1,2)) for v in rgb)
    return f'P6\n{width} {height}\n255\n'.encode()+output


def vector(identifier, objects, marks=(), *, edits=None, requirement='forms-render-filled',
           transform=IDENTITY, scale=1, error=None, page=0, size=(128,64), provider=False, diagnostic=None):
    builder=Builder()
    for n,body in sorted(objects.items()):
        builder.obj(n,body if isinstance(body,bytes) else navigation_value(body))
    edits=edits if edits is not None else [[0,'text','AB']]
    # A line-oriented input keeps the test harness independent of a JSON parser.
    lines=[f'scale={scale}',f'page={page}',f'error={error or ""}',f'provider={"rectangle" if provider else ""}']
    if diagnostic: lines.append(f'diagnostic={diagnostic}')
    lines += [f'edit={field}|{kind}|{value}' for field,kind,value in edits]
    files={'.pdf':builder.classic()[0], '.render':('\n'.join(lines)+'\n').encode()}
    if provider:
        from .font import truetype
        files['.font']=truetype()
    if not error: files['.rgb']=pixels(marks,transform=transform,scale=scale,size=size)
    return Vector('render-'+identifier,files,(requirement,),
                  'Independent rational polygon coverage of filled widget appearances, ISO 32000-2 12.5.5 and 12.7.4.3.',
                  extra={'tolerance':1})


def appearance(content, bbox=(0,0,100,20), matrix=IDENTITY, *, extra=b''):
    entries=(f'/Type /XObject /Subtype /Form /BBox [{" ".join(map(str,bbox))}] '
             f'/Matrix [{" ".join(map(str,matrix))}] /Resources << /Font << /F1 70 0 R >> >> ').encode()
    return stream(content, entries+extra)


def optional_content(objects):
    objects[1]['OCProperties']={'OCGs':[ref(40)],'D':{'BaseState':name('ON'),'OFF':[ref(40)]}}
    objects[40]={'Type':name('OCG'),'Name':'hidden layer'}


def cases(base):
    yield vector('text',base(),glyphs())
    o=base();o[11]['Rect']=[10,8,110,28]
    yield vector('text-placement',o,glyphs(),transform=(1,0,0,1,10,8),requirement='forms-render-placement')
    o=base(widgets=2);o[11]['Q']=2;o[21]['Rect']=[0,20,60,40]
    yield vector('text-multiple-widgets',o,glyphs(x=86)+glyphs(x=46,y=27))
    o=base(ap=True)
    yield vector('text-update',o,[((0,0,100,20),GRAY)]+glyphs())
    o=base(ap=True,need=True);o[11]['V']='AB'
    yield vector('text-need-appearances',o,[((0,0,100,20),GRAY)]+glyphs(),edits=[],requirement='forms-render-need-appearances')
    o=base(ap=True)
    # Without NeedAppearances, an untouched field keeps its original normal AP.
    yield vector('text-unchanged',o,[((0,0,100,20),GRAY)]+glyphs('OLD',x=0,y=0),edits=[])
    o=base(flags=1<<24);o[11]['MaxLen']=4
    yield vector('text-comb',o,glyphs('A',x=11)+glyphs('B',x=35))
    o=base(flags=1<<12);o[11]['Rect']=[0,0,40,40]
    yield vector('text-multiline',o,glyphs('ABC',y=30)+glyphs('DEF',y=18)+glyphs('GHI',y=6),edits=[[0,'text','ABC DEF GHI']])
    o=base();o[10]['DA']=b'/F1 10 Tf 1 Tc 2 Tw 50 Tz 14 TL 2 Ts 2 0 0 1 400 500 Tm 0.25 g';o[11]['Q']=1
    yield vector('text-da-state',o,[((43,7,5,7),(F(1,4),)*3),((50,7,5,7),(F(1,4),)*3)])
    o=base();o[10]['DA']=b'/F1 0 Tf';o[11]['Q']=2
    yield vector('text-auto-size',o,glyphs(x=F(418,5),y=F(32,5),size=12))
    o=base(ap=True);o[11]['Rect']=[20,4,60,44]
    o[80]=appearance(b'/Tx BMC BT /F1 10 Tf (OLD) Tj ET EMC',bbox=(10,20,50,40),matrix=(0,1,-1,0,100,200))
    # Envelope fitting gives (x,y)->(100-2*y,x-6) after the quarter turn.
    yield vector('text-bbox-matrix',o,glyphs(x=12,y=27),transform=(0,1,-2,0,100,-6),requirement='forms-render-placement')
    o=base(ap=True);o[11]['Rect']=[0,0,120,40]
    o[80]=appearance(b'/Tx BMC BT /F1 10 Tf (OLD) Tj ET EMC',matrix=(1,0,1,1,5,7))
    yield vector('text-oblique-envelope',o,glyphs(),transform=(1,0,1,2,0,0),requirement='forms-render-placement')
    o=base(ap=True);o[11]['Rect']=[.25,.5,100.25,20.5]
    yield vector('text-fractional-area',o,[((0,0,100,20),GRAY)]+glyphs(),transform=(1,0,0,1,F(1,4),F(1,2)),requirement='forms-render-placement')
    o=base(ap=True);o[80]=appearance(b'q 0 g -100 -100 300 220 re f Q /Tx BMC BT /F1 10 Tf (OLD) Tj ET EMC')
    # Source artwork extends beyond BBox, but the annotation clips it.
    yield vector('text-bbox-clip',o,[((0,0,100,20),BLACK)],requirement='forms-render-placement')
    o=base();o[3]['Contents']=ref(50);o[50]=stream(b'1 0 0 rg 110 50 5 5 re f 0 0 1 1 re W n 7 0 0 7 200 200 cm')
    yield vector('text-page-state-isolation',o,[((110,50,5,5),(F(1),F(0),F(0)))]+glyphs(),requirement='forms-render-placement')
    o=base(ap=True);o[75]=deepcopy(o[70]);o[75]['CharProcs']={g:ref(71 if g=='space' else 73) for g in o[70]['CharProcs']}
    o[73]=stream(b'600 0 d0 0 0 250 500 re f')
    o[80]=stream(b'/Tx BMC BT /F1 10 Tf (OLD) Tj ET EMC',b'/Type /XObject /Subtype /Form /BBox [0 0 100 20] /Resources << /Font << /F1 75 0 R >> >>')
    yield vector('text-existing-resources',o,glyphs(outline=(F(1,4),F(1,2))))
    o=base(ap=True);o[80]=appearance(b'/Tx BMC BT /F1 10 Tf (OLD) Tj ET EMC',extra=b'/Group << /S /Transparency /I true /CS /DeviceRGB >>')
    yield vector('text-transparency-group',o,glyphs(),requirement='forms-render-placement')
    o=base('Ch',1<<17);o[11]['Opt']=[['a','AA'],['b','BB']]
    yield vector('choice-combo',o,glyphs('BB'),edits=[[0,'indices','1']])
    o[10]['NeedAppearances']=True;o[11]['V']='BB';o[11]['AP']={'N':ref(80)}
    yield vector('choice-need-appearances',o,[((0,0,100,20),GRAY)]+glyphs('BB'),edits=[],requirement='forms-render-need-appearances')
    o=base('Ch',1<<21);o[11]|={'Rect':[0,0,100,40],'Opt':[['a','AA'],['b','BB'],['c','CC']]}
    listing=[((2,26,96,12),SELECT)]+glyphs('AA',y=29)+glyphs('BB',y=17)+[((2,2,96,12),SELECT)]+glyphs('CC',y=5)
    yield vector('choice-list-multi',o,listing,edits=[[0,'indices','2,0']])
    o[10]['NeedAppearances']=True;o[11]|={'V':['AA','CC'],'I':[1]}
    yield vector('choice-list-need-appearances',o,listing,edits=[],requirement='forms-render-need-appearances')
    o=base('Ch');o[11]|={'Rect':[0,0,100,20],'Opt':['AA','BB'],'TI':1}
    # The second row begins at y=6, baseline=9. The third row is absent.
    yield vector('choice-list-top',o,[((2,6,96,12),SELECT)]+glyphs('BB',y=9),edits=[[0,'indices','1']])
    for radio in (False,True):
        o=base('Btn',(1<<15) if radio else 0,widgets=2)
        for i in range(2):o[20+i]|={'AP':{'N':{'Off':ref(81),'Yes':ref(82)}},'AS':name('Off')}
        o[82]=appearance(b'0 g 3 4 10 8 re f',matrix=(0,1,-1,0,20,0))
        # Quarter-turn AP: source bbox 100x20 -> each widget rect 100x20.
        selected=[((40,F(3,5),40,2),BLACK)]
        marks=selected+[((x,y+20,w,h),color) for (x,y,w,h),color in selected] if not radio else [((x,y+20,w,h),color) for (x,y,w,h),color in selected]
        yield vector('radio' if radio else 'checkbox',o,marks,edits=[[0,'widget','1']])
        yield vector('radio-off' if radio else 'checkbox-off',o,edits=[[0,'off','']])
        o[10]['NeedAppearances']=True;o[20]['AS']=name('Yes')
        yield vector('radio-need-preserves-state' if radio else 'checkbox-need-preserves-state',o,selected,edits=[],requirement='forms-render-need-appearances')
    for radio in (False,True):
        o=base('Btn',(1<<15) if radio else 0)
        label='radio' if radio else 'checkbox'
        # The explicit test provider maps ZaDb code 52 to a rectangular glyph A:
        # 100 by 200 at 1000 units/em, painted at size 16 and baseline (2,2).
        yield vector(label+'-generated',o,[((2,2,F(8,5),F(16,5)),BLACK)],edits=[[0,'widget','0']],provider=True)
        yield vector(label+'-generated-off',o,edits=[[0,'off','']])
    o=base('Btn');o[11]|={'AP':{'N':{'Yes':ref(80)}},'AS':name('Yes')}
    yield vector('checkbox-undefined-off',o,edits=[[0,'off','']])
    for flag,label in [(2,'hidden'),(32,'no-view')]:
        o=base();o[11]['F']=flag;o[10]['DA']=b'0 g'
        yield vector('text-'+label,o,requirement='forms-render-visibility')
    o=base();optional_content(o);o[11]['OC']=ref(40);o[10]['DA']=b'0 g'
    yield vector('text-optional-content',o,requirement='forms-render-visibility')
    o=base(ap=True);optional_content(o);o[80]=appearance(b'/Tx BMC BT /F1 10 Tf (OLD) Tj ET EMC',extra=b'/OC 40 0 R')
    yield vector('appearance-optional-content',o,requirement='forms-render-visibility')
    o=base(ap=True);optional_content(o);o[10]['DA']=b'0 g'
    o[80]=appearance(b'/Tx BMC BT /F1 10 Tf (OLD) Tj ET EMC',extra=b'/OC 40 0 R')
    yield vector('invisible-appearance-is-lazy',o,requirement='forms-render-visibility')
    o=base();o[3]['Rotate']=90
    yield vector('text-page-rotation',o,glyphs(),transform=(0,-1,1,0,0,128),size=(64,128),requirement='forms-render-placement')
    o[11]['F']=16
    yield vector('text-no-rotate',o,glyphs(x=22,y=115),size=(64,128),requirement='forms-render-visibility')
    o=base();o[11]['F']=8|16
    # At 2x zoom, NoZoom retains 100x20 device units about Rect's (0,20) anchor.
    yield vector('text-no-zoom',o,glyphs(x=1,y=F(27,2),size=5),scale=2,requirement='forms-render-visibility')
    o=base();o[10]['NeedAppearances']=True;o[10]['Fields'].append(ref(12));o[3]['Annots'].append(ref(12))
    o[12]={'T':'choice','FT':name('Ch'),'Ff':1<<17,'Opt':['AA','BB'],'V':'BB','Subtype':name('Widget'),'Rect':[0,20,100,40],'AP':{'N':ref(80)}}
    yield vector('need-all-variable-fields',o,glyphs()+[((0,20,100,20),GRAY)]+glyphs('BB',y=27),requirement='forms-render-need-appearances')
    o=base();o[10]['NeedAppearances']=True;o[10]['Fields'].append(ref(12));o[2]['Kids'].append(ref(4));o[2]['Count']=2
    o[4]={'Type':name('Page'),'Parent':ref(2),'MediaBox':[0,0,128,64],'Resources':{},'Annots':[ref(12)]}
    o[12]={'T':'other','FT':name('Tx'),'Ff':1<<25,'V':'AB','Subtype':name('Widget'),'Rect':[0,0,100,20]}
    yield vector('other-page-is-lazy',o,glyphs(),requirement='forms-render-visibility')
    o=base();o[3]['Annots'].append(ref(12))
    o[12]={'Subtype':name('Widget'),'Rect':[0,20,100,40],'T':'entry','FT':name('Tx'),'AP':{'N':ref(80)}}
    yield vector('annotation-identity',o,glyphs()+[((0,20,100,20),GRAY)]+glyphs('OLD',x=0,y=20))
    o=base(ap=True);o[10]['DA']=b'0 g'
    o[80]=appearance(b'undefined',bbox=(0,0,0,20))
    yield vector('empty-bbox',o,error='syntax',requirement='forms-render-placement')
    o=base();o[10]['DA']=b'0 g'
    yield vector('generation-error',o,error='syntax')
    o=base(ap=True);o[80]=appearance(b'undefined /Tx BMC BT /F1 10 Tf (OLD) Tj ET EMC')
    yield vector('content-error-position',o,error='syntax')

    # Unsupported generation of an untouched variable field retains valid AP.
    old=[((0,0,100,20),GRAY)]+glyphs('OLD',x=0,y=0)
    for kind in ('type0','hybrid-xfa','oblique-tm'):
        o=base(ap=True,need=True);o[11]['V']='AB'
        if kind=='type0':
            from .font import truetype
            font=truetype()
            o[10]['DA']=b'/F2 10 Tf 0 g';o[10]['DR']['Font']['F2']=ref(75)
            o[75]={'Type':name('Font'),'Subtype':name('Type0'),'BaseFont':name('Rectangle'),
                   'Encoding':name('Identity-H'),'DescendantFonts':[ref(76)]}
            o[76]={'Type':name('Font'),'Subtype':name('CIDFontType2'),'BaseFont':name('Rectangle'),
                   'CIDSystemInfo':{'Registry':b'Adobe','Ordering':b'Identity','Supplement':0},
                   'FontDescriptor':ref(77),'CIDToGIDMap':name('Identity'),'DW':600}
            o[77]={'Type':name('FontDescriptor'),'FontName':name('Rectangle'),'Flags':32,
                   'FontBBox':[0,0,100,200],'ItalicAngle':0,'Ascent':800,'Descent':-200,
                   'CapHeight':700,'StemV':80,'FontFile2':ref(78)}
            o[78]=stream(font,b'/Length1 '+str(len(font)).encode())
        elif kind=='hybrid-xfa':
            o[10]['XFA']=ref(90)
            o[90]=stream(b'<xdp:xdp xmlns:xdp="http://ns.adobe.com/xdp/"><template xmlns="http://www.xfa.org/schema/xfa-template/3.3/"/></xdp:xdp>')
        else:
            o[10]['DA']=b'/F1 10 Tf 0 1 -1 0 400 500 Tm'
        yield vector('fallback-'+kind,o,old,edits=[],diagnostic='form.appearance-kept',requirement='forms-render-fallback')

    o=base();o[3]['Annots'].append(ref(12));o[10]['Fields'].append(ref(12))
    o[12]={'T':'signature','FT':name('Sig'),'Subtype':name('Widget'),'Rect':[0,20,100,40]}
    yield vector('signature-without-ap',o,glyphs(),requirement='forms-render-visibility')
    o=base();o[11]['Rect']=[0,0,100,0]
    yield vector('zero-area-without-ap',o,requirement='forms-render-visibility')
    o=base(ap=True,need=True);o[10]['DA']=b'0 g'
    o[80]=appearance(b'undefined',matrix=(0,0,0,1,0,0))
    yield vector('degenerate-matrix',o,error='syntax',requirement='forms-render-placement')

    o=base(widgets=2)
    o[70]={'Type':name('Font'),'Subtype':name('Type1'),'BaseFont':name('ZapfDingbats')}
    yield vector('shared-font-cache',o,[((2,7,1,2),BLACK),((2,27,1,2),BLACK)],edits=[[0,'text','A']],provider=True)
    o=base(widgets=2)
    from .font import truetype
    font=truetype()
    o[70]={'Type':name('Font'),'Subtype':name('TrueType'),'BaseFont':name('Rectangle'),
           'FirstChar':65,'LastChar':65,'Widths':[600],'Encoding':name('WinAnsiEncoding'),'FontDescriptor':ref(77)}
    o[77]={'Type':name('FontDescriptor'),'FontName':name('Rectangle'),'Flags':32,
           'FontBBox':[0,0,100,200],'ItalicAngle':0,'Ascent':800,'Descent':-200,
           'CapHeight':700,'StemV':80,'FontFile2':ref(78)}
    o[78]=stream(font,b'/Length1 '+str(len(font)).encode())
    yield vector('shared-embedded-font',o,[((2,7,1,2),BLACK),((2,27,1,2),BLACK)],edits=[[0,'text','A']])

    o=base(ap=True,need=True);o[10]['DA']=b'0 g'
    yield vector('malformed-da-with-stored-ap',o,edits=[],error='syntax',requirement='forms-render-fallback')
