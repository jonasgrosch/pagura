"""Inert FileAttachment annotation views, ISO 32000-2 12.5.6.15/Table 187.

Catalog names and page associations are independent discovery routes. Malformed
views stay lazy: these PDFs still open as blank pages before requesting them.
"""
import zlib

from .files import PAYLOAD, attachment_encryption, source, text
from .model import Case, OPEN
from .pdf import stream

AREA = 'file_annotations'
REQUIREMENT = 'document-file-annotations'
BASE = b' /Type /Annot /Subtype /FileAttachment /Rect [9 12 1 2] /FS 5 0 R '


def cases():
    def case(name, annotation=b'<<' + BASE + b'>>', *, annots=b'[8 0 R]', extra=(), **options):
        return Case(name, source(page=b'/Annots ' + annots, extra=(annotation, *extra), **options), OPEN,
                    (REQUIREMENT,), name.replace('-', ' '))

    yield case('populated', b'<<' + BASE + b'/Contents ' + text('Annotation Ω') + b'/Name /Paperclip /Custom (kept) >>')
    yield case('page-only', names=b'')
    yield case('many', annots=b'[' + b'8 0 R ' * 200 + b']')
    yield case('absent', annots=b'null')
    yield case('empty', annots=b'[]')
    yield Case('missing', source(), OPEN, (REQUIREMENT,), 'Missing Annots is an empty view')
    yield case('mixed', b'<< /Subtype /Link /Rect (not read by this view) >>', annots=b'[8 0 R 9 0 R 9 0 R]',
               extra=(b'<<' + BASE + b'/Contents () /Name /CustomIcon >>',))
    yield case('indirect-array-and-geometry', b'<< /Subtype /FileAttachment /Rect 10 0 R /FS 5 0 R /Contents 11 0 R /Name 12 0 R >>',
               annots=b'9 0 R', extra=(b'[8 0 R]', b'[13 0 R 2 9 12]', text('Referenced'), b'/Graph', b'1'))
    yield case('external', b'<<' + BASE.replace(b'/FS 5 0 R', b'/FS (../outside.bin)') + b'>>')
    yield case('direct-external-spec', b'<<' + BASE.replace(b'/FS 5 0 R', b'/FS << /F (https://invalid.example/file) /FS /URL >>') + b'>>')
    yield case('null-optional', b'<<' + BASE + b'/Contents null /Name null >>')
    yield case('no-type', b'<<' + BASE.replace(b'/Type /Annot', b'') + b'>>')
    yield case('absent-payload-type', file_stream=stream(b'attachment\0bytes\xff\n'))
    yield case('flate', file_stream=stream(zlib.compress(PAYLOAD), b'/Filter /FlateDecode'))
    yield case('wrong-array', annots=b'42')
    yield case('direct-annotation', annots=b'[<<' + BASE + b'>>]')
    yield case('null-member', annots=b'[null]')
    yield case('missing-reference', annots=b'[99 0 R]')
    yield case('reference-cycle', b'9 0 R', extra=(b'8 0 R',))
    yield case('stream-annotation', stream(b'', b'/Subtype /FileAttachment /Rect [1 2 9 12] /FS 5 0 R'))
    for name, body in [
        ('missing-subtype', BASE.replace(b'/Subtype /FileAttachment', b'')),
        ('wrong-subtype', BASE.replace(b'/Subtype /FileAttachment', b'/Subtype 42')),
        ('wrong-type', BASE.replace(b'/Type /Annot', b'/Type /Other')),
        ('missing-file', BASE.replace(b'/FS 5 0 R', b'')),
        ('null-file', BASE.replace(b'/FS 5 0 R', b'/FS null')),
        ('wrong-file', BASE.replace(b'/FS 5 0 R', b'/FS 42')),
        ('missing-rectangle', BASE.replace(b'/Rect [9 12 1 2]', b'')),
        ('wrong-rectangle', BASE.replace(b'/Rect [9 12 1 2]', b'/Rect (wrong)')),
        ('short-rectangle', BASE.replace(b'/Rect [9 12 1 2]', b'/Rect [1 2 3]')),
        ('wrong-coordinate', BASE.replace(b'/Rect [9 12 1 2]', b'/Rect [1 2 null 4]')),
        ('wrong-contents', BASE + b'/Contents 42'),
        ('wrong-icon', BASE + b'/Name (Paperclip)'),
        ('invalid-unicode', BASE + b'/Contents <feff004100>'),
    ]:
        yield case(name, b'<<' + body + b'>>')
    yield case('late-failure', annots=b'[8 0 R 9 0 R]', extra=(b'<< /Subtype /FileAttachment /Rect [1 2 9 12] >>',))
    for name, options in [('encrypted-no-type', {'omit_type': True}), ('encrypted-identity', {'identity': True})]:
        yield Case(name, attachment_encryption(annotation=True, **options), OPEN,
                   (REQUIREMENT, 'security-authorization-events'), 'Annotation payload authentication remains lazy',
                   strict_valid=False, repairs=('security.crypt-filter-event',))
