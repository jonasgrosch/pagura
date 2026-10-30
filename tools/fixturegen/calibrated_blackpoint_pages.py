"""Different independent pixels on first and later calibrated-colour pages.

The ordinary conformance case exercises page zero and page enumeration. Named
per-page PPM vectors qualify explicit page-index selection in the private
native/WASI facade; a facade that renders page zero cannot match later oracles.
"""
import hashlib

from .calibrated_blackpoint import command, oracle, resource, values
from .content import cie_rgb
from .model import Case, RENDER, Vector
from .pdf import document, opaque_rectangles, stream

AREA = 'calibrated_blackpoint_pages'
REQUIREMENT = ('content-cie',)


def cases():
    for family, first_value in [('CalGray', (.5,)), ('CalRGB', (.1, .3, .8)), ('Lab', (50, 40, -20))]:
        gamma = 1.0 if family == 'Lab' else 2.2
        first_color = cie_rgb(family, first_value, gamma=gamma)
        first = opaque_rectangles(8, 8, [((0, 0, 8, 8), first_color)])
        second = oracle(family)
        header, pixels = b'P6\n8 8\n255\n', second.split(b'\n', 3)[3]
        # Every row consists of eight RGB patches. Reverse the patch order,
        # keeping the rows intact, to create a genuinely different third page.
        rows = [pixels[y * 24:(y + 1) * 24] for y in range(8)]
        third = header + b''.join(b''.join(row[x:x + 3] for x in range(21, -1, -3)) for row in rows)
        third_command = b'/GS gs /C cs ' + b' '.join((' '.join(map(str, value)) + f' sc {x} 0 1 8 re f').encode() for x, value in enumerate(reversed(values(family))))
        first_command = b'/GS gs /C cs ' + (' '.join(map(str, first_value)) + ' sc 0 0 8 8 re f').encode()
        contents = [first_command, command(family), third_command]
        blacks = [b'[0 0 0]', b'[.2 1 1.7]', b'[50 50 50]']
        objects = [b'<< /Type /Catalog /Pages 2 0 R >>', b'<< /Type /Pages /Count 3 /Kids [3 0 R 5 0 R 7 0 R] >>']
        for index, (content, black) in enumerate(zip(contents, blacks)):
            objects.extend([b'<< /Type /Page /Parent 2 0 R /MediaBox [0 0 8 8] /Contents ' + str(4 + index * 2).encode() + b' 0 R /Resources ' + resource(family, black, b'/UseBlackPtComp /OFF') + b' >>', stream(content)])
        vectors = {'.page-1.ppm': second, '.page-2.ppm': third}
        page_oracles = [{'page': page, 'oracle': f'corpus/generated/{AREA}/{family}-oracles.page-{page}.ppm',
                         'oracle_sha256': hashlib.sha256(vectors[f'.page-{page}.ppm']).hexdigest(),
                         'width': 8, 'height': 8, 'dpi': 72, 'tolerance': 1} for page in (1, 2)]
        yield Case(family, document(objects), RENDER, REQUIREMENT,
                   'Three independently distinct pages: zero, chromatic and above-white diffuse-black metadata with compensation OFF',
                   oracle=first, pages=3, extra={'api': 'compile_page', 'tolerance': 1, 'qualification_pages': page_oracles})
        yield Vector(f'{family}-oracles', vectors, REQUIREMENT,
                     'Independent later-page gamma/matrix/Bradford/sRGB pixels; these are page-index accuracy oracles, not renderer goldens')
