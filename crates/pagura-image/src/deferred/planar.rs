//! Baseline JPEG component planes on their original sample grids.

/// Three JPEG components (Y, Cb, Cr) in their native planes, with the block
/// flags a painter needs to skip unchanged background blocks. Rows are
/// converted to RGB only on demand.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Planar {
    pub(crate) width: usize,
    pub(crate) height: usize,
    /// Maximum horizontal and vertical sampling factors.
    pub(crate) maximum: [usize; 2],
    /// Per component: horizontal factor, vertical factor and plane pitch.
    pub(crate) geometry: [[usize; 3]; 3],
    /// Rounded, clamped component samples, including block padding.
    pub(crate) planes: [Vec<u8>; 3],
    /// Per component block: nonzero when it differs from the background.
    pub(crate) dirty: [Vec<u8>; 3],
    /// The value each plane was initialized with.
    pub(crate) backgrounds: [u8; 3],
}

impl Planar {
    /// Builds a planar image from Y, Cb and Cr planes in eight-by-eight
    /// block rows, as a baseline decoder stores them: component `c` has
    /// sampling factors `geometry[c][..2]` and row pitch `geometry[c][2]`.
    /// Returns None when the planes do not cover the image.
    #[must_use]
    pub fn new(
        (width, height): (usize, usize),
        maximum: [usize; 2],
        geometry: [[usize; 3]; 3],
        planes: [Vec<u8>; 3],
    ) -> Option<Self> {
        const BACKGROUNDS: [u8; 3] = [255, 128, 128];
        if width == 0 || height == 0 || !maximum.iter().all(|m| (1..=4).contains(m)) {
            return None;
        }
        let mut dirty: [Vec<u8>; 3] = Default::default();
        for (c, ([h, v, pitch], plane)) in geometry.iter().zip(&planes).enumerate() {
            let rows = plane.len().checked_div(*pitch)?;
            if !(1..=maximum[0]).contains(h)
                || !(1..=maximum[1]).contains(v)
                || pitch % 8 != 0
                || plane.len() % pitch != 0
                || rows % 8 != 0
                || pitch * maximum[0] < width * h
                || rows * maximum[1] <= (height - 1) * v
            {
                return None;
            }
            let stride = pitch / 8;
            dirty[c] = (0..stride * rows / 8)
                .map(|block| {
                    let (row, column) = (block / stride, block % stride);
                    let differs = (0..8).any(|y| {
                        let start = (row * 8 + y) * pitch + column * 8;
                        plane[start..start + 8].iter().any(|&s| s != BACKGROUNDS[c])
                    });
                    u8::from(differs)
                })
                .collect();
        }
        Some(Self {
            width,
            height,
            maximum,
            geometry,
            planes,
            dirty,
            backgrounds: BACKGROUNDS,
        })
    }

    /// Image width in samples.
    #[must_use]
    pub fn width(&self) -> usize {
        self.width
    }

    /// Image height in samples.
    #[must_use]
    pub fn height(&self) -> usize {
        self.height
    }

    /// Maximum horizontal and vertical sampling factors of the frame.
    #[must_use]
    pub fn maximum(&self) -> [usize; 2] {
        self.maximum
    }

    /// Horizontal factor, vertical factor and plane pitch of component `c`.
    #[must_use]
    pub fn geometry(&self, c: usize) -> [usize; 3] {
        self.geometry[c]
    }

    /// Whether every component's sampling factors give exact repeats a
    /// direct painter supports: full-resolution luma and equal chroma
    /// subsampled by one, two or four horizontally.
    #[must_use]
    pub fn chroma_repeat(&self) -> Option<usize> {
        let [luma, cb, cr] = self.geometry;
        if luma[..2] != self.maximum || cb[..2] != cr[..2] || cb[0] == 0 {
            return None;
        }
        if !self.maximum[0].is_multiple_of(cb[0]) {
            return None;
        }
        let repeat = self.maximum[0] / cb[0];
        matches!(repeat, 1 | 2 | 4).then_some(repeat)
    }

    /// Whether block `block` of component `c`'s block row `row` differs from
    /// the plane's background.
    #[must_use]
    pub fn dirty(&self, c: usize, row: usize, block: usize) -> bool {
        let stride = self.geometry[c][2] / 8;
        self.dirty[c]
            .get(row * stride + block)
            .is_some_and(|&flag| flag != 0)
    }

    pub(crate) fn storage_bytes(&self) -> usize {
        self.planes
            .iter()
            .chain(&self.dirty)
            .fold(size_of::<Self>(), |n, v| n.saturating_add(v.capacity()))
    }

    /// Component `c`'s samples for image row `y`, before horizontal
    /// replication, including block padding.
    #[must_use]
    pub fn row(&self, c: usize, y: usize) -> &[u8] {
        let [_, v, pitch] = self.geometry[c];
        let y = y * v / self.maximum[1];
        &self.planes[c][y * pitch..(y + 1) * pitch]
    }

    /// One interleaved RGB sample converted from the stored components with
    /// Table 13, as the interleaved layout holds it.
    #[must_use]
    pub fn get(&self, index: usize) -> Option<u16> {
        let pixel = index / 3;
        let x = pixel % self.width;
        let y = pixel / self.width;
        if y >= self.height {
            return None;
        }
        let p = std::array::from_fn::<_, 3, _>(|c| {
            self.row(c, y)[x * self.geometry[c][0] / self.maximum[0]]
        });
        let mut out = [0; 3];
        let _ = super::ycbcr(&p[..1], &p[1..2], &p[2..3], 1, &mut out);
        Some(u16::from(out[index % 3]))
    }

    /// Expands and converts source row `y` to interleaved RGB with Table 13
    /// rounding. Returns false when `out` does not hold one row.
    pub fn rgb(&self, y: usize, out: &mut [u8]) -> bool {
        if out.len() != self.width * 3 {
            return false;
        }
        if let Some(repeat) = self.chroma_repeat() {
            return super::ycbcr(
                &self.row(0, y)[..self.width],
                self.row(1, y),
                self.row(2, y),
                repeat,
                out,
            );
        }
        for (x, p) in out.as_chunks_mut::<3>().0.iter_mut().enumerate() {
            for (c, v) in p.iter_mut().enumerate() {
                *v = self
                    .get((y * self.width + x) * 3 + c)
                    .unwrap_or(0)
                    .to_le_bytes()[0];
            }
        }
        true
    }

    /// Whether source rows `a` and `b` have equal components, so they
    /// convert and filter identically.
    #[must_use]
    pub fn same_row(&self, a: usize, b: usize) -> bool {
        (0..3).all(|c| self.row(c, a) == self.row(c, b))
    }

    /// Full-width luma of row `y` when both of its chroma rows are neutral.
    #[must_use]
    pub fn neutral(&self, y: usize) -> Option<&[u8]> {
        if self.geometry[0][0] != self.maximum[0] {
            return None;
        }
        let cb = self.row(1, y);
        let cr = self.row(2, y);
        if super::uniform(cb, 128) && super::uniform(cr, 128) {
            Some(&self.row(0, y)[..self.width])
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Planar;

    #[test]
    fn construction_validates_coverage_and_derives_background_blocks() {
        let mut luma = vec![255; 256];
        luma[16 * 9 + 9] = 7;
        let p = Planar::new(
            (13, 11),
            [2, 2],
            [[2, 2, 16], [1, 1, 8], [1, 1, 8]],
            [luma, vec![128; 64], vec![128; 64]],
        )
        .unwrap();
        assert_eq!(p.dirty, [vec![0, 0, 0, 1], vec![0], vec![0]]);
        assert!(p.same_row(0, 1));
        assert!(!p.same_row(8, 9));
        assert_eq!(p.chroma_repeat(), Some(2));
        assert!(p.dirty(0, 1, 1) && !p.dirty(0, 1, 0));
        assert!(
            Planar::new(
                (17, 11),
                [2, 2],
                [[2, 2, 16], [1, 1, 8], [1, 1, 8]],
                [vec![255; 256], vec![128; 64], vec![128; 64]],
            )
            .is_none()
        );
    }
}
