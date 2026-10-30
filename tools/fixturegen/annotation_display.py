"""Static Sound appearances and Watermark FixedPrint placement probes.

ISO 32000-2 12.5.5, 12.5.6.16/Table 188 and 12.5.6.22/Tables 193-194.
The PDFs are 1.7: missing AP is not a PDF 2.0 conformance assertion. Sound
Speaker/Mic artwork is recommended reader policy, not standardized pixels.
Matched raw observations establish static drawing and view behavior separately
from our independently authored geometric icon oracles. Stored Sound AP
controls isolate static painting from playback.

FixedPrint has mandatory placement semantics. Expected colored rectangles are
literal coordinates derived on paper, independently of the emitted AP and any
product transform. Matrix acts on the origin-translated Rect; the upright
bounding box then fits the AP, so a rotated Matrix does not rotate AP artwork.
Clause 12.5.6.22 explicitly uses MediaBox dimensions for screen display. The
display controls use literal upright-media coordinates, then independently
specified cropped viewports and UserUnit sizes. No product transform is called.
The print-only control is blank on display; no current adapter exercises
physical print media, printer scaling, tiling or n-up behavior.

API applicability: the existing PDFium adapter enables draw_annots but does not
set FPDF_PRINTING; PDF.js uses display/ENABLE on a Node canvas, without an HTML
annotation layer; Quartz draws with CoreGraphics without PDFKit. PyMuPDF,
Poppler and Hayro keep their existing display adapters. A missing icon in a
canvas/CoreGraphics-only configuration cannot prove its viewer lacks an icon.
Scoped hosted observations and exact binary identities are recorded in the probe
README. Public API references identify configuration knobs, not runtime support
in other binaries or viewers:
https://pdfium.googlesource.com/pdfium/+/refs/heads/main/public/fpdfview.h
https://mozilla.github.io/pdf.js/api/draft/module-pdfjsLib.html
https://pymupdf.readthedocs.io/en/latest/page.html#Page.get_pixmap
"""
from functools import lru_cache
from fractions import Fraction as F

from .document import Builder, navigation_value
from .document_forms import name as pdf_name, ref
from .file_attachment_icons import FRAME, pixels as icon_pixels
from .forms import base as form_base
from .forms_render import glyphs as widget_glyphs, pixels as widget_area_pixels
from .markup_annotations import oracle as markup_pixels
from .model import Case, OPEN, RENDER, UNSUPPORTED
from .pdf import document, opaque_rectangles, stream

AREA = 'annotation_display'
REQUIREMENT = 'content-static-annotation-placement'
REDACT_REQUIREMENT = 'content-redact-preview'
WIDGET_REQUIREMENT = 'content-source-widget-display'
BUTTON_REQUIREMENT = 'content-normal-button-display'
SUBTYPE_REQUIREMENT = 'content-static-subtype-display'
RICH_FIELD_REQUIREMENT = 'content-rich-field-appearance'
ARTWORK = b'0 g 0 0 4 4 re f 1 0 0 rg 4 0 4 4 re f'
# Original rectangular ink regions in a 32-unit grid. These are the geometric
# union of one-unit orthogonal strokes with miter corners and butt endpoints,
# independently authored rather than parsed from the product's polylines.
SOUND_INK = {
    'Speaker': [(5,11,13,13),(11,7,13,13),(11,7,21,9),(19,7,21,25),
                (11,23,21,25),(11,19,13,25),(5,19,13,21),(5,11,7,21),
                (23,12,25,20),(27,8,29,24)],
    'Mic': [(11,11,21,13),(11,11,13,27),(11,25,21,27),(19,11,21,27),
            (7,9,9,20),(7,9,25,11),(23,9,25,20),(15,4,17,10),(10,3,22,5)],
}


def source(*, subtype=b'Watermark', entries=b'', fixed=None, stored=True,
           page=b'', flags=4, body=False, media=b'0 0 32 24', rect=b'12 8 20 12',
           appearance_entries=b'', artwork=ARTWORK, appearance_box=b'0 0 8 4',
           sound_reference=b'7 0 R', objects=(), content=None, catalog=b'', resources=b'<< >>'):
    """One inert annotation and a four-sample unsigned PCM sound object."""
    appearance = b'/AP << /N 6 0 R >>' if stored else b''
    sound = b'/Sound ' + sound_reference if subtype == b'Sound' else b''
    fixed_entry = b'/FixedPrint << /Type /FixedPrint ' + fixed + b' >>' if fixed is not None else b''
    annotation = (b'<< /Type /Annot /Subtype ' + b'/' + subtype
                  + b' /Rect [' + rect + b'] /F %d ' % flags
                  + appearance + b' ' + sound + b' ' + fixed_entry + b' ' + entries + b' >>')
    return document([
        b'<< /Type /Catalog /Pages 2 0 R'+(b' '+catalog if catalog else b'')+b' >>',
        b'<< /Type /Pages /Kids [3 0 R] /Count 1 >>',
        b'<< /Type /Page /Parent 2 0 R /MediaBox [' + media + b'] /Resources '+resources+b' /Contents 4 0 R '
        + (b'' if body else b'/Annots [5 0 R] ') + page + b' >>',
        stream(content if content is not None else
               b'0 g 12 8 4 4 re f 1 0 0 rg 16 8 4 4 re f' if body else b''),
        annotation,
        stream(artwork, b'/Type /XObject /Subtype /Form /BBox [' + appearance_box
               + b'] /Resources << >> ' + appearance_entries),
        stream(b'\x80' * 4, b'/Type /Sound /R 8000 /C 1 /B 8 /E /Raw'),
        *objects,
    ])


def oracle(rect=(12, 8, 8, 4), *, dpi=72, size=(32, 24), vertical=False, reverse=False):
    """Literal displayed regions, in a y-up viewport at 72 dpi, then zoomed."""
    rectangles=[]
    if rect is not None:
        x,y,w,h=rect
        rectangles=([((x,y,w,h//2),(0,0,0)),((x,y+h//2,w,h//2),(255,0,0))] if vertical
                    else [((x,y,w//2,h),(0,0,0)),((x+w//2,y,w//2,h),(255,0,0))])
        if reverse:rectangles=[(bounds,(255,0,0) if color==(0,0,0) else (0,0,0)) for bounds,color in rectangles]
    width,height=size
    scale=dpi//72
    return opaque_rectangles(width*scale,height*scale,
                             [((x*scale,y*scale,w*scale,h*scale),color) for (x,y,w,h),color in rectangles])


@lru_cache(maxsize=32)
def redact_pixels(polygons, *, dpi=72, rotate=0, alpha=1., rect=(8,8,24,24)):
    """Exact rational region coverage over preserved, pixel-aligned page marks."""
    coverage=markup_pixels(polygons,dpi=dpi,rotate=rotate,color=(0.,0.,0.),rect=rect).split(b'\n',3)[3]
    scale=dpi//72
    rectangles=[]
    for (x,y,w,h),color in [((0,0,32,32),(0,255,255)),((12,12,4,8),(0,0,0))]:
        if rotate:x,y,w,h=y,32-x-w,h,w
        rectangles.append(((x*scale,y*scale,w*scale,h*scale),color))
    base=opaque_rectangles(32*scale,32*scale,rectangles)
    header=base.split(b'\n',3)[:3];background=base.split(b'\n',3)[3];pixels=bytearray()
    for i in range(0,len(background),3):
        opacity=(255-coverage[i])/255*alpha
        pixels.extend(int(bg*(1-opacity)+fg*opacity+.5) for bg,fg in zip(background[i:i+3],(255,0,0)))
    return b'\n'.join(header)+b'\n'+pixels


def cases():
    yield Case('ordinary-content',source(body=True),RENDER,(REQUIREMENT,),
               'Ordinary content independently paints the same black/red regions as stored AP controls',oracle=oracle())
    for name,entry in [('default',b''),('speaker',b'/Name /Speaker'),('mic',b'/Name /Mic')]:
        yield Case('sound-'+name+'-missing-ap',source(subtype=b'Sound',entries=entry,stored=False),
                   OPEN,(REQUIREMENT,),
                   'Original baseline observation input: a reader-chosen static icon, without a universal pixel oracle')
    for name in [b'Speaker',b'Mic']:
        yield Case('sound-'+name.decode().lower()+'-stored',source(subtype=b'Sound',entries=b'/Name /'+name),
                   RENDER,(REQUIREMENT,),
                   'Stored Sound AP paints the authored regions; no activation or sound decoding is required',oracle=oracle())
    yield Case('sound-hidden-missing-ap',source(subtype=b'Sound',stored=False,flags=2),RENDER,(REQUIREMENT,),
               'Hidden Sound has no displayed mark and requires no substitute artwork',oracle=oracle(None))
    variants=[
        ('watermark-ordinary',None,(12,8,8,4),b'',{}),
        ('fixed-default',b'',(0,0,8,4),b'',{}),
        ('fixed-matrix',b'/Matrix [1 0 0 1 3 5]',(3,5,8,4),b'',{}),
        ('fixed-percent',b'/H .5 /V .25',(16,6,8,4),b'',{}),
        # Rotated origin rectangle has corners (8,2),(8,10),(4,2),(4,10).
        # Its upright box is 4x8. AP's black half stays left, not below.
        ('fixed-rotated-matrix',b'/Matrix [0 1 -1 0 8 2]',(4,2,4,8),b'',{}),
        ('fixed-crop',b'/H .5 /V .25',(12,4,8,4),b'/CropBox [4 2 28 22]',{'size':(24,20)}),
    ]
    for name,fixed,rect,page,view in variants:
        for dpi in ([72,144] if name in {'watermark-ordinary','fixed-percent','fixed-rotated-matrix'} else [72]):
            yield Case(f'{name}-{dpi}',source(fixed=fixed,page=page),RENDER,(REQUIREMENT,),
                       'Display uses literal FixedPrint target coordinates and MediaBox-relative percentages; AP fitting uses the upright target',
                       oracle=oracle(rect,dpi=dpi,**view),dpi=dpi)
    yield Case('fixed-print-only-display',source(fixed=b'/H .5 /V .25',flags=36),RENDER,(REQUIREMENT,),
               'Print plus NoView is blank on display; print target is [16,6,24,10] on 32x24 media, currently unprobed',
               oracle=oracle(None))

    # MediaBox is [10,-6,42,18]. In upright media space the four displayed
    # sizes are 32x24, 24x32, 32x24, 24x32; the H/V fractions give the literal
    # origins below. The asymmetric CropBox [13,-4,37,13] removes (3,2),
    # (2,5), (5,5), (5,3) from those four lower-left media corners.
    # These authored tuples do not use the shared product page-view mapping.
    for rotation,size,target,crop_size,crop_target in [
        (0,(32,24),(16,6,8,4),(24,17),(13,4,8,4)),
        (90,(24,32),(12,8,8,4),(17,24),(10,3,8,4)),
        (180,(32,24),(16,6,8,4),(24,17),(11,1,8,4)),
        (270,(24,32),(12,8,8,4),(17,24),(7,5,8,4)),
    ]:
        page=b'/Rotate %d' % rotation
        for dpi in (72,144):
            yield Case(f'fixed-origin-rotate-{rotation}-{dpi}',
                       source(fixed=b'/H .5 /V .25',media=b'10 -6 42 18',page=page),RENDER,(REQUIREMENT,),
                       'Shifted media origin and page rotation retain upright, media-relative artwork at ordinary display zoom',
                       oracle=oracle(target,size=size,dpi=dpi),dpi=dpi)
        yield Case(f'fixed-crop-rotate-{rotation}',
                   source(fixed=b'/H .5 /V .25',media=b'10 -6 42 18',
                          page=page+b' /CropBox [13 -4 37 13]'),RENDER,(REQUIREMENT,),
                   'Asymmetric CropBox clips the MediaBox-based location, including a clipped right edge at 90 degrees',
                   oracle=oracle(crop_target,size=crop_size))

    for rotation,size,target,vertical,reverse in [
        (0,(32,24),(2,14,8,4),False,False),
        (90,(24,32),(14,22,4,8),True,True),
        (180,(32,24),(22,6,8,4),False,True),
        (270,(24,32),(6,2,4,8),True,False),
    ]:
        yield Case(f'ordinary-origin-rotate-{rotation}',
                   source(media=b'10 -6 42 18',page=b'/Rotate %d' % rotation),RENDER,(REQUIREMENT,),
                   'Absent FixedPrint retains the original Rect and rotates the authored black/red artwork with the page',
                   oracle=oracle(target,size=size,vertical=vertical,reverse=reverse))
    yield Case('fixed-default-rotate-90',source(fixed=b'',page=b'/Rotate 90'),RENDER,(REQUIREMENT,),
               'Default placement reaches the lower-left media edge after rotation; pre-view bounds cannot cull it',
               oracle=oracle((0,0,8,4),size=(24,32)))

    for dpi in (72,144):
        yield Case(f'fixed-user-unit-{dpi}',
                   source(fixed=b'/H .5 /V .25',media=b'10 -6 42 18',page=b'/UserUnit 2 /Rotate 90'),
                   RENDER,(REQUIREMENT,),
                   'UserUnit doubles the 24x32 rotated media and 8x4 artwork; DPI independently scales the full display',
                   oracle=oracle((24,16,16,8),size=(48,64),dpi=dpi),dpi=dpi)
    yield Case('fixed-no-zoom-144',source(fixed=b'/H .5 /V .25',flags=12),RENDER,(REQUIREMENT,),
               'NoZoom fixes only artwork size: its media-relative upper-left anchor still moves with the 144 DPI view',
               oracle=oracle((32,16,8,4),size=(64,48)),dpi=144)
    yield Case('fixed-ap-matrix',source(fixed=b'/Matrix [0 1 -1 0 8 2]',
               appearance_entries=b'/Matrix [0 1 -1 0 4 0]'),RENDER,(REQUIREMENT,),
               'FixedPrint Matrix changes the upright target; the separate AP Matrix turns authored black left into black below',
               oracle=oracle((4,2,4,8),vertical=True))
    yield Case('fixed-group',source(fixed=b'/H .5 /V .25',page=b'/Rotate 90',
               appearance_entries=b'/Group << /S /Transparency /I true /K false >>'),RENDER,(REQUIREMENT,),
               'An AP transparency group uses the same media placement and page-rotation cancellation',
               oracle=oracle((12,8,8,4),size=(24,32)))

    for name,options,target in [
        ('fixed-null',{'entries':b'/FixedPrint null'},(12,8,8,4)),
        ('fixed-indirect-null',{'entries':b'/FixedPrint 8 0 R','objects':(b'null',)},(12,8,8,4)),
        ('fixed-null-defaults',{'fixed':b'/Matrix 8 0 R /H null /V 8 0 R','objects':(b'null',)},(0,0,8,4)),
        ('fixed-reversed-rect',{'fixed':b'','rect':b'20 12 12 8'},(0,0,8,4)),
        ('fixed-negative-offset',{'fixed':b'/Matrix [1 0 0 1 8 6] /H -.125 /V -.125'},(4,3,8,4)),
        ('fixed-unknown-entry',{'fixed':b'/Ignored 8 0 R','objects':(b'8 0 R',)},(0,0,8,4)),
        ('fixed-indirect-values',{'entries':b'/FixedPrint 8 0 R','objects':(
            b'<< /Type 9 0 R /Matrix 10 0 R /H 11 0 R /V 12 0 R >>',b'/FixedPrint',
            b'[13 0 R 0 0 1 3 5]',b'.25',b'.25',b'1')},(11,11,8,4)),
        ('sound-ignored-fixed-print',{'subtype':b'Sound','entries':b'/FixedPrint 8 0 R',
                                    'objects':(b'8 0 R',)},(12,8,8,4)),
        ('sound-inert-payload',{'subtype':b'Sound','sound_reference':b'8 0 R',
                                'entries':b'/Name /Unknown','objects':(b'8 0 R',)},(12,8,8,4)),
        ('fixed-hidden-malformed',{'entries':b'/FixedPrint 8 0 R','flags':2,'objects':(b'8 0 R',)},None),
    ]:
        yield Case(name,source(**options),RENDER,(REQUIREMENT,),
                   'Defaults and ignored data preserve the declared static display without following irrelevant references',
                   oracle=oracle(target))

    malformed=[
        ('fixed-not-dictionary',source(entries=b'/FixedPrint 4')),
        ('fixed-missing-type',source(entries=b'/FixedPrint << >>')),
        ('fixed-wrong-type',source(entries=b'/FixedPrint << /Type /Wrong >>')),
        ('fixed-matrix-count',source(fixed=b'/Matrix [1 0 0 1 0]')),
        ('fixed-matrix-number',source(fixed=b'/Matrix [1 0 0 1 /Bad 0]')),
        ('fixed-horizontal-type',source(fixed=b'/H /Bad')),
        ('fixed-vertical-type',source(fixed=b'/V []')),
    ]
    for name,pdf in malformed:
        yield Case(name,pdf,OPEN,(REQUIREMENT,),
                   'Malformed required placement data is a syntax error when the appearance is requested',
                   extra={'render_error':'syntax'})
    for name,options in [
        ('fixed-reference-limit',{'entries':b'/FixedPrint 8 0 R','objects':(b'8 0 R',)}),
        ('fixed-numeric-limit',{'fixed':b'/Matrix ['+b'1'+b'0'*308+b'.0 0 0 1 0 0]'}),
    ]:
        yield Case(name,source(**options),OPEN,(REQUIREMENT,),
                   'Existing reference depth and finite geometry limits reject hostile placement without new quotas',
                   extra={'render_error':'limit'})

    # Raw baseline observations adjudicate the Sound clause's Text-like view
    # rule. They do not prescribe another reader's icon size or artwork.
    for dpi,rotation,flags in [(72,0,4),(144,0,4),(72,90,4),(144,90,4),
                                (144,90,12),(144,90,20),(144,90,28)]:
        for subtype in (b'Sound',b'Text'):
            if subtype==b'Sound' and (dpi,rotation,flags)==(72,0,4):
                continue  # Existing Speaker missing/stored controls are this pair.
            for stored in (False,True):
                name=(subtype.decode().lower()+f'-view-{dpi}-{rotation}-{flags}-'
                      + ('stored' if stored else 'missing'))
                yield Case(name,source(subtype=subtype,stored=stored,flags=flags,
                           entries=b'/Name /Speaker' if subtype==b'Sound' else b'/Name /Note',
                           page=b'/Rotate %d' % rotation),OPEN,(REQUIREMENT,),
                           'Raw matched Sound/Text view observation, without a universal reader-artwork oracle',
                           extra={'dpi':dpi})

    def sound_case(name, icon='Speaker', *, stored=False, entries=None, dpi=72, rotate=0, flags=4,
                   color=(1.,1.,0.), stroke_alpha=1., fill_alpha=1., visible=True, **options):
        ink=SOUND_INK[icon]
        # The stored counterpart uses a filled union, not the generated strokes.
        regions=' '.join(f'{x} {y} {xx-x} {yy-y} re' for x,y,xx,yy in FRAME+ink).encode()
        return Case(name,source(subtype=b'Sound',stored=stored,flags=flags,
                    entries=b'/Name /'+icon.encode() if entries is None else entries,
                    page=b'/Rotate %d' % rotate,media=b'0 0 48 48',rect=b'8 8 24 24',
                    artwork=b'1 1 0 rg 2 2 28 28 re f 0 g '+regions+b' f',
                    appearance_box=b'0 0 32 32',**options),RENDER,(REQUIREMENT,'content-annotation-view'),
                    'Independent filled-region unions verify original Sound artwork and ordinary or explicit-flag view placement',
                    oracle=icon_pixels(ink,dpi=dpi,rotate=rotate,flags=flags,color=color,
                                       stroke_alpha=stroke_alpha,fill_alpha=fill_alpha,visible=visible),
                    dpi=dpi,extra={'tolerance':1})
    for icon in SOUND_INK:
        for dpi,rotate in ((72,0),(144,0),(72,90),(144,90)):
            for stored in (False,True):
                yield sound_case(f'sound-{icon.lower()}-{dpi}-{rotate}-'+('stored' if stored else 'generated'),
                                 icon,stored=stored,dpi=dpi,rotate=rotate)
    for flags in (12,20,28):
        for stored in (False,True):
            yield sound_case(f'sound-explicit-view-{flags}-'+('stored' if stored else 'generated'),
                             dpi=144,rotate=90,flags=flags,stored=stored)
    for name,entries,options in [
        ('sound-default',b'',{}),('sound-null-name',b'/Name null',{}),
        ('sound-unknown-name',b'/Name /CustomIcon',{}),
        ('sound-indirect-name',b'/Name 8 0 R',{'objects':(b'/Mic',),'icon':'Mic'}),
        ('sound-green',b'/Name /Mic /C [0 1 0]',{'icon':'Mic','color':(0.,1.,0.)}),
        ('sound-transparent-background',b'/C []',{'color':None}),
        ('sound-separate-opacity',b'/Name /Mic /CA .5 /ca .25',{'icon':'Mic','stroke_alpha':.5,'fill_alpha':.25}),
        ('sound-no-view',b'/Name 8 0 R',{'flags':32,'visible':False,'objects':(b'8 0 R',)}),
        ('sound-hidden-malformed',b'/Name (Speaker)',{'flags':2,'visible':False}),
        ('sound-stored-ignores-visual',b'/Name (Speaker) /C (ignored) /CA /Bad',{'stored':True}),
        ('sound-generated-inert-payload',b'',{'sound_reference':b'8 0 R','objects':(b'8 0 R',)}),
        ('sound-generated-ignored-fixed-print',b'/FixedPrint 8 0 R',{'objects':(b'8 0 R',)}),
    ]:
        yield sound_case(name,entries=entries,**options)
    for name,entry in [('sound-wrong-name',b'/Name (Speaker)'),('sound-wrong-color',b'/C [0 0]'),
                       ('sound-wrong-alpha',b'/CA /Opaque')]:
        yield Case(name,source(subtype=b'Sound',stored=False,entries=entry),OPEN,(REQUIREMENT,),
                   'Malformed Sound visual entries fail when drawing is requested, without inspecting the sound object',
                   extra={'render_error':'syntax'})
    yield Case('sound-name-reference-limit',source(subtype=b'Sound',stored=False,
               entries=b'/Name 8 0 R',objects=(b'8 0 R',)),OPEN,(REQUIREMENT,),
               'Sound icon names retain the existing indirect-reference depth limit',extra={'render_error':'limit'})

    # Redact annotations identify content before application. Table 195 assigns
    # IC, RO and overlay text to the later removal/replacement phase. Probe their
    # effect on previews without applying redactions or treating them as AP.
    content=b'0 1 1 rg 12 12 32 20 re f 0 g 12 20 32 4 re f'
    quads=b'/QuadPoints [14 14 24 14 24 20 14 20 30 24 40 24 40 30 30 30]'
    overlay=b'/OverlayText (R) /DA (/F0 8 Tf 0 g) /Q 1'
    fonts={
        'catalog':b'/AcroForm << /Fields [] /DR << /Font << /F0 8 0 R >> >> >>',
        'objects':(
            b'<< /Type /Font /Subtype /Type3 /FontBBox [0 0 600 800] /FontMatrix [.001 0 0 .001 0 0] '
            b'/CharProcs << /R 9 0 R >> /Encoding << /Type /Encoding /Differences [82 /R] >> '
            b'/FirstChar 82 /LastChar 82 /Widths [600] /Resources << >> >>',
            stream(b'600 0 d0 0 0 400 800 re f'),
        ),
    }
    yield Case('redact-observe-ordinary-content',source(body=True,content=content,media=b'0 0 64 48'),
               OPEN,(REDACT_REQUIREMENT,),'Ordinary cyan/black page marks identify content that a non-destructive preview must preserve')
    for variant,entries,options in [
        ('rect',b'',{}),('quads',quads,{}),('border-color',b'/C [0 0 1]',{}),
        ('interior',b'/IC [0 1 0]',{}),('quad-interior',quads+b' /IC [0 1 0]',{}),
        ('overlay',overlay,fonts),('repeat',overlay+b' /Repeat true',fonts),
        ('replacement-form',b'/RO 8 0 R /IC [0 1 0] '+overlay,{'objects':(
            stream(b'0 1 0 rg 0 0 16 20 re f 0 0 1 rg 16 0 16 20 re f',
                   b'/Type /XObject /Subtype /Form /BBox [0 0 32 20] /Resources << >>'),)}),
    ]:
        for stored in (False,True):
            yield Case('redact-observe-'+variant+'-'+('stored' if stored else 'missing'),
                       source(subtype=b'Redact',stored=stored,entries=entries,content=content,
                              media=b'0 0 64 48',rect=b'12 12 44 32',**options),
                       OPEN,(REDACT_REQUIREMENT,),
                       'Matched Redact preview observation: normal AP is separate from post-application fill, text and RO')

    def outline(x0,y0,x1,y1):
        # Supporting lines at distance 1/2 give the exact rectangular stroke;
        # the clockwise inner contour subtracts its hole from the outer box.
        return [[(x0-.5,y0-.5),(x1+.5,y0-.5),(x1+.5,y1+.5),(x0-.5,y1+.5)],
                [(x0+.5,y0+.5),(x0+.5,y1-.5),(x1-.5,y1-.5),(x1-.5,y0+.5)]]
    rectangle=outline(8,8,24,24)
    def redact_case(label, *, regions=rectangle, entries=b'', rect=(8,8,24,24), dpi=72,
                    rotate=0, alpha=1., stored=False, flags=4, text=False, **options):
        box=' '.join(map(str,rect)).encode()
        # Stored equivalence controls use independent filled contours, not a stroke.
        artwork=b'1 0 0 rg '+b' '.join(
            (f'{polygon[0][0]} {polygon[0][1]} m '+' '.join(f'{x} {y} l' for x,y in polygon[1:])+' h').encode()
            for polygon in regions)+b' f'
        page_content=b'0 1 1 rg 0 0 32 32 re f 0 g '
        if text:
            page_content+=b'BT /F0 10 Tf 1 0 0 1 12 12 Tm (R) Tj ET'
            options.update(objects=fonts['objects'],resources=b'<< /Font << /F0 8 0 R >> >>')
        else:page_content+=b'12 12 4 8 re f'
        return Case(label,source(subtype=b'Redact',stored=stored,rect=box,media=b'0 0 32 32',flags=flags,
                    page=b'/Rotate %d' % rotate,entries=f'/CA {alpha} '.encode()+entries,
                    content=page_content,artwork=artwork,appearance_box=box,**options),RENDER,(REDACT_REQUIREMENT,),
                    'Literal offset contours, rational winding coverage and source-over preserve underlying cyan/black content',
                    oracle=redact_pixels(tuple(tuple(p) for p in regions),dpi=dpi,rotate=rotate,
                                         alpha=1. if stored else alpha,rect=rect),dpi=dpi,extra={'tolerance':1})

    # The sloped sides have direction (3,4), length5 and unit normal (4,-3)/5.
    # Their half-unit offsets intersect y=7.5/16.5 and y=8.5/15.5 at these
    # literal coordinates. No generated path or stroker supplies these corners.
    skewed=[[(7,7.5),(20.25,7.5),(27,16.5),(13.75,16.5)],
            [(9,8.5),(14.25,15.5),(25,15.5),(19.75,8.5)]]
    for label,entries,regions,rect in [
        ('rect',b'',rectangle,(8,8,24,24)),
        ('quad',b'/QuadPoints [6 10 26 10 26 20 6 20]',outline(6,10,26,20),(4,4,28,28)),
        ('skew',b'/QuadPoints [8 8 20 8 26 16 14 16]',skewed,(4,4,28,28)),
    ]:
        for dpi in (72,144):
            for stored in (False,True):
                yield redact_case(f'redact-{label}-{dpi}-'+('stored' if stored else 'generated'),
                                  entries=entries,regions=regions,rect=rect,dpi=dpi,stored=stored)
    for dpi in (72,144):
        for stored in (False,True):
            yield redact_case(f'redact-rotated-{dpi}-'+('stored' if stored else 'generated'),
                              dpi=dpi,rotate=90,stored=stored)
        yield redact_case(f'redact-overlap-{dpi}',rect=(4,4,28,28),dpi=dpi,alpha=.5,
                          entries=b'/ca 0 /QuadPoints [6 8 18 8 18 16 6 16 14 12 26 12 26 20 14 20]',
                          regions=outline(6,8,18,16)+outline(14,12,26,20))
        yield redact_case(f'redact-clipped-{dpi}',rect=(10,8,22,18),dpi=dpi,
                          entries=b'/QuadPoints [8 8 20 8 26 16 14 16]',regions=skewed)
    for label,entries,options in [
        ('redact-null-quads',b'/QuadPoints null',{}),
        ('redact-indirect-null-quads',b'/QuadPoints 8 0 R',{'objects':(b'null',)}),
        ('redact-indirect-quad',b'/QuadPoints 8 0 R',{'objects':(b'[9 0 R 8 24 8 24 24 8 24]',b'8')}),
        ('redact-zero-area',b'/QuadPoints [8 8 24 8 24 8 8 8]',{'regions':[]}),
        ('redact-hidden-malformed',b'/QuadPoints (invalid)',{'flags':2,'regions':[]}),
        ('redact-no-view-malformed',b'/QuadPoints (invalid)',{'flags':32,'regions':[]}),
        ('redact-ignored-replacement',b'/IC 8 0 R /RO 8 0 R /OverlayText 8 0 R /Repeat 8 0 R '
         b'/DA 8 0 R /Q 8 0 R /C 8 0 R',{'objects':(b'8 0 R',)}),
        ('redact-stored-ignores-invalid',b'/QuadPoints (invalid) /RO 8 0 R /DA 8 0 R /ca /Bad',
         {'stored':True,'objects':(b'8 0 R',),'alpha':.25}),
        ('redact-text-generated',b'/OverlayText (replacement) /DA (invalid)',{'text':True}),
        ('redact-text-stored',b'/OverlayText (replacement) /DA (invalid)',{'text':True,'stored':True}),
    ]:
        yield redact_case(label,entries=entries,**options)
    for label,entry in [
        ('type',b'/QuadPoints 42'),('empty',b'/QuadPoints []'),
        ('short',b'/QuadPoints [4 8 28 8 28 20 4]'),
        ('number',b'/QuadPoints [4 /Bad 28 8 28 20 4 20]'),
        ('clockwise',b'/QuadPoints [4 8 4 20 28 20 28 8]'),
        ('concave',b'/QuadPoints [4 8 28 8 12 10 4 20]'),('alpha',b'/CA /Bad'),
    ]:
        yield Case('redact-invalid-'+label,source(subtype=b'Redact',stored=False,entries=entry),
                   OPEN,(REDACT_REQUIREMENT,),'Malformed preview geometry/opacity fails without reading replacement artwork',
                   extra={'render_error':'syntax'})
    for label,entry,objects in [
        ('reference',b'/QuadPoints 8 0 R',(b'8 0 R',)),
        ('numeric',b'/QuadPoints ['+b'1'+b'0'*308+b'.0 8 28 8 28 20 4 20]',()),
    ]:
        yield Case('redact-limit-'+label,source(subtype=b'Redact',stored=False,entries=entry,objects=objects),
                   OPEN,(REDACT_REQUIREMENT,),'Existing reference and finite-geometry limits bound preview data',
                   extra={'render_error':'limit'})

    yield from widget_observation_cases()
    yield from button_observation_cases()
    yield from normal_button_cases()
    yield from static_subtype_cases()
    yield from rich_field_cases()


def widget_objects(variant='text', stored=False):
    """Independent source values; the original diagnostic PDF bytes stay fixed."""
    kind='Ch' if variant.startswith(('combo','list')) else 'Tx'
    flags=1<<17 if variant.startswith('combo') else 0
    objects=form_base(kind,flags,need=variant=='text-need')
    objects[3]['MediaBox']=[0,0,64,48]
    objects[11]|={'Rect':[8,8,56,40],'V':'AB','F':4}
    if variant in ('text-inherited','text-null-inheritance'):
        objects[10]['Fields']=[ref(12)]
        objects[12]={'T':'parent','Kids':[ref(11)],'FT':pdf_name('Tx'),'V':'AB',
                     'Ff':0,'DA':b'/F1 10 Tf 0 g','Q':2}
        objects[11]['Parent']=ref(12)
        for key in ('FT','Ff','V','DA','Q'):
            objects[11].pop(key,None)
            if variant=='text-null-inheritance':objects[11][key]=None
    if variant=='text-multiline':objects[11]|={'Ff':1<<12,'V':'AB\nCD'}
    if variant=='text-comb':objects[11]|={'Ff':1<<24,'MaxLen':4,'Q':1}
    if variant=='text-widget-rotation':objects[11]['MK']={'R':90}
    if kind=='Ch':
        objects[11]|={'Opt':[['export-a','AA'],['export-b','BB'],['export-c','CC']], 'V':'BB'}
    if variant=='combo-editable':objects[11]|={'Ff':(1<<17)|(1<<18),'V':'CUSTOM'}
    if variant=='list-multi':objects[11]|={'Ff':1<<21,'V':['AA','CC'],'I':[0,2]}
    if variant=='list-top':objects[11]|={'TI':1,'V':'CC'}
    artwork=b'0 g 4 4 8 10 re f 1 0 0 rg 16 4 8 10 re f'
    objects[80]=stream(artwork,b'/Type /XObject /Subtype /Form /BBox [0 0 48 32] /Resources << >>')
    if stored:objects[11]['AP']={'N':ref(80)}
    if variant=='ordinary-content':
        objects[3].pop('Annots')
        objects[3]['Contents']=ref(50)
        objects[50]=stream(b'q 1 0 0 1 8 8 cm '+artwork+b' Q')
        objects[1].pop('AcroForm')
    return objects


def widget_pdf(objects):
    builder=Builder()
    for number,body in sorted(objects.items()):
        builder.obj(number,body if isinstance(body,bytes) else navigation_value(body))
    return builder.classic()[0]


def button_objects(variant, stored=False):
    """Normal button inputs; push buttons deliberately have neither V nor DV."""
    objects=widget_objects()
    widget=objects[11]
    widget|={'FT':pdf_name('Btn'),'V':pdf_name('Yes'),'AS':pdf_name('Yes')}
    off=b'0 0 1 rg 4 4 8 10 re f 0 1 0 rg 16 4 8 10 re f'
    objects[81]=stream(off,b'/Type /XObject /Subtype /Form /BBox [0 0 48 32] /Resources << >>')
    icon=b'0 g 0 0 8 8 re f 1 0 0 rg 8 0 8 8 re f'
    objects[83]=stream(icon,b'/Type /XObject /Subtype /Form /BBox [0 0 16 8] /Resources << >>')
    objects[84]=stream(b'0 g 0 0 48 64 re f 1 0 0 rg 48 0 48 64 re f',
                       b'/Type /XObject /Subtype /Form /BBox [0 0 96 64] /Resources << >>')
    objects[85]=stream(b'0 g 2 3 8 8 re f 1 0 0 rg 10 3 8 8 re f',
                       b'/Type /XObject /Subtype /Form /BBox [2 3 18 11] /Matrix [0 1 -1 0 11 -2] /Resources << >>')
    if variant.startswith('radio'):widget['Ff']=1<<15
    if variant in ('check-off','radio-off'):widget|={'V':pdf_name('Off'),'AS':pdf_name('Off')}
    if variant=='check-as-off':widget['AS']=pdf_name('Off')
    if variant=='check-as-on':widget['V']=pdf_name('Off')
    if variant in ('check-caption','check-as-off'):widget['MK']={'CA':'A'}
    if variant=='radio-caption':widget['MK']={'CA':'B'}
    if variant=='check-need':objects[10]['NeedAppearances']=True
    if variant.startswith('push'):
        widget['Ff']=1<<16
        widget.pop('V');widget.pop('AS')
        mk={'CA':'AB','I':ref(83)}
        widget['MK']=mk
        if variant=='push-need':objects[10]['NeedAppearances']=True
        if variant not in ('push-caption','push-need'):
            mk['TP']=1
        if variant.startswith('push-position-'):mk['TP']=int(variant.rsplit('-',1)[1])
        if variant=='push-matrix':mk['I']=ref(85)
        if variant.startswith('push-scale-'):
            mode=variant.split('-')[2]
            mk['IF']={'SW':pdf_name(mode)}
            if variant.endswith('-large'):mk['I']=ref(84)
        if variant=='push-anamorphic':mk['IF']={'S':pdf_name('A'),'A':[0,1]}
        if variant=='push-aligned':mk['IF']={'A':[0,1]}
        if variant in ('push-border','push-full-bounds'):
            widget['BS']={'W':2,'S':pdf_name('S')}
            mk|={'BC':[0,0,1],'IF':{'FB':variant=='push-full-bounds'}}
        if variant=='push-widget-rotation':mk|={'R':90,'TP':4}
        if variant=='push-page-rotation':objects[3]['Rotate']=90
    if stored:
        widget['AP']={'N':ref(80) if variant.startswith('push')
                      else {'Yes':ref(80),'Off':ref(81)}}
    return objects


@lru_cache(maxsize=16)
def button_control_pixels(artwork='on', dpi=72, rotated=False):
    """Literal stored/ordinary artwork, independent of button generation policy."""
    marks=[((12,12,8,10),(0,0,0)),((24,12,8,10),(1,0,0))]
    if artwork=='off':marks=[((12,12,8,10),(0,0,1)),((24,12,8,10),(0,1,0))]
    elif artwork=='caption':marks=widget_glyphs(x=10,y=21)
    elif artwork=='icon':marks=[((8,8,8,8),(0,0,0)),((16,8,8,8),(1,0,0))]
    return widget_area_pixels(marks,transform=(0,-1,1,0,0,64) if rotated else (1,0,0,1,0,0),
                              size=(48,64) if rotated else (64,48),scale=dpi//72)


def button_observation_cases():
    """Paired source-only observations; reader layout is not a pixel oracle.

    Table 192 defines caption/icon selection and TP ordering, not exact caption
    partitions. Table 250 defines icon fit. Existing-state controls distinguish
    AS from V. Original Type3 glyphs and rectangle icons isolate font/artwork
    availability. No edit, action, rollover or mouse-down event is requested.
    """
    for artwork in ('on','off','caption','icon'):
        objects=button_objects('push-caption')
        objects[1].pop('AcroForm');objects[3].pop('Annots')
        objects[3]['Contents']=ref(50)
        objects[3]['Resources']={'Font':{'F1':ref(70)},'XObject':{'Icon':ref(83)}}
        contents={'on':b'0 g 12 12 8 10 re f 1 0 0 rg 24 12 8 10 re f',
                  'off':b'0 0 1 rg 12 12 8 10 re f 0 1 0 rg 24 12 8 10 re f',
                  'caption':b'BT /F1 10 Tf 1 0 0 1 10 21 Tm (AB) Tj ET',
                  'icon':b'q 1 0 0 1 8 8 cm /Icon Do Q'}
        objects[50]=stream(contents[artwork])
        label='ordinary-content' if artwork=='on' else 'ordinary-'+artwork
        yield Case('button-observe-'+label,widget_pdf(objects),RENDER,(BUTTON_REQUIREMENT,),
                   'Ordinary-content control with literal original caption/icon or stored-state artwork',
                   oracle=button_control_pixels(artwork))
    variants=('check-on','check-off','check-as-off','check-as-on','check-caption','check-need',
              'radio-on','radio-off','radio-caption','push-caption','push-need','push-icon',
              'push-matrix','push-scale-B-small','push-scale-B-large','push-scale-S-small',
              'push-scale-S-large','push-scale-N-small','push-anamorphic','push-aligned',
              'push-border','push-full-bounds','push-widget-rotation','push-page-rotation',
              *(f'push-position-{position}' for position in range(2,7)))
    for variant in variants:
        for dpi in ((72,144) if variant in ('check-caption','push-caption') else (72,)):
            for stored in (False,True):
                off=variant in ('check-off','check-as-off','radio-off')
                yield Case(f'button-observe-{variant}-{dpi}-'+('stored' if stored else 'missing'),
                           widget_pdf(button_objects(variant,stored)),RENDER if stored else OPEN,
                           (BUTTON_REQUIREMENT,),
                           'Matched normal checkbox/radio/push-button source display; no field edits or interaction',
                           oracle=button_control_pixels('off' if off else 'on',dpi,variant=='push-page-rotation') if stored else None,
                           information=('font.substituted',) if not stored and variant in
                               ('check-on','check-as-on','check-need','radio-on') else (),
                           dpi=dpi,extra={'dpi':dpi})


@lru_cache(maxsize=32)
def widget_pixels(variant='text', dpi=72):
    """Literal Type3 rectangles from metrics, never from an appearance program.

    The 48x32 widget has two units of padding. Ten-unit text has 8/-2
    ascender/descender, six-unit advances and 5x7 ink. Single-line baseline
    is local y=13; multiline y=22,10; list row baselines y=21,9,-3.
    """
    scale=dpi//72
    if variant=='stored':
        return opaque_rectangles(64*scale,48*scale,
            [((12*scale,12*scale,8*scale,10*scale),(0,0,0)),
             ((24*scale,12*scale,8*scale,10*scale),(255,0,0))])
    marks=widget_glyphs(x=10,y=21)
    transform=(1,0,0,1,0,0);size=(64,48)
    if variant in ('text-inherited','text-null-inheritance','separate'):
        marks=widget_glyphs(x=42,y=21)
    elif variant=='center':marks=widget_glyphs(x=26,y=21)
    elif variant=='text-comb':marks=widget_glyphs('A',x=F(47,2),y=21)+widget_glyphs('B',x=F(69,2),y=21)
    elif variant=='text-multiline':marks=widget_glyphs(x=10,y=30)+widget_glyphs('CD',x=10,y=18)
    elif variant=='text-widget-rotation':
        marks=widget_glyphs(x=2,y=21);transform=(0,1,-1,0,56,8)
    elif variant=='combo-editable':marks=widget_glyphs('CUSTOM',x=10,y=21)
    elif variant.startswith('list'):
        selected={'list':{1},'list-multi':{0,2},'list-top':{2},'list-mismatch':{1}}[variant]
        top=1 if variant=='list-top' else 0
        marks=[]
        for row,i in enumerate(range(top,3)):
            y=38-12*row
            if i in selected:marks.append(((10,y-12,44,12),(F(3,5),F(3,4),F(9,10))))
            marks+=widget_glyphs(('AA','BB','CC')[i],x=10,y=y-9)
        # Independent intersection with the declared two-unit text clip.
        marks=[((max(x,10),max(y,10),min(x+w,54)-max(x,10),min(y+h,38)-max(y,10)),color)
               for (x,y,w,h),color in marks if x<54 and x+w>10 and y<38 and y+h>10]
    elif variant=='rebuilt':
        marks=[((12,12,8,10),(F(0),)*3),((24,12,8,10),(F(1),F(0),F(0)))]+marks
    elif variant=='blank':marks=[]
    elif variant=='siblings':marks=widget_glyphs(x=10,y=13)+widget_glyphs(x=34,y=29)
    elif variant=='page-rotate':transform=(0,-1,1,0,0,64);size=(48,64)
    elif variant=='no-rotate':marks=widget_glyphs(x=26,y=45);size=(48,64)
    elif variant=='user-unit':scale*=2
    elif variant=='no-zoom':transform=(1,0,0,1,8,40);size=(128,96);scale=1
    return widget_area_pixels(marks,transform=transform,size=size,scale=scale)


def widget_observation_cases():
    yield Case('widget-observe-ordinary-content',widget_pdf(widget_objects('ordinary-content')),RENDER,(WIDGET_REQUIREMENT,),
               'Literal black/red rectangles provide an ordinary-content control for the matched stored AP',oracle=widget_pixels('stored'))
    for variant in ('text','text-need','text-inherited','text-null-inheritance','text-multiline','text-comb',
                    'text-widget-rotation','combo','combo-editable','list','list-multi','list-top'):
        for dpi in ((72,144) if variant=='text' else (72,)):
            for stored in (False,True):
                yield Case(f'widget-observe-{variant}-{dpi}-'+('stored' if stored else 'missing'),
                           widget_pdf(widget_objects(variant,stored)),RENDER,(WIDGET_REQUIREMENT,),
                           'Matched source Tx/Ch widget: stored AP precedence and missing-AP policy without editing values',
                           oracle=widget_pixels('stored' if stored else variant,dpi),dpi=dpi,
                           extra={'dpi':dpi,'tolerance':0 if stored else 1,
                                  'widget_display':'regenerate' if stored and variant=='text-need' else 'same',
                                  'widget_preview':'widget-source-rebuilt-control' if stored and variant=='text-need' else None})
    yield from widget_source_cases()


def widget_source_cases():
    for variant in ('center','null-ap','null-normal','null-value','stream-value','separate','list-mismatch',
                    'page-rotate','user-unit','no-zoom','no-rotate','no-view','hidden','ignored-actions','ignored-sibling',
                    'stored-ignores-field','zero-width','rebuilt-control','ignored-need','unused-font','siblings'):
        objects=widget_objects('list' if variant=='list-mismatch' else 'text')
        oracle_variant=variant if variant in ('center','separate','list-mismatch','page-rotate','user-unit','no-zoom','no-rotate','siblings') else 'text'
        contract='same';dpi=144 if variant=='no-zoom' else 72
        if variant=='rebuilt-control':
            objects[11]['AP']={'N':ref(80)}
            objects[80]=stream(b'0 g 4 4 8 10 re f 1 0 0 rg 16 4 8 10 re f 0 g 2 13 5 7 re 8 13 5 7 re f',
                               b'/Type /XObject /Subtype /Form /BBox [0 0 48 32] /Resources << >>')
            oracle_variant='rebuilt'
        if variant=='center':objects[11]['Q']=1
        if variant=='null-ap':objects[11]['AP']=None
        if variant=='null-normal':objects[11]['AP']={'N':None}
        if variant=='null-value':objects[11]['V']=None;oracle_variant='blank'
        if variant=='stream-value':objects[11]['V']=ref(90);objects[90]=stream(b'\xfe\xff\x00A\x00B')
        if variant=='separate':
            objects[10]['Fields']=[ref(12)]
            objects[12]={'T':'entry','FT':pdf_name('Tx'),'V':'AB','Q':2,'Kids':[ref(11)],'MK':{'BG':[1,0,0]}}
            objects[11].pop('T');objects[11]['Parent']=ref(12)
            # Unnamed widget field-like entries do not override the actual field.
            objects[11]|={'V':'CUSTOM','Q':0,'DA':b'/missing 9 Tf'}
        if variant=='list-mismatch':objects[11]['I']=[0]
        if variant=='page-rotate':objects[3]['Rotate']=90
        if variant=='user-unit':objects[3]['UserUnit']=2
        if variant=='no-zoom':objects[11]['F']=12
        if variant=='no-rotate':objects[3]['Rotate']=90;objects[11]|={'F':20,'Rect':[8,8,40,24]}
        if variant in ('no-view','hidden'):
            objects[11]['F']=32 if variant=='no-view' else 2
            objects[1]['AcroForm']=ref(90);objects[90]=b'90 0 R';oracle_variant='blank';contract='source-only'
        if variant=='ignored-actions':
            objects[11]|={'A':ref(90),'AA':ref(90),'DV':ref(90)};objects[90]=b'90 0 R';contract='source-only'
        if variant=='ignored-need':
            objects[10]['NeedAppearances']=ref(90);objects[90]=b'90 0 R';contract='source-only'
        if variant=='unused-font':objects[10]['DR']['Font']['Unused']=ref(90);objects[90]=b'90 0 R'
        if variant=='siblings':
            for key in ('Subtype','Rect','F'):objects[11].pop(key)
            objects[11]['Kids']=[ref(20),ref(21)];objects[3]['Annots']=[ref(20),ref(21)]
            for number,rectangle in [(20,[8,8,32,24]),(21,[32,24,56,40])]:
                objects[number]={'Subtype':pdf_name('Widget'),'Parent':ref(11),'Rect':rectangle,'F':4}
        if variant=='ignored-sibling':
            objects[10]['Fields'].append(ref(90));objects[90]={'T':'unrelated','Kids':ref(91),'AA':ref(91)}
            objects[91]=b'91 0 R';contract='source-only'
        if variant=='stored-ignores-field':
            objects[11]['AP']={'N':ref(80)};objects[1]['AcroForm']=ref(90);objects[90]=b'90 0 R'
            oracle_variant='stored';contract='source-only'
        if variant=='zero-width':objects[11]['Rect']=[8,8,8,40];oracle_variant='blank'
        if variant=='null-normal':
            # Table 170 requires N in a present AP dictionary. By 7.3.9, null
            # is absent, so this is malformed rather than missing artwork.
            yield Case('widget-source-'+variant,widget_pdf(objects),OPEN,(WIDGET_REQUIREMENT,),
                       'Present AP requires N (Table 170); a null N is absent (7.3.9) and fails explicitly',
                       extra={'render_error':'syntax'})
            continue
        yield Case('widget-source-'+variant,widget_pdf(objects),RENDER,(WIDGET_REQUIREMENT,),
                   'Independent source display, untouched preview, field boundaries and existing view flags',
                   oracle=widget_pixels(oracle_variant,dpi),dpi=dpi,
                   extra={'widget_display':contract,'tolerance':1})
    invalid=[
        ('missing-da','text',lambda o:o[10].pop('DA'),'syntax'),
        ('wrong-da','text',lambda o:o[11].update(DA=3),'syntax'),
        ('wrong-flags','text',lambda o:o[11].update(Ff=-1),'syntax'),
        ('wrong-value','text',lambda o:o[11].update(V=3),'syntax'),
        ('invalid-comb','text',lambda o:o[11].update(Ff=1<<24),'syntax'),
        ('export-value','combo',lambda o:o[11].update(V='export-b'),'syntax'),
        ('wrong-option','list',lambda o:o[11].update(Opt=[['AA']]),'syntax'),
        ('unsorted-indices','list-multi',lambda o:o[11].update(I=[2,0]),'syntax'),
        ('invalid-index','list',lambda o:o[11].update(I=[3]),'syntax'),
        ('invalid-top','list',lambda o:o[11].update(TI=4),'syntax'),
        ('wrong-top','list',lambda o:o[11].update(TI='1'),'syntax'),
        ('missing-membership','text',lambda o:o[10].update(Fields=[]),'syntax'),
        ('repeated-membership','text',lambda o:o[10].update(Fields=[ref(11),ref(11)]),'cycle'),
        ('wrong-parent','text',lambda o:o[11].update(Parent=3),'syntax'),
    ]
    for label,variant,change,error in invalid:
        objects=widget_objects(variant);change(objects)
        yield Case('widget-source-'+label,widget_pdf(objects),OPEN,(WIDGET_REQUIREMENT,),
                   'Malformed selected field data or inconsistent membership is explicit',extra={'render_error':error})
    for label in ('missing-kid','parent-cycle','nonterminal'):
        objects=widget_objects('text-inherited')
        if label=='missing-kid':objects[12]['Kids']=[]
        elif label=='parent-cycle':objects[12]|={'Parent':ref(12),'Kids':[ref(11),ref(12)]}
        else:
            objects[11].pop('T')
            objects[12]['Kids'].append(ref(13));objects[13]={'T':'child','FT':pdf_name('Tx'),'Parent':ref(12)}
        yield Case('widget-source-'+label,widget_pdf(objects),OPEN,(WIDGET_REQUIREMENT,),
                   'Requested ancestry must agree with forward membership and remain acyclic',
                   extra={'render_error':'cycle' if label=='parent-cycle' else 'syntax'})
    # Table 231 bit26 requires RV when a RichText field has a value. Preserve
    # this original V=AB/no-RV diagnostic PDF; its former generic refusal was
    # imprecise. Empty/no-value rich fields are covered separately below.
    objects=widget_objects();objects[11]['Ff']=1<<25
    yield Case('widget-source-rich',widget_pdf(objects),OPEN,(WIDGET_REQUIREMENT,RICH_FIELD_REQUIREMENT),
               'Valued RichText without RV violates Table231 bit26; original bytes retained',
               extra={'render_error':'syntax'})
    objects=widget_objects();objects[10]['XFA']=b'inert'
    yield Case('widget-source-xfa',widget_pdf(objects),UNSUPPORTED,(WIDGET_REQUIREMENT,),
               'XFA form execution keeps its existing explicit capability boundary',feature='form.xfa')


def normal_button_cases():
    """Literal regions, independent of the generator's layout or Form fitting.

    The widget is [8,8,56,40]. The Type3 font advances six units and paints
    five by seven at size ten. A centered AB thus starts at x=26, y=21.
    Equal caption partitions use [2,2,46,30]; icon partitions use [0,0,48,32]
    before border adjustment. The default 16x8 icon fits as 48x24 at (8,12).
    SW=B leaves its original 16x8 at (24,20); the 96x64 icon fits as 48x32.
    Matrix [0 1 -1 0 11 -2] maps [2,3,18,11] to [0,0,8,16], with its black
    half below the red half. None of these rectangles call product geometry.
    """
    black,red,blue=(0,0,0),(1,0,0),(0,0,1)
    def halves(x,y,w,h):return [((x,y,w/2,h),black),((x+w/2,y,w/2,h),red)]
    caption=widget_glyphs(x=26,y=21)
    full=halves(8,12,48,24)
    small=halves(24,20,16,8)
    large=halves(8,8,48,32)
    border=[((8,8,48,2),blue),((8,38,48,2),blue),((8,10,2,28),blue),((54,10,2,28),blue)]
    expected={
        'check-caption':widget_glyphs('A',x=29,y=21),
        'check-as-off':[], 'radio-caption':widget_glyphs('B',x=29,y=21),
        'push-caption':caption, 'push-need':caption,
        'push-icon':full, 'push-matrix':[((24,8,16,16),black),((24,24,16,16),red)],
        'push-scale-B-small':small, 'push-scale-B-large':large,
        'push-scale-S-small':full, 'push-scale-S-large':large, 'push-scale-N-small':small,
        'push-anamorphic':large, 'push-aligned':halves(8,16,48,24),
        'push-border':border+halves(10,13,44,22), 'push-full-bounds':border+full,
        'push-position-2':halves(16,24,32,16)+widget_glyphs(x=26,y=14),
        'push-position-3':halves(16,8,32,16)+widget_glyphs(x=26,y=28),
        'push-position-4':halves(8,18,24,12)+widget_glyphs(x=37,y=21),
        'push-position-5':halves(32,18,24,12)+widget_glyphs(x=15,y=21),
        'push-position-6':full+caption,
        'push-widget-rotation':[((28,8,8,8),black),((28,16,8,8),red),
                                ((28,25,7,5),black),((28,31,7,5),black)],
        'push-page-rotation':full,
    }
    def case(label,objects,marks,*,dpi=72,preview=True,rotated=False):
        return Case('button-normal-'+label,widget_pdf(objects),RENDER,(BUTTON_REQUIREMENT,),
                    'Independent normal button geometry; stored source, untouched preview and incremental save retain one contract',
                    oracle=widget_area_pixels(marks,transform=(0,-1,1,0,0,64) if rotated else (1,0,0,1,0,0),
                                              size=(48,64) if rotated else (64,48),scale=dpi//72),dpi=dpi,
                    extra={'tolerance':1,**({'button_display':'same'} if preview else {})})
    for variant,marks in expected.items():
        for dpi in (72,144):
            yield case(f'{variant}-{dpi}',button_objects(variant),marks,dpi=dpi,rotated=variant=='push-page-rotation')
    for variant in ('check-caption','radio-caption','push-icon','push-matrix','push-position-6'):
        objects=button_objects(variant);objects[10]['NeedAppearances']=True
        yield case(variant+'-save-generated',objects,expected[variant])
    for variant in ('check-caption','push-caption'):
        objects=button_objects(variant,True);objects[10]['NeedAppearances']=True
        yield case(variant+'-stored-need',objects,[((12,12,8,10),black),((24,12,8,10),red)])
    for label,variant,change,marks in [
        ('check-v-on','check-caption',lambda o:o[11].pop('AS'),expected['check-caption']),
        ('check-as-on-v-off','check-caption',lambda o:o[11].update(V=pdf_name('Off')),expected['check-caption']),
        ('check-off-ignores-caption','check-as-off',lambda o:o[11]['MK'].update(CA=42),[]),
        ('toggle-ignores-push-entries','check-caption',lambda o:o[11]['MK'].update(I=42,IF=42,TP=42),expected['check-caption']),
        ('caption-ignores-icon','push-caption',lambda o:o[11]['MK'].update(I=42,IF=42,RI=42,IX=42,RC=42,AC=42),caption),
        ('icon-ignores-caption','push-icon',lambda o:(o[11]['MK'].update(CA=42),o[10].update(DA=b'/Missing 10 Tf'),o[10]['DR'].update(XObject=42)),full),
        ('empty-caption-no-font','push-caption',lambda o:(o[11]['MK'].update(CA=''),o[10].update(DA=b'/Missing 10 Tf')),[]),
        ('anamorphic-ignores-align','push-anamorphic',lambda o:o[11]['MK']['IF'].update(A=42),large),
        ('null-defaults','push-icon',lambda o:o[11]['MK'].update(IF={'SW':None,'S':None,'FB':None,'A':None}),full),
        ('null-icon','push-icon',lambda o:o[11]['MK'].update(I=None,IF=42),[]),
        ('hidden-invalid','push-icon',lambda o:(o[11].update(F=2),o[11]['MK'].update(TP=42)),[]),
        ('no-view-invalid','push-icon',lambda o:(o[11].update(F=32),o[11]['MK'].update(TP=42)),[]),
        ('resource-name-collision','push-position-6',lambda o:o[10]['DR'].update(XObject={'Icon':ref(80)}),full+caption),
    ]:
        objects=button_objects(variant);change(objects)
        yield case(label,objects,marks)
    objects=button_objects('check-caption');parent=objects[11]
    objects[21]={'Subtype':pdf_name('Widget'),'Rect':[8,8,56,40],'Parent':ref(11),
                 'AS':pdf_name('Yes'),'MK':parent.pop('MK'),'FT':pdf_name('Tx'),'V':'ignored widget value'}
    parent.pop('Subtype');parent.pop('Rect');parent.pop('AS');parent['Kids']=[ref(21)]
    objects[3]['Annots']=[ref(21)];objects[10]['NeedAppearances']=True
    yield case('check-separate-save-generated',objects,expected['check-caption'])
    objects=button_objects('push-caption');objects[10]|={'Fields':[ref(12)],'NeedAppearances':True}
    objects[12]={'T':'parent','Kids':[ref(11)],'FT':objects[11].pop('FT'),'Ff':objects[11].pop('Ff')}
    objects[11]['Parent']=ref(12)
    yield case('push-inherited-save-generated',objects,caption)
    objects=button_objects('push-caption');objects[11]['A']={'S':pdf_name('URI'),'URI':'https://example.invalid/inert-button'}
    yield case('push-action-inert',objects,caption)
    # Scaling an anamorphic icon is a whole-icon decision, not separate clamps
    # on the two dimensions: 96x8 is wider but shorter than a 48x32 widget.
    for when,marks in [('B',large),('S',halves(8,20,48,8))]:
        objects=button_objects('push-icon');objects[11]['MK']['IF']={'SW':pdf_name(when),'S':pdf_name('A')}
        objects[83]=stream(b'0 g 0 0 48 8 re f 1 0 0 rg 48 0 48 8 re f',
                           b'/Type /XObject /Subtype /Form /BBox [0 0 96 8] /Resources << >>')
        yield case('anamorphic-mixed-'+when,objects,marks)
    # Normal source AP wins over unneeded malformed generation entries.
    objects=button_objects('push-caption',True);objects[11]['MK']={'TP':42,'CA':42,'I':42,'IF':42}
    yield case('stored-ignores-generation',objects,[((12,12,8,10),black),((24,12,8,10),red)])
    # Ordinary Form execution isolates the relocated Table 93 parser from
    # annotation fitting: matrix alone yields these original page rectangles.
    objects=button_objects('push-matrix');objects[1].pop('AcroForm');objects[3].pop('Annots')
    objects[3]|={'Contents':ref(50),'Resources':{'XObject':{'Original':ref(85)}}}
    objects[50]=stream(b'q 2 0 0 2 24 8 cm /Original Do Q')
    yield case('ordinary-form-matrix',objects,expected['push-matrix'],preview=False)
    objects[85]=stream(b'0 g -100 -100 300 300 re f',
                       b'/Type /XObject /Subtype /Form /BBox [2 3 18 11] /Matrix [0 1 -1 0 11 -2] /Resources << >>')
    yield case('ordinary-form-bbox-clip',objects,[((24,8,16,32),black)],preview=False)
    # Table 230 permits ordinal on-state names even for duplicate exports.
    # In-unison refers to equal on-state names, not equal export strings.
    for mode,flags in [('radio',1<<15),('unison',(1<<15)|(1<<25)),('check',0)]:
        for selected in (0,1):
            objects=button_group_objects(flags,selected)
            yield case(f'opt-{mode}-{selected}',objects,widget_glyphs('A',x=15+28*selected,y=21))
    objects=button_group_objects(1<<15,1)
    for index in (21,22):objects[index]['AS']=None
    yield case('opt-null-as',objects,widget_glyphs('B',x=43,y=21))
    objects=button_group_objects(1<<15,1)
    for index in (0,1):
        objects[21+index]|={'AS':pdf_name(str(index) if index==1 else 'Off'),
                            'AP':{'N':{str(index):ref(86),'Off':ref(81)}}}
    objects[86]=stream(b'0 g 7 13 5 7 re f',b'/Subtype /Form /BBox [0 0 20 32] /Resources << >>')
    # Off selects the original blue/green stream 81, not a blank appearance.
    # Its 48x32 BBox fits the first 20x32 widget: x'=8+5*x/12, y'=8+y.
    # Retain both the explicit Off artwork and the second widget's selected ink.
    yield case('opt-stored-control',objects,[((F(29,3),12,F(10,3),10),blue),
                                           ((F(44,3),12,F(10,3),10),(0,1,0))]
               +widget_glyphs('B',x=43,y=21))
    for mode,flags in [('unison',(1<<15)|(1<<25)),('check',0)]:
        objects=button_group_objects(flags,1);objects[11]['V']=pdf_name('Shared')
        for index in (21,22):objects[index]['AS']=pdf_name('Shared')
        yield case('opt-'+mode+'-shared-state',objects,widget_glyphs('A',x=15,y=21)+widget_glyphs('B',x=43,y=21))
    objects=button_group_objects(1<<15,1);objects[11].pop('Opt')
    yield Case('button-normal-radio-unidentified',widget_pdf(objects),UNSUPPORTED,(BUTTON_REQUIREMENT,),
               'Exclusive radio group has no AS/Opt evidence identifying the selected child; explicit edits can resolve it',feature='annotation.appearance')
    objects[11]['V']=pdf_name('Off')
    yield case('radio-unidentified-off',objects,[])
    objects=button_group_objects(1<<15,1);objects[11]['Opt']=['one']
    yield Case('button-invalid-opt-count',widget_pdf(objects),OPEN,(BUTTON_REQUIREMENT,),
               'Opt requires one export per widget; a missing-AS source display validates the existing membership',extra={'render_error':'syntax'})
    for label,variant,change in [
        ('position-range','push-icon',lambda o:o[11]['MK'].update(TP=7)),
        ('position-real','push-icon',lambda o:o[11]['MK'].update(TP=1.0)),
        ('caption-type','push-caption',lambda o:o[11]['MK'].update(CA=42)),
        ('caption-da','push-caption',lambda o:o[10].pop('DA')),
        ('icon-reference','push-icon',lambda o:o[11]['MK'].update(I=42)),
        ('icon-stream','push-icon',lambda o:o.update({83:{'Subtype':pdf_name('Form'),'BBox':[0,0,16,8]}})),
        ('icon-subtype','push-icon',lambda o:o.update({83:stream(b'',b'/Subtype /Image /BBox [0 0 16 8]')})),
        ('icon-bbox','push-icon',lambda o:o.update({83:stream(b'',b'/Subtype /Form /BBox [0 0 16]')})),
        ('icon-matrix','push-icon',lambda o:o.update({83:stream(b'',b'/Subtype /Form /BBox [0 0 16 8] /Matrix [0 0 0 0 0 0]')})),
        ('fit-type','push-icon',lambda o:o[11]['MK'].update(IF=42)),
        ('fit-when','push-icon',lambda o:o[11]['MK'].update(IF={'SW':pdf_name('Unknown')})),
        ('fit-scale','push-icon',lambda o:o[11]['MK'].update(IF={'S':42})),
        ('fit-align-length','push-icon',lambda o:o[11]['MK'].update(IF={'A':[0]})),
        ('fit-align-range','push-icon',lambda o:o[11]['MK'].update(IF={'A':[0,2]})),
        ('fit-bounds','push-icon',lambda o:o[11]['MK'].update(IF={'FB':42})),
    ]:
        objects=button_objects(variant);change(objects)
        yield Case('button-invalid-'+label,widget_pdf(objects),OPEN,(BUTTON_REQUIREMENT,),
                   'Selected normal-button generation input has a named syntax boundary',extra={'render_error':'syntax'})


def button_group_objects(flags,selected):
    """Two distinct normal captions share duplicate export strings, not names."""
    objects=button_objects('radio-caption');field=objects[11]
    for key in ('Subtype','Rect','AS','MK'):field.pop(key,None)
    field|={'Kids':[ref(21),ref(22)],'Ff':flags,'Opt':['same','same'],'V':pdf_name(str(selected))}
    objects[10]['NeedAppearances']=True;objects[3]['Annots']=[ref(21),ref(22)]
    for index in range(2):
        objects[21+index]={'Subtype':pdf_name('Widget'),'Parent':ref(11),
                           'Rect':[8+28*index,8,28+28*index,40],'MK':{'CA':'AB'[index]}}
    return objects


def static_subtype_cases():
    """Screen defaults, explicit Movie image posters and required stored AP.

    Table 306 gives the clockwise rotation, not a poster sizing algorithm.
    Fitting the explicit image to Rect is declared reader policy. These four
    original samples have literal quadrant expectations; neither product image
    decoding nor its rotation/frame helpers participate in the oracle.
    """
    colors=((0,0,0),(255,0,0),(0,255,0),(0,0,255))  # TL, TR, BL, BR
    quadrants={0:(0,1,2,3),90:(2,0,3,1),180:(3,2,1,0),270:(1,3,0,2)}
    image=stream(bytes(sum((list(c) for c in colors),[])),
                 b'/Type /XObject /Subtype /Image /Width 2 /Height 2 /ColorSpace /DeviceRGB /BitsPerComponent 8')
    def pixels(rotation=0,*,rect=(12,8,8,4),size=(32,24),scale=1,order=None):
        x,y,w,h=rect
        blocks=((x,y+h//2,w//2,h//2),(x+w//2,y+h//2,w//2,h//2),
                (x,y,w//2,h//2),(x+w//2,y,w//2,h//2))
        return opaque_rectangles(size[0]*scale,size[1]*scale,
                                 [((x*scale,y*scale,w*scale,h*scale),colors[c])
                                  for (x,y,w,h),c in zip(blocks,quadrants[rotation] if order is None else order)])
    def rendered(label,pdf,expected,*,dpi=72):
        return Case('static-'+label,pdf,RENDER,(SUBTYPE_REQUIREMENT,),
                    'Independent static appearance pixels; media payloads/actions remain inert',oracle=expected,dpi=dpi)
    def movie(poster=b'8 0 R',*,extra=b'',image_object=image,**options):
        return source(subtype=b'Movie',stored=False,
                      entries=b'/Movie << /F 999 0 R /Poster '+poster+b' '+extra+b' >>',
                      objects=(image_object,),**options)
    # Missing Screen AP is explicitly blank even if MK/I is supplied (12.5.6.18).
    for label,entries in [('absent',b''),('null',b'/AP null'),
                          ('unused-artwork',b'/MK << /I 6 0 R /BC [1 0 0] >>'),
                          ('ignored-payloads',b'/MK 999 0 R /A 999 0 R /AA 999 0 R /P 999 0 R')]:
        yield rendered('screen-'+label,source(subtype=b'Screen',stored=False,entries=entries,objects=(image,)),oracle(None))
    for dpi in (72,144):
        yield rendered(f'screen-stored-{dpi}',source(subtype=b'Screen',entries=b'/MK 999 0 R /A 999 0 R'),oracle(dpi=dpi),dpi=dpi)
    # Poster=false is the default; no movie or activation entry is evaluated.
    for label,poster in [('false',b'false'),('null',b'null')]:
        yield rendered('movie-'+label,movie(poster,extra=b'/Rotate /Ignored /Aspect 999 0 R'),oracle(None))
    yield rendered('movie-absent-poster',source(subtype=b'Movie',stored=False,
                   entries=b'/Movie << /F 999 0 R /Rotate /Ignored >>'),oracle(None))
    yield rendered('movie-indirect-false',movie(b'8 0 R',image_object=b'false',extra=b'/Rotate /Ignored'),oracle(None))
    for rotation in (0,90,180,270):
        for dpi in (72,144):
            yield rendered(f'movie-poster-{rotation}-{dpi}',movie(extra=b'/Rotate %d' % rotation),
                           pixels(rotation,scale=dpi//72),dpi=dpi)
        # Ordinary page image Do uses explicit quarter-turn matrices, separately
        # authored from the product's frame and the stored vector artwork below.
        matrix={0:b'8 0 0 4 12 8',90:b'0 -4 8 0 12 12',
                180:b'-8 0 0 -4 20 12',270:b'0 4 -8 0 20 8'}[rotation]
        yield rendered(f'movie-ordinary-{rotation}',source(body=True,objects=(image,),
                       content=b'q '+matrix+b' cm /Original Do Q',resources=b'<< /XObject << /Original 8 0 R >> >>'),pixels(rotation))
        art=[]
        for (x,y),index in zip(((0,2),(4,2),(0,0),(4,0)),quadrants[rotation]):
            r,g,b=colors[index]
            art.append(f'{r//255} {g//255} {b//255} rg {x} {y} 4 2 re f'.encode())
        yield rendered(f'movie-stored-{rotation}',source(subtype=b'Movie',entries=b'/Movie 999 0 R /A 999 0 R',
                       artwork=b' '.join(art)),pixels(rotation))
    yield rendered('movie-negative-rotation',movie(extra=b'/Rotate -90'),pixels(270))
    yield rendered('movie-large-rotation',movie(extra=b'/Rotate 450'),pixels(90))
    yield rendered('movie-null-rotation',movie(extra=b'/Rotate null'),pixels())
    yield rendered('movie-inert-entries',source(subtype=b'Movie',stored=False,objects=(image,),
                   entries=b'/Movie << /F 999 0 R /Poster 8 0 R /Aspect [1920 1080] >> /A 999 0 R /AA 999 0 R',
                   catalog=b'/OpenAction 999 0 R'),pixels())
    yield rendered('movie-page-rotation',movie(page=b'/Rotate 90'),pixels(90,rect=(8,12,4,8),size=(24,32)))
    yield rendered('movie-crop',movie(page=b'/CropBox [10 6 22 16]'),pixels(rect=(2,2,8,4),size=(12,10)))
    yield rendered('movie-user-unit-144',movie(page=b'/UserUnit 2'),pixels(scale=4),dpi=144)
    yield rendered('movie-nozoom-144',movie(flags=12),pixels(rect=(24,20,8,4),size=(64,48)),dpi=144)
    yield rendered('movie-norotate',movie(flags=20,page=b'/Rotate 90'),pixels(rect=(12,16,8,4),size=(24,32)))
    yield rendered('movie-page-resources-ignored',movie(resources=b'<< /ColorSpace << /DefaultRGB 999 0 R >> /XObject << /Poster 999 0 R >> >>'),pixels())
    for flags,label in [(2,'hidden'),(32,'no-view')]:
        yield rendered('movie-'+label,source(subtype=b'Movie',stored=False,flags=flags,entries=b'/Movie 999 0 R'),oracle(None))
    # Subtype-specific clauses require normal AP: no generic synthesis exists.
    for subtype,payload in [(b'3D',b'/3DD 999 0 R'),(b'RichMedia',b'/RichMediaContent 999 0 R'),
                            (b'PrinterMark',b'/MN /Original'),(b'TrapNet',b'/AS /Selected /AnnotStates 999 0 R')]:
        label=subtype.decode().lower()
        yield rendered(label+'-stored',source(subtype=subtype,flags=68,entries=payload),oracle())
        for kind,entry in [('absent',b''),('null',b'/AP null')]:
            yield Case(f'static-{label}-{kind}',source(subtype=subtype,stored=False,flags=68,entries=entry),
                       OPEN,(SUBTYPE_REQUIREMENT,),'This subtype requires a normal AP; missing artwork is malformed, not a generator backlog',
                       extra={'render_error':'syntax'})
        yield rendered(label+'-hidden',source(subtype=subtype,stored=False,flags=2,entries=payload),oracle(None))
    for subtype in (b'PrinterMark',b'TrapNet'):
        yield rendered(subtype.decode().lower()+'-state',source(subtype=subtype,stored=False,flags=68,
                       entries=b'/AS /Selected /AP << /N << /Selected 6 0 R >> /R 999 0 R /D 999 0 R >>'),oracle())
    for label,subtype,entries in [
        ('screen-ap-type',b'Screen',b'/AP 42'),
        ('screen-ap-no-normal',b'Screen',b'/AP << >>'),
        ('screen-ap-null-normal',b'Screen',b'/AP << /N null >>'),
        ('movie-missing-dictionary',b'Movie',b''),
        ('movie-dictionary-type',b'Movie',b'/Movie 42'),
        ('movie-null-dictionary',b'Movie',b'/Movie null'),
    ]:
        yield Case('static-'+label,source(stored=False,subtype=subtype,entries=entries),OPEN,(SUBTYPE_REQUIREMENT,),
                   'Selected static-display entry has an explicit syntax boundary',extra={'render_error':'syntax'})
    for label,options in [
        ('poster-number',{'poster':b'42'}),('poster-dictionary',{'image_object':b'<< /Subtype /Image >>'}),
        ('poster-form',{'image_object':stream(b'',b'/Subtype /Form /BBox [0 0 1 1]')}),
        ('poster-subtype',{'image_object':stream(b'',b'/Subtype /Unknown')}),
        ('rotation-type',{'extra':b'/Rotate 90.0'}),('rotation-angle',{'extra':b'/Rotate 45'}),
        ('image-width',{'image_object':stream(b'\0',b'/Subtype /Image /Width /Bad /Height 1 /BitsPerComponent 8 /ColorSpace /DeviceGray')}),
        ('resource-color-name',{'image_object':stream(b'\0\0\0',b'/Subtype /Image /Width 1 /Height 1 /BitsPerComponent 8 /ColorSpace /Alias'),
                                'resources':b'<< /ColorSpace << /Alias /DeviceRGB >> >>'}),
    ]:
        yield Case('static-movie-'+label,movie(**options),OPEN,(SUBTYPE_REQUIREMENT,),
                   'Poster selection validates the stream; ordinary image execution owns its descriptor errors',extra={'render_error':'syntax'})
    yield Case('static-movie-file-poster',movie(b'true'),UNSUPPORTED,(SUBTYPE_REQUIREMENT,),
               'Poster=true requires movie-file retrieval; no media decoding or activation is attempted',feature='annotation.appearance')
    yield Case('static-movie-reference-limit',source(subtype=b'Movie',stored=False,
               entries=b'/Movie << /Poster 8 0 R >>',objects=(b'8 0 R',)),OPEN,(SUBTYPE_REQUIREMENT,),
               'The selected Poster uses the existing reference-depth budget',extra={'render_error':'limit'})


def rich_field_cases():
    """Literal Type3 regions for the shared RV/DS/source/edit/save contract.

    Rect=[8,8,56,40], inner=[10,10,54,38]. At 10pt the font advances6,
    has 8/-2 metrics and paints5x7. Single-line baseline21 is centered in
    the28-unit inner height. A20pt run changes the common baseline to18;
    two10pt paragraphs have baselines30 and18. No rich parser/layout is run
    to derive these coordinates, and stored controls contain only rectangles.
    """
    red,blue,green=(1,0,0),(0,0,1),(0,1,0)
    xml='<body><p><span style="color:#ff0000">A</span>B</p></body>'
    style='font-size:10pt;color:#0000ff'
    base_marks=[((10,21,5,7),red),((16,21,5,7),blue)]
    def objects(markup=xml, ds=style):
        result=widget_objects()
        result[11]|={'Ff':1<<25,'RV':markup,'DS':ds,'V':'AB','DV':'DEFAULT'}
        result[10]['DA']=b'/F1 10 Tf 0 1 0 rg'
        return result
    def art(marks):
        return ' '.join(f'{r} {g} {b} rg {x} {y} {w} {h} re f'
                        for (x,y,w,h),(r,g,b) in marks).encode()
    def case(label, data, marks=base_marks, *, dpi=72, paired='source', extra=None, transform=(1,0,0,1,0,0), size=(64,48)):
        metadata:dict={'tolerance':1}
        if paired!='ordinary':metadata['widget_display']='same'
        if paired=='source' and isinstance(data[11].get('RV'),str):
            metadata|={'rich_input':data[11]['RV'],'rich_plain':data[11].get('V','')}
        if extra:metadata.update(extra)
        return Case('rich-field-'+label,widget_pdf(data),RENDER,(RICH_FIELD_REQUIREMENT,),
                    'Independent constant-width Type3 rectangles and selected rich field data; stored/ordinary controls contain literal paths',
                    oracle=widget_area_pixels(marks,transform=transform,size=size,scale=dpi//72),dpi=dpi,extra=metadata)
    # Paired controls isolate source synthesis from the established Form/page path.
    for label,markup,flags,marks,plain in [
        ('styled',xml,1<<25,base_marks,'AB'),
        ('mixed','<body><p>A<span style="font-size:20pt;color:#ff0000">B</span></p></body>',1<<25,
         [((10,18,5,7),blue),((16,18,10,14),red)],'AB'),
        ('paragraphs','<body><p>AB</p><p>CD</p></body>',(1<<25)|(1<<12),
         widget_glyphs('AB',x=10,y=30,color=blue)+widget_glyphs('CD',x=10,y=18,color=blue),'AB\nCD'),
    ]:
        for dpi in (72,144):
            for mode in ('source','stored','ordinary'):
                data=objects(markup);data[11]|={'Ff':flags,'V':plain}
                if mode=='stored':
                    data[11]['AP']={'N':ref(80)}
                    data[80]=stream(art(marks),b'/Type /XObject /Subtype /Form /BBox [8 8 56 40] /Resources << >>')
                elif mode=='ordinary':
                    data[3].pop('Annots');data[3]['Contents']=ref(50);data[50]=stream(art(marks))
                    data[1].pop('AcroForm')
                yield case(f'{label}-{dpi}-{mode}',data,marks,dpi=dpi,paired=mode)
    variants=[
        ('center',lambda o:o[11].update(Q=1),[((26,21,5,7),red),((32,21,5,7),blue)]),
        ('right',lambda o:o[11].update(Q=2),[((42,21,5,7),red),((48,21,5,7),blue)]),
        ('ds-quadding',lambda o:o[11].update(DS=style+';text-align:center',Q=2),[((26,21,5,7),red),((32,21,5,7),blue)]),
        ('comb',lambda o:o[11].update(Ff=(1<<25)|(1<<24),MaxLen=4,Q=1),[((F(47,2),21,5,7),red),((F(69,2),21,5,7),blue)]),
        ('wrap',lambda o:o[11].update(Ff=(1<<25)|(1<<12),RV='<body><p>AB CD EF</p></body>',V='AB CD EF'),
         widget_glyphs('AB CD',x=10,y=30,color=blue)+widget_glyphs('EF',x=10,y=18,color=blue)),
        ('maxlen',lambda o:o[11].update(MaxLen=2),base_marks),
        ('no-scroll-fit',lambda o:o[11].update(Ff=(1<<25)|(1<<23)),base_marks),
        ('password',lambda o:o[11].update(Ff=(1<<25)|(1<<13)),base_marks),
        ('empty',lambda o:[o[11].pop('V'),o[11].pop('RV')],[]),
        ('empty-null',lambda o:o[11].update(V=None,RV=None),[]),
        ('empty-rich',lambda o:o[11].update(V='',RV='<body/>'),[]),
        ('no-style',lambda o:o[11].pop('DS'),[((10,21,5,7),red),((16,21,5,7),green)]),
        ('null-style',lambda o:o[11].update(DS=None),[((10,21,5,7),red),((16,21,5,7),green)]),
        ('ds-font-size',lambda o:o[10].update(DA=b'/F1 0 Tf 0 g'),base_marks),
        ('inline-font',lambda o:o[11].update(RV='<body><p><span style="font-family:F2;color:#ff0000">A</span>B</p></body>'),
         [((10,21,7,7),red),((19,21,5,7),blue)]),
        ('stream',lambda o:[o[11].update(RV=ref(90)),o.update({90:stream(xml.encode())})],base_marks),
        ('stream-need',lambda o:[o[11].update(RV=ref(90)),o[10].update(NeedAppearances=True),o.update({90:stream(xml.encode())})],base_marks),
        ('utf16-stream',lambda o:[o[11].update(RV=ref(90)),o.update({90:stream(b'\xff\xfe'+xml.encode('utf-16-le'))})],base_marks),
        ('v-independent',lambda o:o[11].update(V='OLD'),base_marks),
        ('plain-ignores-rich',lambda o:o[11].update(Ff=0,RV=ref(91),DS=ref(91)),widget_glyphs('AB',x=10,y=21,color=green)),
    ]
    for label,change,marks in variants:
        data=objects();data[10]['DR']['Font']['F2']=ref(74)
        data[74]=dict(data[70],FontBBox=[0,-200,900,800],Widths=[900]*59,
                      CharProcs={key:ref(76 if key=='space' else 75) for key in data[70]['CharProcs']})
        data[75]=stream(b'900 0 d0 0 0 700 700 re f');data[76]=stream(b'900 0 d0')
        data[91]=b'91 0 R';change(data)
        extra={}
        if label in ('stream','stream-need','utf16-stream'):extra|={'rich_input':xml,'rich_plain':'AB'}
        if label=='v-independent':extra['rich_plain']='AB'
        if label in ('password','empty-null','plain-ignores-rich'):extra['rich_input']=None
        yield case(label,data,marks,extra=extra)
    for rotation,marks in [
        (90,[((28,10,7,5),red),((28,16,7,5),blue)]),
        (180,[((49,20,5,7),red),((43,20,5,7),blue)]),
        (270,[((29,33,7,5),red),((29,27,7,5),blue)]),
    ]:
        data=objects();data[11]['MK']={'R':rotation}
        yield case(f'rotation-{rotation}',data,marks)
    # The field owns local RV/DS. Separate widget and parent entries do not inherit.
    data=objects();field=data[11]
    data[11]={key:value for key,value in field.items() if key not in ('Subtype','Rect','F')}
    data[11]['Kids']=[ref(20)];data[3]['Annots']=[ref(20)]
    data[20]={'Subtype':pdf_name('Widget'),'Parent':ref(11),'Rect':[8,8,56,40],'RV':ref(91),'DS':ref(91)}
    data[91]=b'91 0 R'
    yield case('separate',data)
    data=objects();data[12]={'T':'parent','Kids':[ref(11)],'FT':pdf_name('Tx'),'Ff':1<<25,
                           'DA':b'/F1 10 Tf 0 g','Q':2,'DS':'color:#00ff00','RV':ref(91)}
    data[10]['Fields']=[ref(12)];data[11]['Parent']=ref(12)
    data[11].pop('FT');data[11].pop('Ff');data[91]=b'91 0 R'
    yield case('inherited-da',data,[((42,21,5,7),red),((48,21,5,7),blue)])
    data[11].pop('DS')
    yield case('parent-style-ignored',data,[((42,21,5,7),red),((48,21,5,7),(0,0,0))])
    # Regeneration must discard old non-Tx decoration, BBox/Matrix and font resources.
    data=objects();data[10]['NeedAppearances']=True;data[11]['AP']={'N':ref(80),'R':ref(81),'D':ref(81)}
    data[80]=stream(b'0 0 1 rg 0 0 24 16 re f',b'/Type /XObject /Subtype /Form /BBox [0 0 24 16] /Matrix [2 0 0 2 0 0] /Resources << /Font << /F1 91 0 R >> >>')
    data[91]=b'91 0 R'
    yield case('need-regenerate',data,[((8,8,48,32),blue)],
               extra={'widget_display':'regenerate','widget_preview':'rich-field-styled-72-source'})
    for label,change,error in [
        ('missing',lambda o:o[11].pop('RV'),'syntax'),
        ('null-valued',lambda o:o[11].update(RV=None),'syntax'),
        ('type',lambda o:o[11].update(RV=12),'syntax'),
        ('xml',lambda o:o[11].update(RV='<body><p>AB</body>'),'syntax'),
        ('style',lambda o:o[11].update(DS=12),'syntax'),
        ('single-line',lambda o:o[11].update(RV='<body><p>A</p><p>B</p></body>'),'syntax'),
        ('comb-flags',lambda o:o[11].update(Ff=(1<<25)|(1<<24)|(1<<12),MaxLen=4),'syntax'),
        ('no-scroll',lambda o:o[11].update(Ff=(1<<25)|(1<<23),RV='<body><p>ABCDEFGHI</p></body>'),'invalid-argument'),
        ('reference',lambda o:o[11].update(RV=ref(91)),'limit'),
        ('style-reference',lambda o:o[11].update(DS=ref(91)),'limit'),
    ]:
        data=objects();data[91]=b'91 0 R';change(data)
        yield Case('rich-field-invalid-'+label,widget_pdf(data),OPEN,(RICH_FIELD_REQUIREMENT,),
                   'Selected RV/DS/field constraint fails explicitly; text-only extraction remains lazy',extra={'render_error':error})
    for label,change in [
        ('auto-size',lambda o:[o[10].update(DA=b'/F1 0 Tf'),o[11].pop('DS')]),
        ('paragraph-units',lambda o:o[11].update(DS=style+';margin-left:2em')),
        ('external',lambda o:o[11].update(RV='<!DOCTYPE body SYSTEM "https://invalid.example"><body/>')),
    ]:
        data=objects();change(data)
        yield Case('rich-field-unsupported-'+label,widget_pdf(data),UNSUPPORTED,(RICH_FIELD_REQUIREMENT,),
                   'Existing shared rich-layout boundary, without external requests or fallback to stale V',feature='form.rich-text')
    for label,flags,stored,marks in [('hidden',2,False,[]),('no-view',32,False,[]),('stored',4,True,base_marks)]:
        data=objects();data[11]|={'RV':ref(91),'DS':ref(91),'F':flags};data[91]=b'91 0 R'
        if stored:
            data[11]['AP']={'N':ref(80)}
            data[80]=stream(art(marks),b'/Type /XObject /Subtype /Form /BBox [8 8 56 40] /Resources << >>')
        yield case('lazy-'+label,data,marks,paired='stored')
