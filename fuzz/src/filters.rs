//! `filters`: every decoder stays within its output cap and returns a
//! structured error for damaged data (clause 7.4). The first byte selects the
//! filter and predictor; the rest is the encoded input.

use pagura_codec::{Filter, Predictor, PredictorKind};
use pagura_core::{Diagnostics, Recovery};

const CAP: usize = 1 << 20;

/// Decodes the input with the selected filter in both recovery modes.
///
/// # Panics
///
/// When the input violates a checked property: that is a finding.
pub fn run(data: &[u8]) {
    let Some((&selector, input)) = data.split_first() else {
        return;
    };
    let predictor = Predictor {
        kind: [PredictorKind::None, PredictorKind::Tiff, PredictorKind::Png]
            [usize::from(selector >> 6) % 3],
        colors: 1 + (selector & 3),
        bits_per_component: [1, 2, 4, 8, 16][usize::from((selector >> 2) & 7) % 5],
        columns: 1 + u32::from(selector >> 4),
    };
    let filter = match selector & 7 {
        0 => Filter::AsciiHex,
        1 => Filter::Ascii85,
        2 => Filter::RunLength,
        3 | 4 => Filter::Lzw {
            predictor,
            early_change: selector & 1 == 0,
        },
        _ => Filter::Flate { predictor },
    };
    for recovery in [Recovery::Strict, Recovery::Repair] {
        let mut diagnostics = Diagnostics::new(recovery);
        if let Some(output) = super::checked(filter.decode(input, CAP, &mut diagnostics)) {
            assert!(output.len() <= CAP, "decoder exceeded its output cap");
        }
        super::assert_diagnostics(diagnostics.as_slice(), recovery);
    }
}
