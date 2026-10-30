"""True quadratic glyphs retained through nested patterned stencils.

The glyph is a quadratic cap. After the stated affine map its lower and
upper boundaries are degree <=2 polynomials in x. The oracle splits every
pixel at all boundary intersections, then integrates the active polynomial
exactly by its antiderivative. Quadratic roots use Decimal square roots at
60 and 90 digits; both independent evaluations must produce identical bytes.
No path flattening, font parser, renderer or numerical quadrature is used.
"""
from decimal import Decimal as D, localcontext
from fractions import Fraction as F
from itertools import combinations
import struct

from .font import simple, truetype
from .model import Case, RENDER
from .paint_stencils import number, page, polygon, rectangle
from .pdf import document, stream

AREA = "paint_curved_stencils"
COMMON = b"/Type /Pattern /PatternType 1 /PaintType 1 /TilingType 2 /BBox [0 0 16 16] /XStep 16 /YStep 16 "
PAGE = (F(0), F(0), F(16), F(16))
CAP = (F(9), F(0), F(10), F(9, 4), F(11, 4))  # width, shear, height, x, y
SECOND = (F(8), F(3, 2), F(9), F(7, 2), F(2))
THIRD = (F(-9), F(-1), F(11), F(13), F(5, 2))


def decimal(value):
    value = F(value)
    return D(value.numerator) / D(value.denominator)


def evaluate(poly, x):
    a, b, c = poly
    return (a*x+b)*x+c


def roots(poly):
    a, b, c = poly
    if not a:
        return [-c/b] if b else []
    disc = b*b-4*a*c
    if disc < 0:
        return []
    r = disc.sqrt()
    return [(-b-r)/(2*a), (-b+r)/(2*a)]


def integral(poly, lo, hi):
    a, b, c = poly
    return a*(hi**3-lo**3)/3 + b*(hi**2-lo**2)/2 + c*(hi-lo)


def boundaries(cap):
    width, shear, height, x, y = map(decimal, cap)
    if not width:
        assert not shear and not height
        from decimal import ROUND_FLOOR
        x, y = x.to_integral_value(rounding=ROUND_FLOOR), y.to_integral_value(rounding=ROUND_FLOOR)
        return x, x+1, (D(0), D(0), y), (D(0), D(0), y+1)
    a = -2*height/width**2
    b = (shear+2*height)/width-2*a*x
    c = y-(shear+2*height)*x/width+a*x*x
    lower = (D(0), shear/width, y-shear*x/width)
    return min(x, x+width), max(x, x+width), lower, (a, b, c)


def area(caps, rect):
    """Area of the intersection; winding regions are constructed separately."""
    x0, y0, x1, y1 = map(decimal, rect)
    lowers, uppers = [(D(0), D(0), y0)], [(D(0), D(0), y1)]
    for cap in caps:
        lo, hi, lower, upper = boundaries(cap)
        x0, x1 = max(x0, lo), min(x1, hi)
        lowers.append(lower)
        uppers.append(upper)
    if x0 >= x1 or y0 >= y1:
        return D(0)
    boundaries_all = lowers+uppers
    cuts = {x0, x1}
    for p, q in combinations(boundaries_all, 2):
        cuts.update(r for r in roots(tuple(a-b for a, b in zip(p, q))) if x0 < r < x1)
    total = D(0)
    ordered = sorted(cuts)
    for lo, hi in zip(ordered, ordered[1:]):
        mid = (lo+hi)/2
        lower = max(lowers, key=lambda p: evaluate(p, mid))
        upper = min(uppers, key=lambda p: evaluate(p, mid))
        if evaluate(upper, mid) > evaluate(lower, mid):
            total += integral(tuple(a-b for a, b in zip(upper, lower)), lo, hi)
    # Decimal cancellation may leave a residual many orders below byte bounds.
    assert -D('1e-45') <= total <= (x1-x0)*(y1-y0)+D('1e-45')
    return max(D(0), total)


def intersect(a, b):
    return max(a[0], b[0]), max(a[1], b[1]), min(a[2], b[2]), min(a[3], b[3])


def oracle(caps, *, clip=PAGE, hole=None, marks=None, alpha=F(1), size=16, opacity=None):
    if marks is None:
        marks = [(PAGE, (255, 0, 0))]
    results = []
    for precision in (60, 90):
        with localcontext() as context:
            context.prec = precision
            pixels = bytearray()
            for y in reversed(range(size)):
                for x in range(size):
                    color = [D(1)]*3
                    for ink, rgb in marks:
                        rect = intersect(intersect(clip, ink), (F(x), F(y), F(x+1), F(y+1)))
                        coverage = area(caps, rect)
                        if hole:
                            coverage -= area(caps, intersect(rect, hole))
                        for c, value in enumerate(rgb):
                            color[c] = color[c]*(1-coverage)+D(value)/255*coverage
                    for value in color:
                        weight = decimal(alpha)*(decimal(opacity(x, y)) if opacity else 1)
                        channel = 255*((1-weight)+weight*value)
                        rounded = int(channel+D('.5'))
                        # Ties are allowed by ADR 0004. These fixtures do not
                        # depend on which side an unrepresentable tie chooses.
                        distance = abs(channel+D('.5')-rounded)
                        assert distance == 0 or distance > D('1e-35')
                        pixels.append(rounded)
            results.append(b"P6\n%d %d\n255\n" % (size, size)+pixels)
    assert results[0] == results[1], "analytic curve oracle needs more precision"
    return results[0]


def original_quadratic_oracle():
    """Preserved #323 curve: x(t)=2+9t², y(t)=2+18t-18t².

    Its stencil clips to [9/4,51/4]². Split parameter t at the four
    pixel/stencil boundaries, then integrate min(y(t),y1)-y0 against
    dx/dt=18t. The antiderivative is a polynomial, with algebraic endpoints.
    This intentionally does not replace or flatten the original curve.
    """
    results = []
    for precision in (60, 90):
        with localcontext() as context:
            context.prec = precision
            pixels = bytearray()
            for y in reversed(range(16)):
                for x in range(16):
                    x0, y0 = max(D(x), D('2.25')), max(D(y), D('2.25'))
                    x1, y1 = min(D(x+1), D('12.75')), min(D(y+1), D('12.75'))
                    total = D(0)
                    if x0 < x1 and y0 < y1:
                        cuts = {D(0), D(1)}
                        for boundary in (x0, x1):
                            cuts.update(t for t in roots((D(9), D(0), 2-boundary)) if 0 < t < 1)
                        for boundary in (y0, y1):
                            cuts.update(t for t in roots((D(-18), D(18), 2-boundary)) if 0 < t < 1)
                        ordered = sorted(cuts)
                        for lo, hi in zip(ordered, ordered[1:]):
                            t = (lo+hi)/2
                            if not x0 < 2+9*t*t < x1 or 2+18*t-18*t*t <= y0:
                                continue
                            if 2+18*t-18*t*t >= y1:
                                total += 9*(y1-y0)*(hi*hi-lo*lo)
                            else:
                                total += 9*(2-y0)*(hi*hi-lo*lo)+108*(hi**3-lo**3)-81*(hi**4-lo**4)
                    value = int(255*(1-total)+D('.5'))
                    pixels.extend((255, value, value))
            results.append(b"P6\n16 16\n255\n"+pixels)
    assert results[0] == results[1]
    return results[0]


def glyph(cap):
    width, shear, height, x, y = cap
    return b"BT /F 1 Tf " + b" ".join(number(v) for v in (width, shear, 0, height, x, y)) + b" Tm (A) Tj ET "


def fixture(name, caps=(CAP,), *, leaf="fill", clip=PAGE, winding=None,
            alpha=F(1), soft=False, group=False, ais=False, direct=False,
            reverse=False, duplicate=False, repetitions=1, tiles=1):
    """The outer all-painted image stencil activates inherited geometry.

    A pattern marks each ancestor glyph using the next pattern; each glyph
    keeps its own affine map. The final cell contains the chosen leaf mark.
    The direct twin paints a solid glyph in the original cell, a path the
    frozen refactoring baseline already supports with true curves.
    """
    objects = []
    def add(body):
        objects.append(body)
        return str(len(objects)+4).encode()+b" 0 R"
    points = [(0, 0, True), (500, 1000, False), (1000, 0, True)]
    if reverse:
        points.reverse()
    points *= 2 if duplicate else 1
    contours = [2, 5] if duplicate else [2]
    outline = simple(points, contours=contours)
    outline = outline[:2]+struct.pack(">hhhh", 0, 0, 1000, 1000)+outline[10:]
    program = truetype(glyph_override=outline,
                       table_overrides={b"hmtx": struct.pack(">HhHh", 500, 0, 600, 0)+bytes(6)})
    program_ref = add(stream(program, b"/Length1 "+str(len(program)).encode()))
    descriptor = add(b"<< /Type /FontDescriptor /FontName /Cap /Flags 32 /FontBBox [0 0 1000 1000] /ItalicAngle 0 /Ascent 1000 /Descent 0 /CapHeight 1000 /StemV 50 /FontFile2 "+program_ref+b" >>")
    font = add(b"<< /Type /Font /Subtype /TrueType /BaseFont /Cap /FirstChar 65 /LastChar 65 /Widths [600] /Encoding /WinAnsiEncoding /FontDescriptor "+descriptor+b" >>")
    image = add(stream(b"\0", b"/Type /XObject /Subtype /Image /Width 1 /Height 1 /ImageMask true /BitsPerComponent 1"))
    marks = [(PAGE, (255, 0, 0))]
    leaf_content = b"1 0 0 rg 0 0 16 16 re f"
    leaf_resources = b"/Font << /F "+font+b" >> "
    effective_caps = list(caps)
    if leaf == "glyph":
        leaf_content = b"1 0 0 rg "+glyph(THIRD)
        effective_caps.append(THIRD)
    elif leaf == "half-triangle":
        # CAP wholly contains [4,5]×[3,4]. This right triangle has
        # exactly half that pixel's area; its equal-area rectangle below
        # supplies the same independent pixel oracle without flattening.
        assert area(caps, (4, 3, 5, 4)) == 1
        leaf_content = b"1 0 0 rg 4 3 m 5 3 l 4 4 l h f"
        marks = [((F(4), F(3), F(9, 2), F(4)), (255, 0, 0))]
    elif leaf == "half":
        leaf_content = b"1 0 0 rg 4 3 .5 1 re f"
        marks = [((F(4), F(3), F(9, 2), F(4)), (255, 0, 0))]
    elif leaf == "stroke":
        leaf_content = b"1 0 0 RG 1.5 w 1 4.25 m 15 4.25 l S"
        marks = [((F(1), F(7, 2), F(15), F(5)), (255, 0, 0))]
    elif leaf == "image":
        colored = add(stream(bytes([255, 0, 0]), b"/Type /XObject /Subtype /Image /Width 1 /Height 1 /ColorSpace /DeviceRGB /BitsPerComponent 8"))
        leaf_resources += b"/XObject << /C "+colored+b" >> "
        leaf_content = b"q 16 0 0 16 0 0 cm /C Do Q"
    elif leaf == "shading":
        leaf_resources += b"/Shading << /S << /ShadingType 2 /ColorSpace /DeviceRGB /Coords [0 0 16 0] /Function << /FunctionType 2 /Domain [0 1] /C0 [1 0 0] /C1 [1 0 0] /N 1 >> /Extend [true true] >> >> "
        leaf_content = b"/S sh"
    elif leaf == "source-order":
        blue = (F(25, 4), F(13, 4), F(43, 4), F(29, 4))
        leaf_content += b" 0 0 1 rg 6.25 3.25 4.5 4 re f"
        marks.append((blue, (0, 0, 255)))
    if direct:
        assert len(caps) == 1 and leaf == "fill"
        leaf_content = b"1 0 0 rg "+glyph(caps[0])
        ancestors = []
    else:
        ancestors = list(caps)
    if group:
        form = add(stream(leaf_content, b"/Type /XObject /Subtype /Form /BBox [0 0 16 16] /Group << /S /Transparency /I true /CS /DeviceRGB >> /Resources << "+leaf_resources+b">>"))
        leaf_content, leaf_resources = b"/G Do", b"/XObject << /G "+form+b" >> "
    target = add(stream(leaf_content, COMMON+b"/Resources << "+leaf_resources+b">>"))
    for cap in reversed(ancestors):
        target = add(stream(b"/Pattern cs /Q scn "+glyph(cap), COMMON+b"/Resources << /Pattern << /Q "+target+b" >> /Font << /F "+font+b" >> >>"))
    hole = (F(25, 4), F(13, 4), F(37, 4), F(25, 4)) if winding else None
    clipping = b""
    if winding:
        assert hole is not None
        contours_path = polygon(rectangle(*PAGE))
        hole_points = rectangle(*hole)
        if winding == "reverse":
            hole_points.reverse()
        contours_path += polygon(hole_points)
        clipping = contours_path+(b"W* n " if winding == "evenodd" else b"W n ")
        if winding == "same":
            hole = None
    extent = 16*tiles
    assert tiles == 1 or clip == PAGE and not winding
    clipping += polygon(rectangle(*(clip if tiles == 1 else (0, 0, extent, extent))))+b"W n "
    resources = b"<< /Pattern << /P "+target+b" >> /Font << /F "+font+b" >> /XObject << /I "+image+b" >> /ExtGState << /A << /ca "+number(alpha)+b" /AIS "+(b"true" if ais else b"false")
    if soft:
        mask = add(stream(b"/H gs 0 0 16 16 re f", b"/Type /XObject /Subtype /Form /BBox [0 0 16 16] /Group << /S /Transparency /I true /CS /DeviceRGB >> /Resources << /ExtGState << /H << /ca .5 >> >> >>"))
        resources += b" /SMask << /S /Alpha /G "+mask+b" >>"
    resources += b" >> >> >>"
    content = clipping+b"/A gs /Pattern cs /P scn q "+str(extent).encode()+b" 0 0 "+str(extent).encode()+b" 0 0 cm /I Do Q"
    if direct:
        if tiles == 1:
            content = clipping+b"/A gs 1 0 0 rg "+glyph(caps[0])
        else:
            content = clipping+b"/A gs /Pattern cs /P scn 0 0 "+str(extent).encode()+b" "+str(extent).encode()+b" re f"
    if repetitions != 1:
        content = b" ".join(b"q "+content+b" Q" for _ in range(repetitions))
        # Source-over recurrence is evaluated independently per repetition.
        marks *= repetitions
    expected = oracle(effective_caps, clip=clip, hole=hole, marks=marks,
                      alpha=alpha*(F(1, 2) if soft else 1))
    if tiles == 1:
        source = page(content, objects, resources)
    else:
        source = document([b"<< /Type /Catalog /Pages 2 0 R >>",
                           b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
                           b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 "+str(extent).encode()+b" "+str(extent).encode()+b"] /Resources "+resources+b" /Contents 4 0 R >>",
                           stream(content), *objects])
        body = expected.split(b"\n", 3)[3]
        rows = [body[row*48:(row+1)*48]*tiles for row in range(16)]
        expected = b"P6\n%d %d\n255\n" % (extent, extent)+b"".join(rows)*tiles
    return Case(name, source, RENDER,
                ("paint-image-masks", "paint-text-outlines", "raster-curves"),
                "Analytic true quadratic caps jointly intersect stencil, differently mapped glyph ancestors, leaf geometry and winding clips",
                oracle=expected, extra={"tolerance": 1})


def cases():
    yield fixture("cap-fractional")
    yield fixture("cap-fractional-direct", direct=True)
    yield fixture("cap-sheared", (SECOND,))
    yield fixture("cap-sheared-direct", (SECOND,), direct=True)
    yield fixture("cap-reflected", (THIRD,))
    yield fixture("cap-reflected-direct", (THIRD,), direct=True)
    yield fixture("cap-reversed-contour", reverse=True)
    yield fixture("cap-duplicate-nonzero", duplicate=True)
    yield fixture("cap-fractional-clip", clip=(F(15, 4), F(13, 4), F(39, 4), F(29, 4)))
    yield fixture("cap-evenodd-clip", winding="evenodd")
    yield fixture("cap-nonzero-hole", winding="reverse")
    yield fixture("cap-nonzero-filled", winding="same")
    yield fixture("two-curved-ancestors", (CAP, SECOND))
    yield fixture("three-curved-ancestors", (CAP, SECOND, THIRD))
    yield fixture("coincident-curved-ancestors", (CAP, CAP))
    yield fixture("curved-leaf", leaf="glyph")
    yield fixture("two-ancestors-curved-leaf", (CAP, SECOND), leaf="glyph")
    yield fixture("curved-ancestor-stroke", leaf="stroke")
    yield fixture("curved-ancestor-image", leaf="image")
    yield fixture("curved-ancestor-shading", leaf="shading")
    yield fixture("curved-ancestor-group", group=True)
    yield fixture("curved-ancestor-softmask", soft=True)
    yield fixture("curved-ancestor-alpha", alpha=F(1, 2))
    yield fixture("curved-ancestor-alpha-is-shape", alpha=F(1, 2), ais=True)
    yield fixture("curved-ancestor-source-order", leaf="source-order")
    yield fixture("curved-source-order-alpha", leaf="source-order", alpha=F(1, 2))
    yield fixture("multi-curved-group-softmask", (CAP, SECOND), leaf="glyph", group=True, soft=True, ais=True)
    yield fixture("stress-256-curved", tiles=16)
    yield fixture("stress-256-direct", tiles=16, direct=True)
    yield fixture("stress-256-three-curves", (CAP, SECOND), leaf="glyph", tiles=16)
    yield fixture("half-pixel-leaf", leaf="half")
    yield fixture("half-pixel-clip", clip=(F(4), F(3), F(9, 2), F(4)))
    yield fixture("singular-curve-point", ((F(0), F(0), F(0), F(17, 4), F(17, 4)),))
    yield fixture("singular-curve-point-direct", ((F(0), F(0), F(0), F(17, 4), F(17, 4)),), direct=True)
    yield fixture("half-pixel-diagonal-leaf", leaf="half-triangle")
    yield fixture("three-coincident-curved-ancestors", (CAP, CAP, CAP))
