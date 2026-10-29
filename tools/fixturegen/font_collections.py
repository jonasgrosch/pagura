"""Diagnostic TTC/OTC containers with independent named-face rectangles.

OpenType Font Collections/TTC Header and name table version 0 define the
container and names. PDF Table 124 defines the embedding routes but supplies
no collection index; multi-face name selection is an observation to adjudicate.
No font engine is used for construction, selection or expected pixels.
"""
import struct

from .font import cff_font, i16, simple, truetype, u16, u32
from .model import Vector
from .paint import BLACK, FONT, case
from .pdf import stream

AREA = 'font_collections'
NAMES = ('CollectionFirst', 'CollectionSecond')


def naming(name):
    records, storage = b'', b''
    values = {1:name, 2:'Regular', 3:'PaguraProbe:'+name, 4:name, 5:'Version 1.0', 6:name}
    for key, text in sorted(values.items()):
        data = text.encode('utf-16-be')
        records += u16(3)+u16(1)+u16(0x409)+u16(key)+u16(len(data))+u16(len(storage))
        storage += data
    return u16(0)+u16(len(values))+u16(6+len(records))+records+storage


def face(kind, index, cid=False):
    # Both routes produce a 100x200 first glyph or a 200x400 second glyph.
    # The PDF width remains 600. CFF explicitly sets Top FontMatrix so this
    # probe cannot depend on the separately unresolved omitted-Top semantics.
    width, height = ((100,200),(200,400))[index]
    glyph = simple([(0,0,True),(width,0,True),(width,height,True),(0,height,True)])
    glyph = struct.pack('>hhhhh',1,0,0,width,height)+glyph[10:]
    if kind == 'glyf':
        program = truetype(glyph_override=glyph,table_overrides={b'name':naming(NAMES[index])})
    else:
        matrix = b'\x1e\xa0\x01\xff\x8b\x8b\x1e\xa0\x01\xff\x8b\x8b'
        cff = cff_font(conforming=True,cid=True,variant='fd-array' if index else None,top_matrix=matrix)
        # FDArray's second matrix doubles the original rectangle at GID1.
        program = truetype(cff=cff,table_overrides={b'name':naming(NAMES[index])})
    count = struct.unpack_from('>H',program,4)[0]
    tables = {}
    for n in range(count):
        tag, _, offset, length = struct.unpack_from('>4sIII',program,12+16*n)
        tables[tag] = program[offset:offset+length]
    glyph_count, extent = (2,width) if kind=='glyf' else (5,300)
    if kind=='glyf':
        # Only .notdef and the one rectangle: no unused composite/hint programs.
        tables[b'glyf'] = glyph+bytes(len(glyph)%2)
        tables[b'loca'] = u32(0)+u32(0)+u32(len(tables[b'glyf']))
        tables[b'maxp'] = u32(0x10000)+struct.pack('>14H',2,4,1,0,0,1,0,0,0,0,0,0,0,0)
        if cid:
            # ISO 32000-2 9.9.1 excludes cmap from a CIDFontType2 FontFile2.
            del tables[b'cmap']
    tables[b'post'] = u32(0x30000)+bytes(28)
    tables[b'hhea'] = (u32(0x10000)+struct.pack('>hhhHhhhhhh',800,0,0,600,0,600-extent,extent,1,0,0)
                       +bytes(10)+u16(glyph_count))
    tables[b'hmtx'] = u16(500)+i16(0)+(u16(600)+i16(0))*(glyph_count-1)
    # Version-0 OS/2 metrics, regular weight/width and unrestricted embedding.
    tables[b'OS/2'] = (struct.pack('>HhHHH',0,600,400,5,0)
                       +struct.pack('>11h',650,600,0,75,650,600,0,350,50,200,0)
                       +bytes(10)+u32(1)+bytes(12)+b'PGRA'+u16(64)+u16(65)+u16(65)
                       +struct.pack('>hhhHH',800,0,0,800,0))
    head = bytearray(tables[b'head'])
    head[4:8] = u32(0x10000)
    head[8:12] = bytes(4)  # Collections do not use checksumAdjustment.
    head[16:18] = u16(3)
    head[36:44] = struct.pack('>hhhh',0,0,extent,height)
    head[46:50] = u16(8)+i16(2)
    tables[b'head'] = bytes(head)
    return program[:4], tables


def checksum(data):
    padded = data+bytes(-len(data)%4)
    return sum(value[0] for value in struct.iter_unpack('>I',padded)) & 0xffffffff


def container(faces, version=None):
    """Repack directories with file-relative offsets and shared immutable tables."""
    header_size = 0 if version is None else 12+4*len(faces)+(12 if version==2 else 0)
    directory_offsets, offset = [], header_size
    for _, tables in faces:
        directory_offsets.append(offset)
        offset += 12+16*len(tables)
    pool, body, directories = {}, bytearray(), bytearray()
    head_at = None
    for flavor, tables in faces:
        count = len(tables)
        power = 1 << (count.bit_length()-1)
        directories += flavor+u16(count)+u16(16*power)+u16(power.bit_length()-1)+u16(16*(count-power))
        for tag, data in sorted(tables.items()):
            key = (tag,data)
            if key not in pool:
                pool[key] = offset+len(body)
                body += data+bytes(-len(data)%4)
            at = pool[key]
            directories += tag+u32(checksum(data))+u32(at)+u32(len(data))
            if tag==b'head': head_at = at
    header = b'' if version is None else b'ttcf'+u16(version)+u16(0)+u32(len(faces))+b''.join(u32(at) for at in directory_offsets)
    if version==2: header += bytes(12)  # No DSIG is present.
    result = bytearray(header+directories+body)
    if version is None:
        assert len(faces)==1 and head_at is not None
        result[head_at+8:head_at+12] = u32((0xb1b0afba-checksum(result)) & 0xffffffff)
    return bytes(result)


def objects(program, kind, cid, name):
    name = name.encode('ascii')
    subtype = b'TrueType' if kind=='glyf' else b'Type1'
    if cid:
        subtype = b'CIDFontType2' if kind=='glyf' else b'CIDFontType0'
        fonts = [b'<< /Type /Font /Subtype /Type0 /BaseFont /'+name+b' /Encoding /Identity-H /DescendantFonts [6 0 R] >>',
                 b'<< /Type /Font /Subtype /'+subtype+b' /BaseFont /'+name+b' /DW 600 '
                 b'/CIDSystemInfo << /Registry (Adobe) /Ordering (Identity) /Supplement 0 >> '
                 +(b'/CIDToGIDMap /Identity ' if kind=='glyf' else b'')+b'/FontDescriptor 7 0 R >>']
        stream_at = 8
    else:
        fonts = [b'<< /Type /Font /Subtype /'+subtype+b' /BaseFont /'+name+b' /FirstChar 65 /LastChar 65 '
                 b'/Widths [600] /Encoding /WinAnsiEncoding /FontDescriptor 6 0 R >>']
        stream_at = 7
    descriptor = (b'<< /Type /FontDescriptor /FontName /'+name+b' /Flags '+(b'4' if cid else b'32')
                  +b' /FontBBox [0 0 400 800] /ItalicAngle 0 /Ascent 800 /Descent 0 /CapHeight 400 /StemV 50 /'
                  +(b'FontFile2' if kind=='glyf' else b'FontFile3')+b' %d 0 R >>'%stream_at)
    return [*fonts,descriptor,stream(program,b'/Length1 %d'%len(program) if kind=='glyf' else b'/Subtype /OpenType')]


def cases():
    for kind in ('glyf','cff'):
        for cid in ((False,True) if kind=='glyf' else (True,)):
            faces = [face(kind,n,cid) for n in (0,1)]
            first, second = (container([f]) for f in faces)
            sources = [('first-control',first,0),('second-control',second,1),
                       ('single-v1',container([faces[0]],1),0),('single-v2',container([faces[0]],2),0),
                       ('pair-first',container(faces,1),0),('pair-second',container(faces,1),1)]
            # CFF1 uses its explicit charset CID17, while glyf uses GID1.
            text = (b'<0001>' if kind=='glyf' else b'<0011>') if cid else b'(A)'
            for label, program, selected in sources:
                width,height = ((5,10),(10,20))[selected]
                yield case(f'{kind}-'+('cid-' if cid else 'simple-')+label,
                           b'BT /F 50 Tf 1 0 0 1 4 4 Tm '+text+b' Tj ET',objects(program,kind,cid,NAMES[selected]),FONT,
                           [((4,4,width,height),BLACK)],requirement='font-truetype' if kind=='glyf' else 'font-cff',size=64)
    for index,(width,height) in enumerate(((5,10),(10,20))):
        yield case('rectangle-'+str(index),b'0 g 4 4 %d %d re f'%(width,height),
                   rectangles=[((4,4,width,height),BLACK)],requirement='font-truetype',size=64)
    yield from native_cases()


def native_cases():
    """Literal selected outlines; the second directory can follow shared tables."""
    for kind in ('glyf','cff'):
        faces = [face(kind,n,kind=='cff') for n in (0,1)]
        sources = [('v1',container(faces,1)),('v2',container(faces,2)),
                   ('standalone',container([faces[0]]))]
        late = bytearray(container(faces,1))
        second = struct.unpack_from('>I',late,16)[0]
        count = struct.unpack_from('>H',late,second+4)[0]
        directory = late[second:second+12+16*count]
        late[16:20] = u32(len(late))
        late += directory
        sources.append(('late-directory',bytes(late)))
        signed = bytearray(container(faces,2))
        signed[20:32] = b'DSIG'+u32(8)+u32(len(signed))
        signed += u32(1)+u16(0)+u16(0)  # Inert Format 1 header; no signature records.
        sources.append(('dsig',bytes(signed)))
        for label,program in sources:
            yield Vector('native-'+kind+'-'+label,{'.font':program},
                         ('font-truetype' if kind=='glyf' else 'font-cff',),
                         'Index/name selection: first rectangle 100x200; second 200x400; original bytes retained.')
