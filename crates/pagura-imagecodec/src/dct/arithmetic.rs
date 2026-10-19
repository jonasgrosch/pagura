//! T.81 F.2.4 and G.2 coefficient models over the Annex D QM decoder.
mod qm;
mod table;
use super::scan::{Scan, Selected};
use pagura_core::bytes::Cursor;
use pagura_core::{Error, Result};
use qm::{Bin, Qm};

/// The statistics bins of one DC table (T.81 F.1.4.4.1, Figure F.17): 20
/// for the five sign and magnitude contexts and 29 for the magnitude
/// categories and bits.
const DC_BINS: usize = 49;
/// The statistics bins of one AC table (T.81 F.1.4.4.2, Figure F.18): three
/// per coefficient position 1 to 63, then the two magnitude groups.
const AC_BINS: usize = 245;

/// The first DC magnitude-category bins (T.81 Figure F.17): X1 and X2.
const DC_X1: usize = 20;
const DC_X2: usize = 21;
/// The first AC magnitude-category bin for coefficients up to the
/// conditioning bound Kx, and above it (T.81 Figure F.18).
const AC_LOW_X2: usize = 189;
const AC_HIGH_X2: usize = 217;

/// T.81 B.2.4.3 defaults are established by SOI and replaced by DAC entries.
pub(super) struct Conditioning {
    dc: [(u8, u8); 4],
    ac: [u8; 4],
}
impl Default for Conditioning {
    fn default() -> Self {
        Self {
            dc: [(0, 1); 4],
            ac: [5; 4],
        }
    }
}
impl Conditioning {
    /// Reads a DAC segment payload (T.81 B.2.4.3).
    pub(super) fn define(&mut self, mut data: Cursor<'_>) -> Result<()> {
        if data.at_end() || !data.remaining().is_multiple_of(2) {
            return Err(Error::syntax("invalid JPEG conditioning table length"));
        }
        while !data.at_end() {
            let [selector, value] = data.take_array()?;
            let index = usize::from(selector & 15);
            if index > 3 || selector >> 4 > 1 {
                return Err(Error::syntax("invalid JPEG conditioning table selector"));
            }
            if selector >> 4 == 0 {
                let (lower, upper) = (value & 15, value >> 4);
                if lower > upper {
                    return Err(Error::syntax("invalid JPEG DC conditioning bounds"));
                }
                self.dc[index] = (lower, upper);
            } else {
                if !(1..=63).contains(&value) {
                    return Err(Error::syntax("invalid JPEG AC conditioning bound"));
                }
                self.ac[index] = value;
            }
        }
        Ok(())
    }
}

pub(super) struct Arithmetic<'a> {
    coder: Qm<'a>,
    dc: [[Bin; DC_BINS]; 4],
    ac: [[Bin; AC_BINS]; 4],
    context: [usize; 4],
}
impl<'a> Arithmetic<'a> {
    /// Starts decoding the entropy-coded segment at `data`.
    pub(super) fn new(data: Cursor<'a>) -> Result<Self> {
        Ok(Self {
            coder: Qm::new(data)?,
            dc: [[Bin::default(); DC_BINS]; 4],
            ac: [[Bin::default(); AC_BINS]; 4],
            context: [0; 4],
        })
    }

    pub(super) fn restart(&mut self, sequence: u8) -> Result<()> {
        self.coder.restart(sequence)?;
        self.dc.fill([Bin::default(); DC_BINS]);
        self.ac.fill([Bin::default(); AC_BINS]);
        self.context.fill(0);
        Ok(())
    }

    /// Reads to the marker after the scan and returns its offset.
    pub(super) fn finish(mut self) -> Result<usize> {
        self.coder.finish()
    }

    pub(super) fn block(
        &mut self,
        block: &mut [i32; 64],
        scan: &Scan,
        conditioning: &Conditioning,
        selected: Selected,
        predictor: &mut i32,
    ) -> Result<()> {
        let (precision, component) = (scan.precision, selected.component);
        if scan.start == 0 {
            if scan.high != 0 {
                block[0] |= i32::from(self.coder.fixed()?) << scan.low;
            } else {
                let bins = &mut self.dc[selected.dc];
                let context = &mut self.context[component];
                let difference = if self.coder.decision(&mut bins[*context])? {
                    let negative = self.coder.decision(&mut bins[*context + 1])?;
                    let magnitude = magnitude(
                        &mut self.coder,
                        bins,
                        *context + 2 + usize::from(negative),
                        DC_X1,
                        DC_X2,
                    )? + 1;
                    if magnitude >= 1 << (precision + 3) {
                        return Err(self.coder.error("invalid JPEG arithmetic DC magnitude"));
                    }
                    let (lower, upper) = conditioning.dc[selected.dc];
                    let small = if lower == 0 { 0 } else { 1 << (lower - 1) };
                    *context = if magnitude <= small {
                        0
                    } else {
                        4 + 4 * usize::from(negative) + if magnitude > 1 << upper { 8 } else { 0 }
                    };
                    if negative { -magnitude } else { magnitude }
                } else {
                    *context = 0;
                    0
                };
                let bound = 1 << (precision + 3);
                *predictor = predictor
                    .checked_add(difference)
                    .filter(|&v| (-bound..bound).contains(&v))
                    .ok_or(
                        self.coder
                            .error("JPEG arithmetic DC predictor out of range"),
                    )?;
                block[0] = *predictor << scan.low;
            }
            if scan.progressive() {
                return Ok(());
            }
        }
        let bins = &mut self.ac[selected.ac];
        if scan.high != 0 {
            return refinement(&mut self.coder, bins, block, scan);
        }
        let mut k = scan.start.max(1);
        while k <= scan.end {
            if self.coder.decision(&mut bins[3 * (k - 1)])? {
                break;
            }
            while !self.coder.decision(&mut bins[3 * (k - 1) + 1])? {
                k += 1;
                if k > scan.end {
                    return Err(self.coder.error("JPEG arithmetic zero run exceeds band"));
                }
            }
            let negative = self.coder.fixed()?;
            let first = 3 * (k - 1) + 2;
            let extended = if k <= usize::from(conditioning.ac[selected.ac]) {
                AC_LOW_X2
            } else {
                AC_HIGH_X2
            };
            let value = magnitude(&mut self.coder, bins, first, first, extended)? + 1;
            if value >= 1 << (precision + 2) {
                return Err(self.coder.error("invalid JPEG arithmetic AC magnitude"));
            }
            block[k] = (if negative { -value } else { value }) << scan.low;
            k += 1;
        }
        Ok(())
    }
}

/// T.81 F.2.4.3: decode |V|-1; the category and its bits share one bounded tree.
fn magnitude(
    coder: &mut Qm<'_>,
    bins: &mut [Bin],
    first: usize,
    x1: usize,
    x2: usize,
) -> Result<i32> {
    if !coder.decision(&mut bins[first])? {
        return Ok(0);
    }
    let (mut leading, mut index) = (1, x1);
    while coder.decision(&mut bins[index])? {
        leading <<= 1;
        if leading > 16384 {
            return Err(coder.error("JPEG arithmetic magnitude exceeds fifteen bits"));
        }
        index = if leading == 2 { x2 } else { index + 1 };
    }
    let mut value = leading;
    let mut bit = leading >> 1;
    while bit != 0 {
        if coder.decision(&mut bins[index + 14])? {
            value |= bit;
        }
        bit >>= 1;
    }
    Ok(value)
}

/// Reverse of T.81 G.1.3.3: EOB decisions resume after the previous band's last
/// nonzero; existing values consume correction bits even inside a zero run.
fn refinement(
    coder: &mut Qm<'_>,
    bins: &mut [Bin; AC_BINS],
    block: &mut [i32; 64],
    scan: &Scan,
) -> Result<()> {
    let previous_end = (scan.start..=scan.end)
        .rfind(|&k| block[k] != 0)
        .map_or(scan.start, |k| k + 1);
    let bit = 1 << scan.low;
    let mut k = scan.start;
    while k <= scan.end {
        if k >= previous_end && coder.decision(&mut bins[3 * (k - 1)])? {
            break;
        }
        loop {
            let base = 3 * (k - 1);
            if block[k] != 0 {
                if coder.decision(&mut bins[base + 2])? {
                    block[k] += if block[k] > 0 { bit } else { -bit };
                }
                break;
            }
            if coder.decision(&mut bins[base + 1])? {
                block[k] = if coder.fixed()? { -bit } else { bit };
                break;
            }
            k += 1;
            if k > scan.end {
                return Err(coder.error("JPEG arithmetic refinement run exceeds band"));
            }
        }
        k += 1;
    }
    Ok(())
}
