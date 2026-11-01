"""Independent interactions of qualified tiling, CIE metadata and EMC recovery.

The three disjoint marks have literal integral geometry. CalGray zero with
compensation disabled is black; fully covering opaque Gray-zero tiling is
black. A blue witness and white gutters expose state or clipping corruption.
No renderer, reference engine or observed output supplies these expectations.
"""
import hashlib
from .calibrated_blackpoint import space
from .model import Case, OPEN, RENDER, UNSUPPORTED, Vector
from .pdf import document, opaque_rectangles, stream

AREA = 'compatibility_union'
REQUIREMENTS = ('content-cie', 'content-tiling', 'recovery-marked-closure')
TAG = 'content.marked-unmatched'
BLACK = (0, 0, 0)
BLUE = (0, 0, 255)
LEFT = ((1, 1, 10, 10), BLACK)
RIGHT = ((20, 1, 10, 10), BLACK)
WITNESS = ((1, 20, 10, 10), BLUE)
CIE = b'q /GS gs /C cs 0 sc 1 1 10 10 re f Q '
TILING = b'q /Pattern cs /P scn 20 1 10 10 re f Q '
BLUE_MARK = b'0 0 1 rg 1 20 10 10 re f '


def oracle(rectangles, dpi):
    scale = dpi // 72
    return opaque_rectangles(32 * scale, 32 * scale, [
        (tuple(value * scale for value in rect), color)
        for rect, color in rectangles
    ])


def pdf(content, mode=b'/OFF', *, sparse=False, child=None, later=False):
    # Fixed numbering keeps both page scopes and child resources explicit.
    resources = (b'<< /ColorSpace << /C ' + space('CalGray', b'[.2 1 1.7]')
                 + b' >> /ExtGState << /GS << /UseBlackPtComp ' + mode
                 + b' >> >> /Pattern << /P 5 0 R >> /XObject << /Child 6 0 R >> >>')
    form = stream(child if child is not None else b'/Child Do',
                  b'/Type /XObject /Subtype /Form /BBox [0 0 32 32] /Resources ' + resources)
    pattern = stream(b'0 g 0 0 .0001 .0001 re f' if sparse else b'0 g 0 0 10 10 re f',
                     b'/Type /Pattern /PatternType 1 /PaintType 1 /TilingType 1 '
                     b'/BBox [0 0 10 10] /XStep .001 /YStep .001 /Resources << >> ')
    page = b'<< /Type /Page /Parent 2 0 R /MediaBox [0 0 32 32] /Resources ' + resources
    return document([
        b'<< /Type /Catalog /Pages 2 0 R >>',
        b'<< /Type /Pages /Kids [3 0 R' + (b' 7 0 R' if later else b'')
        + b'] /Count ' + (b'2' if later else b'1') + b' >>',
        page + b' /Contents 4 0 R >>',
        stream(BLUE_MARK if later else content), pattern, form,
        *([page + b' /Contents 8 0 R >>', stream(content)] if later else []),
    ])


def cases():
    full = [LEFT, RIGHT, WITNESS]
    for dpi in (72, 144):
        # An old-engine supported semantic twin is cost evidence only.
        twin = b'0 g 1 1 10 10 re f 20 1 10 10 re f ' + BLUE_MARK
        yield Case(f'direct-twin-{dpi}', pdf(twin), RENDER, REQUIREMENTS,
                   'Literal direct rectangles are the independent semantic cost twin',
                   oracle=oracle(full, dpi), dpi=dpi,
                   extra={'tolerance': 0, 'independent_rectangles': full})
        variants = [
            ('off', CIE + b'EMC ' + TILING + BLUE_MARK, b'/OFF', None, full),
            ('default', CIE + b'EMC ' + TILING + BLUE_MARK, b'/Default', None, full),
            ('reverse-order', TILING + b'EMC ' + CIE + BLUE_MARK, b'/OFF', None, full),
            ('clip-kept', CIE + b'q 22 3 6 6 re W EMC n ' + TILING + b'Q ' + BLUE_MARK,
             b'/OFF', None, [LEFT, ((22, 3, 6, 6), BLACK), WITNESS]),
            ('form-keeps-caller', b'/Span BMC /Child Do EMC ' + BLUE_MARK,
             b'/OFF', CIE + b'EMC ' + TILING, full),
        ]
        for name, content, mode, child, rectangles in variants:
            yield Case(f'{name}-{dpi}', pdf(content, mode, child=child), RENDER,
                       REQUIREMENTS, 'Three feature interaction with disjoint CIE, tiling and state witnesses',
                       oracle=oracle(rectangles, dpi), dpi=dpi, strict_valid=False,
                       repairs=(TAG,), extra={'tolerance': 0, 'independent_rectangles': rectangles})

        for name, suffix, error in [('required-resource', b'/Missing Do', 'Syntax'),
                                    ('required-cycle', b'/Child Do', 'Cycle')]:
            yield Case(f'{name}-{dpi}', pdf(CIE + b'EMC ' + TILING + suffix), OPEN,
                       REQUIREMENTS, 'Later required failure cannot return an apparently complete repaired page',
                       dpi=dpi, strict_valid=False, repairs=(TAG,),
                       extra={'render_error': error, 'dpi': dpi})
        yield Case(f'sparse-stays-limit-{dpi}', pdf(CIE + b'EMC ' + TILING, sparse=True),
                   OPEN, REQUIREMENTS, 'Sparse cells cannot use the opaque coverage proof or bypass enumeration limits',
                   dpi=dpi, strict_valid=False, repairs=(TAG,), extra={'render_error': 'Limit', 'dpi': dpi})
        yield Case(f'compensation-on-refused-{dpi}', pdf(b'EMC ' + CIE + TILING, b'/ON'),
                   UNSUPPORTED, REQUIREMENTS, 'A reported ending never authorizes explicit black point compensation',
                   dpi=dpi, strict_valid=False, repairs=(TAG,), feature='extgstate.UseBlackPtComp', extra={'dpi': dpi})

        # Page0 is valid and visibly distinct. Page1 needs all three changes;
        # custom runtime qualification compiles it after page0 in one context.
        later_oracle = oracle(full, dpi)
        target = f'corpus/generated/{AREA}/later-oracle-{dpi}.page-1.ppm'
        yield Case(f'later-page-{dpi}', pdf(CIE + b'EMC ' + TILING + BLUE_MARK, later=True),
                   RENDER, REQUIREMENTS, 'Clean first page followed by the three-feature interaction on page1',
                   oracle=oracle([WITNESS], dpi), dpi=dpi, pages=2,
                   strict_valid=False, repairs=(TAG,),
                   extra={'tolerance': 0, 'independent_rectangles': [WITNESS],
                          'page0_strict_valid': True, 'page0_repairs': [],
                          'qualification_pages': [{'page': 1, 'oracle': target,
                                                   'oracle_sha256': hashlib.sha256(later_oracle).hexdigest(),
                                                   'dpi': dpi, 'width': dpi * 32 // 72,
                                                   'height': dpi * 32 // 72, 'tolerance': 0,
                                                   'strict_valid': False, 'repairs': [TAG]}]})
        yield Vector(f'later-oracle-{dpi}', {'.page-1.ppm': later_oracle}, REQUIREMENTS,
                     'Independent target-page geometry; not a renderer golden',
                     extra={'independent_rectangles': full})
