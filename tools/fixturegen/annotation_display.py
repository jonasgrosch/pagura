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
from .file_attachment_icons import FRAME, pixels as icon_pixels
from .model import Case, OPEN, RENDER, UNSUPPORTED
from .pdf import document, opaque_rectangles, stream

AREA = 'annotation_display'
REQUIREMENT = 'content-static-annotation-placement'
REDACT_REQUIREMENT = 'content-redact-preview'
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
           sound_reference=b'7 0 R', objects=(), content=None, catalog=b''):
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
        b'<< /Type /Page /Parent 2 0 R /MediaBox [' + media + b'] /Resources << >> /Contents 4 0 R '
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
                       OPEN if stored else UNSUPPORTED,(REDACT_REQUIREMENT,),
                       'Matched Redact preview observation: normal AP is separate from post-application fill, text and RO',
                       feature=None if stored else 'annotation.appearance')
