"""Independent PDF recovery and sampled-function oracles (7.4, 7.10, 12.7).

All cubic expectations use rational Hermite evaluation with centred interior
and one-sided endpoint tangents, independently of the renderer's tensor weights.
"""
from fractions import Fraction as F
import struct
import zlib
from .model import Case, RENDER, OPEN, UNSUPPORTED
from .pdf import document, stream, opaque_rectangles
from . import image, jpx, jbig2

AREA = "recovery"
WHITE = (255, 255, 255)
BLACK = (0, 0, 0)


def page(content, objects=(), resources=b"<< >>", extra=b"", size=16, catalog=b""):
    return document([
        b"<< /Type /Catalog /Pages 2 0 R " + catalog + b">>",
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 %d %d] /Resources " % (size, size)
        + resources + b" /Contents 4 0 R " + extra + b" >>",
        stream(content), *objects,
    ])


def render(name, content, objects=(), resources=b"<< >>", rectangles=(), *,
           requirement="recovery-content", repairs=(), extra=b"", size=16, oracle=None, tolerance=0,
           catalog=b""):
    return Case(name, page(content, objects, resources, extra, size, catalog), RENDER,
                (requirement,), name.replace("-", " "),
                oracle=oracle if oracle is not None else opaque_rectangles(size, size, rectangles),
                repairs=repairs, strict_valid=not repairs, extra={"tolerance": tolerance})


def spline(values, x):
    """Local cubic Hermite interpolation, with linear fallback for small axes."""
    x = min(len(values)-1, max(F(0), x))
    low = min(int(x), len(values)-2)
    if len(values) == 1:
        return values[0]
    t = x-low
    if len(values) < 4:
        return (1-t)*values[low] + t*values[low+1]
    def slope(i):
        if i == 0: return values[1]-values[0]
        if i == len(values)-1: return values[-1]-values[-2]
        return (values[i+1]-values[i-1])/2
    return ((2*t**3-3*t**2+1)*values[low] + (t**3-2*t**2+t)*slope(low)
            + (-2*t**3+3*t**2)*values[low+1] + (t**3-t**2)*slope(low+1))


def tensor(values, sizes, point):
    # First dimension varies fastest (7.10.2). Reduce one whole axis at a time.
    for size, x in zip(sizes, point):
        values = [spline(values[i:i+size], x) for i in range(0, len(values), size)]
    return values[0]


def sampled_cases():
    examples = [
        ("cubic-interior", [4], [0, 20, 220, 255], 8, [F(0),F(1,4),F(1,2),F(3,4),F(1),F(5,4),F(3,2),F(7,4),F(2),F(9,4),F(5,2),F(11,4),F(3)], None, [F(0),F(1)]),
        ("cubic-clipped", [4], [255,0,0,255], 8, [F(1),F(5,4),F(3,2),F(7,4),F(2)], None, [F(0),F(1)]),
        ("cubic-12-reversed", [4], [0,1024,3072,4095], 12, [F(0),F(1,8),F(1,4),F(1,2),F(3,4),F(1)], [F(3),F(0)], [F(1),F(0)]),
        ("cubic-32", [4], [0,1<<29,1<<31,(1<<32)-1], 32, [F(0),F(1,2),F(1),F(3,2),F(2),F(5,2),F(3)], None, [F(0),F(1)]),
        ("cubic-two-inputs", [4,4], [x*x*8+y*y*12 for y in range(4) for x in range(4)], 8, [(F(1,4),F(1,2)),(F(3,2),F(5,4)),(F(11,4),F(5,2)),(F(3),F(3))], None, [F(0),F(1)]),
        ("cubic-mixed-linear", [4,2], [x*x*10+y*100 for y in range(2) for x in range(4)], 8, [(F(1,2),F(1,4)),(F(3,2),F(3,4)),(F(5,2),F(1,2))], None, [F(0),F(1)]),
        ("cubic-constant-many-inputs", [1]*32, [128], 8, [tuple([F(0)]*32)], None, [F(0),F(1)]),
        ("cubic-single-axis", [1,4], [0,32,200,255], 8, [(F(0),F(1,2)),(F(0),F(3,2)),(F(0),F(5,2))], None, [F(0),F(1)]),
    ]
    for name, sizes, values, bits, points, encode, decode in examples:
        points = [(x,) if not isinstance(x,tuple) else x for x in points]
        max_sample = (1<<bits)-1
        bitstring = ''.join(f'{v:0{bits}b}' for v in values)
        bitstring += '0'*(-len(bitstring)%8)
        data = int(bitstring,2).to_bytes(len(bitstring)//8, 'big')
        # With Encode absent, Domain matches the grid coordinates, including a
        # singleton's degenerate domain. Explicit Encode maps [0,1] backwards.
        domain = [0,1]*len(sizes)
        params = b'/FunctionType 0 /Domain [' + b' '.join(str(x).encode() for x in domain) + b'] /Range [0 1] /Order 3 /BitsPerSample ' + str(bits).encode() + b' /Size [' + b' '.join(str(n).encode() for n in sizes) + b']'
        if encode: params += b' /Encode [' + b' '.join(str(x).encode() for x in encode) + b']'
        params += b' /Decode [' + b' '.join(str(x).encode() for x in decode) + b']'
        space = b'[/Separation /Ink /DeviceGray 5 0 R]' if len(sizes)==1 else b'[/DeviceN ['+b' '.join(b'/I'+str(i).encode() for i in range(len(sizes)))+b'] /DeviceGray 5 0 R]'
        rectangles=[]; content=b'/C cs '
        for i, point in enumerate(points):
            position = [(encode[0]+x*(encode[1]-encode[0])) if encode else x for x in point]
            value = tensor([F(v,max_sample) for v in values],sizes,position)
            value = decode[0]+value*(decode[1]-decode[0])
            byte = int(255*min(1,max(0,value))+F(1,2))
            arguments = point if encode else [x/(n-1) if n>1 else F(0) for x,n in zip(point,sizes)]
            content += b' '.join(str(float(x)).encode() for x in arguments) + b' scn %d 0 1 16 re f ' % i
            rectangles.append(((i,0,1,16),(byte,)*3))
        yield render(name,content,[stream(data,params)],b'<< /ColorSpace << /C '+space+b' >> >>',rectangles,requirement='recovery-cubic')

    samples=[(0,255,17),(21,200,245),(219,40,33),(255,0,201)]
    data=b''.join(struct.pack('>H',v*257) for row in samples for v in row)
    function=stream(data,b'/FunctionType 0 /Domain [0 1] /Range [0 1 0 1 0 1] /Order 3 /BitsPerSample 16 /Size [4]')
    content=b'/C cs ';rectangles=[]
    for i in range(9):
        x=F(i,8)
        color=tuple(int(min(255,max(0,spline([F(row[c]) for row in samples],x*3)))+F(1,2)) for c in range(3))
        content+=str(float(x)).encode()+b' scn %d 0 1 16 re f '%i
        rectangles.append(((i,0,1,16),color))
    yield render('cubic-rgb-16',content,[function],b'<< /ColorSpace << /C [/Separation /Ink /DeviceRGB 5 0 R] >> >>',rectangles,requirement='recovery-cubic')


def widget_cases():
    for name, field, parent, rectangles in [
        ('widget-empty-text', b'/FT /Tx', b'', []),
        ('widget-empty-signature', b'/FT /Sig', b'', []),
        ('widget-parent-null', b'/FT /Tx /Parent null', b'', []),
        ('widget-parent-undefined', b'/FT /Tx /Parent 99 0 R', b'', []),
        ('widget-inherited-empty', b'/Parent 6 0 R', b'<< /FT /Tx /V () >>', []),
        ('widget-styled-empty', b'/FT /Tx /MK << /BG [1 0 0] /BC [0] >> /BS << /W 2 >>', b'', [((1,1,6,6),BLACK),((3,3,2,2),(255,0,0))]),
        ('widget-no-border-color', b'/FT /Tx /MK << /BG [1 0 0] >> /BS << /W 2 >>', b'', [((1,1,6,6),(255,0,0))]),
        ('widget-rotated-background', b'/FT /Tx /MK << /R 90 /BG [1 0 0] >>', b'', [((1,1,6,6),(255,0,0))]),
    ]:
        objects=[b'<< /Type /Annot /Subtype /Widget /Rect [1 1 7 7] '+field+b' >>']
        if parent: objects.append(parent)
        yield render(name,b'',objects,rectangles=rectangles,extra=b'/Annots [5 0 R]',requirement='recovery-widgets')
    # 12.7.4.5: an unsigned signature field in the form, without an appearance;
    # its MK background is generated as for any other widget, and filling an
    # untouched form must not change that.
    yield render('widget-unsigned-signature-background',b'',
                 [b'<< /Type /Annot /Subtype /Widget /FT /Sig /T (Approval) /Rect [1 1 7 7] /MK << /BG [1 0 0] >> >>'],
                 rectangles=[((1,1,6,6),(255,0,0))],extra=b'/Annots [5 0 R]',
                 catalog=b'/AcroForm << /Fields [5 0 R] >> ',requirement='recovery-widgets')
    for name, field, parent, error in [
        ('widget-value', b'/FT /Tx /V (visible)', b'', 'unsupported'),
        ('widget-inherited-value', b'/FT /Tx /Parent 6 0 R', b'<< /V (visible) >>','unsupported'),
        ('widget-null-inherits-value', b'/FT /Tx /V null /Parent 6 0 R', b'<< /V (visible) >>','unsupported'),
        ('widget-signed', b'/FT /Sig /V << /Type /Sig >>', b'','unsupported'),
        ('widget-parent-cycle', b'/Parent 6 0 R', b'<< /Parent 5 0 R >>','cycle'),
        ('widget-rotated-border', b'/FT /Tx /MK << /R 90 /BC [0] >>', b'','unsupported'),
        ('widget-invalid-rotation', b'/FT /Tx /MK << /R 45 /BG [1 0 0] >>', b'','syntax'),
    ]:
        objects=[b'<< /Subtype /Widget /Rect [1 1 7 7] '+field+b' >>']
        if parent: objects.append(parent)
        yield Case(name,page(b'',objects,extra=b'/Annots [5 0 R]'),UNSUPPORTED if error=='unsupported' else OPEN,
                   ('recovery-widgets',),name,feature='annotation.appearance' if error=='unsupported' else None,
                   extra={} if error=='unsupported' else {'render_error':error})


def transfer_cases():
    inverse=b'<< /FunctionType 2 /Domain [0 1] /C0 [1] /C1 [0] /N 1 >>'
    resources=b'<< /ExtGState << /I << /TR '+inverse+b' >> /T << /ca .5 /CA .5 >> /Z << /ca 0 >> /N << /TR /Identity >> >> >>'
    # A Gray page group exercises the floating compositor, including opaque marks.
    extra=b'/Group << /S /Transparency /CS /DeviceGray /I true >>'
    for name, content, rectangles in [
        ('transfer-opaque',b'/I gs .25 g 0 0 8 16 re f', [((0,0,8,16),(191,)*3)]),
        ('transfer-translucent-default',b'/I gs /T gs .25 g 0 0 16 16 re f', [((0,0,16,16),(159,)*3)]),
        ('transfer-reset-by-topmost',b'/I gs .25 g 0 0 16 16 re f /T gs .75 g 0 0 8 16 re f', [((0,0,16,16),(191,)*3),((0,0,8,16),(128,)*3)]),
        ('transfer-identity-topmost',b'/I gs .25 g 0 0 16 16 re f /N gs .5 g 0 0 8 16 re f', [((0,0,16,16),(191,)*3),((0,0,8,16),(128,)*3)]),
        ('transfer-transparent-topmost',b'/I gs .25 g 0 0 16 16 re f /Z gs .75 g 0 0 8 16 re f', [((0,0,16,16),(191,)*3),((0,0,8,16),(64,)*3)]),
    ]:
        yield render(name, content, resources=resources, rectangles=rectangles, extra=extra,requirement='recovery-transfer',tolerance=1)
    form=stream(b'/I gs .25 g 0 0 16 16 re f',b'/Subtype /Form /BBox [0 0 16 16] /Resources '+resources+b' /Group << /S /Transparency /I true >>')
    for name,content,color in [('transfer-opaque-ancestor',b'/F Do',191),('transfer-translucent-ancestor',b'/T gs /F Do',159)]:
        yield render(name,content,[form],resources[:-2]+b' /XObject << /F 5 0 R >> >>',rectangles=[((0,0,16,16),(color,)*3)],extra=extra,requirement='recovery-transfer',tolerance=1)
    data=stream(bytes([64]),b'/Type /XObject /Subtype /Image /Width 1 /Height 1 /BitsPerComponent 8 /ColorSpace /DeviceGray')
    yield render('transfer-image',b'/I gs 16 0 0 16 0 0 cm /X Do',[data],resources[:-2]+b' /XObject << /X 5 0 R >> >>',[((0,0,16,16),(191,)*3)],extra=extra,requirement='recovery-transfer')

    shading=b'<< /ShadingType 2 /ColorSpace /DeviceGray /Coords [0 0 16 0] /Extend [true true] /Function << /FunctionType 2 /Domain [0 1] /C0 [.25] /C1 [.25] /N 1 >> >>'
    yield render('transfer-shading',b'/I gs /S sh',[shading],resources[:-2]+b' /Shading << /S 5 0 R >> >>',[((0,0,16,16),(191,)*3)],extra=extra,requirement='recovery-transfer')
    for name,mask_transfer,value in [('transfer-soft-mask-source',b'',207),('transfer-soft-mask-function',b'/TR '+inverse,112)]:
        mask=stream(b'/I gs .25 g 0 0 16 16 re f',b'/Subtype /Form /BBox [0 0 16 16] /Group << /S /Transparency /CS /DeviceGray /I true >> /Resources '+resources)
        states=resources.replace(b'/ExtGState <<',b'/ExtGState << /M << /SMask << /S /Luminosity /G 5 0 R '+mask_transfer+b' >> >>')
        yield render(name,b'/I gs /M gs .25 g 0 0 16 16 re f',[mask],states,[((0,0,16,16),(value,)*3)],extra=extra,requirement='recovery-transfer',tolerance=1)
    # Mask artwork cannot select page transfers, including its patterns and
    # overprint state. Luminosity .25 gives .25*.25 + .75*1 = .8125.
    states=resources.replace(b'/ExtGState <<',b'/ExtGState << /M << /SMask << /S /Luminosity /G 5 0 R >> >>')
    cell=stream(b'.25 g 0 0 2 2 re f',b'/Type /Pattern /PatternType 1 /PaintType 1 /TilingType 1 /BBox [0 0 2 2] /XStep 2 /YStep 2 /Resources << >>')
    for name,content,mask_resources,objects in [
        ('pattern',b'/Pattern cs /P scn 0 0 16 16 re f',b'<< /Pattern << /P 6 0 R >> >>',[cell]),
        ('overprint',b'/O gs .25 g 0 0 16 16 re f',b'<< /ExtGState << /O << /op true >> >> >>',[]),
    ]:
        mask=stream(content,b'/Subtype /Form /BBox [0 0 16 16] /Group << /S /Transparency /CS /DeviceGray /I true >> /Resources '+mask_resources)
        yield render('transfer-soft-mask-'+name,b'/I gs /M gs .25 g 0 0 16 16 re f',[mask,*objects],states,[((0,0,16,16),(207,)*3)],extra=extra,requirement='recovery-transfer',tolerance=1)

    # A transfer present only in a pattern cell must not disappear. Pattern
    # transfer-region selection is explicitly unsupported, also through groups.
    for suffix,content,cell_resources,objects in [
        ('direct',b'/I gs .25 g 0 0 2 2 re f',resources,[]),
        ('group',b'/F Do',b'<< /XObject << /F 6 0 R >> >>',
         [stream(b'/I gs .25 g 0 0 2 2 re f',b'/Subtype /Form /BBox [0 0 2 2] /Group << /S /Transparency /I true >> /Resources '+resources)]),
    ]:
        pattern=stream(content,b'/Type /Pattern /PatternType 1 /PaintType 1 /TilingType 1 /BBox [0 0 2 2] /XStep 2 /YStep 2 /Resources '+cell_resources)
        yield Case('transfer-pattern-cell-'+suffix,page(b'/Pattern cs /P scn 0 0 16 16 re f',[pattern,*objects],b'<< /Pattern << /P 5 0 R >> >>',extra),UNSUPPORTED,('recovery-transfer',),'Transfers inside pattern cells require explicit pattern-region support',feature='transparency.transfer')
    mask=stream(bytes([128]),b'/Type /XObject /Subtype /Image /Width 1 /Height 1 /BitsPerComponent 8 /ColorSpace /DeviceGray')
    masked=data.replace(b'/Length 1 ',b'/SMask 6 0 R /Length 1 ')
    yield render('transfer-soft-mask-image',b'/I gs 16 0 0 16 0 0 cm /X Do',[masked,mask],resources[:-2]+b' /XObject << /X 5 0 R >> >>',[((0,0,16,16),(159,)*3)],extra=extra,requirement='recovery-transfer',tolerance=1)
    for suffix,value,objects in [('direct',b'[/Identity /Identity /Identity /Identity]',[]),('indirect',b'[5 0 R 5 0 R 5 0 R 5 0 R]',[b'/Identity'])]:
        states=b'<< /ExtGState << /I << /TR '+value+b' /op true /ca .5 >> >> >>'
        yield render('transfer-identity-array-'+suffix,b'/I gs .25 g 0 0 16 16 re f',objects,states,[((0,0,16,16),(159,)*3)],extra=extra,requirement='recovery-transfer',tolerance=1)
    yield Case('transfer-fractional-region',page(b'/I gs .25 g 0 0 .5 16 re f',resources=resources,extra=extra),UNSUPPORTED,('recovery-transfer',),'Fractional transfer boundaries require spatial colour correlation',feature='transparency.transfer')


def codec_cases():
    resources=b'<< /XObject << /X 5 0 R >> >>'
    params=b'/Type /XObject /Subtype /Image /Width 4 /Height 1 /BitsPerComponent 8 /ColorSpace /DeviceGray '
    codes=[0,64,128,255,257]; packed=''.join(f'{code:09b}' for code in codes);packed+='0'*(-len(packed)%8)
    yield render('lzw-no-initial-clear',b'4 0 0 1 0 0 cm /X Do',[stream(int(packed,2).to_bytes(len(packed)//8,'big'),params+b'/Filter /LZWDecode')],resources,[((i,0,1,1),(v,)*3) for i,v in enumerate(codes[:-1])],repairs=('filter.lzw-initial-clear',),requirement='recovery-codecs')
    # Empty filter chains and one-name device space arrays are conforming syntax.
    yield render('inline-empty-filters',b'4 0 0 1 0 0 cm BI /W 4 /H 1 /BPC 8 /CS /G /F [] ID '+bytes([0,64,128,255])+b' EI',rectangles=[((i,0,1,1),(v,)*3) for i,v in enumerate(codes[:-1])],requirement='recovery-image-syntax')
    yield render('image-device-array',b'4 0 0 1 0 0 cm /X Do',[stream(bytes([0,64,128,255]),params.replace(b'/DeviceGray',b'[/DeviceGray]'))],resources,[((i,0,1,1),(v,)*3) for i,v in enumerate(codes[:-1])],requirement='recovery-image-syntax')
    jpeg,expected=image.constant_jpeg()
    yield render('jpeg-trailing-restart',b'8 0 0 8 0 0 cm /X Do',[stream(jpeg[:-2]+b'\xff\xd2'+jpeg[-2:],b'/Type /XObject /Subtype /Image /Width 1 /Height 1 /BitsPerComponent 8 /ColorSpace /DeviceGray /Filter /DCTDecode')],resources,[((0,0,8,8),(expected[0],)*3)],repairs=('jpeg.trailing-restart',),requirement='recovery-codecs',tolerance=1)
    rows=[[0,1,0,1,1,0,1,0,1,1] for _ in range(2)]
    fax,_=image.ccitt(rows,k=-1)
    yield render('ccitt-cropped-columns',b'8 0 0 2 0 0 cm /X Do',[stream(fax,b'/Type /XObject /Subtype /Image /Width 8 /Height 2 /BitsPerComponent 1 /ColorSpace /DeviceGray /Filter /CCITTFaxDecode /DecodeParms << /K -1 /Columns 10 >>')],resources,[((x,y,1,1),(255*(1-v),)*3) for y in range(2) for x,v in enumerate(rows[0][:8])],repairs=('image.ccitt-columns',),requirement='recovery-codecs')
    raw=bytes(10)
    yield render('predictor-image-precision',b'4 0 0 2 0 0 cm /X Do',[stream(zlib.compress(raw),params.replace(b'/Height 1',b'/Height 2')+b'/Filter /FlateDecode /DecodeParms << /Predictor 15 /Columns 4 /BitsPerComponent 4 /Colors 1 >>')],resources,[((0,0,4,2),BLACK)],repairs=('image.predictor-precision',),requirement='recovery-codecs')
    predictor_params=params+b'/Filter /FlateDecode /DecodeParms << /Predictor 15 /Columns 4 /BitsPerComponent 4 /Colors 1 >>'
    # Six bytes make complete rows at the declared predictor precision. The
    # resulting four sample bytes already match the image, so no retry applies.
    yield render('predictor-unusual-valid',b'4 0 0 1 0 0 cm /X Do',[stream(zlib.compress(bytes([0,0,64,0,128,255])),predictor_params)],resources,[((i,0,1,1),(v,)*3) for i,v in enumerate([0,64,128,255])],requirement='recovery-codecs')
    # Original framing is incomplete; the alternate precision exposes an
    # invalid PNG tag. Preserve the original failure without a stale repair.
    failed=bytes(5)+b'\x05'+bytes(4)
    yield Case('predictor-retry-invalid-tag',page(b'4 0 0 1 0 0 cm /X Do',[stream(zlib.compress(failed),predictor_params)],resources),OPEN,('recovery-codecs',),'A failed predictor retry leaves the original error and diagnostics',extra={'render_error':'syntax'})
    values=[x*16+y*8 for y in range(4) for x in range(4)]
    code=jpx.codestream([values],4,4,levels=1,tile_parts=True)
    # Length-delimited parts have a consistent zero-based order, but the writer
    # accidentally stores the last part's index as the number of parts.
    code=bytearray(code)
    for i in range(len(code)-12):
        if code[i:i+2]==b'\xff\x90': code[i+11]-=1
    encoded=jpx.jp2(bytes(code),4,4,1)
    yield render('jpx-part-count',b'4 0 0 4 0 0 cm /X Do',[stream(encoded,b'/Type /XObject /Subtype /Image /Width 4 /Height 4 /ColorSpace /DeviceGray /Filter /JPXDecode')],resources,[((x,3-y,1,1),(values[y*4+x],)*3) for y in range(4) for x in range(4)],repairs=('jpx.tile-part-count',),requirement='recovery-codecs')

    palette=bytes([0,0,0, 255,0,0, 0,255,0, 0,0,255])
    container_palette=jpx.box(b'pclr',struct.pack('>HB',4,3)+bytes([7,7,7])+bytes([255,255,255])*4)
    mapping=jpx.box(b'cmap',b''.join(struct.pack('>HBB',0,1,i) for i in range(3)))
    code=jpx.codestream([[0,1,2,3]],4,1)
    encoded=jpx.jp2(code,4,1,1,extra=container_palette+mapping)
    params=b'/Type /XObject /Subtype /Image /Width 4 /Height 1 /Filter /JPXDecode /ColorSpace [/Indexed /DeviceRGB 3 <'+palette.hex().encode()+b'>]'
    yield render('jpx-pdf-palette',b'4 0 0 1 0 0 cm /X Do',[stream(encoded,params)],resources,[((i,0,1,1),tuple(palette[3*i:3*i+3])) for i in range(4)],requirement='recovery-image-syntax')

    malformed=jpx.jp2(code,4,1,1,extra=container_palette)
    index=malformed.index(b'jp2c')-4
    malformed=malformed[:index]+mapping+malformed[index:]
    yield render('jpx-misplaced-mapping',b'4 0 0 1 0 0 cm /X Do',[stream(malformed,params)],resources,[((i,0,1,1),tuple(palette[3*i:3*i+3])) for i in range(4)],requirement='recovery-codecs',repairs=('jpx.mapping-box-location',))

    params=b'/Subtype /Image /Width 4 /Height 1 /BitsPerComponent 8 /ColorSpace /DeviceGray'
    values=bytes([0,64,128,255])
    yield render('image-xobject-type',b'4 0 0 1 0 0 cm /X Do',[stream(values,b'/Type /XObjcect '+params)],resources,[((i,0,1,1),(v,)*3) for i,v in enumerate(values)],requirement='recovery-image-syntax',repairs=('image.xobject-type',))
    parameters=b'/Type /XObject '+params+b'/Filter /DCTDecode /DecodeParms << /Predictor 15 /Columns 4 /Colors 1 /BitsPerComponent 8 >>'
    encoded=zlib.compress(b'\0'+values)
    yield render('image-flate-mislabeled-dct',b'4 0 0 1 0 0 cm /X Do',[stream(encoded,parameters)],resources,[((i,0,1,1),(v,)*3) for i,v in enumerate(values)],requirement='recovery-codecs',repairs=('image.filter-mismatch',))
    for name,data,params in [
        ('checksum',encoded[:-1]+bytes([encoded[-1]^1]),parameters),
        ('trailing',encoded+b'extra',parameters),
        ('length',zlib.compress(b'\0'+values+b'\0'),parameters),
        ('predictor',encoded,parameters.replace(b'/Columns 4',b'/Columns 2')),
    ]:
        yield Case('image-mislabeled-invalid-'+name,page(b'4 0 0 1 0 0 cm /X Do',[stream(data,params)],resources),OPEN,('recovery-codecs',),'Unproven alternate image encoding remains a syntax error',extra={'render_error':'syntax'})


def type3_case():
    font=b'<< /Type /Font /Subtype /Type3 /FontBBox [0 0 400 600] /FontMatrix [.001 0 0 .001 0 0] /FirstChar 65 /LastChar 65 /Widths [400] /Encoding << /Differences [65 /A] >> /CharProcs << /A 6 0 R >> /Resources << >> >>'
    procedure=stream(b'401 0 d0 0 0 400 600 re f')
    return render('type3-width-advance',b'BT /F 10 Tf 2 2 Td (AA) Tj ET',[font,procedure],b'<< /Font << /F 5 0 R >> >>',[((2,2,8,6),BLACK)],repairs=('font.type3-width',),requirement='recovery-type3')


def pattern_cases():
    params=b'/Type /Pattern /PatternType 1 /PaintType 1 /TilingType 1 /BBox [0 0 2 2] /XStep 2 /YStep 2 /Resources << >>'
    colored=stream(b'1 0 0 rg 0 0 1 2 re f',params)
    uncolored=stream(b'0 0 1 2 re f',params.replace(b'/PaintType 1',b'/PaintType 2'))
    resources=b'<< /ColorSpace << /C [/Pattern /DeviceRGB] >> /Pattern << /P 5 0 R /Q 6 0 R >> >>'
    yield render('pattern-coloured-base-no-operands',b'/C cs /P scn 0 0 16 16 re f',[colored,uncolored],resources,[((x,0,1,16),(255,0,0)) for x in range(0,16,2)],requirement='recovery-patterns')
    for name,content,rectangles in [
        ('pattern-coloured-underlying',b'/C cs 0 1 0 /P scn 0 0 16 16 re f',[((x,0,1,16),(255,0,0)) for x in range(0,16,2)]),
        ('pattern-underlying-retained',b'/C cs 0 1 0 /P scn 0 0 8 16 re f 0 0 1 /Q scn 8 0 8 16 re f',[((x,0,1,16),(255,0,0) if x<8 else (0,0,255)) for x in range(0,16,2)]),
    ]:
        yield render(name,content,[colored,uncolored],resources,rectangles,requirement='recovery-patterns',repairs=('content.pattern-underlying',))
    yield Case('pattern-wrong-unused-count',page(b'/C cs 0 1 /P scn',[colored,uncolored],resources),OPEN,('recovery-patterns',),'Unused pattern operands must still match the selected base',extra={'render_error':'syntax'})
    font=b'<< /Type /Font /Subtype /Type3 /FontBBox [0 0 400 600] /FontMatrix [.001 0 0 .001 0 0] /FirstChar 65 /LastChar 65 /Widths [400] /Encoding << /Differences [65 /A] >> /CharProcs << /A 6 0 R >> /Resources << >> >>'
    yield render('type3-vertical-metric',b'BT /F 10 Tf 2 2 Td (AA) Tj ET',[font,stream(b'400 17 d0 0 0 400 600 re f')],b'<< /Font << /F 5 0 R >> >>',[((2,2,8,6),BLACK)],requirement='recovery-type3',repairs=('font.type3-width',))


def cases():
    yield from jbig2_capability_cases()


    yield from raw_jpx_cases()
    yield from pattern_cases()
    yield type3_case()
    yield from sampled_cases()
    yield from widget_cases()
    yield from transfer_cases()
    yield from codec_cases()
    yield from type1_sparse_subr_cases()
    yield render('unclosed-graphics',b'q 1 0 0 rg 1 1 4 4 re f',rectangles=[((1,1,4,4),(255,0,0))],repairs=('content.graphics-unclosed',))
    yield render('unclosed-marked',b'/P <</MCID 0>> BDC 1 0 0 rg 1 1 4 4 re f',rectangles=[((1,1,4,4),(255,0,0))],repairs=('content.marked-unclosed',))
    # Printer PCLm output opens a sequence per strip and never closes one.
    yield render('unclosed-marked-deep',b'/P BMC '*400+b'1 0 0 rg 1 1 4 4 re f',rectangles=[((1,1,4,4),(255,0,0))],repairs=('content.marked-unclosed',))
    yield from type3_empty_bounds_cases()


def jbig2_capability_cases():
    """T.88 7.4.8.5: only the capability promises contradict complete data."""
    reference = [[int(x == y or x == 7-y) for x in range(8)] for y in range(4)]
    target = [[int(1 <= x <= 6 and 1 <= y <= 2) for x in range(8)] for y in range(4)]
    first = jbig2._segment(2,36,1,jbig2.region(reference,jbig2.arithmetic(reference,2)))
    refinement = jbig2._segment(3,42,1,jbig2.region(target,jbig2.refinement_payload(target,reference,1)),refs=(2,))
    rectangles = [((x,3-y,1,1),BLACK) for y,row in enumerate(target) for x,value in enumerate(row) if value]
    for name,flags in [('auxiliary',66),('refinement',96),('both',64)]:
        data = jbig2._segment(1,48,1,jbig2._page_info(8,4,flags))+first+refinement
        image_dict = b'/Type /XObject /Subtype /Image /Width 8 /Height 4 /BitsPerComponent 1 /ColorSpace /DeviceGray /Filter /JBIG2Decode'
        yield render('jbig2-page-capabilities-'+name,b'8 0 0 4 0 0 cm /X Do',
                     [stream(data,image_dict)],b'<< /XObject << /X 5 0 R >> >>',rectangles,
                     requirement='recovery-jbig2-page-capabilities',repairs=('jbig2.page-capabilities',))

    # T.88 Figures 14–15: SLTP shares the state whose reference centre
    # alone is one. A later explicit dot exposes an incorrectly reused state.
    centre = [[int(x==2 and y==2) for x in range(5)] for y in range(5)]
    dot = [[int(x==4 and y==4) for x in range(5)] for y in range(5)]
    prior = jbig2._segment(2,36,1,jbig2.region(centre,jbig2.arithmetic(centre,2)))
    for template in (0,1):
        refinement = jbig2._segment(3,42,1,jbig2.region(dot,jbig2.refinement_payload(dot,centre,template,typical=True)),refs=(2,))
        data = jbig2._segment(1,48,1,jbig2._page_info(5,5,64))+prior+refinement
        image_dict = b'/Type /XObject /Subtype /Image /Width 5 /Height 5 /BitsPerComponent 1 /ColorSpace /DeviceGray /Filter /JBIG2Decode'
        yield render(f'jbig2-page-capabilities-prediction-centre-t{template}',b'5 0 0 5 0 0 cm /X Do',
                     [stream(data,image_dict)],b'<< /XObject << /X 5 0 R >> >>',[((4,0,1,1),BLACK)],
                     requirement='recovery-jbig2-page-capabilities',repairs=('jbig2.page-capabilities',))


def raw_jpx_cases():
    """PDF 7.4.9 framing repair; colour is stated by the dictionary, never guessed."""
    resources = b'<< /XObject << /X 5 0 R >> >>'
    for name,planes,space in [
        ('gray',[[0,64,128,255]],b'/DeviceGray'),
        ('rgb',[[255,0,0,255],[0,255,0,255],[0,0,255,255]],b'/DeviceRGB'),
    ]:
        data = jpx.codestream(planes,4,1,levels=0)
        params = b'/Type /XObject /Subtype /Image /Width 4 /Height 1 /ColorSpace '+space+b' /Filter /JPXDecode'
        colors = [(v,)*3 for v in planes[0]] if len(planes)==1 else list(zip(*planes))
        yield render('jpx-raw-'+name,b'4 0 0 1 0 0 cm /X Do',[stream(data,params)],resources,
                     [((x,0,1,1),c) for x,c in enumerate(colors)],
                     requirement='recovery-jpx-codestream',repairs=('jpx.raw-codestream',))
        for suffix,extra in [('no-space',params.replace(b'/ColorSpace '+space,b'')),
                             ('opacity',params+b' /SMaskInData 1'),
                             ('truncated',params)]:
            encoded = data[:-1] if suffix=='truncated' else data
            yield Case('jpx-raw-'+name+'-'+suffix,page(b'4 0 0 1 0 0 cm /X Do',[stream(encoded,extra)],resources),
                       OPEN,('recovery-jpx-codestream',),'Raw JPX without sufficient metadata or complete data remains rejected',
                       # Strict rejects repairable raw framing before it can
                       # inspect the incomplete codestream. Repair still must
                       # reject truncation instead of returning partial pixels.
                       strict_valid=suffix!='truncated', extra={'render_error':'syntax'})

    yield raw_jpx_subsampling_case()


def raw_jpx_subsampling_case():
    """7.4.9 explicit RGB is authoritative; T.800 B.2 locates each sample grid."""
    planes = [[16*i for i in range(16)], [0,80,160,240], [20,220,40,200,60,180,80,160]]
    steps = [(1,1),(2,2),(2,1)]
    # Independently encode each component on its own native grid, then combine
    # their level-zero packets under one SIZ/COD declaration (T.800 A.5.1/B.2).
    streams = [jpx.codestream([values],4//dx,4//dy) for values,(dx,dy) in zip(planes,steps)]
    first = streams[0]
    coding, tile = first.index(b'\xff\x52'), first.index(b'\xff\x90')
    size = struct.pack('>H8IH',0,4,4,0,0,4,4,0,0,3) + b''.join(bytes([7,dx,dy]) for dx,dy in steps)
    header = b'\xff\x4f' + jpx.marker(0x51,size) + first[coding:tile]
    packets = b''.join(data[data.index(b'\xff\x93')+2:-2] for data in streams)
    encoded = header + jpx.marker(0x90,struct.pack('>HIBB',0,14+len(packets),0,1)) + b'\xff\x93' + packets + b'\xff\xd9'
    params = b'/Type /XObject /Subtype /Image /Width 4 /Height 4 /ColorSpace /DeviceRGB /Filter /JPXDecode'
    # The oracle uses only the declared source samples and integer grid
    # locations, never decoder output. No sampling ratio implies YCbCr.
    pixels = bytes(planes[c][(y//dy)*(4//dx)+(x//dx)]
                   for y in range(4) for x in range(4) for c,(dx,dy) in enumerate(steps))
    return render('jpx-raw-rgb-subsampled',b'4 0 0 4 0 0 cm /X Do',
                  [stream(encoded,params)],b'<< /XObject << /X 5 0 R >> >>',size=4,
                  oracle=b'P6\n4 4\n255\n'+pixels,
                  requirement='recovery-jpx-codestream',repairs=('jpx.raw-codestream',))


def type3_empty_bounds_cases():
    """An empty glyph still advances; a following painted glyph pins placement."""
    font = (b'<< /Type /Font /Subtype /Type3 /FontBBox [0 0 400 600] '
            b'/FontMatrix [.001 0 0 .001 0 0] /FirstChar 65 /LastChar 66 '
            b'/Widths [400 400] /Encoding << /Differences [65 /A /B] >> '
            b'/CharProcs << /A 6 0 R /B 7 0 R >> '
            b'/Resources << /XObject << /X 8 0 R >> >> >>')
    painted = stream(b'400 0 0 0 400 600 d1 0 0 400 600 re f')
    nested = stream(b'0 0 400 600 re f', b'/Subtype /Form /BBox [0 0 400 600] /Resources << >>')
    content = b'BT /F 10 Tf 2 2 Td (AAB) Tj ET 1 0 0 rg 1 12 3 2 re f'
    resources = b'<< /Font << /F 5 0 R >> >>'
    for name, bounds, tail in [
        ('x', b'420 140 30 220', b''),
        ('y', b'30 220 420 140', b''),
        ('xy', b'420 220 30 140', b' q 0 0 100 100 re n Q'),
        ('zero', b'0 0 0 0', b''),
    ]:
        yield render('type3-empty-bounds-' + name, content,
                     [font, stream(b'400 0 ' + bounds + b' d1' + tail), painted, nested],
                     resources, [((10,2,4,6),BLACK), ((1,12,3,2),(255,0,0))],
                     requirement='recovery-type3',
                     repairs=() if name == 'zero' else ('font.type3-empty-bounds',))
    for name, tail in [
        ('painted', b'0 0 400 600 re f'),
        ('nested', b'/X Do'),
        ('duplicate', b'400 0 d0'),
        ('malformed', b'0 0 m 1 l'),
    ]:
        yield Case('type3-empty-bounds-' + name,
                   page(content, [font, stream(b'400 0 30 220 420 140 d1 ' + tail), painted, nested], resources),
                   OPEN, ('recovery-type3',),
                   'Reversed bounds do not hide painting or later procedure errors',
                   extra={'render_error': 'syntax'})


def type1_sparse_subr_cases():
    """PDF9.9/Type1 6/8: unchanged rectangle outlines despite unused Subrs slots."""
    from .font import sparse_subr_program
    font = (b'<< /Type /Font /Subtype /Type1 /BaseFont /Fixture /FirstChar 65 /LastChar 65 '
            b'/Widths [600] /Encoding /WinAnsiEncoding /FontDescriptor 6 0 R >>')
    descriptor = (b'<< /Type /FontDescriptor /FontName /Fixture /Flags 32 /FontBBox [0 0 100 200] '
                  b'/ItalicAngle 0 /Ascent 200 /Descent 0 /CapHeight 200 /StemV 50 /FontFile 7 0 R >>')
    resources = b'<< /Font << /F 5 0 R >> >>'
    for name in ('tail','interior','nested','all-unused'):
        for filled in (False,True):
            program = sparse_subr_program(name,filled=filled)
            marker = program.index(b'currentfile eexec\n')+len(b'currentfile eexec\n')
            params = b'/Length1 %d /Length2 %d /Length3 0' % (marker,len(program)-marker)
            yield render('type1-sparse-'+name+('-valid' if filled else ''),b'BT /F 20 Tf 4 4 Td (A) Tj ET',
                         [font,descriptor,stream(program,params)],resources,[((4,4,2,4),BLACK)],
                         requirement='font-type1-sparse-subrs',repairs=() if filled else ('font.type1-sparse-subrs',))
