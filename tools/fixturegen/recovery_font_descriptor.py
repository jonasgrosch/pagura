"""Independent supplied glyph/advance oracles for narrow descriptor recovery.

Frozen before candidate execution. TrueType source bytes and H coverage use
the independent fixture encoders; no renderer supplies expected output.
"""
import struct
from .font import simple, truetype
from .pdf import stream, opaque_rectangles
from .model import Case, RENDER, OPEN, UNSUPPORTED

u16=lambda x:struct.pack(">H",x)
u32=lambda x:struct.pack(">I",x)

AREA = "recovery_font_descriptor"
REQUIREMENT = "text-descriptor-key-recovery"

def authored_case(name, data, wanted, strict, oracle=None, tags=(), note="", page=0):
    extra = {"scan_recovery": "repair", "tolerance": 0, "page": page}
    if strict != "render":
        contract = {"kind": strict.title(), "repair": None}
        if strict == "unsupported":
            contract.update(feature="font.non-embedded", message="font has no safe available substitute")
        extra["mode_errors"] = {"strict": contract}
    if wanted not in ("render", "unsupported"):
        extra["render_error"] = wanted
    if name in ("selection-strict-used-glyph", "selection-strict-invisible-glyph"):
        extra.setdefault("mode_errors", {})["repair"] = {"kind": "Syntax", "repair": "font.glyph-data"}
    if name == "selection-descriptor-bbox-witness":
        extra["text_quads"] = [[[4,0],[14,0],[14,20],[4,20]], [[14,0],[24,0],[24,20],[14,20]]]
    if name in ("selection-descriptor-bbox-witness", "selection-canonical-descriptor-bbox-witness"):
        extra["text_geometry_contract"] = {"quads": [[4,0,14,0,14,20,4,20], [14,0,24,0,24,20,14,20]]}
    elif name == "selection-legacy-invalid-bbox":
        extra["text_geometry_contract"] = {"quads": [[4,4,14,4,14,24,4,24], [14,4,24,4,24,24,14,24]]}
    elif name in ("selection-invalid-supplied-bbox", "selection-cyclic-supplied-bbox"):
        extra["text_geometry_contract"] = {"error_kind": "Syntax" if name == "selection-invalid-supplied-bbox" else "Limit"}
    return Case(name, data, RENDER if wanted == "render" else UNSUPPORTED if wanted == "unsupported" else OPEN,
                (REQUIREMENT,), note or name.replace("-", " "), oracle=oracle,
                repairs=tuple(tags), strict_valid=strict == "render", pages=2 if page else 1,
                feature=("font.descriptor-glyph" if tags else "font.non-embedded") if wanted == "unsupported" else None,
                extra=extra)

def pdf(records):
 out=bytearray(b'%PDF-1.7\n%\xe2\xe3\xcf\xd3\n');offsets={}
 for n,body in sorted(records.items()):offsets[n]=len(out);out+=f'{n} 0 obj\n'.encode()+body+b'\nendobj\n'
 start=len(out);count=max(records)+1;out+=f'xref\n0 {count}\n0000000000 65535 f \n'.encode()
 for n in range(1,count):out+=(f'{offsets[n]:010} 00000 n \n'.encode() if n in offsets else b'0000000000 00000 f \n')
 return bytes(out+f'trailer\n<< /Size {count} /Root 1 0 R >>\nstartxref\n{start}\n%%EOF\n'.encode())
def cmap4(code,gid):
 # Two complete segments including the required sentinel; modulo delta is exact.
 return u16(4)+u16(32)+u16(0)+u16(4)+u16(4)+u16(1)+u16(0)+u16(code)+u16(65535)+u16(0)+u16(code)+u16(65535)+u16((gid-code)%65536)+u16(1)+u16(0)+u16(0)
def cmap_records(records):
 start=4+8*len(records);header=u16(0)+u16(len(records));body=b''
 for platform,encoding,sub in records:header+=u16(platform)+u16(encoding)+u32(start+len(body));body+=sub
 return header+body
def post_name(name):
 # Five source glyphs; custom glyph1 name only. Other names are irrelevant holes.
 names=[name,b'curve',b'composite',b'attached']
 return u32(0x20000)+bytes(28)+u16(5)+u16(0)+b''.join(u16(258+i) for i in range(4))+b''.join(bytes([len(n)])+n for n in names)
points=[(0,0,True),(100,0,True),(100,250,True),(300,250,True),(300,0,True),(400,0,True),(400,600,True),(300,600,True),(300,350,True),(100,350,True),(100,600,True),(0,600,True)]
glyph=simple(points);glyph=glyph[:2]+struct.pack('>hhhh',0,0,400,600)+glyph[10:]
def program(kind='windows'):
 cm=cmap_records([(3,1,cmap4(65,1))]);post=post_name(b'A')
 if kind=='byte-trap':cm=cmap_records([(3,0,cmap4(65,1)),(3,1,cmap4(65,1))])
 if kind=='post-B':post=post_name(b'B')
 if kind=='mac-B':
  codes=bytearray(256);codes[66]=1;cm=cmap_records([(1,0,u16(0)+u16(262)+u16(0)+bytes(codes))]);post=u32(0x30000)+bytes(28)
 if kind=='gid-zero':cm=cmap_records([(3,1,cmap4(65,0))]);post=u32(0x30000)+bytes(28)
 selected=glyph
 if kind=='broken-glyph':selected=struct.pack('>hhhhh',1,0,0,400,600)+u16(11)+u16(0)
 if kind=='empty-space':selected=b'';cm=cmap_records([(3,1,cmap4(32,1))]);post=post_name(b'space')
 value=truetype(glyph_override=selected,table_overrides={b'cmap':cm,b'post':post})
 if kind=='bad-version':value=b'\x00\x02\x00\x00'+value[4:]
 return value
def rects(shown='AA'):
 rectangles=[((24,2,4,4),(0,255,0))]
 for index,_ in enumerate(shown):
  x=4+10*index;rectangles += [((x,4,2,12),(0,0,0)),((x+6,4,2,12),(0,0,0)),((x+2,9,4,2),(0,0,0))]
 return rectangles
def records(*,kind='windows',canonical=False,encoding=b'/WinAnsiEncoding',flags=32,text=b'(AA) Tj',tounicode=False,extra=b'',form=False,later=False,bad_length=None,bad_widths=False,bbox=None):
 p=program(kind);key=b'FontDescriptor' if canonical else b'GontDescriptor';enc=b' /Encoding '+encoding if encoding else b''
 font=b'<< /Type /Font /Subtype /TrueType /BaseFont /IndependentH /FirstChar 65 /LastChar 66 /Widths [500 500]'+enc+b' /'+key+b' 6 0 R'+(b' /ToUnicode 9 0 R' if tounicode else b'')+b' >>'
 desc=b'<< /Type /FontDescriptor /FontName /IndependentH /Flags '+str(flags).encode()+b' /FontBBox [0 0 400 600] /ItalicAngle 0 /Ascent 600 /Descent 0 /CapHeight 600 /StemV 100 /FontFile2 7 0 R >>'
 content=b'BT /F 20 Tf 1 0 0 1 4 4 Tm '+extra+text+b' ET 0 1 0 rg 24 2 4 4 re f'
 r={1:b'<< /Type /Catalog /Pages 2 0 R >>',2:b'<< /Type /Pages /Kids [3 0 R] /Count 1 >>',3:b'<< /Type /Page /Parent 2 0 R /MediaBox [0 0 32 32] /Resources << /Font << /F 5 0 R >> /XObject << /X 8 0 R >> >> /Contents 4 0 R >>',4:stream(content),5:font,6:desc,7:stream(p,b'/Length1 '+str(len(p)).encode())}
 if bbox is not None:r[6]=r[6].replace(b'/FontBBox [0 0 400 600]',b'/FontBBox '+bbox)
 if bbox==b'9 0 R':r[9]=b'9 0 R'
 if bad_widths:r[5]=r[5].replace(b'/Widths [500 500]',b'/Widths [500]')
 if bad_length=='decoded':r[7]=stream(p,b'/Length1 '+str(len(p)+1).encode())
 if bad_length=='encoded':r[7]=r[7].replace(b'/Length '+str(len(p)).encode(),b'/Length '+str(len(p)-1).encode(),1)
 if kind=='empty-space':r[5]=r[5].replace(b'/FirstChar 65 /LastChar 66 /Widths [500 500]',b'/FirstChar 32 /LastChar 32 /Widths [500]')
 if tounicode:r[9]=stream(b'/CIDInit /ProcSet findresource begin 12 dict begin begincmap /CIDSystemInfo << /Registry (Author) /Ordering (UCS) /Supplement 0 >> def /CMapName /Author def /CMapType 2 def 1 begincodespacerange <00> <FF> endcodespacerange 2 beginbfchar <41> <0041> <42> <0041> endbfchar endcmap CMapName currentdict /CMap defineresource pop end end')
 if form:
  r[4]=stream(b'BT /F 20 Tf 1 0 0 1 4 4 Tm (A) Tj ET /X Do');r[8]=stream(b'BT /F 20 Tf 1 0 0 1 14 4 Tm (B) Tj ET',b'/Type /XObject /Subtype /Form /BBox [0 0 32 32] /Resources << /Font << /F 5 0 R >> >>')
 if later:
  r[2]=b'<< /Type /Pages /Kids [10 0 R 3 0 R] /Count 2 >>';r[10]=b'<< /Type /Page /Parent 2 0 R /MediaBox [0 0 32 32] /Resources << /Font << /F 5 0 R >> >> /Contents 11 0 R >>';r[11]=stream(b'BT /F 20 Tf 1 0 0 1 4 4 Tm (AA) Tj ET 0 1 0 rg 24 2 4 4 re f')
 return r

def descriptor_cases():
    result = []
    def save(_family, name, data, *, status, strict, oracle=None, tags=(), note=''):
        result.append(authored_case('descriptor-' + name, data, status, strict, oracle, tags, note))
    # H has two stems and one crossbar, wholly explicit supplied TrueType contours.
    points = [(0,0,True),(100,0,True),(100,250,True),(300,250,True),(300,0,True),(400,0,True),
              (400,600,True),(300,600,True),(300,350,True),(100,350,True),(100,600,True),(0,600,True)]
    glyph = simple(points)
    import struct
    glyph = glyph[:2] + struct.pack('>hhhh',0,0,400,600) + glyph[10:]
    font_program = truetype(glyph_override=glyph)
    descriptor = b'<< /Type /FontDescriptor /FontName /IndependentH /Flags 32 /FontBBox [0 0 400 600] /ItalicAngle 0 /Ascent 600 /Descent 0 /CapHeight 600 /StemV 100 /FontFile2 7 0 R >>'
    font = b'<< /Type /Font /Subtype /TrueType /BaseFont /IndependentH /FirstChar 65 /LastChar 65 /Widths [500] /Encoding /WinAnsiEncoding KEY 6 0 R >>'
    font_base = {1:b'<< /Type /Catalog /Pages 2 0 R >>',2:b'<< /Type /Pages /Kids [3 0 R] /Count 1 >>',
        3:b'<< /Type /Page /Parent 2 0 R /MediaBox [0 0 32 32] /Resources << /Font << /F 5 0 R >> >> /Contents 4 0 R >>',
        4:stream(b'BT /F 20 Tf 1 0 0 1 4 4 Tm (AA) Tj ET 0 1 0 rg 24 2 4 4 re f'),5:font,6:descriptor,
        7:stream(font_program,b'/Length1 %d' % len(font_program))}
    rectangles = [((24,2,4,4),(0,255,0))]
    for x in (4,14): rectangles += [((x,4,2,12),(0,0,0)),((x+6,4,2,12),(0,0,0)),((x+2,9,4,2),(0,0,0))]
    font_oracle = opaque_rectangles(32,32,rectangles)
    for name, key, change, wanted in [
        ('canonical',b'/FontDescriptor',None,'render'), ('alias',b'/GontDescriptor',None,'render'),
        ('canonical-null',b'/GontDescriptor',('font',b'/FontDescriptor null'),'unsupported'),
        ('canonical-malformed',b'/GontDescriptor',('font',b'/FontDescriptor true'),'syntax'),
        ('wrong-name',b'/GontDescriptor',('name',b'Other'),'unsupported'),
        ('wrong-case',b'/GontDescriptor',('name',b'independentH'),'unsupported'),
        ('wrong-subset',b'/GontDescriptor',('name',b'ABCDEF+IndependentH'),'unsupported'),
        ('wrong-type',b'/GontDescriptor',('type',b'Other'),'unsupported'),
        ('missing-type',b'/GontDescriptor',('missing-type',b''),'unsupported'),
        ('missing-program',b'/GontDescriptor',('missing-program',b''),'unsupported'),
        ('program-not-stream',b'/GontDescriptor',('program',b'<< >>'),'unsupported'),
        ('malformed-program',b'/GontDescriptor',('program',stream(b'not a font')),'unsupported'),
        ('wrong-subtype',b'/GontDescriptor',('subtype',b'Type1'),'unsupported'),
        ('different-key',b'/FontDescript0r',None,'unsupported'),
        ('later-error',b'/GontDescriptor',('later',b' /Missing Do'),'syntax'),
    ]:
        r = dict(font_base); r[5] = font.replace(b'KEY',key)
        if change:
            kind, value = change
            if kind == 'font': r[5] = r[5][:-2]+b' '+value+b' >>'
            elif kind == 'name': r[6] = descriptor.replace(b'/IndependentH',b'/'+value)
            elif kind == 'type': r[6] = descriptor.replace(b'/Type /FontDescriptor',b'/Type /'+value)
            elif kind == 'missing-type': r[6] = descriptor.replace(b'/Type /FontDescriptor',b'')
            elif kind == 'missing-program': r[6] = descriptor.replace(b'/FontFile2 7 0 R',b'')
            elif kind == 'program': r[7] = value
            elif kind == 'subtype': r[5] = r[5].replace(b'/Subtype /TrueType',b'/Subtype /'+value)
            elif kind == 'later': r[4] = stream(b'BT /F 20 Tf 1 0 0 1 4 4 Tm (AA) Tj ET'+value)
        alias = key == b'/GontDescriptor' and name not in ('canonical-null','canonical-malformed')
        save('gont-descriptor',name,pdf(r),status=wanted,strict='unsupported' if alias else wanted,
             oracle=font_oracle if wanted == 'render' else None,
             tags=('font.descriptor-key',) if name in ('alias','later-error') else (),
             note='Exact descriptor key recovery only after original unavailable; actual supplied H, source advance500 vs sfnt advance600, no candidate-derived oracle.')

    return result

def selection_cases():
    result = []
    def save(name, options, wanted, shown='AA', note=''):
        refused_load = {'raw-absent-encoding','raw-symbolic','ambiguous-flags','strict-sfnt-version',
                        'decoded-length-mismatch','encoded-length-mismatch','unknown-encoding',
                        'unknown-base-encoding','absent-base-encoding','mismatched-widths'}
        canonical = options.get('canonical', False)
        tags = () if canonical or name in refused_load else ('font.descriptor-key',)
        result.append(authored_case('selection-' + name, pdf(records(**options)), wanted,
                                    'render' if canonical else 'unsupported',
                                    opaque_rectangles(32,32,rects(shown)) if wanted == 'render' else None,
                                    tags, note, 1 if options.get('later') else 0))
    B=b'<< /BaseEncoding /WinAnsiEncoding /Differences [65 /B] >>'
    for name,options,wanted,shown in [
     ('named-windows-holes',{},'render','AA'),('named-post-B',{'kind':'post-B','encoding':B},'render','AA'),('named-mac-B',{'kind':'mac-B','encoding':B},'render','AA'),
     ('encoding-vs-symbol-byte',{'kind':'byte-trap','encoding':B},'unsupported',''),('encoding-vs-symbol-byte-tounicode',{'kind':'byte-trap','encoding':B,'tounicode':True},'unsupported',''),('legacy-encoding-byte-fallback',{'kind':'byte-trap','encoding':B,'canonical':True},'render','AA'),
     ('actually-missing',{'text':b'(B) Tj'},'unsupported',''),('explicit-gid-zero',{'kind':'gid-zero'},'unsupported',''),('missing-tounicode-cannot-rescue',{'text':b'(B) Tj','tounicode':True},'unsupported',''),
     ('invisible-missing',{'text':b'(B) Tj','extra':b'3 Tr '},'unsupported',''),('visible-prefix-same-string',{'text':b'(AB) Tj'},'unsupported',''),('visible-prefix-separate-strings',{'text':b'(A) Tj (B) Tj'},'unsupported',''),('visible-prefix-TJ',{'text':b'[(A) 0 (B)] TJ'},'unsupported',''),('form-after-prefix',{'form':True},'unsupported',''),('later-shared-font',{'later':True,'text':b'(AB) Tj'},'unsupported',''),
     ('empty-unselected-holes',{'text':b'() Tj'},'render',''),('raw-absent-encoding',{'encoding':None},'unsupported',''),('raw-symbolic',{'flags':4,'kind':'byte-trap'},'unsupported',''),('ambiguous-flags',{'flags':36},'unsupported',''),('legacy-raw-absent-encoding',{'encoding':None,'canonical':True},'render','AA'),('legacy-raw-symbolic',{'flags':4,'kind':'byte-trap','canonical':True},'render','AA')]:
     save(name,options,wanted,shown,'Independent source H outlines and PDF advance500 versus sfnt600. Alias RequiredNamed never consults ToUnicode or literal-byte/symbol fallback; canonical legacy controls remain separate.')
    save('strict-sfnt-version',{'kind':'bad-version'},'unsupported',note='No repaired sfnt program may justify descriptor identity.')
    save('strict-used-glyph',{'kind':'broken-glyph'},'syntax',note='Complete structure with truncated glyph point data must refuse under original strict outline policy, never repair to empty.')
    save('strict-invisible-glyph',{'kind':'broken-glyph','extra':b'3 Tr '},'syntax',note='Invisible selected malformed glyph is still validated before decoding succeeds.')
    save('decoded-length-mismatch',{'bad_length':'decoded'},'unsupported',note='Whole decoded length must equal original Length1, no reported length repair may supply alias proof.')
    save('encoded-length-mismatch',{'bad_length':'encoded'},'unsupported',note='Corrected encoded extent cannot supply original complete program proof.')
    save('valid-empty-nonzero-space',{'kind':'empty-space','text':b'( ) Tj'},'render','',note='Strict-valid empty glyph1 selected by source space mapping is legitimate; Widths32=500 remains source authoritative.')
    save('unknown-encoding',{'encoding':b'/InferredEncoding'},'unsupported',note='Unknown encoding cannot be repaired to a guessed default for alias proof.')
    save('unknown-base-encoding',{'encoding':b'<< /BaseEncoding /InferredEncoding /Differences [65 /A] >>'},'unsupported',note='A supplied Difference does not authorize an unknown base.')
    save('absent-base-encoding',{'encoding':b'<< /Differences [65 /A] >>'},'unsupported',note='This narrow alias lane conservatively requires an explicit known base.')
    save('mismatched-widths',{'bad_widths':True},'unsupported',note='No Widths-length repair may supply source metrics for alias proof.')
    save('source-encoding-hole',{'encoding':b'<< /BaseEncoding /MacExpertEncoding >>'},'unsupported',note='MacExpert code65 is undefined; no StandardEncoding hole fill for recovered descriptor.')
    save('legacy-encoding-hole',{'encoding':b'<< /BaseEncoding /MacExpertEncoding >>','canonical':True},'render','AA',note='Existing canonical legacy StandardEncoding hole fill is preserved independently.')
    save('partial-source-glyph-name',{'encoding':b'<< /BaseEncoding /WinAnsiEncoding /Differences [65 /A_unknown] >>'},'unsupported',note='Unknown glyph-name components cannot be dropped to infer a scalar in this alias lane.')
    save('legacy-partial-glyph-name',{'encoding':b'<< /BaseEncoding /WinAnsiEncoding /Differences [65 /A_unknown] >>','canonical':True},'render','AA',note='The ordinary legacy Unicode-name policy remains unchanged.')
    for name,glyphname,wanted in [('uni-scalar',b'uni0041','render'),('u-scalar',b'u0041','render'),('u-leading-zero-scalar',b'u000041','render'),('glyph-name-suffix',b'A.alt','unsupported'),('multi-scalar-name',b'uni00410042','unsupported'),('surrogate-name',b'uniD800','unsupported'),('out-of-range-name',b'u110000','unsupported'),('short-uni-name',b'uni041','unsupported')]:
     save(name,{'encoding':b'<< /BaseEncoding /WinAnsiEncoding /Differences [65 /'+glyphname+b'] >>'},wanted,'AA',note='Whole exact AGL name or full scalar uni/u form only; no suffix/component deletion or partial decoding.')
    save('legacy-glyph-name-suffix',{'encoding':b'<< /BaseEncoding /WinAnsiEncoding /Differences [65 /A.alt] >>','canonical':True},'render','AA',note='Ordinary legacy suffix decomposition is preserved.')
    save('descriptor-bbox-witness',{'bbox':b'[0 -200 400 800]'},'render','AA',note='Source descriptor y[-200,800] differs from program headBBox and painted H; sourceWidth500 gives text quads x4..14 and14..24,y0..20.')
    save('canonical-descriptor-bbox-witness',{'bbox':b'[0 -200 400 800]','canonical':True},'render','AA',note='Canonical selected sourceBBox/Widths counterpart; programBBox must not replace supplied descriptor.')
    save('invalid-supplied-bbox',{'bbox':b'true'},'render','AA',note='RGB uses proven glyphs; text-only supplied malformedBBox must refuse rather than guessing0..1.')
    save('cyclic-supplied-bbox',{'bbox':b'9 0 R'},'render','AA',note='RGB remains glyph-defined; text-only BBox reference limit/cycle must propagate, not disappear through .ok.')
    save('legacy-invalid-bbox',{'bbox':b'true','canonical':True},'render','AA',note='Existing canonical optionalBBox fallback stays unchanged; no legacy geometry tightening.')
    return result

def cases():
    yield from descriptor_cases()
    yield from selection_cases()
