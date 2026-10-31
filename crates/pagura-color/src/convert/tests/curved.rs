//! The per-component first stage of a conversion ([`Converter::curve`])
//! and the conversion from it ([`Converter::convert_curved`]) give the
//! whole conversion's values bit for bit, and so does the conversion from it
//! resolved once ([`Converter::curved_rgb`]).

use super::*;
use crate::function::tests::random::Rng;
use crate::space::IccSpace;
use pagura_core::{Limits, Name};

const D50: [f64; 3] = [0.9642, 1.0, 0.8249];
const MATRIX: [f64; 9] = [
    0.436_074_7,
    0.222_504_5,
    0.013_932_2,
    0.385_064_9,
    0.716_878_6,
    0.097_104_5,
    0.143_080_4,
    0.060_616_9,
    0.714_173_3,
];

/// `s15Fixed16` of `value`.
#[expect(clippy::cast_possible_truncation, reason = "test values are small")]
fn fixed(value: f64) -> [u8; 4] {
    ((value * 65536.0).round() as i32).to_be_bytes()
}

fn xyz(values: [f64; 3]) -> Vec<u8> {
    let mut tag = b"XYZ \0\0\0\0".to_vec();
    for value in values {
        tag.extend(fixed(value));
    }
    tag
}

/// A `curv` gamma, `curv` table or `para` curve of `kind` 0 to 4.
fn curve(kind: i32, parameters: &[f64]) -> Vec<u8> {
    let mut tag = Vec::new();
    match kind {
        -1 => {
            tag.extend(b"curv\0\0\0\0\0\0\0\x01");
            #[expect(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "a small positive gamma"
            )]
            tag.extend(((parameters[0] * 256.0).round() as u16).to_be_bytes());
        }
        -2 => {
            tag.extend(b"curv\0\0\0\0");
            tag.extend(u32::try_from(parameters.len()).unwrap().to_be_bytes());
            for &value in parameters {
                #[expect(
                    clippy::cast_possible_truncation,
                    clippy::cast_sign_loss,
                    reason = "values in 0..=1"
                )]
                tag.extend(((value * 65535.0).round() as u16).to_be_bytes());
            }
        }
        _ => {
            tag.extend(b"para\0\0\0\0");
            tag.extend(u16::try_from(kind).unwrap().to_be_bytes());
            tag.extend([0, 0]);
            for &value in parameters {
                tag.extend(fixed(value));
            }
        }
    }
    tag
}

/// An ICC v4 display profile of `count` (1 or 3) components with `trc`
/// for every curve, after ICC.1:2010 7.2–7.4 as the fixture generator
/// writes it.
fn profile(count: usize, trc: &[u8], white: [f64; 3]) -> Arc<Profile> {
    let mluc = |text: &str| {
        let data: Vec<u8> = text.encode_utf16().flat_map(u16::to_be_bytes).collect();
        let mut tag = b"mluc\0\0\0\0".to_vec();
        tag.extend(1_u32.to_be_bytes());
        tag.extend(12_u32.to_be_bytes());
        tag.extend(b"enUS");
        tag.extend(u32::try_from(data.len()).unwrap().to_be_bytes());
        tag.extend(28_u32.to_be_bytes());
        tag.extend(data);
        tag
    };
    let mut tags: Vec<([u8; 4], Vec<u8>)> = vec![
        (*b"wtpt", xyz(white)),
        (*b"cprt", mluc("Public domain test data")),
        (*b"desc", mluc("Independent profile")),
    ];
    if count == 3 {
        for (i, prefix) in b"rgb".iter().copied().enumerate() {
            let column = [MATRIX[i * 3], MATRIX[i * 3 + 1], MATRIX[i * 3 + 2]];
            tags.push(([prefix, b'X', b'Y', b'Z'], xyz(column)));
            tags.push(([prefix, b'T', b'R', b'C'], trc.to_vec()));
        }
    } else {
        tags.push((*b"kTRC", trc.to_vec()));
    }
    tags.sort();
    let mut header = vec![0_u8; 128];
    header[8..12].copy_from_slice(&[4, 0x30, 0, 0]);
    // A display's media white is D50; an input device's may be any.
    header[12..16].copy_from_slice(if white == D50 { b"mntr" } else { b"scnr" });
    header[16..20].copy_from_slice(if count == 3 { b"RGB " } else { b"GRAY" });
    header[20..24].copy_from_slice(b"XYZ ");
    for (i, part) in [2026_u16, 10, 17, 12, 0, 0].into_iter().enumerate() {
        header[24 + 2 * i..26 + 2 * i].copy_from_slice(&part.to_be_bytes());
    }
    header[36..40].copy_from_slice(b"acsp");
    for (i, value) in D50.into_iter().enumerate() {
        header[68 + 4 * i..72 + 4 * i].copy_from_slice(&fixed(value));
    }
    let start = 132 + 12 * tags.len();
    let (mut entries, mut payload) = (Vec::new(), Vec::new());
    for (signature, data) in &tags {
        let offset = u32::try_from(start + payload.len()).unwrap();
        entries.extend(signature);
        entries.extend(offset.to_be_bytes());
        entries.extend(u32::try_from(data.len()).unwrap().to_be_bytes());
        payload.extend(data);
        payload.resize(payload.len().next_multiple_of(4), 0);
    }
    let mut bytes = header;
    bytes.extend(u32::try_from(tags.len()).unwrap().to_be_bytes());
    bytes.extend(entries);
    bytes.extend(payload);
    let length = u32::try_from(bytes.len()).unwrap().to_be_bytes();
    bytes[0..4].copy_from_slice(&length);
    Arc::new(
        Profile::parse(&bytes, count, &Limits::default())
            .unwrap()
            .unwrap(),
    )
}

fn icc(profile: Arc<Profile>, count: usize) -> ColorSpace {
    let alternate = if count == 3 {
        ColorSpace::DeviceRgb
    } else {
        ColorSpace::DeviceGray
    };
    ColorSpace::Icc(Arc::new(IccSpace {
        profile: Some(profile),
        alternate: Arc::new(alternate),
        components: u8::try_from(count).unwrap(),
        range: [[0.0, 1.0]; 4],
        object: None,
    }))
}

/// The F32 level bits of `v`, as the paint crate keeps a level.
#[expect(clippy::cast_possible_truncation, reason = "the rounding under test")]
fn level(v: f64) -> u32 {
    let value = v.clamp(0.0, 1.0) * 255.0;
    let nearest = value as f32;
    let truncated = nearest.to_bits() - u32::from(f64::from(nearest) > value);
    truncated | u32::from(f64::from(nearest) != value)
}

/// The N16 word of `v`, as the paint crate keeps a word.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the rounding under test"
)]
fn word(v: f64) -> u32 {
    (v * 65535.0).round().clamp(0.0, 65535.0) as u32
}

#[test]
fn curved_conversions_are_whole_conversions() {
    let curves = [
        curve(-1, &[2.2]),
        curve(
            -2,
            &(0..17)
                .map(|i| (f64::from(i) / 16.0).powf(2.2))
                .collect::<Vec<_>>(),
        ),
        curve(0, &[2.2]),
        curve(1, &[2.0, 1.0, -0.1]),
        curve(2, &[2.0, 1.0, -0.1, 0.05]),
        curve(3, &[2.0, 1.0, 0.0, 0.2, 0.2]),
        curve(
            4,
            &[
                2.4,
                1.0 / 1.055,
                0.055 / 1.055,
                1.0 / 12.92,
                0.04045,
                0.0,
                0.0,
            ],
        ),
    ];
    let mut rng = Rng::new(0xc0de_0c4e);
    let mut checked = 0;
    for trc in &curves {
        for count in [1, 3] {
            let space = icc(profile(count, trc, D50), count);
            for process in [Process::Rgb, Process::Gray, Process::Cmyk] {
                let target = Target::new(process);
                let converter = Converter::new(&space, &target, &Conversion::default()).unwrap();
                let channels = target.channels();
                for i in 0..2000 {
                    // Decoded eight-bit codes, then values anywhere.
                    let values: Vec<f64> = (0..count)
                        .map(|_| {
                            if i < 1000 {
                                f64::from(u8::try_from(rng.below(256)).unwrap()) / 255.0
                            } else {
                                rng.range(-0.25, 1.25)
                            }
                        })
                        .collect();
                    let curved: Vec<f64> = values
                        .iter()
                        .enumerate()
                        .map(|(c, &v)| converter.curve(c, v).unwrap())
                        .collect();
                    let (mut want, mut got) = (vec![0.0; channels], vec![0.0; channels]);
                    converter.convert(&values, &mut want);
                    converter.convert_curved(&curved, &mut got);
                    let bits = |v: &[f64]| v.iter().map(|x| x.to_bits()).collect::<Vec<_>>();
                    assert_eq!(bits(&want), bits(&got), "{values:?} {process:?}");
                    converter.convert_rounded(&values, &mut want, level);
                    converter.convert_curved_rounded(&curved, &mut got, level);
                    assert_eq!(bits(&want), bits(&got), "rounded {values:?} {process:?}");
                    checked += 1;
                }
            }
        }
    }
    assert_eq!(checked, curves.len() * 2 * 3 * 2000);
    // Conversions without such a stage have none.
    let lut = Converter::new(
        &ColorSpace::Icc(Arc::new(IccSpace {
            profile: Some(super::profile("lut16-cmyk-xyz", 4)),
            alternate: Arc::new(ColorSpace::DeviceCmyk),
            components: 4,
            range: [[0.0, 1.0]; 4],
            object: None,
        })),
        &Target::new(Process::Rgb),
        &Conversion::default(),
    )
    .unwrap();
    for converter in [
        Converter::new(
            &ColorSpace::DeviceRgb,
            &Target::new(Process::Rgb),
            &Conversion::default(),
        )
        .unwrap(),
        lut,
    ] {
        assert!(converter.curve(0, 0.5).is_none());
    }
}

#[test]
fn resolved_curved_rgb_conversions_are_curved_conversions() {
    let trc = curve(
        4,
        &[
            2.4,
            1.0 / 1.055,
            0.055 / 1.055,
            1.0 / 12.92,
            0.04045,
            0.0,
            0.0,
        ],
    );
    let rounds: [fn(f64) -> u32; 2] = [level, word];
    let mut rng = Rng::new(0x5eed_0c4e);
    let mut checked = 0;
    for white in [D50, [0.9505, 1.0, 1.089]] {
        for intent in [
            Intent::Perceptual,
            Intent::Relative,
            Intent::Saturation,
            Intent::Absolute,
        ] {
            let space = icc(profile(3, &trc, white), 3);
            let conversion = Conversion {
                intent,
                ..Conversion::default()
            };
            let converter =
                Converter::new(&space, &Target::new(Process::Rgb), &conversion).unwrap();
            let rgb = converter.curved_rgb().unwrap();
            for i in 0..4000 {
                // Curved eight-bit codes, then curved values anywhere.
                let curved: [f64; 3] = std::array::from_fn(|c| {
                    if i < 2000 {
                        let code = f64::from(u8::try_from(rng.below(256)).unwrap());
                        converter.curve(c, code / 255.0).unwrap()
                    } else {
                        rng.range(-0.25, 1.25)
                    }
                });
                for round in rounds {
                    let mut want = [0.0; 3];
                    converter.convert_curved_rounded(&curved, &mut want, round);
                    let got = rgb.convert_rounded(curved, round);
                    assert_eq!(
                        want.map(f64::to_bits),
                        got.map(f64::to_bits),
                        "{curved:?} {intent:?}"
                    );
                    checked += 1;
                }
            }
        }
    }
    assert_eq!(checked, 2 * 4 * 4000 * 2);
    // Gray profiles, other targets and targets with spots resolve none.
    let rgb = icc(profile(3, &trc, D50), 3);
    let spots: Vec<Name> = vec![Name::new(b"Orange")];
    for (space, target) in [
        (icc(profile(1, &trc, D50), 1), Target::new(Process::Rgb)),
        (rgb.clone(), Target::new(Process::Gray)),
        (rgb.clone(), Target::new(Process::Cmyk)),
        (rgb, Target::separated(Process::Rgb, spots.into())),
        (ColorSpace::DeviceRgb, Target::new(Process::Rgb)),
    ] {
        let converter = Converter::new(&space, &target, &Conversion::default()).unwrap();
        assert!(converter.curved_rgb().is_none(), "{target:?}");
    }
}

/// The bounds of a resolved conversion ([`CurvedRgb::encoded_bounds`])
/// hold the value each channel converts to ([`Converter::convert_curved`])
/// for curved eight-bit codes and curved values anywhere, under every
/// intent; they are tight, apart by at most `2^-37` of the value's
/// magnitude before the encoding's offset, and equal where the encoding is
/// linear or the value is 1.
#[test]
fn encoded_bounds_hold_curved_conversions() {
    let trc = curve(
        4,
        &[
            2.4,
            1.0 / 1.055,
            0.055 / 1.055,
            1.0 / 12.92,
            0.04045,
            0.0,
            0.0,
        ],
    );
    let mut rng = Rng::new(0xb0_0d5);
    let (mut checked, mut equal) = (0, 0);
    for intent in [
        Intent::Perceptual,
        Intent::Relative,
        Intent::Saturation,
        Intent::Absolute,
    ] {
        let space = icc(profile(3, &trc, D50), 3);
        let conversion = Conversion {
            intent,
            ..Conversion::default()
        };
        let converter = Converter::new(&space, &Target::new(Process::Rgb), &conversion).unwrap();
        let rgb = converter.curved_rgb().unwrap();
        for i in 0..6000 {
            let curved: [f64; 3] = std::array::from_fn(|c| match i {
                // The ends, then curved codes, then values anywhere.
                0 => 0.0,
                1 => -0.0,
                2 => converter.curve(c, 1.0).unwrap(),
                _ if i < 3000 => {
                    let code = f64::from(u8::try_from(rng.below(256)).unwrap());
                    converter.curve(c, code / 255.0).unwrap()
                }
                _ => rng.range(-0.25, 1.25),
            });
            let mut exact = [0.0; 3];
            converter.convert_curved(&curved, &mut exact);
            for (&value, [low, high]) in exact.iter().zip(rgb.encoded_bounds(curved)) {
                assert!(low <= value && value <= high, "{curved:?} {intent:?}");
                let room = (value.abs() + 0.055) * 2.0_f64.powi(-37);
                assert!(high - low <= room, "{curved:?} {intent:?}");
                equal += usize::from(low == high);
                checked += 1;
            }
        }
    }
    assert_eq!(checked, 4 * 6000 * 3);
    assert!(
        equal > 0 && equal < checked / 2,
        "{equal} of {checked} equal"
    );
}
