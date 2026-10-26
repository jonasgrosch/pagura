"""Independent text and image painting oracles (ISO 32000-2 clauses 8.9 and 9).

Rectangle glyphs use analytic rectangles; triangle glyphs use rational polygon
intersection. Codec payloads come from the independent format encoders. Affine
image oracles explicitly map source sample rectangles into page coordinates.
No product executable is imported or run.
"""
from fractions import Fraction as F
import math
import struct
import zlib
from .model import Case, RENDER, OPEN, UNSUPPORTED
from .pdf import document, stream, opaque_rectangles
from .font import truetype, simple
from .raster import clipped_area
from . import image as codecs, jpx, jbig2

AREA = "paint"
SIZE = 32
RED, BLUE, GREEN, BLACK, WHITE = (255,0,0),(0,0,255),(0,255,0),(0,0,0),(255,255,255)


def page(content, objects=(), resources=b"<< >>", size=SIZE):
    return document([
        b"<< /Type /Catalog /Pages 2 0 R >>",
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 %d %d] /Resources " % (size,size) + resources + b" /Contents 4 0 R >>",
        stream(content), *objects,
    ])


def rects(values, size=SIZE):
    return opaque_rectangles(size,size,values)


def case(name, content, objects=(), resources=b"<< >>", rectangles=(), *, requirement="paint-text-state", oracle=None, tolerance=0, size=SIZE):
    return Case(name,page(content,objects,resources,size),RENDER,(requirement,),name.replace("-"," "),
                oracle=oracle if oracle is not None else rects(rectangles,size),extra={"tolerance":tolerance})


def ttf(triangle=False):
    points = [(0,0,True),(400,0,True),(0,600,True)] if triangle else [(0,0,True),(400,0,True),(400,600,True),(0,600,True)]
    glyph=simple(points)
    glyph=glyph[:2]+struct.pack(">hhhh",0,0,400,600)+glyph[10:]
    # Match the embedded horizontal advance to the PDF Widths array (9.2.4).
    metrics=struct.pack(">HhHh",500,0,400,0)+bytes(6)
    program = truetype(glyph_override=glyph,table_overrides={b"hmtx":metrics})
    font = b"<< /Type /Font /Subtype /TrueType /BaseFont /Rect /FirstChar 65 /LastChar 65 /Widths [400] /Encoding /WinAnsiEncoding /FontDescriptor 6 0 R >>"
    descriptor = b"<< /Type /FontDescriptor /FontName /Rect /Flags 32 /FontBBox [0 0 400 600] /ItalicAngle 0 /Ascent 600 /Descent 0 /CapHeight 600 /StemV 50 /FontFile2 7 0 R >>"
    # FontFile2 records the decoded program length (Table 125).
    return [font,descriptor,stream(program,b"/Length1 %d" % len(program))]

FONT = b"<< /Font << /F 5 0 R >> >>"


def type3(procedure=b"400 0 d0 0 0 400 600 re f", extra=b"", resources=b"<< >>"):
    widths = [0]*34
    widths[0] = 200
    widths[33] = 400
    font = (b"<< /Type /Font /Subtype /Type3 /FontBBox [0 0 400 600] /FontMatrix [.001 0 0 .001 0 0] /FirstChar 32 /LastChar 65 /Widths [" + b" ".join(str(w).encode() for w in widths)
            + b"] /Encoding << /Differences [32 /space 65 /A] >> /CharProcs << /A 6 0 R >> /Resources " + resources + b" " + extra + b" >>")
    return [font,stream(procedure)]


def mode_cases():
    for mode in range(8):
        marks=[]
        if mode%4 in (0,2): marks.append(((4,4,8,12),RED))
        if mode%4 in (1,2):
            marks.extend([((3,3,10,2),BLUE),((3,15,10,2),BLUE),((3,5,2,10),BLUE),((11,5,2,10),BLUE)])
        marks.append(((8,4,4,12),GREEN) if mode>=4 else ((8,0,24,32),GREEN))
        yield case(f"text-mode-{mode}", b"1 0 0 rg 0 0 1 RG 2 w BT /F 20 Tf %d Tr 1 0 0 1 4 4 Tm (A) Tj ET 0 1 0 rg 8 0 24 32 re f"%mode,
                   ttf(),FONT,marks,requirement="paint-text-modes")
        marks=[] if mode in (3,7) else [((4,4,8,12),RED)]
        marks.append(((8,0,24,32),GREEN))
        yield case(f"type3-mode-{mode}",b"1 0 0 rg 0 0 1 RG 2 w BT /F 20 Tf %d Tr 1 0 0 1 4 4 Tm (A) Tj ET 0 1 0 rg 8 0 24 32 re f"%mode,
                   type3(),FONT,marks,requirement="paint-type3")
    points=[(F(17,4),F(31,4)),(F(49,4),F(31,4)),(F(17,4),F(79,4))]
    oracle=b"P6\n32 32\n255\n"+bytes(255-int(clipped_area(points,x,31-y)*255+F(1,2)) for y in range(32) for x in range(32) for _ in range(3))
    yield case("text-triangle-area",b"BT /F 20 Tf 1 0 0 1 4.25 7.75 Tm (A) Tj ET",ttf(True),FONT,
               oracle=oracle,tolerance=1,requirement="paint-text-outlines")


def text_cases():
    yield from mode_cases()
    points=[(F(17,4),F(31,4)),(F(49,4),F(31,4)),(F(17,4),F(79,4))]
    pixels=[]
    for y in range(32):
        for x in range(32):
            shape=F(int(clipped_area(points,x,31-y)*255+F(1,2)),255)
            remainder=int(255*(1-shape)**2+F(1,2))
            pixels.extend([255,remainder,remainder])
    yield case("text-knockout-opaque-overlap",b"1 0 0 rg BT /F 20 Tf 4.25 7.75 Td [(A) 400 (A)] TJ ET",ttf(True),FONT,
               oracle=b"P6\n32 32\n255\n"+bytes(pixels),tolerance=1,requirement="paint-text-knockout")

    yield case("text-tj-adjustments",b"BT /F 10 Tf 1 0 0 1 4 4 Tm [(A) -200 (A) 100 (A)] TJ ET",ttf(),FONT,
               [((4,4,4,6),BLACK),((10,4,4,6),BLACK),((13,4,4,6),BLACK)])
    yield case("text-spacing-rise",b"BT /F 10 Tf 200 Tz 1 Tc 3 Tw 2 Ts 1 0 0 1 2 2 Tm (A A) Tj ET",type3(),FONT,
               [((2,4,8,6),BLACK),((24,4,8,6),BLACK)])
    yield case("text-positioning-quotes",b"BT /F 10 Tf 1 0 0 1 2 24 Tm (A) Tj 8 TL (A) ' 2 1 (A A) \" 12 -4 TD (A) Tj T* (A) Tj ET",type3(),FONT,
               [((2,24,4,6),BLACK),((2,16,4,6),BLACK),((2,8,4,6),BLACK),((12,8,4,6),BLACK),((14,4,4,6),BLACK),((14,0,4,6),BLACK)])
    yield case("text-matrix-q",b"BT /F 10 Tf 1 0 0 1 2 2 Tm q 2 Tc (A) Tj Q (A) Tj ET",ttf(),FONT,
               [((2,2,4,6),BLACK),((8,2,4,6),BLACK)])
    yield case("text-state-persists",b"/F 10 Tf 2 Tc BT 2 2 Td (A) Tj ET BT 2 12 Td (A) Tj ET",ttf(),FONT,
               [((2,2,4,6),BLACK),((2,12,4,6),BLACK)])
    yield case("text-negative-size",b"BT /F -10 Tf 1 0 0 1 12 12 Tm (AA) Tj ET",ttf(),FONT,
               [((8,6,4,6),BLACK),((4,6,4,6),BLACK)])
    yield case("text-zero-size",b"BT /F 0 Tf 7 Tr (A) Tj ET 0 0 32 32 re f",ttf(),FONT,[((0,0,32,32),BLACK)])
    yield case("text-empty-clip",b"BT /F 10 Tf 7 Tr () Tj ET 0 0 32 32 re f",ttf(),FONT,[((0,0,32,32),BLACK)],requirement="paint-text-modes")
    yield case("text-clip-union",b"BT /F 10 Tf 7 Tr 2 2 Td (A) Tj 10 0 Td (A) Tj ET 0 0 32 32 re f",ttf(),FONT,
               [((2,2,4,6),BLACK),((12,2,4,6),BLACK)],requirement="paint-text-modes")
    yield case("text-clip-deferred",b"1 0 0 rg BT /F 10 Tf 4 Tr 2 2 Td (A) Tj 10 0 Td (A) Tj ET 0 0 1 rg 12 2 4 6 re f",ttf(),FONT,
               [((2,2,4,6),RED),((12,2,4,6),BLUE)],requirement="paint-text-modes")
    yield case("text-clip-restored",b"q BT /F 10 Tf 7 Tr 2 2 Td (A) Tj ET Q 0 0 32 32 re f",ttf(),FONT,[((0,0,32,32),BLACK)],requirement="paint-text-modes")
    # W2 position vector translates the horizontal origin before Tfs/Th.
    program=ttf()[2]
    objects=[b"<< /Type /Font /Subtype /Type0 /BaseFont /Rect /Encoding /Identity-V /DescendantFonts [6 0 R] >>",
             b"<< /Type /Font /Subtype /CIDFontType2 /BaseFont /Rect /CIDSystemInfo << /Registry (X) /Ordering (Y) /Supplement 0 >> /FontDescriptor 7 0 R /CIDToGIDMap /Identity /DW 400 /W2 [1 [-1000 200 800]] >>",
             ttf()[1].replace(b"7 0 R",b"8 0 R"),program]
    yield case("text-vertical",b"BT /F 10 Tf 200 Tz 1 Tc 2 Ts 1 0 0 1 12 26 Tm [<0001> 100 <0001>] TJ ET",objects,FONT,
               [((8,20,8,6),BLACK),((8,10,8,6),BLACK)])
    for knockout in (True,False):
        # Two half-opaque identical glyphs: TK replaces the prior contribution.
        color=(255,128,128) if knockout else (255,64,64)
        resources=b"<< /Font << /F 5 0 R >> /ExtGState << /G << /ca .5 /TK "+str(knockout).lower().encode()+b" >> >> >>"
        yield case(f"text-knockout-{str(knockout).lower()}",b"/G gs 1 0 0 rg BT /F 20 Tf 4 4 Td [(A) 400 (A)] TJ ET",ttf(),resources,
                   [((4,4,8,12),color)],requirement="paint-text-knockout")
    resources=b"<< /ExtGState << /G << /Font [5 0 R 20] >> >> >>"
    yield case("text-extgstate-font",b"/G gs BT 4 4 Td (A) Tj ET",ttf(),resources,[((4,4,8,12),BLACK)])
    yield case("type3-d0-colour",b"1 0 0 rg BT /F 20 Tf 4 4 Td (A) Tj ET 20 4 4 12 re f",type3(b"400 0 d0 0 0 1 rg 0 0 400 600 re f"),FONT,
               [((4,4,8,12),BLUE),((20,4,4,12),RED)],requirement="paint-type3")
    yield case("type3-d1-colour-ignored",b"1 0 0 rg BT /F 20 Tf 4 4 Td (A) Tj ET",type3(b"400 0 0 0 400 600 d1 0 0 1 rg 0 0 400 600 re f"),FONT,
               [((4,4,8,12),RED)],requirement="paint-type3")
    for name,content,objects in [
        ("text-show-outside",b"(A) Tj",[]),
        ("text-nested",b"BT BT ET ET",[]),
        ("text-end-without-begin",b"ET",[]),
        ("text-unfinished",b"BT",[]),
        ("text-mode-invalid",b"8 Tr",[]),
        ("text-q-crosses",b"q BT Q ET",[]),
        ("text-et-crosses",b"BT q ET Q",[]),
        ("text-no-font",b"BT (A) Tj ET",[]),
        ("type3-missing-metrics",b"BT /F 20 Tf (A) Tj ET",type3(b"0 0 400 600 re f")),
        ("type3-metrics-repeated",b"BT /F 20 Tf (A) Tj ET",type3(b"400 0 d0 400 0 d0")),
        ("type3-metrics-outside",b"400 0 d0",[]),
    ]:
        yield Case(name,page(content,objects,FONT if objects else b"<< >>"),OPEN,("paint-text-errors",),name.replace("-"," "),extra={"render_error":"syntax"})


def image_case(name, data, dictionary, samples, width, height, *, matrix=None, masks=(), resources=b"", requirement="paint-images", tolerance=0, fill=b"", colors=None, alpha=None):
    if matrix is None: matrix=(width*2,0,0,height*2,2,2)
    if colors is None: colors=[(v,v,v) for v in samples]
    rectangles=[]
    a,b,c,d,e,f=matrix
    for row in range(height):
        for col in range(width):
            points=[(e+a*u+c*v,f+b*u+d*v) for u,v in [(F(col,width),1-F(row,height)),(F(col+1,width),1-F(row,height)),(F(col+1,width),1-F(row+1,height)),(F(col,width),1-F(row+1,height))]]
            x0,y0=min(x for x,y in points),min(y for x,y in points)
            x1,y1=max(x for x,y in points),max(y for x,y in points)
            opacity=1 if alpha is None else alpha[row*width+col]
            color=tuple(int(F(v)*opacity+255*(1-opacity)+F(1,2)) for v in colors[row*width+col])
            rectangles.append(((int(x0),int(y0),int(x1-x0),int(y1-y0)),color))
    body=b"/Type /XObject /Subtype /Image /Width %d /Height %d "% (width,height)+dictionary
    resource=b"<< /XObject << /I 5 0 R >> "+resources+b" >>"
    content=fill+b" q "+b" ".join(str(v).encode() for v in matrix)+b" cm /I Do Q"
    return case(name,content,[stream(data,body),*masks],resource,rectangles,requirement=requirement,tolerance=tolerance)


def image_cases():
    colors=[RED,GREEN,BLUE,(17,85,170)]
    for name,matrix in [
        ("image-page-rgb",(32,0,0,32,0,0)),
        ("image-page-rgb-crop",(64,0,0,32,0,0)),
        ("image-page-rgb-quarter-turn",(0,32,-32,0,32,0)),
        ("image-page-rgb-reflected",(-32,0,0,32,32,0)),
    ]:
        yield image_case(name,bytes(v for color in colors for v in color),
                         b"/ColorSpace /DeviceRGB /BitsPerComponent 8",[],2,2,
                         matrix=matrix,colors=colors,requirement="paint-image-resampling")
    yield image_case("image-rgb-colour-key",bytes(RED+GREEN+RED+BLUE),b"/ColorSpace /DeviceRGB /BitsPerComponent 8 /Mask [255 255 0 0 0 0]",[],4,1,colors=[RED,GREEN,RED,BLUE],alpha=[0,1,0,1],requirement="paint-image-masks")
    pixels=[0,85,170,255,255,170,85,0]
    for turns,matrix in enumerate([(8,0,0,4,2,2),(0,8,-4,0,6,2),(-8,0,0,-4,10,6),(0,-8,4,0,2,10)]):
        yield image_case(f"image-quarter-turn-{turns}",bytes(pixels),b"/ColorSpace /DeviceGray /BitsPerComponent 8",pixels,4,2,matrix=matrix)
    yield image_case("image-decode-reversed",bytes(pixels),b"/ColorSpace /DeviceGray /BitsPerComponent 8 /Decode [1 0]",[255-v for v in pixels],4,2,requirement="paint-image-decode")
    yield image_case("image-decode-clipped",bytes([0,85,170,255]),b"/ColorSpace /DeviceGray /BitsPerComponent 8 /Decode [-1 2]",[0,0,255,255],4,1,requirement="paint-image-decode")
    yield image_case("image-indexed",bytes([0,1,2,3]),b"/ColorSpace [/Indexed /DeviceRGB 3 <ff000000ff000000ffffff00>] /BitsPerComponent 8",[],4,1,
                     colors=[RED,GREEN,BLUE,(255,255,0)],requirement="paint-image-decode")
    for bits in (1,2,4,8,16):
        maximum=(1<<bits)-1
        values=[0,maximum,maximum,0]
        wire=codecs.Wire()
        for value in values: wire.put(value,bits)
        yield image_case(f"image-depth-{bits}",wire.bytes(),b"/ColorSpace /DeviceGray /BitsPerComponent %d"%bits,[0,255,255,0],4,1,requirement="paint-image-decode")
    for inverted in (False,True):
        colors=[RED if (v==int(inverted)) else WHITE for v in [0,1,1,0]]
        yield image_case(f"image-stencil-{int(inverted)}",b"\x60",b"/ImageMask true /BitsPerComponent 1"+(b" /Decode [1 0]" if inverted else b""),[],4,1,
                         colors=colors,fill=b"1 0 0 rg",requirement="paint-image-masks")
    yield image_case("image-colour-key",bytes(pixels),b"/ColorSpace /DeviceGray /BitsPerComponent 8 /Decode [1 0] /Mask [80 180]",[255-v for v in pixels],4,2,
                     alpha=[0 if 80<=v<=180 else 1 for v in pixels],requirement="paint-image-masks")
    mask=stream(b"\x40",b"/Subtype /Image /Width 2 /Height 1 /BitsPerComponent 1 /ImageMask true")
    yield image_case("image-explicit-mask",bytes(pixels),b"/ColorSpace /DeviceGray /BitsPerComponent 8 /Mask 6 0 R",pixels,4,2,masks=[mask],
                     alpha=[1,1,0,0]*2,requirement="paint-image-masks")
    soft=stream(bytes([0,85,170,255]),b"/Subtype /Image /Width 4 /Height 1 /BitsPerComponent 8 /ColorSpace /DeviceGray")
    yield image_case("image-soft-mask",bytes([0]*4),b"/ColorSpace /DeviceGray /BitsPerComponent 8 /SMask 6 0 R",[0]*4,4,1,masks=[soft],alpha=[F(v,255) for v in [0,85,170,255]],requirement="paint-image-soft-mask")
    matte=stream(bytes([128]*4),b"/Subtype /Image /Width 4 /Height 1 /BitsPerComponent 8 /ColorSpace /DeviceGray /Matte [1]")
    yield image_case("image-soft-mask-matte",bytes([127]*4),b"/ColorSpace /DeviceGray /BitsPerComponent 8 /SMask 6 0 R",[0]*4,4,1,masks=[matte],alpha=[F(128,255)]*4,requirement="paint-image-soft-mask")
    yield image_case("image-flate",zlib.compress(bytes(pixels)),b"/ColorSpace /DeviceGray /BitsPerComponent 8 /Filter /FlateDecode",pixels,4,2,requirement="paint-image-codecs")
    encoded,expected=codecs.jpeg(width=8,height=8,n=1)
    yield image_case("image-dct",encoded,b"/ColorSpace /DeviceGray /BitsPerComponent 8 /Filter /DCTDecode",list(expected),8,8,requirement="paint-image-codecs",tolerance=1)
    rows=[[int((x+y)%3==0) for x in range(8)] for y in range(4)]
    fax,_=codecs.ccitt(rows,k=-1)
    yield image_case("image-ccitt",fax,b"/ColorSpace /DeviceGray /BitsPerComponent 1 /Filter /CCITTFaxDecode /DecodeParms << /K -1 /Columns 8 /Rows 4 >>",[255*(1-v) for row in rows for v in row],8,4,requirement="paint-image-codecs")
    data=jbig2.segment(1,48,1,jbig2.page_info(8,4))+jbig2.segment(2,38,1,jbig2.region(rows,jbig2.mmr(rows)))
    yield image_case("image-jbig2",data,b"/ColorSpace /DeviceGray /BitsPerComponent 1 /Filter /JBIG2Decode",[255*(1-v) for row in rows for v in row],8,4,requirement="paint-image-codecs")
    gray=[x*17+y*3 for y in range(4) for x in range(8)]
    data=jpx.jp2(jpx.codestream([gray],8,4),8,4,1)
    yield image_case("image-jpx",data,b"/Filter /JPXDecode",gray,8,4,requirement="paint-image-codecs")
    # Independently associated red and alpha planes for SMaskInData=1 and 2.
    alphas=[0,85,170,255]*2
    for mode in (1,2):
        red=[255]*8 if mode==1 else alphas
        definitions=struct.pack(">H",4)+b"".join(struct.pack(">HHH",*v) for v in [(0,0,1),(1,0,2),(2,0,3),(3,1,0)])
        data=jpx.jp2(jpx.codestream([red,[0]*8,[0]*8,alphas],4,2),4,2,4,extra=jpx.box(b"cdef",definitions))
        yield image_case(f"image-jpx-alpha-{mode}",data,b"/Filter /JPXDecode /SMaskInData %d"%mode,[],4,2,colors=[RED]*8,alpha=[F(a,255) for a in alphas],requirement="paint-image-soft-mask")
    yield case("image-inline",b"q 8 0 0 4 2 2 cm BI /W 4 /H 2 /BPC 8 /CS /G ID "+bytes(pixels)+b" EI Q",rectangles=[((2+x*2,2+(1-y)*2,2,2),(pixels[y*4+x],)*3) for y in range(2) for x in range(4)],requirement="paint-inline")
    yield case("image-inline-filtered",b"q 8 0 0 4 2 2 cm BI /W 4 /H 2 /BPC 8 /CS /G /F /AHx ID "+bytes(pixels).hex().encode()+b"> EI Q",rectangles=[((2+x*2,2+(1-y)*2,2,2),(pixels[y*4+x],)*3) for y in range(2) for x in range(4)],requirement="paint-inline")
    # Minification integrates four samples into one device pixel.
    yield case("image-minification",b"q 1 0 0 1 2 2 cm /I Do Q",[stream(bytes([0,255,255,0]),b"/Subtype /Image /Width 2 /Height 2 /BitsPerComponent 8 /ColorSpace /DeviceGray")],
               b"<< /XObject << /I 5 0 R >> >>",[((2,2,1,1),(128,128,128))],requirement="paint-image-resampling")
    # Bilinear interpolation between sample centres at .25 and .75 in unit space.
    values=[0,0,32,96,159,223,255,255]
    yield case("image-interpolate",b"q 8 0 0 2 2 2 cm /I Do Q",[stream(b"\0\xff",b"/Subtype /Image /Width 2 /Height 1 /BitsPerComponent 8 /ColorSpace /DeviceGray /Interpolate true")],
               b"<< /XObject << /I 5 0 R >> >>",[((2+x,2,1,2),(v,v,v)) for x,v in enumerate(values)],requirement="paint-image-resampling")


def direct_image_oracle(width, height, components, bits, codes, placement, *, inverted=False, interpolate=False, size=SIZE):
    """Exact page pixels of one opaque image over white: rational area means
    when minified, nearest or bilinear centre samples when magnified (8.9.5.3),
    each mixed by its pixel's coverage byte and rounded half up once.
    `placement` maps the unit square to device pixels, axis-aligned or by a
    quarter turn. Returns the PPM and the number of exact rounding ties."""
    a, b, c, d, e, f = (F(v) for v in placement)
    step = 255 // ((1 << bits) - 1)
    def level(column, row, channel):
        value = codes[(row * width + column) * components + min(channel, components - 1)] * step
        return 255 - value if inverted else value
    def rect(u0, u1, v0, v1):
        p = (u0 * a + v0 * c + e, u0 * b + v0 * d + f)
        q = (u1 * a + v1 * c + e, u1 * b + v1 * d + f)
        return min(p[0], q[0]), max(p[0], q[0]), min(p[1], q[1]), max(p[1], q[1])
    def overlap(lo, hi, p):
        return max(F(0), min(hi, F(p + 1)) - max(lo, F(p)))
    full = rect(F(0), F(1), F(0), F(1))
    minifying = F(width) / abs(a or b) > 1 or F(height) / abs(c or d) > 1
    det = a * d - b * c
    pixels, ties = bytearray(), 0
    for y in range(size):
        for x in range(size):
            covered = overlap(full[0], full[1], x) * overlap(full[2], full[3], y)
            coverage = int(covered * 255 + F(1, 2))
            for channel in range(3):
                if coverage == 0:
                    pixels.append(255)
                    continue
                if minifying:
                    total, area = F(0), F(0)
                    for row in range(height):
                        for column in range(width):
                            cell = rect(F(column, width), F(column + 1, width), 1 - F(row + 1, height), 1 - F(row, height))
                            part = overlap(cell[0], cell[1], x) * overlap(cell[2], cell[3], y)
                            total += part * level(column, row, channel)
                            area += part
                    value = total / area
                else:
                    px, py = F(2 * x + 1, 2) - e, F(2 * y + 1, 2) - f
                    column = (px * d - py * c) / det * width
                    row = height - (py * a - px * b) / det * height
                    clamp = lambda v, n: min(max(v, 0), n - 1)
                    if not interpolate:
                        value = F(level(clamp(math.floor(column), width), clamp(math.floor(row), height), channel))
                    else:
                        column, row = column - F(1, 2), row - F(1, 2)
                        left, top = math.floor(column), math.floor(row)
                        fx, fy = column - left, row - top
                        value = sum(level(clamp(left + dx, width), clamp(top + dy, height), channel) * wx * wy
                                    for dx, wx in ((0, 1 - fx), (1, fx)) for dy, wy in ((0, 1 - fy), (1, fy)))
                mixed = (value * coverage + 255 * (255 - coverage)) / 255
                ties += (mixed - math.floor(mixed)) == F(1, 2)
                pixels.append(math.floor(mixed + F(1, 2)))
    return b"P6\n%d %d\n255\n" % (size, size) + bytes(pixels), ties


def direct_image_cases():
    """Sole opaque images that the compositor paints straight into the page:
    one-bit and inverted two-bit quarter-turned two-to-one means, reflected
    RGB minification with fractional edges mixed by coverage, and bilinear
    /Interpolate magnification by eight. Dyadic placements keep the product's
    f64 sums exact, so half-level ties round up like the oracle; the
    fractional case has none. Every oracle byte is matched exactly."""
    def make(name, width, height, components, bits, codes, placement, *, inverted=False, interpolate=False, ties_expected=False):
        a, b, c, d, e, f = placement
        # Device pixels to PDF user space: y is measured from the page bottom.
        matrix = (a, -b, c, -d, e, SIZE - f)
        wire = codecs.Wire()
        for value in codes:
            wire.put(value, bits)
        space = b"/DeviceGray" if components == 1 else b"/DeviceRGB"
        extra = (b" /Decode [1 0]" if inverted else b"") + (b" /Interpolate true" if interpolate else b"")
        image = stream(wire.bytes(), b"/Subtype /Image /Width %d /Height %d /BitsPerComponent %d /ColorSpace %s%s" % (width, height, bits, space, extra))
        oracle, ties = direct_image_oracle(width, height, components, bits, codes, placement, inverted=inverted, interpolate=interpolate)
        assert (ties > 0) == ties_expected, (name, ties)
        content = b"q " + b" ".join(str(v).encode() for v in matrix) + b" cm /I Do Q"
        return case(name, content, [image], b"<< /XObject << /I 5 0 R >> >>", oracle=oracle, requirement="paint-image-resampling")
    binary = [(x * 7 + y * 3 + x * y) % 5 % 2 for y in range(32) for x in range(32)]
    yield make("image-direct-binary-halves", 32, 32, 1, 1, binary, (16, 0, 0, -16, 8, 24), ties_expected=True)
    two = [(x * 5 + y * 3) % 4 for y in range(24) for x in range(20)]
    yield make("image-direct-gray-quarter-turn", 20, 24, 1, 2, two, (0, 10, 12, 0, 4, 6), inverted=True)
    rgb = [(x * 31 + y * 71 + channel * 113 + x * y) % 256 for y in range(36) for x in range(48) for channel in range(3)]
    yield make("image-direct-rgb-fractional", 48, 36, 3, 8, rgb, (-20.5, 0, 0, 13.75, 26.25, 4.5))
    small = [17, 85, 170, 255, 0, 120, 60, 200, 30, 240, 90, 10, 220, 5, 140, 75, 180, 35]
    yield make("image-direct-bilinear", 3, 2, 3, 8, small, (24, 0, 0, -16, 4, 26), interpolate=True, ties_expected=True)


def cases():
    yield from text_cases()
    yield from image_cases()
    yield from image_geometry_cases()
    yield from extra_cases()
    yield from bundled_cases()
    yield from repair_cases()
    yield cjk_substitute_case()
    yield from affine_area_cases()
    yield shared_font_case()
    yield late_font_repair_case()
    yield from image_colour_cases()
    yield damaged_glyph_case()
    yield from separable_area_cases()
    yield from area_threshold_cases()
    yield from direct_image_cases()


def type1_lengths(program):
    """Length1-Length3 of a PFA-style Type 1 program (Table 125): the cleartext
    through eexec and its end-of-line, the encrypted part, and the trailer of
    zeros and cleartomark."""
    marker = program.index(b"eexec") + len(b"eexec")
    while program[marker:marker + 1] in (b"\r", b"\n", b" ", b"\t"):
        marker += 1
    trailer = program.find(b"0" * 64, marker)
    trailer = len(program) if trailer < 0 else trailer
    return b"/Length1 %d /Length2 %d /Length3 %d" % (marker, trailer - marker, len(program) - trailer)


def extra_cases():
    content=b"BT /F 10 Tf 12 TL 1 0 0 1 4 244 Tm ("+b"A"*50+b") Tj"+(b" ("+b"A"*50+b") '")*19+b" ET"
    rectangles=[((4+col*4,244-row*12,4,6),BLACK) for row in range(20) for col in range(50)]
    yield case("text-heavy-page",content,ttf(),FONT,rectangles,size=256,requirement="paint-text-outlines")
    from .font import cff_font, type1_font
    for name,program,key,extra in [("type1",type1_font(),b"FontFile",b""),("cff",cff_font(),b"FontFile3",b"/Subtype /Type1C")]:
        font=ttf()[0].replace(b"/TrueType",b"/Type1").replace(b"/Widths [400]",b"/Widths [600]")
        descriptor=ttf()[1].replace(b"/FontFile2",b"/"+key)
        # Both programs define glyphs with non-Latin names such as curve;
        # the descriptor therefore sets Symbolic (9.8.2/Table 121).
        descriptor = descriptor.replace(b"/Flags 32", b"/Flags 4")
        if key == b"FontFile":
            extra = type1_lengths(program)
        yield case("text-"+name,b"BT /F 20 Tf 4 4 Td (AA) Tj ET",[font,descriptor,stream(program,extra)],FONT,
                   [((4,4,2,4),BLACK),((16,4,2,4),BLACK)],requirement="paint-text-outlines")
    objects=type3(b"400 0 0 0 400 600 d1 /G gs 100 200 m 300 200 l S",resources=b"<< /ExtGState << /G << /LW 100 /CA 0 >> >> >>")
    yield case("type3-d1-gstate",b"1 0 0 RG BT /F 20 Tf 4 4 Td (A) Tj ET",objects,FONT,[((6,7,4,2),RED)],requirement="paint-type3")
    objects=type3(b"400 0 0 0 400 600 d1 400 0 0 600 0 0 cm /I Do")
    objects[0]=objects[0].replace(b"/Resources << >>",b"")
    objects.append(stream(b"\0",b"/Subtype /Image /Width 1 /Height 1 /BitsPerComponent 1 /ImageMask true"))
    yield case("type3-stencil-page-resources",b"1 0 0 rg BT /F 20 Tf 4 4 Td (A) Tj ET",objects,b"<< /Font << /F 5 0 R >> /XObject << /I 7 0 R >> >>",[((4,4,8,12),RED)],requirement="paint-type3")
    objects=type3(b"400 0 d0 BT /F 10 Tf (A) Tj ET",resources=b"<< /Font << /F 5 0 R >> >>")
    yield Case("type3-recursion",page(b"BT /F 20 Tf (A) Tj ET",objects,FONT),OPEN,("paint-budgets",),"Recursive CharProc fails with a cycle",extra={"render_error":"cycle"})
    objects=type3(b"401 0 d0")
    yield Case("type3-inconsistent-width",page(b"BT /F 20 Tf (A) Tj ET",objects,FONT),OPEN,("paint-text-errors",),"CharProc width disagrees with Widths; Widths advance retained with a repair",repairs=("font.type3-width",),strict_valid=False)
    # An Indexed palette is unblended in its base colour space, before conversion.
    mask=stream(bytes([128,128]),b"/Subtype /Image /Width 2 /Height 1 /BitsPerComponent 8 /ColorSpace /DeviceGray /Matte [1 1 1]")
    yield image_case("image-indexed-matte",bytes([0,0]),b"/ColorSpace [/Indexed /DeviceRGB 0 <ff7f7f>] /BitsPerComponent 8 /SMask 6 0 R",[],2,1,masks=[mask],colors=[RED,RED],alpha=[F(128,255)]*2,requirement="paint-image-soft-mask")
    yield image_case("image-rgb",bytes(RED+GREEN+BLUE+WHITE),b"/ColorSpace /DeviceRGB /BitsPerComponent 8",[],4,1,colors=[RED,GREEN,BLUE,WHITE])
    yield image_case("image-cmyk",bytes([0,255,255,0,255,0,255,0,255,255,0,0,0,0,0,255]),b"/ColorSpace /DeviceCMYK /BitsPerComponent 8",[],4,1,colors=[RED,GREEN,BLUE,BLACK])
    # The current mask makes everything transparent, but image-local masks win.
    resources=(b"/ExtGState << /G << /SMask << /S /Alpha /G 7 0 R >> >> >>")
    group=stream(b"",b"/Subtype /Form /BBox [0 0 32 32] /Group << /S /Transparency /I true /CS /DeviceRGB >>")
    mask=stream(bytes([255]),b"/Subtype /Image /Width 1 /Height 1 /BitsPerComponent 8 /ColorSpace /DeviceGray")
    yield image_case("image-soft-overrides-state",b"\0",b"/ColorSpace /DeviceGray /BitsPerComponent 8 /SMask 6 0 R /Mask [0 255]",[0],1,1,masks=[mask,group],resources=resources,fill=b"/G gs",requirement="paint-image-soft-mask")
    # Nearest explicit mask has a different resolution from the base image.
    mask=stream(b"\x80\x00",b"/Subtype /Image /Width 1 /Height 2 /BitsPerComponent 1 /ImageMask true /Decode [1 0]")
    yield image_case("image-mask-resolution",b"\0\0",b"/ColorSpace /DeviceGray /BitsPerComponent 8 /Mask 6 0 R",[0,0],1,2,masks=[mask],alpha=[1,0],requirement="paint-image-masks")
    for content,name in [(b"BI","image-inline-unfinished"),(b"ID","image-inline-id-outside"),(b"EI","image-inline-ei-outside"),(b"BI /W 1 /W 2 ID \0 EI","image-inline-duplicate"),(b"BI /W 1 /H 1 /BPC 8 /CS /G ID \0","image-inline-missing-ei")]:
        yield Case(name,page(content),OPEN,("paint-inline",),name.replace("-"," "),extra={"render_error":"syntax"})

    yield image_case("image-jpx-icc-explicit",jpx.jp2(jpx.codestream([[0]],1,1),1,1,1,method=2),
                     b"/Filter /JPXDecode",[0],1,1,requirement="paint-jpx-icc")
    for name,data,dictionary,fill,tag in [
        ("image-stencil-pattern-explicit",b"\0",b"/ImageMask true /BitsPerComponent 1",b"/Pattern cs ","pattern.image-mask"),
    ]:
        image=stream(data,b"/Subtype /Image /Width 1 /Height 1 "+dictionary)
        yield Case(name,page(fill+b"2 0 0 2 2 2 cm /I Do",[image],b"<< /XObject << /I 5 0 R >> >>"),UNSUPPORTED,("paint-images",),name.replace("-"," "),feature=tag)

    mask=stream(b"/Half gs 0 0 32 32 re f",b"/Subtype /Form /BBox [0 0 32 32] /Group << /S /Transparency /CS /DeviceRGB /I true >> /Resources << /ExtGState << /Half << /ca .5 >> >> >>")
    resources=b"<< /Font << /F 5 0 R >> /ExtGState << /G << /SMask << /S /Alpha /G 8 0 R >> >> >> >>"
    yield case("text-soft-mask-inside-object",b"1 0 0 rg BT /F 20 Tf /G gs 4 4 Td (A) Tj ET",ttf()+[mask],resources,[((4,4,8,12),(255,128,128))],requirement="paint-text-knockout")

    parent=type3(b"400 0 0 0 400 600 d1 BT /N 1000 Tf (A) Tj ET",resources=b"<< /Font << /N 7 0 R >> >>")
    child=type3(b"400 0 d0 0 0 1 rg 0 0 400 600 re f")
    child[0]=child[0].replace(b"/A 6 0 R",b"/A 8 0 R")
    yield case("type3-d1-nested-colour",b"1 0 0 rg BT /F 20 Tf 4 4 Td (A) Tj ET",parent+child,FONT,[((4,4,8,12),RED)],requirement="paint-type3")

    objects=type3(b"400 0 0 0 400 600 d1 /I Do /Unused sh 0 0 400 600 re f",resources=b"<< /XObject << /I 7 0 R >> >>")
    objects.append(stream(b"invalid JPEG",b"/Subtype /Image /Width 1 /Height 1 /BitsPerComponent 8 /ColorSpace /DeviceRGB /Filter /DCTDecode"))
    yield case("type3-d1-image-shading-ignored",b"1 0 0 rg BT /F 20 Tf 4 4 Td (A) Tj ET",objects,FONT,[((4,4,8,12),RED)],requirement="paint-type3")

    objects=type3()
    objects[0]=objects[0].replace(b"[.001 0 0 .001 0 0]",b"[0 .001 -.001 0 0 0]")
    yield case("type3-rotated-horizontal-width",b"1 0 0 rg BT /F 20 Tf 16 4 Td (A) Tj 0 0 1 rg (A) Tj ET",objects,FONT,[((4,4,12,8),BLUE)],requirement="paint-type3")


def bundled_contours(path, character):
    """Read simple TrueType contours independently, per the sfnt/glyf format.
    The selected fixture letters have only on-curve points, so their covered
    area has an exact rational polygon oracle, with no curve approximation.
    """
    data = path.read_bytes()
    u16 = lambda at: struct.unpack_from('>H', data, at)[0]
    i16 = lambda at: struct.unpack_from('>h', data, at)[0]
    u32 = lambda at: struct.unpack_from('>I', data, at)[0]
    tables = {data[at:at+4]: u32(at+8) for at in range(12, 12+16*u16(4), 16)}
    cmap = tables[b'cmap']
    sub = next(cmap+u32(at+4) for at in range(cmap+4, cmap+4+8*u16(cmap+2), 8)
               if (u16(at), u16(at+2)) == (3, 1))
    assert u16(sub) == 4
    n = u16(sub+6)//2
    ends, starts = sub+14, sub+16+2*n
    deltas, offsets = starts+2*n, starts+4*n
    code = ord(character)
    i = next(i for i in range(n) if u16(starts+2*i) <= code <= u16(ends+2*i))
    delta, offset = i16(deltas+2*i), u16(offsets+2*i)
    glyph = u16(offsets+2*i+offset+2*(code-u16(starts+2*i))) if offset else code
    glyph = (glyph+delta) & 65535 if glyph else 0
    long_loca = i16(tables[b'head']+50) == 1
    loc = tables[b'loca']+(4 if long_loca else 2)*glyph
    at = tables[b'glyf']+(u32(loc) if long_loca else 2*u16(loc))
    count = i16(at)
    assert count > 0, 'fixture glyph must have simple contours'
    ends = [u16(at+10+2*i) for i in range(count)]
    pos = at+10+2*count
    pos += 2+u16(pos)
    flags = []
    while len(flags) <= ends[-1]:
        flag = data[pos]; pos += 1
        repeat = data[pos] if flag & 8 else 0
        pos += bool(flag & 8)
        flags.extend([flag]*(repeat+1))
    coordinates = []
    for short, same in ((2,16),(4,32)):
        value = 0; axis = []
        for flag in flags:
            if flag & short:
                change = data[pos]; pos += 1
                value += change if flag & same else -change
            elif not flag & same:
                value += i16(pos); pos += 2
            axis.append(value)
        coordinates.append(axis)
    assert all(flag & 1 for flag in flags), 'select straight-sided glyphs for an exact oracle'
    contours = []; start = 0
    for end in ends:
        contours.append(list(zip(coordinates[0][start:end+1], coordinates[1][start:end+1])))
        start = end+1
    return contours, u16(tables[b'head']+18)


def bundled_oracle(font_path, afm_path, text, *, advance=None, size=64):
    """AFM widths control placement, while bundled TrueType contours control ink."""
    widths = {}
    for line in afm_path.read_text().splitlines():
        if line.startswith('C '):
            fields = dict(part.strip().split(' ',1) for part in line.split(';') if part.strip())
            widths[fields['N']] = F(fields['WX'])
    polygons = []; x = F(3)
    font_size = 24
    for character in text:
        contours, upem = bundled_contours(font_path, character)
        for contour in contours:
            polygons.append([(x+F(px*font_size,upem),F(8)+F(py*font_size,upem)) for px,py in contour])
        x += F(font_size,1000)*(widths[character] if advance is None else advance)
    # Contours carry winding, so holes subtract ink independently.
    signed = []
    for polygon in polygons:
        area = sum(a[0]*b[1]-a[1]*b[0] for a,b in zip(polygon,polygon[1:]+polygon[:1]))
        signed.append((1 if area > 0 else -1, polygon))
    pixels = []
    for y in range(size):
        for x in range(size):
            coverage = abs(sum(sign*clipped_area(polygon,x,size-1-y) for sign,polygon in signed))
            level = 255-int(min(F(1),coverage)*255+F(1,2))
            pixels.extend([level]*3)
    return b'P6\n%d %d\n255\n' % (size,size)+bytes(pixels)


def bundled_cases():
    """Provider-dependent PDF vectors; facade tests exercise these with and
    without data. RGB oracle files keep their dedicated provider test driver
    separate from the renderer's embedded-resource oracle suite.
    """
    from pathlib import Path
    import hashlib
    from .model import Vector
    root = Path(__file__).resolve().parents[2]
    entries = [('helvetica','Helvetica','Arimo-Regular'),
               ('times-bold','Times-Bold','Tinos-Bold'),
               ('courier-oblique','Courier-Oblique','Cousine-Italic')]
    for name,base,outline in entries:
        text = 'HH'
        oracle = bundled_oracle(root/'data/fonts'/f'{outline}.ttf',root/'data/afm'/f'{base}.afm',text)
        font = b'<< /Type /Font /Subtype /Type1 /BaseFont /'+base.encode()+b' >>'
        identifier = 'bundled-'+name
        yield Vector(identifier,{'.pdf':page(b'BT /F 24 Tf 3 8 Td (HH) Tj ET',[font],FONT,64),'.rgb':oracle},
                     ('paint-bundled-fonts',), 'Non-embedded '+base+' uses AFM widths and bundled outlines',
                     extra={'path':f'corpus/generated/paint/{identifier}.pdf','sha256':hashlib.sha256(page(b'BT /F 24 Tf 3 8 Td (HH) Tj ET',[font],FONT,64)).hexdigest(),'kind':'pdf','expected':RENDER,'pages':1,'repairs':[],'information':['font.substituted'],'strict_valid':True,
                            'oracle':f'corpus/generated/paint/{identifier}.rgb','oracle_sha256':hashlib.sha256(oracle).hexdigest(),'tolerance':1,'width':64,'height':64,'dpi':72})
    base, outline = 'Courier-Oblique','Cousine-Italic'
    oracle = bundled_oracle(root/'data/fonts'/f'{outline}.ttf',root/'data/afm'/f'{base}.afm','HH',advance=650)
    font = b'<< /Type /Font /Subtype /Type1 /BaseFont /UnknownFamily /Encoding /WinAnsiEncoding /FirstChar 72 /LastChar 72 /Widths [650] /FontDescriptor 6 0 R >>'
    descriptor = b'<< /Type /FontDescriptor /FontName /UnknownFamily /Flags 97 /FontBBox [-100 -300 1200 1000] /ItalicAngle -12 /Ascent 800 /Descent -200 /CapHeight 700 /StemV 80 >>'
    identifier = 'bundled-descriptor-flags'
    yield Vector(identifier,{'.pdf':page(b'BT /F 24 Tf 3 8 Td (HH) Tj ET',[font,descriptor],FONT,64),'.rgb':oracle},
                 ('paint-bundled-fonts',),'FontDescriptor flags select fixed-pitch italic substitute; PDF widths take precedence',
                 extra={'path':f'corpus/generated/paint/{identifier}.pdf','sha256':hashlib.sha256(page(b'BT /F 24 Tf 3 8 Td (HH) Tj ET',[font,descriptor],FONT,64)).hexdigest(),'kind':'pdf','expected':RENDER,'pages':1,'repairs':[],'information':['font.substituted'],'strict_valid':True,'oracle':f'corpus/generated/paint/{identifier}.rgb','oracle_sha256':hashlib.sha256(oracle).hexdigest(),'tolerance':1,'width':64,'height':64,'dpi':72})
    identifier = 'bundled-diagnostic-kinds'
    damaged_descriptor = descriptor[:-2]+b'/FontWeight 0 >>'
    pdf = page(b'BT /F 24 Tf 3 8 Td (HH) Tj ET',[font,damaged_descriptor],FONT,64)
    yield render_vector(identifier,pdf,oracle,'paint-diagnostic-kinds',
                        'Invalid optional weight is a repair; permitted font substitution is information',
                        repairs=('font.descriptor-weight',),information=('font.substituted',),
                        strict_valid=False,tolerance=1,size=64)
    from .paint_cff import oracle as cff_oracle
    for base, outline, names in [('Symbol','FoxitSymbol',('Omega','alpha')),
                                  ('ZapfDingbats','FoxitDingbats',('a2','a1'))]:
        for differences in (False, True):
            identifier = 'bundled-'+base.lower()+('-differences' if differences else '')
            encoding = b''
            mapping = dict(zip(b'AB', names)) if differences else None
            advances = {65: F(850), 66: F(850)} if differences and base == 'Symbol' else None
            codes = b'ABA' if advances else b'AB'
            if differences:
                encoding = b' /Encoding << /Differences [65 /'+names[0].encode()+b' /'+names[1].encode()+b'] >>'
            if advances:
                encoding += b' /FirstChar 65 /LastChar 66 /Widths [850 850]'
            font = b'<< /Type /Font /Subtype /Type1 /BaseFont /'+base.encode()+encoding+b' >>'
            pdf = page(b'BT /F 24 Tf 3 8 Td ('+codes+b') Tj ET',[font],FONT,64)
            oracle = cff_oracle(root/'data/fonts'/f'{outline}.cff',root/'data/afm'/f'{base}.afm',
                                codes,differences=mapping,advances=advances)
            yield render_vector(identifier,pdf,oracle,'paint-bundled-fonts',
                                'AFM names and advances select bundled CFF outlines; rational cubic area oracle',
                                information=('font.substituted',),tolerance=1,size=64)
    # H is Adobe-Japan1's predefined two-byte JIS map. Code 0x2422 maps
    # to CID 843 (hiragana A). A licensed bundled H outline is deliberately
    # assigned to that CID in the embedded subset, separating code/CID/glyph
    # identities while keeping an exact polygon oracle.
    contours, upem = bundled_contours(root/'data/fonts/Arimo-Regular.ttf','H')
    points = [(x,y,True) for contour in contours for x,y in contour]
    ends = []; n = 0
    for contour in contours:
        n += len(contour); ends.append(n-1)
    glyph = simple(points,ends)
    xs, ys = [p[0] for p in points],[p[1] for p in points]
    glyph = glyph[:2]+struct.pack('>hhhh',min(xs),min(ys),max(xs),max(ys))+glyph[10:]
    head = bytearray(54); struct.pack_into('>I',head,0,0x10000)
    struct.pack_into('>I',head,12,0x5f0f3cf5); struct.pack_into('>H',head,18,upem); struct.pack_into('>h',head,50,1)
    import re
    cmap = (root/'data/cmaps/Adobe-Japan1-7/H').read_text()
    code = 0x2422
    cid = next(int(cid)+code-int(lo,16) for lo,hi,cid in re.findall(r'<([0-9a-f]+)>\s*<([0-9a-f]+)>\s*(\d+)',cmap)
               if int(lo,16)<=code<=int(hi,16))
    cid_map = bytearray((cid+1)*2); struct.pack_into('>H',cid_map,cid*2,1)
    # Read the advance from AFM, then quantize it to the TrueType unit grid.
    # The embedded hmtx and PDF DW agree exactly (9.2.4).
    afm = root/'data/afm/Helvetica.afm'
    width = next(int(re.search(r'WX\s+(\d+)',line).group(1)) for line in afm.read_text().splitlines() if '; N H ;' in line)  # pyright: ignore[reportOptionalMemberAccess]
    advance = round(F(width*upem,1000))
    pdf_width = F(advance*1000,upem)
    metrics = struct.pack('>HhHh',advance,0,advance,0)+bytes(6)
    program = truetype(glyph_override=glyph,table_overrides={b'head':bytes(head),b'hmtx':metrics})
    objects = [b'<< /Type /Font /Subtype /Type0 /BaseFont /Subset /Encoding /H /DescendantFonts [6 0 R] >>',
               b'<< /Type /Font /Subtype /CIDFontType2 /BaseFont /Subset /CIDSystemInfo << /Registry (Adobe) /Ordering (Japan1) /Supplement 1 >> /FontDescriptor 7 0 R /CIDToGIDMap 9 0 R /DW '+str(float(pdf_width)).encode()+b' >>',
               b'<< /Type /FontDescriptor /FontName /Subset /Flags 4 /FontBBox [0 -300 1200 1000] /ItalicAngle 0 /Ascent 800 /Descent -200 /CapHeight 700 /StemV 80 /FontFile2 8 0 R >>',
               stream(program,b'/Length1 %d' % len(program)),stream(bytes(cid_map))]
    identifier = 'bundled-cjk-cmap'
    pdf = page(b'BT /F 24 Tf 3 8 Td <24222422> Tj ET',objects,FONT,64)
    oracle = bundled_oracle(root/'data/fonts/Arimo-Regular.ttf',afm,'HH',advance=pdf_width)
    yield Vector(identifier,{'.pdf':pdf,'.rgb':oracle},('paint-bundled-cmaps',),
                 'Predefined CJK CMap selects an embedded CID font through a nonidentity CIDToGIDMap',
                 extra={'kind':'pdf','path':f'corpus/generated/paint/{identifier}.pdf','sha256':hashlib.sha256(pdf).hexdigest(),
                        'expected':RENDER,'pages':1,'strict_valid':True,'repairs':[], 'oracle':f'corpus/generated/paint/{identifier}.rgb',
                        'oracle_sha256':hashlib.sha256(oracle).hexdigest(),'tolerance':1,'width':64,'height':64,'dpi':72})


def affine_area_cases():
    """Independent source-cell polygon clipping, including partial image edges.
    The sampling mean is premultiplied by the rasterizer's quantized image-shape
    coverage, matching the documented composition boundary rather than assuming
    the background participates in the resampling filter.
    """
    for name,matrix in [('fractional',(F(3,2),0,0,F(3,2),F(9,4),F(9,4))),
                        ('reflected',(-F(3,2),0,0,-F(3,2),F(17,4),F(17,4))),
                        ('quarter',(0,F(3,2),-F(3,2),0,F(17,4),F(9,4))),
                        ('sheared',(F(3,2),F(1,4),F(1,2),F(3,2),F(9,4),F(9,4)))]:
        a,b,c,d,e,f=matrix
        polygons=[]; values=[0,85,170,255]
        for row in range(2):
            for col in range(2):
                polygons.append([(e+a*u+c*v,f+b*u+d*v) for u,v in [(F(col,2),1-F(row,2)),(F(col+1,2),1-F(row,2)),(F(col+1,2),1-F(row+1,2)),(F(col,2),1-F(row+1,2))]])
        pixels=[]
        for y in range(32):
            for x in range(32):
                areas=[clipped_area(p,x,31-y) for p in polygons]
                covered=sum(areas)
                shape=F(int(covered*255+F(1,2)),255)
                mean=sum(area*value for area,value in zip(areas,values))/covered if covered else 0
                pixels.extend([int(mean*shape+255*(1-shape)+F(1,2))]*3)
        oracle=b'P6\n32 32\n255\n'+bytes(pixels)
        content=b'q '+b' '.join(str(float(v)).encode() for v in matrix)+b' cm /I Do Q'
        image=stream(bytes(values),b'/Subtype /Image /Width 2 /Height 2 /BitsPerComponent 8 /ColorSpace /DeviceGray')
        yield case('image-area-'+name,content,[image],b'<< /XObject << /I 5 0 R >> >>',oracle=oracle,tolerance=1,requirement='paint-image-resampling')


def shared_font_case():
    objects=[b'<< /Type /Catalog /Pages 2 0 R >>',
             b'<< /Type /Pages /Kids [3 0 R 8 0 R] /Count 2 >>',
             b'<< /Type /Page /Parent 2 0 R /MediaBox [0 0 32 32] /Resources << /Font << /F 5 0 R >> >> /Contents 4 0 R >>',
             stream(b'BT /F 10 Tf 2 2 Td (A) Tj ET'),*ttf(),
             b'<< /Type /Page /Parent 2 0 R /MediaBox [0 0 32 32] /Resources << /Font << /Other 5 0 R >> >> /Contents 9 0 R >>',
             stream(b'BT /Other 10 Tf 2 10 Td (AA) Tj ET')]
    return Case('text-font-shared-pages',document(objects),RENDER,('paint-font-cache',),
                'One immutable font dictionary and outline are shared across pages with different resource names',
                oracle=rects([((2,2,4,6),BLACK)]),pages=2)


def late_font_repair_case():
    """The second page alone discovers a wrong FontFile2 Length1, so a
    streaming strict audit must retry it without losing the first page.
    """
    import re
    first=ttf(); second=ttf()
    second[0]=second[0].replace(b'6 0 R',b'11 0 R')
    second[1]=second[1].replace(b'7 0 R',b'12 0 R')
    second[2]=re.sub(rb'/Length1 (\d+)',lambda m:b'/Length1 '+str(int(m[1])+1).encode(),second[2],count=1)
    objects=[b'<< /Type /Catalog /Pages 2 0 R >>',
             b'<< /Type /Pages /Kids [3 0 R 8 0 R] /Count 2 >>',
             b'<< /Type /Page /Parent 2 0 R /MediaBox [0 0 32 32] /Resources << /Font << /F 5 0 R >> >> /Contents 4 0 R >>',
             stream(b'BT /F 10 Tf 2 2 Td (A) Tj ET'),*first,
             b'<< /Type /Page /Parent 2 0 R /MediaBox [0 0 32 32] /Resources << /Font << /F 10 0 R >> >> /Contents 9 0 R >>',
             stream(b'BT /F 10 Tf 2 10 Td (AA) Tj ET'),*second]
    return Case('text-late-font-repair',document(objects),RENDER,('paint-font-cache',),
                'A repair on page two preserves both rendered pages and strict audit status',
                oracle=rects([((2,2,4,6),BLACK)]),pages=2,repairs=('font.stream-lengths',),strict_valid=False)


def image_colour_cases():
    """Table 88, 8.6.6.3 and Table 87, with independent profile functions."""
    from . import content as colour
    profile = stream(colour.icc_profile(1, version=5),
                     b'/N 1 /Alternate /DeviceGray /Range [.25 .75]')
    for indexed in (False, True):
        name = 'image-icc-indexed-range' if indexed else 'image-icc-default-decode'
        space = b'[/Indexed [/ICCBased 6 0 R] 3 <0055aaff>]' if indexed else b'[/ICCBased 6 0 R]'
        data = bytes([0,1,2,3] if indexed else [0,85,170,255])
        yield image_case(name,data,b'/ColorSpace '+space+b' /BitsPerComponent 8',
                         [64,106,149,191],4,1,masks=[profile],requirement='paint-image-icc-range')
    tags = colour.icc_tags(4)
    for i, lightness in enumerate((20,60,85)):
        tags[f'A2B{i}'.encode()] = colour.icc_legacy(4,pcs=b'Lab ',
            constant=(lightness/100*65280/65535,32768/65535,32768/65535))[0]
    profile = stream(colour.icc_pack(tags,4,pcs=b'Lab '),b'/N 4 /Alternate /DeviceCMYK')
    for name, intent, lightness in [('perceptual',b'/Intent /Perceptual',20),
                                   ('saturation',b'/Intent /Saturation',85),
                                   ('inherited',b'',60),('null',b'/Intent null',60),
                                   ('indirect-null',b'/Intent 7 0 R',60)]:
        rgb = colour.cie_rgb('Lab',(lightness,0,0),white=colour.ICC_D50)
        yield image_case('image-intent-'+name,bytes([128]*4),
                         b'/ColorSpace [/ICCBased 6 0 R] /BitsPerComponent 8 '+intent,
                         [],1,1,masks=[profile,b'null'] if name=='indirect-null' else [profile],colors=[rgb],fill=b'/RelativeColorimetric ri',
                         tolerance=1,requirement='paint-image-intent')
    rgb = colour.cie_rgb('Lab',(20,0,0),white=colour.ICC_D50)
    for name, intent in [('named',b'/Saturation'),('wrong-type',b'true')]:
        image = stream(bytes([0]),b'/Subtype /Image /Width 1 /Height 1 /BitsPerComponent 1 /ImageMask true /Intent '+intent)
        yield case('image-stencil-intent-ignored-'+name,
                   b'/C cs /Perceptual ri .5 .5 .5 .5 scn q 2 0 0 2 2 2 cm /I Do Q',
                   [image,profile],b'<< /XObject << /I 5 0 R >> /ColorSpace << /C [/ICCBased 6 0 R] >> >>',
                   [((2,2,2,2),rgb)],tolerance=1,requirement='paint-image-intent')
    image = stream(bytes([0]),b'/Subtype /Image /Width 1 /Height 1 /BitsPerComponent 8 /ColorSpace /DeviceGray /Intent true')
    yield Case('image-intent-wrong-type',page(b'/I Do',[image],b'<< /XObject << /I 5 0 R >> >>'),OPEN,
               ('paint-image-intent',),'A non-mask image Intent must be a name',extra={'render_error':'syntax'})
    # Inline image dictionaries retain Intent too (8.9.7, Table 91).
    rgb = colour.cie_rgb('Lab',(85,0,0),white=colour.ICC_D50)
    yield case('image-inline-intent',
               b'/Perceptual ri q 2 0 0 2 2 2 cm BI /W 1 /H 1 /BPC 8 /CS /C /Intent /Saturation ID '+bytes([128]*4)+b' EI Q',
               [profile],b'<< /ColorSpace << /C [/ICCBased 5 0 R] >> >>',
               [((2,2,2,2),rgb)],tolerance=1,requirement='paint-image-intent')


def damaged_glyph_case():
    # Glyph 1 lacks its header. Glyph 2 remains a complete rectangle; A's PDF
    # advance still positions B at x=6. No result is derived from a renderer.
    rectangle = simple([(0,0,True),(400,0,True),(400,600,True),(0,600,True)])
    rectangle = rectangle[:2]+struct.pack('>hhhh',0,0,400,600)+rectangle[10:]
    glyf = bytes(2)+rectangle
    loca = struct.pack('>6I',0,0,2,len(glyf),len(glyf),len(glyf))
    cmap = struct.pack('>HHHHI',0,1,3,1,12)+struct.pack('>6H',6,14,0,65,2,1)+struct.pack('>H',2)
    program = truetype(table_overrides={b'glyf':glyf,b'loca':loca,b'cmap':cmap,
                        b'hmtx':struct.pack('>HhHh',500,0,400,0)+bytes(6)})
    font,descriptor,_ = ttf()
    font = font.replace(b'/LastChar 65 /Widths [400]',b'/LastChar 66 /Widths [400 400]')
    from .model import Vector
    import hashlib
    name = 'text-unusable-glyph-advance'
    pdf = page(b'BT /F 10 Tf 2 2 Td (AAB) Tj ET',
               [font,descriptor,stream(program,b'/Length1 %d'%len(program))],FONT)
    oracle = rects([((10,2,4,6),BLACK)])
    return Vector(name,{'.pdf':pdf,'.rgb':oracle},('paint-glyph-diagnostics',),
                  'An unusable glyph reports its repair and advances; an intact following glyph still paints',
                  extra=dict(kind='pdf',expected=RENDER,path=f'corpus/generated/paint/{name}.pdf',
                             sha256=hashlib.sha256(pdf).hexdigest(),pages=1,strict_valid=False,
                             repairs=['font.glyph-data'],oracle=f'corpus/generated/paint/{name}.rgb',
                             oracle_sha256=hashlib.sha256(oracle).hexdigest(),dpi=72,width=SIZE,height=SIZE))


def separable_area_cases():
    # Four equal source-cell areas per device pixel. Include half-level ties
    # and associated colour/alpha; expectations use integers alone.
    # ADR 0004 admits either neighbour at a half level; a separate test
    # checks this tighter interval instead of only the rounded-byte tolerance.
    data = list(range(24))
    for masked in (False, True):
        marks = []
        for y in range(2):
            for x in range(3):
                samples = [data[j*6+i] for j in range(y*2,y*2+2) for i in range(x*2,x*2+2)]
                value = (sum(255 if masked and v == 0 else v for v in samples)+2)//4
                marks.append(((2+x,3-y,1,1),(value,)*3))
        name = 'image-area-separable'+('-colour-key' if masked else '')
        image = stream(bytes(data),b'/Subtype /Image /Width 6 /Height 4 /BitsPerComponent 8 /ColorSpace /DeviceGray'+(b' /Mask [0 0]' if masked else b''))
        yield case(name,b'q 3 0 0 2 2 2 cm /I Do Q',[image],b'<< /XObject << /I 5 0 R >> >>',
                   rectangles=marks,tolerance=1,requirement='paint-image-resampling')


def area_threshold_cases():
    # Equal counts of adjacent 8-bit levels give the exact rational mean
    # (2*lo+1)/2. The independent half-up oracle is therefore lo+1.
    for n, lo, tolerance in [(1024,159,1),(2048,95,0),(2048,223,0),(2048,253,0)]:
        image = stream(zlib.compress(bytes([lo,lo+1])*(n*n//2)),
                       b'/Subtype /Image /Width %d /Height %d /BitsPerComponent 8 /ColorSpace /DeviceGray /Filter /FlateDecode' % (n,n))
        yield case('image-area-tie-%d-%d'%(n,lo),b'/I Do',[image],
                   b'<< /XObject << /I 5 0 R >> >>',size=1,
                   oracle=b'P6\n1 1\n255\n'+bytes([lo+1])*3,tolerance=tolerance,
                   requirement='paint-image-resampling')
    # Its exact covered area is 10^-307, below half a coverage level.
    # Sampling is never requested, even if source-coordinate scaling overflows.
    image = stream(bytes(2048),b'/Subtype /Image /Width 2048 /Height 1 /BitsPerComponent 8 /ColorSpace /DeviceGray')
    yield case('image-area-zero-coverage',b'q 0.'+b'0'*306+b'1 0 0 1 0 0 cm /I Do Q',
               [image],b'<< /XObject << /I 5 0 R >> >>',size=1,
               requirement='paint-image-resampling')
    # Parameterized sampling fixtures keep the large, repeated source planes
    # implicit. Columns alternate lo and lo+1; every row is identical. These
    # cover all half-level differences seen in the exhaustive 64/1024/2048
    # square sweep without storing hundreds of redundant image streams.
    from .model import Vector
    values = []
    for n, levels in [(1024, AREA_TIES_1024), (2048, AREA_TIES_2048),
                      (4096, (31,159,191,223)), (8192, (159,191,223))]:
        values.extend('%d %d %d' % (n,lo,lo+1) for lo in levels)
    yield Vector('image-area-thresholds',{'.txt': ('# side low half_up_oracle; alternating low/high columns\n'+'\n'.join(values)+'\n').encode()},
                 ('paint-image-resampling',),'Exact rational means of alternating adjacent levels at large reduction ratios')

AREA_TIES_1024 = (
    70, 73, 74, 75, 76, 77, 78, 90, 91, 92, 93, 94,
    133, 134, 135, 136, 137, 138, 139, 140, 141, 142, 143, 144,
    145, 146, 147, 148, 149, 150, 151, 152, 153, 154, 155, 156,
    157, 158, 159, 170, 171, 172, 173, 174, 175, 176, 177, 178,
    179, 180, 181, 182, 183, 184, 185, 186, 187, 188, 189, 190,
)

AREA_TIES_2048 = (
    13, 14, 23, 24, 25, 26, 27, 28, 29, 30, 43, 44,
    45, 46, 47, 48, 49, 50, 51, 52, 53, 54, 55, 56,
    57, 58, 59, 60, 61, 62, 83, 84, 85, 86, 87, 88,
    89, 90, 91, 92, 93, 94, 95, 96, 97, 98, 99, 100,
    101, 102, 103, 104, 105, 106, 107, 108, 109, 110, 111, 112,
    113, 114, 115, 116, 117, 118, 119, 120, 121, 122, 123, 124,
    125, 126, 186, 187, 188, 189, 190, 203, 204, 205, 206, 207,
    208, 209, 210, 211, 212, 213, 214, 215, 216, 217, 218, 219,
    220, 221, 222, 223, 224, 225, 226, 227, 228, 229, 230, 231,
    232, 233, 234, 235, 236, 237, 238, 239, 240, 241, 242, 243,
    244, 245, 246, 247, 248, 249, 250, 251, 252, 253, 254,
)


def render_vector(name, pdf, oracle, requirement, description, *, repairs=(), information=(), strict_valid=True, tolerance=0, size=SIZE):
    """PDF oracle handled by recovery/provider tests rather than the strict suite."""
    import hashlib
    from .model import Vector
    return Vector(name,{'.pdf':pdf,'.rgb':oracle},(requirement,),description,
                  extra=dict(kind='pdf',expected=RENDER,path=f'corpus/generated/paint/{name}.pdf',
                             sha256=hashlib.sha256(pdf).hexdigest(),pages=1,strict_valid=strict_valid,
                             repairs=list(repairs),**({"information":list(information)} if information else {}),oracle=f'corpus/generated/paint/{name}.rgb',
                             oracle_sha256=hashlib.sha256(oracle).hexdigest(),dpi=72,
                             width=size,height=size,tolerance=tolerance))


def repair_cases():
    tag = 'content.text-clip-reset'
    yield render_vector('text-clip-reset',page(b'BT 7 Tr 0 Tr ET'),rects([]),
                        'paint-text-clip-recovery','A nonclipping Tr before ET is reported even without glyphs',
                        repairs=(tag,),strict_valid=False)
    yield render_vector('text-clip-reset-retained',
                        page(b'1 0 0 rg BT /F 10 Tf 2 2 Td 7 Tr (A) Tj 0 Tr 10 0 Td (A) Tj ET 0 1 0 rg 0 0 32 32 re f',ttf(),FONT),
                        rects([((2,2,4,6),GREEN),((12,2,4,6),RED)]),
                        'paint-text-clip-recovery','Clipping accumulated before a reset survives ET; later nonclipping text paints before ET',
                        repairs=(tag,),strict_valid=False)
    yield render_vector('text-clip-reset-reentered',
                        page(b'BT /F 10 Tf 2 2 Td 7 Tr (A) Tj 3 Tr (A) Tj 7 Tr (A) Tj 0 Tr ET 0 0 32 32 re f',ttf(),FONT),
                        rects([((2,2,4,6),BLACK),((10,2,4,6),BLACK)]),
                        'paint-text-clip-recovery','Reentering a clipping mode appends to the earlier clip; each reset is positioned',
                        repairs=(tag,tag),strict_valid=False)
    yield render_vector('text-clip-reset-restored',
                        page(b'BT /F 10 Tf 2 2 Td q 7 Tr (A) Tj Q ET 0 0 32 32 re f',ttf(),FONT),
                        rects([((2,2,4,6),BLACK)]),
                        'paint-text-clip-recovery','Q restoring a nonclipping mode also reports the reset and preserves the accumulated clip',
                        repairs=(tag,),strict_valid=False)
    yield render_vector('text-clip-reset-inherited',
                        page(b'7 Tr BT /F 10 Tf 2 2 Td (A) Tj 0 Tr ET 0 0 32 32 re f',ttf(),FONT),
                        rects([((2,2,4,6),BLACK)]),
                        'paint-text-clip-recovery','A clipping mode inherited at BT also forbids a nonclipping reset before ET',
                        repairs=(tag,),strict_valid=False)
    # The first appearance must never execute: its unresolved shading resource
    # would fail if entered. A following valid appearance proves
    # that skipping preserves page graphics and annotation iteration.
    for name, box, matrix, rect in [
        ('bbox-width',b'[0 0 0 4]',b'[1 0 0 1 0 0]',b'[2 2 10 14]'),
        ('indirect',b'[0 0 0 4]',b'[1 0 0 1 0 0]',b'[2 2 10 14]'),
        ('bbox-height',b'[0 0 4 0]',b'[1 0 0 1 0 0]',b'[2 2 10 14]'),
        ('bbox-point',b'[0 0 0 0]',b'[1 0 0 1 0 0]',b'[2 2 10 14]'),
        ('bbox-rotated',b'[0 0 0 4]',b'[0 1 -1 0 12 3]',b'[2 2 10 14]'),
        ('rect-width',b'[0 0 2 4]',b'[1 0 0 1 0 0]',b'[2 2 2 14]'),
        ('rect-height',b'[0 0 2 4]',b'[1 0 0 1 0 0]',b'[2 2 10 2]'),
        ('rect-point',b'[0 0 2 4]',b'[1 0 0 1 0 0]',b'[2 2 2 2]'),
    ]:
        objects = [
            b'<< /Type /Catalog /Pages 2 0 R >>',
            b'<< /Type /Pages /Kids [3 0 R] /Count 1 >>',
            b'<< /Type /Page /Parent 2 0 R /MediaBox [0 0 32 32] /Contents 4 0 R /Annots [<< /Subtype /Stamp /Rect '+rect+b' /AP << /N 5 0 R >> >> << /Subtype /Stamp /Rect [20 4 24 12] /AP << /N 6 0 R >> >>] >>',
            stream(b'0 0 1 rg 0 0 32 32 re f'),
            stream(b'/Unavailable sh',b'/Type /XObject /Subtype /Form /BBox '+box+b' /Matrix '+matrix+b' /Resources << >>'),
            stream(b'1 0 0 rg 0 0 2 4 re f',b'/Type /XObject /Subtype /Form /BBox [0 0 2 4] /Resources << >>'),
        ]
        if name == 'indirect':
            annotation = b'<< /Subtype /Stamp /Rect '+rect+b' /AP << /N 5 0 R >> >>'
            objects[2] = objects[2].replace(annotation,b'7 0 R')
            objects.append(annotation)
        yield render_vector('annotation-empty-'+name,document(objects),
                            rects([((0,0,32,32),BLUE),((20,4,4,8),RED)]),
                            'paint-appearance-recovery','Zero extent prevents appearance fitting; skip it and retain the following annotation',
                            repairs=('annotation.appearance-empty',),strict_valid=False)


def cjk_substitute_case():
    """Feature-gated CID substitution, independently selected through data files."""
    from pathlib import Path
    from .model import Vector
    from .text import bundled_cmap, bundled_unicode
    from .paint_cff import otf_contours, outline_oracle
    root = Path(__file__).resolve().parents[2]/'data'
    folder = root/'cmaps/Adobe-Japan1-7'
    inventory = {p.name:p for p in folder.iterdir()}
    codes = (0x4e00, 0x3042)  # CJK ideograph ONE and hiragana A.
    mapping, mode, _, _ = bundled_cmap(inventory['UniJIS-UCS2-H'],inventory,{})
    assert mode == 0
    cids = [mapping[2,code] for code in codes]
    unicode = bundled_unicode(folder/'cid2unicode.txt')
    scalars = [ord(unicode[cid]) for cid in cids]
    outline = root/'cjk/SourceHanSansJP-Regular.otf'
    oracle = outline_oracle([otf_contours(outline,scalar) for scalar in scalars],[1100,923])
    font = b'<< /Type /Font /Subtype /Type0 /BaseFont /Collection /Encoding /UniJIS-UCS2-H /DescendantFonts [6 0 R] >>'
    cidfont = b'<< /Type /Font /Subtype /CIDFontType0 /BaseFont /Collection /CIDSystemInfo << /Registry (Adobe) /Ordering (Japan1) /Supplement 7 >> /FontDescriptor 7 0 R /DW 923 /W [%d [1100]] >>' % cids[0]
    descriptor = b'<< /Type /FontDescriptor /Flags 4 /FontName /Collection /FontBBox [-100 -300 1200 1000] /ItalicAngle 0 /Ascent 800 /Descent -200 /CapHeight 700 /StemV 80 >>'
    pdf = page(b'BT /F 24 Tf 3 8 Td <'+b''.join(b'%04x'%c for c in codes)+b'> Tj ET',
               [font,cidfont,descriptor],FONT,64)
    return Vector('bundled-cjk-substitute',{'.pdf':pdf,'.rgb':oracle},('paint-bundled-cjk-substitute',),
                  'Opt-in Source Han Sans: predefined CMap to CID to collection Unicode to OTF cmap and CID CFF outlines',
                  extra={'driver':'bundled-cjk-paint','cargo_feature':'cjk-fonts','tolerance':1})


def image_geometry_cases():
    from dataclasses import replace
    for suffix, columns, width, black in [('crop', 13, 9, False), ('pad', 9, 13, False), ('pad-inverted', 9, 13, True)]:
        rows = [[int((x+2*y) % 4 < 2) for x in range(columns)] for y in range(3)]
        encoded, _ = codecs.ccitt(rows, k=-1, black_is_1=black)
        logical = [row[:width]+[0]*max(0, width-columns) for row in rows]
        expected = [255*(1-v) for row in logical for v in row]
        dictionary = b'/Filter /CCITTFaxDecode /ColorSpace /DeviceGray /BitsPerComponent 1 /DecodeParms << /K -1 /Columns %d /BlackIs1 %s >>' % (columns, b'true' if black else b'false')
        if black:
            dictionary += b' /Decode [1 0]'
        result = image_case('image-fax-grid-'+suffix, encoded, dictionary, expected, width, 3, requirement='image-ccitt-columns')
        yield replace(result, repairs=('image.ccitt-columns',), strict_valid=False)

    rows = [[0, 1, 1, 0, 1], [1, 0, 0, 1, 0]]
    encoded, _ = codecs.ccitt(rows, k=-1)
    content = b'q 14 0 0 4 2 2 cm BI /W 7 /H 2 /BPC 1 /CS /G /F /CCF /DP << /K -1 /Columns 5 >> ID '+encoded+b' EI Q'
    rectangles = [((2+2*x, 2+2*(1-y), 2, 2), (255*(1-v),)*3)
                  for y, row in enumerate(rows) for x, v in enumerate(row+[0, 0])]
    result = case('image-inline-fax-grid', content, rectangles=rectangles, requirement='image-ccitt-columns')
    yield replace(result, repairs=('image.ccitt-columns',), strict_valid=False)

    # Table 5's zero-filter array is valid inline, with deterministic raw
    # framing even when image samples themselves spell an EI operator.
    for suffix, space, width, data, colors in [
        ('cmyk', b'/CMYK', 2, bytes(7)+b'\xff', [WHITE, BLACK]),
        ('embedded-ei', b'/G', 4, b' EI ', [(v,)*3 for v in b' EI ']),
    ]:
        content = b'q %d 0 0 2 2 2 cm BI /W %d /H 1 /BPC 8 /CS ' % (width*2, width)+space+b' /F [] ID '+data+b' EI Q 1 0 0 rg 2 10 4 2 re f'
        rectangles = [((2+2*x, 2, 2, 2), color) for x, color in enumerate(colors)]
        rectangles.append(((2, 10, 4, 2), RED))
        yield case('image-inline-empty-filter-'+suffix, content, rectangles=rectangles, requirement='image-inline-filter')
