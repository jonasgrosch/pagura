"""D02 inert file specifications and attachments, authored without a PDF engine.

ISO 32000-2 7.11, Tables 43-45, and 14.13. Payloads, names and declared
parameters are independent bytes; zlib and hashlib supply filter/checksum data.
Malformed views remain lazy and do not make the blank page invalid at opening.
"""
import hashlib
import zlib

from .document import encrypted
from .model import Case, OPEN
from .pdf import document, stream

AREA = "files"
REQUIREMENT = "document-file-views"
PAYLOAD = b'attachment\x00bytes\xff\n'
UNICODE = 'Grüße Ω.txt'


def text(value):
    return b'<' + (b'\xfe\xff' + value.encode('utf-16-be')).hex().encode() + b'>'


def source(*, spec=None, payload=PAYLOAD, catalog=b'', page=b'', names=None, file_stream=None,
           stream_extra=b'', related=None, extra=()):
    if spec is None:
        spec = (b'<< /Type /Filespec /F (legacy.txt) /UF ' + text(UNICODE)
                + b' /Desc ' + text('Attachment €')
                + b' /EF << /F 6 0 R /UF 6 0 R >> /AFRelationship /Data /V true'
                + b' /ID [(original)(current)] /Custom (preserved) >>')
    if names is None:
        names = b'/Names << /EmbeddedFiles << /Names [(lookup-key) 5 0 R] >> >> '
    if file_stream is None:
        params = b'/Size %d /CreationDate (D:20261026000000Z) /ModDate (D:20261026010000Z) /CheckSum <%s>' % (len(payload), hashlib.md5(payload).hexdigest().encode())
        file_stream = stream(payload, b'/Type /EmbeddedFile /Subtype /application#2Foctet-stream /Params << ' + params + b' >> ' + stream_extra)
    return document([
        b'<< /Type /Catalog /Pages 2 0 R ' + names + catalog + b' >>',
        b'<< /Type /Pages /Kids [3 0 R] /Count 1 >>',
        b'<< /Type /Page /Parent 2 0 R /MediaBox [0 0 32 32] /Contents 4 0 R ' + page + b' >>',
        stream(b''), spec, file_stream, related if related is not None else stream(b'related bytes', b'/Type /EmbeddedFile '),
        *extra,
    ]).replace(b'%PDF-1.7', b'%PDF-2.0', 1)


def attachment_encryption(*, omit_type=False, explicit=False, identity=False):
    # Add a name tree and Filespec in a classic incremental revision of the
    # existing independent Standard-handler fixture. StrF is Identity, so
    # these literal filenames remain plaintext. Object 6 keeps its cipher bytes.
    base = encrypted(4, attachment_only=True, auth_event=b'EFOpen', user_password=b'attachment')
    body = base.split(b'\n6 0 obj\n', 1)[1].split(b'\nendobj\n', 1)[0]
    if omit_type:
        body = body.replace(b'/Type /EmbeddedFile ', b'', 1)
    if explicit:
        body = body.replace(b'>>\nstream', b'/Filter /Crypt /DecodeParms << /Name /StdCF >> >>\nstream', 1)
    if identity:
        body = stream(b'public attachment bytes', b'/Filter /Crypt /DecodeParms << /Name /Identity >>')
    objects = {
        1: b'<< /Type /Catalog /Pages 2 0 R /Names << /EmbeddedFiles << /Names [(protected) 7 0 R] >> >> >>',
        6: body,
        7: b'<< /Type /Filespec /F (protected.bin) /EF << /F 6 0 R >> >>',
    }
    data = bytearray(base)
    offsets = {}
    for number, value in objects.items():
        offsets[number] = len(data)
        data += b'%d 0 obj\n' % number + value + b'\nendobj\n'
    xref = len(data)
    data += b'xref\n'
    for number, offset in offsets.items():
        data += b'%d 1\n%010d 00000 n \n' % (number, offset)
    previous = int(base.rsplit(b'startxref\n', 1)[1].splitlines()[0])
    identifier = b'00112233445566778899aabbccddeeff'
    data += b'trailer\n<< /Size 8 /Root 1 0 R /Encrypt 5 0 R /ID [<' + identifier + b'><' + identifier + b'>] /Prev %d >>\n' % previous
    data += b'startxref\n%d\n%%%%EOF\n' % xref
    return bytes(data)


def cases():
    def case(name, **options):
        return Case(name, source(**options), OPEN, (REQUIREMENT,), name.replace('-', ' '))

    yield case('populated', catalog=b'/AF [5 0 R]', page=b'/AF [5 0 R 5 0 R]')
    yield case('flate', file_stream=stream(zlib.compress(PAYLOAD), b'/Type /EmbeddedFile /Filter /FlateDecode'))
    yield case('empty-payload', payload=b'')
    yield case('stream-type-absent', file_stream=stream(PAYLOAD))
    yield case('external-url', spec=b'<< /F (https://example.invalid/never-fetch) /FS /URL /AFRelationship /Vendor_Relation >>', catalog=b'/AF [5 0 R]')
    yield case('simple-string', spec=b'<2f726177ff2f66696c65>')
    yield case('legacy-paths', spec=b'<< /DOS (C:\\134legacy.dat) /Mac (disk:legacy.dat) /Unix (/tmp/legacy.dat) >>')
    yield case('null-entries', spec=b'<< /F (null.bin) /EF null /UF null /Desc null /V null /ID null /AFRelationship null >>', catalog=b'/AF null')
    yield case('empty-unicode-name', spec=b'<< /F (fallback) /UF () >>')
    yield case('absent', names=b'')
    yield case('null-tree', names=b'/Names << /EmbeddedFiles null >>')
    yield case('empty-tree', names=b'/Names << /EmbeddedFiles << /Names [] >> >>')
    yield case('related', spec=b'<< /Type /Filespec /F (parent) /EF << /F 6 0 R >> /RF << /F [(sidecar) 7 0 R] >> >>')
    yield case('duplicate-payload', names=b'/Names << /EmbeddedFiles << /Names [(a) 5 0 R (b) 5 0 R] >> >>')
    yield case('tree-order', names=b'/Names << /EmbeddedFiles << /Names [(z) 5 0 R (a) 5 0 R] >> >>')
    yield case('spec-stream', spec=stream(b'not a specification', b'/Type /Filespec /F (bad)'))
    for name, spec in [
        ('wrong-spec-type', b'42'),
        ('missing-file-name', b'<< /Type /Filespec >>'),
        ('wrong-file-name', b'<< /F 42 >>'),
        ('wrong-unicode-name', b'<< /F (x) /UF /name >>'),
        ('invalid-unicode', b'<< /F (x) /UF <FEFFD800> >>'),
        ('wrong-file-system', b'<< /F (x) /FS (URL) >>'),
        ('wrong-volatile', b'<< /F (x) /V /true >>'),
        ('wrong-relationship', b'<< /F (x) /AFRelationship (Data) >>'),
        ('wrong-identifier', b'<< /F (x) /ID [(one)] >>'),
        ('wrong-spec-type-name', b'<< /Type /Other /F (x) >>'),
        ('embedded-missing-type', b'<< /F (x) /EF << /F 6 0 R >> >>'),
        ('embedded-wrong-type', b'<< /Type /Filespec /F (x) /EF [] >>'),
        ('embedded-no-matching-name', b'<< /Type /Filespec /F (x) /EF << /UF 6 0 R >> >>'),
        ('embedded-invalid-key', b'<< /Type /Filespec /F (x) /Desc (x) /EF << /Desc 6 0 R >> >>'),
        ('embedded-direct-stream', b'<< /Type /Filespec /F (x) /EF << /F << /Length 0 >> >> >>'),
        ('embedded-nonstream', b'<< /Type /Filespec /F (x) /EF << /F 8 0 R >> >>'),
        ('spec-cycle', b'8 0 R'),
        ('related-without-embedded', b'<< /Type /Filespec /F (x) /RF << /F [] >> >>'),
        ('related-unmatched-key', b'<< /Type /Filespec /F (x) /UF (x) /EF << /F 6 0 R >> /RF << /UF [] >> >>'),
        ('related-odd-array', b'<< /Type /Filespec /F (x) /EF << /F 6 0 R >> /RF << /F [(name)] >> >>'),
    ]:
        yield case(name, spec=spec, extra=(b'5 0 R' if name == 'spec-cycle' else b'<< >>',))
    for name, body in [
        ('stream-wrong-type', b'/Type /Other'),
        ('stream-wrong-subtype', b'/Subtype (text/plain)'),
        ('stream-wrong-params', b'/Params 42'),
        ('negative-size', b'/Params << /Size -1 >>'),
        ('checksum-length', b'/Params << /CheckSum (short) >>'),
        ('wrong-date', b'/Params << /ModDate 42 >>'),
        ('external-stream', b'/F (never-open.bin)'),
    ]:
        yield case(name, file_stream=stream(PAYLOAD, body))
    yield case('associated-wrong-type', catalog=b'/AF 42')
    yield case('associated-string', catalog=b'/AF [(not-a-dictionary)]')
    yield case('associated-direct-external', catalog=b'/AF [<< /F (external.bin) >>]')
    yield case('associated-direct-embedded', catalog=b'/AF [<< /Type /Filespec /F (x) /EF << /F 6 0 R >> >>]')
    for name, options in [('encrypted', {}), ('encrypted-no-type', {'omit_type': True}),
                          ('encrypted-explicit-no-type', {'omit_type': True, 'explicit': True}),
                          ('encrypted-identity-no-type', {'identity': True})]:
        yield Case(name, attachment_encryption(**options), OPEN, (REQUIREMENT, 'security-authorization-events'),
                   'Synthetic attachment password is required only when payload bytes are requested',
                   strict_valid=False, repairs=('security.crypt-filter-event',))
