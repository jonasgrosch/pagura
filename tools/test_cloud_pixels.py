"""Analytic controls for the independent cloud-stroke coverage oracle."""
import unittest

from fixturegen.cloud_annotations import polygon_coverage, stroke_contours


class CloudCoverageTests(unittest.TestCase):
    def test_rectangle_clipping_and_orientation(self):
        rectangle=[(-1,-1),(1.5,-1),(1.5,1.5),(-1,1.5)]
        expected=[1.,.5,.5,.25]
        self.assertEqual(polygon_coverage([rectangle],2),expected)
        self.assertEqual(polygon_coverage([list(reversed(rectangle))],2),expected)

    def test_overlap_is_a_union_and_reversed_holes_subtract(self):
        box=[(0,0),(2,0),(2,2),(0,2)]
        overlapping=[(.5,.5),(1.5,.5),(1.5,1.5),(.5,1.5)]
        self.assertEqual(polygon_coverage([box,overlapping],2),[1.]*4)
        self.assertEqual(polygon_coverage([box,list(reversed(overlapping))],2),[.75]*4)

    def test_crossing_edges_split_inside_a_pixel(self):
        bowtie=[(0,0),(1,1),(0,1),(1,0)]
        self.assertEqual(polygon_coverage([bowtie],1),[.5])

    def test_slanted_intersections_and_disconnected_islands(self):
        triangle=[(0,0),(2,0),(0,2)]
        self.assertEqual(polygon_coverage([triangle],2),[1.,.5,.5,0.])
        island=[(.5,1.5),(1,1.5),(1,2),(.5,2)]
        self.assertAlmostEqual(sum(polygon_coverage([triangle,island],2)),2.25)

    def test_concave_join_excludes_inner_loop_and_keeps_exterior_miter(self):
        # Two lobes meet at (16,8.5), with tangents (1,2) and (1,-2).
        # For the left lobe, squared distance from (16,7.4) minus 1/4 is
        # 225u^4-450u^3+314.25u^2-33u+.96. On [0,1/15] it exceeds
        # 284.25u^2-33u+.96 > .002. On [1/15,.07], |dy|>.12 and
        # |dx|>=.5; afterwards |dx|>=.525. The other lobe is symmetric.
        # Thus the tiny lower square is outside the half-unit stroke, even
        # though a bogus inner miter would extend to y=8.5-sqrt(5)/2.
        # The upper square is inside the exterior miter's literal triangle,
        # whose base endpoints are (16 +/- 1/sqrt(5),8.5+1/(2sqrt(5)))
        # and apex is (16,8.5+sqrt(5)/2).
        contours=stroke_contours('Square',1.,())
        for y,expected in [(7.4,0.),(9.5,1.)]:
            tiny=[[(5000*(x-15.9999),5000*(yy-(y-.0001))) for x,yy in contour] for contour in contours]
            self.assertAlmostEqual(polygon_coverage(tiny,1)[0],expected)


if __name__=='__main__':
    unittest.main()
