"""Literal null Annots members: complete authored appearances and exact pixels.

ISO 32000-2 Table31/12.5.2 require annotation references; array null is a
reported reader recovery policy, not the dictionary-null rule of7.3.9.
No product reader, writer or renderer is imported or run.
"""
from .model import Case, OPEN, RENDER, Vector
from .pdf import document, opaque_rectangles, stream

AREA = 'recovery_annotations'
REQUIREMENT = ('recovery-annotation-null-entry',)
TAG = 'pages.annotation-null'


def pdf(annots, *, indirect=False, fault=None):
    page_annots = b'9 0 R' if indirect else annots
    annotation = b'<< /Type /Annot /Subtype /Square /Rect [2 2 10 10] /AP << /N 5 0 R >> >>'
    link = b'<< /Type /Annot /Subtype /Link /Rect [6 6 14 14] /AP << /N 7 0 R >> /A << /S /URI /URI (https://example.test/one) >> >>'
    appearance = b'/Type /XObject /Subtype /Form /BBox [0 0 4 4] /Resources << >> '
    red = stream(b'1 0 0 rg 0 0 4 4 re f', appearance)
    if fault == 'bbox':
        red = stream(b'1 0 0 rg 0 0 4 4 re f', b'/Type /XObject /Subtype /Form /Resources << >> ')
    if fault == 'cycle':
        red = stream(b'/Self Do', b'/Type /XObject /Subtype /Form /BBox [0 0 4 4] /Resources << /XObject << /Self 5 0 R >> >> ')
    if fault == 'subtype':
        link = b'<< /Type /Annot /Rect [6 6 14 14] /AP << /N 7 0 R >> >>'
    return document([
        b'<< /Type /Catalog /Pages 2 0 R >>',
        b'<< /Type /Pages /Kids [3 0 R] /Count 1 >>',
        b'<< /Type /Page /Parent 2 0 R /MediaBox [0 0 32 32] /Resources << >> /Annots '+page_annots+b' /Contents 8 0 R >>',
        annotation, red, link, stream(b'0 1 0 rg 0 0 4 4 re f', appearance),
        stream(b'0 0 1 rg 20 2 4 4 re f'),
        annots if indirect else b'null',
    ])


def oracle(members):
    rectangles = [((20,2,4,4),(0,0,255))]
    for member in members:
        rectangles.append(((2,2,8,8),(255,0,0)) if member == 4 else ((6,6,8,8),(0,255,0)))
    return opaque_rectangles(32,32,rectangles)


def cases():
    for name, array, members, repaired, indirect in [
        ('clean',b'[4 0 R 6 0 R]',[4,6],False,False),
        ('empty',b'[]',[],False,False),
        ('leading',b'[null 4 0 R 6 0 R]',[4,6],True,False),
        ('middle',b'[4 0 R null 6 0 R]',[4,6],True,False),
        ('trailing',b'[4 0 R 6 0 R null]',[4,6],True,False),
        ('repeated',b'[null 4 0 R null null 6 0 R null]',[4,6],True,False),
        ('null-only',b'[null]',[],True,False),
        ('reverse-order',b'[6 0 R null 4 0 R]',[6,4],True,False),
        ('indirect-array',b'[4 0 R null 6 0 R]',[4,6],True,True),
        ('many-nulls',b'['+b'null '*1025+b'4 0 R 6 0 R]',[4,6],True,False),
    ]:
        yield Case(name,pdf(array,indirect=indirect),RENDER,REQUIREMENT,
                   'Stored red Square and green Link appearances retain source order; literal array null is reported before it is skipped',
                   oracle=oracle(members),repairs=(TAG,) if repaired else (),strict_valid=not repaired,
                   extra={'expected_annotation_ids':members,'expected_link_count':int(6 in members)})
    for name, array, error, repaired, fault in [
        ('indirect-null',b'[4 0 R 9 0 R 6 0 R]','syntax',False,None),
        ('undefined-reference',b'[4 0 R 99 0 R 6 0 R]','syntax',False,None),
        ('wrong-generation',b'[4 1 R 6 0 R]','syntax',False,None),
        ('boolean',b'[4 0 R true 6 0 R]','syntax',False,None),
        ('number',b'[4 0 R 42 6 0 R]','syntax',False,None),
        ('string',b'[4 0 R (null) 6 0 R]','syntax',False,None),
        ('nested-array',b'[4 0 R [] 6 0 R]','syntax',False,None),
        ('after-null-boolean',b'[4 0 R null true 6 0 R]','syntax',True,None),
        ('after-null-bbox',b'[null 4 0 R]','syntax',True,'bbox'),
        ('after-null-cycle',b'[null 4 0 R]','cycle',True,'cycle'),
        ('after-null-subtype',b'[4 0 R null 6 0 R]','syntax',True,'subtype'),
    ]:
        yield Case(name,pdf(array,fault=fault),OPEN,REQUIREMENT,
                   'Non-null references and malformed real annotations remain required; a painted page prefix must not become a partial successful output',
                   repairs=(TAG,) if repaired else (),strict_valid=not repaired,
                   extra={'render_error':error})
    yield from signed_cases()


def signed_cases():
    """Structural policy only: placeholder signatures assert no integrity."""
    from .signature_changes import base
    from .signatures import patch, signature

    annotation = b'<< /Type /Annot /Subtype /Text /Rect [1 1 2 2] /Contents (old) /P 3 0 R >>'
    for location in ['clean', 'signed-null', 'added-null']:
        for operation in ['annotation', 'content', 'signing']:
            array = b'[10 0 R null]' if location == 'signed-null' else b'[10 0 R]'
            page = b'<< /Type /Page /Parent 2 0 R /MediaBox [0 0 32 32] /Annots '+array+b' /Contents 11 0 R >>'
            source, _ = base(3, initial={3:page,10:annotation,11:stream(b'0 g 0 0 4 4 re f')})
            changes = {}
            signed = None
            if location == 'added-null':
                changes[3] = page.replace(b'/Annots [10 0 R]', b'/Annots [10 0 R null]')
            if operation == 'annotation':
                changes[10] = annotation.replace(b'(old)', b'(new)')
            elif operation == 'content':
                changes[11] = stream(b'1 g 0 0 4 4 re f')
            else:
                signed = signature()
                body, _ = signed
                changes.update({4:b'<< /Fields [6 0 R 7 0 R 8 0 R 13 0 R] /SigFlags 3 >>',
                                12:body,13:b'<< /FT /Sig /T (Second) /V 12 0 R >>'})
            positions = source.append(changes)
            if signed is not None:
                body, span = signed
                patch(source, positions[12], body, span, revision=1)
            outcome = 'Inconclusive' if location != 'clean' else 'Disallowed' if operation == 'content' else 'Allowed'
            yield Vector('signature-'+location+'-'+operation,
                         {'.pdf':bytes(source.data),'.outcome':(outcome+'\n').encode()},REQUIREMENT,
                         'Authored DocMDP P3 mutation: null recovery cannot establish permission; clean content edits remain forbidden. Placeholder signatures assert no integrity.',
                         {'driver':'annotation-null-signature-policy'})
    # The same referenced identity is invalid in the signed revision and an
    # actual annotation dictionary in the current revision. Validation must
    # use the historical snapshot while retaining the shared work budget.
    page = b'<< /Type /Page /Parent 2 0 R /MediaBox [0 0 32 32] /Annots [10 0 R] /Contents 11 0 R >>'
    source, _ = base(3, initial={3:page,10:b'null',11:stream(b'0 g 0 0 4 4 re f')})
    source.append({10:annotation})
    yield Vector('signature-historical-reference',
                 {'.pdf':bytes(source.data),'.outcome':b'Inconclusive\n'},REQUIREMENT,
                 'Current valid annotation dictionary cannot rescue an invalid signed-revision target with the same identity. Structural policy only; placeholder signature asserts no integrity.',
                 {'driver':'annotation-null-signature-policy'})
