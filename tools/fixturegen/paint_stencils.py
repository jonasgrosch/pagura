"""Patterned stencil geometry from ISO 32000-2 8.7.3 and 8.9.6.2.

The oracle independently intersects rational sample rectangles, each cell's
ink and clipping polygons, then composites cell marks with rational source-over
(11.3.3, 11.6.7; ADR 0004 per-pixel sources). No engine or image decoder
generates expectations. A single-cell fractional case additionally has the
unambiguous continuous geometric-union result.
"""
from collections.abc import Callable
from fractions import Fraction as F

from .model import Case, RENDER
from .pdf import document, stream
from .raster import winding_area

AREA = "paint_stencils"
SIZE = 16
RED, GREEN = (255, 0, 0), (0, 255, 0)
ROWS = ((0, 1, 0), (1, 0, 1))


def number(value):
    return str(float(value)).encode()


def polygon(points):
    return (b" ".join(number(v) for v in points[0]) + b" m "
            + b" ".join(b" ".join(number(v) for v in p) + b" l" for p in points[1:]) + b" h ")


def rectangle(x0, y0, x1, y1):
    return [(x0, y0), (x1, y0), (x1, y1), (x0, y1)]


def transformed(points, matrix):
    a, b, c, d, e, f = matrix
    return [(a*x + c*y + e, b*x + d*y + f) for x, y in points]


def mask_polygons(matrix, inverted=False, rows=ROWS):
    return [transformed(rectangle(F(x, 3), F(1)-F(y+1, 2), F(x+1, 3), F(1)-F(y, 2)), matrix)
            for y, row in enumerate(rows) for x, value in enumerate(row) if value == int(inverted)]


def oracle(masks, ink, clip, color, alpha=F(1), color_at: Callable[[F, F], tuple[F, F, F]] | None = None):
    pixels = bytearray()
    for y in reversed(range(SIZE)):
        for x in range(SIZE):
            remaining = F(1)
            for cell in ink:
                if (max(p[0] for p in cell) <= x or min(p[0] for p in cell) >= x+1
                        or max(p[1] for p in cell) <= y or min(p[1] for p in cell) >= y+1):
                    continue
                groups = [("nonzero", masks), ("nonzero", [cell])]
                if clip:
                    groups.append(("nonzero", [clip]))
                remaining *= 1 - winding_area(groups, x, y)
            coverage = (1 - remaining) * alpha
            sample = color if color_at is None else color_at(F(x)+F(1,2), F(y)+F(1,2))
            pixels.extend(int(255 - (255-c)*coverage + F(1, 2)) for c in sample)
    return b"P6\n16 16\n255\n" + pixels



def interpolated_oracle(matrix, ink, clip, color, alpha=F(1), inverted=False, rows=ROWS,
                        color_at: Callable[[F, F], tuple[F, F, F]] | None = None):
    """Rational ADR0004 source sampling, distinct from geometric coverage.

    Magnified Interpolate sources take four clamped sample-centre weights
    at the global pixel centre. Unit-square/clip/ink areas are intersected
    independently. Apply one opacity to the completed pattern, not once per
    tile mark. Minification keeps the exact painted source-cell geometry.
    """
    a, b, c, d, e, f = matrix
    determinant = a*d-b*c
    # Device-pixel basis vectors mapped to the source grid, before the
    # source y reversal. Rational comparisons suffice for these fixtures.
    minified = ((3*d/determinant)**2 + (2*b/determinant)**2 > 1
                or (3*c/determinant)**2 + (2*a/determinant)**2 > 1)
    if minified:
        return oracle(mask_polygons(matrix, inverted, rows), ink, clip, color, alpha, color_at)
    unit = transformed(rectangle(F(0), F(0), F(1), F(1)), matrix)
    pixels = bytearray()
    for y in reversed(range(SIZE)):
        for x in range(SIZE):
            remaining = F(1)
            for cell in ink:
                if (max(p[0] for p in cell) <= x or min(p[0] for p in cell) >= x+1
                        or max(p[1] for p in cell) <= y or min(p[1] for p in cell) >= y+1):
                    continue
                groups = [("nonzero", [unit]), ("nonzero", [cell])]
                if clip:
                    groups.append(("nonzero", [clip]))
                remaining *= 1-winding_area(groups, x, y)
            px, py = F(x)+F(1, 2)-e, F(y)+F(1, 2)-f
            column = 3*(d*px-c*py)/determinant-F(1, 2)
            row = 2*(1-(-b*px+a*py)/determinant)-F(1, 2)
            left, top = column//1, row//1
            fx, fy = column-left, row-top
            opacity = F(0)
            for ix, iy, weight in [(left, top, (1-fx)*(1-fy)), (left+1, top, fx*(1-fy)),
                                   (left, top+1, (1-fx)*fy), (left+1, top+1, fx*fy)]:
                opacity += int(rows[max(0,min(1,iy))][max(0,min(2,ix))] == int(inverted))*weight
            coverage = (1-remaining)*alpha*opacity
            sample = color if color_at is None else color_at(F(x)+F(1,2), F(y)+F(1,2))
            pixels.extend(int(255-(255-value)*coverage+F(1, 2)) for value in sample)
    return b"P6\n16 16\n255\n"+pixels


def page(content, objects, resources):
    return document([
        b"<< /Type /Catalog /Pages 2 0 R >>",
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 16 16] /Resources " + resources + b" /Contents 4 0 R >>",
        stream(content), *objects,
    ])


def control(name, kind, matrix, *, inverted=False, clip=None, twin=False,
            interpolate=False, alpha=F(1), rows=ROWS, origin=(F(1, 4), F(1, 4)),
            nested=False, soft=False, overlap=False, gradient=False):
    masks = mask_polygons(matrix, inverted, rows)
    color = GREEN if kind == "uncolored" else RED
    ox, oy = origin
    if kind == "shading":
        pattern = b"<< /Type /Pattern /PatternType 2 /Shading << /ShadingType 2 /ColorSpace /DeviceRGB /Coords [0 0 16 0] /Function << /FunctionType 2 /Domain [0 1] /C0 [1 0 0] /C1 [1 0 0] /N 1 >> /Extend [true true] >> >>"
        if gradient:
            pattern = pattern.replace(b"/C1 [1 0 0]", b"/C1 [0 0 1]")
            pattern = pattern[:-2]+b" /Matrix [1 .25 .5 1 0 0] >>"
        ink = [rectangle(F(0), F(0), F(SIZE), F(SIZE))]
    else:
        paint_type = b"2" if kind == "uncolored" else b"1"
        ink_width, x_step = (F(7,2), F(2)) if overlap else (F(3,2), F(4))
        content = (b"" if kind == "uncolored" else b"1 0 0 rg ")+b"0 0 "+number(ink_width)+b" 4 re f"
        pattern = stream(content, b"/Type /Pattern /PatternType 1 /PaintType " + paint_type
                         + b" /TilingType 2 /BBox [0 0 4 4] /XStep "+(b"2" if overlap else b"4")+b" /YStep 4 /Resources << >> /Matrix [1 0 0 1 "
                         + number(ox) + b" " + number(oy) + b"]")
        ink = [rectangle(x_step*x+ox, F(4*y)+oy, x_step*x+ox+ink_width, F(4*y)+oy+4)
               for y in range(-1, 5) for x in (range(-2, 9) if overlap else range(-1, 5))]
    packed = bytes(sum(value << (7-x) for x, value in enumerate(row)) for row in rows)
    image = stream(packed, b"/Type /XObject /Subtype /Image /Width 3 /Height 2 /ImageMask true /BitsPerComponent 1"
                   + (b" /Decode [1 0]" if inverted else b"")
                   + (b" /Interpolate true" if interpolate else b""))
    resources = b"<< /ColorSpace << /CS [/Pattern /DeviceRGB] >> /Pattern << /P 5 0 R >> /XObject << /I 6 0 R >> /ExtGState << /G << /ca " + number(alpha) + b" >> >> >>"
    selected = b"/CS cs 0 1 0 /P scn " if kind == "uncolored" else b"/Pattern cs /P scn "
    clipping = polygon(clip) + b"W n " if clip else b""
    if twin:
        painted = b"".join(polygon(p) for p in masks) + b"f"
    else:
        painted = b" ".join(number(v) for v in matrix) + b" cm /I Do"
    objects = [pattern, image]
    if nested or soft:
        soft_group = stream(b"/Half gs 0 0 16 16 re f",
                            b"/Type /XObject /Subtype /Form /BBox [0 0 16 16] /Group << /S /Transparency /I true /CS /DeviceRGB >> /Resources << /ExtGState << /Half << /ca .5 >> >> >>")
        objects.append(soft_group)
    if soft:
        resources = resources.replace(b"/ca " + number(alpha), b"/SMask << /S /Alpha /G 7 0 R >> /ca " + number(alpha))
    if nested:
        group = stream(clipping + selected + painted,
                       b"/Type /XObject /Subtype /Form /BBox [0 0 16 16] /Group << /S /Transparency /I true /CS /DeviceRGB >> /Resources " + resources)
        objects.append(group)
        resources = resources.replace(b"/I 6 0 R", b"/I 6 0 R /F 8 0 R")
        content = b"/G gs /F Do"
    else:
        content = clipping + b"/G gs " + selected + painted
    data = page(content, objects, resources)
    effective_alpha = alpha * (F(1, 2) if soft else 1)
    def gradient_color(x: F, y: F) -> tuple[F, F, F]:
        # Invert pattern matrix [1 .25 .5 1 0 0], then project onto
        # the [0,0]..[16,0] axial segment and clamp to its extended ends.
        t = max(F(0),min(F(1),(2*x-y)/28))
        return 255*(1-t),F(0),255*t
    source_color = gradient_color if gradient else None
    expected = (interpolated_oracle(matrix, ink, clip, color, effective_alpha, inverted, rows, source_color) if interpolate
                else oracle(masks, ink, clip, color, effective_alpha, source_color))
    return Case(name, data, RENDER, ("paint-image-masks",),
                "Exact geometric coverage and rational pixel-centre bilinear source opacity" if interpolate
                else "Exact geometric patterned stencil; independent rational joint-area oracle",
                oracle=expected, extra={"tolerance": 1})


def different_masks(interpolate=False):
    """The same tiling paint uses two different image sources and placements."""
    first = (F(6), F(0), F(0), F(4), F(1), F(1))
    second = (F(6), F(0), F(0), F(4), F(8), F(9))
    opposite = tuple(tuple(1-code for code in row) for row in ROWS)
    masks = mask_polygons(first) + mask_polygons(second, rows=opposite)
    pattern = stream(b"1 0 0 rg 0 0 1.5 4 re f", b"/Type /Pattern /PatternType 1 /PaintType 1 /TilingType 2 /BBox [0 0 4 4] /XStep 4 /YStep 4 /Resources << >>")
    def image(rows):
        packed = bytes(sum(value << (7-x) for x, value in enumerate(row)) for row in rows)
        return stream(packed, b"/Type /XObject /Subtype /Image /Width 3 /Height 2 /ImageMask true /BitsPerComponent 1"+(b" /Interpolate true" if interpolate else b""))
    content = b"/Pattern cs /P scn q " + b" ".join(number(v) for v in first) + b" cm /I Do Q q " + b" ".join(number(v) for v in second) + b" cm /J Do Q"
    resources = b"<< /Pattern << /P 5 0 R >> /XObject << /I 6 0 R /J 7 0 R >> >>"
    ink = [rectangle(F(4*x), F(4*y), F(4*x)+F(3, 2), F(4*y)+4) for y in range(4) for x in range(4)]
    expected = oracle(masks, ink, None, RED)
    if interpolate:
        # The two unit squares have disjoint columns. Every pixel belongs
        # to at most one placement; combine their independently calculated
        # RGB values on white without introducing a second quantization.
        a = interpolated_oracle(first, ink, None, RED)
        b = interpolated_oracle(second, ink, None, RED, rows=opposite)
        header = b"P6\n16 16\n255\n"
        expected = header+bytes(min(x,y) for x,y in zip(a[len(header):], b[len(header):]))
    return Case("colored-different-interpolated-masks" if interpolate else "colored-different-masks",
                page(content, [pattern, image(ROWS), image(opposite)], resources),
                RENDER, ("paint-image-masks",), "Separate source and clipping identities for two masks sharing one pattern",
                oracle=expected, extra={"tolerance": 1})


def nested_tiling_cases():
    """A half-area sample distinguishes a mask applied once from twice.

    A red full-cell pattern is geometrically identical whether specified
    directly or through another pattern. Each nested mark propagates its
    outer geometry so the inherited mask is intersected once, before
    quantization, rather than multiplying its area average again.
    """
    common = b"/Type /Pattern /PatternType 1 /PaintType 1 /TilingType 2 /BBox [0 0 16 16] /XStep 16 /YStep 16 "
    solid = stream(b"1 0 0 rg 0 0 16 16 re f", common+b"/Resources << >>")
    mask = stream(b"\0", b"/Type /XObject /Subtype /Image /Width 1 /Height 1 /ImageMask true /BitsPerComponent 1")
    resources = b"<< /Pattern << /P 5 0 R >> /XObject << /I 6 0 R >> >>"
    content = b"/Pattern cs /P scn 1 0 0 1 4.5 4 cm /I Do"
    half = [rectangle(F(9, 2), F(4), F(11, 2), F(5))]
    expected = oracle(half, [rectangle(F(0), F(0), F(16), F(16))], None, RED)
    yield Case("tiling-flat-half-area", page(content, [solid, mask, solid], resources), RENDER,
               ("paint-image-masks",), "One opaque red stencil covers exactly half of two boundary pixels",
               oracle=expected, extra={"tolerance": 1})
    nested = stream(b"/Pattern cs /Q scn 0 0 16 16 re f", common+b"/Resources << /Pattern << /Q 7 0 R >> >>")
    # Historical identifier retained to preserve the original reproducer bytes.
    yield Case("tiling-nested-ordinary-refused", page(content, [nested, mask, solid], resources), RENDER,
               ("paint-image-masks",), "Ordinary nested tiling integrates inherited stencil geometry once",
               oracle=expected, extra={"tolerance": 1})
    nested_image = stream(b"/Pattern cs /Q scn 16 0 0 16 0 0 cm /J Do",
                          common+b"/Resources << /Pattern << /Q 7 0 R >> /XObject << /J 8 0 R >> >>")
    yield Case("tiling-nested-image-stencil", page(content, [nested_image, mask, solid, mask], resources), RENDER,
               ("paint-image-masks",), "Nested image stencils preserve the full inherited geometric intersection",
               oracle=expected, extra={"tolerance": 1})



def interpolated_knockout_cases():
    """A blue knockout backdrop detects missing stencil shape modulation.

    The red stencil has alpha A=m/2 and shape S=m (AIS false) or m/2
    (AIS true). The knockout group removes blue by S, then contributes A
    red, leaving group alpha 1-S+A. On white this gives RGB=(S,S-A,1-A).
    """
    for kind in ("colored", "shading"):
        for ais in (False, True):
            if kind == "colored":
                pattern = stream(b"1 0 0 rg 0 0 16 16 re f", b"/Type /Pattern /PatternType 1 /PaintType 1 /TilingType 2 /BBox [0 0 16 16] /XStep 16 /YStep 16 /Resources << >>")
            else:
                pattern = b"<< /Type /Pattern /PatternType 2 /Shading << /ShadingType 2 /ColorSpace /DeviceRGB /Coords [0 0 16 0] /Function << /FunctionType 2 /Domain [0 1] /C0 [1 0 0] /C1 [1 0 0] /N 1 >> /Extend [true true] >> >>"
            image = stream(b"\x40", b"/Type /XObject /Subtype /Image /Width 2 /Height 1 /ImageMask true /BitsPerComponent 1 /Interpolate true")
            state = b"<< /ca .5 /AIS "+(b"true" if ais else b"false")+b" >>"
            resources = b"<< /Pattern << /P 5 0 R >> /XObject << /I 6 0 R >> /ExtGState << /G "+state+b" >> >>"
            content = b"0 0 1 rg 0 0 16 16 re f /G gs /Pattern cs /P scn 12 0 0 8 2 4 cm /I Do"
            group = stream(content, b"/Type /XObject /Subtype /Form /BBox [0 0 16 16] /Group << /S /Transparency /I true /K true /CS /DeviceRGB >> /Resources "+resources)
            data = page(b"/F Do", [pattern,image,group], b"<< /XObject << /F 7 0 R >> >>")
            pixels = bytearray()
            for y in reversed(range(SIZE)):
                for x in range(SIZE):
                    m = max(F(0),min(F(1), F(3,2)-(F(x)+F(1,2)-2)/6)) if 2<=x<14 and 4<=y<12 else F(0)
                    alpha, shape = m/2, m/2 if ais else m
                    pixels.extend(int(255*v+F(1,2)) for v in (shape,shape-alpha,1-alpha))
            yield Case(kind+"-interpolate-knockout-ais-"+("true" if ais else "false"), data, RENDER,
                       ("paint-image-masks",), "Rational bilinear stencil alpha and shape in isolated knockout group with blue backdrop and AIS",
                       oracle=b"P6\n16 16\n255\n"+pixels, extra={"tolerance":1})



def interpolated_equivalent_cases():
    """Equivalent-output controls for interpolation representation costs.

    Pure process colours keep image associated-colour sampling exact.
    Every pattern paints one full cell; the counterpart is an ordinary
    Interpolate stencil with the same colour, source bits and placement.
    """
    matrix = (F(12),F(0),F(0),F(10),F(2),F(3))
    ink = [rectangle(F(0),F(0),F(16),F(16))]
    for kind in ("colored", "uncolored", "shading"):
        color = GREEN if kind == "uncolored" else RED
        if kind == "shading":
            pattern = b"<< /Type /Pattern /PatternType 2 /Shading << /ShadingType 2 /ColorSpace /DeviceRGB /Coords [0 0 16 0] /Function << /FunctionType 2 /Domain [0 1] /C0 [1 0 0] /C1 [1 0 0] /N 1 >> /Extend [true true] >> >>"
        else:
            data = b"0 0 16 16 re f" if kind == "uncolored" else b"1 0 0 rg 0 0 16 16 re f"
            pattern = stream(data,b"/Type /Pattern /PatternType 1 /PaintType "+(b"2" if kind=="uncolored" else b"1")+b" /TilingType 2 /BBox [0 0 16 16] /XStep 16 /YStep 16 /Resources << >>")
        packed = bytes(sum(value<<(7-x) for x,value in enumerate(row)) for row in ROWS)
        image = stream(packed,b"/Type /XObject /Subtype /Image /Width 3 /Height 2 /ImageMask true /BitsPerComponent 1 /Interpolate true")
        resources = b"<< /ColorSpace << /CS [/Pattern /DeviceRGB] >> /Pattern << /P 5 0 R >> /XObject << /I 6 0 R >> >>"
        selected = b"/CS cs 0 1 0 /P scn " if kind=="uncolored" else b"/Pattern cs /P scn "
        solid = b"0 1 0 rg " if kind=="uncolored" else b"1 0 0 rg "
        placement = b"12 0 0 10 2 3 cm /I Do"
        expected = interpolated_oracle(matrix,ink,None,color)
        for variant,paint in [("pattern",selected),("solid",solid)]:
            yield Case("interpolate-matched-"+kind+"-"+variant,page(paint+placement,[pattern,image],resources),
                       RENDER,("paint-image-masks",),"Equivalent complete raster: bilinear patterned stencil versus ordinary solid stencil",
                       oracle=expected,extra={"tolerance":1})
    size, samples = 512, 64
    packed = bytes(sum(((x+y)%2)<<(7-x%8) for x in range(column,column+8))
                   for y in range(samples) for column in range(0,samples,8))
    image = stream(packed,b"/Type /XObject /Subtype /Image /Width 64 /Height 64 /ImageMask true /BitsPerComponent 1 /Interpolate true")
    pattern = stream(b"1 0 0 rg 0 0 512 512 re f",b"/Type /Pattern /PatternType 1 /PaintType 1 /TilingType 2 /BBox [0 0 512 512] /XStep 512 /YStep 512 /Resources << >>")
    pixels = bytearray()
    for y in range(size):
        top, fy = divmod(2*y+1-8,16)
        for x in range(size):
            left, fx = divmod(2*x+1-8,16)
            opacity = sum(int((max(0,min(samples-1,column))+max(0,min(samples-1,row)))%2==0)*weight
                          for column,row,weight in [(left,top,(16-fx)*(16-fy)),(left+1,top,fx*(16-fy)),
                                                    (left,top+1,(16-fx)*fy),(left+1,top+1,fx*fy)])
            white = (255*(256-opacity)+128)//256
            pixels.extend((255,white,white))
    expected = b"P6\n512 512\n255\n"+pixels
    for variant,paint in [("pattern",b"/Pattern cs /P scn "),("solid",b"1 0 0 rg ")]:
        data = document([b"<< /Type /Catalog /Pages 2 0 R >>",b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
                         b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 512 512] /Resources << /Pattern << /P 5 0 R >> /XObject << /I 6 0 R >> >> /Contents 4 0 R >>",
                         stream(paint+b"512 0 0 512 0 0 cm /I Do"),pattern,image])
        yield Case("interpolate-stress-"+variant,data,RENDER,("paint-image-masks",),
                   "262144-pixel equivalent-output stress; exact integer bilinear opacity weights over a 64x64 checkerboard",
                   oracle=expected,extra={"tolerance":1})



def interpolated_overprint_cases():
    """Clause 11.7.4.3 on a half-alpha CMYK backdrop, with K=0.

    Non-Normal overprinting substitutes each unpainted source component by
    Cs'=(1-Ab)Cs+Ab*Cb before blending. A neutral one-mark group produces
    exactly this implicit source; substituting again at the result would
    fail these translucent-backdrop cases. Normal uses backdrop components
    in its mixed result. All arithmetic below is rational and independent.
    """
    width, height = F(12), F(8)
    left, lower = F(9,4), F(17,4)
    back_alpha = F(1,2)
    back = (F(4,5),F(3,5),F(2,5),F(1))
    for kind in ("shading", "tiling"):
        for space in ("separation", "devicen"):
            tint = b"<< /FunctionType 2 /Domain [0 1] /C0 [0 0 0 0] /C1 [1 0 0 0] /N 1 >>"
            colorspace = (b"[/Separation /Cyan /DeviceCMYK "+tint+b"]" if space=="separation"
                          else b"[/DeviceN [/Cyan] /DeviceCMYK "+tint+b"]")
            for varying in ((False, True) if kind=="shading" else (False,)):
                endpoints = b"/C0 [.25] /C1 [.75]" if varying else b"/C0 [.5] /C1 [.5]"
                if kind=="shading":
                    pattern = b"<< /Type /Pattern /PatternType 2 /Shading << /ShadingType 2 /ColorSpace 7 0 R /Coords [0 0 16 0] /Function << /FunctionType 2 /Domain [0 1] "+endpoints+b" /N 1 >> /Extend [true true] >> >>"
                else:
                    pattern = stream(b"/On gs /C cs .5 scn 0 0 16 16 re f",b"/Type /Pattern /PatternType 1 /PaintType 1 /TilingType 2 /BBox [0 0 16 16] /XStep 16 /YStep 16 /Resources << /ColorSpace << /C 7 0 R >> /ExtGState << /On << /op true /OP true /OPM 1 >> >> >>")
                for blend in ("Normal", "Multiply", "Screen"):
                    image = stream(b"\x40",b"/Type /XObject /Subtype /Image /Width 2 /Height 1 /ImageMask true /BitsPerComponent 1 /Interpolate true")
                    state = b"<< /op true /OP true /OPM 1 /ca .5 /AIS true /BM /"+blend.encode()+b" /SMask << /S /Alpha /G 8 0 R >> >>"
                    resources = b"<< /Pattern << /P 5 0 R >> /XObject << /I 6 0 R >> /ExtGState << /Back << /ca .5 >> /G "+state+b" >> >>"
                    mask = stream(b"/Half gs 0 g 0 0 16 16 re f",b"/Type /XObject /Subtype /Form /BBox [0 0 16 16] /Group << /S /Transparency /I true /CS /DeviceGray >> /Resources << /ExtGState << /Half << /ca .5 >> >> >>")
                    content = b"/Back gs .2 .4 .6 0 k 0 0 16 16 re f /G gs /Pattern cs /P scn 12 0 0 8 2.25 4.25 cm /I Do"
                    data = document([b"<< /Type /Catalog /Pages 2 0 R /OutputIntents [<< /Type /OutputIntent /S /GTS_PDFX /OutputConditionIdentifier (Independent overprint fixture) >>] >>",
                                     b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
                                     b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 16 16] /Group << /S /Transparency /I true /CS /DeviceCMYK >> /Resources "+resources+b" /Contents 4 0 R >>",
                                     stream(content),pattern,image,colorspace,mask])
                    pixels = bytearray()
                    for y in reversed(range(SIZE)):
                        gy = max(F(0),min(F(y+1),lower+height)-max(F(y),lower))
                        for x in range(SIZE):
                            gx = max(F(0),min(F(x+1),left+width)-max(F(x),left))
                            m = max(F(0),min(F(1),F(3,2)-(F(x)+F(1,2)-left)/6))
                            alpha = gx*gy*m/4  # ca=.5, soft-mask alpha=.5
                            tint_value = F(1,4)+(F(x)+F(1,2))/32 if varying else F(1,2)
                            source = (1-tint_value,F(1),F(1),F(1))
                            values = []
                            for i,(cb,cs) in enumerate(zip(back,source)):
                                preserved = i!=0
                                if blend!="Normal" and preserved:
                                    cs = (1-back_alpha)*cs+back_alpha*cb
                                if blend=="Normal":
                                    mixed = cb if preserved else cs
                                elif blend=="Multiply":
                                    mixed = cb*cs
                                else:
                                    mixed = cb+cs-cb*cs
                                values.append((1-alpha)*back_alpha*cb+alpha*((1-back_alpha)*cs+back_alpha*mixed))
                            out_alpha = back_alpha+alpha*(1-back_alpha)
                            # Zero black ink makes additive CMYK-to-RGB the
                            # corresponding premultiplied component on white.
                            pixels.extend(int(255*(v+1-out_alpha)+F(1,2)) for v in values[:3])
                    name=kind+"-interpolate-"+space+("-gradient" if varying else "-constant")+"-overprint-"+blend.lower()
                    yield Case(name,data,RENDER,("paint-image-masks","overprint-spot","overprint-groups"),
                               "Rational partial-colorant source substitution with translucent CMYK backdrop, fractional geometry, AIS, alpha and soft mask",
                               oracle=b"P6\n16 16\n255\n"+pixels,extra={"tolerance":1})


def cases():
    aligned = (F(12), F(0), F(0), F(10), F(2), F(3))
    fractional = (F(21, 2), F(7, 2), F(-3, 2), F(19, 2), F(13, 4), F(9, 4))
    clip = [(F(1), F(1)), (F(15), F(4)), (F(7), F(15))]
    for kind in ("colored", "uncolored", "shading"):
        yield control(kind+"-aligned", kind, aligned)
        yield control(kind+"-fractional-clip", kind, fractional, clip=clip)
        yield control(kind+"-reversed", kind, fractional, inverted=True)
        yield control(kind+"-single-cell", kind, (F(2), F(0), F(0), F(2), F(1, 2), F(1, 2)))
        yield control(kind+"-interpolate", kind, aligned, interpolate=True)
        yield control(kind+"-interpolate-fractional-clip", kind, fractional, clip=clip, interpolate=True)
        yield control(kind+"-interpolate-reversed", kind, fractional, inverted=True, interpolate=True)
        yield control(kind+"-interpolate-minified", kind, (F(1),F(0),F(0),F(1),F(9,4),F(17,4)), interpolate=True)
        yield control(kind+"-interpolate-anisotropic", kind, (F(12),F(0),F(0),F(1),F(2),F(3)), interpolate=True)
        yield control(kind+"-interpolate-quarter-turn", kind, (F(0),F(12),F(-10),F(0),F(13),F(2)), interpolate=True)
        # Strict-valid old-engine equivalent for controlled timing: integer
        # cell/ink edges make coverage products coincide with joint areas.
        yield control(kind+"-matched-stencil", kind, aligned, origin=(F(0), F(0)))
        yield control(kind+"-matched-path", kind, aligned, twin=True, origin=(F(0), F(0)))
    yield control("colored-empty", "colored", aligned, rows=((1, 1, 1), (1, 1, 1)))
    yield control("colored-alpha", "colored", aligned, alpha=F(1, 2))
    yield control("colored-nested-soft-mask", "colored", aligned, nested=True, soft=True)
    yield control("colored-interpolate-alpha", "colored", aligned, alpha=F(1,2), interpolate=True)
    yield control("colored-interpolate-nested-soft-mask", "colored", aligned, nested=True, soft=True, interpolate=True)
    yield control("shading-interpolate-soft-mask", "shading", fractional, clip=clip, soft=True, interpolate=True)
    yield control("shading-interpolate-gradient-transform", "shading", fractional, clip=clip, interpolate=True, gradient=True)
    yield control("colored-interpolate-empty", "colored", aligned, rows=((1,1,1),(1,1,1)), interpolate=True)
    yield control("colored-interpolate-overlapping-cells", "colored", aligned, origin=(F(0),F(0)), overlap=True, interpolate=True)
    yield from interpolated_knockout_cases()
    yield from interpolated_equivalent_cases()
    yield from interpolated_overprint_cases()
    yield different_masks()
    yield different_masks(interpolate=True)
    yield from nested_tiling_cases()
