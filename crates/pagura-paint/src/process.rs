//! Process colour arithmetic of compositing in additive coordinates (CMYK
//! channels hold `1 − tint`): the conversions group results (11.3.4,
//! 10.3.4), soft-mask luminosity (11.5.3) and output need.

use crate::blend::lum;
use crate::plan::Curve;
use pagura_color::Process;

/// The RGB of the process components `c` (10.4.2): CMYK subtracts black
/// from each additive component.
pub(crate) fn rgb(process: Process, c: &[f64]) -> [f64; 3] {
    let at = |i: usize| c.get(i).copied().unwrap_or(0.0);
    match process {
        Process::Rgb => [at(0), at(1), at(2)],
        Process::Gray => [at(0); 3],
        Process::Cmyk => [0, 1, 2].map(|i| (at(i) + at(3) - 1.0).max(0.0)),
    }
}

/// Device-gray luminosity for soft masks (11.5.3). CMYK weighs the inks
/// before clipping, keeping the black component.
pub(crate) fn luminosity(process: Process, c: &[f64]) -> f64 {
    let at = |i: usize| c.get(i).copied().unwrap_or(0.0);
    match process {
        Process::Gray => at(0),
        Process::Rgb => lum([at(0), at(1), at(2)]),
        Process::Cmyk => (lum([at(0), at(1), at(2)]) + at(3) - 1.0).max(0.0),
    }
}

/// Black generation and undercolour removal (10.3.4); `None` is the
/// default policy, which uses the common ink amount for both.
#[derive(Debug, Clone, Default)]
pub(crate) struct Separation<'a> {
    /// `BG`.
    pub(crate) black: Option<&'a Curve>,
    /// `UCR`.
    pub(crate) removal: Option<&'a Curve>,
}

impl Separation<'_> {
    /// The function evaluations one conversion into CMYK costs.
    pub(crate) fn cost(&self) -> usize {
        self.black
            .map_or(0, Curve::cost)
            .saturating_add(self.removal.map_or(0, Curve::cost))
    }
}

/// Writes `rgb` in `process` to the first components of `out`.
pub(crate) fn encode_rgb(
    process: Process,
    rgb: [f64; 3],
    separation: &Separation<'_>,
    out: &mut [f64],
) {
    let mut put = |i: usize, value: f64| {
        if let Some(slot) = out.get_mut(i) {
            *slot = value;
        }
    };
    match process {
        Process::Rgb => {
            for (i, value) in rgb.into_iter().enumerate() {
                put(i, value);
            }
        }
        Process::Gray => put(0, lum(rgb)),
        Process::Cmyk => {
            let inks = rgb.map(|v| 1.0 - v);
            let amount = inks.into_iter().fold(1.0, f64::min);
            let black = separation
                .black
                .map_or(amount, |f| f.map(amount))
                .clamp(0.0, 1.0);
            let removal = separation
                .removal
                .map_or(amount, |f| f.map(amount))
                .clamp(-1.0, 1.0);
            for (i, ink) in inks.into_iter().enumerate() {
                put(i, 1.0 - (ink - removal).clamp(0.0, 1.0));
            }
            put(3, 1.0 - black);
        }
    }
}

/// The additive values of device components given in `process`: clipped,
/// and complemented for CMYK.
pub(crate) fn native(process: Process, components: &[f64]) -> [f64; 4] {
    let mut values = [0.0; 4];
    for (value, &component) in values.iter_mut().zip(components) {
        let clipped = component.clamp(0.0, 1.0);
        *value = if process == Process::Cmyk {
            1.0 - clipped
        } else {
            clipped
        };
    }
    values
}

/// Black in `process`, additively: no ink but black for CMYK.
pub(crate) fn black(process: Process) -> [f64; 4] {
    if process == Process::Cmyk {
        [1.0, 1.0, 1.0, 0.0]
    } else {
        [0.0; 4]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conversions_follow_clause_10() {
        assert_eq!(rgb(Process::Gray, &[0.25]), [0.25; 3]);
        assert_eq!(
            rgb(Process::Cmyk, &[1.0, 0.5, 0.25, 0.75]),
            [0.75, 0.25, 0.0]
        );
        assert_eq!(luminosity(Process::Rgb, &[1.0, 0.0, 0.0]), 0.3);
        let mut out = [0.0; 4];
        encode_rgb(
            Process::Cmyk,
            [0.25, 0.5, 1.0],
            &Separation::default(),
            &mut out,
        );
        // Inks 0.75, 0.5, 0: the common amount 0 removes nothing.
        assert_eq!(out, [0.25, 0.5, 1.0, 1.0]);
        encode_rgb(
            Process::Cmyk,
            [0.25, 0.5, 0.75],
            &Separation::default(),
            &mut out,
        );
        assert_eq!(out, [1.0 - 0.5, 1.0 - 0.25, 1.0, 1.0 - 0.25]);
        assert_eq!(
            native(Process::Cmyk, &[0.0, 1.0, 2.0, -1.0]),
            [1.0, 0.0, 0.0, 1.0]
        );
        assert_eq!(black(Process::Cmyk), [1.0, 1.0, 1.0, 0.0]);
    }
}
