"""D02 source-metadata fixtures, ISO 32000-2 14.3 and 7.7.2.

Constructed independently of Pagura. These are lazy-view fixtures: opening the
page remains valid even when a metadata field has the wrong type or cycles.
"""
import zlib

from .model import Case, OPEN
from .pdf import document, stream

AREA = 'metadata'
REQUIREMENT = 'document-metadata-read'
PACKET = b'<?xpacket begin=""?><x:xmpmeta xmlns:x="adobe:ns:meta/"><title>XMP title</title></x:xmpmeta><?xpacket end="w"?>'
OBJECT_PACKET = b'<object-metadata>separate packet</object-metadata>'


def fixture(name, *, info=b'null', title=b'(Title)', trailer=b'/Info 5 0 R',
            catalog=b'', packet=None, packet_dictionary=b'/Type /Metadata /Subtype /XML',
            compressed=False, object_packet=False, last=b'null'):
    if packet is not None:
        catalog += b' /Metadata 6 0 R'
    if compressed:
        assert packet is not None
        packet = zlib.compress(packet)
        packet_dictionary += b' /Filter /FlateDecode'
    objects = [
        b'<< /Type /Catalog /Pages 2 0 R ' + catalog + b' >>',
        b'<< /Type /Pages /Kids [3 0 R] /Count 1 >>',
        b'<< /Type /Page /Parent 2 0 R /MediaBox [0 0 64 64] /Resources << >> /Contents 4 0 R '
        + (b'/Metadata 7 0 R' if object_packet else b'') + b' >>',
        stream(b''), info,
        stream(packet, packet_dictionary) if packet is not None else last,
        stream(OBJECT_PACKET, b'/Type /Metadata /Subtype /XML') if object_packet else b'null',
        title,
    ]
    return Case(name, document(objects, trailer), OPEN, (REQUIREMENT,), name.replace('-', ' '))


def cases():
    info = (b'<< /Title 8 0 R /Author (Euro \xa0) /Subject <EFBBBF' + '測試'.encode().hex().encode()
            + b'> /Keywords (one; two) /Creator (Original app) /Producer (PDF app) '
            + b'/CreationDate (D:20261026030000+01\'00) /ModDate (D:20261026040000Z) '
            + b'/Trapped /True /Custom (preserved) >>')
    title = b'<' + (b'\xfe\xff' + 'Grüße Ω'.encode('utf-16-be')).hex().encode() + b'>'
    for compressed in (False, True):
        yield fixture('populated-flate' if compressed else 'populated', info=info, title=title,
                      catalog=b'/Lang (de-DE)', packet=PACKET, compressed=compressed, object_packet=True)
    yield fixture('absent', trailer=b'')
    yield fixture('null', catalog=b'/Lang null /Metadata null')
    yield fixture('undefined', trailer=b'/Info 99 0 R', catalog=b'/Lang 99 0 R /Metadata 99 0 R')
    yield fixture('empty', info=b'<< >>', packet=b'')
    for trapped in (b'False', b'Unknown'):
        yield fixture('trapped-' + trapped.decode().lower(), info=b'<< /Trapped /' + trapped + b' >>')
    yield fixture('info-string', info=b'<< /Title (abcdefghij) >>')
    yield fixture('info-wrong-type', info=b'42')
    yield fixture('info-field-wrong-type', info=b'<< /Title 8 0 R >>', title=b'42')
    yield fixture('info-trapped-boolean', info=b'<< /Trapped true >>')
    yield fixture('info-cycle', info=b'6 0 R', last=b'5 0 R')
    yield fixture('invalid-text', info=b'<< /Title <FEFFD800> >>')
    yield fixture('language-wrong-type', catalog=b'/Lang 42')
    yield fixture('metadata-direct-dictionary', catalog=b'/Metadata << /Type /Metadata /Subtype /XML >>')
    yield fixture('metadata-nonstream', catalog=b'/Metadata 6 0 R', last=b'<< /Type /Metadata /Subtype /XML >>')
    yield fixture('metadata-cycle', catalog=b'/Metadata 5 0 R', info=b'6 0 R', last=b'5 0 R', trailer=b'')
    for name, dictionary in [('missing-type', b'/Subtype /XML'), ('wrong-subtype', b'/Type /Metadata /Subtype /Text')]:
        yield fixture('metadata-' + name, packet=PACKET, packet_dictionary=dictionary)
    # Byte retrieval does not validate/execute XML or resolve external entities.
    inert = b'<!DOCTYPE x [<!ENTITY e SYSTEM "https://example.invalid/metadata">]><x>&e;</x>'
    yield fixture('metadata-inert-xml', packet=inert)
