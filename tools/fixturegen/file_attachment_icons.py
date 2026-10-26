"""Original FileAttachment artwork: ISO 32000-2 12.5.6.15/Table 187.

The standard specifies icon names, not their pixels. The reader's geometric
artwork is checked by independently authored rectangular ink regions, exact
quarter-pixel area masks and source-over formulas. Stored appearances use filled
regions instead of the product generator's stroked polylines. No PDF engine,
product path parser, rasterizer or private assets supply an expectation.
"""
from .model import Case, OPEN, RENDER
from .pdf import document, stream

AREA = 'file_attachment_icons'
REQUIREMENT = 'content-file-attachment-icons'
# Bounds (x0, y0, x1, y1) in a 32-unit artwork grid. Regions overlap as one
# nonzero-filled black mask, not repeated translucent paint operations.
FRAME = [(1, 1, 3, 31), (29, 1, 31, 31), (3, 1, 29, 3), (3, 29, 29, 31)]
INK = {
    'Graph': [(5, 5, 7, 26), (6, 5, 26, 7), (9, 6, 11, 12), (15, 6, 17, 18), (21, 6, 23, 24)],
    'Paperclip': [(15, 10, 17, 23), (15, 21, 21, 23), (19, 7, 21, 23), (9, 7, 21, 9),
                  (9, 7, 11, 27), (9, 25, 25, 27), (23, 6, 25, 27)],
    'PushPin': [(9, 21, 11, 27), (11, 25, 21, 27), (21, 21, 23, 27), (11, 21, 21, 23),
                (11, 13, 13, 22), (7, 13, 13, 15), (7, 9, 9, 15), (7, 9, 25, 11),
                (23, 9, 25, 15), (19, 13, 25, 15), (19, 13, 21, 22), (15, 4, 17, 10)],
    'Tag': [(5, 7, 7, 25), (7, 7, 23, 9), (21, 7, 23, 13), (21, 11, 27, 13),
             (25, 11, 27, 21), (21, 19, 27, 21), (21, 19, 23, 25), (7, 23, 23, 25),
             (17, 13, 19, 19), (19, 17, 21, 19), (21, 13, 23, 19), (19, 13, 21, 15)],
}


def source(icon, *, stored=False, dpi=72, rotate=0, flags=0, extra=b'', page=b''):
    del dpi  # The fixture's render request, not its PDF, controls DPI.
    appearance = b''
    if stored:
        # Independent representation: union of filled rectangles; no strokes.
        ink = ' '.join(f'{x} {y} {xx-x} {yy-y} re' for x, y, xx, yy in FRAME + INK[icon]).encode()
        appearance = stream(b'1 1 0 rg 2 2 28 28 re f 0 g ' + ink + b' f',
                            b'/Type /XObject /Subtype /Form /BBox [0 0 32 32] /Resources << >>')
    annotation = (b'<< /Type /Annot /Subtype /FileAttachment /Rect [8 8 24 24] /FS 6 0 R '
                  + f'/F {flags} '.encode() + (b'/Name /' + icon.encode() if icon else b'')
                  + extra + (b' /AP << /N 7 0 R >>' if stored else b'') + b' >>')
    objects = [b'<< /Type /Catalog /Pages 2 0 R >>', b'<< /Type /Pages /Kids [3 0 R] /Count 1 >>',
               b'<< /Type /Page /Parent 2 0 R /MediaBox [0 0 48 48] /Annots [5 0 R] /Contents 4 0 R /Resources << >> '
               + f'/Rotate {rotate} '.encode() + page + b' >>', stream(b''), annotation,
               b'<< /Type /Filespec /F (inert-external-file.bin) >>']
    if stored:
        objects.append(appearance)
    return document(objects)


def pixels(icon, *, dpi=72, rotate=0, flags=0, color=(1., 1., 0.), stroke_alpha=1., fill_alpha=1., visible=True):
    scale = dpi // 72
    size = 48 * scale
    grid = size * 2
    background, foreground = bytearray(grid * grid), bytearray(grid * grid)
    # Transform source rectangles about the PDF's upper-left annotation anchor.
    # Coordinates here have a bottom-left origin, including after page rotation.
    anchor = (24 * scale, 40 * scale) if rotate else (8 * scale, 24 * scale)
    magnification = 1 if flags & 8 else scale
    def point(x, y):
        dx, dy = x / 2, y / 2 - 16
        if rotate and not flags & 16:
            dx, dy = dy, -dx
        return int(2 * (anchor[0] + magnification * dx)), int(2 * (anchor[1] + magnification * dy))
    def cover(mask, rectangles):
        for x0, y0, x1, y1 in rectangles:
            a, b = point(x0, y0), point(x1, y1)
            left, right = max(0, min(a[0], b[0])), min(grid, max(a[0], b[0]))
            bottom, top = max(0, min(a[1], b[1])), min(grid, max(a[1], b[1]))
            for y in range(bottom, top):
                mask[y * grid + left:y * grid + right] = bytes([1]) * (right - left)
    if visible:
        if color is not None:
            cover(background, [(2, 2, 30, 30)])
        cover(foreground, FRAME + INK[icon])
    output = bytearray()
    for row in reversed(range(size)):
        for column in range(size):
            at = 2 * row * grid + 2 * column
            offsets = (at, at + 1, at + grid, at + grid + 1)
            bg = ((sum(background[i] for i in offsets) * 255 + 2) // 4) / 255 * fill_alpha
            fg = ((sum(foreground[i] for i in offsets) * 255 + 2) // 4) / 255 * stroke_alpha
            output.extend(int(255 * (1 - bg + component * bg) * (1 - fg) + .5) for component in (color or (1, 1, 1)))
    return f'P6\n{size} {size}\n255\n'.encode() + output


def cases():
    def rendered(name, icon, *, stored=False, extra=b'', color=(1., 1., 0.), stroke_alpha=1., fill_alpha=1., visible=True, **view):
        return Case(name, source(icon, stored=stored, extra=extra, **view), RENDER,
                    (REQUIREMENT, 'content-annotation-view'),
                    'Independent region-union artwork and exact coverage verify icon placement, opacity and stored/generated equivalence',
                    dpi=view.get('dpi', 72),
                    oracle=pixels(icon if icon in INK else 'PushPin', color=color, stroke_alpha=stroke_alpha,
                                  fill_alpha=fill_alpha, visible=visible, **view), extra={'tolerance': 1})
    for icon in INK:
        for dpi, rotate in ((72, 0), (144, 0), (72, 90)):
            for stored in (False, True):
                yield rendered(f'{icon.lower()}-{dpi}-{rotate}-' + ('stored' if stored else 'generated'), icon,
                               dpi=dpi, rotate=rotate, stored=stored)
    for flags in (8, 16, 24):
        for stored in (False, True):
            yield rendered(f'graph-view-{flags}-' + ('stored' if stored else 'generated'), 'Graph',
                           dpi=144, rotate=90, flags=flags, stored=stored)
    yield rendered('default', '')
    yield rendered('null-name', '', extra=b' /Name null')
    yield rendered('unknown-name', 'CustomIcon')
    yield rendered('green', 'Paperclip', extra=b' /C [0 1 0]', color=(0., 1., 0.))
    yield rendered('transparent-background', 'Tag', extra=b' /C []', color=None)
    yield rendered('separate-opacity', 'PushPin', extra=b' /CA .5 /ca .25', stroke_alpha=.5, fill_alpha=.25)
    yield rendered('hidden', 'Graph', flags=2, visible=False)
    yield rendered('no-view', 'Graph', flags=32, visible=False)
    yield rendered('stored-ignores-style', 'Graph', stored=True, extra=b' /C (ignored) /CA (ignored)')
    for name, extra in [('wrong-name', b' /Name (Graph)'), ('wrong-color', b' /C [0 0]'), ('wrong-alpha', b' /CA /Opaque')]:
        yield Case(name, source('', extra=extra), OPEN, (REQUIREMENT,),
                   'Malformed visual entries fail without an appearance', extra={'render_error': 'syntax'})
