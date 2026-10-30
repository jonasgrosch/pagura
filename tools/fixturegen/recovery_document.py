"""Explicit structure/rectangle oracles for bounded file framing repairs."""
from .document import Builder, CONTENT, OBJECTS, RED
from .model import Case, RENDER

AREA = 'recovery_document'


def cases():
    for name, prefix in [('header-absent', b''), ('header-absent-binary-comment', b'%\xe2\xe3\xcf\xd3\n')]:
        builder = Builder()
        builder.data = bytearray(prefix)
        for number, body in enumerate(OBJECTS, 1):
            builder.obj(number, body)
        data, _ = builder.classic()
        yield Case(name, data, RENDER, ('document-repairs',),
                   'A complete leading Catalog identifies a headerless PDF body without inventing a version',
                   oracle=RED, strict_valid=False, repairs=('file.header-absent',))
    for name, length in [('stream-length-crosses-next-object', 60),
                         ('stream-length-crosses-xref', 200),
                         ('stream-length-crosses-EOF', 99999)]:
        builder = Builder()
        for number, body in enumerate(OBJECTS[:3], 1):
            builder.obj(number, body)
        builder.obj(4, b'<< /Length %d >>\nstream\n' % length + CONTENT + b'\nendstream')
        builder.obj(5, b'null')
        data, _ = builder.classic()
        yield Case(name, data, RENDER, ('stream-boundary-recovery',),
                   'Overlong Length is corrected only to one complete terminator before the next physical object',
                   oracle=RED, strict_valid=False, repairs=('stream.length-corrected',))
