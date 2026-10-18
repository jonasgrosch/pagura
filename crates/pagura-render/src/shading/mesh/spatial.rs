//! Median-split bounds hierarchy; stream order resolves overlapping primitives.
use super::{Primitive, charge};
use pagura_core::{Meter, Result};
use pagura_raster::Point;
#[derive(Debug, Clone, Copy)]
pub(super) struct Bounds {
    lo: Point,
    hi: Point,
}
impl Bounds {
    pub(super) fn points(points: impl IntoIterator<Item = Point>) -> Self {
        let mut b = Self {
            lo: Point::new(f64::INFINITY, f64::INFINITY),
            hi: Point::new(f64::NEG_INFINITY, f64::NEG_INFINITY),
        };
        for p in points {
            b.lo.x = b.lo.x.min(p.x);
            b.lo.y = b.lo.y.min(p.y);
            b.hi.x = b.hi.x.max(p.x);
            b.hi.y = b.hi.y.max(p.y);
        }
        b
    }
    pub(super) fn contains(self, p: Point) -> bool {
        p.x >= self.lo.x && p.x <= self.hi.x && p.y >= self.lo.y && p.y <= self.hi.y
    }
    fn union(self, b: Self) -> Self {
        Self::points([self.lo, self.hi, b.lo, b.hi])
    }
}
impl Primitive {
    fn bounds(&self) -> Bounds {
        match self {
            Self::Triangle(t) => Bounds::points(t.points),
            Self::Patch(p) => Bounds::points(p.points),
        }
    }
}
#[derive(Debug)]
pub(super) struct Node {
    bounds: Bounds,
    kind: Kind,
}
#[derive(Debug)]
enum Kind {
    Leaf(usize, usize),
    Branch(usize, usize),
}
#[derive(Debug)]
pub(super) struct Index {
    nodes: Vec<Node>,
    order: Vec<usize>,
}
impl Index {
    pub(super) fn retained_bytes(&self) -> usize {
        self.nodes.capacity() * size_of::<Node>() + self.order.capacity() * size_of::<usize>()
    }

    pub(super) fn new(primitives: &[Primitive], bytes: &mut Meter) -> Result<Self> {
        let count = primitives.len();
        let mut index = Self {
            nodes: Vec::new(),
            order: Vec::new(),
        };
        bytes.reserve(&mut index.nodes, count.saturating_mul(2))?;
        bytes.reserve(&mut index.order, count)?;
        index.order.extend(0..count);
        index.build(primitives, 0, count);
        Ok(index)
    }
    fn build(&mut self, p: &[Primitive], start: usize, end: usize) -> usize {
        let bounds = self.order[start..end]
            .iter()
            .map(|i| p[*i].bounds())
            .reduce(Bounds::union)
            .unwrap_or_else(|| Bounds::points([]));
        let id = self.nodes.len();
        self.nodes.push(Node {
            bounds,
            kind: Kind::Leaf(start, end),
        });
        if end - start > 8 {
            let x = (bounds.hi.x - bounds.lo.x) >= (bounds.hi.y - bounds.lo.y);
            let center = |i: usize| {
                let b = p[i].bounds();
                if x { b.lo.x + b.hi.x } else { b.lo.y + b.hi.y }
            };
            let middle = (end - start) / 2;
            self.order[start..end]
                .select_nth_unstable_by(middle, |a, b| center(*a).total_cmp(&center(*b)));
            let a = self.build(p, start, start + middle);
            let b = self.build(p, start + middle, end);
            self.nodes[id].kind = Kind::Branch(a, b);
        }
        id
    }
    pub(super) fn query(
        &self,
        p: Point,
        visit: &mut impl FnMut(usize, &mut usize) -> Result<()>,
        work: &mut usize,
        maximum: usize,
    ) -> Result<()> {
        self.walk(0, p, visit, work, maximum)
    }
    fn walk(
        &self,
        id: usize,
        p: Point,
        visit: &mut impl FnMut(usize, &mut usize) -> Result<()>,
        work: &mut usize,
        maximum: usize,
    ) -> Result<()> {
        charge(work, maximum)?;
        let node = &self.nodes[id];
        if !node.bounds.contains(p) {
            return Ok(());
        }
        match node.kind {
            Kind::Leaf(a, b) => {
                for &i in &self.order[a..b] {
                    visit(i, work)?;
                }
            }
            Kind::Branch(a, b) => {
                self.walk(a, p, visit, work, maximum)?;
                self.walk(b, p, visit, work, maximum)?;
            }
        }
        Ok(())
    }
}
