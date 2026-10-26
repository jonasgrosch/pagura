//! Coefficient bit modelling, T.800 D.1–D.7, Tables D.1–D.7.

use pagura_core::Result;

use super::io::{Budget, syntax};
use super::mq::{Context, Decoder};
use super::segments::{self, Segment};

#[derive(Clone, Copy, Default)]
pub(super) struct Sample {
    magnitude: u32,
    flags: u8,
    // Plane + 1 is a visitation epoch; zero means no propagation pass visited it.
    visited_plane: u8,
    missing: u8,
}

impl Sample {
    /// D.2.1, H.1, E-6–E-8: r=1/2, with a coefficient-specific decoded prefix.
    pub fn reconstruct(self, roi: u8, reversible: bool) -> f64 {
        let mut magnitude = self.magnitude;
        let mut missing = self.missing;
        if roi != 0 && magnitude >= (1_u32 << roi) {
            magnitude >>= roi;
            missing = missing.saturating_sub(roi);
        }
        if magnitude == 0 {
            return 0.0;
        }
        let offset = if reversible && missing == 0 {
            0.0
        } else {
            f64::from(1_u32 << missing) * 0.5
        };
        (f64::from(magnitude) + offset)
            * if self.flags & NEGATIVE != 0 {
                -1.0
            } else {
                1.0
            }
    }
}

const NEGATIVE: u8 = 1;
const REFINED: u8 = 2;

// D.3, Figure D.2: row-major neighbours, excluding the centre, and the
// state bit of a sample whose neighbour there is significant.
#[cfg(test)]
const NEIGHBOURS: [(isize, isize, u16); 8] = [
    (-1, -1, 1),
    (0, -1, 2),
    (1, -1, 4),
    (-1, 0, 8),
    (1, 0, 16),
    (-1, 1, 32),
    (0, 1, 64),
    (1, 1, 128),
];

/// State bits of a sample whose orthogonal neighbour is significant and
/// negative, for sign coding (D.3.2): north, west, east and south.
const NORTH_NEGATIVE: u16 = 1 << 8;
const WEST_NEGATIVE: u16 = 1 << 9;
const EAST_NEGATIVE: u16 = 1 << 10;
const SOUTH_NEGATIVE: u16 = 1 << 11;

/// The state bit of a significant sample.
const SIGNIFICANT: u16 = 1 << 12;

/// The bits of a sample's neighbours' significance.
const NEIGHBOURHOOD: u16 = 0xff;

/// The significance and sign bits of a sample's orthogonal neighbours.
const SIGN_STATE: u16 =
    2 | 8 | 16 | 64 | NORTH_NEGATIVE | WEST_NEGATIVE | EAST_NEGATIVE | SOUTH_NEGATIVE;

/// Table D.1: the zero-coding context of each orientation (`2` codes as
/// `0`) and neighbour significance.
const ZERO_CONTEXTS: [[u8; 256]; 4] = {
    let mut table = [[0; 256]; 4];
    let mut orientation = 0;
    while orientation < 4 {
        let mut neighbours = 0;
        while neighbours < 256 {
            table[orientation][neighbours] = zero_context(orientation, neighbours);
            neighbours += 1;
        }
        orientation += 1;
    }
    table
};

const fn zero_context(orientation: usize, neighbours: usize) -> u8 {
    let mut h = (neighbours & 0b0001_1000).count_ones();
    let mut v = (neighbours & 0b0100_0010).count_ones();
    let d = (neighbours & 0b1010_0101).count_ones();
    if orientation == 1 {
        (h, v) = (v, h);
    }
    if orientation == 3 {
        match (d, h + v) {
            (0, 0) => 0,
            (0, 1) => 1,
            (0, _) => 2,
            (1, 0) => 3,
            (1, 1) => 4,
            (1, _) => 5,
            (2, 0) => 6,
            (2, _) => 7,
            _ => 8,
        }
    } else {
        match (h, v, d) {
            (0, 0, 0) => 0,
            (0, 0, 1) => 1,
            (0, 0, _) => 2,
            (0, 1, _) => 3,
            (0, _, _) => 4,
            (1, 0, 0) => 5,
            (1, 0, _) => 6,
            (1, _, _) => 7,
            _ => 8,
        }
    }
}

/// Tables D.2 and D.3: by [`SIGN_STATE`] bits, the sign context and the
/// sign prediction XORed with the decoded bit, as `context << 1 | bit`.
const SIGN_CONTEXTS: [u8; 4096] = {
    let mut table = [0; 4096];
    let mut state = 0;
    while state < 4096 {
        #[expect(clippy::cast_possible_truncation, reason = "states below 4096")]
        let bits = state as u16;
        table[state] = sign_context(bits);
        state += 1;
    }
    table
};

const fn sign_context(state: u16) -> u8 {
    const fn contribution(state: u16, significant: u16, negative: u16) -> i8 {
        if state & significant == 0 {
            0
        } else if state & negative != 0 {
            -1
        } else {
            1
        }
    }
    let h = contribution(state, 8, WEST_NEGATIVE) + contribution(state, 16, EAST_NEGATIVE);
    let v = contribution(state, 2, NORTH_NEGATIVE) + contribution(state, 64, SOUTH_NEGATIVE);
    let (mut h, mut v) = (h.signum(), v.signum());
    let prediction = h < 0 || (h == 0 && v < 0);
    if prediction {
        h = -h;
        v = -v;
    }
    let context: u8 = match (h, v) {
        (0, 0) => 9,
        (0, _) => 10,
        (_, -1) => 11,
        (_, 0) => 12,
        _ => 13,
    };
    context << 1 | prediction as u8
}

/// A coding pass's source of decisions: the arithmetic decoder, or raw
/// bits that ignore the context (bypass, D.6).
trait Bits {
    const RAW: bool;
    fn bit(&mut self, context: usize) -> Result<bool>;
    /// Resets the contexts (D.4.2), where they exist.
    fn reset(&mut self) {}
}

impl Bits for Decoder<'_> {
    const RAW: bool = false;

    #[inline]
    fn bit(&mut self, context: usize) -> Result<bool> {
        Decoder::bit(self, context)
    }

    fn reset(&mut self) {
        Decoder::reset(self);
    }
}

impl Bits for Raw<'_> {
    const RAW: bool = true;

    #[inline]
    fn bit(&mut self, _: usize) -> Result<bool> {
        Raw::bit(self)
    }
}

struct Block {
    samples: Vec<Sample>,
    /// Per sample, with a one-sample border, rows `width + 2` long: its own
    /// significance ([`SIGNIFICANT`]), its neighbours' ([`NEIGHBOURS`]) and
    /// its orthogonal neighbours' signs ([`SIGN_STATE`]). Significance is
    /// monotonic (D.3), so a sample becoming significant updates its eight
    /// neighbours once, rather than each context rescanning its
    /// neighbourhood; the border takes the updates of neighbours outside
    /// the block. A sample without significant neighbours has not been
    /// visited by this plane's propagation pass either, since it would
    /// have had them then; passes skip such samples from the state alone.
    states: Vec<u16>,
    width: usize,
    height: usize,
    orientation: usize,
    causal: bool,
    insignificant: usize,
}

impl Block {
    /// The state index of sample `(x, y)`.
    #[inline]
    fn state(&self, x: usize, y: usize) -> usize {
        (y + 1) * (self.width + 2) + x + 1
    }

    /// Records sample `(x, y)` significant in its neighbours' states. In the
    /// vertically causal mode (D.7) a stripe's last row does not see the
    /// next stripe.
    fn update_neighbours(&mut self, x: usize, y: usize, negative: bool) {
        let stride = self.width + 2;
        let at = self.state(x, y);
        let sign = |bit: u16| if negative { bit } else { 0 };
        let below = at + stride;
        let above = at - stride;
        let mut set = |index: usize, bits: u16| {
            if let Some(state) = self.states.get_mut(index) {
                *state |= bits;
            }
        };
        set(below + 1, 1);
        set(below, 2 | sign(NORTH_NEGATIVE));
        set(below - 1, 4);
        set(at + 1, 8 | sign(WEST_NEGATIVE));
        set(at - 1, 16 | sign(EAST_NEGATIVE));
        if !(self.causal && y.is_multiple_of(4)) {
            set(above + 1, 32);
            set(above, 64 | sign(SOUTH_NEGATIVE));
            set(above - 1, 128);
        }
    }

    /// The state of sample `(x, y)`.
    #[inline]
    fn get(&self, x: usize, y: usize) -> u16 {
        self.states.get(self.state(x, y)).copied().unwrap_or(0)
    }

    /// Whether the four samples of stripe column `x` from row `stripe` are
    /// all insignificant without significant neighbours.
    #[inline]
    fn quiet(&self, x: usize, stripe: usize) -> bool {
        (stripe..stripe + 4).all(|y| self.get(x, y) & (SIGNIFICANT | NEIGHBOURHOOD) == 0)
    }

    /// The zero-coding context of a sample in `state`.
    #[inline]
    fn zero_context(&self, state: u16) -> usize {
        usize::from(ZERO_CONTEXTS[self.orientation][usize::from(state.to_le_bytes()[0])])
    }

    fn sign<B: Bits>(&self, decoder: &mut B, x: usize, y: usize) -> Result<bool> {
        if B::RAW {
            return decoder.bit(0);
        }
        let state = self.states.get(self.state(x, y)).copied().unwrap_or(0);
        let entry = SIGN_CONTEXTS[usize::from(state & SIGN_STATE)];
        Ok(decoder.bit(usize::from(entry >> 1))? ^ (entry & 1 != 0))
    }

    fn make_significant<B: Bits>(
        &mut self,
        decoder: &mut B,
        x: usize,
        y: usize,
        plane: u8,
    ) -> Result<()> {
        let negative = self.sign(decoder, x, y)?;
        let sample = &mut self.samples[y * self.width + x];
        sample.magnitude = 1 << plane;
        sample.flags = u8::from(negative) * NEGATIVE;
        sample.missing = plane;
        self.insignificant -= 1;
        let at = self.state(x, y);
        if let Some(state) = self.states.get_mut(at) {
            *state |= SIGNIFICANT;
        }
        self.update_neighbours(x, y, negative);
        Ok(())
    }

    fn propagation<B: Bits>(&mut self, decoder: &mut B, plane: u8) -> Result<()> {
        if self.insignificant == 0 {
            return Ok(());
        }
        for stripe in (0..self.height).step_by(4) {
            let end = (stripe + 4).min(self.height);
            for x in 0..self.width {
                if end - stripe == 4 && self.quiet(x, stripe) {
                    continue;
                }
                for y in stripe..end {
                    let state = self.get(x, y);
                    if state & SIGNIFICANT != 0 || state & NEIGHBOURHOOD == 0 {
                        continue;
                    }
                    self.samples[y * self.width + x].visited_plane = plane + 1;
                    if decoder.bit(self.zero_context(state))? {
                        self.make_significant(decoder, x, y, plane)?;
                    }
                }
            }
        }
        Ok(())
    }

    fn refinement<B: Bits>(&mut self, decoder: &mut B, plane: u8) -> Result<()> {
        for stripe in (0..self.height).step_by(4) {
            for x in 0..self.width {
                for y in stripe..(stripe + 4).min(self.height) {
                    let state = self.get(x, y);
                    if state & SIGNIFICANT == 0 {
                        continue;
                    }
                    let index = y * self.width + x;
                    let sample = self.samples[index];
                    if sample.visited_plane == plane + 1 {
                        continue;
                    }
                    let context = if sample.flags & REFINED != 0 {
                        16
                    } else {
                        14 + usize::from(state & NEIGHBOURHOOD != 0)
                    };
                    let value = decoder.bit(context)?;
                    let sample = &mut self.samples[index];
                    sample.magnitude |= u32::from(value) << plane;
                    sample.flags |= REFINED;
                    sample.missing = plane;
                }
            }
        }
        Ok(())
    }

    fn cleanup<B: Bits>(&mut self, decoder: &mut B, plane: u8) -> Result<()> {
        if self.insignificant == 0 {
            return Ok(());
        }
        for stripe in (0..self.height).step_by(4) {
            for x in 0..self.width {
                let end = (stripe + 4).min(self.height);
                let run_mode = end - stripe == 4 && self.quiet(x, stripe);
                let mut start = stripe;
                if run_mode {
                    if !decoder.bit(17)? {
                        continue;
                    }
                    let run = 2 * usize::from(decoder.bit(18)?) + usize::from(decoder.bit(18)?);
                    start += run;
                    self.make_significant(decoder, x, start, plane)?;
                    start += 1;
                }
                for y in start..end {
                    let state = self.get(x, y);
                    if state & SIGNIFICANT != 0
                        || state & NEIGHBOURHOOD != 0
                            && self.samples[y * self.width + x].visited_plane == plane + 1
                    {
                        continue;
                    }
                    if decoder.bit(self.zero_context(state))? {
                        self.make_significant(decoder, x, y, plane)?;
                    }
                }
            }
        }
        Ok(())
    }
}

pub(super) struct Parameters {
    pub width: usize,
    pub height: usize,
    pub orientation: u8,
    pub planes: u8,
    pub passes: usize,
    pub modes: u8,
}

struct Raw<'a> {
    data: &'a [u8],
    offset: usize,
    base: usize,
    byte: u8,
    remaining: u8,
    synthetic: bool,
}

impl Raw<'_> {
    fn finish(mut self, predictable: bool) -> Result<()> {
        for (offset, bytes) in self.data.windows(2).enumerate() {
            if bytes[0] == 255 && bytes[1] & 128 != 0 {
                return Err(syntax("invalid JPEG 2000 bypass bit stuffing", offset + 1));
            }
        }
        if !predictable {
            return Ok(());
        }
        // D.6: unused raw bits alternate 0,1; a final FF requires a stuffed byte.
        if self.remaining == 0 && self.byte == 255 {
            self.byte =
                self.data.get(self.offset).copied().ok_or_else(|| {
                    syntax("missing JPEG 2000 bypass termination byte", self.offset)
                })?;
            self.offset += 1;
            self.remaining = 7;
            if self.byte & 128 != 0 {
                return Err(syntax(
                    "invalid JPEG 2000 bypass bit stuffing",
                    self.offset - 1,
                ));
            }
        }
        let mask = (1_u16 << self.remaining) - 1;
        let expected = (0..self.remaining).fold(0_u16, |v, i| (v << 1) | u16::from(i % 2));
        if self.synthetic
            || u16::from(self.byte) & mask != expected
            || self.offset != self.data.len()
        {
            return Err(syntax(
                "invalid JPEG 2000 predictable bypass termination",
                self.offset.saturating_sub(1),
            ));
        }
        Ok(())
    }

    fn bit(&mut self) -> Result<bool> {
        if self.remaining == 0 {
            let stuffed = self.byte == 255;
            let next = self.data.get(self.offset).copied();
            self.synthetic |= next.is_none();
            self.byte = next.unwrap_or(255);
            if stuffed && next.is_some() && self.byte & 128 != 0 {
                return Err(syntax(
                    "invalid JPEG 2000 bypass bit stuffing",
                    self.base + self.offset,
                ));
            }
            self.offset += usize::from(next.is_some());
            self.remaining = if stuffed { 7 } else { 8 };
        }
        self.remaining -= 1;
        Ok(self.byte & (1 << self.remaining) != 0)
    }
}

pub(super) fn decode(
    segments: &[Segment],
    p: &Parameters,
    budget: &mut Budget<'_>,
) -> Result<Vec<Sample>> {
    let base = segments.first().map_or(0, |s| s.base);
    let valid_modes = segments::BYPASS
        | segments::RESET
        | segments::TERMINATE
        | segments::CAUSAL
        | segments::PREDICTABLE
        | segments::SYMBOLS;
    if p.planes == 0
        || p.planes > 30
        || p.passes == 0
        || p.passes > usize::from(p.planes) * 3 - 2
        || p.modes & !valid_modes != 0
    {
        return Err(syntax("invalid JPEG 2000 bit planes", base));
    }
    let count = p.width * p.height;
    let bordered = (p.width + 2) * (p.height + 2);
    let mut block = Block {
        samples: budget.filled(count, Sample::default())?,
        states: budget.filled(bordered, 0)?,
        width: p.width,
        height: p.height,
        orientation: usize::from(p.orientation & 3),
        causal: p.modes & segments::CAUSAL != 0,
        insignificant: count,
    };
    let mut contexts = Decoder::initial_contexts();
    for segment in segments {
        decode_segment(&mut block, segment, p, &mut contexts, budget)?;
    }
    Ok(block.samples)
}

fn decode_segment_inner(
    block: &mut Block,
    segment: &Segment,
    p: &Parameters,
    contexts: &mut [Context; 19],
    budget: &mut Budget<'_>,
) -> Result<()> {
    // B.10.6/D.4: a final layer may end at a nonterminated coding pass.
    let end = segment.first + segment.passes;
    let terminated =
        end == usize::from(p.planes) * 3 - 2 || end == segments::end(segment.first, p.modes);
    let predictable = terminated && p.modes & segments::PREDICTABLE != 0;
    if segments::raw(segment.first, p.modes) {
        let mut decoder = Raw {
            data: &segment.data,
            offset: 0,
            base: 0,
            byte: 0,
            remaining: 0,
            synthetic: false,
        };
        passes(block, segment, p, (&mut decoder, contexts), budget)?;
        decoder.finish(predictable)
    } else {
        let mut decoder = Decoder::new(&segment.data, 0, *contexts, predictable)?;
        passes(block, segment, p, (&mut decoder, contexts), budget)?;
        *contexts = decoder.contexts();
        decoder.finish()
    }
}

/// The segment's coding passes over `block`, decoded by one source.
fn passes<B: Bits>(
    block: &mut Block,
    segment: &Segment,
    p: &Parameters,
    (decoder, contexts): (&mut B, &mut [Context; 19]),
    budget: &mut Budget<'_>,
) -> Result<()> {
    for pass in segment.first..segment.first + segment.passes {
        let plane = p.planes
            - 1
            - u8::try_from(pass.div_ceil(3))
                .map_err(|_| syntax("JPEG 2000 bit plane overflow", 0))?;
        // D.3.1/D.3.4: propagation and cleanup cannot code a significant
        // coefficient. Even empty passes still reset contexts and read symbols.
        if pass % 3 == 2 || block.insignificant != 0 {
            budget.work(block.samples.len())?;
        }
        match pass % 3 {
            1 => block.propagation(decoder, plane)?,
            2 => block.refinement(decoder, plane)?,
            _ => {
                block.cleanup(decoder, plane)?;
                if p.modes & segments::SYMBOLS != 0 {
                    let mut symbol = 0;
                    for _ in 0..4 {
                        symbol = (symbol << 1) | u8::from(decoder.bit(18)?);
                    }
                    if symbol != 10 {
                        return Err(syntax("invalid JPEG 2000 cleanup segmentation symbol", 0));
                    }
                }
            }
        }
        if p.modes & segments::RESET != 0 {
            *contexts = Decoder::initial_contexts();
            decoder.reset();
        }
    }
    Ok(())
}

fn decode_segment(
    block: &mut Block,
    segment: &Segment,
    p: &Parameters,
    contexts: &mut [Context; 19],
    budget: &mut Budget<'_>,
) -> Result<()> {
    decode_segment_inner(block, segment, p, contexts, budget).map_err(|mut error| {
        if let pagura_core::Position::Buffer(offset) = error.position {
            error.position = pagura_core::Position::Buffer(super::io::source_position(
                &segment.spans,
                offset,
                segment.base,
            ));
        }
        error
    })
}

#[cfg(test)]
mod tests {
    use super::{
        Block, EAST_NEGATIVE, NEGATIVE, NEIGHBOURS, NORTH_NEGATIVE, SOUTH_NEGATIVE, Sample,
        WEST_NEGATIVE,
    };

    /// Each sample's state holds exactly its in-block neighbours'
    /// significance, and its orthogonal neighbours' negative signs, as a
    /// scan of the neighbourhood finds them: at block edges and stripe
    /// boundaries (D.3, D.7), including partial stripes.
    #[test]
    fn monotonic_contexts_match_current_neighbours_at_edges_and_stripes() {
        assert_eq!(size_of::<Sample>(), 8);
        for width in [1, 2, 5] {
            for height in [1, 3, 4, 5, 9] {
                for causal in [false, true] {
                    let mut block = Block {
                        samples: vec![Sample::default(); width * height],
                        states: vec![0; (width + 2) * (height + 2)],
                        width,
                        height,
                        orientation: 0,
                        causal,
                        insignificant: width * height,
                    };
                    // Insert in reverse scan order to exercise neighbours on both
                    // sides of stripe boundaries, with alternating signs.
                    for index in (0..width * height).rev() {
                        let negative = index % 3 == 1;
                        block.samples[index].magnitude = 1;
                        block.samples[index].flags = u8::from(negative) * NEGATIVE;
                        block.update_neighbours(index % width, index / width, negative);
                        for y in 0..height {
                            for x in 0..width {
                                let mut expected = 0;
                                for (dx, dy, mask) in NEIGHBOURS {
                                    if causal && y % 4 == 3 && dy == 1 {
                                        continue;
                                    }
                                    let Some(nx) = x.checked_add_signed(dx).filter(|&n| n < width)
                                    else {
                                        continue;
                                    };
                                    let Some(ny) = y.checked_add_signed(dy).filter(|&n| n < height)
                                    else {
                                        continue;
                                    };
                                    let neighbour = block.samples[ny * width + nx];
                                    if neighbour.magnitude == 0 {
                                        continue;
                                    }
                                    expected |= mask;
                                    if neighbour.flags & NEGATIVE != 0 {
                                        expected |= match (dx, dy) {
                                            (0, -1) => NORTH_NEGATIVE,
                                            (-1, 0) => WEST_NEGATIVE,
                                            (1, 0) => EAST_NEGATIVE,
                                            (0, 1) => SOUTH_NEGATIVE,
                                            _ => 0,
                                        };
                                    }
                                }
                                assert_eq!(
                                    block.states[block.state(x, y)],
                                    expected,
                                    "{width}x{height}, causal {causal}, insert {index}, at ({x}, {y})"
                                );
                            }
                        }
                    }
                }
            }
        }
    }
}
