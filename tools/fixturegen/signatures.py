"""Independently authored PDF signature structures, without cryptographic claims.

ISO 32000-2:2020, 7.5.6, 12.7.5.5, 12.8.1–12.8.3. The placeholder
Contents contains a small DER-shaped value, not a real CMS signature. Offsets
and expected ranges come from authoring positions, never from a PDF reader.
"""
from .model import Vector

AREA = 'signatures'
CONTENTS = bytes.fromhex('3003040100') + bytes(15)
SLOT = b' '.join([b'@' * 10] * 4)


class Pdf:
    def __init__(self):
        self.data = bytearray(b'%PDF-2.0\n%\xe2\xe3\xcf\xd3\n')
        self.previous = None
        self.size = 0
        self.revisions = []

    def append(self, objects, stream=False, eof=b'\n'):
        positions = {}
        offsets = {}
        for number, body in sorted(objects.items()):
            offsets[number] = len(self.data)
            self.data += b'%d 0 obj\n' % number
            positions[number] = len(self.data)
            self.data += body + b'\nendobj\n'
        self.size = max(self.size, max(objects, default=0) + 1)
        at = len(self.data)
        if stream:
            number = 50 + len(self.revisions)
            offsets[number] = at
            self.size = max(self.size, number + 1)
            entries = {0: (0, 0, 65535)} | {n: (1, o, 0) for n, o in offsets.items()}
            index = b' '.join(b'%d 1' % n for n in sorted(entries))
            data = b''.join(bytes([t]) + o.to_bytes(4, 'big') + g.to_bytes(2, 'big')
                            for n, (t, o, g) in sorted(entries.items()))
            prev = b'' if self.previous is None else b' /Prev %d' % self.previous
            self.data += (b'%d 0 obj\n<< /Type /XRef /Size %d /Root 1 0 R /W [1 4 2] /Index ['
                          % (number, self.size) + index + b'] /Length %d' % len(data) + prev
                          + b' >>\nstream\n' + data + b'\nendstream\nendobj\n')
        else:
            self.data += b'xref\n'
            if self.previous is None:
                self.data += b'0 %d\n0000000000 65535 f \n' % self.size
                for n in range(1, self.size):
                    self.data += b'%010d 00000 n \n' % offsets[n] if n in offsets else b'0000000000 00000 f \n'
            else:
                for n, offset in sorted(offsets.items()):
                    self.data += b'%d 1\n%010d 00000 n \n' % (n, offset)
            prev = b'' if self.previous is None else b' /Prev %d' % self.previous
            self.data += b'trailer\n<< /Size %d /Root 1 0 R' % self.size + prev + b' >>\n'
        self.data += b'startxref\n%d\n%%%%EOF' % at + eof
        self.previous = at
        self.revisions.append(len(self.data))
        return positions


def structure(field_value=b'5 0 R', field_extra=b'', catalog_extra=b''):
    return {
        1: b'<< /Type /Catalog /Pages 2 0 R /AcroForm 4 0 R ' + catalog_extra + b' >>',
        2: b'<< /Type /Pages /Kids [3 0 R] /Count 1 >>',
        3: b'<< /Type /Page /Parent 2 0 R /MediaBox [0 0 10 10] >>',
        4: b'<< /Fields [6 0 R] /SigFlags 3 >>',
        6: b'<< /FT /Sig /T (Approval)' + (b' /V ' + field_value if field_value else b'') + b' ' + field_extra + b' >>',
    }


def signature(subfilter=b'adbe.pkcs7.detached', extra=b'', contents=None, escaped=False, literal=False):
    payload = CONTENTS if contents is None else contents
    prefix = (b'<< /Type ' + (b'/DocTimeStamp' if subfilter == b'ETSI.RFC3161' else b'/Sig')
              + b' /Filter /Adobe.PPKLite /SubFilter /' + subfilter
              + b' /ByteRange [' + SLOT + b'] ' + extra
              + (b' /Cont#65nts ' if escaped else b' /Contents '))
    token = b'(' + payload.hex().encode() + b')' if literal else b'<' + payload.hex().encode() + b'>'
    return prefix + token + b' >>', (len(prefix), len(prefix) + len(token))


def patch(pdf, body_at, body, span, revision=0, ranges=None):
    start, end = (body_at + span[0], body_at + span[1])
    values = (0, start, end, pdf.revisions[revision] - end) if ranges is None else ranges(start, end, pdf.revisions[revision])
    encoded = b' '.join(str(n).encode().rjust(10, b' ') for n in values)
    assert len(encoded) == len(SLOT)
    slot = body_at + body.index(SLOT)
    pdf.data[slot:slot + len(SLOT)] = encoded
    return values


def vector(name, pdf, rows, req='signature-byte-range', error=None):
    expectation = '\n'.join(rows) + '\n' if error is None else 'error|' + error + '\n'
    return Vector(name, {'.pdf': bytes(pdf.data), '.expect': expectation.encode()}, (req,),
                  'Independently authored signature structure: ' + name)


def row(name='Approval', integrity='ByteRangeCheckedCryptographyNotChecked', revision='0', later=0,
        algorithms='NotCheckedYet', ranges=None, contents=CONTENTS, refs='_', lock='_', metadata='_'):
    fields = [name, integrity, revision, str(later), algorithms,
              ','.join(map(str, ranges)) if ranges else '_', contents.hex(), refs, lock, metadata]
    return '|'.join(fields)


def single(name, *, subfilter=b'adbe.pkcs7.detached', extra=b'', field_extra=b'', other=None,
           expected=None, ranges=None, direct=False, stream=False, escaped=False, literal=False,
           eof=b'\n', req='signature-byte-range', error=None, **kwargs):
    body, span = signature(subfilter, extra, escaped=escaped, literal=literal)
    objects = structure(field_value=body if direct else b'5 0 R', field_extra=field_extra)
    if not direct:
        objects[5] = body
    if other:
        objects.update(other)
    pdf = Pdf()
    positions = pdf.append(objects, stream=stream, eof=eof)
    body_at = positions[6] + objects[6].index(body) if direct else positions[5]
    actual = patch(pdf, body_at, body, span, ranges=ranges)
    if expected is not None:
        kwargs.setdefault('revision', 'NotCheckedYet')
    if literal:
        kwargs.setdefault('contents', CONTENTS.hex().encode())
    return vector(name, pdf, [row(integrity=expected or 'ByteRangeCheckedCryptographyNotChecked',
                                 ranges=actual if expected is None else None, **kwargs)], req, error)


def cases():
    yield single('structure', req='signature-fields')
    yield single('gap-mismatch', ranges=lambda a, b, n: (0, a - 1, b, n - b), expected='ContentsGapMismatch')
    yield single('overlap', ranges=lambda a, b, n: (0, b + 1, b, n - b), expected='OverlappingRanges')
    yield single('outside-file', ranges=lambda a, b, n: (0, a, b, n), expected='OutsideFile')
    yield single('negative-offset', ranges=lambda a, b, n: (0, a, -1, n - b), expected='InvalidInteger')
    yield single('negative-length', ranges=lambda a, b, n: (0, -1, b, n - b), expected='InvalidInteger')
    yield single('nonzero-first-offset', ranges=lambda a, b, n: (1, a, b, n - b), expected='DoesNotStartAtZero')
    yield single('outside-first-range', ranges=lambda a, b, n: (0, n + 1, b, n - b), expected='OutsideFile')
    yield single('outside-second-offset', ranges=lambda a, b, n: (0, a, n + 1, 0), expected='OutsideFile')
    yield single('unsigned-tail', ranges=lambda a, b, n: (0, a, b, 0), expected='SignatureOutsideSignedBytes')
    yield single('direct-value', direct=True, req='signature-fields')
    yield single('escaped-contents-name', escaped=True)
    yield single('literal-contents', literal=True, expected='ContentsNotHex')
    yield single('nested-decoy', extra=b'/Extension << /Contents <0000> >> /ExtensionText (text /Contents <1234>) % /Contents <abcd>\n')
    yield single('metadata', extra=b'/M (D:20261018120000Z) /Name (Signer) /Reason (Review) /Location (Berlin)',
                 metadata='D:20261018120000Z,Signer,Review,Berlin', req='signature-fields')
    yield single('cades', subfilter=b'ETSI.CAdES.detached', req='signature-fields')
    yield single('timestamp', subfilter=b'ETSI.RFC3161', req='signature-fields')
    yield single('legacy-pkcs7-sha1', subfilter=b'adbe.pkcs7.sha1', algorithms='WeakSha1Unsupported', req='signature-results')
    yield single('legacy-x509-sha1', subfilter=b'adbe.x509.rsa_sha1', algorithms='LegacyEncodingUnsupported', req='signature-results')
    yield single('unknown-subfilter', subfilter=b'Example.encoding', algorithms='UnsupportedEncoding', req='signature-results')
    yield single('xref-stream', stream=True, req='signature-revision-coverage')
    yield single('eof-no-eol', eof=b'', req='signature-revision-coverage')
    yield single('eof-crlf', eof=b'\r\n', req='signature-revision-coverage')
    # A declared revision can end at %%EOF without the optional final EOL.
    yield single('coverage-without-final-eol', ranges=lambda a, b, n: (0, a, b, n - b - 1), req='signature-revision-coverage')
    yield single('not-revision-boundary', ranges=lambda a, b, n: (0, a, b, n - b - 3), revision='NotRevisionBoundary', req='signature-revision-coverage')
    for permission in (1, 2, 3):
        transform = b'/Reference [<< /Type /SigRef /TransformMethod /DocMDP /DigestMethod /SHA256 /TransformParams << /Type /TransformParams /P %d /V /1.2 >> >>]' % permission
        yield single('docmdp-' + str(permission), extra=transform, refs='DocMDP:' + str(permission), req='signature-transforms')
    yield single('docmdp-default', extra=b'/Reference [<< /TransformMethod /DocMDP /DigestMethod /SHA256 >>]', refs='DocMDP:2', req='signature-transforms')
    yield single('fieldmdp', extra=b'/Reference [<< /TransformMethod /FieldMDP /DigestMethod /SHA256 /Data 1 0 R /TransformParams << /Type /TransformParams /V /1.2 /Action /Include /Fields [(Amount) (Recipient)] >> >>]', refs='FieldMDP:Include:Amount,Recipient', req='signature-transforms')
    yield single('ur-transform', extra=b'/Reference [<< /TransformMethod /UR /DigestMethod /SHA256 /TransformParams << /V /2.2 /Form [/FillIn] /Signature [/Modify] >> >>]', refs='UR', req='signature-transforms')
    for action in (b'All', b'Include', b'Exclude'):
        fields = b'' if action == b'All' else b' /Fields [(Amount)]'
        lock = b'<< /Type /SigFieldLock /Action /' + action + fields + b' /P 1 >>'
        expected = action.decode() + ('' if action == b'All' else ':Amount') + ':1'
        yield single('lock-' + action.decode().lower(), field_extra=b'/Lock 7 0 R', other={7: lock}, lock=expected, req='signature-transforms')
    yield single('lock-no-permission', field_extra=b'/Lock 7 0 R', other={7: b'<< /Action /All >>'}, lock='All:_', req='signature-transforms')
    yield single('docmdp-bad-permission', extra=b'/Reference [<< /TransformMethod /DocMDP /TransformParams << /P 4 >> >>]', error='Syntax', req='signature-transforms')
    yield single('lock-bad-action', field_extra=b'/Lock 7 0 R', other={7: b'<< /Action /Unknown >>'}, error='Syntax', req='signature-transforms')
    yield single('fieldmdp-missing-data', extra=b'/Reference [<< /TransformMethod /FieldMDP /TransformParams << /Action /All >> >>]', error='Syntax', req='signature-transforms')
    pdf = Pdf()
    pdf.append(structure(field_value=b''))
    yield vector('unsigned-field', pdf, ['Approval|unsigned'], 'signature-fields')
    # Two signatures applied in distinct incremental updates; all positions are
    # recorded before patching each fixed-width ByteRange slot.
    for stream in (False, True):
        first, first_span = signature()
        pdf = Pdf()
        positions = pdf.append(structure() | {5: first}, stream=stream)
        ranges1 = patch(pdf, positions[5], first, first_span)
        second, second_span = signature(b'ETSI.CAdES.detached')
        positions = pdf.append({4: b'<< /Fields [6 0 R 10 0 R] /SigFlags 3 >>', 9: second,
                                10: b'<< /FT /Sig /T (Second) /V 9 0 R >>'}, stream=stream)
        ranges2 = patch(pdf, positions[9], second, second_span, revision=1)
        yield vector('multiple-revisions' + ('-stream' if stream else ''), pdf,
                     [row(revision='1', later=1, ranges=ranges1), row(name='Second', ranges=ranges2)], 'signature-revision-coverage')
    first, span = signature()
    pdf = Pdf()
    positions = pdf.append(structure() | {5: first})
    ranges = patch(pdf, positions[5], first, span)
    pdf.append({8: b'<< /Producer (Later update) >>'})
    yield vector('update-after-signing', pdf, [row(revision='1', later=1, ranges=ranges)], 'signature-revision-coverage')
    # Fake boundary text in a later object cannot substitute for the parsed
    # terminator of the original cross-reference section.
    body, span = signature()
    pdf = Pdf()
    positions = pdf.append(structure() | {5: body})
    original_end = pdf.revisions[0]
    fake = b'(startxref\n%d\n%%%%EOF\n)' % pdf.previous
    positions2 = pdf.append({8: fake})
    fake_end = positions2[8] + len(fake) - 1
    ranges = patch(pdf, positions[5], body, span, ranges=lambda a, b, n: (0, a, b, fake_end - b))
    assert fake_end > original_end
    yield vector('fake-revision-marker', pdf, [row(revision='NotRevisionBoundary', ranges=ranges)], 'signature-revision-coverage')
    body, span = signature(extra=b'/Reference [<< /TransformMethod /UR /DigestMethod /SHA256 >>]')
    pdf = Pdf()
    objects = structure(field_value=b'', catalog_extra=b'/Perms << /UR3 5 0 R >>') | {5: body}
    positions = pdf.append(objects)
    ranges = patch(pdf, positions[5], body, span)
    yield vector('catalog-ur3', pdf, ['Approval|unsigned', row(name='@UR3', ranges=ranges, refs='UR')], 'signature-fields')
    # Duplicate names are repaired by the document parser, but must never bind
    # signature Contents to whichever spelling happens to win that repair.
    yield single('duplicate-contents', extra=b'/Contents <abcd>', expected='AmbiguousSource', revision='RepairedDocument')

    for case, array, expected in [
        ('range-count', b'[0 1]', 'ExpectedTwoRanges'),
        ('missing-range', b'null', 'ExpectedTwoRanges'),
        ('range-real', b'[0 1.5 2 3]', 'InvalidInteger'),
        ('huge-tail', b'[0 1 2 9223372036854775807]', 'OutsideFile'),
    ]:
        body, _ = signature()
        body = body.replace(b'[' + SLOT + b']', array)
        pdf = Pdf()
        pdf.append(structure() | {5: body})
        yield vector(case, pdf, [row(integrity=expected, revision='NotCheckedYet')])
    body, span = signature()
    pdf = Pdf()
    objects = structure() | {5: body,
        4: b'<< /Fields [8 0 R] >>',
        6: b'<< /T (Approval) /Parent 8 0 R >>',
        8: b'<< /FT /Sig /T (Parent) /V 5 0 R /Kids [6 0 R] >>'}
    positions = pdf.append(objects)
    ranges = patch(pdf, positions[5], body, span)
    yield vector('inherited-signature-value', pdf, [row(name='Parent.Approval', ranges=ranges)], 'signature-fields')

    body, span = signature()
    pdf = Pdf()
    objects = structure(field_extra=b'/Kids [7 0 R 8 0 R]') | {5: body,
        7: b'<< /Type /Annot /Subtype /Widget /Rect [0 0 1 1] /Parent 6 0 R >>',
        8: b'<< /Type /Annot /Subtype /Widget /Rect [1 1 2 2] /Parent 6 0 R >>'}
    positions = pdf.append(objects)
    patch(pdf, positions[5], body, span)
    yield vector('multiple-widgets', pdf, [], 'signature-fields', error='Syntax')
    for case, remove, algorithm in [
        ('default-type', b' /Type /Sig', 'NotCheckedYet'),
        ('unspecified-subfilter', b' /SubFilter /adbe.pkcs7.detached', 'UnsupportedEncoding'),
    ]:
        body, span = signature()
        body = body.replace(remove, b'')
        span = (span[0] - len(remove), span[1] - len(remove))
        pdf = Pdf()
        positions = pdf.append(structure() | {5: body})
        ranges = patch(pdf, positions[5], body, span)
        yield vector(case, pdf, [row(ranges=ranges, algorithms=algorithm)], 'signature-fields')

    body, span = signature(extra=b'/Reference [<< /TransformMethod /UR /DigestMethod /SHA256 >>]')
    pdf = Pdf()
    objects = structure(field_value=b'', catalog_extra=b'/Perms << /UR3 ' + body + b' >>')
    positions = pdf.append(objects)
    at = positions[1] + objects[1].index(body)
    ranges = patch(pdf, at, body, span)
    yield vector('catalog-direct-ur3', pdf, ['Approval|unsigned', row(name='@UR3', ranges=ranges, refs='UR')], 'signature-fields')
    body, span = signature()
    pdf = Pdf()
    positions = pdf.append(structure() | {5: body})
    # The newest startxref and Prev remain correct; an earlier startxref is
    # not inspected when opening the cross-reference chain. Coverage must
    # expose the missing confirmation instead of asserting a negative match.
    marker = pdf.data.rindex(b'startxref\n') + len(b'startxref\n')
    old = str(pdf.previous).encode()
    replacement = str(pdf.previous + 1).encode()
    assert len(old) == len(replacement)
    pdf.data[marker:marker + len(old)] = replacement
    ranges = patch(pdf, positions[5], body, span)
    pdf.append({8: b'<< /Producer (Later update) >>'})
    yield vector('unconfirmed-revision-terminator', pdf, [row(revision='Indeterminate', ranges=ranges)], 'signature-revision-coverage')
    yield single('indirect-signature-entry', extra=b'/Extension 8 0 R', other={8: b'42'}, expected='IndirectValue')
    body, span = signature()
    pdf = Pdf()
    field = b'<< /FT /Sig /T (Approval) /V ' + body + b' >>'
    objects = structure(field_value=b'') | {4: b'<< /Fields [' + field + b'] >>'}
    del objects[6]
    positions = pdf.append(objects)
    at = positions[4] + objects[4].index(body)
    ranges = patch(pdf, at, body, span)
    yield vector('direct-field-array', pdf, [row(revision='RepairedDocument', ranges=ranges)], 'signature-byte-range')
