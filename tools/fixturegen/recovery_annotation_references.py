"""Immediate defined Null references in Annots; exact identities and full pixels.

Only this explicit reader recovery is proposed. Undefined/free identities,
generations, chains and real annotation validation keep their original rules.
No product parser, writer or renderer constructs the inputs or output oracles.
"""
from .model import Case, OPEN, RENDER, Vector
from .pdf import opaque_rectangles, stream
import struct

AREA = 'recovery_annotation_references'
REQUIREMENT = ('recovery-annotation-null-reference',)
TAG = 'pages.annotation-null-reference'


def document(records, *, free=(), generations=None):
    generations = generations or {}
    out = bytearray(b'%PDF-1.7\n%\xe2\xe3\xcf\xd3\n')
    offsets = {}
    for number, body in sorted(records.items()):
        offsets[number] = len(out)
        out += f'{number} {generations.get(number, 0)} obj\n'.encode() + body + b'\nendobj\n'
    start = len(out)
    count = max([*records, *free]) + 1
    out += f'xref\n0 {count}\n0000000000 65535 f \n'.encode()
    for number in range(1, count):
        if number in offsets:
            out += f'{offsets[number]:010} {generations.get(number, 0):05} n \n'.encode()
        else:
            out += f'0000000000 {1 if number in free else 0:05} f \n'.encode()
    out += f'trailer\n<< /Size {count} /Root 1 0 R >>\nstartxref\n{start}\n%%EOF\n'.encode()
    return bytes(out)


def records():
    appearance = b'/Type /XObject /Subtype /Form /BBox [0 0 4 4] /Resources << >> '
    return {
        1: b'<< /Type /Catalog /Pages 2 0 R >>',
        2: b'<< /Type /Pages /Kids [3 0 R] /Count 1 >>',
        3: b'<< /Type /Page /Parent 2 0 R /MediaBox [0 0 32 32] /Resources << >> /Annots [4 0 R TOKEN 6 0 R] /Contents 8 0 R >>',
        4: b'<< /Type /Annot /Subtype /Square /Rect [2 2 10 10] /AP << /N 5 0 R >> >>',
        5: stream(b'1 0 0 rg 0 0 4 4 re f', appearance),
        6: b'<< /Type /Annot /Subtype /Link /Rect [6 6 14 14] /AP << /N 7 0 R >> /A << /S /URI /URI (https://example.test/kept) >> >>',
        7: stream(b'0 1 0 rg 0 0 4 4 re f', appearance),
        8: stream(b'0 0 1 rg 20 2 4 4 re f'), 9: b'null',
    }


def oracle():
    return opaque_rectangles(32, 32, [((20,2,4,4),(0,0,255)), ((2,2,8,8),(255,0,0)), ((6,6,8,8),(0,255,0))])


def cases():
    for name, token, value, generation, wanted in [
        ('defined-zero', b'9 0 R', b'null', 0, RENDER),
        ('defined-seven', b'9 7 R', b'null', 7, RENDER),
        ('wrong-generation', b'9 1 R', b'null', 0, 'syntax'),
        ('undefined', b'99 0 R', b'null', 0, 'syntax'),
        ('reference-chain', b'9 0 R', b'10 0 R', 0, 'syntax'),
        ('reference-cycle', b'9 0 R', b'9 0 R', 0, 'limit'),
        ('integer', b'9 0 R', b'42', 0, 'syntax'),
        ('string', b'9 0 R', b'(null)', 0, 'syntax'),
        ('array', b'9 0 R', b'[]', 0, 'syntax'),
        ('malformed-dictionary', b'9 0 R', b'<< /Type /Annot >>', 0, 'syntax'),
    ]:
        data = records(); data[3] = data[3].replace(b'TOKEN', token); data[9] = value
        if name == 'reference-chain':
            data[10] = b'null'
        yield make(name, document(data, generations={9:generation}), wanted)
    data = records(); data[3] = data[3].replace(b'TOKEN', b'9 1 R'); del data[9]
    yield make('free', document(data, free=(9,)), 'syntax')
    data = records(); data[3] = data[3].replace(b'TOKEN', b'9 0 R'); data[6] = b'<< /Type /Annot >>'
    yield make('later-invalid', document(data), 'syntax', repaired=True)
    data = records(); data[3] = data[3].replace(b'TOKEN', b'9 0 R 9 0 R')
    yield make('repeat-identity', document(data), RENDER)
    data = records(); data[3] = data[3].replace(b'TOKEN', b'9 0 R')
    data[2] = b'<< /Type /Pages /Kids [11 0 R 3 0 R] /Count 2 >>'
    data[11] = b'<< /Type /Page /Parent 2 0 R /MediaBox [0 0 32 32] /Resources << >> /Contents 12 0 R >>'
    data[12] = stream(b'0 0 1 rg 0 0 32 32 re f')
    yield make('later-first-use', document(data), RENDER, page=1)
    data = records(); data[3] = data[3].replace(b'TOKEN', b'9 0 R '*1025)
    yield make('many-references', document(data), RENDER)
    yield make('compressed-null', compressed_null(), RENDER)
    yield from signature_cases()
    data = records(); data[3] = data[3].replace(b'TOKEN', b'9 0 R'); data[9] = b'0 0 R'
    yield Case('zero-reference-body', document(data), RENDER, REQUIREMENT,
               'Existing separately reported zero-reference parser repair yields immediate parsed Null; no nonzero chain is followed and Strict retains its earlier parser error',
               oracle=oracle(), repairs=('syntax.reference-zero', TAG), strict_valid=False,
               extra={'page':0, 'expected_annotation_ids':[4,6], 'expected_link_count':1,
                      'scan_recovery':'repair',
                      'mode_errors':{'strict':{'kind':'Syntax', 'repair':'syntax.reference-zero',
                                               'message':'object-zero reference interpreted as null'}}})


def make(name, data, wanted, *, repaired=False, page=0):
    positive = wanted == RENDER
    extra = {'page':page, 'expected_annotation_ids':[4,6], 'expected_link_count':1} if positive else {'render_error':wanted}
    extra['scan_recovery'] = 'repair'
    extra['mode_errors'] = {'strict':{'kind':'Syntax', 'repair':None}} if positive or repaired else {}
    return Case(name, data, RENDER if positive else OPEN, REQUIREMENT,
                'Only an immediate exact in-use Null is skipped; supplied neighbors, source order and original refusal paths remain required',
                oracle=oracle() if positive else None, repairs=(TAG,) if positive or repaired else (),
                strict_valid=not positive and not repaired, pages=2 if page else 1, extra=extra)


def signature_cases():
    # The already qualified literal policy controls stay byte-identical in
    # their original area. These independent variants change only literal
    # null entries to a real, explicit object reference, before either run.
    from .signature_changes import base
    from .signatures import patch, signature

    annotation = b'<< /Type /Annot /Subtype /Text /Rect [1 1 2 2] /Contents (old) /P 3 0 R >>'
    for location in ['clean', 'signed-null', 'added-null']:
        for operation in ['annotation', 'content', 'signing']:
            array = b'[10 0 R 14 0 R]' if location == 'signed-null' else b'[10 0 R]'
            page = b'<< /Type /Page /Parent 2 0 R /MediaBox [0 0 32 32] /Annots '+array+b' /Contents 11 0 R >>'
            source, _ = base(3, initial={3:page,10:annotation,11:stream(b'0 g 0 0 4 4 re f'),14:b'null'})
            changes = {}
            signed = None
            if location == 'added-null':
                changes[3] = page.replace(b'/Annots [10 0 R]', b'/Annots [10 0 R 14 0 R]')
            if operation == 'annotation':
                changes[10] = annotation.replace(b'(old)', b'(new)')
            elif operation == 'content':
                changes[11] = stream(b'1 g 0 0 4 4 re f')
            else:
                signed = signature()
                body, _ = signed
                changes.update({4:b'<< /Fields [6 0 R 7 0 R 8 0 R 13 0 R] /SigFlags 3 >>',12:body,13:b'<< /FT /Sig /T (Second) /V 12 0 R >>'})
            positions = source.append(changes)
            if signed is not None:
                body, span = signed
                patch(source, positions[12], body, span, revision=1)
            outcome = 'Inconclusive' if location != 'clean' else 'Disallowed' if operation == 'content' else 'Allowed'
            yield Vector('signature-'+location+'-'+operation,
                         {'.pdf':bytes(source.data),'.outcome':(outcome+'\n').encode()}, REQUIREMENT,
                         'Explicit Null reference recovery cannot establish signed-edit permission. Placeholder signatures assert no integrity.',
                         {'driver':'annotation-null-reference-signature-policy'})
    for name, initial, current in [('historical-null-current-valid', b'null', annotation),
                                   ('historical-valid-current-null', annotation, b'null')]:
        page = b'<< /Type /Page /Parent 2 0 R /MediaBox [0 0 32 32] /Annots [10 0 R] /Contents 11 0 R >>'
        source, _ = base(3, initial={3:page,10:initial,11:stream(b'0 g 0 0 4 4 re f')})
        source.append({10:current})
        yield Vector('signature-'+name,
                     {'.pdf':bytes(source.data),'.outcome':b'Inconclusive\n'}, REQUIREMENT,
                     'Each source revision retains its own annotation identity and Strict reopening; placeholder signature asserts no integrity.',
                     {'driver':'annotation-null-reference-signature-policy'})


def compressed_null():
    """One ordinary object-stream member, independently emitted xref types."""
    data = records(); data[3] = data[3].replace(b'TOKEN', b'9 0 R'); del data[9]
    data[13] = stream(b'9 0 null', b'/Type /ObjStm /N 1 /First 4')
    out = bytearray(b'%PDF-1.7\n%\xe2\xe3\xcf\xd3\n')
    offsets = {}
    for number, body in sorted(data.items()):
        offsets[number] = len(out)
        out += f'{number} 0 obj\n'.encode()+body+b'\nendobj\n'
    offsets[14] = len(out)
    table = bytearray()
    for number in range(15):
        kind, field, generation = (2,13,0) if number == 9 else (1,offsets[number],0) if number in offsets else (0,0,65535 if number == 0 else 0)
        table += struct.pack('>BIH',kind,field,generation)
    out += b'14 0 obj\n'+stream(bytes(table),b'/Type /XRef /Size 15 /Root 1 0 R /W [1 4 2]')+b'\nendobj\n'
    out += f'startxref\n{offsets[14]}\n%%EOF\n'.encode()
    return bytes(out)
