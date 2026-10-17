"""Independent geometric coverage oracles (no engine imports).

Simple polygon coverage uses rational Sutherland-Hodgman clipping against each
pixel and the shoelace formula. Curves and circular pens use 128x128 midpoint
sampling of their geometric definitions; their maximum accepted difference is
3/255 (including Rust's 1/256-pixel flattening tolerance). Polygon cases allow
1/255 for final rounding. Path coordinates here have a top-left origin.
"""
from fractions import Fraction as F
import math

from .model import Vector

AREA = "raster"
SIZE = 8


def clipped_area(points, x, y):
    points = [(F(a), F(b)) for a, b in points]
    for axis, bound, sign in ((0, x, 1), (0, x + 1, -1), (1, y, 1), (1, y + 1, -1)):
        output = []
        if not points:
            return F(0)
        for a, b in zip(points, points[1:] + points[:1]):
            ain = sign * (a[axis] - bound) >= 0
            bin = sign * (b[axis] - bound) >= 0
            if ain:
                output.append(a)
            if ain != bin:
                t = (bound - a[axis]) / (b[axis] - a[axis])
                output.append(tuple(p + t * (q - p) for p, q in zip(a, b)))
        points = output
    if not points:
        return F(0)
    return abs(sum(a[0] * b[1] - a[1] * b[0]
                   for a, b in zip(points, points[1:] + points[:1]))) / 2


def pgm(areas):
    return b"P5\n8 8\n255\n" + bytes(max(0, min(255, int(a * 255 + F(1, 2)))) for a in areas)


def polygon_case(name, points, path=None):
    if path is None:
        path = "m %s %s\n" % tuple(map(float, points[0])) + "".join("l %s %s\n" % tuple(map(float, p)) for p in points[1:]) + "h\n"
    return Vector(name, {".path": path.encode(), ".pgm": pgm(
        clipped_area(points, x, y) for y in range(SIZE) for x in range(SIZE))},
        ("raster-exact-area",), "Rational polygon/pixel intersection area", extra={"tolerance": 1})


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


def cases():
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
    yield polygon_case("square-dot", [(2.5,2.5),(5.5,2.5),(5.5,5.5),(2.5,5.5)], "stroke 3 square miter 10\nm 4 4\nl 4 4\n")
    miter = [(1,6),(3,6),(3,3),(6,3),(6,1),(1,1)]
    bevel = [(1,6),(3,6),(3,3),(6,3),(6,1),(2,1),(1,2)]
    for name, join, limit, outline in [("join-miter", "miter", 10, miter), ("join-bevel", "bevel", 10, bevel), ("miter-limit", "miter", 1, bevel)]:
        yield polygon_case(name, outline, f"stroke 2 butt {join} {limit}\nm 2 6\nl 2 2\nl 6 2\n")
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
    yield sampled("round-dot", "stroke 3 round miter 10\nm 4 4\nl 4 4\n", lambda x,y: (x-4)**2+(y-4)**2 <= 2.25)
    # Closed cubic y=1+3t(1-t), x=1+6t; no circular-arc approximation.
    yield sampled("cubic", "m 1 1\nc 3 2 5 2 7 1\nh\n",
                  lambda x,y: 1 <= x <= 7 and 1 <= y <= 1 + 3*((x-1)/6)*(1-(x-1)/6), "raster-curves")
