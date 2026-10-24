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
    /// One bit per luma block, `clean_words` words per block row: set
    /// where the block and every chroma block under it hold only their
    /// planes' backgrounds, so its samples convert to one colour.
    pub(crate) clean: Vec<u64>,
    pub(crate) clean_words: usize,
    /// The RGB codes the backgrounds convert to.
    pub(crate) background_rgb: [u8; 3],
}

/// Samples per block side.
const SIDE: usize = 8;

/// The shortest run of background blocks taken apart from its neighbours.
const GAP: usize = 64;

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
        let planar = Self {
            width,
            height,
            maximum,
            geometry,
            planes,
            dirty,
            backgrounds: BACKGROUNDS,
            clean: Vec::new(),
            clean_words: 0,
            background_rgb: [0; 3],
        };
        Some(planar.with_clean())
    }

    /// The planes with their background blocks marked: luma blocks that,
    /// with the chroma blocks under them, hold only their planes'
    /// backgrounds.
    pub(crate) fn with_clean(mut self) -> Self {
        self.find_clean();
        let [y, cb, cr] = self.backgrounds;
        let _ = super::ycbcr(&[y], &[cb], &[cr], 1, &mut self.background_rgb);
        self
    }

    fn find_clean(&mut self) {
        let [_, _, pitch] = self.geometry[0];
        let [max_h, max_v] = self.maximum;
        let columns = pitch / SIDE;
        let rows = self.height.div_ceil(SIDE);
        let words = columns.div_ceil(64);
        let mut bits = vec![0u64; words * rows];
        // The chroma blocks under luma samples `a..=b` of a component with
        // sampling factor `factor` of `maximum`.
        let under = |(a, b): (usize, usize), factor: usize, maximum: usize| {
            (a * factor / maximum / SIDE)..=(b * factor / maximum / SIDE)
        };
        // Per chroma component: each luma column's chroma columns, and per
        // luma row the chroma columns any chroma row under it marks.
        let chroma: [(Vec<std::ops::RangeInclusive<usize>>, usize); 2] = [1, 2].map(|c| {
            let [h, _, chroma_pitch] = self.geometry[c];
            let ranges = (0..columns)
                .map(|column| under((column * SIDE, column * SIDE + SIDE - 1), h, max_h))
                .collect();
            (ranges, chroma_pitch / SIDE)
        });
        let mut marked: Vec<bool> = Vec::new();
        for (row, line) in bits.chunks_exact_mut(words.max(1)).enumerate() {
            let samples = (row * SIDE, row * SIDE + SIDE - 1);
            for column in 0..columns {
                if !self.dirty(0, row, column) {
                    line[column / 64] |= 1 << (column % 64);
                }
            }
            for (c, (ranges, chroma_columns)) in (1..3).zip(&chroma) {
                let [_, v, _] = self.geometry[c];
                marked.clear();
                marked.resize(*chroma_columns, false);
                for y in under(samples, v, max_v) {
                    for (x, mark) in marked.iter_mut().enumerate() {
                        *mark |= self.dirty(c, y, x);
                    }
                }
                if !marked.contains(&true) {
                    continue;
                }
                for (column, range) in ranges.iter().enumerate() {
                    let last = (*range.end()).min(marked.len().saturating_sub(1));
                    if marked
                        .get(*range.start()..=last)
                        .is_some_and(|marks| marks.contains(&true))
                    {
                        line[column / 64] &= !(1 << (column % 64));
                    }
                }
            }
        }
        self.clean = bits;
        self.clean_words = words;
    }

    /// Hands `run` the runs of samples `first..end` of rows `top..=bottom`
    /// that are background in every one of them (`true`) or may not be,
    /// background shorter than 64 samples joining the runs around it.
    pub fn background_runs(
        &self,
        (top, bottom): (usize, usize),
        (first, end): (usize, usize),
        mut run: impl FnMut(usize, usize, bool),
    ) {
        let mut pending: Option<(usize, usize, bool)> = None;
        let mut push = |start: usize, stop: usize, background: bool| {
            let background = background && (stop - start >= GAP || start == first && stop == end);
            match &mut pending {
                Some(last) if last.2 == background => last.1 = stop,
                _ => {
                    if let Some((a, b, c)) = pending.replace((start, stop, background)) {
                        run(a, b, c);
                    }
                }
            }
        };
        let (mut column, last) = (first / SIDE, (end - 1) / SIDE);
        while column <= last {
            // The background bits of this word's columns from `column` on,
            // over every block row.
            let (word, shift) = (column / 64, column % 64);
            let mut bits = u64::MAX;
            for row in top / SIDE..=bottom / SIDE {
                bits &= self
                    .clean
                    .get(row * self.clean_words + word)
                    .copied()
                    .unwrap_or(0);
            }
            let bits = bits >> shift;
            let background = bits & 1 == 1;
            let same = if background {
                bits.trailing_ones()
            } else {
                bits.trailing_zeros()
            };
            let available = u32::try_from(64 - shift).unwrap_or(64);
            let count = usize::try_from(same.min(available))
                .unwrap_or(1)
                .max(1)
                .min(last + 1 - column);
            let start = (column * SIDE).max(first);
            column += count;
            push(start, (column * SIDE).min(end), background);
        }
        if let Some((a, b, c)) = pending {
            run(a, b, c);
        }
    }

    /// The RGB codes the planes' backgrounds convert to.
    #[must_use]
    pub fn background_rgb(&self) -> &[u8; 3] {
        &self.background_rgb
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

    /// The horizontal chroma repeat when every component's sampling factors
    /// give exact repeats: full-resolution luma and equal chroma subsampled
    /// by one, two or four horizontally.
    fn chroma_repeat(&self) -> Option<usize> {
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

    /// The value each component's plane holds where no block was stored.
    #[must_use]
    pub fn backgrounds(&self) -> [u8; 3] {
        self.backgrounds
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
            .saturating_add(self.clean.capacity() * size_of::<u64>())
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

    /// Converts samples `x0..x1` of source row `y` to interleaved RGB as
    /// [`Self::rgb`] does for the whole row; runs of background blocks take
    /// their one colour without conversion. Returns false when the range is
    /// empty or outside the row, or `out` does not hold it.
    pub fn rgb_range(&self, y: usize, x0: usize, x1: usize, out: &mut [u8]) -> bool {
        if x0 >= x1 || x1 > self.width || y >= self.height || out.len() != (x1 - x0) * 3 {
            return false;
        }
        let background = self.background_rgb;
        let mut converted = true;
        self.background_runs((y, y), (x0, x1), |start, stop, clean| {
            let Some(part) = out.get_mut((start - x0) * 3..(stop - x0) * 3) else {
                converted = false;
                return;
            };
            if clean {
                fill_rgb(part, background);
            } else {
                converted &= self.convert(y, start, stop, part);
            }
        });
        converted
    }

    /// Converts samples `x0..x1` of row `y`, which the caller checked.
    fn convert(&self, y: usize, x0: usize, x1: usize, out: &mut [u8]) -> bool {
        let Some(repeat) = self.chroma_repeat() else {
            for (x, p) in (x0..).zip(out.as_chunks_mut::<3>().0) {
                for (c, v) in p.iter_mut().enumerate() {
                    *v = self
                        .get((y * self.width + x) * 3 + c)
                        .unwrap_or(0)
                        .to_le_bytes()[0];
                }
            }
            return true;
        };
        // Chroma runs start at multiples of `repeat`; convert from the run
        // holding `x0` and drop its leading samples.
        let lead = x0 % repeat;
        let start = x0 - lead;
        let (luma, cb, cr) = (self.row(0, y), self.row(1, y), self.row(2, y));
        let (Some(luma), Some(cb), Some(cr)) = (
            luma.get(start..x1),
            cb.get(start / repeat..),
            cr.get(start / repeat..),
        ) else {
            return false;
        };
        if lead == 0 {
            return super::ycbcr(luma, cb, cr, repeat, out);
        }
        let mut first = [0u8; 12];
        let head = (repeat).min(x1 - start);
        let Some(first) = first.get_mut(..head * 3) else {
            return false;
        };
        if !super::ycbcr(&luma[..head], cb, cr, repeat, first) {
            return false;
        }
        let taken = (head - lead) * 3;
        out[..taken].copy_from_slice(&first[lead * 3..]);
        if start + head == x1 {
            return true;
        }
        super::ycbcr(
            &luma[head..],
            &cb[head / repeat..],
            &cr[head / repeat..],
            repeat,
            &mut out[taken..],
        )
    }
}

/// Fills `out` with `rgb` repeated, sixteen samples at a copy.
fn fill_rgb(out: &mut [u8], rgb: [u8; 3]) {
    let mut pattern = [0u8; 48];
    for chunk in pattern.as_chunks_mut::<3>().0 {
        *chunk = rgb;
    }
    let (chunks, rest) = out.as_chunks_mut::<48>();
    for chunk in chunks {
        *chunk = pattern;
    }
    for (out, &value) in rest.iter_mut().zip(&pattern) {
        *out = value;
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

    /// Background blocks are those whose luma block and every chroma block
    /// under it hold the background, for every sampling of the chroma, and
    /// their rows convert as every other row does.
    #[test]
    fn background_blocks_have_clean_chroma_under_them() {
        let mut state = 0x1234_5678u32;
        let mut random = move || {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            state >> 16
        };
        let cases: [([usize; 2], (usize, usize)); 4] = [
            ([2, 2], (37, 29)),
            ([1, 1], (24, 16)),
            ([2, 1], (40, 9)),
            ([4, 1], (70, 15)),
        ];
        for ([h, v], (width, height)) in cases {
            let maximum = [h, v];
            let geometry = [
                [h, v, (width.div_ceil(8 * h) * 8 * h).next_multiple_of(8)],
                [1, 1, width.div_ceil(8 * h) * 8],
                [1, 1, width.div_ceil(8 * h) * 8],
            ];
            let rows = [
                height.div_ceil(8 * v) * 8 * v,
                height.div_ceil(8 * v) * 8,
                height.div_ceil(8 * v) * 8,
            ];
            let backgrounds = [255u8, 128, 128];
            let planes: [Vec<u8>; 3] = std::array::from_fn(|c| {
                let [_, _, pitch] = geometry[c];
                let mut plane = vec![backgrounds[c]; pitch * rows[c]];
                for block in 0..(pitch / 8) * (rows[c] / 8) {
                    if random() % 5 == 0 {
                        let (row, column) = (block / (pitch / 8), block % (pitch / 8));
                        plane[(row * 8 + 3) * pitch + column * 8 + 2] = 7;
                    }
                }
                plane
            });
            let planar = Planar::new((width, height), maximum, geometry, planes).unwrap();
            let side = 8;
            let under = |(a, b): (usize, usize), factor: usize, maximum: usize| {
                (a * factor / maximum / side)..=(b * factor / maximum / side)
            };
            for row in 0..height.div_ceil(side) {
                for column in 0..geometry[0][2] / side {
                    let samples = (row * side, row * side + side - 1);
                    let across = (column * side, column * side + side - 1);
                    let expected = !planar.dirty(0, row, column)
                        && (1..3).all(|c| {
                            let [h, v, _] = planar.geometry(c);
                            under(samples, v, maximum[1]).all(|y| {
                                under(across, h, maximum[0]).all(|x| !planar.dirty(c, y, x))
                            })
                        });
                    let actual =
                        planar.clean[row * planar.clean_words + column / 64] >> (column % 64) & 1
                            == 1;
                    assert_eq!(actual, expected, "{maximum:?} block {row} {column}");
                }
            }
            // Filled background runs read as converted rows.
            for y in 0..height {
                let mut filled = vec![0u8; width * 3];
                assert!(planar.rgb_range(y, 0, width, &mut filled));
                let mut converted = vec![0u8; width * 3];
                assert!(planar.convert(y, 0, width, &mut converted));
                assert_eq!(filled, converted, "{maximum:?} row {y}");
                for (x0, x1) in [(1, width / 2), (width / 3, width - 1), (5, 6)] {
                    let mut part = vec![0u8; (x1 - x0) * 3];
                    assert!(planar.rgb_range(y, x0, x1, &mut part));
                    assert_eq!(part, converted[x0 * 3..x1 * 3], "row {y} {x0}..{x1}");
                }
            }
        }
    }
}
