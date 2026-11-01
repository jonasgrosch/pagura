"""Independent source-only controls for checked CID plus descriptor recovery.

Frozen before any selected candidate execution. Collection containers are
independently authored source tables; supplied H contours and locked Tinos H
straight contours supply pixels without any Pagura execution.
"""
from pathlib import Path

from . import font_collections as collections
from .model import Case, RENDER, UNSUPPORTED
from .paint import bundled_oracle
from .pdf import opaque_rectangles, stream
from .recovery_font_descriptor import pdf, records

AREA = 'selected_font_interactions'
REQUIREMENTS = ('text-descriptor-key-recovery', 'text-checked-cid-substitution')


def mixed(*, alias_space=False, cid_missing=False, alias_missing=False,
          invisible=False, later=False, bbox=False):
    r = records(kind='empty-space' if alias_space else 'windows',
                bbox=b'[0 -200 400 800]' if bbox else None)
    r[3] = r[3].replace(b'/F 5 0 R', b'/F 5 0 R /C 20 0 R').replace(b'32 32', b'64 64')
    r[20] = (b'<< /Type /Font /Subtype /Type0 /BaseFont /MinionPro-Regular '
             b'/Encoding 23 0 R /DescendantFonts [21 0 R] /ToUnicode 24 0 R >>')
    r[21] = (b'<< /Type /Font /Subtype /CIDFontType2 /BaseFont /MinionPro-Regular '
             b'/CIDSystemInfo << /Registry (Adobe) /Ordering (Identity) /Supplement 0 >> '
             b'/FontDescriptor 22 0 R /DW 877 /W [29 [1000] 73 [423]] >>')
    r[22] = (b'<< /Type /FontDescriptor /FontName /MinionPro-Regular /Flags 6 '
             b'/FontBBox [-200 -300 1200 1000] /ItalicAngle 0 /Ascent 800 '
             b'/Descent -200 /CapHeight 700 /StemV 80 >>')
    r[23] = stream(b'1 begincodespacerange <0000> <ffff> endcodespacerange '
                   b'2 begincidchar <0101> 29 <0202> 73 endcidchar')
    r[24] = stream(b'1 begincodespacerange <0000> <ffff> endcodespacerange '
                   b'2 beginbfchar <0101> <0048> <0202> <0020> endbfchar')
    cid = b'<0303>' if cid_missing else b'<0101>'
    alias = b'( )' if alias_space else b'(AB)' if alias_missing else b'(AA)'
    content = (b'BT /C 24 Tf 1 0 0 1 3 8 Tm '+cid+b' Tj ET '
               b'BT /F 20 Tf 1 0 0 1 40 4 Tm '+(b'3 Tr ' if invisible else b'')+
               alias+b' Tj ET 0 1 0 rg 60 2 4 4 re f')
    r[4] = stream(content)
    if later:
        r[2] = b'<< /Type /Pages /Kids [10 0 R 3 0 R] /Count 2 >>'
        r[10] = (b'<< /Type /Page /Parent 2 0 R /MediaBox [0 0 64 64] '
                 b'/Resources << /Font << /F 5 0 R /C 20 0 R >> >> /Contents 11 0 R >>')
        r[11] = stream(b'0 0 1 rg 0 0 64 64 re f')
    return r


def mixed_oracle(alias_space):
    root = Path(__file__).resolve().parents[2]
    cid = bundled_oracle(root/'data/fonts/Tinos-Regular.ttf', root/'data/afm/Times-Roman.afm',
                         'H', advance=1000, size=64)
    header = b'P6\n64 64\n255\n'
    assert cid.startswith(header)
    shapes = [((60, 2, 4, 4), (0, 255, 0))]
    if not alias_space:
        for x in (40, 50):
            shapes.extend([((x,4,2,12),(0,0,0)), ((x+6,4,2,12),(0,0,0)),
                           ((x+2,9,4,2),(0,0,0))])
    supplied = opaque_rectangles(64,64,shapes)
    # Supports are separated in x: CID H stays left of x32; alias starts x40.
    a, b = cid[len(header):], supplied[len(header):]
    assert all(a[i] == 255 or b[i] == 255 for i in range(len(a)))
    return header+bytes(min(x,y) for x,y in zip(a,b))


def cases():
    for name, options in [
        ('mixed-visible', {}), ('empty-nonzero-alias-space', {'alias_space':True}),
        ('source-widths-bbox', {'bbox':True}),
        ('after-visible-alias-missing', {'alias_missing':True}),
        ('invisible-alias-missing', {'alias_missing':True,'invisible':True}),
        ('later-first-alias-missing', {'alias_missing':True,'later':True}),
        ('later-first-alias-valid', {'later':True}),
        ('cid-hole-with-empty-alias', {'alias_space':True,'cid_missing':True}),
    ]:
        failure = options.get('alias_missing') or options.get('cid_missing')
        page = int(bool(options.get('later')))
        extra = {'scan_recovery':'repair', 'page':page, 'tolerance':1,
                 'mode_errors':{'strict':{'kind':'Unsupported','feature':'font.non-embedded',
                                          'repair':None}}}
        if options.get('bbox'):
            extra['selected_font_geometry'] = {'alias_quads':[[40,0,50,0,50,20,40,20],
                                                               [50,0,60,0,60,20,50,20]],
                                              'cid_advance':24}
        yield Case(name,pdf(mixed(**options)),UNSUPPORTED if failure else RENDER,
                   REQUIREMENTS, 'Immutable RequiredNamed and checked CID policies coexist; source widths remain authoritative.',
                   oracle=None if failure else mixed_oracle(options.get('alias_space',False)),
                   repairs=() if options.get('cid_missing') else ('font.descriptor-key',),
                   information=('font.substituted',), strict_valid=False, pages=2 if page else 1,
                   feature=('font.non-embedded' if options.get('cid_missing') else 'font.descriptor-glyph') if failure else None,
                   extra=extra)
    # Canonical TTC uses the existing unique-name PDF policy. Alias recovery
    # conservatively refuses every TTC: no new native default-face selection.
    faces = [collections.face('glyf',i) for i in (0,1)]
    for name, source, selected, alias in [
        ('canonical-ttc-second',faces,1,False),
        ('alias-ttc-single',faces[:1],0,True),
        ('alias-ttc-pair-first',faces,0,True),
        ('alias-ttc-pair-second',faces,1,True),
        ('alias-ttc-ambiguous', [faces[0], faces[0]],0,True),
    ]:
        data = collections.container(source,1)
        fonts = collections.objects(data,'glyf',False,collections.NAMES[selected])
        r = {1:b'<< /Type /Catalog /Pages 2 0 R >>',
             2:b'<< /Type /Pages /Kids [3 0 R] /Count 1 >>',
             3:b'<< /Type /Page /Parent 2 0 R /MediaBox [0 0 64 64] /Resources << /Font << /F 5 0 R >> >> /Contents 4 0 R >>',
             4:stream(b'BT /F 50 Tf 1 0 0 1 4 4 Tm (A) Tj ET'),
             5:fonts[0].replace(b'/FontDescriptor',b'/GontDescriptor') if alias else fonts[0],
             6:fonts[1],7:fonts[2]}
        yield Case(name,pdf(r),UNSUPPORTED if alias else RENDER,
                   ('text-descriptor-key-recovery','font-truetype'),
                   'Alias TTC cannot guess native face zero; canonical unique second-name policy is retained.',
                   oracle=None if alias else opaque_rectangles(64,64,[((4,4,10,20),(0,0,0))]),
                   repairs=() if alias else ('font.collection-face',), strict_valid=False,
                   feature='font.non-embedded' if alias else None,
                   extra={'scan_recovery':'repair','tolerance':0,
                          'mode_errors':{'strict':{'kind':'Unsupported' if alias else 'Syntax',
                                                    'feature':'font.non-embedded' if alias else None,
                                                    'repair':None if alias else 'font.collection-face'}}})
