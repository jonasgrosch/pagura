"""Independent complete single-page JBIG2 wrapper controls (T.88 D.1/D.4).

Literal bitmap matrices precede encoding. Header/EOP/EOF are metadata only;
separate EOF1 cases pin that additional reported departure from T.88 7.3.2.
No product code is imported or executed.
"""
import struct

from . import jbig2
from .model import Case, OPEN, RENDER, Vector
from .pdf import opaque_rectangles, stream
from .recovery import page

AREA = 'recovery_jbig2_wrapper'
REQUIREMENT = 'jbig2-single-page-wrapper-recovery'
WRAPPER = 'jbig2.file-wrapper'
EOF_PAGE = 'jbig2.eof-page-association'
MAGIC = bytes.fromhex('974a42320d0a1a0a')
ROWS = [[int(x == y or 2 <= x < 8 and 2 <= y < 5) for x in range(11)] for y in range(7)]


def embedded(coding='arithmetic', *, long_page=False):
    width,height = len(ROWS[0]),len(ROWS)
    prefix = jbig2.segment(10,48,1,jbig2.page_info(width,height,flags=98),long_page=long_page)
    if coding == 'refinement':
        reference = [[int(x == y or x == 5) for x in range(width)] for y in range(height)]
        prefix += jbig2.segment(11,36,1,jbig2.region(reference,jbig2.arithmetic(reference,2)),retain=True)
        payload = jbig2.refinement_payload(ROWS,reference,1)
        return prefix+jbig2.segment(12,43,1,jbig2.region(ROWS,payload),refs=(11,),long_page=long_page)
    payload = jbig2.mmr(ROWS) if coding == 'mmr' else jbig2.arithmetic(ROWS,2)
    return prefix+jbig2.segment(11,38,1,jbig2.region(ROWS,payload),long_page=long_page,retain=True)


def wrapped(data, eof_page=0, *, end_page=True, end_file=True, flags=1, pages=1, long_page=False):
    header = MAGIC+bytes([flags])+(struct.pack('>I',pages) if flags&2 == 0 else b'')
    if end_page:data += jbig2.segment(900,49,1,b'',long_page=long_page)
    if end_file:data += jbig2.segment(901,51,eof_page,b'',long_page=long_page)
    return header+data


def rgba(rows, size=16):
    height = len(rows)
    clear = bytearray(size*size*4)
    for y,row in enumerate(rows):
        for x,bit in enumerate(row):
            at=((size-(2+height)+y)*size+2+x)*4
            clear[at:at+4]=bytes([0 if bit else 255]*3+[255])
    return bytes(clear)


def records(name, data, *, repairs=(), error=None, before_error=False, dimensions=None, globals=b'', raw_error=None):
    width,height = len(ROWS[0]),len(ROWS)
    rows = ROWS
    raw_expected = error if raw_error is None else None if raw_error == 'valid' else raw_error
    diagnostics = repairs or ((WRAPPER,) if before_error else ())
    params = (f'outcome={"syntax" if repairs or raw_expected else "valid"}\n'
              f'repair_outcome={raw_expected or "valid"}\nwidth={width}\nheight={height}\n'
              f'repairs={",".join(diagnostics)}\n')
    yield Vector('native-'+name,{'.jb2':data,'.globals':globals,'.out':b'' if raw_expected else jbig2.packed(rows),'.params':params.encode()},
                 (REQUIREMENT,),'Independent matrix and complete T.88 bytes; original strict refusal retained')
    declared_width,declared_height = dimensions or (width,height)
    dictionary = b'/Type /XObject /Subtype /Image /Width %d /Height %d /BitsPerComponent 1 /ColorSpace /DeviceGray /Filter /JBIG2Decode'%(declared_width,declared_height)
    if globals:dictionary += b' /DecodeParms << /JBIG2Globals 6 0 R >>'
    content = b'q %d 0 0 %d 2 2 cm /X Do Q'%(declared_width,declared_height)
    pdf = page(content,[stream(data,dictionary)]+([stream(globals)] if globals else []),b'<< /XObject << /X 5 0 R >> >>',size=16)
    extra = {'tolerance':0,'native_case':'native-'+name}
    if error:
        yield Case(name,pdf,OPEN,(REQUIREMENT,),'Malformed metadata/coding stays an explicit render refusal',
                   strict_valid=not before_error,extra={**extra,'render_error':error})
    else:
        rectangles = [((2+x,2+height-1-y,1,1),(0,0,0)) for y,row in enumerate(rows) for x,bit in enumerate(row) if bit]
        oracle = opaque_rectangles(16,16,rectangles)
        clear = rgba(rows)
        yield Case(name,pdf,RENDER,(REQUIREMENT,),'Exact independent bitmap; packaging repairs are separately reported',
                   oracle=oracle,repairs=repairs,strict_valid=not repairs,extra=extra)
        yield Vector('rgba-'+name,{'.white':b''.join(oracle.split(b'\n',3)[3][i:i+3]+b'\xff' for i in range(0,16*16*3,3)),'.clear':clear},
                     (REQUIREMENT,),'Independent premultiplied opaque-white and transparent-background RGBA')


def cases():
    for coding in ['arithmetic','mmr','refinement']:
        data = embedded(coding)
        yield from records(coding+'-embedded-control',data)
        for eof in [0,1]:
            yield from records(coding+'-file-eof'+str(eof),wrapped(data,eof),repairs=(WRAPPER,)+(EOF_PAGE,) * eof)
    data = embedded()
    yield from records('long-page-associations',wrapped(embedded(long_page=True),1,long_page=True),repairs=(WRAPPER,EOF_PAGE))
    glyph_body,symbols = jbig2.dictionary([ROWS],2)
    text,_ = jbig2.text_region(symbols,[(0,0,0)],11,7,complete_last_strip=True)
    global_prefix = jbig2.segment(0,0,0,glyph_body,retain=True)
    page_data = jbig2.segment(10,48,1,jbig2.page_info(11,7,flags=98))+jbig2.segment(11,6,1,text,refs=(0,))
    yield from records('internal-global-prefix',wrapped(global_prefix+page_data),repairs=(WRAPPER,))
    yield from records('external-globals',wrapped(page_data),repairs=(WRAPPER,),globals=global_prefix)
    # Recognition is deliberately limited to a declared single sequential page.
    for label,flags,pages in [('random-access',0,1),('unknown-page-count',3,1),('two-pages',1,2),('zero-pages',1,0),('reserved-header-bit',5,1)]:
        yield from records('hostile-'+label,wrapped(data,flags=flags,pages=pages),error='syntax')
    for label,broken in [('truncated-header',MAGIC+b'\x01\x00\x00')]:
        yield from records('hostile-'+label,broken,error='syntax')
    truncated_arithmetic = jbig2.segment(10,48,1,jbig2.page_info(11,7,flags=98))
    truncated_arithmetic += jbig2.segment(11,38,1,jbig2.region(ROWS,jbig2.arithmetic(ROWS,2)[:-2]))
    for label,broken in [
        ('missing-end-page',wrapped(data,end_page=False)),
        ('missing-end-file',wrapped(data,end_file=False)),
        ('trailing-bytes',wrapped(data)+b'\0'),
        ('duplicate-end-page',wrapped(data+jbig2.segment(800,49,1,b''))),
        ('end-page-before-info',wrapped(jbig2.segment(800,49,1,b'')+data)),
        ('region-after-end-page',wrapped(data+jbig2.segment(800,49,1,b'')+jbig2.segment(801,38,1,jbig2.region(ROWS,jbig2.mmr(ROWS))))),
        ('end-page-data',wrapped(data+jbig2.segment(900,49,1,b'\0'),end_page=False)),
        ('end-page-referral',wrapped(data+jbig2.segment(900,49,1,b'',refs=(11,)),end_page=False)),
        ('end-page-association',MAGIC+b'\x01'+struct.pack('>I',1)+data+jbig2.segment(900,49,0,b'')+jbig2.segment(901,51,0,b'')),
        ('end-file-data',wrapped(data,end_file=False)+jbig2.segment(901,51,0,b'\0')),
        ('end-file-referral',wrapped(data,end_file=False)+jbig2.segment(901,51,0,b'',refs=(11,))),
        ('end-file-other-page',wrapped(data,2)),
        ('second-page-info',wrapped(data+jbig2.segment(800,48,2,jbig2.page_info(11,7)))),
        ('late-global',wrapped(data+jbig2.segment(800,0,0,glyph_body))),
        ('missing-arithmetic-marker',wrapped(truncated_arithmetic)),
    ]:
        yield from records('hostile-'+label,broken,error='syntax',before_error=True)
    # Raw JBIG2 is valid; only the distinct PDF declaration is wrong.
    yield from records('hostile-pdf-dimension-mismatch',wrapped(data),repairs=(WRAPPER,),error='syntax',before_error=True,dimensions=(12,7),raw_error='valid')
