"""Independent oracles for the pinned CFF substitutes.

Adobe TN 5176 sections 4, 5, 9, 13, 15 and TN 5177 section 4. This is a
fixture reader, not a general font loader: unneeded operators and predefined
charsets fail explicitly. No product implementation supplies paths or pixels.
"""
from fractions import Fraction as F
from functools import lru_cache


# Device-space control-polygon bound, at the fixture's 24-point size / 72 dpi.
FLATNESS = F(1, 4096)


def index(data, at):
    count = int.from_bytes(data[at:at+2], 'big')
    at += 2
    if not count:
        return [], at
    size = data[at]
    assert 1 <= size <= 4
    at += 1
    offsets = [int.from_bytes(data[p:p+size], 'big')
               for p in range(at, at+(count+1)*size, size)]
    start = at+(count+1)*size-1
    assert offsets[0] == 1 and start+offsets[-1] <= len(data)
    return [data[start+a:start+b] for a,b in zip(offsets, offsets[1:])], start+offsets[-1]


def number(data, at, *, charstring=False):
    b = data[at]
    at += 1
    if 32 <= b <= 246:
        return F(b-139), at
    if 247 <= b <= 250:
        return F((b-247)*256+data[at]+108), at+1
    if 251 <= b <= 254:
        return F(-(b-251)*256-data[at]-108), at+1
    length = {28: 2, 29: 4, 255: 4}.get(b)
    if length is not None:
        value = int.from_bytes(data[at:at+length], 'big', signed=True)
        return F(value, 65536 if b == 255 else 1), at+length
    assert b == 30 and not charstring, 'unknown CFF number'
    chars = ''
    while True:
        for nibble in (data[at] >> 4, data[at] & 15):
            if nibble == 15:
                return F(chars), at+1
            chars += '0123456789.E'[nibble] if nibble <= 11 else {12: 'E-', 14: '-'}[nibble]
        at += 1


def dictionary(data):
    at, stack, values = 0, [], {}
    while at < len(data):
        b = data[at]
        if b >= 28:
            value, at = number(data, at)
            stack.append(value)
        else:
            at += 1
            op = b
            if b == 12:
                op = (12, data[at])
                at += 1
            values[op], stack = stack, []
    assert not stack
    return values


def charset(data, at, count):
    assert at > 2, 'fixture requires an explicit charset'
    form, at, sids = data[at], at+1, [0]
    assert form in (0, 1, 2)
    while len(sids) < count:
        first = int.from_bytes(data[at:at+2], 'big')
        at += 2
        left = 0
        if form:
            size = form
            left = int.from_bytes(data[at:at+size], 'big')
            at += size
        sids.extend(range(first, first+left+1))
    assert len(sids) == count
    return sids


def flatten(a, b, c, d, tolerance):
    # The Bézier curve and its linearly parameterized chord differ by at
    # most the largest interior control-vector distance (convex-hull bound).
    controls = [(b[i]-(2*a[i]+d[i])/3, c[i]-(a[i]+2*d[i])/3) for i in (0, 1)]
    if max(sum(v[j]**2 for v in controls) for j in (0, 1)) <= tolerance**2:
        return [d]
    midpoint = lambda p,q: tuple((x+y)/2 for x,y in zip(p,q))
    ab, bc, cd = midpoint(a,b), midpoint(b,c), midpoint(c,d)
    abc, bcd = midpoint(ab,bc), midpoint(bc,cd)
    middle = midpoint(abc,bcd)
    return flatten(a,ab,abc,middle,tolerance)+flatten(middle,bcd,cd,d,tolerance)


class CharString:
    """Execute only the Type 2 operators needed by these pinned fixtures."""

    def __init__(self, local, global_subrs, tolerance):
        self.local, self.global_subrs, self.tolerance = local, global_subrs, tolerance
        self.stack, self.contours = [], []
        self.point = (F(0), F(0))
        self.hints, self.width_seen = 0, False

    def relative(self, dx, dy):
        self.point = self.point[0]+dx, self.point[1]+dy
        return self.point

    def curve(self, values):
        a = self.point
        b, c, d = [self.relative(*values[i:i+2]) for i in (0, 2, 4)]
        self.contours[-1].extend(flatten(a,b,c,d,self.tolerance))

    def run(self, data, depth=0):
        assert depth <= 10
        at = 0
        while at < len(data):
            op = data[at]
            if op == 28 or op >= 32:
                value, at = number(data, at, charstring=True)
                self.stack.append(value)
                continue
            at += 1
            if op in (10, 29):
                subrs = self.local if op == 10 else self.global_subrs
                bias = 107 if len(subrs) < 1240 else 1131 if len(subrs) < 33900 else 32768
                target = self.stack.pop()
                assert target.denominator == 1
                self.run(subrs[int(target)+bias], depth+1)
                continue
            if op in (11, 14):
                if op == 14:
                    assert len(self.stack) == (0 if self.width_seen else 1) or not self.stack
                    self.stack.clear()
                return
            v, self.stack = self.stack, []
            if op in (1, 3, 18, 23, 19, 20):
                if not self.width_seen and len(v) % 2:
                    v.pop(0)
                self.width_seen = True
                assert len(v) % 2 == 0
                self.hints += len(v)//2
                if op in (19, 20):
                    at += (self.hints+7)//8
            elif op in (4, 21, 22):
                count = 2 if op == 21 else 1
                if not self.width_seen and len(v) == count+1:
                    v.pop(0)
                self.width_seen = True
                assert len(v) == count
                point = self.relative(v[0] if op != 4 else 0, v[-1] if op != 22 else 0)
                self.contours.append([point])
            elif op == 5:
                assert len(v) % 2 == 0
                self.contours[-1].extend(self.relative(*v[i:i+2]) for i in range(0,len(v),2))
            elif op in (6, 7):
                for i, value in enumerate(v):
                    horizontal = (i % 2 == 0) == (op == 6)
                    self.contours[-1].append(self.relative(value if horizontal else 0, 0 if horizontal else value))
            elif op in (8, 24, 25):
                if op == 25:
                    for i in range(0,len(v)-6,2):
                        self.contours[-1].append(self.relative(*v[i:i+2]))
                    v = v[-6:]
                tail = 2 if op == 24 else 0
                assert (len(v)-tail) % 6 == 0
                for i in range(0,len(v)-tail,6):
                    self.curve(v[i:i+6])
                if tail:
                    self.contours[-1].append(self.relative(*v[-2:]))
            elif op in (26, 27):
                offset = v.pop(0) if len(v) % 4 == 1 else 0
                assert len(v) % 4 == 0
                for i in range(0,len(v),4):
                    a,b,c,d = v[i:i+4]
                    self.curve([offset,a,b,c,0,d] if op == 26 else [a,offset,b,c,d,0])
                    offset = 0
            elif op in (30, 31):
                horizontal = op == 31
                while v:
                    assert len(v) >= 4
                    a,b,c,d, *v = v
                    last = v.pop() if len(v) == 1 else 0
                    self.curve([a,0,b,c,last,d] if horizontal else [0,a,b,c,d,last])
                    horizontal = not horizontal
            else:
                raise AssertionError(f'unneeded Type 2 operator {op}')
        raise AssertionError('charstring lacks return/endchar')


@lru_cache(maxsize=None)
def contours(path, name, tolerance=FLATNESS):
    return program_contours(path.read_bytes(), name, tolerance)


def program_contours(data, selector, tolerance):
    assert data[0] == 1
    _, at = index(data, data[2])
    tops, at = index(data, at)
    strings, at = index(data, at)
    global_subrs, _ = index(data, at)
    top = dictionary(tops[0])
    matrix = top.get((12, 7), [F(1,1000),0,0,F(1,1000),0,0])
    assert matrix == [F(1,1000),0,0,F(1,1000),0,0]
    charstrings, _ = index(data, int(top[17][0]))
    sids = charset(data, int(top[15][0]), len(charstrings))
    if (12, 30) in top:
        # OTF cmap supplies the GID, independently of the CFF charset's CIDs.
        assert isinstance(selector, int) and 0 < selector < len(charstrings)
        glyph = selector
        fd = int(top[(12, 37)][0])
        form, fd = data[fd], fd+1
        if form == 0:
            selected = data[fd+glyph]
        else:
            assert form == 3
            count = int.from_bytes(data[fd:fd+2], 'big')
            starts = [int.from_bytes(data[p:p+2], 'big') for p in range(fd+2,fd+2+3*count,3)]
            starts.append(int.from_bytes(data[fd+2+3*count:fd+4+3*count], 'big'))
            i = next(i for i in range(count) if starts[i] <= glyph < starts[i+1])
            selected = data[fd+4+3*i]
        fonts, _ = index(data, int(top[(12, 36)][0]))
        top = dictionary(fonts[selected])
        assert (12, 7) not in top, 'fixture expects the top-level FontMatrix'
    else:
        from .text import CFF_SYMBOL_SIDS
        standard = {0: '.notdef', 1: 'space', **CFF_SYMBOL_SIDS}
        names = {strings[sid-391].decode('ascii') if sid >= 391 else standard[sid]: gid
                 for gid,sid in enumerate(sids)}
        glyph = names[selector]
    size, offset = map(int, top[18])
    private = dictionary(data[offset:offset+size])
    local, _ = index(data, offset+int(private[19][0])) if 19 in private else ([], 0)
    charstring = CharString(local, global_subrs, tolerance*1000/24)
    charstring.run(charstrings[glyph])
    return charstring.contours


@lru_cache(maxsize=None)
def otf_contours(path, scalar, tolerance=FLATNESS):
    import struct
    from .text import bundled_font_bytes, bundled_otf_glyph
    raw = bundled_font_bytes(path)
    tables = {tag: (offset,size) for tag,_,offset,size in
              (struct.unpack_from('>4sIII',raw,12+16*i) for i in range(struct.unpack_from('>H',raw,4)[0]))}
    offset,size = tables[b'CFF ']
    return program_contours(raw[offset:offset+size],bundled_otf_glyph(path,scalar),tolerance)


def afm(path):
    """AFM C entries are these fonts' built-in code-to-name encodings."""
    encoding, widths = {}, {}
    for line in path.read_text().splitlines():
        if line.startswith('C '):
            fields = dict(part.strip().split(' ',1) for part in line.split(';') if part.strip())
            name = fields['N']
            widths[name] = F(fields['WX'])
            if int(fields['C']) >= 0:
                encoding[int(fields['C'])] = name
    return encoding, widths


def strip(points, bottom):
    """Clip once per row before exact per-pixel polygon intersection."""
    for bound, sign in ((bottom, 1), (bottom+1, -1)):
        output = []
        if not points:
            break
        for a,b in zip(points, points[1:]+points[:1]):
            ain, bin = sign*(a[1]-bound) >= 0, sign*(b[1]-bound) >= 0
            if ain:
                output.append(a)
            if ain != bin:
                t = (bound-a[1])/(b[1]-a[1])
                output.append((a[0]+t*(b[0]-a[0]), F(bound)))
        points = output
    return points


def oracle(font_path, afm_path, codes, *, differences=None, advances=None, size=64, tolerance=FLATNESS):
    """AFM encoding/name/advance, CFF charset/outline, exact rational coverage.

    Cubics use rational de Casteljau subdivision with a 1/4096-device-pixel
    convex-hull error bound. Winding-signed contour areas preserve holes.
    """
    encoding, widths = afm(afm_path)
    encoding.update(differences or {})
    return outline_oracle([contours(font_path,encoding[code],tolerance) for code in codes],
                          [advances[code] if advances else widths[encoding[code]] for code in codes],size)


def outline_oracle(glyphs, advances, size=64):
    from .raster import clipped_area
    import math
    polygons, x, previous = [], F(3), None
    for shape, advance in zip(glyphs,advances,strict=True):
        bounds = [x+px*F(24,1000) for contour in shape for px,_ in contour]
        # Separate glyphs must occupy disjoint pixel columns: coverage union
        # and repeated alpha composition then have the same analytic answer.
        assert previous is None or math.ceil(previous) <= math.floor(min(bounds))
        previous = max(bounds)
        for contour in shape:
            polygon = [(x+px*F(24,1000), F(8)+py*F(24,1000)) for px,py in contour]
            area = sum(a[0]*b[1]-a[1]*b[0] for a,b in zip(polygon,polygon[1:]+polygon[:1]))
            assert area != 0
            polygons.append((1 if area > 0 else -1, polygon))
        x += F(24,1000)*advance
    pixels = []
    for y in reversed(range(size)):
        row = []
        for sign,polygon in polygons:
            if min(p[1] for p in polygon) <= y+1 and max(p[1] for p in polygon) >= y:
                clipped = strip(polygon,y)
                if clipped:
                    row.append((sign,clipped,min(p[0] for p in clipped),max(p[0] for p in clipped)))
        for x in range(size):
            area = abs(sum(sign*clipped_area(polygon,x,y) for sign,polygon,lo,hi in row if lo <= x+1 and hi >= x))
            level = 255-int(min(F(1),area)*255+F(1,2))
            pixels.extend([level]*3)
    return b'P6\n%d %d\n255\n' % (size,size)+bytes(pixels)
