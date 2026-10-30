"""Nested tiling stencils, independently intersected rational regions.

ISO 32000-2 8.7.3 and 8.9.6.2; composition follows 11.3.3/11.6.7.
All pattern cells here are 16-unit, non-overlapping cells. The marks of each
nested cell retain the complete outer geometry; no already averaged stencil
coverage is multiplied back into a cell result. No engine supplies an oracle.
"""
from fractions import Fraction as F
import struct

from .font import simple, truetype
from .paint_curved_stencils import original_quadratic_oracle
from .model import Case, RENDER
from .paint_stencils import (RED, ROWS, SIZE, mask_polygons, number, page,
                             polygon, rectangle)
from .pdf import stream
from .raster import winding_area

AREA = "paint_nested_stencils"
COMMON = b"/Type /Pattern /PatternType 1 /TilingType 2 /BBox [0 0 16 16] /XStep 16 /YStep 16 "
MASK_MATRIX = (F(21, 2), F(1), F(-1), F(19, 2), F(13, 4), F(9, 4))


def oracle(groups, marks, alpha=F(1)):
    pixels = bytearray()
    for y in reversed(range(SIZE)):
        for x in range(SIZE):
            color = [F(1), F(1), F(1)]
            for shape, ink in marks:
                area = winding_area(groups + [("nonzero", [shape])], x, y)
                for c, value in enumerate(ink):
                    color[c] = color[c]*(1-area) + F(value, 255)*area
            pixels.extend(int(255*((1-alpha)+alpha*c)+F(1, 2)) for c in color)
    return b"P6\n16 16\n255\n" + pixels


def nested(name, *, depth=1, matrix=MASK_MATRIX, clip=None, evenodd=False,
           alpha=F(1), group=False, soft=False, stroke=False, hairline=False,
           uncolored=False, multi=False, flat=False, pen_transform=False):
    masks = mask_polygons(matrix)
    # All ordinary outer marks are clipped in pattern space. Alternating
    # rectangles make a missing intermediate ancestor geometrically visible.
    limits = [rectangle(F(9, 4), F(7, 4), F(55, 4), F(57, 4)),
              rectangle(F(7, 4), F(13, 4), F(53, 4), F(55, 4)),
              rectangle(F(5, 4), F(5, 4), F(51, 4), F(53, 4))]
    regions = [("nonzero", masks)]
    if clip:
        regions.append(("nonzero", [clip]))
    # 5: outer pattern, 6: stencil, 7: next ordinary pattern; further
    # ancestors and the leaf follow. Object numbers are assigned without
    # importing a parser or renderer.
    patterns = []
    flat_clips = []
    for level in range(depth):
        shape = limits[level % len(limits)]
        rule = b"f"
        clip_rule = b"W n "
        clip_path = polygon(shape)
        if level == 0 and evenodd:
            hole = rectangle(F(25, 4), F(23, 4), F(37, 4), F(35, 4))
            draw = polygon(shape) + polygon(hole)
            regions.append(("evenodd", [shape, hole]))
            rule = b"f*"
            clip_rule = b"W* n "
            clip_path = draw
        elif level == 0 and stroke:
            width = F(1) if hairline else F(3, 4) if pen_transform else F(3, 2)
            shape = rectangle(F(3), F(29, 4)-width/2, F(13), F(29, 4)+width/2)
            draw = (b"0 w " if hairline else b"1.5 w ") + b"3 7.25 m 13 7.25 l "
            if pen_transform:
                draw = b"2 0 0 .5 0 0 cm 1.5 w 1.5 14.5 m 6.5 14.5 l "
            regions.append(("nonzero", [shape]))
            rule = b"S"
            clip_path = polygon(shape)
        else:
            draw = polygon(shape)
            regions.append(("nonzero", [shape]))
        flat_clips.append(clip_path+clip_rule)
        target = 7 + level
        selected = b"/CS cs 0 1 0 /Q scn " if uncolored and level == depth-1 else b"/Pattern cs /Q scn "
        if level == 0 and stroke:
            selected = b"/Pattern CS /Q SCN "
        content = selected + draw + rule
        if level == 0 and group:
            # The first pattern calls an isolated form that performs the
            # ordinary patterned mark, preserving the same geometry.
            content = b"/F Do"
        resources = b"/Resources << /Pattern << /Q " + str(target).encode() + b" 0 R >> /ColorSpace << /CS [/Pattern /DeviceRGB] >>"
        if level == 0 and group:
            resources += b" /XObject << /F " + str(7+depth).encode() + b" 0 R >>"
        resources += b" >>"
        patterns.append(stream(content, COMMON+b"/PaintType 1 "+resources))
    leaf_marks = [(rectangle(F(0), F(0), F(16), F(16)), (0,255,0) if uncolored else RED)]
    leaf_content = b"0 0 16 16 re f" if uncolored else b"1 0 0 rg 0 0 16 16 re f"
    if multi:
        blue = rectangle(F(29, 4), F(19, 4), F(49, 4), F(43, 4))
        leaf_marks.append((blue, (0,0,255)))
        leaf_content += b" 0 0 1 rg " + polygon(blue) + b"f"
    if flat:
        patterns[0] = stream(b"".join(flat_clips)+leaf_content,COMMON+b"/PaintType 1 /Resources << >>")
    patterns.append(stream(leaf_content, COMMON+(b"/PaintType 2 " if uncolored else b"/PaintType 1 ")+b"/Resources << >>"))
    packed = bytes(sum(value << (7-x) for x,value in enumerate(row)) for row in ROWS)
    image = stream(packed,b"/Type /XObject /Subtype /Image /Width 3 /Height 2 /ImageMask true /BitsPerComponent 1")
    objects = [patterns[0], image, *patterns[1:]]
    if group:
        shape = limits[0]
        selected = b"/CS cs 0 1 0 /Q scn " if uncolored and depth == 1 else b"/Pattern cs /Q scn "
        content = selected + polygon(shape) + b"f"
        objects.append(stream(content,b"/Type /XObject /Subtype /Form /BBox [0 0 16 16] /Group << /S /Transparency /I true /CS /DeviceRGB >> /Resources << /Pattern << /Q 7 0 R >> /ColorSpace << /CS [/Pattern /DeviceRGB] >> >>"))
    resources = b"<< /Pattern << /P 5 0 R >> /XObject << /I 6 0 R >> /ExtGState << /G << /ca " + number(alpha)
    if soft:
        soft_id = 5 + len(objects)
        objects.append(stream(b"/H gs 0 0 16 16 re f",b"/Type /XObject /Subtype /Form /BBox [0 0 16 16] /Group << /S /Transparency /I true /CS /DeviceRGB >> /Resources << /ExtGState << /H << /ca .5 >> >> >>"))
        resources += b" /SMask << /S /Alpha /G " + str(soft_id).encode() + b" 0 R >>"
    resources += b" >> >> >>"
    content = (polygon(clip)+b"W n " if clip else b"") + b"/G gs /Pattern cs /P scn " + b" ".join(number(v) for v in matrix) + b" cm /I Do"
    return Case(name,page(content,objects,resources),RENDER,("paint-image-masks",),
                "Joint inherited stencil, ancestor fill/stroke and clip geometry through ordinary tiling",
                oracle=oracle(regions,leaf_marks,alpha*(F(1,2) if soft else 1)),extra={"tolerance":1})


def glyph_case(name, curve):
    # Preserve the original source and IDs. Its former refusal is now a
    # positive true-quadratic analytic oracle, never a flattened replacement.
    points = ([(0,0,True),(0,300,False),(300,0,True)] if curve else
              [(0,0,True),(300,0,True),(300,300,True),(0,300,True)])
    program = truetype(glyph_override=simple(points), table_overrides={b"hmtx":struct.pack(">HhHh",500,0,600,0)+bytes(6)})
    pattern = stream(b"/Pattern cs /Q scn BT /F 30 Tf 2 2 Td (A) Tj ET", COMMON+b"/PaintType 1 /Resources << /Pattern << /Q 7 0 R >> /Font << /F 8 0 R >> >>")
    image = stream(b"\0",b"/Type /XObject /Subtype /Image /Width 1 /Height 1 /ImageMask true /BitsPerComponent 1")
    solid = stream(b"1 0 0 rg 0 0 16 16 re f", COMMON+b"/PaintType 1 /Resources << >>")
    font = b"<< /Type /Font /Subtype /TrueType /BaseFont /Quad /FirstChar 65 /LastChar 65 /Widths [600] /Encoding /WinAnsiEncoding /FontDescriptor 9 0 R >>"
    descriptor = b"<< /Type /FontDescriptor /FontName /Quad /Flags 32 /FontBBox [0 0 300 300] /ItalicAngle 0 /Ascent 300 /Descent 0 /CapHeight 300 /StemV 50 /FontFile2 10 0 R >>"
    content = b"/Pattern cs /P scn 10.5 0 0 10.5 2.25 2.25 cm /I Do"
    resources = b"<< /Pattern << /P 5 0 R >> /XObject << /I 6 0 R >> >>"
    return Case(name,page(content,[pattern,image,solid,font,descriptor,stream(program,b"/Length1 "+str(len(program)).encode())],resources),
                RENDER,("paint-image-masks",),
                "Curved patterned glyphs retain true quadratics; linear glyph outlines propagate their exact geometry",
                oracle=original_quadratic_oracle() if curve else oracle([("nonzero",[rectangle(F(9,4),F(9,4),F(51,4),F(51,4))])],[(rectangle(F(2),F(2),F(11),F(11)),RED)]),
                extra={"tolerance":1})


def tiny_quadratic_proof(patterned):
    """101 exact quadratic slivers, each with rational area 1/1000 pixel.

    With a one-pixel em, the control point is .003 pixel above the
    baseline, below the generic path-flattener tolerance 1/256. Dropping
    those tiny curves into their chords would lose all .101 pixel of ink.
    True quadratic glyph integration retains the exact area instead.
    """
    points = [(x,y+9*i,on) for i in range(101)
              for x,y,on in [(0,0,True),(500,3,False),(1000,0,True)]]
    glyph = simple(points,contours=[3*i+2 for i in range(101)])
    glyph = glyph[:2]+struct.pack(">hhhh",0,0,1000,1000)+glyph[10:]
    program = truetype(glyph_override=glyph,table_overrides={b"hmtx":struct.pack(">HhHh",500,0,600,0)+bytes(6)})
    selection = b"/Pattern cs /Q scn " if patterned else b"1 0 0 rg "
    pattern = stream(selection+b"BT /F 1 Tf 4 4 Td (A) Tj ET", COMMON+b"/PaintType 1 /Resources << /Pattern << /Q 7 0 R >> /Font << /F 8 0 R >> >>")
    mask = stream(b"\0",b"/Type /XObject /Subtype /Image /Width 1 /Height 1 /ImageMask true /BitsPerComponent 1")
    solid = stream(b"1 0 0 rg 0 0 16 16 re f", COMMON+b"/PaintType 1 /Resources << >>")
    font = b"<< /Type /Font /Subtype /TrueType /BaseFont /Slivers /FirstChar 65 /LastChar 65 /Widths [600] /Encoding /WinAnsiEncoding /FontDescriptor 9 0 R >>"
    descriptor = b"<< /Type /FontDescriptor /FontName /Slivers /Flags 32 /FontBBox [0 0 1000 1000] /ItalicAngle 0 /Ascent 1000 /Descent 0 /CapHeight 1000 /StemV 50 /FontFile2 10 0 R >>"
    content = b"/Pattern cs /P scn 10.5 0 0 10.5 2.25 2.25 cm /I Do"
    resources = b"<< /Pattern << /P 5 0 R >> /XObject << /I 6 0 R >> >>"
    pixels = bytearray(bytes([255,255,255])*256)
    # PDF y=4 maps to top-down device row 11 at 72 DPI. Correctly rounded
    # red over white at exact coverage 101/1000 is [255,229,229].
    at = (11*16+4)*3
    pixels[at:at+3] = bytes([255,229,229])
    return Case("glyph-tiny-quadratics-refused" if patterned else "glyph-tiny-quadratics-control",
                page(content,[pattern,mask,solid,font,descriptor,stream(program,b"/Length1 "+str(len(program)).encode())],resources),
                RENDER,("paint-image-masks",),
                "101 sub-flattener-tolerance quadratic glyph slivers have exact total area 101/1000 pixel; preserve true curves",
                oracle=b"P6\n16 16\n255\n"+pixels,
                extra={"tolerance":1})


def cases():
    clip = [(F(1),F(1)),(F(15),F(4)),(F(7),F(15))]
    yield nested("fill-one-level")
    yield nested("fill-three-levels",depth=3)
    yield nested("fill-eight-levels",depth=8)
    yield nested("fill-one-level-flat",flat=True)
    yield nested("fill-three-levels-flat",depth=3,flat=True)
    yield nested("fill-eight-levels-flat",depth=8,flat=True)
    yield nested("fill-triangle-clip",depth=2,clip=clip)
    yield nested("fill-evenodd",depth=2,evenodd=True)
    yield nested("fill-alpha",depth=2,alpha=F(1,2))
    yield nested("fill-isolated-group",depth=2,group=True)
    yield nested("fill-soft-mask",depth=2,soft=True)
    yield nested("fill-uncolored-leaf",depth=2,uncolored=True)
    yield nested("fill-source-order",depth=2,multi=True)
    yield nested("fill-source-order-alpha",depth=2,multi=True,alpha=F(1,2))
    yield nested("stroke-butt",stroke=True)
    yield nested("stroke-hairline",stroke=True,hairline=True)
    yield nested("stroke-butt-flat",stroke=True,flat=True)
    yield nested("stroke-nonuniform-pen",stroke=True,pen_transform=True)
    yield glyph_case("glyph-linear",False)
    yield glyph_case("glyph-quadratic-refused",True)
    yield tiny_quadratic_proof(False)
    yield tiny_quadratic_proof(True)
