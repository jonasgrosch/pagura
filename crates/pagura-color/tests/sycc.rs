#![forbid(unsafe_code)]
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

#[test]
fn jp2_channel_precision_maps_to_the_same_colour_value_domain() {
    let converter = Converter::new(
        &ColorSpace::Sycc,
        &Target::new(Process::Rgb),
        &Conversion::default(),
    )
    .unwrap();
    // T.800 I.5.3.3 / Table I.10 followed by IEC F.15/F.16. A sixteen-bit
    // half-code is slightly below neutral chroma after the prescribed
    // mapping to 0..255; 32896/65535 is exactly 128/255 instead.
    for (input, expected) in [
        ([32768.0 / 65535.0; 3], [127, 128, 127]),
        (
            [32768.0 / 65535.0, 32896.0 / 65535.0, 32896.0 / 65535.0],
            [128, 128, 128],
        ),
        ([0.0, 1.0 / 7.0, 127.0 / 1023.0], [0, 100, 0]),
        ([1.0, 4.0 / 7.0, 513.0 / 1023.0], [255, 249, 255]),
    ] {
        let mut output = [0.0; 3];
        converter.convert(&input, &mut output);
        assert_eq!(output.map(byte), expected);
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
