"""Patterned stencil geometry from ISO 32000-2 8.7.3 and 8.9.6.2.

The oracle independently intersects rational sample rectangles, each cell's
ink and clipping polygons, then composites cell marks with rational source-over
(11.3.3, 11.6.7; ADR 0004 per-pixel sources). No engine or image decoder
generates expectations. A single-cell fractional case additionally has the
unambiguous continuous geometric-union result.
"""
from fractions import Fraction as F

from .model import Case, RENDER, UNSUPPORTED
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


def oracle(masks, ink, clip, color, alpha=F(1)):
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
            pixels.extend(int(255 - (255-c)*coverage + F(1, 2)) for c in color)
    return b"P6\n16 16\n255\n" + pixels


def page(content, objects, resources):
    return document([
        b"<< /Type /Catalog /Pages 2 0 R >>",
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 16 16] /Resources " + resources + b" /Contents 4 0 R >>",
        stream(content), *objects,
    ])


def control(name, kind, matrix, *, inverted=False, clip=None, twin=False,
            interpolate=False, alpha=F(1), rows=ROWS, origin=(F(1, 4), F(1, 4)),
            nested=False, soft=False):
    masks = mask_polygons(matrix, inverted, rows)
    color = GREEN if kind == "uncolored" else RED
    ox, oy = origin
    if kind == "shading":
        pattern = b"<< /Type /Pattern /PatternType 2 /Shading << /ShadingType 2 /ColorSpace /DeviceRGB /Coords [0 0 16 0] /Function << /FunctionType 2 /Domain [0 1] /C0 [1 0 0] /C1 [1 0 0] /N 1 >> /Extend [true true] >> >>"
        ink = [rectangle(F(0), F(0), F(SIZE), F(SIZE))]
    else:
        paint_type = b"2" if kind == "uncolored" else b"1"
        content = b"0 0 1.5 4 re f" if kind == "uncolored" else b"1 0 0 rg 0 0 1.5 4 re f"
        pattern = stream(content, b"/Type /Pattern /PatternType 1 /PaintType " + paint_type
                         + b" /TilingType 2 /BBox [0 0 4 4] /XStep 4 /YStep 4 /Resources << >> /Matrix [1 0 0 1 "
                         + number(ox) + b" " + number(oy) + b"]")
        ink = [rectangle(F(4*x)+ox, F(4*y)+oy, F(4*x)+ox+F(3, 2), F(4*y)+oy+4)
               for y in range(-1, 5) for x in range(-1, 5)]
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
    return Case(name, data, UNSUPPORTED if interpolate else RENDER, ("paint-image-masks",),
                "Exact geometric patterned stencil; independent rational joint-area oracle",
                oracle=None if interpolate else oracle(masks, ink, clip, color, alpha * (F(1, 2) if soft else 1)),
                feature="pattern.image-mask" if interpolate else None,
                extra={} if interpolate else {"tolerance": 1})


def different_masks():
    """The same tiling paint uses two different image sources and placements."""
    first = (F(6), F(0), F(0), F(4), F(1), F(1))
    second = (F(6), F(0), F(0), F(4), F(8), F(9))
    opposite = tuple(tuple(1-code for code in row) for row in ROWS)
    masks = mask_polygons(first) + mask_polygons(second, rows=opposite)
    pattern = stream(b"1 0 0 rg 0 0 1.5 4 re f", b"/Type /Pattern /PatternType 1 /PaintType 1 /TilingType 2 /BBox [0 0 4 4] /XStep 4 /YStep 4 /Resources << >>")
    def image(rows):
        packed = bytes(sum(value << (7-x) for x, value in enumerate(row)) for row in rows)
        return stream(packed, b"/Type /XObject /Subtype /Image /Width 3 /Height 2 /ImageMask true /BitsPerComponent 1")
    content = b"/Pattern cs /P scn q " + b" ".join(number(v) for v in first) + b" cm /I Do Q q " + b" ".join(number(v) for v in second) + b" cm /J Do Q"
    resources = b"<< /Pattern << /P 5 0 R >> /XObject << /I 6 0 R /J 7 0 R >> >>"
    ink = [rectangle(F(4*x), F(4*y), F(4*x)+F(3, 2), F(4*y)+4) for y in range(4) for x in range(4)]
    return Case("colored-different-masks", page(content, [pattern, image(ROWS), image(opposite)], resources),
                RENDER, ("paint-image-masks",), "Separate geometric clipping identities for two masks sharing one pattern",
                oracle=oracle(masks, ink, None, RED), extra={"tolerance": 1})


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
        # Strict-valid old-engine equivalent for controlled timing: integer
        # cell/ink edges make coverage products coincide with joint areas.
        yield control(kind+"-matched-stencil", kind, aligned, origin=(F(0), F(0)))
        yield control(kind+"-matched-path", kind, aligned, twin=True, origin=(F(0), F(0)))
    yield control("colored-empty", "colored", aligned, rows=((1, 1, 1), (1, 1, 1)))
    yield control("colored-alpha", "colored", aligned, alpha=F(1, 2))
    yield control("colored-nested-soft-mask", "colored", aligned, nested=True, soft=True)
    yield different_masks()
