"""Static Sound appearances and Watermark FixedPrint placement probes.

ISO 32000-2 12.5.5, 12.5.6.16/Table 188 and 12.5.6.22/Tables 193-194.
The PDFs are 1.7: missing AP is not a PDF 2.0 conformance assertion. Sound
Speaker/Mic artwork is recommended reader policy, not standardized pixels;
missing-AP inputs record Pagura's current explicit gap without inventing an
icon oracle. Stored Sound AP controls isolate static painting from playback.

FixedPrint has mandatory placement semantics. Expected colored rectangles are
literal coordinates derived on paper, independently of the emitted AP and any
product transform. Matrix acts on the origin-translated Rect; the upright
bounding box then fits the AP, so a rotated Matrix does not rotate AP artwork.
Clause 12.5.6.22 explicitly uses MediaBox dimensions for screen display. These
oracle candidates use display intent, a zero-origin MediaBox and no page rotation;
Matrix rotation tests only the specified upright target box. The print-only
control is blank on display; its future print target is stated but no current
adapter exercises print media, rotation cancellation, tiling or n-up behavior.

API applicability: the existing PDFium adapter enables draw_annots but does not
set FPDF_PRINTING; PDF.js uses display/ENABLE on a Node canvas, without an HTML
annotation layer; Quartz draws with CoreGraphics without PDFKit. PyMuPDF,
Poppler and Hayro keep their existing display adapters. A missing icon in a
canvas/CoreGraphics-only configuration cannot prove its viewer lacks an icon.
No engine has been executed for this batch; retain every baseline/configuration
as unprobed until its exact adapter result is observed. Public API references
identify configuration knobs, not runtime support in the pinned binaries:
https://pdfium.googlesource.com/pdfium/+/refs/heads/main/public/fpdfview.h
https://mozilla.github.io/pdf.js/api/draft/module-pdfjsLib.html
https://pymupdf.readthedocs.io/en/latest/page.html#Page.get_pixmap
"""
from .model import Case, RENDER, UNSUPPORTED
from .pdf import document, opaque_rectangles, stream

AREA = 'annotation_display'
REQUIREMENT = 'content-static-annotation-placement'
ARTWORK = b'0 g 0 0 4 4 re f 1 0 0 rg 4 0 4 4 re f'


def source(*, subtype=b'Watermark', entries=b'', fixed=None, stored=True,
           page=b'', flags=4, body=False):
    """One inert annotation and a four-sample unsigned PCM sound object."""
    appearance = b'/AP << /N 6 0 R >>' if stored else b''
    sound = b'/Sound 7 0 R' if subtype == b'Sound' else b''
    fixed_entry = b'/FixedPrint << /Type /FixedPrint ' + fixed + b' >>' if fixed is not None else b''
    annotation = (b'<< /Type /Annot /Subtype ' + b'/' + subtype
                  + b' /Rect [12 8 20 12] /F %d ' % flags
                  + appearance + b' ' + sound + b' ' + fixed_entry + b' ' + entries + b' >>')
    return document([
        b'<< /Type /Catalog /Pages 2 0 R >>',
        b'<< /Type /Pages /Kids [3 0 R] /Count 1 >>',
        b'<< /Type /Page /Parent 2 0 R /MediaBox [0 0 32 24] /Resources << >> /Contents 4 0 R '
        + (b'' if body else b'/Annots [5 0 R] ') + page + b' >>',
        stream(b'0 g 12 8 4 4 re f 1 0 0 rg 16 8 4 4 re f' if body else b''),
        annotation,
        stream(ARTWORK, b'/Type /XObject /Subtype /Form /BBox [0 0 8 4] /Resources << >>'),
        stream(b'\x80' * 4, b'/Type /Sound /R 8000 /C 1 /B 8 /E /Raw'),
    ])


def oracle(rect=(12, 8, 8, 4), *, dpi=72, crop=False):
    """Pixel-aligned black/red regions with independent viewport arithmetic."""
    rectangles=[]
    if rect is not None:
        x,y,w,h=rect
        rectangles=[((x,y,w//2,h),(0,0,0)),((x+w//2,y,w//2,h),(255,0,0))]
    width,height=(24,20) if crop else (32,24)
    if crop:rectangles=[((x-4,y-2,w,h),color) for (x,y,w,h),color in rectangles]
    scale=dpi//72
    return opaque_rectangles(width*scale,height*scale,
                             [((x*scale,y*scale,w*scale,h*scale),color) for (x,y,w,h),color in rectangles])


def cases():
    yield Case('ordinary-content',source(body=True),RENDER,(REQUIREMENT,),
               'Ordinary content independently paints the same black/red regions as stored AP controls',oracle=oracle())
    for name,entry in [('default',b''),('speaker',b'/Name /Speaker'),('mic',b'/Name /Mic')]:
        yield Case('sound-'+name+'-missing-ap',source(subtype=b'Sound',entries=entry,stored=False),
                   UNSUPPORTED,(REQUIREMENT,),
                   'Missing Sound AP records the current Pagura gap; reader-chosen static icon observation remains unprobed',
                   feature='annotation.appearance')
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
        ('fixed-crop',b'/H .5 /V .25',(16,6,8,4),b'/CropBox [4 2 28 22]',{'crop':True}),
    ]
    for name,fixed,rect,page,view in variants:
        for dpi in ([72,144] if name in {'watermark-ordinary','fixed-percent','fixed-rotated-matrix'} else [72]):
            yield Case(f'{name}-{dpi}',source(fixed=fixed,page=page),RENDER,(REQUIREMENT,),
                       'Display uses literal FixedPrint target coordinates and MediaBox-relative percentages; AP fitting uses the upright target',
                       oracle=oracle(rect,dpi=dpi,**view),dpi=dpi)
    yield Case('fixed-print-only-display',source(fixed=b'/H .5 /V .25',flags=36),RENDER,(REQUIREMENT,),
               'Print plus NoView is blank on display; print target is [16,6,24,10] on 32x24 media, currently unprobed',
               oracle=oracle(None))
