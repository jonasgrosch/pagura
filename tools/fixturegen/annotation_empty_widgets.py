"""Empty-widget geometry stays independent of unused text/XFA data.

Literal pixel-aligned borders and Type3 rectangles are specified independently
of the appearance program. Nonempty values still select the full form path.
"""
from .document import Builder, navigation_value
from .document_forms import name, ref
from .forms import base
from .model import Case, OPEN, RENDER, UNSUPPORTED
from .pdf import opaque_rectangles, stream

AREA = 'annotation_empty_widgets'
REQUIREMENT = 'content-source-widget-display'


def objects():
    o = base()
    o[3]['MediaBox'] = [0, 0, 32, 32]
    o[11]['Rect'] = [4, 4, 28, 24]
    o[11]['MK'] = {'BG': [0, 1, 0], 'BC': [1, 0, 0]}
    o[11]['BS'] = {'W': 1}
    return o


def pdf(o):
    b = Builder()
    for n, body in sorted(o.items()):
        b.obj(n, body if isinstance(body, bytes) else navigation_value(body))
    return b.classic()[0]


def frame():
    return opaque_rectangles(32, 32, [
        ((4,4,24,20),(0,255,0)), ((4,4,24,1),(255,0,0)),
        ((4,23,24,1),(255,0,0)), ((4,5,1,18),(255,0,0)),
        ((27,5,1,18),(255,0,0))])


def cases():
    for label, change in [
        ('missing-font', lambda o: (o[10].pop('DR'), o[11].update(DA=b'/Missing 10 Tf'))),
        ('wrong-da', lambda o: o[11].update(DA=name('Helv#2010#20Tf'))),
        ('unused-resources', lambda o: (o[10].update(DR=ref(90)), o.update({90:ref(90)}))),
        ('xfa', lambda o: o[10].update(XFA=b'inert XFA data')),
        ('no-scroll', lambda o: o[11].update(Ff=1<<23, DA=b'/F1 32 Tf')),
        ('null-value', lambda o: o[11].update(V=None)),
        ('empty-value', lambda o: o[11].update(V='')),
        ('signature', lambda o: o[11].update(FT=name('Sig'))),
    ]:
        o = objects(); change(o)
        yield Case(label, pdf(o), RENDER, (REQUIREMENT,),
                   'Empty Tx/Sig paints only the explicit MK frame; unused field text resources are not selected',
                   oracle=frame())
    o=objects();o[11].pop('MK');o[11].pop('BS');o[11].pop('T');o[11].pop('FT')
    o[11]|={'Parent':ref(12),'V':''};o[10]['Fields']=[ref(12)]
    o[12]={'T':'owner','FT':name('Tx'),'V':'AB','Kids':[ref(11)]}
    yield Case('parent-value-not-shadowed', pdf(o), RENDER, (REQUIREMENT,),
               'Unnamed child V is ignored: owning AB paints two5x7 Type3 glyphs at x6/12,y11',
               oracle=opaque_rectangles(32,32,[((6,11,5,7),(0,0,0)),((12,11,5,7),(0,0,0))]))
    for label, change, error in [
        ('missing-membership',lambda o:o[10].update(Fields=[]),'syntax'),
        ('repeated-membership',lambda o:o[10].update(Fields=[ref(11),ref(11)]),'cycle'),
        ('parent-cycle',lambda o:(o[11].update(Parent=ref(12)),o.update({12:{'Parent':ref(12),'FT':name('Tx')}})),'cycle'),
    ]:
        o=objects();change(o)
        yield Case(label,pdf(o),OPEN,(REQUIREMENT,),
                   'Empty content does not bypass missing/repeated membership or cyclic ancestry',
                   extra={'render_error':error})
    o=objects();o[1].pop('AcroForm');o[11]['Parent']=ref(12)
    for n in range(12,52):o[n]={'Parent':ref(n+1)} if n<51 else {}
    yield Case('legacy-long-ancestry',pdf(o),RENDER,(REQUIREMENT,),
               'A bounded40-parent legacy chain remains geometric; low traversal work must also bound its membership comparisons',
               oracle=frame())
    o=objects();o[11]['V']='AB';o[10]['XFA']=b'inert XFA data'
    yield Case('nonempty-xfa',pdf(o),UNSUPPORTED,(REQUIREMENT,),
               'A nonempty source value retains the existing XFA generation boundary',feature='form.xfa')
    o=objects();o[11]|={'V':'AB','AP':{'N':ref(80)},'DA':name('ignored')};o[10]|={'Fields':[],'XFA':b'inert'}
    o[80]=stream(b'0 0 1 rg 0 0 24 20 re f',b'/Type /XObject /Subtype /Form /BBox [0 0 24 20] /Resources << >>')
    yield Case('stored-precedence',pdf(o),RENDER,(REQUIREMENT,),
               'Stored AP paints blue before any source-field membership, DA, XFA or fit request',
               oracle=opaque_rectangles(32,32,[((4,4,24,20),(0,0,255))]))
