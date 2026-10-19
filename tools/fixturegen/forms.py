"""Independent fill-in and appearance oracles, ISO 32000-2 12.7.4–12.7.5.

The appearance oracle uses a project-authored Type 3 font with 0.6 em widths
and 0.8/-0.2 em metrics. Programs follow declared layout policy, not Rust output.
"""
from fractions import Fraction
from .model import Vector
from .document import Builder, navigation_value
from .document_forms import name, ref
from .pdf import stream

AREA = 'forms'
GLYPHS = ['space', 'asterisk'] + list('ABCDEFGHIJKLMNOPQRSTUVWXYZ')


def base(kind='Tx', flags=0, *, widgets=1, need=False, ap=False):
    objects = {
        1: {'Type':name('Catalog'), 'Pages':ref(2), 'AcroForm':ref(10)},
        2: {'Type':name('Pages'), 'Kids':[ref(3)], 'Count':1},
        3: {'Type':name('Page'), 'Parent':ref(2), 'MediaBox':[0,0,128,64], 'Resources':{},
            'Annots':[ref(11)] if widgets == 1 else [ref(20+i) for i in range(widgets)]},
        10: {'Fields':[ref(11)], 'DA':b'/F1 10 Tf 0 g', 'DR':{'Font':{'F1':ref(70)}}, 'NeedAppearances':need},
        11: {'T':'entry', 'FT':name(kind), 'Ff':flags},
        70: {'Type':name('Font'), 'Subtype':name('Type3'), 'FontBBox':[0,-200,600,800],
            'FontMatrix':[.001,0,0,.001,0,0], 'FirstChar':32, 'LastChar':90, 'Widths':[600]*59,
            'Encoding':{'Type':name('Encoding'), 'Differences':[32,name('space'),42,name('asterisk'),65]+[name(c) for c in 'ABCDEFGHIJKLMNOPQRSTUVWXYZ']},
            'CharProcs':{g:ref(71 if g=='space' else 72) for g in GLYPHS}, 'Resources':{}},
        71: stream(b'600 0 d0\n'), 72:stream(b'600 0 d0\n0 0 500 700 re f\n'),
        80:stream(b'q\n0.9 g\n0 0 100 20 re f\nQ\n/Tx BMC\nBT /F1 10 Tf (OLD) Tj ET\nEMC\n', b'/Type /XObject /Subtype /Form /BBox [0 0 100 20] /Resources << /Font << /F1 70 0 R >> >>'),
        81:stream(b'', b'/Type /XObject /Subtype /Form /BBox [0 0 100 20] /Resources << >>'),
    }
    if widgets == 1:
        objects[11] |= {'Subtype':name('Widget'),'Rect':[0,0,100,20]}
        if ap: objects[11]['AP']={'N':ref(80)}
    else:
        objects[11]['Kids']=[ref(20+i) for i in range(widgets)]
        for i in range(widgets):
            objects[20+i]={'Subtype':name('Widget'),'Parent':ref(11),'Rect':[0,i*20,100,(i+1)*20]}
    return objects


def number(value):
    """Exact terminating decimal spelling, independent of platform floats."""
    value = Fraction(value)
    if value.denominator == 1: return str(value.numerator)
    return ('%.12f' % value).rstrip('0').rstrip('.')


def tx(text='AB', *, q=0, size=10, width=100, height=20, comb=None, multiline=False, password=False, da_state='0 g\n', ascent=Fraction(4,5), descent=Fraction(-1,5)):
    """Small analytic program oracle for the constant-width test font."""
    text = text.replace('\r\n','\n').replace('\r','\n')
    if password: text='*'*len(text)
    area_w, area_h = Fraction(width-4), Fraction(height-4)
    if size == 0: size=min(Fraction(12), area_h/(ascent-descent))
    size=Fraction(size)
    # Cases that need shrinking use a single line, so this is a direct ratio.
    if comb and size*Fraction(3,5)>area_w/comb:
        size=Fraction(int(area_w/comb/Fraction(3,5)*64),64)
    elif not multiline and not comb and len(text)*Fraction(3,5)*size > area_w:
        size=Fraction(int(area_w/max(1,len(text))/Fraction(3,5)*64),64)
    lines=[]
    if multiline:
        for paragraph in text.split('\n'):
            current=''
            for word in paragraph.split(' '):
                candidate=(current+' '+word) if current else word
                if len(candidate)*Fraction(3,5)*size > area_w and current:
                    lines.append(current);current=word
                else:current=candidate
            lines.append(current)
    else:lines=[text]
    out=f'/Tx BMC\nq\n2 2 {number(area_w)} {number(area_h)} re W n\nBT\n{da_state}/F1 {number(size)} Tf\n'
    baseline = height-2-ascent*size if multiline else 2+(area_h-(ascent-descent)*size)/2-descent*size
    if comb:
        offset=[0,(comb-len(text))//2,comb-len(text)][q]
        for i,c in enumerate(text):
            x=2+(offset+i)*area_w/comb+(area_w/comb-Fraction(3,5)*size)/2
            out+=f'1 0 0 1 {number(x)} {number(baseline)} Tm\n<{ord(c):02X}> Tj\n'
    else:
        for i,line in enumerate(lines):
            remaining=area_w-len(line)*Fraction(3,5)*size
            x=2+[0,remaining/2,remaining][q]
            y=baseline-i*size*Fraction(6,5)
            out+=f'1 0 0 1 {number(x)} {number(y)} Tm\n<{line.encode().hex().upper()}> Tj\n'
    return (out+'ET\nQ\nEMC\n').encode()


def vector(identifier, objects, *, mode='text', value='AB', expected='text|AB', error='', content=None, appearance='', changed=True, requirement='forms-fill-text'):
    builder=Builder()
    for number_,body in sorted(objects.items()):
        builder.obj(number_,body if isinstance(body,bytes) else navigation_value(body))
    params=f'mode={mode}\nerror={error}\nappearance={appearance}\nchanged={str(changed).lower()}\n'
    files={'.pdf':builder.classic()[0], '.params':params.encode(), '.value':value.encode(), '.expected':expected.encode()}
    if content is not None:
        if objects[11]['FT'] != name('Btn'):
            content=content.replace(b'BT\n', b'BT\n0 Tc\n0 Tw\n100 Tz\n0 TL\n0 Tr\n0 Ts\n0 g\n')
        files['.content']=content
    return Vector(identifier,files,(requirement,), 'Independent typed field replacement or appearance program, ISO 32000-2 12.7.')


def cases():
    for q in range(3):
        o=base();o[11]['Q']=q
        yield vector(f'text-quadding-{q}',o,content=b'q\nQ\n'+tx(q=q),appearance='generated')
    o=base();o[10]['DA']=b'/F1 0 Tf 0 g'
    yield vector('text-auto-size',o,content=b'q\nQ\n'+tx(size=0),appearance='generated')
    o=base();o[10]['DA']=b'/F1 0 Tf 0 g';o[11]['Rect']=[0,0,20,20]
    yield vector('text-auto-shrink',o,value='ABCDEFGH',expected='text|ABCDEFGH',content=b'q\nQ\n'+tx('ABCDEFGH',size=0,width=20),appearance='generated')
    o=base(flags=1<<24);o[11]['MaxLen']=4
    yield vector('text-comb',o,content=b'q\nQ\n'+tx(comb=4),appearance='generated')
    o[11]['Q']=2
    yield vector('text-comb-right',o,content=b'q\nQ\n'+tx(comb=4,q=2),appearance='generated')
    o=base(flags=1<<12);o[11]['Rect']=[0,0,40,40]
    yield vector('text-multiline-wrap',o,value='ABC DEF GHI\r\nJK',expected='text|ABC DEF GHI\nJK',content=b'q\nQ\n'+tx('ABC DEF GHI\nJK',multiline=True,width=40,height=40),appearance='generated')
    o=base(flags=1<<13)
    yield vector('text-password',o,content=b'q\nQ\n'+tx(password=True),appearance='generated')
    o=base();o[11]['MaxLen']=2
    yield vector('text-unicode-length',o,value='Ω😀',expected='text|Ω😀')
    yield vector('text-maxlen',o,value='ABC',error='invalid-argument')
    o=base(flags=1)
    yield vector('text-read-only',o,error='invalid-argument')
    o=base(flags=2|4)
    yield vector('text-required-noexport-empty',o,value='',expected='text|')
    o=base()
    yield vector('text-singleline-break',o,value='A\nB',error='invalid-argument')
    for flag in [0,1<<12,1<<13,1<<20]:
        o=base(flags=(1<<24)|flag)
        if flag:o[11]['MaxLen']=4
        yield vector(f'text-invalid-comb-{flag}',o,error='invalid-argument')
    o=base(flags=1<<25)
    yield vector('text-rich',o,error='unsupported:form.rich-text')
    o=base();o[10]['XFA']=b'<template/>'
    yield vector('text-xfa',o,error='unsupported:form.xfa')
    o=base(ap=True)
    yield vector('text-update-existing',o,content=b'q\n0.9 g\n0 0 100 20 re f\nQ\n'+tx()+b'\n',appearance='generated',requirement='forms-appearance-update')
    o=base(ap=True,need=True);o[11]['V']='AB'
    yield vector('text-need-appearances',o,changed=False,content=b'q\n0.9 g\n0 0 100 20 re f\nQ\n'+tx()+b'\n',appearance='generated',requirement='forms-appearance-update')
    o=base(ap=True);o[11]['V']='OLD'
    yield vector('text-preserve-existing',o,changed=False,appearance='existing:80',requirement='forms-appearance-update')
    o=base();o[11]['MK']={'BG':[1], 'BC':[0]};o[11]['BS']={'W':1}
    # Border creates three units of inset; this program is specified explicitly.
    content=b'q\n1 g\n0 0 100 20 re f\n0 G\n1 w\n[] 0 d\n0.5 0.5 99 19 re S\nQ\n/Tx BMC\nq\n3 3 94 14 re W n\nBT\n0 g\n/F1 10 Tf\n1 0 0 1 3 7 Tm\n<4142> Tj\nET\nQ\nEMC\n'
    yield vector('text-background-border',o,content=content,appearance='generated',requirement='forms-appearance-style')
    for q in range(3):
        o=base('Ch',1<<17);o[11]|={'Opt':[['a','AA'],['b','BB']],'Q':q}
        yield vector(f'choice-combo-{q}',o,mode='indices',value='1',expected='choice|BB|b|1',content=b'q\nQ\n'+tx('BB',q=q),appearance='generated',requirement='forms-fill-choice')
    o=base('Ch',(1<<17)|(1<<18));o[11]['Opt']=['AA']
    yield vector('choice-editable',o,mode='choice-text',value='CUSTOM',expected='choice|CUSTOM|CUSTOM|',content=b'q\nQ\n'+tx('CUSTOM'),appearance='generated',requirement='forms-fill-choice')
    o=base('Ch',1<<21);o[11]['Opt']=[['a','AA'],['b','BB'],['c','CC']]
    yield vector('choice-multi',o,mode='indices',value='2,0',expected='choice|AA,CC|a,c|0,2',requirement='forms-fill-choice')
    yield vector('choice-duplicate-index',o,mode='indices',value='1,1',error='invalid-argument',requirement='forms-fill-choice')
    yield vector('choice-index-range',o,mode='indices',value='3',error='invalid-argument',requirement='forms-fill-choice')
    yield vector('choice-empty',o,mode='indices',value='',expected='choice|||',requirement='forms-fill-choice')
    o[11]['Ff']=0
    yield vector('choice-single-multi',o,mode='indices',value='0,1',error='invalid-argument',requirement='forms-fill-choice')
    yield vector('choice-custom-noneditable',o,mode='choice-text',value='CUSTOM',error='invalid-argument',requirement='forms-fill-choice')
    yield vector('choice-export',o,mode='choice-export',value='b',expected='choice|BB|b|1',requirement='forms-fill-choice')
    o[11]['Opt']=[['a','AA'],['a','BB']]
    yield vector('choice-ambiguous-export',o,mode='choice-export',value='a',error='invalid-argument',requirement='forms-fill-choice')
    o[11]['Ff']=1<<18
    yield vector('choice-invalid-edit',o,mode='choice-text',value='AA',error='invalid-argument',requirement='forms-fill-choice')
    for radio in (False,True):
        o=base('Btn',(1<<15) if radio else 0,widgets=2)
        for i in range(2):o[20+i]['AP']={'N':{'Off':ref(81),'Yes':ref(80)}}
        prefix='radio' if radio else 'checkbox'
        yield vector(prefix+'-widget',o,mode='widget',value='0',expected='button|Yes|0:Yes,1:'+('Off' if radio else 'Yes'),appearance='existing:80',requirement='forms-fill-button')
        yield vector(prefix+'-off',o,mode='off',value='',expected='button|Off|0:Off,1:Off',appearance='existing:81',requirement='forms-fill-button')
        if radio:
            yield vector('radio-ambiguous-state',o,mode='state',value='Yes',error='invalid-argument',requirement='forms-fill-button')
            o[11]['Ff']|=1<<25
            yield vector('radio-unison',o,mode='widget',value='0',expected='button|Yes|0:Yes,1:Yes',appearance='existing:80',requirement='forms-fill-button')
            o[11]['Ff']|=1<<14
            yield vector('radio-no-toggle-off',o,mode='off',value='',error='invalid-argument',requirement='forms-fill-button')
    o=base('Btn',1<<15,widgets=2);o[11]['Opt']=['first','second']
    for i in range(2):o[20+i]['AP']={'N':{'Off':ref(81),str(i):ref(80)}}
    yield vector('radio-export-opt',o,mode='button-export',value='second',expected='button|1|0:Off,1:1',appearance='existing:81',requirement='forms-fill-button')
    o=base('Btn')
    check=b'q\nQ\nq\n2 2 96 16 re W n\n0 g\nBT\n/ZaDb 16 Tf\n1 0 0 1 2 2 Tm\n<34> Tj\nET\nQ\n'
    yield vector('checkbox-generated',o,mode='widget',value='0',expected='button|Yes|0:Yes',content=check,appearance='generated',requirement='forms-fill-button')
    o=base('Btn',1<<16)
    yield vector('button-push',o,mode='off',value='',error='invalid-argument',requirement='forms-fill-button')
    # List rows have a fixed 1.2 em line height; selection uses I and displayed V.
    o=base('Ch',1<<21);o[11]|={'Rect':[0,0,100,40],'Opt':[['a','AA'],['b','BB'],['c','CC']]}
    listing=(b'q\nQ\n/Tx BMC\nq\n2 2 96 36 re W n\n'
             b'q\n0.6 0.75 0.9 rg\n2 26 96 12 re f\nQ\nBT\n0 g\n/F1 10 Tf\n1 0 0 1 2 29 Tm\n<4141> Tj\nET\n'
             b'BT\n0 g\n/F1 10 Tf\n1 0 0 1 2 17 Tm\n<4242> Tj\nET\n'
             b'q\n0.6 0.75 0.9 rg\n2 2 96 12 re f\nQ\nBT\n0 g\n/F1 10 Tf\n1 0 0 1 2 5 Tm\n<4343> Tj\nET\nQ\nEMC\n')
    yield vector('choice-list-appearance',o,mode='indices',value='2,0',expected='choice|AA,CC|a,c|0,2',content=listing,appearance='generated',requirement='forms-fill-choice')
    o[11]|={'V':['AA','CC'],'I':[1]};o[10]['NeedAppearances']=True
    yield vector('choice-list-v-overrides-i',o,changed=False,content=listing,appearance='generated',requirement='forms-fill-choice')
    o[11]|={'V':['AA','CC'],'I':[0,2]}
    yield vector('choice-list-source-indices',o,changed=False,content=listing,appearance='generated',requirement='forms-fill-choice')
    o=base();o[10]['DA']=b'/F1 10 Tf 1 Tc 2 Tw 50 Tz 14 TL 2 Ts 2 0 0 1 400 500 Tm 0.25 g';o[11]['Q']=1
    content=b'q\nQ\n/Tx BMC\nq\n2 2 96 16 re W n\nBT\n1 Tc\n2 Tw\n50 Tz\n14 TL\n2 Ts\n0.25 g\n/F1 10 Tf\n2 0 0 1 43 5 Tm\n<4142> Tj\nET\nQ\nEMC\n'
    yield vector('text-default-appearance-state',o,content=content,appearance='generated',requirement='forms-appearance-style')
    o=base('Btn',1<<15)
    yield vector('radio-generated',o,mode='widget',value='0',expected='button|Yes|0:Yes',content=check,appearance='generated',requirement='forms-fill-button')
    o=base('Btn')
    yield vector('checkbox-generated-off',o,mode='off',value='',expected='button|Off|0:Off',content=b'q\nQ\n',appearance='generated',requirement='forms-fill-button')
    # Generation failures are separate from successful, atomic value validation.
    o=base(flags=1<<23);o[11]['Rect']=[0,0,20,20]
    yield vector('text-no-scroll-overflow',o,value='ABCDEFGH',expected='text|ABCDEFGH',appearance='error:unsupported:appearance.text-fit')
    o=base()
    yield vector('text-missing-font-character',o,value='Ω',expected='text|Ω',appearance='error:unsupported:appearance.font-encoding')
    o=base();o[10]['DA']=b'0 g'
    yield vector('text-invalid-da',o,appearance='error:syntax')
    o=base();o[10]['DA']=b'/F1 10 Tf 0 1 -1 0 30 40 Tm'
    yield vector('text-oblique-matrix',o,appearance='error:unsupported:appearance.text-matrix')
    o=base();o[11]['MK']={'R':90}
    yield vector('text-rotated-widget',o,appearance='error:unsupported:appearance.widget-rotation')
    o=base('Btn',1<<15,widgets=2);o[11]['Opt']=['shared','shared']
    for i in range(2):o[20+i]['AP']={'N':{'Off':ref(81),str(i):ref(80)}}
    yield vector('radio-duplicate-exports',o,mode='button-export',value='shared',error='invalid-argument',requirement='forms-fill-button')
    o=base('Btn');o[11]['AP']={'N':{'Off':ref(81)}}
    yield vector('checkbox-existing-no-on-state',o,mode='widget',value='0',error='invalid-argument',requirement='forms-fill-button')
    o=base('Btn');o[11]['AP']={'N':{'Yes':ref(80)}}
    yield vector('checkbox-undefined-off-state',o,mode='off',value='',expected='button|Off|0:Off',appearance='hidden',requirement='forms-fill-button')

    o=base();o[11]|={'MK':None,'BS':ref(82),'Border':None};o[82]=None;o[70]['FontDescriptor']=None
    yield vector('text-optional-null',o,content=b'q\nQ\n'+tx(),appearance='generated',requirement='forms-appearance-style')
    o=base(ap=True)
    o[80]=stream(b'q\n0.9 g\n0 0 100 20 re f\nQ\nBT 9 Tc 250 Tz 1 Ts ET\n/Tx BMC BT (OLD) Tj ET EMC\n', b'/Type /XObject /Subtype /Form /BBox [0 0 100 20] /Resources << /Font << /F1 70 0 R >> >>')
    expected=b'q\n0.9 g\n0 0 100 20 re f\nQ\nBT 9 Tc 250 Tz 1 Ts ET\n'+tx()+b'\n'
    yield vector('text-existing-state-reset',o,content=expected,appearance='generated',requirement='forms-appearance-update')

    o=base('Btn');o[11]|={'V':name('Custom'),'AS':name('Custom')}
    yield vector('checkbox-absent-ap-custom-state',o,mode='state',value='Custom',expected='button|Custom|0:Custom',content=check,appearance='generated',requirement='forms-fill-button')

    o=base();o[10]['DA']=b'/F1 -10 Tf'
    yield vector('text-negative-font-size',o,appearance='error:unsupported:appearance.text-matrix',requirement='forms-appearance-style')

    o=base(ap=True)
    o[80]=stream(b'BT /F1 10 Tf /Tx BMC (OLD) Tj EMC ET', b'/Type /XObject /Subtype /Form /BBox [0 0 100 20] /Resources << /Font << /F1 70 0 R >> >>')
    yield vector('text-existing-inside-text-object',o,appearance='error:unsupported:appearance.update',requirement='forms-appearance-update')

    o=base(flags=1<<12);o[11]['Rect']=[0,0,18,40]
    o[70]['Encoding']['Differences']=[32,name('A'),65,name('space'),66]+[name(c) for c in 'BCDEFGHIJKLMNOPQRSTUVWXYZ']
    content=b'q\nQ\n'+tx('A\nB',multiline=True,width=18,height=40).replace(b'<41>',b'<20>')
    yield vector('text-remapped-space-wrap',o,value='A B',expected='text|A B',content=content,appearance='generated',requirement='forms-appearance-style')

    o=base(flags=1<<24);o[11]|={'MaxLen':5,'Q':1,'Rect':[0,0,104,20]}
    yield vector('text-comb-centre-odd-cells',o,content=b'q\nQ\n'+tx(comb=5,q=1,width=104),appearance='generated',requirement='forms-appearance-style')

    # Without AP, a signature has nothing to generate and a widget without area nothing to show.
    o=base();o[3]['Annots']=o[10]['Fields']=[ref(11),ref(12)]
    o[12]={'T':'signature','FT':name('Sig'),'Subtype':name('Widget'),'Rect':[0,20,100,40]}
    yield vector('text-beside-signature',o,content=b'q\nQ\n'+tx(),appearance='generated',requirement='forms-appearance-update')
    o=base();o[11]['Rect']=[0,0,100,0]
    yield vector('text-zero-area',o,appearance='hidden',requirement='forms-appearance-update')

    from .forms_render import cases as render_cases
    yield from render_cases(base)
