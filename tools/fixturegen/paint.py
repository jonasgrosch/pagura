"""Independent text and image painting oracles (ISO 32000-2 clauses 8.9 and 9).

Rectangle glyphs use analytic rectangles; triangle glyphs use rational polygon
intersection. Codec payloads come from the independent format encoders. Affine
image oracles explicitly map source sample rectangles into page coordinates.
No product executable is imported or run.
"""
from fractions import Fraction as F
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
        ("text-clip-reset",b"BT 7 Tr 0 Tr ET",[]),
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


def cases():
    yield from text_cases()
    yield from image_cases()
    yield from extra_cases()


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
    yield Case("type3-inconsistent-width",page(b"BT /F 20 Tf (A) Tj ET",objects,FONT),OPEN,("paint-text-errors",),"CharProc width disagrees with Widths",extra={"render_error":"syntax"})
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

    for name,data,dictionary,fill,tag in [
        ("image-jpx-icc-explicit",jpx.jp2(jpx.codestream([[0]],1,1),1,1,1,method=2),b"/Filter /JPXDecode",b"","image.embedded-color"),
        ("image-stencil-pattern-explicit",b"\0",b"/ImageMask true /BitsPerComponent 1",b"/Pattern cs ","image.stencil-pattern"),
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
