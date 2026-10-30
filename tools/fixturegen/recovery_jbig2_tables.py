"""Independent unused Huffman aggregation-selector controls, T.88 7.4.2.1.1.

Only bit7's unused selector is malformed. All referenced custom tables remain
encoded and valid; literal matrices, not any renderer, supply expected pixels.
"""
import struct

from . import jbig2
from .model import Case, OPEN, RENDER, Vector
from .pdf import opaque_rectangles, stream
from .recovery import page

AREA = 'recovery_jbig2_tables'
REQUIREMENT = 'recovery-jbig2-unused-aggregation'
REPAIR = 'jbig2.unused-aggregation-table'
GLYPH = [[1,0,1],[0,1,0],[1,1,1]]


def custom_table(oob=False, prefix=2):
    # HTPS=2, HTRS=3. The regular range0..7 has prefix00 and three
    # extra bits; lower/upper escapes are absent. Optional OOB is01.
    fields = format(prefix,'02b')+'011'+'00'+'00'+(format(prefix,'02b') if oob else '')
    return bytes([0x22+int(oob)])+struct.pack('>ii',0,8)+jbig2.bits_bytes(fields)


def placed(glyphs, placements, width, height):
    result = [[0]*width for _ in range(height)]
    for x,y,index in placements:
        for dy,row in enumerate(glyphs[index]):
            for dx,value in enumerate(row): result[y+dy][x+dx] |= value
    return result


def ordinary(*, unused=False, all_custom=False, global_dictionary=False,
             flag_extra=0, missing=False, extra=False, bad_table=None, non_table=False):
    if all_custom:
        tables = [custom_table(),custom_table(True),custom_table(),custom_table()]
        flags = 0x7d+0x80*unused+flag_extra
        # Independently encode DH=3, DW=3, OOB, raw bitmap size0.
        codes = '00011'+'00011'+'01'+'00000'
        body = struct.pack('>HII',flags,1,1)+jbig2.bits_bytes(codes)+jbig2.packed(GLYPH)
        body += jbig2.bits_bytes(jbig2.huffman_word(1,0)+jbig2.huffman_word(1,1))
        refs = tuple(range(1,5 if unused else 4))
    else:
        tables = [custom_table()]+([custom_table()] if extra else [])
        body,_ = jbig2.huffman_dictionary([GLYPH])
        flags = 1+0x80*unused+flag_extra
        body = struct.pack('>H',flags)+body[2:]
        refs = tuple(range(1,len(tables)+1)) if unused else ()
    if bad_table == 'reserved': tables[0] = bytes([tables[0][0]|0x80])+tables[0][1:]
    if bad_table == 'truncated': tables[0] = tables[0][:-1]
    if bad_table == 'oob': tables[0] = custom_table(True)
    if missing: refs = ()
    if non_table: refs = (0,)
    width,height = 8,6
    text,_ = jbig2._text_region([GLYPH],[(2,1,0)],width,height)
    expected = placed([GLYPH],[(2,1,0)],width,height)
    if global_dictionary:
        # Global references use segment0, with no page association.
        prefix = jbig2._segment(0,53,0,tables[0])
        refs = (0,) if unused else ()
        offset = len(prefix)+len(jbig2._segment(1,0,0,b'',refs=refs))
        globals = prefix+jbig2._segment(1,0,0,body,refs=refs)
        data = jbig2._segment(2,48,1,jbig2._page_info(width,height))+jbig2._segment(3,6,1,text,refs=(1,))
        return data,globals,expected,'globals',offset
    prefix = jbig2._segment(0,48,1,jbig2._page_info(width,height))
    for number,table in enumerate(tables,1):prefix += jbig2._segment(number,53,1,table)
    dictionary = len(tables)+1
    offset = len(prefix)+len(jbig2._segment(dictionary,0,1,b'',refs=refs))
    data = prefix+jbig2._segment(dictionary,0,1,body,refs=refs)
    data += jbig2._segment(dictionary+1,6,1,text,refs=(dictionary,))
    return data,b'',expected,'main',offset


def repeated(*, unused=False, both_streams=False, late_missing=False):
    second = [[1,1,1],[0,1,0],[1,0,1]]
    first_body,_ = jbig2.huffman_dictionary([GLYPH])
    second_body,_ = jbig2.huffman_dictionary([second])
    flags = 1+0x80*unused
    first_body = struct.pack('>H',flags)+first_body[2:]
    second_body = struct.pack('>H',flags)+second_body[2:]
    text,_ = jbig2._text_region([second],[(2,1,0)],8,6)
    rows = placed([second],[(2,1,0)],8,6)
    if both_streams:
        prefix = jbig2._segment(0,53,0,custom_table())
        refs = (0,) if unused else ()
        offset = len(prefix)+len(jbig2._segment(1,0,0,b'',refs=refs))
        globals = prefix+jbig2._segment(1,0,0,first_body,refs=refs)
        data = jbig2._segment(2,48,1,jbig2._page_info(8,6))
        data += jbig2._segment(3,0,1,second_body,refs=() if late_missing else refs)
        data += jbig2._segment(4,6,1,text,refs=(3,))
        return data,globals,rows,'globals',offset
    prefix = jbig2._segment(0,48,1,jbig2._page_info(8,6))+jbig2._segment(1,53,1,custom_table())
    refs = (1,) if unused else ()
    offset = len(prefix)+len(jbig2._segment(2,0,1,b'',refs=refs))
    data = prefix+jbig2._segment(2,0,1,first_body,refs=refs)
    data += jbig2._segment(3,0,1,second_body,refs=refs)+jbig2._segment(4,6,1,text,refs=(3,))
    return data,b'',rows,'main',offset


def used_aggregation(missing=False):
    # Bit7 is genuinely used here. Its custom words for counts1and2 differ
    # from TableB.1; silently dropping the selector cannot reproduce this.
    target = [[int(x==y or x+y==3) for x in range(4)] for y in range(4)]
    body,_ = jbig2.huffman_dictionary([GLYPH])
    globals = jbig2._segment(0,0,0,body)+jbig2._segment(1,53,0,custom_table(prefix=3))
    encoder = jbig2.ArithmeticEncoder()
    jbig2.refinement_pixels(encoder,target,GLYPH,0)
    payload = encoder.finish()
    def count(value): return '000'+format(value,'03b')
    bits = jbig2.huffman_word(4,4)+jbig2.huffman_word(2,4)+count(1)+'00'
    bits += jbig2.huffman_word(15,0)*2+jbig2.huffman_word(1,len(payload))
    coded = jbig2.bits_bytes(bits)+payload
    bits = jbig2.huffman_word(2,None)+jbig2.huffman_word(4,1)+jbig2.huffman_word(2,8)+count(2)
    bits += jbig2.huffman_word(11,1)*2+jbig2.huffman_word(6,0)+'00'+'0'
    bits += jbig2.huffman_word(8,2)+'01'+'0'+jbig2.huffman_word(8,None)
    bits += jbig2.huffman_word(2,None)+jbig2.huffman_word(1,0)+jbig2.huffman_word(1,3)
    coded += jbig2.bits_bytes(bits)
    header = struct.pack('>H',0x283)+bytes([255]*4)+struct.pack('>II',3,2)
    globals += jbig2._segment(2,0,0,header+coded,refs=(0,) if missing else (0,1))
    aggregate = placed([GLYPH,target],[(0,0,0),(4,0,1)],8,5)
    glyphs = [GLYPH,target,aggregate]
    positions = [(0,0,0),(4,0,1),(0,5,2)]
    text,_ = jbig2._text_region(glyphs,positions,13,11)
    data = jbig2._segment(3,48,1,jbig2._page_info(13,11))+jbig2._segment(4,6,1,text,refs=(2,))
    return data,globals,placed(glyphs,positions,13,11),'globals',0


def records(name, encoded, *, repair=False, error=False, stencil=False, before_error=False, first_repair=False):
    data,globals,rows,origin,offset = encoded
    width,height = len(rows[0]),len(rows)
    params = f'outcome={"syntax" if repair or error else "valid"}\nwidth={width}\nheight={height}\n'
    if repair:params += f'repair={REPAIR}\norigin={origin}\noffset={offset}\n'
    params += f'repair_before_error={int(before_error)}\n'
    if first_repair:params += f'first_repair_origin={origin}\nfirst_repair_offset={offset}\n'
    yield Vector('native-'+name,{'.jb2':data,'.globals':globals,'.out':b'' if error else jbig2.packed(rows),'.params':params.encode()},
                 (REQUIREMENT,),'Independent literal matrices and T.88 Huffman words; Strict outcome retained')
    dictionary = b'/Type /XObject /Subtype /Image /Width %d /Height %d /BitsPerComponent 1 /Filter /JBIG2Decode'%(width,height)
    dictionary += b' /ImageMask true' if stencil else b' /ColorSpace /DeviceGray'
    if globals: dictionary += b' /DecodeParms << /JBIG2Globals 6 0 R >>'
    objects = [stream(data,dictionary)]+([stream(globals)] if globals else [])
    content = (b'1 0 0 rg ' if stencil else b'')+b'q %d 0 0 %d 2 2 cm /X Do Q'%(width,height)
    pdf = page(content,objects,b'<< /XObject << /X 5 0 R >> >>',size=16)
    extra = {'tolerance':0,'native_case':'native-'+name,'repair_before_error':before_error,'stencil':stencil}
    if repair:extra.update(repair_object=6 if origin=='globals' else 5,repair_offset=offset)
    if first_repair:extra.update(first_repair_object=6 if origin=='globals' else 5,first_repair_offset=offset)
    if error:
        yield Case(name,pdf,OPEN,(REQUIREMENT,),'Malformed table/reference/flags remain errors',strict_valid=False,extra={**extra,'render_error':'syntax'})
    else:
        color = (255,0,0) if stencil else (0,0,0)
        rectangles = [((2+x,2+height-1-y,1,1),color) for y,row in enumerate(rows) for x,bit in enumerate(row) if bit]
        yield Case(name,pdf,RENDER,(REQUIREMENT,),'Exact literal matrix; unused selector is separately reported',
                   oracle=opaque_rectangles(16,16,rectangles),repairs=(REPAIR,) if repair else (),strict_valid=not repair,extra=extra)


def cases():
    for label,options,stencil in [('gray',{},False),('all-custom',{'all_custom':True},False),('stencil',{},True),('globals',{'global_dictionary':True},False)]:
        yield from records(label+'-valid',ordinary(**options),stencil=stencil)
        yield from records(label+'-unused',ordinary(unused=True,**options),repair=True,stencil=stencil)
    yield from records('used-aggregation-custom',used_aggregation())
    for name,options,before in [
        ('missing-table',{'missing':True},True),
        ('extra-table',{'extra':True},True),
        ('non-table-referral',{'non_table':True},False),
        ('reserved-table',{'bad_table':'reserved'},False),
        ('truncated-table',{'bad_table':'truncated'},False),
        ('oob-unused-table',{'bad_table':'oob'},True),
        ('context-used',{'flag_extra':0x100},False),
        ('arithmetic-selector',{'flag_extra':-1},False),
    ]:
        yield from records('hostile-'+name,ordinary(unused=True,**options),error=True,before_error=before)
    yield from records('hostile-used-missing-table',used_aggregation(missing=True),error=True)
    for label,both in [('main',False),('both',True)]:
        yield from records('repeated-'+label+'-valid',repeated(both_streams=both))
        yield from records('repeated-'+label+'-unused',repeated(unused=True,both_streams=both),repair=True)
    yield from records('hostile-repeated-missing-table',repeated(unused=True,both_streams=True,late_missing=True),error=True,before_error=True,first_repair=True)
