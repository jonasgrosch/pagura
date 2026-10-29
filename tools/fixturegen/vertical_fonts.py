"""Original GSUB byte layouts with literal glyph-selection expectations.

OpenType 1.9.1 GSUB types 1 and 7, common Script/Language/Feature/Lookup and
Coverage tables, and registered vert/vrt2 features. No font engine is invoked.
"""
from .font import truetype, u16, u32
from .model import Vector
from .text import cid_substitute_document

AREA = 'vertical_fonts'
REQUIREMENT = 'font-vertical-substitution'


def records(entries):
    """Tag records with offsets measured from the start of this list."""
    offset = 2 + 6 * len(entries)
    header, body = u16(len(entries)), b''
    for tag, data in entries:
        header += tag + u16(offset)
        body += data
        offset += len(data)
    return header + body


def offsets(entries, prefix=b''):
    offset = len(prefix) + 2 + 2 * len(entries)
    header, body = prefix + u16(len(entries)), b''
    for data in entries:
        header += u16(offset)
        body += data
        offset += len(data)
    return header + body


def language(indices):
    return u16(0) + u16(65535) + u16(len(indices)) + b''.join(u16(i) for i in indices)


def script(indices, named=None):
    default = language(indices)
    if named is None:
        return u16(4) + u16(0) + default
    tag, indices = named
    return u16(10) + u16(1) + tag + u16(10 + len(default)) + default + language(indices)


def coverage(glyphs):
    return u16(1) + u16(len(glyphs)) + b''.join(u16(g) for g in glyphs)


def single(glyphs=(1,), outputs=(2,), *, delta=None, cover=None):
    cover = coverage(glyphs) if cover is None else cover
    if delta is not None:
        return u16(1) + u16(6) + u16(delta % 65536) + cover
    body = u16(len(outputs)) + b''.join(u16(g) for g in outputs)
    return u16(2) + u16(4 + len(body)) + body + cover


def lookup(subtables, *, kind=1, flags=0):
    return offsets(subtables, u16(kind) + u16(flags))


def gsub(lookups, features=None, scripts=None, *, variations=False):
    features = [(b'vert', [0])] if features is None else features
    scripts = [(b'hani', script(list(range(len(features)))))] if scripts is None else scripts
    sl = records(scripts)
    fl = records([(tag, u16(0) + u16(len(indices)) + b''.join(u16(i) for i in indices))
                  for tag, indices in features])
    ll = offsets(lookups)
    size = 14 if variations else 10
    header = u32(0x10001 if variations else 0x10000) + u16(size) + u16(size + len(sl)) + u16(size + len(sl) + len(fl))
    if variations:
        header += u32(size + len(sl) + len(fl) + len(ll))
    return header + sl + fl + ll + (u32(0x10000) + u32(0) if variations else b'')


def with_u16(table, offset, value):
    return table[:offset] + u16(value) + table[offset+2:]


def cases():
    cmap = b'/WMode %d def 1 begincodespacerange <0000> <ffff> endcodespacerange 1 begincidchar <0001> 1 endcidchar'
    unicode = b'1 begincodespacerange <0000> <ffff> endcodespacerange 1 beginbfchar <0001> <005a> endbfchar'
    for mode in (0,1):
        yield Vector('pdf-mode-'+str(mode), {'.pdf':cid_substitute_document(
            'Japan1',b'7 0 R',extra=cmap % mode,unicode_map=unicode,
            descendant=b'/W [1 [731]] /W2 [1 [-913 107 809]] /DW 900 /DW2 [880 -1100]')},
            (REQUIREMENT,'text-cid-substitution'),
            'Provider Unicode A selects shape while ToUnicode extracts Z; PDF owns all horizontal/vertical metrics')
    direct = lookup([single()])
    chain = [lookup([single()]), lookup([single((2,), (4,))])]
    ranges = u16(2) + u16(2) + u16(1) + u16(2) + u16(0) + u16(3) + u16(3) + u16(2)
    extension = u16(1) + u16(1) + u32(8) + single((1,), (3,))
    alternate = gsub([direct], [(b'vert',[0]), (b'vrt2',[0])])
    feature_list = int.from_bytes(alternate[6:8], 'big')
    first_feature = feature_list + int.from_bytes(alternate[feature_list+6:feature_list+8], 'big')
    fallback = gsub([direct], scripts=[(b'DFLT',script([0])), (b'hani',script([0]))])
    script_list = int.from_bytes(fallback[4:6], 'big')
    for name, table, expected in [
        ('unused-vert-offset', with_u16(alternate, feature_list+6, 0), '0 2 2 3 4'),
        ('unused-vert-entries', with_u16(alternate, first_feature+2, 65535), '0 2 2 3 4'),
        ('unused-default-script', with_u16(fallback, script_list+6, 0), '0 2 2 3 4'),
        ('single-delta', gsub([lookup([single(delta=1)])]), '0 2 2 3 4'),
        ('single-ranges', gsub([lookup([single(outputs=(2,3,4), cover=ranges)])]), '0 2 3 4 4'),
        ('extension', gsub([lookup([extension], kind=7)]), '0 3 2 3 4'),
        ('lookup-order', gsub(chain, [(b'vert', [1,0,1])]), '0 4 4 3 4'),
        ('first-subtable', gsub([lookup([single(), single((1,), (3,))])]), '0 2 2 3 4'),
        ('vrt2-preferred', gsub([direct, lookup([single((1,), (4,))])], [(b'vert',[0]), (b'vrt2',[1])]), '0 4 2 3 4'),
        ('default-script', gsub([direct], scripts=[(b'DFLT',script([0]))]), '0 2 2 3 4'),
        ('named-language', gsub(chain, [(b'vert',[0]),(b'vert',[1])], [(b'hani',script([0],(b'JAN ',[1])))]), '0 1 4 3 4'),
        ('missing-feature', gsub([direct], [(b'liga',[0])]), 'none'),
        ('missing-script', gsub([direct], scripts=[(b'latn',script([0]))]), 'none'),
    ]:
        yield Vector(name, {'.font':truetype(table_overrides={b'GSUB':table}), '.map':(expected+'\n').encode()},
                     (REQUIREMENT,), 'Literal five-glyph map after selecting hani/JAN; PDF metrics and cmap are untouched')
    bad_ranges = u16(2)+u16(1)+u16(1)+u16(2)+u16(1)
    for name, table, expected in [
        ('coverage-order',gsub([lookup([single((2,1),(3,4))])]),'syntax'),
        ('coverage-range-index',gsub([lookup([single(outputs=(2,3),cover=bad_ranges)])]),'syntax'),
        ('coverage-count',gsub([lookup([single((1,2),(3,))])]),'syntax'),
        ('output-range',gsub([lookup([single((1,),(5,))])]),'syntax'),
        ('coverage-range',gsub([lookup([single((5,),(1,))])]),'syntax'),
        ('null-coverage',gsub([lookup([u16(1)+u16(0)+u16(1)])]),'syntax'),
        ('lookup-index',gsub([direct],[(b'vert',[1])]),'syntax'),
        ('feature-index',gsub([direct],scripts=[(b'hani',script([1]))]),'syntax'),
        ('reserved-flags',gsub([lookup([single()],flags=0x20)]),'syntax'),
        ('filtering',gsub([lookup([single()],flags=2)]),'unsupported'),
        ('contextual',gsub([lookup([single()],kind=5)]),'unsupported'),
        ('variable-feature',gsub([direct],variations=True),'unsupported'),
        *[(f'extension-kind-{kind}',gsub([lookup([u16(1)+u16(kind)+u32(8)+single()],kind=7)]),
           'unsupported' if kind == 5 else 'syntax') for kind in (0,7,9,5)],
    ]:
        yield Vector(name, {'.font':truetype(table_overrides={b'GSUB':table}), '.error':(expected+'\n').encode()},
                     (REQUIREMENT,), 'Selected GSUB errors remain explicit; an ordinary horizontal font parse ignores the unused layout table')
