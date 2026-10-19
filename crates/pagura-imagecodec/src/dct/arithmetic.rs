//! T.81 F.2.4 and G.2 coefficient models over the Annex D QM decoder.
mod qm;
mod table;
use super::scan::Scan;
use pagura_core::{Error, Result};
use qm::{Bin, Qm};

/// B.2.4.3 defaults are established by SOI and replaced by DAC entries.
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
    pub(super) fn define(&mut self, data: &[u8]) -> Result<()> {
        if data.is_empty() || !data.len().is_multiple_of(2) {
            return Err(Error::syntax("invalid JPEG conditioning table length"));
        }
        for &[selector, value] in data.as_chunks::<2>().0 {
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
    dc: [[Bin; 49]; 4],
    ac: [[Bin; 245]; 4],
    context: [usize; 4],
}
impl<'a> Arithmetic<'a> {
    pub(super) fn new(data: &'a [u8], offset: usize, budget: usize) -> Result<Self> {
        Ok(Self {
            coder: Qm::new(data, offset, budget)?,
            dc: [[Bin::default(); 49]; 4],
            ac: [[Bin::default(); 245]; 4],
            context: [0; 4],
        })
    }

    pub(super) fn restart(&mut self, sequence: u8) -> Result<()> {
        self.coder.restart(sequence)?;
        self.dc.fill([Bin::default(); 49]);
        self.ac.fill([Bin::default(); 245]);
        self.context.fill(0);
        Ok(())
    }

    pub(super) fn finish(mut self) -> Result<(usize, usize)> {
        let offset = self.coder.finish()?;
        Ok((offset, self.coder.used()))
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "scan, component and table state have distinct lifetimes"
    )]
    pub(super) fn block(
        &mut self,
        block: &mut [i32; 64],
        scan: &Scan,
        progressive: bool,
        conditioning: &Conditioning,
        component: usize,
        selectors: (usize, usize),
        predictor: &mut i32,
        precision: u8,
    ) -> Result<()> {
        if scan.start == 0 {
            if scan.high != 0 {
                block[0] |= i32::from(self.coder.fixed()?) << scan.low;
            } else {
                let bins = &mut self.dc[selectors.0];
                let context = &mut self.context[component];
                let difference = if self.coder.decision(&mut bins[*context])? {
                    let negative = self.coder.decision(&mut bins[*context + 1])?;
                    let magnitude = magnitude(
                        &mut self.coder,
                        bins,
                        *context + 2 + usize::from(negative),
                        20,
                        21,
                    )? + 1;
                    if magnitude >= 1 << (precision + 3) {
                        return Err(self.coder.error("invalid JPEG arithmetic DC magnitude"));
                    }
                    let (lower, upper) = conditioning.dc[selectors.0];
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
            if progressive {
                return Ok(());
            }
        }
        let bins = &mut self.ac[selectors.1 - 4];
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
            let extended = if k <= usize::from(conditioning.ac[selectors.1 - 4]) {
                189
            } else {
                217
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

/// F.2.4.3: decode |V|-1; the category and its bits share one bounded tree.
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

/// Reverse G.1.3.3: EOB decisions resume after the previous band's last
/// nonzero; existing values consume correction bits even inside a zero run.
fn refinement(
    coder: &mut Qm<'_>,
    bins: &mut [Bin; 245],
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
