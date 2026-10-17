//! Decoders stay within their output cap and never panic. The first byte
//! selects the filter and predictor; the rest is the encoded input.
#![no_main]

use libfuzzer_sys::fuzz_target;
use pagura_codec::{Filter, Predictor, PredictorKind};
use pagura_core::{Diagnostics, Recovery};

const CAP: usize = 1 << 20;

fuzz_target!(|data: &[u8]| {
    let Some((&selector, input)) = data.split_first() else {
        return;
    };
    let predictor = Predictor {
        kind: [PredictorKind::None, PredictorKind::Tiff, PredictorKind::Png][usize::from(selector >> 6) % 3],
        colors: 1 + (selector & 3),
        bits_per_component: [1, 2, 4, 8, 16][usize::from((selector >> 2) & 7) % 5],
        columns: 1 + u32::from(selector >> 4),
    };
    let filter = match selector & 7 {
        0 => Filter::AsciiHex,
        1 => Filter::Ascii85,
        2 => Filter::RunLength,
        3 | 4 => Filter::Lzw { predictor, early_change: selector & 1 == 0 },
        _ => Filter::Flate { predictor },
    };
    for recovery in [Recovery::Strict, Recovery::Repair] {
        if let Ok(output) = filter.decode(input, CAP, &mut Diagnostics::new(recovery)) {
            assert!(output.len() <= CAP, "decoder exceeded its output cap");
        }
    }
});
