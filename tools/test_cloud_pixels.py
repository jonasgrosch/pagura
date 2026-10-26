"""Analytic controls for the independent cloud-stroke coverage oracle."""
import unittest

from fixturegen.cloud_annotations import polygon_coverage


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


if __name__=='__main__':
    unittest.main()
