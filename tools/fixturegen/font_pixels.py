"""Independent analytic quadratic-outline coverage for bounded font fixtures.

Green's theorem gives signed pixel coverage as the contour integral of
clamp(x - left, 0, width) dy over bottom <= y <= top. Splitting each quadratic
at its intersections with four pixel edges leaves polynomial integrals, not
flattening or sampling. Font contours carry winding; selected simple Latin
glyphs have no same-direction overlapping contours. Opposite holes subtract.
Binary64 arithmetic is used only for the roots/integrals at small page scales;
analytic tests cover clipping, curved areas, holes, reversal and subdivision.
"""
import math
from functools import lru_cache

from .paint import bundled_points


def roots(a, b, c):
    if a == 0:
        return [] if b == 0 else [-c/b]
    discriminant=b*b-4*a*c
    if discriminant < 0:
        return []
    q=-.5*(b+math.copysign(math.sqrt(discriminant),b))
    return [-b/(2*a)] if q == 0 else [q/a,c/q]


def polynomial(points, axis):
    a,b,c=(point[axis] for point in points)
    return a-2*b+c,2*(b-a),a


def segment_area(points, bounds):
    left,bottom,right,top=bounds
    a,b,c=polynomial(points,0);d,e,f=polynomial(points,1)
    cuts={0.,1.}
    for coefficients,edge in [((a,b,c),left),((a,b,c),right),((d,e,f),bottom),((d,e,f),top)]:
        aa,bb,cc=coefficients
        cuts.update(t for t in roots(aa,bb,cc-edge) if 0<t<1)
    cuts=sorted(cuts);pieces=[]
    for lo,hi in zip(cuts,cuts[1:]):
        t=(lo+hi)/2;x=(a*t+b)*t+c;y=(d*t+e)*t+f
        if not bottom<y<top or x<=left:
            continue
        if x>=right:
            pieces.append((right-left)*(d*(hi*hi-lo*lo)+e*(hi-lo)))
        else:
            cc=c-left
            coefficients=(cc*e,(b*e+2*cc*d)/2,(a*e+2*b*d)/3,a*d/2)
            pieces.append(math.fsum(coefficient*(hi**power-lo**power)
                                   for power,coefficient in enumerate(coefficients,1)))
    return math.fsum(pieces)


def signed_area(segments, bounds):
    return math.fsum(segment_area(segment,bounds) for segment in segments)


def line(a,b):
    return a,tuple((x+y)/2 for x,y in zip(a,b)),b


def contour_segments(points):
    first,last=points[0],points[-1]
    if first[2]:
        start=first[:2];rest=points[1:]+[first]
    elif last[2]:
        start=last[:2];rest=points[:-1]+[last]
    else:
        start=tuple((a+b)/2 for a,b in zip(first[:2],last[:2]));rest=points+[(*start,True)]
    current=start;control=None;segments=[]
    for x,y,on in rest:
        point=(x,y)
        if on:
            segments.append(line(current,point) if control is None else (current,control,point))
            current=point;control=None
        elif control is None:
            control=point
        else:
            middle=tuple((a+b)/2 for a,b in zip(control,point))
            segments.append((current,control,middle));current=middle;control=point
    return segments


@lru_cache(maxsize=128)
def outline(path, character):
    contours,units=bundled_points(path,character)
    return tuple(segment for contour in contours for segment in contour_segments(contour)),units


def glyph_segments(path, character, size, origin):
    segments,units=outline(path,character)
    x,y=origin
    return [tuple((x+px*size/units,y+py*size/units) for px,py in segment)
            for segment in segments]
