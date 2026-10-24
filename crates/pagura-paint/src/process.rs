//! Process colour arithmetic of compositing in additive coordinates (CMYK
//! channels hold `1 − tint`): the RGB of process colours that group
//! results (11.3.4) and output need, and soft-mask luminosity (11.5.3).
//! Encoding RGB into a process space, black generation and undercolour
//! removal included, is `pagura_color::Encoder`'s.

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
        Process::Rgb => pagura_color::luminosity([at(0), at(1), at(2)]),
        Process::Cmyk => (pagura_color::luminosity([at(0), at(1), at(2)]) + at(3) - 1.0).max(0.0),
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
        assert_eq!(
            native(Process::Cmyk, &[0.0, 1.0, 2.0, -1.0]),
            [1.0, 0.0, 0.0, 1.0]
        );
        assert_eq!(black(Process::Cmyk), [1.0, 1.0, 1.0, 0.0]);
    }
}
