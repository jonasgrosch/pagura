//! Independent IEC 61966-2-1 Amendment 1 F.15–F.17 code-value examples.

use pagura_color::{ColorSpace, Conversion, Converter, Process, Target};

#[test]
fn eight_bit_sycc_preserves_neutrals_and_decodes_chroma() {
    let space = ColorSpace::Sycc;
    assert_eq!(space.components(), 3);
    let converter =
        Converter::new(&space, &Target::new(Process::Rgb), &Conversion::default()).unwrap();
    let mut output = [0.0; 3];
    converter.convert(space.initial().as_slice(), &mut output);
    assert_eq!(output, [0.0; 3]);
    // Expected byte values are independently calculated from the published
    // decimal coefficients, including the 128 (not 127.5) chroma centre.
    for (codes, expected) in [
        ([0_u8, 128, 128], [0_u8, 0, 0]),
        ([64, 128, 128], [64, 64, 64]),
        ([128, 128, 128], [128, 128, 128]),
        ([255, 128, 128], [255, 255, 255]),
        ([76, 85, 255], [254, 0, 0]),
        ([150, 44, 21], [0, 255, 1]),
        ([29, 255, 107], [0, 0, 254]),
        ([0, 0, 0], [0, 135, 0]),
        ([255, 255, 255], [255, 121, 255]),
    ] {
        let input = codes.map(|value| f64::from(value) / 255.0);
        converter.convert(&input, &mut output);
        assert_eq!(
            output.map(|value| (value * 255.0).round()),
            expected.map(f64::from)
        );
        let mut rounded = [0.0; 3];
        converter.convert_rounded(&input, &mut rounded, byte);
        assert_eq!(rounded.map(byte), expected.map(u32::from));
    }
}

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "clipped byte"
)]
fn byte(value: f64) -> u32 {
    (value.clamp(0.0, 1.0) * 255.0).round() as u32
}
