"""Independent geometric coverage oracles (no engine imports).

Simple polygon coverage uses rational Sutherland-Hodgman clipping against each
pixel and the shoelace formula. Curves and circular pens use 128x128 midpoint
sampling of their geometric definitions; their maximum accepted difference is
3/255 (including Rust's 1/256-pixel flattening tolerance). Polygon cases allow
1/255 for final rounding. Path coordinates here have a top-left origin.
"""
from fractions import Fraction as F
import math

from .model import Case, RENDER, Vector
from .pdf import document, stream

AREA = "raster"
SIZE = 8


def clipped_polygon(points, x, y):
    """Clip a closed contour to one closed pixel using rational arithmetic."""
    points = [(F(a), F(b)) for a, b in points]
    for axis, bound, sign in ((0, x, 1), (0, x + 1, -1), (1, y, 1), (1, y + 1, -1)):
        output = []
        if not points:
            return []
        for a, b in zip(points, points[1:] + points[:1]):
            ain = sign * (a[axis] - bound) >= 0
            bin = sign * (b[axis] - bound) >= 0
            if ain:
                output.append(a)
            if ain != bin:
                t = (bound - a[axis]) / (b[axis] - a[axis])
                output.append(tuple(p + t * (q - p) for p, q in zip(a, b)))
        points = output
    return points


def clipped_area(points, x, y):
    points = clipped_polygon(points, x, y)
    if not points:
        return F(0)
    return abs(sum(a[0] * b[1] - a[1] * b[0]
                   for a, b in zip(points, points[1:] + points[:1]))) / 2


def winding_area(groups, x, y):
    """Exact pixel area for an intersection of nonzero/even-odd path groups.

    ISO 32000-2 8.5.3.3.2–3 defines the interiors, and 8.5.4 defines their
    geometric intersection. Vertical slabs between vertices and intersections
    have a fixed edge order; their filled trapezoids integrate in Fraction.
    This includes self-intersections and coincident edges, without sampling.
    """
    edges, cuts = [], {F(x), F(x + 1)}
    for group, (_, contours) in enumerate(groups):
        for contour in contours:
            points = clipped_polygon(contour, x, y)
            for a, b in zip(points, points[1:] + points[:1]):
                if a[0] != b[0]:
                    edges.append((a, b, group, 1 if b[0] > a[0] else -1))
                    cuts.update((a[0], b[0]))
    def cross(a, b):
        return a[0] * b[1] - a[1] * b[0]
    for i, (a, b, _, _) in enumerate(edges):
        direction = tuple(q - p for p, q in zip(a, b))
        for c, d, _, _ in edges[:i]:
            other = tuple(q - p for p, q in zip(c, d))
            denominator = cross(direction, other)
            if denominator:
                offset = tuple(q - p for p, q in zip(a, c))
                t = cross(offset, other) / denominator
                u = cross(offset, direction) / denominator
                if 0 < t < 1 and 0 < u < 1:
                    cuts.add(a[0] + t * direction[0])
    cuts, area = sorted(cuts), F(0)
    for left, right in zip(cuts, cuts[1:]):
        middle, active = (left + right) / 2, []
        for a, b, group, delta in edges:
            if min(a[0], b[0]) <= left and right <= max(a[0], b[0]):
                def height(at):
                    return a[1] + (b[1] - a[1]) * (at - a[0]) / (b[0] - a[0])
                active.append((height(middle), height(left), height(right), group, delta))
        active.sort()
        winding = [0] * len(groups)
        for edge, following in zip(active, active[1:]):
            _, top_left, top_right, group, delta = edge
            winding[group] += delta
            if all((value != 0 if rule == "nonzero" else value % 2 != 0)
                   for value, (rule, _) in zip(winding, groups)):
                area += (following[1] - top_left + following[2] - top_right) * (right - left) / 2
    return area


def pgm(areas):
    return b"P5\n8 8\n255\n" + bytes(max(0, min(255, int(a * 255 + F(1, 2)))) for a in areas)


def polygon_case(name, points, path=None, requirement="raster-exact-area"):
    if path is None:
        path = "m %s %s\n" % tuple(map(float, points[0])) + "".join("l %s %s\n" % tuple(map(float, p)) for p in points[1:]) + "h\n"
    return Vector(name, {".path": path.encode(), ".pgm": pgm(
        clipped_area(points, x, y) for y in range(SIZE) for x in range(SIZE))},
        (requirement,), "Project-policy rational polygon/pixel intersection area", extra={"tolerance": 1})


def sampled(name, path, predicate, requirement="raster-strokes"):
    rate = 128
    values = []
    for y in range(SIZE):
        for x in range(SIZE):
            count = sum(predicate(x + (sx + .5) / rate, y + (sy + .5) / rate)
                        for sy in range(rate) for sx in range(rate))
            values.append(F(count, rate * rate))
    return Vector(name, {".path": path.encode(), ".pgm": pgm(values)},
                  (requirement,), "128x128 midpoint samples of geometric definition",
                  extra={"tolerance": 3, "sample_rate": rate})



def convex_miter_cases():
    """Parallel supporting lines define convex miter strokes, in Fraction."""
    def cross(a, b):
        return a[0] * b[1] - a[1] * b[0]

    def offset_lines(points, distance):
        lines = []
        for a, b in zip(points, points[1:] + points[:1]):
            dx, dy = b[0] - a[0], b[1] - a[1]
            square = dx * dx + dy * dy
            numerator, denominator = math.isqrt(square.numerator), math.isqrt(square.denominator)
            length = F(numerator, denominator)
            assert length * length == square
            normal = (-distance * dy / length, distance * dx / length)
            lines.append((tuple(p + n for p, n in zip(a, normal)),
                          tuple(p + n for p, n in zip(b, normal))))
        return lines

    def intersection(first, second):
        a, b = first
        c, d = second
        v, w = tuple(q - p for p, q in zip(a, b)), tuple(q - p for p, q in zip(c, d))
        t = cross(tuple(q - p for p, q in zip(a, c)), w) / cross(v, w)
        return tuple(p + t * q for p, q in zip(a, v))

    def interior(points, lines):
        for a, b in lines:
            direction = tuple(q - p for p, q in zip(a, b))
            def side(p):
                return cross(direction, tuple(q - r for r, q in zip(a, p)))
            output = []
            for p, q in zip(points, points[1:] + points[:1]):
                sp, sq = side(p), side(q)
                if sp >= 0:
                    output.append(p)
                if (sp >= 0) != (sq >= 0):
                    output.append(tuple(x + sp / (sp - sq) * (y - x) for x, y in zip(p, q)))
            points = output
        return points

    shapes = [
        ("rectangle", [(2, 2), (6, 2), (6, 6), (2, 6)], F(1, 2)),
        ("triangle", [(1, 1), (7, 1), (7, F(11, 2))], F(1, 2)),
        ("octagon", [(1, 2), (F(5, 2), 0), (F(11, 2), 0), (7, 2),
                     (7, 5), (F(11, 2), 7), (F(5, 2), 7), (1, 5)], F(3, 4)),
        ("closed-hole", [(2, 2), (6, 2), (6, 6), (2, 6)], F(4)),
        ("reversed-side", [(2, 2), (6, 2), (6, 6), (2, 6)], F(6)),
    ]
    directions = [(5, 0), (12, 5), (4, 3), (3, 4), (5, 12),
                  (0, 5), (-5, 12), (-3, 4), (-4, 3), (-12, 5)]
    steps = directions + [(-x, -y) for x, y in directions]
    polygon, current = [], (0, 0)
    for x, y in steps:
        polygon.append(current)
        current = (current[0] + x, current[1] + y)
    assert current == (0, 0)
    offset = tuple(F(8) / 2 - F(min(p[a] for p in polygon) + max(p[a] for p in polygon), 32)
                   for a in (0, 1))
    polygon = [(F(x, 16) + offset[0], F(y, 16) + offset[1]) for x, y in polygon]
    shapes.extend((name, polygon, width) for name, width in
                  [("icosagon", F(1, 2)), ("thin", F(1, 64)), ("wide", F(4))])
    for name, points, width in shapes:
        points = [tuple(map(F, p)) for p in points]
        lines = offset_lines(points, -width / 2)
        outer = [intersection(lines[i - 1], lines[i]) for i in range(len(lines))]
        inner = interior(points, offset_lines(points, width / 2))
        expected = pgm(clipped_area(outer, x, y) - clipped_area(inner, x, y)
                       for y in range(SIZE) for x in range(SIZE))
        for order in ("forward", "reverse"):
            ordered = points if order == "forward" else list(reversed(points))
            path = f"stroke {float(width)} butt miter 10\n"
            path += "".join(("m" if i == 0 else "l") + f" {float(x)} {float(y)}\n"
                            for i, (x, y) in enumerate(ordered)) + "h\n"
            yield Vector(f"convex-miter-{name}-{order}", {".path": path.encode(), ".pgm": expected},
                         ("raster-strokes",), "Rational parallel supporting lines and inward half-plane intersection",
                         extra={"tolerance": 0})

    # Sixteen strict turns exercise the boundary construction; each edge has
    # rational length, so the supporting-line oracle remains entirely exact.
    quadrant = [(5, 0), (12, 5), (3, 4), (5, 12)]
    steps16 = []
    for quarter in range(4):
        for x, y in quadrant:
            for _ in range(quarter):
                x, y = -y, x
            steps16.append((F(x, 16), F(y, 16)))
    for name, width, miter, reverse_side in [
        ("sixteen-thin", F(1, 64), F(10), False),
        ("sixteen-wide", F(3, 2), F(10), False),
        ("sixteen-filled", F(8), F(10), False),
        ("sixteen-near-miter", F(3, 4), F(1037, 1000), False),
        ("sixteen-reversed-inner", F(3, 4), F(10), True),
    ]:
        points, current = [], (F(0), F(0))
        for i, (x, y) in enumerate(steps16):
            points.append(current)
            factor = F(1, 1024) if reverse_side and i in (0, 8) else F(1)
            current = (current[0] + factor * x, current[1] + factor * y)
        assert current == (0, 0)
        shift = tuple(F(4) - (min(p[a] for p in points) + max(p[a] for p in points)) / 2
                      for a in (0, 1))
        points = [(x + shift[0], y + shift[1]) for x, y in points]
        lines = offset_lines(points, -width / 2)
        outer = [intersection(lines[i - 1], lines[i]) for i in range(len(lines))]
        inner = interior(points, offset_lines(points, width / 2))
        expected = pgm(clipped_area(outer, x, y) - clipped_area(inner, x, y)
                       for y in range(SIZE) for x in range(SIZE))
        for order in ("forward", "reverse"):
            ordered = points if order == "forward" else list(reversed(points))
            path = f"stroke {float(width)} butt miter {float(miter)}\n"
            path += "".join(("m" if i == 0 else "l") + f" {float(x)} {float(y)}\n"
                            for i, (x, y) in enumerate(ordered)) + "h\n"
            yield Vector(f"convex-miter-{name}-{order}", {".path": path.encode(), ".pgm": expected},
                         ("raster-strokes",), "Sixteen rational sides, exact parallel offsets and inward half-plane intersection",
                         extra={"tolerance": 0})
            commands = "1 0 0 -1 0 8 cm 0 g 0 0 8 8 re f 1 G "
            commands += f"{float(width)} w 0 J 0 j {float(miter)} M "
            commands += " ".join(f"{float(x)} {float(y)} " + ("m" if i == 0 else "l")
                                 for i, (x, y) in enumerate(ordered)) + " h S"
            pdf = document([
                b"<< /Type /Catalog /Pages 2 0 R >>",
                b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
                b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 8 8] /Resources << >> /Contents 4 0 R >>",
                stream(commands.encode()),
            ])
            ppm = b"P6\n8 8\n255\n" + b"".join(bytes([v, v, v]) for v in expected[-SIZE * SIZE:])
            yield Case(f"convex-miter-{name}-{order}-pdf", pdf, RENDER, ("raster-strokes",),
                       "Sixteen-sided white miter stroke on black, exact rational pixel area", oracle=ppm)

    shifted = [(x + F(3, 4), y + F(1, 2)) for x, y in polygon]
    triangle = [(F(1), F(1)), (F(5), F(1)), (F(5), F(4))]
    compounds = [
        ("opposed", F(3, 4), [(polygon, False), (shifted, True)]),
        ("reversed", F(3, 4), [(polygon, True), (shifted, True)]),
        ("filled", F(4), [(polygon, True), (shifted, False)]),
        ("fallback", F(3, 4), [(polygon, True), (triangle, False)]),
    ]
    for name, width, parts in compounds:
        path = f"stroke {float(width)} butt miter 10\n"
        contours = []
        for points, reverse in parts:
            lines = offset_lines(points, -width / 2)
            outer = [intersection(lines[i - 1], lines[i]) for i in range(len(lines))]
            inner = interior(points, offset_lines(points, width / 2))
            contours.extend([outer, list(reversed(inner))])
            ordered = list(reversed(points)) if reverse else points
            path += "".join(("m" if i == 0 else "l") + f" {float(x)} {float(y)}\n"
                            for i, (x, y) in enumerate(ordered)) + "h\n"
        yield Vector(f"convex-miter-compound-{name}", {
            ".path": path.encode(),
            ".pgm": pgm(winding_area([("nonzero", contours)], x, y)
                        for y in range(SIZE) for x in range(SIZE))},
            ("raster-strokes",), "Exact rational union of independently offset stroke regions",
            extra={"tolerance": 0})


def cases():
    yield from convex_miter_cases()
    # Coincident quadrilaterals have the same union as one contour. Even-odd
    # coverage depends only on parity, including at the cell-density boundary.
    quad = [(0, 0), (1, F(1, 4)), (1, F(5, 4)), (0, 1)]
    contour = "m 0 0\nl 1 .25\nl 1 1.25\nl 0 1\nh\n"
    for count in (128, 129):
        for rule in ("nonzero", "evenodd"):
            visible = rule == "nonzero" or count % 2 != 0
            yield Vector(f"quad-union-{count}-{rule}", {
                ".path": (rule + "\n" + contour * count).encode(),
                ".pgm": pgm(clipped_area(quad, x, y) if visible else F(0)
                            for y in range(SIZE) for x in range(SIZE))},
                ("raster-exact-area",),
                "Rational union/parity of coincident slanted quadrilaterals",
                extra={"tolerance": 0})
    yield polygon_case("triangle", [(F(1, 4), F(1, 4)), (F(29, 4), F(5, 4)), (F(7, 4), F(27, 4))])
    yield polygon_case("concave", [(-2, 1), (7, 1), (7, 3), (3, 3), (3, 7), (-2, 7)])
    yield polygon_case("fractional-rectangle", [(F(1, 4), F(1, 2)), (F(21, 4), F(1, 2)), (F(21, 4), F(13, 2)), (F(1, 4), F(13, 2))])
    # Two triangles separated at the crossing give a bow-tie's geometric area.
    triangles = [[(1, 1), (7, 1), (4, 4)], [(1, 7), (7, 7), (4, 4)]]
    yield Vector("self-intersection", {".path": b"m 1 1\nl 7 7\nl 1 7\nl 7 1\nh\n",
        ".pgm": pgm(sum(clipped_area(p, x, y) for p in triangles) for y in range(8) for x in range(8))},
        ("raster-exact-area",), "Bow tie split analytically into disjoint triangles", extra={"tolerance": 1})
    # Fractional overlap: winding must be selected before area is integrated.
    for rule in ("nonzero", "evenodd"):
        a = [(F(1, 4), F(1, 4)), (F(3, 4), F(1, 4)), (F(3, 4), F(3, 4)), (F(1, 4), F(3, 4))]
        b = [(F(1, 2), F(1, 4)), (1, F(1, 4)), (1, F(3, 4)), (F(1, 2), F(3, 4))]
        intersection = [(F(1, 2), F(1, 4)), (F(3, 4), F(1, 4)), (F(3, 4), F(3, 4)), (F(1, 2), F(3, 4))]
        yield Vector("overlap-" + rule, {".path": (rule + "\nm .25 .25\nl .75 .25\nl .75 .75\nl .25 .75\nh\nm .5 .25\nl 1 .25\nl 1 .75\nl .5 .75\nh\n").encode(),
            ".pgm": pgm(clipped_area(a,x,y) + clipped_area(b,x,y) - (1 if rule == "nonzero" else 2) * clipped_area(intersection,x,y) for y in range(8) for x in range(8))},
            ("raster-exact-area",), "Exact set union or symmetric difference of fractional rectangles", extra={"tolerance": 1})
    for cap in ("butt", "round", "square"):
        def hit(x, y, cap=cap):
            if cap == "butt":
                return 2 <= x <= 6 and 3 <= y <= 5
            if cap == "square":
                return 1 <= x <= 7 and 3 <= y <= 5
            return (x - min(6, max(2, x)))**2 + (y - 4)**2 <= 1
        yield sampled("cap-" + cap, f"stroke 2 {cap} miter 10\nm 2 4\nl 6 4\n", hit)
    yield polygon_case("square-dot", [], "stroke 3 square miter 10\nm 4 4\nl 4 4\n", "raster-strokes")
    miter = [(1,6),(3,6),(3,3),(6,3),(6,1),(1,1)]
    bevel = [(1,6),(3,6),(3,3),(6,3),(6,1),(2,1),(1,2)]
    for name, join, limit, outline in [("join-miter", "miter", 10, miter), ("join-bevel", "bevel", 10, bevel), ("miter-limit", "miter", 1, bevel)]:
        yield polygon_case(name, outline, f"stroke 2 butt {join} {limit}\nm 2 6\nl 2 2\nl 6 2\n", "raster-strokes")
    yield sampled("join-round", "stroke 2 butt round 10\nm 2 6\nl 2 2\nl 6 2\n",
                  lambda x,y: (1<=x<=3 and 2<=y<=6) or (2<=x<=6 and 1<=y<=3) or (x<=2 and y<=2 and (x-2)**2+(y-2)**2<=1))
    # Odd arrays repeat twice. Phase 1 in [2,1,1] yields on intervals
    # [0,1], [2,3], [5,6], [7,8] along an eight-unit line.
    yield sampled("dash-odd-phase", "stroke 2 butt miter 10\ndash 1 2 1 1\nm 0 4\nl 8 4\n",
                  lambda x,y: 3<=y<=5 and any(a<=x<=b for a,b in [(0,1),(2,3),(5,6),(7,8)]))
    outer = [(1,1),(7,1),(7,7),(1,7)]
    inner = [(3,3),(5,3),(5,5),(3,5)]
    yield Vector("closed-dash-seam", {".path": b"stroke 2 butt miter 10\ndash 0 16 8\nm 2 2\nl 6 2\nl 6 6\nl 2 6\nh\n",
        ".pgm": pgm(clipped_area(outer,x,y)-clipped_area(inner,x,y) for y in range(8) for x in range(8))},
        ("raster-strokes",), "An on-dash ending exactly at a closed seam keeps its join", extra={"tolerance":1})
    yield sampled("zero-on-dash", "stroke 1 round miter 10\ndash 0 0 1\nm 1 4\nl 7 4\n",
                  lambda x,y: any((x-c)**2+(y-4)**2 <= .25 for c in range(1,8)))
    diamonds=[[(c-1,c),(c,c+1),(c+1,c),(c,c-1)] for c in (1,3,5,7)]
    yield Vector('zero-square-oblique',{'.path':b'stroke 1.4142135623730951 square miter 10\ndash 0 0 2.8284271247461903\nm 1 1\nl 7.5 7.5\n','.pgm':pgm(sum(clipped_area(p,x,y) for p in diamonds) for y in range(8) for x in range(8))},('raster-strokes',),'Zero-length dash square caps retain the oblique underlying tangent; rational diamond areas',extra={'tolerance':1})
    yield sampled("round-dot", "stroke 3 round miter 10\nm 4 4\nl 4 4\n", lambda x,y: (x-4)**2+(y-4)**2 <= 2.25)
    # Closed cubic y=1+3t(1-t), x=1+6t; no circular-arc approximation.
    yield sampled("cubic", "m 1 1\nc 3 2 5 2 7 1\nh\n",
                  lambda x,y: 1 <= x <= 7 and 1 <= y <= 1 + 3*((x-1)/6)*(1-(x-1)/6), "raster-curves")

    yield from crossing_cases()
    yield from winding_cases()
    yield from collapsing_cases()
    yield from winding_stress_cases()
    yield from micro_sliver_cases()


def intersect_convex(subject, clip):
    """Rational convex intersection independent of the scanline implementation."""
    subject=[tuple(map(F,p)) for p in subject]
    clip=[tuple(map(F,p)) for p in clip]
    orientation=sum(a[0]*b[1]-a[1]*b[0] for a,b in zip(clip,clip[1:]+clip[:1]))
    for c,d in zip(clip,clip[1:]+clip[:1]):
        if not subject:
            return []
        result=[]
        def distance(p): return (d[0]-c[0])*(p[1]-c[1])-(d[1]-c[1])*(p[0]-c[0])
        for a,b in zip(subject,subject[1:]+subject[:1]):
            da,db=distance(a),distance(b)
            ain=da*orientation>=0;bin=db*orientation>=0
            if ain: result.append(a)
            if ain!=bin:
                t=da/(da-db)
                result.append(tuple(x+t*(y-x) for x,y in zip(a,b)))
        subject=result
    return subject


def crossing_cases():
    import random
    rng=random.Random(917431)
    def path(points):
        return ('m %g %g\n'%tuple(points[0])+''.join('l %g %g\n'%tuple(p) for p in points[1:])+'h\n')
    for i in range(16):
        a=[(F(rng.randrange(-8,41),4),F(rng.randrange(-8,41),4)) for _ in range(3)]
        b=[(F(rng.randrange(-8,41),4),F(rng.randrange(-8,41),4)) for _ in range(3)]
        def positive(points):
            area=sum(p[0]*q[1]-p[1]*q[0] for p,q in zip(points,points[1:]+points[:1]))
            return list(reversed(points)) if area<0 else points
        a,b=positive(a),positive(b)
        intersection=intersect_convex(a,b)
        mode=i%4
        text=('evenodd\n' if mode==1 else '')+path(a)+('clip\n' if mode==3 else '')+path(list(reversed(b)) if mode==2 else b)
        values=[]
        for y in range(8):
            for x in range(8):
                overlap=clipped_area(intersection,x,y) if intersection else F(0)
                values.append(overlap if mode==3 else clipped_area(a,x,y)+clipped_area(b,x,y)-(2 if mode in (1,2) else 1)*overlap)
        yield Vector('crossing-set-'+str(i),{'.path':text.encode(),'.pgm':pgm(values)},('raster-exact-area',),'Rational union, parity, opposite-winding or geometric intersection of two independently generated triangles',extra={'tolerance':1})


def contour_path(points):
    """Round-trip binary64 coordinates; dyadic stress inputs remain exact."""
    def pair(point):
        return " ".join(format(float(value), ".17g") for value in point)
    return "m " + pair(points[0]) + "\n" + "".join("l " + pair(p) + "\n" for p in points[1:]) + "h\n"


def winding_cases():
    square = [(F(1, 4), F(1, 4)), (F(3, 4), F(1, 4)),
              (F(3, 4), F(3, 4)), (F(1, 4), F(3, 4))]
    shifted = [(x + F(1, 8), y + F(1, 8)) for x, y in square]
    shapes = {
        "boundary-triangle": [[[(0, 0), (1, 0), (0, 1)]]],
        "opposite-chains": [[[(-1, F(-1, 2)), (F(3, 2), F(-1, 2)), (-1, F(9, 10))],
                              [(-1, F(3, 2)), (F(3, 2), F(3, 2)), (-1, F(2, 5))]]],
        "crossed-rules": [[[(-1, F(3, 8)), (2, F(3, 8)), (2, F(5, 8)), (-1, F(5, 8))],
                            [(F(3, 8), -1), (F(5, 8), -1), (F(5, 8), 2), (F(3, 8), 2)]]],
        "tiny-bowtie": [[[(F(1, 8), F(1, 8)), (F(7, 8), F(7, 8)),
                          (F(1, 8), F(7, 8)), (F(7, 8), F(1, 8))]]],
        "overlapping-contours": [[square, shifted]],
        "coincident-opposite": [[square, square[::-1]]],
        "touching-contours": [[square, [(x + F(1, 2), y) for x, y in square]]],
        "same-clip": [[square], [square]],
        "nested-clips": [[[(-1, -1), (2, -1), (2, 2), (-1, 2)]],
                         [[(-1, 0), (2, 0), (2, 1), (-1, 1)]], [square], [shifted]],
        "crossing-clips": [[[(0, 0), (1, 0), (0, 1)]], [[(0, 0), (1, 1), (0, 1)]]],
        "star-in-cell": [[[(F(1, 2), 0), (F(7, 8), 1), (0, F(3, 8)),
                           (1, F(3, 8)), (F(1, 8), 1)]]],
        "double-crossing-chains": [[[(F(1, 8), F(1, 8)), (F(7, 8), F(3, 8)),
                                    (F(1, 8), F(5, 8)), (F(7, 8), F(7, 8)),
                                    (F(7, 8), F(1, 8)), (F(1, 8), F(7, 8))]]],
        "thin-rule": [[[(F(1, 8), F(1, 8)), (F(27, 200), F(1, 8)),
                        (F(27, 200), F(7, 8)), (F(1, 8), F(7, 8))]]],
        "coincident-same": [[square, square]],
        "collinear": [[[(1, 1), (2, 2), (3, 3)]]],
        "huge-endpoints": [[[(-10**12, -10**12), (10**12, 10**12), (10**12, -10**12)]]],
        "translated-half-tie": [[[(-2, 5 + F(3, 1 << 22)),
                                    (F(-3, 1 << 22), 2 + F(2, 1 << 22)),
                                    (7 + F(3, 1 << 22), 4 - F(2, 1 << 22))]]],
        "rounding-threshold": [[[(0, 0), (F(1, 1020), 0), (F(1, 510), 0),
                                  (F(1, 510), 1), (0, 1)]]],
    }
    for name, contours in shapes.items():
        for rule in ("nonzero", "evenodd"):
            groups = [(rule, group) for group in contours]
            text = (rule + "\n") + "clip\n".join("".join(contour_path(c) for c in group) for group in contours)
            yield Vector("winding-" + name + "-" + rule,
                         {".path": text.encode(), ".pgm": pgm(
                             winding_area(groups, x, y) for y in range(SIZE) for x in range(SIZE))},
                         ("raster-exact-area",),
                         "Rational vertical-slab winding integration with geometric clip intersection",
                         extra={"tolerance": 1})


def collapsing_cases():
    grid = 1 << 20
    for count in (127, 128, 129, 512, 513):
        for axis in ("horizontal", "vertical"):
            for direction in ("forward", "reverse"):
                contours = []
                for i in range(count):
                    center = F(4 * i + 2, grid)
                    lo, hi = center - F(1, 4 * grid), center + F(1, 4 * grid)
                    points = [(F(1, 8), lo), (F(7, 8), lo), (F(7, 8), hi), (F(1, 8), hi)]
                    if axis == "vertical":
                        points = [(y, x) for x, y in points]
                    if direction == "reverse":
                        points.reverse()
                    contours.append(points)
                # The rectangles are disjoint. Their union and parity areas are
                # count * (3/4) * 2^-21, independently of the contour direction.
                # Four original segments per contour straddle the 512 cutoff
                # at counts 127, 128 and 129, even if snapping removes them all.
                expected = [F(3 * count, 8 * grid)] + [F(0)] * (SIZE * SIZE - 1)
                for rule in ("nonzero", "evenodd"):
                    name = f"collapse-{axis}-{direction}-{count}-{rule}"
                    text = rule + "\n" + "".join(contour_path(c) for c in contours)
                    yield Vector(name, {".path": text.encode(), ".pgm": pgm(expected)},
                                 ("raster-exact-area",),
                                 "Exact disjoint dyadic rectangle areas across the original-segment density cutoff",
                                 extra={"tolerance": 0})


def winding_stress_cases():
    import random
    rng = random.Random(391771)
    for case in range(16):
        groups = []
        for _ in range(1 + case % 3):
            contours = []
            for _ in range(1 + case % 2):
                points = [(F(rng.randrange(-8, 72), 8), F(rng.randrange(-8, 72), 8))
                          for _ in range(3 + case % 5)]
                if all(p == points[0] for p in points):
                    points[-1] = (points[-1][0] + F(1, 8), points[-1][1])
                contours.append(points)
            groups.append(contours)
        for rule in ("nonzero", "evenodd"):
            text = rule + "\n" + "clip\n".join("".join(contour_path(c) for c in group) for group in groups)
            yield Vector(f"winding-random-{case}-{rule}",
                         {".path": text.encode(), ".pgm": pgm(
                             winding_area([(rule, g) for g in groups], x, y)
                             for y in range(SIZE) for x in range(SIZE))},
                         ("raster-exact-area",),
                         "Seeded rational winding arrangements with self-intersections and multiple clip groups",
                         extra={"tolerance": 1})


def micro_sliver_cases():
    count, width = 1000, F(1, 10**6)
    for axis in ("horizontal", "vertical"):
        contours = []
        for i in range(count):
            lo = F(1, 8) + 4 * i * width
            points = [(lo, F(1, 8)), (lo + width, F(1, 8)),
                      (lo + width, F(7, 8)), (lo, F(7, 8))]
            if axis == "horizontal":
                points = [(y, x) for x, y in points]
            contours.append(points)
        expected = [count * width * F(3, 4)] + [F(0)] * (SIZE * SIZE - 1)
        for rule in ("nonzero", "evenodd"):
            yield Vector(f"dense-micro-slivers-{axis}-{rule}",
                         {".path": (rule + "\n" + "".join(contour_path(c) for c in contours)).encode(),
                          ".pgm": pgm(expected)}, ("raster-exact-area",),
                         "Exact area of 1000 disjoint millionth-pixel slivers in one cell",
                         extra={"tolerance": 0})
