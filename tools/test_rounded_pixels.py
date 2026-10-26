"""Closed-form checks for the rounded-rectangle fixture oracle."""
import math
import unittest
from fixturegen.rounded_annotations import area, ellipse_region, stroke_contours
from fixturegen.cloud_annotations import polygon_coverage

class RoundedAreaTests(unittest.TestCase):
    def test_quadrants_and_rectangular_parts(self):
        self.assertAlmostEqual(ellipse_region((-1,-1,1,1),(0,0,1,1)),math.pi/4)
        self.assertEqual(ellipse_region((-1,-1,1,1),(2,2,3,3)),0.)
        self.assertAlmostEqual(area((0,0,4,4),(1,1),0,0),math.pi/4)
        self.assertEqual(area((0,0,4,4),(1,1),1,1),1.)
        self.assertEqual(area((0,0,4,4),(0,1),0,0),1.)
        self.assertAlmostEqual(sum(area((0,0,6,4),(2,1),x,y) for x in range(6) for y in range(4)),24-2*(4-math.pi))

    def test_closed_parallel_curve_area_matches_steiner_difference(self):
        # P=4*(22-12)+2*pi*6; area(outside)-area(inside)=width*P.
        contours=stroke_contours(radii=(6.,6.))
        coverage=polygon_coverage(contours,32)
        self.assertAlmostEqual(sum(coverage),80+24*math.pi,delta=.0001)

    def test_elliptical_offsets_follow_unit_normals_at_axes(self):
        outer,inner=stroke_contours()
        self.assertAlmostEqual(outer[0][0],19.)
        self.assertAlmostEqual(outer[0][1],4.)
        self.assertAlmostEqual(inner[-1][0],19.)
        self.assertAlmostEqual(inner[-1][1],6.)
        self.assertAlmostEqual(min(x for x,_ in outer),4.)
        self.assertAlmostEqual(max(x for x,_ in outer),28.)
        self.assertAlmostEqual(min(y for _,y in outer),4.)
        self.assertAlmostEqual(max(y for _,y in outer),28.)

    def test_dash_ends_at_exact_circular_arclength_with_butt_cap(self):
        first=stroke_contours(radii=(6.,6.),dash=(3,3))[0]
        outer_end=first[len(first)//2-1];inner_end=first[len(first)//2]
        self.assertAlmostEqual(outer_end[0],21+7*math.sin(.5))
        self.assertAlmostEqual(outer_end[1],11-7*math.cos(.5))
        self.assertAlmostEqual(inner_end[0],21+5*math.sin(.5))
        self.assertAlmostEqual(inner_end[1],11-5*math.cos(.5))

    def test_circular_dash_pixel_is_an_exact_trapezoid(self):
        # Pixel [22,23]×[26,27] is wholly inside the annulus about (21,21).
        # Only the dash end cuts it; y=tan(theta)*x in circle-local coordinates.
        # The preceding corner and line consume 3*pi+10 of the on-dash end at27.
        theta=(17-3*math.pi)/6
        expected=2-5.5/math.tan(theta)
        contours=stroke_contours(radii=(6.,6.),dash=(3,3))
        pixel=[[(x-22,y-26) for x,y in contour] for contour in contours]
        self.assertAlmostEqual(polygon_coverage(pixel,1)[0],expected,delta=.000001)
        self.assertEqual(255-int(255*expected+.5),192)

    def test_short_circular_dash_at_144_dpi_has_exact_trapezoid_coverage(self):
        # Pixel 725 maps to [10.5,11]×[26,26.5], wholly in the annulus
        # about (11,21). The on interval ends at source arclength 39.
        theta=19/6-math.pi/2
        expected=-10.5/math.tan(theta)
        contours=stroke_contours(radii=(6.,6.),dash=(3,3))
        pixel=[[(2*x-21,2*y-52) for x,y in contour] for contour in contours]
        self.assertAlmostEqual(polygon_coverage(pixel,1)[0],expected,delta=.000001)
        self.assertEqual(255-int(255*expected+.5),188)

if __name__=='__main__':
    unittest.main()
