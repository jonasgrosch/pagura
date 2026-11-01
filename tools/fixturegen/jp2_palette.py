"""Reported JP2 palette padding, T.800 I.5.3.4 and PDF 7.4.9/Table 87.

Inputs use the independent Python JPEG 2000 encoder. Sample codes and RGB
answers below are literal known answers, not obtained from a PDF engine.
"""
import struct
from . import jpx
from .model import Case, OPEN, RENDER, UNSUPPORTED
from .pdf import document, opaque_rectangles, stream

AREA = "jp2_palette"
REQUIREMENT = "recovery-jpx-palette-padding"
REPAIR = "jpx.palette-padding"
BLACK = (0, 0, 0)
WHITE = (255, 255, 255)

# (name, wire depth bytes, literal stored rows, unsigned-offset code planes,
#  literal byte RGB). Signed columns retain the declared sign-bit position.
MODELS = [
    ("gray-signed1", (128,), [(1,), (0,), (0,), (1,)],
     [[0, 1, 1, 0]], [BLACK, WHITE, WHITE, BLACK]),
    ("gray-unsigned4", (3,), [(0,), (5,), (10,), (15,)],
     [[0, 5, 10, 15]], [BLACK, (85, 85, 85), (170, 170, 170), WHITE]),
    ("rgb-mixed-small", (0, 131, 8), [(0, 8, 0), (1, 15, 170), (1, 0, 341), (0, 7, 511)],
     [[0, 1, 1, 0], [0, 7, 8, 15], [0, 170, 341, 511]],
     [BLACK, (255, 119, 85), (255, 136, 170), (0, 255, 255)]),
    ("rgb-mixed-wide", (3, 136, 142), [(0, 256, 16384), (5, 511, 32767), (10, 0, 0), (15, 255, 16383)],
     [[0, 5, 10, 15], [0, 255, 256, 511], [0, 16383, 16384, 32767]],
     [BLACK, (85, 127, 127), (170, 128, 128), WHITE]),
    ("rgb-full16", (15, 143, 7), [(0, 32768, 0), (21845, 65535, 85), (43690, 0, 170), (65535, 32767, 255)],
     [[0, 21845, 43690, 65535], [0, 32767, 32768, 65535], [0, 85, 170, 255]],
     [BLACK, (85, 127, 85), (170, 128, 170), WHITE]),
    ("rgb-full16-meaningful-high", (15, 143, 7), [(32768, 32768, 0), (54613, 65535, 85), (10922, 0, 170), (32767, 32767, 255)],
     [[32768, 54613, 10922, 32767], [0, 32767, 32768, 65535], [0, 85, 170, 255]],
     [(128, 0, 0), (213, 127, 85), (42, 128, 170), (127, 255, 255)]),
]
LOOKUP = [(255, 0, 0), (0, 255, 0), (0, 0, 255), (255, 0, 255)]
PDF_INDEXED = b"/ColorSpace [/Indexed /DeviceRGB 3 <FF000000FF000000FFFF00FF>] /BitsPerComponent 16"


def palette(depths, rows, padded):
    body = struct.pack(">HB", len(rows), len(depths)) + bytes(depths)
    for row in rows:
        for value, depth in zip(row, depths):
            bits = (depth & 127) + 1
            size = (bits + 7) // 8
            # Set one otherwise unused high bit, never the declared sign bit.
            if padded and bits % 8:
                value |= 1 << (8 * size - 1)
            body += value.to_bytes(size, "big")
    return jpx.box(b"pclr", body)


def encoded(depths, rows, padded, *, fault="", alpha=0):
    if alpha:
        depths = (*depths, 7)
        rows = [(*row, opacity) for row, opacity in zip(rows, (0, 85, 170, 255))]
    table = palette(depths, rows, padded)
    if fault == "short-palette":
        # Keep the box complete while omitting one required table octet.
        table = jpx.box(b"pclr", table[8:-1])
    component = 2 if fault == "bad-component" else 0
    mapping = jpx.box(b"cmap", b"".join(struct.pack(">HBB", component, 1, i) for i in range(len(depths))))
    extra = table + (b"" if fault == "missing-mapping" else mapping)
    if alpha:
        channels = [(0, 0, 1), (1, 0, 2), (2, 0, 3), (3, alpha, 0)]
        extra += jpx.box(b"cdef", struct.pack(">H", len(channels))
                         + b"".join(struct.pack(">3H", *entry) for entry in channels))
    raw = jpx.codestream([[0, 1, 2, 3]], 4, 1, bits=2)
    if fault == "missing-eoc":
        assert raw[-2:] == b"\xff\xd9"
        raw = raw[:-2]
    return container(raw, 1, extra, len(depths) > 1)


def container(raw, components, extra, rgb):
    header = jpx.box(b"ihdr", struct.pack(">IIH4B", 1, 4, components, 1, 7, 0, 0))
    header += jpx.box(b"colr", b"\1\0\0" + struct.pack(">I", 16 if rgb else 17)) + extra
    return (jpx.box(b"jP  ", b"\r\n\x87\n") + jpx.box(b"ftyp", b"jp2 \0\0\0\0jp2 ")
            + jpx.box(b"jp2h", header) + jpx.box(b"jp2c", raw))


def page(payload, params=b"", mask=False):
    image = stream(payload, b"/Type /XObject /Subtype /Image /Width 4 /Height 1 /Filter /JPXDecode " + params)
    objects = [
        b"<< /Type /Catalog /Pages 2 0 R >>",
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 32 32] /Resources << /XObject << /I 5 0 R >> >> /Contents 4 0 R >>",
        stream(b"q 16 0 0 4 8 8 cm /I Do Q 0 1 0 rg 24 24 4 4 re f"), image,
    ]
    if mask:
        objects.append(stream(bytes([0, 85, 170, 255]), b"/Subtype /Image /Width 4 /Height 1 /BitsPerComponent 8 /ColorSpace /DeviceGray"))
    return document(objects)


def oracle(colors):
    return opaque_rectangles(32, 32, [((8 + i * 4, 8, 4, 4), color) for i, color in enumerate(colors)]
                             + [((24, 24, 4, 4), (0, 255, 0))])


def success(name, data, colors, padded, params=b"", *, depths=(), codes=(), mask=False):
    return Case(name, page(data, params, mask), RENDER, (REQUIREMENT,),
                "Declared palette codes and signed offset are preserved; only unused high padding is repairable",
                oracle=oracle(colors), strict_valid=not padded, repairs=(REPAIR,) if padded else (),
                extra={"palette_depths": list(depths), "codec_expected_codes": list(codes)})


def cases():
    for name, depths, rows, codes, colors in MODELS:
        for padded in ([False, True] if any((d & 127) + 1 < 8 * (((d & 127) + 8) // 8) for d in depths) else [False]):
            params = b"/ColorSpace /DeviceRGB" if len(depths) == 3 else b""
            yield success(name + ("-padding" if padded else "-conforming"), encoded(depths, rows, padded), colors,
                          padded, params, depths=depths, codes=codes)
    _, depths, rows, _, _ = MODELS[2]
    for padded in (False, True):
        yield success("indexed-override-" + ("padding" if padded else "conforming"),
                      encoded(depths, rows, padded), LOOKUP, padded, PDF_INDEXED)
    # The fractional-alpha answers are literal rational composites over white.
    yield success("mixed-padding-external-alpha", encoded(depths, rows, True),
                  [WHITE, (255, 210, 198), (255, 176, 198), (0, 255, 255)], True,
                  b"/ColorSpace /DeviceRGB /SMask 6 0 R", mask=True)
    # Existing raw index/opacity semantics remain available without a container palette.
    raw = jpx.codestream([[0, 1, 2, 3], [0, 1, 2, 3]], 4, 1, bits=2)
    channels = jpx.box(b"cdef", struct.pack(">H6H", 2, 0, 0, 1, 1, 1, 0))
    direct = container(raw, 2, channels, False)
    yield success("raw-index-opacity-control", direct,
                  [WHITE, (170, 255, 170), (85, 85, 255), (255, 0, 255)], False,
                  PDF_INDEXED + b" /SMaskInData 1")
    for fault in ("short-palette", "missing-mapping", "missing-eoc", "bad-component", "opacity-1", "opacity-2"):
        for padded in (False, True):
            alpha = int(fault[-1]) if fault.startswith("opacity-") else 0
            data = encoded(depths, rows, padded, fault=fault, alpha=alpha)
            params = PDF_INDEXED + b" /SMaskInData %d" % alpha if alpha else b"/ColorSpace /DeviceRGB"
            yield Case(fault + ("-padding" if padded else "-conforming"), page(data, params),
                       UNSUPPORTED if alpha else OPEN, (REQUIREMENT,),
                       "Palette padding does not repair incomplete data, missing mappings, invalid components or palette-derived opacity",
                       feature="image.JPX-sample-layout" if alpha else None,
                       strict_valid=not padded,
                       extra=({"strict_repair": REPAIR} if padded else {}) | ({} if alpha else {"render_error": "syntax"}))

    # A complete premultiplied palette has black colour at fractional alpha;
    # the final fully opaque cyan retains its literal source codes.
    premultiplied_rows = [rows[0], rows[0], rows[0], rows[3]]
    for padded in (False, True):
        yield Case("opacity-2-premultiplied-" + ("padding" if padded else "conforming"),
                   page(encoded(depths, premultiplied_rows, padded, alpha=2), PDF_INDEXED + b" /SMaskInData 2"),
                   UNSUPPORTED, (REQUIREMENT,),
                   "A complete valid premultiplied container palette still requires the explicit Indexed opacity mapping feature",
                   feature="image.JPX-sample-layout", strict_valid=not padded,
                   extra={"strict_repair": REPAIR} if padded else {})
