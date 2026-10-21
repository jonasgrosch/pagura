"""Independent AcroForm structures and semantic expectations (ISO 32000-2, 12.7)."""
from copy import deepcopy
from typing import Any
from .model import Vector


def name(value):
    return ('name', value)


def ref(value):
    return ('ref', value)


def boolean(value):
    return str(bool(value)).lower()


def text(value):
    return '_' if value is None else value


def actions(value):
    if value is None:
        return '_'
    result = [value['S'][1]]
    children = value.get('Next', [])
    if isinstance(children, dict):
        children = [children]
    for child in children:
        result += actions(child).split(',')
    return ','.join(result)


def additional(d):
    return ','.join(k+':'+actions(v) for k, v in d.get('AA', {}).items()) or '_'


def field_value(value):
    if value is None:
        return '_'
    if isinstance(value, tuple):
        return '/'+value[1]
    if isinstance(value, list):
        return '['+','.join(value)+']'
    if isinstance(value, dict):
        return 'signature'
    return value


def expectation(objects):
    """Evaluate the authoring structures, without reading serialized PDF bytes."""
    form = objects[10]
    lines = ['form|'+ '|'.join([
        boolean(form.get('NeedAppearances')), str(form.get('SigFlags', 0)),
        boolean(form.get('XFA')), text(form.get('DA')), str(form.get('Q', 0)),
        str(len(form.get('CO', []))), boolean('DR' in form)])]
    fields, widgets = [], []
    seen = set()

    def dictionary(v):
        return objects[v[1]] if isinstance(v, tuple) else v

    def visit(v, parent, prefix, inherited):
        d = dictionary(v)
        token = v if isinstance(v, tuple) else id(v)
        if token in seen:
            return
        seen.add(token)
        is_widget = parent is not None and 'T' not in d
        if is_widget:
            widget(d, parent, False)
            return
        own = inherited | {k: d[k] for k in ('FT', 'Ff', 'V', 'DV', 'DA', 'Q', 'Opt', 'MaxLen') if d.get(k) is not None}
        children = [dictionary(k) for k in d.get('Kids', [])]
        group = any('T' in child for child in children)
        partial = d.get('T', '')
        qualified = prefix+'.'+partial if parent is not None and 'T' in d else partial if parent is None else prefix
        flags = own.get('Ff', 0)
        flag = lambda bit: boolean(flags & (1 << (bit-1)))
        ft = own.get('FT', name(''))[1]
        if group:
            kind = 'Group'
        elif ft == 'Btn':
            kind = 'Btn:'+('Push' if flags & (1<<16) else 'Radio' if flags & (1<<15) else 'CheckBox')+':'+','.join(own.get('Opt', []))
        elif ft == 'Tx':
            kind = 'Tx:'+str(own.get('MaxLen', '_'))+':'+':'.join(flag(b) for b in (13,14,25,26))+':'+boolean('RV' in d)
        elif ft == 'Ch':
            opts = [o if isinstance(o, list) else [o,o] for o in own.get('Opt', [])]
            kind = 'Ch:'+':'.join(flag(b) for b in (18,19,22))+':'+','.join('>'.join(o) for o in opts)
        elif ft == 'Sig':
            kind = 'Sig:'+boolean(own.get('V') is not None)
        else:
            kind = 'Other:'+ft
        index = len(fields)
        fields.append('|'.join(['field',str(index),str(parent) if parent is not None else '_',qualified,text(d.get('T')),kind,str(flags),
            '_' if group else field_value(own.get('V')), '_' if group else field_value(own.get('DV')),
            text(own.get('DA')),str(own.get('Q', 0)),text(d.get('TU')),text(d.get('TM')),additional(d)]))
        if d.get('Subtype') == name('Widget'):
            widget(d,index,True)
        for child in d.get('Kids', []):
            visit(child,index,qualified,own)

    def widget(d, index, merged):
        x0,y0,x1,y1 = d['Rect']
        normal = d.get('AP',{}).get('N', {})
        states = [n for n in normal if n != 'Off'] if isinstance(normal,dict) else []
        widgets.append('|'.join(['widget',str(index),boolean(merged),f'{min(x0,x1)},{min(y0,y1)},{max(x0,x1)},{max(y0,y1)}',
            ','.join(states) or '_', text(d.get('AS',(None,None))[1]), actions(d.get('A')),'_' if merged else additional(d)]))

    inherited = {k:form[k] for k in ('DA','Q') if k in form}
    for root in form.get('Fields', []):
        visit(root,None,'',inherited)
    return '\n'.join(lines+fields+widgets)+'\n'


def vector(identifier, objects, *, repair='', error='', expected=None, requirements=('forms-model',)):
    from .document import Builder, navigation_value
    builder = Builder()
    def appearances(value) -> Any:
        if isinstance(value,dict):
            return {k:(v.encode() if k=='DA' and isinstance(v,str) else appearances(v)) for k,v in value.items()}
        if isinstance(value,list):return [appearances(v) for v in value]
        return value
    for number, value in sorted(appearances(objects).items()):
        builder.obj(number, value if isinstance(value, bytes) else navigation_value(value))
    return Vector(identifier, {'.pdf':builder.classic()[0],
        '.expected': (expectation(objects) if expected is None and not error else expected or '').encode(),
        '.params':f'repair={repair}\nerror={error}\n'.encode()},
        requirements, 'Independent read-only interactive form model, ISO 32000-2 12.7.')


def base():
    from .document import navigation_base
    objects = navigation_base()
    objects[1]['AcroForm'] = ref(10)
    objects[10] = {'Fields':[ref(11)], 'DA':'/F1 10 Tf', 'DR':{'Font':{'F1':ref(90)}}}
    objects[90] = {'Type':name('Font'),'Subtype':name('Type1'),'BaseFont':name('Helvetica')}
    objects[11] = {'T':'entry','FT':name('Tx'),'V':'value'}
    return objects


def cases():
    objects=base()
    objects[10] |= {'Fields':[ref(11),ref(20),ref(21),ref(22),ref(23),ref(24),ref(25),ref(26),ref(27),ref(28)],
        'NeedAppearances':True,'SigFlags':3,'CO':[ref(14)],'Q':1,'XFA':['template',ref(91)]}
    objects[91]=b'<< /Length 0 >>\nstream\n\nendstream'
    objects[11]={'T':'Personal','FT':name('Tx'),'V':'inherited','DV':'default','Ff':1,'MaxLen':64,'DA':'/F1 9 Tf','Q':2,'Kids':[ref(12)]}
    objects[12]={'T':'Address','Parent':ref(11),'Kids':[ref(13)],'MaxLen':32,'Q':0}
    objects[13]={'T':'City €','Parent':ref(12),'Ff':1<<12,'Kids':[ref(14)],'TU':'Accessible','TM':'export-city'}
    objects[14]={'Parent':ref(13),'Subtype':name('Widget'),'Rect':[9,12,1,2],
        'AA':{'Fo':{'S':name('JavaScript'),'JS':b'never()'}}}
    objects[20]={'T':'submit','FT':name('Btn'),'Ff':1<<16,'Subtype':name('Widget'),'Rect':[0,0,5,5],
        'A':{'S':name('Named'),'N':name('NextPage'),'Next':{'S':name('JavaScript'),'JS':b'never()'}}}
    objects[21]={'T':'check','FT':name('Btn'),'V':name('Yes'),'DV':name('Off'),'Subtype':name('Widget'),'Rect':[0,0,5,5],
        'AP':{'N':{'Off':ref(91),'Yes':ref(91)}},'AS':name('Yes'),'Opt':['export-yes']}
    objects[22]={'T':'radio','FT':name('Btn'),'Ff':(1<<15)|(1<<25),'V':name('Shared'),'Kids':[ref(30),ref(31)],'Opt':['first','second']}
    for i in (30,31):
        objects[i]={'Parent':ref(22),'Subtype':name('Widget'),'Rect':[0,0,5,5],
            'AP':{'N':{'Off':ref(91),'Shared':ref(91)}},'AS':name('Shared')}
    objects[23]={'T':'password','FT':name('Tx'),'Ff':1<<13}
    objects[24]={'T':'comb','FT':name('Tx'),'Ff':1<<24,'MaxLen':8,'V':'123'}
    objects[25]={'T':'rich','FT':name('Tx'),'Ff':1<<25,'V':'plain','RV':'<p>plain</p>',
        'AA':{'K':{'S':name('JavaScript'),'JS':b'never()'},'C':{'S':name('Named'),'N':name('FirstPage')}}}
    objects[26]={'T':'combo','FT':name('Ch'),'Ff':(1<<17)|(1<<18),'V':'Blue','Opt':['Red',['b','Blue']]}
    objects[27]={'T':'list','FT':name('Ch'),'Ff':1<<21,'V':['Red','Blue'],'DV':['Blue'],'Opt':['Red',['b','Blue']]}
    objects[28]={'T':'signatures','Kids':[ref(32),ref(33)]}
    objects[32]={'T':'empty','Parent':ref(28),'FT':name('Sig')}
    objects[33]={'T':'signed','Parent':ref(28),'FT':name('Sig'),'V':{'Type':name('Sig'),'Filter':name('Example')}}
    yield vector('forms-types-inheritance',objects)

    original=base()
    original[10]['Fields']=[ref(11),ref(12)]
    original[12]=deepcopy(original[11])
    yield vector('forms-duplicate-equivalent',original)
    changed=deepcopy(original);changed[12]['V']='different'
    yield vector('forms-duplicate-conflict',changed,repair='form.duplicate-name')
    changed=base();changed[11]['T']='partial.period'
    yield vector('forms-name-period',changed,repair='form.name')
    changed=base();del changed[11]['T']
    yield vector('forms-name-absent',changed,repair='form.name')
    changed=base();changed[11]['Kids']=[ref(12)];changed[12]={'T':'child','FT':name('Tx')}
    yield vector('forms-parent-missing',changed,repair='form.parent')
    changed=base();changed[11]['Kids']=[ref(11)]
    yield vector('forms-cycle',changed,repair='form.cycle')
    changed=base();changed[11]['Kids']=[ref(12)];changed[12]={'T':'child','Parent':ref(11),'Kids':[ref(11)]}
    yield vector('forms-cycle-mutual',changed,repair='form.cycle')
    changed=base();changed[10]['Fields']=[changed[11]]
    yield vector('forms-direct-field',changed,repair='form.structure')
    changed=base();changed[11]['Kids']=[ref(12),ref(13)];changed[12]={'T':'child','Parent':ref(11)}
    changed[13]={'Parent':ref(11),'Subtype':name('Widget'),'Rect':[0,0,1,1]}
    yield vector('forms-mixed-children',changed,repair='form.structure')
    changed=base();changed[11]['Kids']=[ref(12)];changed[12]={'Parent':ref(11),'Rect':[0,0,1,1]}
    yield vector('forms-widget-subtype',changed,repair='form.structure')
    changed=base();changed[11]['AA']={'K':ref(12)};changed[12]={'S':name('JavaScript'),'JS':b'never()','Next':ref(12)}
    yield vector('forms-action-cycle',changed,repair='action.cycle',expected=expectation(base()).replace('|_|_|_\n','|_|_|K:JavaScript\n'))
    changed=base();changed[11]['T']=b'\x80\xa0';changed[11]['V']=b'\xfe\xff\x00Z'
    yield vector('forms-text-encodings',changed,expected=expectation(base()).replace('entry|entry','•€|•€').replace('|value|','|Z|'))
    changed=base();changed[11]['T']=b'\xff';changed[11]['V']=ref(91)
    changed[91]=b'<< /Length 6 >>\nstream\n\xfe\xff\x03\xa9\x00Z\nendstream'
    yield vector('forms-text-stream',changed,expected=expectation(base()).replace('entry|entry','ÿ|ÿ').replace('|value|','|ΩZ|'))
    changed=base();changed[10]|={'CO':None,'DR':None,'NeedAppearances':None,'XFA':None,'Q':None};changed[11]|={'Ff':None,'DV':None,'AA':None}
    yield vector('forms-optional-null',changed,expected='form|false|0|false|/F1 10 Tf|0|0|false\n'+expectation(base()).split('\n',1)[1])
    for suffix, change in [('field-type',lambda o:o[11].update(FT=17)),('choice-option',lambda o:o[11].update(FT=name('Ch'),Opt=[['a']])),('value-type',lambda o:o[11].update(V=17)),('kids-type',lambda o:o[11].update(Kids=17))]:
        changed=base();change(changed)
        yield vector('forms-invalid-'+suffix,changed,error='syntax')
    changed=base();changed[10]['Fields']=[ref(11),ref(11)]
    yield vector('forms-shared-field',changed,repair='form.cycle')
    changed=base()
    for i in range(11,61):
        changed[i]={'T':f'n{i}','FT':name('Tx')}
        if i>11:changed[i]['Parent']=ref(i-1)
        if i<60:changed[i]['Kids']=[ref(i+1)]
    yield vector('forms-deep',changed)

    changed=base();del changed[10]['Fields']
    yield vector('forms-fields-absent',changed,repair='form.fields',expected=expectation(base()).split('\n')[0]+'\n')
    changed=base();changed[11]['T']=17
    yield vector('forms-name-type',changed,repair='form.name',expected=expectation(base()).replace('entry|entry','|_'))
    changed=base();changed[11]['Kids']=[ref(12)];changed[12]={'Parent':ref(11),'FT':name('Tx'),'Subtype':name('Widget'),'Rect':[0,0,1,1]}
    yield vector('forms-widget-redundant-type',changed)

    changed=base();changed[11]={'T':'options','FT':name('Ch'),'Opt':['One',['two','Two']],'V':'Two','DV':'One','Kids':[ref(12)]}
    changed[12]={'T':'level','Parent':ref(11),'Opt':None,'Kids':[ref(13)]}
    changed[13]={'T':'leaf','Parent':ref(12),'V':'One'}
    yield vector('forms-choice-inheritance',changed)
    changed=base();changed[11]|={'Subtype':name('Widget'),'Rect':[0,0,4,5],
        'A':{'S':name('URI'),'URI':b'https://example.invalid/'},
        'AA':{k:{'S':name('JavaScript'),'JS':b'never()','Next':{'S':name('Named'),'N':name('LastPage')}} for k in ('K','F','V','C','E','X','D','U','Fo','Bl','PO','PC','PV','PI')}}
    changed[10]['CO']=[ref(11)]
    yield vector('forms-actions',changed,requirements=('forms-model','forms-actions'))
    changed=base();changed[10]['Fields']=[ref(i) for i in range(100,300)]
    for i in range(100,300):changed[i]={'T':f'field{i}','FT':name('Tx'),'V':'shared defaults'}
    yield vector('forms-wide',changed)
    changed=base();changed[10]['Fields']=[ref(11),ref(12)];changed[12]={'T':'entry','FT':name('Tx'),'V':'value','DV':'conflict'}
    yield vector('forms-duplicate-default-conflict',changed,repair='form.duplicate-name')
    changed=base();changed[11]['Kids']=[ref(12)];changed[12]={'T':'child','Parent':ref(90)}
    yield vector('forms-parent-wrong',changed,repair='form.parent')
    changed=base();changed[11]['AA']={'K':{'S':name('GoTo'),'D':[None,name('Fit')]}}
    yield vector('forms-invalid-action-target',changed,error='syntax')

    changed=base();changed[11]['V']=b'\xfe\xff\xd8\x00'
    yield vector('forms-text-invalid',changed,repair='string.invalid-text',expected=expectation(base()).replace('|value|','|�|'))
    changed=base();changed[10]['Fields']=[ref(11),ref(12)];changed[12]=deepcopy(changed[11])
    expected=expectation(changed);changed[12]['V']=b'value'
    yield vector('forms-duplicate-text-encodings',changed,expected=expected)

    changed=base();changed[11]['T']=''
    yield vector('forms-name-empty',changed)
    changed=base();changed[11]['T']='';changed[11]['Kids']=[ref(12)];changed[12]={'T':'child','Parent':ref(11)}
    yield vector('forms-name-empty-parent',changed)
