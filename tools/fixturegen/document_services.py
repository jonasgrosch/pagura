"""Independent D02 source-service data, ISO 32000-2 Tables 28, 46-47, 153-156.

Only Python's zlib/hashlib implement expected compression/checksum data; no PDF
engine or Pagura writer is imported. Asset transfer is distinct from rendering.
"""
import hashlib
import zlib
from .model import Case, OPEN
from .pdf import document, stream

AREA = 'document_services'
REQUIREMENT = 'document-services-scoped-extraction'
PAYLOAD = b'primary\x00payload\xff\n'
RELATED = [b'first sidecar', b'second sidecar\x00']
PIXELS = bytes([255, 0, 0, 0, 255, 0])
PACKET = b'<x:xmpmeta xmlns:x="adobe:ns:meta/">object packet</x:xmpmeta>'


def source(*, collection=None, spec=None, image=None, thumb=None, page_extra=b'', owner=None):
    if collection is None:
        collection = (b'<< /Type /Collection /View /H /D (primary) /Schema << /Type /CollectionSchema '
                      b'/label << /Subtype /S /N (Label) /O 1 /E true >> '
                      b'/amount << /Subtype /N /N (Amount) /O 2 /V false >> '
                      b'/date << /Subtype /D /N (Date) >> '
                      b'/size << /Subtype /Size /N (Size) >> >> '
                      b'/Sort << /S [/amount /label /date] /A [false] >> >>')
    if spec is None:
        spec = (b'<< /Type /Filespec /F (primary.bin) /UF (Primary) /EF << /F 6 0 R >> '
                b'/RF << /F [(same-name) 7 0 R (same-name) 13 0 R] >> /AFRelationship /Data '
                b'/CI << /Type /CollectionItem /label << /Type /CollectionSubitem /D (Value) /P (Prefix: ) >> '
                b'/amount 9007199254740993 /date (D:20261026150000Z) /empty << /P (No value) >> >> '
                b'/EP << /Type /EncryptedPayload /Subtype /ExampleCrypt /Version /1.2 >> >>')
    if image is None:
        image = stream(zlib.compress(PIXELS), b'/Type /XObject /Subtype /Image /Width 2 /Height 1 /ColorSpace /DeviceRGB /BitsPerComponent 8 /Filter /FlateDecode')
    if thumb is None:
        thumb = stream(b'\x7f', b'/Width 1 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8 /SMask /IgnoredThumbnailEntry')
    params = b'/Params << /Size %d /CheckSum <%s> >>' % (len(PAYLOAD), hashlib.md5(PAYLOAD).hexdigest().encode())
    return document([
        b'<< /Type /Catalog /Pages 2 0 R /AF [5 0 R] /Names << /EmbeddedFiles << /Names [(primary) 5 0 R] >> >> /Collection 14 0 R /Metadata 15 0 R >>',
        b'<< /Type /Pages /Kids [3 0 R] /Count 1 >>',
        b'<< /Type /Page /Parent 2 0 R /MediaBox [0 0 10 10] /Contents 4 0 R /AF [5 0 R 5 0 R] /Metadata 8 0 R /Thumb 12 0 R /Resources << /XObject << /Image 9 0 R /Alias 9 0 R /UnusedForm 11 0 R >> >> ' + page_extra + b' >>',
        stream(b''), spec, stream(zlib.compress(PAYLOAD), b'/Filter /FlateDecode ' + params),
        stream(RELATED[0]), stream(PACKET, b'/Type /Metadata /Subtype /XML'), image,
        owner if owner is not None else b'<< /AF [5 0 R] /Metadata 8 0 R /Thumb 12 0 R >>',
        stream(b'', b'/Type /XObject /Subtype /Form /BBox [0 0 1 1]'), thumb,
        stream(zlib.compress(RELATED[1]), b'/Filter /FlateDecode'), collection,
        stream(b'<catalog/>', b'/Type /Metadata /Subtype /XML'),
    ])


def cases():
    def case(name, **kwargs):
        return Case(name, source(**kwargs), OPEN, (REQUIREMENT,), name.replace('-', ' '))
    yield case('complete')
    yield case('empty-collection', collection=b'<< >>')
    yield case('null-collection', collection=b'null')
    yield case('nonstring-initial', collection=b'<< /D 12 >>')
    yield case('invalid-view', collection=b'<< /View /Invalid >>')
    yield case('navigator-missing', collection=b'<< /View /C >>')
    yield case('navigator-inert', collection=b'<< /View /C /Navigator << /Type /Navigator /Name (Never run) >> >>')
    yield case('invalid-schema', collection=b'<< /Schema << /Bad << /Subtype /S /N 10 >> >> >>')
    yield case('invalid-sort', collection=b'<< /Sort << /S [/A] /A [17] >> >>')
    yield case('cyclic-collection', collection=b'14 0 R')
    yield case('invalid-item', spec=b'<< /F (item) /CI << /Bad [1 2] >> >>')
    yield case('invalid-encrypted-payload', spec=b'<< /Type /Filespec /F (item) /EP << /Version /1 >> >>')
    yield case('invalid-image-width', image=stream(PIXELS, b'/Subtype /Image /Width 0 /Height 1'))
    yield case('invalid-image-type', image=b'<< /Subtype /Image /Width 2 /Height 1 >>')
    yield case('invalid-thumbnail', thumb=b'<< /Width 1 /Height 1 >>')
    yield case('invalid-thumbnail-subtype', thumb=stream(b'x', b'/Subtype /Form /Width 1 /Height 1'))
    yield case('empty-owner', owner=b'<< >>')
    yield case('null-owner', owner=b'null')
