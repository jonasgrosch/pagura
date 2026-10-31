"""Complete inline samples with a missing separator, independently authored.

8.9.7 requires whitespace before EI. Repair policy permits its absence only
at the independently framed end of the complete sample/filter payload. No
expected pixel is obtained from a product executable.
"""
from .model import Case, OPEN, RENDER
from .paint import BLUE, GREEN, RED, page, rects, type3, FONT

AREA = "paint_inline_framing"
REPAIR = "inline.ei-separator"
REQUIREMENT = ("paint-inline",)
PREFIX = b"0 0 1 rg 0 0 32 32 re f "
SUFFIX = b" Q 0 1 0 rg 20 20 4 4 re f"
SAMPLES = bytes([0, 255, 69, 73])
RECTANGLES = [((0, 0, 32, 32), BLUE)] + [
    ((4 + 4*x, 4, 4, 8), (v, v, v)) for x, v in enumerate(SAMPLES)
] + [((20, 20, 4, 4), GREEN)]


def cases():
    forms = [
        ("raw", b"/W 4 /H 1 /BPC 8 /CS /G", SAMPLES),
        ("length", b"/W 4 /H 1 /BPC 8 /CS /G /L 4", SAMPLES),
        ("asciihex", b"/W 4 /H 1 /BPC 8 /CS /G /F /AHx", SAMPLES.hex().encode()+b">"),
        ("runlength", b"/W 4 /H 1 /BPC 8 /CS /G /F /RL", bytes([3])+SAMPLES+bytes([128])),
    ]
    for name, dictionary, data in forms:
        for separated in (True, False):
            suffix = "valid" if separated else "missing-separator"
            content = PREFIX+b"q 16 0 0 8 4 4 cm BI "+dictionary+b" ID "+data+(b" " if separated else b"")+b"EI"+SUFFIX
            yield Case(name+"-"+suffix, page(content), RENDER, REQUIREMENT,
                       "Complete four-sample image and downstream green witness; "+suffix,
                       oracle=rects(RECTANGLES), strict_valid=separated,
                       repairs=() if separated else (REPAIR,))
    # Mirrors the source structure of a raw inline stencil in a Type3 glyph,
    # without using any upstream bytes or the renderer to derive its pixels.
    for separated in (True, False):
        procedure = b"400 0 d0 q 400 0 0 400 0 0 cm BI /W 4 /H 1 /IM true /BPC 1 ID \x60"+(b" " if separated else b"")+b"EI Q"
        content = PREFIX+b"1 0 0 rg BT /F 20 Tf 1 0 0 1 4 4 Tm (A) Tj ET 0 1 0 rg 20 20 4 4 re f"
        rectangles = [((0, 0, 32, 32), BLUE), ((4, 4, 2, 8), RED), ((10, 4, 2, 8), RED), ((20, 20, 4, 4), GREEN)]
        yield Case("type3-"+("valid" if separated else "missing-separator"), page(content, type3(procedure), FONT),
                   RENDER, REQUIREMENT, "Raw Type3 stencil and downstream witness", oracle=rects(rectangles),
                   strict_valid=separated, repairs=() if separated else (REPAIR,))
    for name, payload in [
        ("attached-operator", SAMPLES+b"EIQ"),
        ("wrong-token", SAMPLES+b"EX"),
        ("missing-token", SAMPLES),
        ("short-data", SAMPLES[:2]+b"EI"),
        ("false-embedded-token", b"EI\x00\xff"),
        ("comment-before-token", SAMPLES+b"%not a separator\nEI"),
    ]:
        content = PREFIX+b"q 16 0 0 8 4 4 cm BI /W 4 /H 1 /BPC 8 /CS /G ID "+payload+b" Q"
        yield Case(name, page(content), OPEN, REQUIREMENT,
                   "Invalid framing after a visible prefix must refuse the whole page",
                   extra={"render_error": "syntax"})
    for name, dictionary, payload in [
        ("short-length", b"/W 4 /H 1 /BPC 8 /CS /G /L 2", SAMPLES[:2]),
        ("excess-length", b"/W 4 /H 1 /BPC 8 /CS /G /L 5", SAMPLES+b"x"),
        ("short-asciihex", b"/W 4 /H 1 /BPC 8 /CS /G /F /AHx", b"00ff>"),
        ("short-runlength", b"/W 4 /H 1 /BPC 8 /CS /G /F /RL", b"\x01\x00\xff\x80"),
        ("short-packed-rows", b"/W 9 /H 2 /BPC 1 /CS /G /L 3", b"\x80\x00\x00"),
        ("short-row-padding", b"/W 4 /H 2 /BPC 1 /CS /G /L 1", b"\x00"),
        ("short-sixteen-bit", b"/W 2 /H 1 /BPC 16 /CS /G /L 3", b"\x00\x00\xff"),
        ("excess-asciihex", b"/W 4 /H 1 /BPC 8 /CS /G /F /AHx", b"00ff454978>"),
        ("excess-runlength", b"/W 4 /H 1 /BPC 8 /CS /G /F /RL", b"\x04"+SAMPLES+b"x\x80"),
        ("invalid-asciihex", b"/W 4 /H 1 /BPC 8 /CS /G /F /AHx", b"zz>"),
        ("invalid-runlength", b"/W 4 /H 1 /BPC 8 /CS /G /F /RL /L 4", b"\x05\x00\xff\x80"),
    ]:
        content = PREFIX+b"q 16 0 0 8 4 4 cm BI "+dictionary+b" ID "+payload+b"EI Q"
        yield Case(name, page(content), OPEN, REQUIREMENT,
                   "A known encoded boundary must not authorize missing-sample padding",
                   strict_valid=False, repairs=(REPAIR,),
                   extra={"render_error": "syntax"})
    # Existing, explicitly reported padding is a separate retention scope.
    # These separated twins must not become new public-page admissions.
    for name, dictionary, payload, rectangles in [
        ("length", b"/W 4 /H 1 /BPC 8 /CS /G /L 2", SAMPLES[:2],
         [((4+4*x, 4, 4, 8), (v, v, v)) for x, v in enumerate([0, 255, 0, 0])]),
        ("asciihex", b"/W 4 /H 1 /BPC 8 /CS /G /F /AHx", b"00ff>",
         [((4+4*x, 4, 4, 8), (v, v, v)) for x, v in enumerate([0, 255, 0, 0])]),
        ("runlength", b"/W 4 /H 1 /BPC 8 /CS /G /F /RL", b"\x01\x00\xff\x80",
         [((4+4*x, 4, 4, 8), (v, v, v)) for x, v in enumerate([0, 255, 0, 0])]),
        ("packed-rows", b"/W 4 /H 2 /BPC 1 /CS /G /L 1", b"\x00",
         [((4, 4, 16, 8), (0, 0, 0))]),
        ("sixteen-bit", b"/W 2 /H 1 /BPC 16 /CS /G /L 3", b"\x00\x00\xff",
         [((4, 4, 8, 8), (0, 0, 0)), ((12, 4, 8, 8), (254, 254, 254))]),
    ]:
        content = PREFIX+b"q 16 0 0 8 4 4 cm BI "+dictionary+b" ID "+payload+b" EI"+SUFFIX
        expected = [((0, 0, 32, 32), BLUE), *rectangles, ((20, 20, 4, 4), GREEN)]
        yield Case("legacy-separated-short-"+name, page(content), RENDER, REQUIREMENT,
                   "Existing reported padding retained; excluded from complete-output admissions",
                   oracle=rects(expected), strict_valid=False, repairs=("image.data-short",))
    for separated in (True, False):
        suffix = "valid" if separated else "missing-separator"
        content = PREFIX+b"q 16 0 0 8 4 4 cm BI /W 4 /H 1 /BPC 8 /CS /C ID "+SAMPLES+(b" " if separated else b"")+b"EI"+SUFFIX
        yield Case("named-space-"+suffix, page(content, resources=b"<< /ColorSpace << /C /DeviceGray >> >>"),
                   RENDER, REQUIREMENT, "Named colour-space resource is resolved before completeness proof",
                   oracle=rects(RECTANGLES), strict_valid=separated, repairs=() if separated else (REPAIR,))
        payload = b"\xff\x80\x00\x80"
        content = PREFIX+b"q 18 0 0 4 4 4 cm BI /W 9 /H 2 /BPC 1 /CS /G ID "+payload+(b" " if separated else b"")+b"EI"+SUFFIX
        cells = [((4+2*x, 4+2*(1-y), 2, 2), ((255 if y==0 or x==8 else 0),)*3) for y in range(2) for x in range(9)]
        expected = [((0, 0, 32, 32), BLUE), *cells, ((20, 20, 4, 4), GREEN)]
        yield Case("packed-rows-"+suffix, page(content), RENDER, REQUIREMENT,
                   "Nine bits in each of two rows require four bytes, including row padding",
                   oracle=rects(expected), strict_valid=separated, repairs=() if separated else (REPAIR,))
    for suffix, terminator in [("valid", b" EI"), ("missing-separator", b"EI"), ("invalid-token", b"EIQ")]:
        procedure = (b"400 0 0 0 400 400 d1 q 100 0 0 100 200 200 cm "
                     b"BI /W 1 /H 1 /BPC 8 /CS /RGB ID \x00\xff\x00"+terminator+b" Q 0 0 100 100 re f")
        content = PREFIX+b"1 0 0 rg BT /F 20 Tf 1 0 0 1 4 4 Tm (A) Tj ET 0 1 0 rg 20 20 4 4 re f"
        pdf = page(content, type3(procedure), FONT)
        if suffix == "invalid-token":
            yield Case("ignored-type3-"+suffix, pdf, OPEN, REQUIREMENT,
                       "Ignored coloured image still requires complete token framing", extra={"render_error": "syntax"})
        else:
            expected = [((0, 0, 32, 32), BLUE), ((4, 4, 2, 2), RED), ((20, 20, 4, 4), GREEN)]
            yield Case("ignored-type3-"+suffix, pdf, RENDER, REQUIREMENT,
                       "d1 ignores the disjoint coloured image but retains a later stencil mark",
                       oracle=rects(expected), strict_valid=suffix=="valid", repairs=() if suffix=="valid" else (REPAIR,))
