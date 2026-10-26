"""Closed-form checks for the rounded-rectangle fixture oracle."""
import math
import unittest
from fixturegen.rounded_annotations import area, ellipse_region

class RoundedAreaTests(unittest.TestCase):
    def test_quadrants_and_rectangular_parts(self):
        self.assertAlmostEqual(ellipse_region((-1,-1,1,1),(0,0,1,1)),math.pi/4)
        self.assertEqual(ellipse_region((-1,-1,1,1),(2,2,3,3)),0.)
        self.assertAlmostEqual(area((0,0,4,4),(1,1),0,0),math.pi/4)
        self.assertEqual(area((0,0,4,4),(1,1),1,1),1.)
        self.assertEqual(area((0,0,4,4),(0,1),0,0),1.)
        self.assertAlmostEqual(sum(area((0,0,6,4),(2,1),x,y) for x in range(6) for y in range(4)),24-2*(4-math.pi))

if __name__=='__main__':
    unittest.main()
