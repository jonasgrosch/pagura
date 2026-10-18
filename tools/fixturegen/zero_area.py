"""Independent empty-fill boundaries, ISO 32000-2 10.7.4 and 8.4.3.2.

One-device-pixel butt hairlines are geometric rectangles. Coverage is the
rational intersection of those rectangles with pixel squares, independent of
any rasterizer. All cases use black on white at 72 dpi on an 8x8 page.
"""
from fractions import Fraction as F

from .model import Case, RENDER
from .pdf import document, single_page, stream
from .raster import clipped_area

AREA = "zero_area"
REQUIREMENT = ("raster-zero-area-fill",)


def oracle(polygons, opacity=F(1)):
    pixels = bytearray()
    for row in range(8):
        for x in range(8):
            # Polygons in these cases have disjoint interiors.
            area = sum(clipped_area(polygon, x, 7 - row) for polygon in polygons)
            coverage = int(area * 255 + F(1, 2))
            pixels.extend(bytes([int(255 - coverage * opacity + F(1, 2))]) * 3)
    return b"P6\n8 8\n255\n" + pixels


def rectangle(x0, y0, x1, y1):
    return [(x0, y0), (x1, y0), (x1, y1), (x0, y1)]


def case(name, content, polygons=(), tolerance=0, resources=b"<< >>", opacity=F(1)):
    return Case(name, single_page([content], media_box=b"[0 0 8 8]", resources=resources),
                RENDER, REQUIREMENT, "Empty fill boundary: " + name,
                oracle=oracle(polygons, opacity), extra={"tolerance": tolerance})


def cases():
    vertical = [rectangle(3, 1, 4, 7)]
    horizontal = [rectangle(1, 3, 7, 4)]
    yield case("zero-width-rectangle", b"3.5 1 0 6 re f", vertical)
    yield case("zero-width-rectangle-reversed", b"3.5 7 0 -6 re f", vertical)
    yield case("zero-height-rectangle", b"1 3.5 6 0 re f", horizontal)
    yield case("zero-height-rectangle-reversed", b"7 3.5 -6 0 re f", horizontal)
    yield case("integral-boundary", b"3 1 0 6 re f", [rectangle(F(5, 2), 1, F(7, 2), 7)])
    yield case("collinear-triangle", b"3.5 1 m 3.5 4 l 3.5 7 l h f", vertical)
    # A 3-4-5 segment has rational unit normal (-4/5, 3/5).
    diagonal = [[(F(3, 5), F(13, 10)), (F(18, 5), F(53, 10)),
                 (F(22, 5), F(47, 10)), (F(7, 5), F(7, 10))]]
    yield case("oblique-collinear-triangle", b"1 1 m 2.5 3 l 4 5 l h f", diagonal, 1)
    yield case("open-segment-evenodd", b"1 3.5 m 7 3.5 l f*", horizontal)
    yield case("collinear-cubic", b"3.5 1 m 3.5 3 3.5 5 3.5 7 c f", vertical)
    yield case("singular-transform", b"0 0 0 1 3.5 0 cm 1 1 6 6 re f", vertical)
    yield case("multiple-subpaths", b"2.5 1 0 6 re 5.5 1 0 6 re f*",
               [rectangle(2, 1, 3, 7), rectangle(5, 1, 6, 7)])
    yield case("point-and-segment", b"3.5 1 0 6 re 6.25 2.25 m h f",
               vertical + [rectangle(6, 2, 7, 3)])
    yield case("point-pair-evenodd", b"3.5 1 0 6 re 6.25 2.25 m h 6.75 2.75 m h f*", vertical)
    yield case("point-pair-nonzero", b"3.5 1 0 6 re 6.25 2.25 m h 6.75 2.75 m h f",
               vertical + [rectangle(6, 2, 7, 3)])
    yield case("trailing-move", b"3.5 1 0 6 re 6.25 2.25 m f", vertical)
    # Graphics-state stroke width, caps, joins and dash do not affect a fill.
    yield case("stroke-state-independent", b"5 w 2 J 2 j [1 3] 2 d 3.5 1 0 6 re f", vertical)
    yield case("degenerate-clip", b"3.5 1 0 6 re W n 0 0 8 8 re f", vertical)
    yield case("degenerate-clip-evenodd", b"1 3.5 6 0 re W* n 0 0 8 8 re f", horizontal)
    yield case("paint-before-clip", b"3.5 1 0 6 re W f 0 0 8 8 re f", vertical)
    yield case("hairline-clipped-geometrically", b"3.5 0 4.5 8 re W n 3.5 1 0 6 re f",
               [rectangle(F(7, 2), 1, 4, 7)])
    # Retracing a box cancels its interior under the selected fill rule.
    # Mitered hairline union = outer box minus inner box, expressed as strips.
    ring = [rectangle(F(3, 2), F(3, 2), F(13, 2), F(5, 2)),
            rectangle(F(3, 2), F(11, 2), F(13, 2), F(13, 2)),
            rectangle(F(3, 2), F(5, 2), F(5, 2), F(11, 2)),
            rectangle(F(11, 2), F(5, 2), F(13, 2), F(11, 2))]
    yield case("cancelled-evenodd", b"2 2 4 4 re 2 2 4 4 re f*", ring)
    yield case("cancelled-nonzero-subdivided",
               b"2 2 4 4 re 2 2 m 2 4 l 2 6 l 6 6 l 6 2 l h f", ring)
    # Positive area must remain ordinary exact-area coverage even below a byte.
    yield case("positive-sliver", b"3.5 1 .000001 6 re f")
    yield case("tiny-nonpoint-curve", b"4 4 m 4.001 4 4 4.001 4 4 c h f")
    yield case("positive-bow-tie", b"1 1 m 7 7 l 1 7 l 7 1 l h f",
               [[(1, 1), (7, 1), (4, 4)], [(1, 7), (7, 7), (4, 4)]])
    # Force the general transparency compositor as well as the opaque path.
    yield case("transparent-compositor", b"/G gs 3.5 1 0 6 re f", vertical,
               resources=b"<< /ExtGState << /G << /BM /Multiply >> >> >>")
    yield case("half-alpha", b"/G gs 3.5 1 0 6 re f", vertical,
               resources=b"<< /ExtGState << /G << /ca .5 >> >> >>", opacity=F(1, 2))

    yield case("zero-area-clip-content", b"3 1 0 6 re W n /G gs 3 2 4 4 re f",
               [rectangle(3, 2, F(7, 2), 6)],
               resources=b"<< /ExtGState << /G << /BM /Multiply >> >> >>")
    # Table 74: even a zero-size pattern BBox paints a device pixel. A cell's
    # clipping region follows exactly the same scan conversion as a fill.
    for name, bbox, polygons in [
        ("zero-width-pattern-bbox", b"[3 1 3 7]", [rectangle(F(5, 2), 1, F(7, 2), 7)]),
        ("zero-size-pattern-bbox", b"[3.25 2.25 3.25 2.25]", [rectangle(3, 2, 4, 3)]),
    ]:
        pdf = document([
            b"<< /Type /Catalog /Pages 2 0 R >>",
            b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 8 8] /Resources << /Pattern << /P 5 0 R >> >> /Contents 4 0 R >>",
            stream(b"/Pattern cs /P scn 0 0 8 8 re f"),
            stream(b"0 0 8 8 re f", b"/Type /Pattern /PatternType 1 /PaintType 1 /TilingType 1 /BBox "
                   + bbox + b" /XStep 16 /YStep 16 /Resources << >>"),
        ])
        yield Case(name, pdf, RENDER, REQUIREMENT, "Empty cell BBox: " + name,
                   oracle=oracle(polygons), extra={"tolerance": 0})
    shading = (b"<< /ShadingType 2 /ColorSpace /DeviceGray /Coords [0 0 8 0] "
               b"/BBox [3 1 3 7] /Extend [true true] /Function "
               b"<< /FunctionType 2 /Domain [0 1] /C0 [0] /C1 [0] /N 1 >> >>")
    half_vertical = [rectangle(F(5, 2), 1, F(7, 2), 7)]
    yield case("zero-width-shading-bbox", b"/S sh", half_vertical,
               resources=b"<< /Shading << /S " + shading + b" >> >>")
    yield case("zero-width-shading-pattern-bbox", b"/Pattern cs /P scn 0 0 8 8 re f",
               half_vertical, resources=b"<< /Pattern << /P << /Type /Pattern /PatternType 2 /Shading "
               + shading + b" >> >> >>")

    yield case("zero-width-shading-pattern-background", b"/Pattern cs /P scn 0 0 8 8 re f",
               half_vertical, resources=b"<< /Pattern << /P << /Type /Pattern /PatternType 2 /Shading "
               + shading[:-2] + b" /Background [1] >> >> >> >>")

    point_shading = shading.replace(b"[3 1 3 7]", b"[3.25 2.25 3.25 2.25]")
    point_cell = [rectangle(3, 2, 4, 3)]
    yield case("zero-size-shading-bbox", b"/S sh", point_cell,
               resources=b"<< /Shading << /S " + point_shading + b" >> >>")
    yield case("zero-size-shading-pattern-bbox", b"/Pattern cs /P scn 0 0 8 8 re f",
               point_cell, resources=b"<< /Pattern << /P << /Type /Pattern /PatternType 2 /Shading "
               + point_shading + b" >> >> >>")
    yield case("zero-size-shading-pattern-background", b"/Pattern cs /P scn 0 0 8 8 re f",
               point_cell, resources=b"<< /Pattern << /P << /Type /Pattern /PatternType 2 /Shading "
               + point_shading[:-2] + b" /Background [1] >> >> >> >>")
