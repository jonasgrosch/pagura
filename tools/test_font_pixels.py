"""Analytic controls for the independent font-fixture area oracle."""
import math
import unittest

from fixturegen.font_pixels import contour_segments, line, signed_area


class FontAreaTests(unittest.TestCase):
    def test_rectangle_and_hole(self):
        def rectangle(a,b,c,d):
            points=[(a,b),(c,b),(c,d),(a,d)]
            return [line(x,y) for x,y in zip(points,points[1:]+points[:1])]
        outside=rectangle(-1,-1,2,2)
        hole=[tuple(reversed(s)) for s in reversed(rectangle(.25,.25,.75,.75))]
        self.assertAlmostEqual(signed_area(outside,(0,0,1,1)),1.)
        self.assertAlmostEqual(signed_area(outside+hole,(0,0,1,1)),.75)
        self.assertAlmostEqual(signed_area(outside,(3,0,4,1)),0.)

    def test_quadratic_arch_and_clipping(self):
        arch=[((0.,0.),(.5,2.),(1.,0.)),line((1.,0.),(0.,0.))]
        self.assertAlmostEqual(abs(signed_area(arch,(0,0,1,1))),2/3)
        self.assertAlmostEqual(abs(signed_area(arch,(0,0,.5,1))),1/3)
        self.assertAlmostEqual(abs(signed_area(arch,(0,0,1,.5))),2/3-1/(3*math.sqrt(2)))
        reversed_arch=[tuple(reversed(s)) for s in reversed(arch)]
        self.assertAlmostEqual(signed_area(reversed_arch,(0,0,1,1)),-signed_area(arch,(0,0,1,1)))
        # Exact de Casteljau halves preserve every clipped area.
        halves=[((0.,0.),(.25,1.),(.5,1.)),((.5,1.),(.75,1.),(1.,0.)),arch[-1]]
        for bounds in [(0,0,1,1),(.2,.3,.7,.8),(.8,-1,2,.1)]:
            self.assertAlmostEqual(signed_area(halves,bounds),signed_area(arch,bounds))

    def test_true_type_implied_on_curve_points(self):
        outline=[(0,0,True),(0,2,False),(2,2,False),(2,0,True)]
        expected=[((0,0),(0,2),(1,2)),((1,2),(2,2),(2,0)),line((2,0),(0,0))]
        self.assertEqual(contour_segments(outline),expected)
        # Rotation of a closed contour can start on an off-curve point.
        for shift in range(1,len(outline)):
            actual=contour_segments(outline[shift:]+outline[:shift])
            self.assertAlmostEqual(signed_area(actual,(0,0,2,2)),signed_area(expected,(0,0,2,2)))


if __name__ == '__main__':
    unittest.main()
