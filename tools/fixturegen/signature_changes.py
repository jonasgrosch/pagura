"""Object-level MDP expectations authored from ISO 32000-2 12.8.2.2-4.

Each declaration describes an authored mutation. Permission decisions, changed
identities and cumulative results are derived from that ledger and the standard;
no PDF parser or engine is used. Placeholder signatures do not assert integrity.
"""
from .model import Vector
from .signature_policy import edit, expectations
import re
from typing import Any
from .signatures import Pdf as BasicPdf, signature, patch, structure

AREA = 'signature-changes'


class Pdf(BasicPdf):
    data: bytearray
    # Set by base() for the expectation oracle.
    author_permission: Any
    author_selection: Any
    author_unknown_transform: bool
    author_field_unknown: bool

    def __init__(self):
        super().__init__()
        self.author_states = []
        self.author_history_intact = True

    def record(self, objects, trailer=b'', freed=()):
        state = dict(self.author_states[-1]) if self.author_states else {'trailer': b''}
        for number in freed:
            state = {key: body for key, body in state.items() if key.split(':')[0] != str(number)}
        for target in objects:
            if isinstance(target, tuple):
                state = {key: body for key, body in state.items() if key.split(':')[0] != str(target[0])}
        state.update({str(n) if isinstance(n, int) else str(n[0]) + (':' + str(n[1]) if n[1] else ''): body for n, body in objects.items()})
        if trailer:
            state['trailer'] = trailer
        self.author_states.append(state)

    def append(self, objects, *, trailer=b'', **kwargs):  # pyright: ignore[reportIncompatibleMethodOverride]
        positions = super().append(objects, **kwargs)
        if trailer:
            at = self.data.rindex(b'>>\nstartxref')
            self.data[at:at] = trailer
            self.revisions[-1] += len(trailer)
        authored = dict(objects)
        if kwargs.get('stream'):
            authored[50 + len(self.revisions) - 1] = b'xref bookkeeping'
        self.record(authored, trailer)
        return positions

    def compressed(self, members, *, list_members=True):
        header, bodies = b'', b''
        for number, body in sorted(members.items()):
            header += b'%d %d ' % (number, len(bodies))
            bodies += body + b' '
        data = header + bodies
        body = b'<< /Type /ObjStm /N %d /First %d /Length %d >>\nstream\n' % (len(members), len(header), len(data)) + data + b'\nendstream'
        at = len(self.data)
        self.data += b'40 0 obj\n' + body + b'\nendobj\n'
        xref_at = len(self.data)
        xref_id = 50 + len(self.revisions)
        self.size = max(self.size, xref_id + 1)
        entries = {40: (1, at, 0), xref_id: (1, xref_at, 0)}
        if list_members:
            entries.update({n: (2, 40, i) for i, n in enumerate(sorted(members))})
        index = b' '.join(b'%d 1' % n for n in sorted(entries))
        rows = b''.join(bytes([t]) + o.to_bytes(4, 'big') + g.to_bytes(2, 'big') for n, (t,o,g) in sorted(entries.items()))
        self.data += (b'%d 0 obj\n<< /Type /XRef /Size %d /Root 1 0 R /W [1 4 2] /Index [' % (xref_id, self.size) + index
                      + b'] /Length %d /Prev %d >>\nstream\n' % (len(rows), self.previous) + rows
                      + b'\nendstream\nendobj\nstartxref\n%d\n%%%%EOF\n' % xref_at)
        self.previous = xref_at
        self.revisions.append(len(self.data))
        self.record(members | {40: body, xref_id: b'xref bookkeeping'})

    def update(self, objects, freed=None):
        positions, entries = {}, {}
        for (number, generation), body in sorted(objects.items()):
            entries[number] = (len(self.data), generation, b'n')
            self.data += b'%d %d obj\n' % (number, generation)
            positions[number, generation] = len(self.data)
            self.data += body + b'\nendobj\n'
            self.size = max(self.size, number + 1)
        for number, generation in (freed or {}).items():
            entries[number] = (0, generation, b'f')
        at = len(self.data)
        self.data += b'xref\n'
        for number, (offset, generation, kind) in sorted(entries.items()):
            self.data += b'%d 1\n%010d %05d %s \n' % (number, offset, generation, kind)
        self.data += b'trailer\n<< /Size %d /Root 1 0 R /Prev %d >>\nstartxref\n%d\n%%%%EOF\n' % (self.size, self.previous, at)
        self.previous = at
        self.revisions.append(len(self.data))
        self.record(objects, freed=freed or ())
        return positions


def docmdp(permission):
    return (b'<< /TransformMethod /DocMDP /DigestMethod /SHA256 /TransformParams << /P %d /V /1.2 >> >>' % permission)


def fieldmdp(action, names, data=b'1 0 R'):
    return (b'<< /TransformMethod /FieldMDP /DigestMethod /SHA256 /Data ' + data + b' /TransformParams << /V /1.2 /Action /'
            + action + b' /Fields [' + b' '.join(b'(' + n + b')' for n in names) + b'] >> >>')


def base(permission=2, action=None, names=(), *, widget=False, extra=b'', initial=None, refs_extra=b'', trailer=b'', field_data=b'1 0 R'):
    refs = docmdp(permission) + (b' ' + fieldmdp(action, names, field_data) if action else b'') + refs_extra
    body, span = signature(extra=b'/Reference [' + refs + b'] ' + extra)
    objects = structure(catalog_extra=b'/Perms << /DocMDP 5 0 R >>') | {
        4: b'<< /Fields [6 0 R 7 0 R 8 0 R] /SigFlags 3 >>',
        5: body,
        7: b'<< /FT /Tx /T (Amount) /V (old) >>',
        8: b'<< /FT /Tx /T (Recipient) /V (old) >>',
    }
    if widget:
        objects[7] = b'<< /FT /Tx /T (Amount) /V (old) /Subtype /Widget /Rect [0 0 1 1] /AP << /N 11 0 R >> >>'
        objects[11] = stream(b'old')
        objects[3] = b'<< /Type /Page /Parent 2 0 R /MediaBox [0 0 10 10] /Annots [7 0 R] >>'
    objects.update(initial or {})
    pdf = Pdf()
    positions = pdf.append(objects, trailer=trailer)
    patch(pdf, positions[5], body, span)
    pdf.author_permission = permission
    pdf.author_selection = (action.decode() if action else None, tuple(n.decode() for n in names))
    if b'/Lock 12 0 R' in objects[6]:
        lock = objects[12]
        pdf.author_selection = (re.search(rb'/Action /(\w+)', lock)[1].decode(), tuple(n.decode() for n in re.findall(rb'\(([^()]*)\)', lock)))  # pyright: ignore[reportOptionalSubscript]
    pdf.author_unknown_transform = b'/Unknown' in refs_extra or b'/UR ' in refs_extra
    pdf.author_field_unknown = bool(action) and field_data != b'1 0 R'
    return pdf, objects


def stream(data):
    return b'<< /Length %d >>\nstream\n' % len(data) + data + b'\nendstream'


def linearized_base(first_page_marker, main_trailer=b''):
    """Author the forward Prev layout of F.3.4/F.3.11 without scanning bytes.

    Hints are inert in the appended, no-longer-linearized file (G.7). Only the
    main section terminates the base; an optional first-page EOF is ignored.
    """
    pdf = Pdf()
    objects = {
        1: b'<< /Type /Catalog /Pages 2 0 R >>',
        2: b'<< /Type /Pages /Kids [3 0 R] /Count 1 >>',
        3: b'<< /Type /Page /Parent 2 0 R /MediaBox [0 0 10 10] >>',
        4: b'<< /S 36 /Length 64 >>\nstream\n' + bytes(64) + b'\nendstream',
        5: b'<< /Linearized 1 /L LLLLLLLLLL /H [HHHHHHHHHH IIIIIIIIII] /O 3 /E EEEEEEEEEE /N 1 /T TTTTTTTTTT >>',
    }
    offsets = {5: len(pdf.data)}
    pdf.data += b'5 0 obj\n' + objects[5] + b'\nendobj\n'
    first = len(pdf.data)
    pdf.data += b'xref\n1 5\n'
    rows = len(pdf.data)
    pdf.data += b'0000000000 00000 n \n' * 5
    pdf.data += b'trailer\n<< /Size 6 /Root 1 0 R /Prev PPPPPPPPPP >>\n'
    if first_page_marker:
        pdf.data += b'startxref\n0\n%%EOF\n'
    for number in range(1, 5):
        offsets[number] = len(pdf.data)
        pdf.data += b'%d 0 obj\n' % number + objects[number] + b'\nendobj\n'
    main = len(pdf.data)
    pdf.data += b'xref\n0 1\n0000000000 65535 f \ntrailer\n<< /Size 6' + main_trailer + b' >>\nstartxref\n%d\n%%%%EOF\n' % first
    for number in range(1, 6):
        at = rows + 20 * (number - 1)
        pdf.data[at:at+10] = b'%010d' % offsets[number]
    for token, value in ((b'LLLLLLLLLL', len(pdf.data)), (b'HHHHHHHHHH', offsets[4]),
                         (b'IIIIIIIIII', main-offsets[4]), (b'EEEEEEEEEE', main),
                         (b'TTTTTTTTTT', main+len(b'xref\n0 1\n')), (b'PPPPPPPPPP', main)):
        at = pdf.data.index(token)
        pdf.data[at:at+10] = b'%010d' % value
    pdf.previous, pdf.size, pdf.revisions = first, 6, [len(pdf.data)]
    pdf.record(objects, trailer=main_trailer)
    return pdf


def vector(name, pdf, declarations):
    expected, net = expectations(pdf, declarations)
    return Vector(name, {'.pdf': bytes(pdf.data), '.mdp': expected, '.net': net},
                  ('signature-permitted-changes',), 'ISO 32000-2 authored mutation ledger: ' + name)


def cases():
    for p in (1, 2, 3):
        pdf, _ = base(p)
        pdf.append({7: b'<< /FT /Tx /T (Amount) /V (new) >>'})
        yield vector(f'form-fill-p{p}', pdf, [edit(7, 'fill')])
    for p in (2, 3):
        pdf, objects = base(p)
        pdf.append({3: objects[3][:-2] + b' /Annots [10 0 R] >>', 10: b'<< /Type /Annot /Subtype /Text /Rect [1 1 2 2] /Contents (note) /P 3 0 R >>'})
        yield vector(f'annotation-added-p{p}', pdf, [edit(3, 'annotation'), edit(10, 'annotation')])
    for p in (1, 2, 3):
        pdf, objects = base(p)
        pdf.append({3: objects[3][:-2] + b' /Contents 10 0 R >>', 10: stream(b'0 0 m 1 1 l S')})
        rule = 'page-content'
        yield vector(f'page-content-p{p}', pdf, [edit(3, rule), edit(10, rule)])
    for action, names, changed, allowed in [(b'Include', (b'Amount',), 7, False), (b'Include', (b'Amount',), 8, True), (b'Exclude', (b'Amount',), 7, True), (b'Exclude', (b'Amount',), 8, False), (b'All', (), 7, False)]:
        pdf, objects = base(action=action, names=names)
        pdf.append({changed: objects[changed].replace(b'(old)', b'(new)')})
        name = action.decode().lower() + ('-unlocked' if allowed else '-locked')
        yield vector(name, pdf, [edit(changed, 'fill')])
    pdf, _ = base()
    body, span = signature()
    positions = pdf.append({4: b'<< /Fields [6 0 R 7 0 R 8 0 R 10 0 R] /SigFlags 3 >>', 9: body, 10: b'<< /FT /Sig /T (Second) /V 9 0 R >>'})
    patch(pdf, positions[9], body, span, revision=1)
    yield vector('signature-added', pdf, [edit(4, 'sign'), edit(9, 'sign'), edit(10, 'sign')])
    pdf, objects = base(widget=True)
    pdf.append({7: objects[7].replace(b'(old)', b'(new)'), 11: stream(b'new')})
    yield vector('fill-with-appearance', pdf, [edit(7, 'fill'), edit(11, 'appearance')])
    pdf, objects = base(widget=True)
    pdf.append({11: stream(b'new')})
    yield vector('appearance-without-fill', pdf, [edit(11, 'appearance')])
    pdf, objects = base(widget=True)
    pdf.append({3: objects[3][:-2] + b' /Contents 11 0 R >>'})
    pdf.append({7: objects[7].replace(b'(old)', b'(new)'), 11: stream(b'new')})
    yield vector('shared-appearance-content', pdf, ['@1', edit(3, 'configuration'), '@0', edit(7, 'fill'), edit(11, 'configuration')])
    pdf, objects = base()
    pdf.append({7: objects[7].replace(b'(old)', b'(new)')})
    pdf.append({7: objects[7]})
    yield vector('fill-then-revert', pdf, ['@1', edit(7, 'fill'), '@0', edit(7, 'fill')])
    pdf, objects = base()
    pdf.append({3: objects[3][:-2] + b' /Contents 10 0 R >>', 10: stream(b'changed')})
    pdf.append({3: objects[3]})
    yield vector('content-then-revert', pdf, ['@1', edit(3, 'configuration'), edit(10, 'configuration'), '@0', edit(3, 'configuration')])
    pdf, _ = base()
    pdf.append({10: b'<< /Type /Annot /Subtype /Text /Contents (orphan) >>'})
    yield vector('orphan-disguised-annotation', pdf, [edit(10, 'unresolved')])
    pdf, _ = base(extra=b'/Extension (not a transform)')
    pdf.append({7: b'<< /FT /Tx /T (Amount) /V (new) >>'}, stream=True)
    yield vector('fill-xref-stream', pdf, [edit(7, 'fill'), edit(51, 'bookkeeping')])
    pdf, objects = base(action=b'Include', names=(b'Amount',))
    pdf.append({7: objects[7].replace(b'(Amount)', b'(Escaped)').replace(b'(old)', b'(new)')})
    yield vector('rename-locked-field', pdf, [edit(7, 'configuration')])
    for locked in (False, True):
        pdf, objects = base(widget=True, action=b'Include' if locked else None, names=(b'Amount',), initial={7: b'<< /FT /Tx /T (Amount) /V 12 0 R /Subtype /Widget /Rect [0 0 1 1] /AP << /N 11 0 R >> >>', 12: stream(b'old')})
        pdf.append({12: stream(b'new'), 11: stream(b'new')})
        yield vector('indirect-fill' + ('-locked' if locked else ''), pdf, [edit(11, 'appearance'), edit(12, 'fill')])
    annotation = b'<< /Type /Annot /Subtype /Text /Rect [1 1 2 2] /Contents (old) /P 3 0 R >>'
    page = b'<< /Type /Page /Parent 2 0 R /MediaBox [0 0 10 10] /Annots [10 0 R] >>'
    for p in (2, 3):
        pdf, objects = base(p, initial={3: page, 10: annotation})
        pdf.append({10: annotation.replace(b'(old)', b'(new)')})
        yield vector(f'annotation-modified-p{p}', pdf, [edit(10, 'annotation')])
    pdf, objects = base(3, initial={3: page, 10: annotation})
    pdf.append({3: objects[3].replace(b'[10 0 R]', b'[]')})
    yield vector('annotation-removed-p3', pdf, [edit(3, 'annotation')])
    pdf, objects = base(3, widget=True, action=b'All')
    pdf.append({3: objects[3].replace(b'[7 0 R]', b'[]')})
    yield vector('locked-widget-removed', pdf, [edit(3, 'annotation')])
    pdf, objects = base(refs_extra=b' << /TransformMethod /Unknown /DigestMethod /SHA256 >>')
    pdf.append({7: objects[7].replace(b'(old)', b'(new)')})
    yield vector('unknown-transform', pdf, [edit(7, 'fill')])
    pdf, objects = base(refs_extra=b' << /TransformMethod /UR /DigestMethod /SHA256 >>')
    pdf.append({7: objects[7].replace(b'(old)', b'(new)')})
    yield vector('usage-rights-unclassified', pdf, [edit(7, 'fill')])
    for p in (1, 2):
        template = b'<< /Type /Template /MediaBox [0 0 10 10] /CropBox [0 0 10 10] /Rotate 0 /Resources << >> /Contents 14 0 R >>'
        pdf, objects = base(p, initial={1: b'<< /Type /Catalog /Pages 2 0 R /AcroForm 4 0 R /Perms << /DocMDP 5 0 R >> /Names << /Templates << /Names [(Blank) 13 0 R] >> >> >>', 13: template, 14: stream(b'0 0 m 1 1 l S')})
        pdf.append({2: b'<< /Type /Pages /Kids [3 0 R 15 0 R] /Count 2 >>', 15: template.replace(b'/Type /Template', b'/Type /Page /Parent 2 0 R')})
        rule = 'template'
        yield vector(f'template-p{p}', pdf, [edit(2, rule), edit(15, rule)])
    pdf, objects = base(action=b'Include', names=(b'Group',), initial={4: b'<< /Fields [6 0 R 16 0 R] /SigFlags 3 >>', 16: b'<< /T (Group) /Kids [7 0 R 8 0 R] >>', 7: b'<< /FT /Tx /T (Amount) /Parent 16 0 R /V (old) >>', 8: b'<< /FT /Tx /T (Recipient) /Parent 16 0 R /V (old) >>'})
    pdf.append({7: objects[7].replace(b'(old)', b'(new)')})
    yield vector('locked-descendant', pdf, [edit(7, 'fill')])
    pdf, objects = base(action=b'Include', names=(b'Amount',))
    pdf.append({6: objects[6][:-2] + b' /Lock 12 0 R >>', 12: b'<< /Action /Exclude /Fields [(Amount)] >>', 7: objects[7].replace(b'(old)', b'(new)')})
    yield vector('later-lock-cannot-unlock', pdf, [edit(6, 'configuration'), edit(7, 'fill'), edit(12, 'unresolved')])
    pdf, objects = base()
    pdf.append({4: b'<< /Fields [7 0 R 8 0 R] /SigFlags 3 >>'})
    yield vector('removed-signature', pdf, [edit(4, 'configuration')])
    pdf, objects = base()
    body, span = signature()
    positions = pdf.append({6: b'<< /FT /Sig /T (Approval) /V 9 0 R >>', 9: body})
    patch(pdf, positions[9], body, span, revision=1)
    yield vector('replaced-signature', pdf, [edit(6, 'configuration'), edit(9, 'sign')])
    pdf, objects = base(initial={7: b'<< /FT /Tx /T (Amount) /V 12 0 R >>', 12: b'<< /Length 13 0 R >>\nstream\nold\nendstream', 13: b'3'})
    pdf.append({12: b'<< /Length 13 0 R >>\nstream\nnewtext\nendstream', 13: b'7'})
    yield vector('historical-stream-length', pdf, [edit(12, 'fill'), edit(13, 'fill')])
    pdf, objects = base(initial={3: b'<< /Type /Page /Parent 2 0 R /MediaBox [0 0 10 10] /Resources 12 0 R >>', 12: b'<< /ProcSet [/PDF] >>'})
    pdf.append({12: b'<< /ProcSet [/PDF /Text] >>'})
    yield vector('page-resource-change', pdf, [edit(12, 'configuration')])
    pdf, objects = base(initial={7: b'<< /FT /Tx /T (Amount) /V 12 0 R >>', 12: b'(old)'})
    old_xref = pdf.previous
    positions = pdf.append({12: b'(new)'})
    entry = old_xref + len(b'xref\n0 13\n') + 12 * len(b'0000000000 00000 n \n')
    assert pdf.data[entry + 10:entry + 19] == b' 00000 n '
    pdf.data[entry:entry + 10] = str(positions[12] - len(b'12 0 obj\n')).encode().rjust(10, b'0')
    pdf.author_history_intact = False
    yield vector('historical-forward-reference', pdf, [])
    pdf, objects = base(3, initial={3: page, 10: annotation})
    pdf.update({(3, 0): objects[3].replace(b'[10 0 R]', b'[]')}, freed={10: 1})
    yield vector('annotation-deleted-p3', pdf, [edit(3, 'annotation'), edit(10, 'annotation')])
    pdf, objects = base(action=b'All')
    pdf.update({(4, 0): b'<< /Fields [6 0 R 7 1 R 8 0 R] /SigFlags 3 >>', (7, 1): b'<< /FT /Tx /T (Amount) /V (new) >>'})
    yield vector('field-generation-replaced', pdf, [edit(4, 'configuration'), edit(7, 'configuration'), edit('7:1', 'configuration')])
    pdf, objects = base(widget=True, action=b'All', initial={3: b'<< /Type /Page /Parent 2 0 R /MediaBox [0 0 10 10] /Annots 12 0 R >>', 12: b'[7 0 R]'})
    pdf.append({12: b'[]'})
    yield vector('indirect-locked-widget-removal', pdf, [edit(12, 'annotation')])
    pdf, objects = base()
    body, span = signature()
    positions = pdf.append({4: b'<< /Fields [6 0 R 7 0 R 8 0 R 10 0 R] /SigFlags 3 >>', 3: objects[3][:-2] + b' /Annots [10 0 R] >>', 9: body, 10: b'<< /FT /Sig /T (Second) /V 9 0 R /Subtype /Widget /Rect [0 0 1 1] /P 3 0 R >>'})
    patch(pdf, positions[9], body, span, revision=1)
    yield vector('signature-widget-added', pdf, [edit(3, 'sign'), edit(4, 'sign'), edit(9, 'sign'), edit(10, 'sign')])
    pdf, objects = base(1)
    body, span = signature(b'ETSI.RFC3161')
    positions = pdf.append({4: b'<< /Fields [6 0 R 7 0 R 8 0 R 10 0 R] /SigFlags 3 >>', 9: body, 10: b'<< /FT /Sig /T (Timestamp) /V 9 0 R >>'})
    patch(pdf, positions[9], body, span, revision=1)
    yield vector('timestamp-exception-unclassified', pdf, [edit(4, 'unresolved'), edit(9, 'unresolved'), edit(10, 'unresolved')])
    pdf, objects = base(1)
    pdf.append({1: objects[1][:-2] + b' /DSS 12 0 R >>', 12: b'<< /VRI << >> >>'})
    yield vector('dss-exception-unclassified', pdf, [edit(1, 'unresolved'), edit(12, 'unresolved')])
    pdf, objects = base(initial={6: b'<< /FT /Sig /T (Approval) /V 5 0 R /Lock 12 0 R >>', 12: b'<< /Action /Include /Fields [(Amount)] >>'})
    pdf.append({7: objects[7].replace(b'(old)', b'(new)')})
    yield vector('signed-field-lock', pdf, [edit(7, 'fill')])
    for stable in (True, False):
        pdf, objects = base(trailer=b' /ID [<0101> <0101>] ')
        pdf.append({7: objects[7].replace(b'(old)', b'(new)')}, trailer=b' /ID [' + (b'<0101>' if stable else b'<0303>') + b' <0202>] ')
        yield vector('trailer-' + ('changing-id' if stable else 'permanent-id-changed'), pdf, [edit(7, 'fill'), edit('trailer', 'identifier')])
    pdf, objects = base()
    pdf.append({7: objects[7].replace(b'(old)', b'(new)')}, trailer=b' /Extension 42 ')
    yield vector('unknown-trailer-entry', pdf, [edit(7, 'fill'), edit('trailer', 'unresolved')])
    initial = {4: b'<< /Fields [6 0 R 16 0 R] /SigFlags 3 >>', 16: b'<< /T (Group) /FT /Tx /Kids [7 0 R 8 0 R] >>', 7: b'<< /T (Amount) /Parent 16 0 R /V (old) >>', 8: b'<< /T (Recipient) /Parent 16 0 R /V (old) >>'}
    pdf, objects = base(action=b'Include', names=(b'Group.Amount',), initial=initial)
    pdf.append({16: objects[16].replace(b'(Group)', b'(Renamed)')})
    yield vector('locked-descendant-renamed', pdf, [edit(16, 'configuration')])
    pdf, objects = base(action=b'Include', names=(b'Group.Amount',), initial=initial)
    pdf.append({16: objects[16].replace(b'/FT /Tx', b'/FT /Ch')})
    yield vector('inherited-field-configuration', pdf, [edit(16, 'configuration')])
    pdf, objects = base(action=b'Include', names=(b'Amount',), field_data=b'7 0 R')
    pdf.append({8: objects[8].replace(b'(old)', b'(new)')})
    yield vector('fieldmdp-data-unclassified', pdf, [edit(8, 'fill')])
    body, span = signature()
    pdf = Pdf()
    positions = pdf.append(structure() | {5: body})
    wrong_gap = lambda a, b, n: (0, a - 1, b, n - b)
    patch(pdf, positions[5], body, span, ranges=wrong_gap)
    positions = pdf.append({5: body})
    patch(pdf, positions[5], body, span, revision=1, ranges=wrong_gap)
    yield Vector('replaced-malformed-signatures', {'.pdf': bytes(pdf.data), '.history': b'1|Approval|5|NotAnalysedYet\n'}, ('signature-permitted-changes',), 'Distinct malformed signature sources remain visible')
    pdf, objects = base(refs_extra=b' << /TransformMethod /UR /DigestMethod /SHA256 >>', trailer=b' /ID [<0101> <0101>] ')
    pdf.append({7: objects[7]}, trailer=b' /ID [<0101> <0202>] ')
    yield vector('usage-rights-trailer-unclassified', pdf, [edit('trailer', 'bookkeeping')])
    pdf, objects = base(action=b'Include', names=(b'Amount',), field_data=b'7 0 R', trailer=b' /ID [<0101> <0101>] ')
    pdf.append({8: objects[8]}, trailer=b' /ID [<0101> <0202>] ')
    yield vector('fieldmdp-trailer-unclassified', pdf, [edit('trailer', 'bookkeeping')])
    for prior, next_value, allowed in [(3, 0, False), (0, 3, True)]:
        pdf, objects = base(initial={4: b'<< /Fields [6 0 R 7 0 R 8 0 R] /SigFlags %d >>' % prior})
        body, span = signature()
        positions = pdf.append({4: b'<< /Fields [6 0 R 7 0 R 8 0 R 10 0 R] /SigFlags %d >>' % next_value, 9: body, 10: b'<< /FT /Sig /T (Second) /V 9 0 R >>'})
        patch(pdf, positions[9], body, span, revision=1)
        yield vector('signature-flags-' + ('set' if allowed else 'cleared'), pdf, [edit(4, 'sign'), edit(9, 'sign'), edit(10, 'sign')])
    pdf, objects = base(widget=True, trailer=b' /Info 12 0 R ', initial={11: b'<< /Length 3 /Resources << /Properties << /Info 12 0 R >> >> >>\nstream\nold\nendstream', 12: b'<< /Title (old) >>'})
    pdf.append({7: objects[7].replace(b'(old)', b'(new)'), 12: b'<< /Title (new) >>'})
    yield vector('appearance-shares-document-info', pdf, [edit(7, 'fill'), edit(12, 'information')])
    for p in (2, 3):
        pdf, objects = base(p, widget=True)
        pdf.append({7: objects[7].replace(b'/Rect [0 0 1 1]', b'/Rect [0 0 2 2]')})
        yield vector(f'widget-rectangle-p{p}', pdf, [edit(7, 'annotation')])
    pdf, objects = base(3, widget=True, action=b'Include', names=(b'Amount',))
    pdf.append({7: objects[7].replace(b'/Rect [0 0 1 1]', b'/Rect [0 0 2 2]')})
    yield vector('locked-widget-rectangle', pdf, [edit(7, 'annotation')])
    pdf, objects = base(3, widget=True)
    pdf.append({11: stream(b'new')})
    yield vector('widget-appearance-p3', pdf, [edit(11, 'widget-appearance')])
    pdf, objects = base(initial={7: b'<< /FT /Tx /T (Amount) /V (old) /AP << /N 11 0 R >> >>', 11: stream(b'old')})
    pdf.append({7: objects[7].replace(b'(old)', b'(new)'), 11: stream(b'new')})
    yield vector('non-widget-appearance-unclassified', pdf, [edit(7, 'fill'), edit(11, 'unresolved')])
    template = b'<< /Type /Template /MediaBox [0 0 10 10] /CropBox [0 0 10 10] /Rotate 0 /Resources << >> /Annots [17 0 R] >>'
    pdf, objects = base(initial={1: b'<< /Type /Catalog /Pages 2 0 R /AcroForm 4 0 R /Perms << /DocMDP 5 0 R >> /Names << /Templates << /Names [(Form) 13 0 R] >> >> >>', 4: b'<< /Fields [6 0 R 7 0 R 8 0 R 17 0 R] /SigFlags 3 >>', 13: template, 17: b'<< /FT /Tx /T (TemplateAmount) /V (old) /Subtype /Widget /Rect [0 0 1 1] >>'})
    pdf.append({2: b'<< /Type /Pages /Kids [3 0 R 15 0 R] /Count 2 >>', 4: b'<< /Fields [6 0 R 7 0 R 8 0 R 17 0 R 18 0 R] /SigFlags 3 >>', 15: template.replace(b'/Type /Template', b'/Type /Page /Parent 2 0 R').replace(b'[17 0 R]', b'[18 0 R]'), 18: b'<< /FT /Tx /T (CopyAmount) /V (old) /Subtype /Widget /Rect [0 0 1 1] /P 15 0 R >>'})
    yield vector('template-fields-unclassified', pdf, [edit(2, 'template'), edit(4, 'configuration'), edit(15, 'template'), edit(18, 'configuration')])

    # One rendering resource is shared by two widgets. Filling Amount cannot
    # authorize replacing Recipient's unchanged appearance (Table 257).
    def appearance(resource, kind):
        content = b'BT /F1 1 Tf (x) Tj ET' if kind == b'Font' else b'/F1 Do'
        return (b'<< /Type /XObject /Subtype /Form /BBox [0 0 1 1] /Length %d /Resources << /' % len(content) + kind
                + b' << /F1 %d 0 R >> >> >>\nstream\n' % resource + content + b'\nendstream')
    def form_xobject(gray):
        content = gray + b' g 0 0 1 1 re f'
        return b'<< /Type /XObject /Subtype /Form /BBox [0 0 1 1] /Length %d >>\nstream\n' % len(content) + content + b'\nendstream'
    for kind in (b'Font', b'XObject'):
        for both in (False, True):
            initial = {
                3: b'<< /Type /Page /Parent 2 0 R /MediaBox [0 0 10 10] /Annots [7 0 R 8 0 R] >>',
                7: b'<< /FT /Tx /T (Amount) /V (old) /Subtype /Widget /Rect [0 0 1 1] /P 3 0 R /AP << /N 11 0 R >> >>',
                8: b'<< /FT /Tx /T (Recipient) /V (old) /Subtype /Widget /Rect [0 0 1 1] /P 3 0 R /AP << /N 12 0 R >> >>',
                11: appearance(20, kind), 12: appearance(20, kind),
                20: b'<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>' if kind == b'Font' else form_xobject(b'0'),
            }
            pdf, objects = base(2, initial=initial)
            update = {7: objects[7].replace(b'(old)', b'(new)'),
                      20: objects[20].replace(b'Helvetica', b'Symbol') if kind == b'Font' else form_xobject(b'1')}
            edits = [edit(7, 'fill'), edit(20, 'appearance', ('Amount', 'Recipient'))]
            if both:
                update[8] = objects[8].replace(b'(old)', b'(new)')
                edits.append(edit(8, 'fill'))
            pdf.append(update)
            yield vector('shared-' + kind.decode().lower() + ('-both-filled' if both else '-p2'), pdf, edits)

    pdf, objects = base(2, trailer=b' /Info 12 0 R ', initial={12: b'<< /ModDate (D:2020) >>'})
    body, span = signature()
    positions = pdf.append({4: b'<< /Fields [6 0 R 7 0 R 8 0 R 10 0 R] /SigFlags 3 >>',
                            9: body, 10: b'<< /FT /Sig /T (Second) /V 9 0 R >>',
                            12: b'<< /ModDate (D:2021) >>'})
    patch(pdf, positions[9], body, span, revision=1)
    yield vector('info-moddate-signing', pdf, [edit(4, 'sign'), edit(9, 'sign'), edit(10, 'sign'), edit(12, 'information')])

    for entry in (b'Names', b'OCProperties', b'DR'):
        if entry == b'Names':
            initial = {21: b'<< /Dests 22 0 R >>', 22: b'<< /Names [(Old) [3 0 R /Fit]] >>'}
            target, replacement = 22, b'<< /Names [(New) [3 0 R /Fit]] >>'
        elif entry == b'OCProperties':
            initial = {21: b'<< /OCGs [22 0 R] /D << /Name (Old) >> >>',
                       22: b'<< /Type /OCG /Name (Layer) >>'}
            target, replacement = 21, initial[21].replace(b'(Old)', b'(New)')
        else:
            initial = {21: b'<< /Font << /F1 22 0 R >> >>',
                       22: b'<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>'}
            target, replacement = 22, initial[22].replace(b'/Helvetica', b'/Symbol')
        if entry == b'DR':
            initial[4] = b'<< /Fields [6 0 R 7 0 R 8 0 R] /SigFlags 3 /DR 21 0 R >>'
        else:
            initial[1] = b'<< /Type /Catalog /Pages 2 0 R /AcroForm 4 0 R /Perms << /DocMDP 5 0 R >> /' + entry + b' 21 0 R >>'
        pdf, objects = base(2, initial=initial)
        pdf.append({target: replacement})
        yield vector('indirect-' + entry.decode().lower(), pdf, [edit(target, 'unresolved')])
    for key, value, target in ((b'OpenAction', b'<< /S /JavaScript /JS (inert) >>', 1),
                               (b'NeedAppearances', b'true', 4)):
        pdf, objects = base(2)
        pdf.append({target: objects[target][:-2] + b' /' + key + b' ' + value + b' >>'})
        yield vector('changed-' + key.decode().lower(), pdf, [edit(target, 'configuration')])

    pdf, objects = base(2)
    pdf.append({4: b'<< /Fields [6 0 R 7 0 R 8 0 R 10 0 R] /SigFlags 3 >>',
                10: b'<< /FT /Tx /T (Added) /V (text) >>'})
    yield vector('added-unsigned-field', pdf, [edit(4, 'configuration'), edit(10, 'configuration')])

    # Compressed fills, including a container-only rewrite whose unchanged
    # type-2 entries still select its new member bodies (7.5.7-7.5.8).
    for rewrite in (False, True):
        pdf, objects = base(2)
        pdf.compressed({7: objects[7].replace(b'(old)', b'(new)')})
        edits = [edit(7, 'fill'), edit(40, 'bookkeeping'), edit(51, 'bookkeeping')]
        if rewrite:
            pdf.compressed({7: objects[7].replace(b'(old)', b'(next)')}, list_members=False)
            edits = ['@1', *edits, '@0', edit(7, 'fill'), edit(40, 'bookkeeping'), edit(52, 'bookkeeping')]
        yield vector('object-stream-' + ('redefined' if rewrite else 'fill'), pdf, edits)

    # Only the indirect Length changes: the stream has no new xref entry.
    pdf, objects = base(2, initial={
        3: b'<< /Type /Page /Parent 2 0 R /MediaBox [0 0 10 10] /Contents 12 0 R >>',
        12: b'<< /Length 13 0 R >>\nstream\nold\nendstream', 13: b'3'})
    pdf.append({13: b'4'})
    pdf.author_states[-1]['12'] += b'\n'  # authored stream extent includes its EOL
    yield vector('indirect-length-only', pdf, [edit(12, 'page-content'), edit(13, 'page-content')])

    # An appearance can cover signed text while remaining an annotation
    # operation allowed under P=3. The host needs a separate overlay warning.
    for permission in (2, 3):
        pdf, objects = base(permission, initial={
            3: b'<< /Type /Page /Parent 2 0 R /MediaBox [0 0 10 10] /Contents 12 0 R >>',
            12: stream(b'BT (signed content) Tj ET')})
        pdf.append({3: objects[3][:-2] + b' /Annots [10 0 R] >>',
                    10: b'<< /Type /Annot /Subtype /Stamp /Rect [0 0 10 10] /P 3 0 R /AP << /N 11 0 R >> >>',
                    11: b'<< /Type /XObject /Subtype /Form /BBox [0 0 10 10] /Length 18 >>\nstream\n1 g 0 0 10 10 re f\nendstream'})
        yield vector(f'visible-overlay-p{permission}', pdf, [edit(3, 'annotation'), edit(10, 'annotation'), edit(11, 'annotation-appearance')])

    # Signing widgets and resized existing widgets can obscure signed content
    # even when the signing/annotation operation is permitted (12.5.5, 12.8.2.2).
    opaque = b'<< /Type /XObject /Subtype /Form /BBox [0 0 10 10] /Length 18 >>\nstream\n1 g 0 0 10 10 re f\nendstream'
    page = b'<< /Type /Page /Parent 2 0 R /MediaBox [0 0 10 10] /Contents 30 0 R >>'
    pdf, objects = base(2, initial={3: page, 30: stream(b'BT (pay 10) Tj ET')})
    body, span = signature()
    positions = pdf.append({
        3: page[:-2] + b' /Annots [10 0 R] >>',
        4: b'<< /Fields [6 0 R 7 0 R 8 0 R 10 0 R] /SigFlags 3 >>',
        9: body,
        10: b'<< /FT /Sig /T (Second) /V 9 0 R /Subtype /Widget /Rect [0 0 10 10] /P 3 0 R /AP << /N 11 0 R >> >>',
        11: opaque})
    patch(pdf, positions[9], body, span, revision=1)
    yield vector('signature-widget-overlay-p2', pdf, [edit(3, 'sign'), edit(4, 'sign'), edit(9, 'sign'), edit(10, 'sign'), edit(11, 'signing-appearance', ('Second',))])

    pdf, objects = base(3, widget=True, initial={
        3: page[:-2] + b' /Annots [7 0 R] >>', 30: stream(b'BT (pay 10) Tj ET')})
    pdf.append({7: objects[7].replace(b'[0 0 1 1]', b'[0 0 10 10]'), 11: opaque})
    yield vector('widget-overlay-p3', pdf, [edit(7, 'annotation'), edit(11, 'widget-appearance')])

    pdf, objects = base(3, initial={
        3: page[:-2] + b' /Annots [12 0 R] >>', 30: stream(b'BT (pay 10) Tj ET'),
        7: b'<< /FT /Tx /T (Amount) /V (old) /Kids [12 0 R] >>',
        11: stream(b''),
        12: b'<< /Subtype /Widget /Parent 7 0 R /Rect [0 0 1 1] /P 3 0 R /AP << /N 11 0 R >> >>'})
    pdf.append({12: objects[12].replace(b'[0 0 1 1]', b'[0 0 10 10]'), 11: opaque})
    yield vector('separate-widget-overlay-p3', pdf, [edit(12, 'annotation', ('Amount',)), edit(11, 'widget-appearance')])

    for null in (False, True):
        field = b'<< /FT /Tx /T (Amount) /V (old) /Subtype /Widget /Rect [0 0 10 10]' + (b' /AP null' if null else b'') + b' >>'
        pdf, objects = base(2, widget=True, initial={7: field})
        pdf.append({7: field.replace(b'(old)', b'(new)').replace(b' /AP null', b'')[:-2] + b' /AP << /N 11 0 R >> >>', 11: opaque})
        yield vector('field-' + ('null-' if null else '') + 'appearance-added', pdf, [edit(7, 'fill'), edit(11, 'appearance')])

    # An indirect null or undefined AP is absent (7.3.9-7.3.10). Turning it
    # into an appearance during a fill needs the same overlay warning.
    for defined in (False, True):
        initial = {7: b'<< /FT /Tx /T (Amount) /V (old) /Subtype /Widget /Rect [0 0 10 10] /AP 12 0 R >>'}
        if defined:
            initial[12] = b'null'
        pdf, objects = base(2, widget=True, initial=initial)
        pdf.append({7: objects[7].replace(b'(old)', b'(new)'), 11: opaque, 12: b'<< /N 11 0 R >>'})
        yield vector('field-' + ('indirect-null' if defined else 'undefined') + '-appearance-added', pdf, [edit(7, 'fill'), edit(11, 'appearance'), edit(12, 'appearance')])

    # An inconsistent unsigned page tree predates the signed revision. It is
    # outside both comparisons and cannot invalidate a later ordinary fill.
    signed, objects = base(2)
    pdf = Pdf()
    pdf.append({1: b'<< /Type /Catalog /Pages 2 0 R >>',
                2: b'<< /Type /Pages /Kids [3 0 R] /Count 2 >>', 3: objects[3]})
    positions = pdf.append(objects)
    body, span = signature(extra=b'/Reference [' + docmdp(2) + b'] ')
    patch(pdf, positions[5], body, span, revision=1)
    for attribute in ('author_permission', 'author_selection', 'author_unknown_transform', 'author_field_unknown'):
        setattr(pdf, attribute, getattr(signed, attribute))
    pdf.author_states = pdf.author_states[1:]
    pdf.append({7: objects[7].replace(b'(old)', b'(new)')})
    yield vector('inconsistent-unsigned-base', pdf, [edit(7, 'fill')])

    # Unused slots are not interpretation roots. Undefined reachable references
    # are null (7.3.10); neither changes this form-filling operation.
    for name, initial in (
        ('unused-invalid-slot', {99: b'not-a-value'}),
        ('undefined-names-reference', {1: b'<< /Type /Catalog /Pages 2 0 R /AcroForm 4 0 R /Perms << /DocMDP 5 0 R >> /Names 99 0 R >>'}),
        ('undefined-annots-reference', {3: b'<< /Type /Page /Parent 2 0 R /MediaBox [0 0 10 10] /Annots 99 0 R >>'}),
        ('undefined-resource-reference', {3: b'<< /Type /Page /Parent 2 0 R /MediaBox [0 0 10 10] /Resources << /Font << /Unused 99 0 R >> >> >>'}),
    ):
        pdf, objects = base(2, initial=initial)
        pdf.append({7: objects[7].replace(b'(old)', b'(new)')})
        yield vector(name, pdf, [edit(7, 'fill')])

    for marker, trailer in ((False, b''), (True, b''), (False, b' /ID [<0101> <0101>] ')):
        pdf = linearized_base(marker, trailer)
        body, span = signature(extra=b'/Reference [' + docmdp(2) + b'] ')
        positions = pdf.append({
            1: b'<< /Type /Catalog /Pages 2 0 R /AcroForm 7 0 R /Perms << /DocMDP 10 0 R >> >>',
            7: b'<< /Fields [8 0 R 9 0 R] /SigFlags 3 >>',
            8: b'<< /FT /Sig /T (Approval) /V 10 0 R >>',
            9: b'<< /FT /Tx /T (Amount) /V (old) >>', 10: body})
        patch(pdf, positions[10], body, span, revision=1)
        pdf.author_states = pdf.author_states[-1:]
        pdf.author_permission, pdf.author_selection = 2, (None, ())
        pdf.author_unknown_transform = pdf.author_field_unknown = False
        pdf.append({9: b'<< /FT /Tx /T (Amount) /V (new) >>'})
        yield vector('linearized-base' + ('-early-eof' if marker else '-main-id' if trailer else ''), pdf, [edit(9, 'fill')])

    # Xref re-pointing can change content even without a new object body. Both
    # definitions below are authored in the signed bytes; only the selected
    # definition changes in the update.
    old, replacement = stream(b'old'), stream(b'new')
    pdf, objects = base(2, initial={
        3: b'<< /Type /Page /Parent 2 0 R /MediaBox [0 0 10 10] /Contents 12 0 R >>',
        12: old + b'\nendobj\n12 0 obj\n' + replacement})
    duplicate = pdf.data.index(b'12 0 obj\n', pdf.data.index(b'12 0 obj\n') + 1)
    pdf.append({12: old})
    at = pdf.previous + len(b'xref\n12 1\n')
    pdf.data[at:at+10] = b'%010d' % duplicate
    pdf.author_states[0]['12'] = old
    pdf.author_states[-1]['12'] = replacement
    yield vector('xref-repointed-content', pdf, [edit(12, 'page-content')])

    pdf, objects = base(2)
    positions = pdf.append({7: objects[7].replace(b'(old)', b'(new)')})
    at = pdf.previous + len(b'xref\n7 1\n')
    pdf.data[at:at+10] = b'%010d' % (positions[7] - len(b'7 0 obj\n') + 1)
    pdf.author_history_intact = False
    yield vector('repaired-xref-history', pdf, [edit(7, 'fill')])

    template = b'<< /Type /Template /MediaBox [0 0 10 10] >>'
    pdf, objects = base(2, initial={
        1: b'<< /Type /Catalog /Pages 2 0 R /AcroForm 4 0 R /Perms << /DocMDP 5 0 R >> /Names << /Templates << /Names [(Blank) 13 0 R] >> >> >>',
        13: template})
    pdf.append({2: b'<< /Type /Pages /Kids [3 0 R 15 0 R] /Count 2 >>',
                15: template.replace(b'/Type /Template', b'/Type /Page /Parent 2 0 R'),
                3: objects[3][:-2] + b' /Contents 14 0 R >>', 14: stream(b'changed')})
    yield vector('template-with-content-change', pdf, [edit(2, 'template'), edit(3, 'configuration'), edit(14, 'page-content'), edit(15, 'template')])

    # A second field pointing at the old certification dictionary is not a
    # signing operation: that signature's ByteRange ends before this field.
    pdf, objects = base(2)
    pdf.append({4: b'<< /Fields [6 0 R 7 0 R 8 0 R 10 0 R] /SigFlags 3 >>',
                10: b'<< /FT /Sig /T (Reused) /V 5 0 R >>'})
    yield vector('reattached-old-signature', pdf, [edit(4, 'configuration'), edit(10, 'unresolved')])
