//! Differential tests of prepared conversion against the previous colour
//! selection (`oracle`) over random colour spaces, targets and conversions.
#![expect(
    clippy::cast_possible_truncation,
    reason = "small random counts and bytes"
)]

mod oracle;

use super::*;
use crate::cie::Calibrated;
use crate::function::tests::oracle::Oracle;
use crate::function::tests::random::{self, Rng};
use crate::icc::Profile;
use crate::space::{Colorant, DeviceN, IccSpace, Indexed, Separation};
use oracle::{Model, Separations, Space, State, context_source};
use pagura_core::{Limits, Name};

/// A colour space and its previous resolution.
struct Pair {
    new: ColorSpace,
    old: Space,
}

const NAMES: [&str; 8] = [
    "Cyan", "Magenta", "Yellow", "Black", "SpotA", "SpotB", "SpotC", "None",
];

fn profile(name: &str, components: usize) -> Arc<Profile> {
    let bytes = pagura_testkit::fixture("color", &format!("{name}.icc"));
    Arc::new(
        Profile::parse(&bytes, components, &Limits::default())
            .unwrap()
            .unwrap(),
    )
}

fn device(rng: &mut Rng) -> Pair {
    match rng.below(3) {
        0 => Pair {
            new: ColorSpace::DeviceGray,
            old: Space::Gray,
        },
        1 => Pair {
            new: ColorSpace::DeviceRgb,
            old: Space::Rgb,
        },
        _ => Pair {
            new: ColorSpace::DeviceCmyk,
            old: Space::Cmyk,
        },
    }
}

fn calibrated(rng: &mut Rng) -> Pair {
    let white = [
        0.9505 + rng.range(-0.05, 0.05),
        1.0,
        1.089 + rng.range(-0.05, 0.05),
    ];
    let space = match rng.below(3) {
        0 => Calibrated::gray(white, [0.0; 3], rng.range(0.5, 2.5)),
        1 => Calibrated::rgb(
            white,
            [0.0; 3],
            [rng.range(0.5, 2.5), 1.8, 2.2],
            [
                [0.4124, 0.3576, 0.1805],
                [0.2126, 0.7152, 0.0722],
                [0.0193, 0.1192, 0.9505],
            ],
        ),
        _ => Calibrated::lab(
            white,
            [0.0; 3],
            [-100.0, 100.0, -rng.range(50.0, 120.0), 90.0],
        ),
    };
    let space = Arc::new(space.unwrap());
    Pair {
        new: ColorSpace::Calibrated(Arc::clone(&space)),
        old: Space::Cie(space),
    }
}

/// A space that may serve as an alternate: device, CIE or ICCBased.
fn alternate(rng: &mut Rng) -> Pair {
    match rng.below(4) {
        0 | 1 => device(rng),
        2 => calibrated(rng),
        _ => icc(rng),
    }
}

fn range(rng: &mut Rng, profiled: bool) -> [[f64; 2]; 4] {
    let mut range = [[0.0, 1.0]; 4];
    for pair in &mut range {
        if rng.chance(40) {
            let (a, b) = if profiled {
                (rng.range(0.0, 1.0), rng.range(0.0, 1.0))
            } else {
                (rng.range(-1.0, 2.0), rng.range(-1.0, 2.0))
            };
            *pair = [a.min(b), a.max(b)];
        }
    }
    range
}

fn icc(rng: &mut Rng) -> Pair {
    let profiled = rng.chance(50);
    let range = range(rng, profiled);
    if rng.chance(50) {
        let (name, components, base) = if rng.chance(50) {
            ("sampled-rgb", 3, Space::Rgb)
        } else {
            ("lut16-cmyk-xyz", 4, Space::Cmyk)
        };
        let profile = profile(name, components);
        let range = range.map(|[a, b]| [a.clamp(0.0, 1.0), b.clamp(0.0, 1.0)]);
        let alternate = Arc::new(if components == 3 {
            ColorSpace::DeviceRgb
        } else {
            ColorSpace::DeviceCmyk
        });
        return Pair {
            new: ColorSpace::Icc(Arc::new(IccSpace {
                profile: Some(Arc::clone(&profile)),
                alternate,
                components: components as u8,
                range,
                object: None,
            })),
            old: Space::Icc {
                base: Arc::new(base),
                profile: Some(profile),
                range,
            },
        };
    }
    let base = if rng.chance(70) {
        device(rng)
    } else {
        calibrated(rng)
    };
    let components = base.new.components();
    Pair {
        new: ColorSpace::Icc(Arc::new(IccSpace {
            profile: None,
            alternate: Arc::new(base.new),
            components: components as u8,
            range,
            object: None,
        })),
        old: Space::Icc {
            base: Arc::new(base.old),
            profile: None,
            range,
        },
    }
}

fn tint_function(rng: &mut Rng, inputs: usize, outputs: usize) -> (Arc<Function>, Arc<Oracle>) {
    let pair = if inputs == 1 && rng.chance(50) {
        random::exponential(rng, outputs)
    } else {
        random::sampled_with(rng, inputs, outputs)
    };
    (pair.new, Arc::new(pair.old))
}

fn separation(rng: &mut Rng) -> Pair {
    let base = alternate(rng);
    let (function, oracle) = tint_function(rng, 1, base.new.components());
    let (colorant, old) = match rng.below(6) {
        0 => (
            Colorant::All,
            Space::Special {
                inputs: 1,
                all: true,
            },
        ),
        1 => (
            Colorant::None,
            Space::Special {
                inputs: 1,
                all: false,
            },
        ),
        _ => {
            let name = Name::new(NAMES[rng.below(7)].as_bytes());
            (
                Colorant::Named(name.clone()),
                Space::Tint {
                    names: Arc::new([name]),
                    base: Arc::new(base.old),
                    function: oracle,
                },
            )
        }
    };
    Pair {
        new: ColorSpace::Separation(Arc::new(Separation {
            colorant,
            alternate: Arc::new(base.new),
            tint: function,
        })),
        old,
    }
}

fn device_n(rng: &mut Rng) -> Pair {
    let count = 1 + rng.below(4);
    let mut names: Vec<&str> = Vec::new();
    while names.len() < count {
        let name = NAMES[rng.below(NAMES.len())];
        if name == "None" || !names.contains(&name) {
            names.push(name);
        }
    }
    let base = alternate(rng);
    let (function, oracle) = tint_function(rng, count, base.new.components());
    let colorants = names
        .iter()
        .map(|&n| {
            if n == "None" {
                Colorant::None
            } else {
                Colorant::Named(Name::new(n.as_bytes()))
            }
        })
        .collect();
    let old = if names.iter().all(|&n| n == "None") {
        Space::Special {
            inputs: count,
            all: false,
        }
    } else {
        Space::Tint {
            names: names.iter().map(|n| Name::new(n.as_bytes())).collect(),
            base: Arc::new(base.old),
            function: oracle,
        }
    };
    Pair {
        new: ColorSpace::DeviceN(Arc::new(DeviceN {
            colorants,
            alternate: Arc::new(base.new),
            tint: function,
        })),
        old,
    }
}

fn space(rng: &mut Rng) -> Pair {
    let base = match rng.below(6) {
        0 => device(rng),
        1 => calibrated(rng),
        2 => icc(rng),
        3 => separation(rng),
        _ => device_n(rng),
    };
    if !rng.chance(25) {
        return base;
    }
    let high = rng.below(12) as u8;
    let count = base.new.components();
    let lookup: Arc<[u8]> = (0..(usize::from(high) + 1) * count)
        .map(|_| rng.next() as u8)
        .collect();
    Pair {
        new: ColorSpace::Indexed(Arc::new(Indexed {
            base: Arc::new(base.new),
            high,
            lookup: Arc::clone(&lookup),
        })),
        old: Space::Indexed {
            base: Arc::new(base.old),
            high,
            lookup,
        },
    }
}

fn components(rng: &mut Rng, space: &ColorSpace) -> Vec<f64> {
    if let ColorSpace::Indexed(indexed) = space {
        return vec![rng.range(-2.0, f64::from(indexed.high) + 2.0)];
    }
    (0..space.components())
        .map(|_| match rng.below(10) {
            0..=4 => rng.range(-0.2, 1.2),
            5 | 6 => [0.0, 1.0][rng.below(2)],
            _ => rng.range(-150.0, 150.0),
        })
        .collect()
}

fn bits(values: &[f64]) -> Vec<u64> {
    values.iter().map(|v| v.to_bits()).collect()
}

/// The previous colour of all channels and its preserved channels, or
/// `None` when the previous selection failed.
fn previous(
    pair: &Pair,
    components: &[f64],
    (target, state): (&Target, State),
    conversion: &oracle::Conversion,
    work: &mut usize,
) -> Option<(Vec<f64>, u64)> {
    let model = match target.process {
        Process::Gray => Model::Gray,
        Process::Rgb => Model::Rgb,
        Process::Cmyk => Model::Cmyk,
    };
    let separations = Separations {
        enabled: target.colorants == Colorants::Separated,
        spots: &target.spots,
    };
    let n = model.components();
    let mut colors = vec![1.0; target.channels()];
    let mut painted = 0_u64;
    let mut batches = vec![None];
    batches.extend((0..target.spots.len().div_ceil(4)).map(Some));
    for batch in batches {
        let pass = context_source(
            &pair.old,
            components,
            model,
            (&separations, state, batch),
            conversion,
            work,
        )
        .ok()?;
        let offset = batch.map_or(0, |b| n + 4 * b);
        let count = if batch.is_some() { 4 } else { n };
        for i in 0..count {
            if offset + i < colors.len() {
                colors[offset + i] = pass.color[i];
                if !pass.preserve[i] {
                    painted |= 1 << (offset + i);
                }
            }
        }
    }
    Some((colors, painted))
}

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "one table of every space, target and conversion parameter, checked together"
)]
fn conversions_match_the_previous_selection_bit_for_bit() {
    let mut rng = Rng::new(0xc0_10_25);
    let (mut compared, mut rejected, mut selective, mut zeroed) = (0, 0, 0, 0);
    for case in 0..4000 {
        let pair = space(&mut rng);
        let process = [Process::Gray, Process::Rgb, Process::Cmyk][rng.below(3)];
        let target = if rng.chance(50) {
            let spots: Vec<Name> = ["SpotA", "SpotB", "SpotX", "SpotC", "SpotY"]
                .iter()
                .take(rng.below(6))
                .map(|n| Name::new(n.as_bytes()))
                .collect();
            Target::separated(process, spots.into())
        } else {
            Target::new(process)
        };
        let mut conversion = Conversion::default();
        let mut old_conversion = oracle::Conversion::default();
        conversion.intent = [
            Intent::Perceptual,
            Intent::Relative,
            Intent::Saturation,
            Intent::Absolute,
        ][rng.below(4)];
        old_conversion.intent = conversion.intent;
        for (new, old) in [
            (&mut conversion.black_generation, &mut old_conversion.black),
            (&mut conversion.undercolor_removal, &mut old_conversion.ucr),
        ] {
            if rng.chance(30) {
                let function = random::exponential(&mut rng, 1);
                *new = Some(function.new);
                *old = Some(Arc::new(function.old));
            }
        }
        let context = format!("case {case}: {:?} into {target:?}", pair.new);
        let converter = match Converter::new(&pair.new, &target, &conversion) {
            Ok(converter) => converter,
            Err(error) => {
                rejected += 1;
                // Only conversions that always failed before are rejected.
                for _ in 0..8 {
                    let c = components(&mut rng, &pair.new);
                    let state = State {
                        overprint: true,
                        nonzero: false,
                        direct: false,
                    };
                    assert!(
                        previous(&pair, &c, (&target, state), &old_conversion, &mut 0).is_none(),
                        "{context}: {error}"
                    );
                }
                continue;
            }
        };
        for _ in 0..16 {
            let c = components(&mut rng, &pair.new);
            let state = State {
                overprint: true,
                nonzero: false,
                direct: false,
            };
            let mut work = 0;
            let Some((expected, painted)) =
                previous(&pair, &c, (&target, state), &old_conversion, &mut work)
            else {
                continue;
            };
            let mut output = vec![f64::NAN; target.channels()];
            let new_painted = converter.convert(&c, &mut output);
            if !pair.old.paints() {
                assert_eq!(new_painted, Painted(0), "{context}");
                continue;
            }
            compared += 1;
            assert_eq!(
                bits(&output),
                bits(&expected),
                "{context} at {c:?}: {output:?} != {expected:?}"
            );
            assert_eq!(new_painted, Painted(painted), "{context} at {c:?}");
            assert!(
                work <= converter.cost(),
                "{context}: work {work} > {}",
                converter.cost()
            );
            let state = State {
                overprint: true,
                nonzero: true,
                direct: true,
            };
            let (_, nonzero) =
                previous(&pair, &c, (&target, state), &old_conversion, &mut 0).unwrap();
            assert_eq!(
                converter.nonzero(&c),
                Painted(nonzero),
                "{context} at {c:?}"
            );
            selective += usize::from(
                new_painted != Painted::all(target.channels())
                    && new_painted != Painted::all(target.process.components()),
            );
            zeroed += usize::from(nonzero != painted);
        }
    }
    // Every kind of selection occurs: direct tints and OPM 1 zero selection.
    assert!(
        compared > 30_000 && rejected < 400 && selective > 1000 && zeroed > 300,
        "{compared} {rejected} {selective} {zeroed}"
    );
}

#[test]
fn patterns_and_wide_targets_are_rejected() {
    let target = Target::new(Process::Rgb);
    let error =
        Converter::new(&ColorSpace::Pattern(None), &target, &Conversion::default()).unwrap_err();
    assert_eq!(error.kind, pagura_core::ErrorKind::InvalidArgument);
    let spots: Vec<Name> = (0..62)
        .map(|i| Name::new(format!("S{i}").as_bytes()))
        .collect();
    let wide = Target::separated(Process::Cmyk, spots.into());
    let error = Converter::new(&ColorSpace::DeviceGray, &wide, &Conversion::default()).unwrap_err();
    assert_eq!(error.kind, pagura_core::ErrorKind::Limit);
}

#[test]
fn metered_converters_charge_owned_tables_before_allocating() {
    let target = Target::new(Process::Rgb);
    let conversion = Conversion::default();
    let native = Converter::new_metered(
        &ColorSpace::DeviceRgb,
        &target,
        &conversion,
        &mut Meter::new(0, "test"),
    )
    .unwrap();
    assert_eq!(native.owned_bytes(), 0);
    let source = ColorSpace::Indexed(Arc::new(Indexed {
        base: Arc::new(ColorSpace::DeviceRgb),
        high: 3,
        lookup: vec![17; 12].into(),
    }));
    let required = 12 * size_of::<f64>();
    let mut bytes = Meter::new(required, "test");
    let converter = Converter::new_metered(&source, &target, &conversion, &mut bytes).unwrap();
    assert_eq!(converter.owned_bytes(), required);
    assert_eq!(bytes.used(), converter.owned_bytes());
    assert!(
        Converter::new_metered(
            &source,
            &target,
            &conversion,
            &mut Meter::new(required - 1, "test"),
        )
        .is_err()
    );
    let mut expected = [0.0; 3];
    Converter::new(&source, &target, &conversion)
        .unwrap()
        .convert(&[2.0], &mut expected);
    let mut actual = [0.0; 3];
    converter.convert(&[2.0], &mut actual);
    assert_eq!(bits(&actual), bits(&expected));
}

#[test]
fn metered_chains_include_live_buffer_overlap() {
    let mut source = ColorSpace::DeviceRgb;
    for _ in 0..3 {
        let components = source.components();
        source = ColorSpace::Indexed(Arc::new(Indexed {
            base: Arc::new(source),
            high: 3,
            lookup: vec![1; 4 * components].into(),
        }));
    }
    let target = Target::new(Process::Rgb);
    let conversion = Conversion::default();
    let mut bytes = Meter::new(usize::MAX, "test");
    let converter = Converter::new_metered(&source, &target, &conversion, &mut bytes).unwrap();
    let retained = converter.owned_bytes();
    assert!(retained > 0);
    assert_eq!(bytes.used(), retained);
    assert!(
        Converter::new_metered(
            &source,
            &target,
            &conversion,
            &mut Meter::new(retained, "test"),
        )
        .is_err()
    );
    let mut bytes = Meter::new(retained * 2, "test");
    let rebuilt = Converter::new_metered(&source, &target, &conversion, &mut bytes).unwrap();
    assert_eq!(bytes.used(), rebuilt.owned_bytes());
}

#[test]
fn painted_sets_name_their_channels() {
    assert_eq!(Painted::all(3), Painted(0b111));
    assert_eq!(Painted::all(64), Painted(u64::MAX));
    assert!(Painted(0b100).contains(2) && !Painted(0b100).contains(1) && !Painted(1).contains(64));
}

#[test]
fn arithmetic_conversions_evaluate_no_function_or_transform() {
    let mut rng = Rng::new(7);
    let (tint, _) = tint_function(&mut rng, 1, 3);
    let (black, _) = tint_function(&mut rng, 1, 1);
    let (rgb, cmyk) = (Target::new(Process::Rgb), Target::new(Process::Cmyk));
    let plain = Conversion::default();
    let generated = Conversion {
        black_generation: Some(black),
        ..Conversion::default()
    };
    let arithmetic = |space: ColorSpace, target: &Target, conversion: &Conversion| {
        Converter::new(&space, target, conversion)
            .unwrap()
            .arithmetic()
    };
    let icc = |profile| {
        ColorSpace::Icc(Arc::new(IccSpace {
            profile,
            alternate: Arc::new(ColorSpace::DeviceRgb),
            components: 3,
            range: [[0.0, 1.0]; 4],
            object: None,
        }))
    };
    let indexed = ColorSpace::Indexed(Arc::new(Indexed {
        base: Arc::new(ColorSpace::DeviceCmyk),
        high: 1,
        lookup: vec![0; 8].into(),
    }));
    for space in [
        ColorSpace::DeviceGray,
        ColorSpace::DeviceRgb,
        indexed,
        icc(None),
    ] {
        assert!(arithmetic(space.clone(), &rgb, &plain));
        assert!(arithmetic(space, &cmyk, &plain));
    }
    assert!(arithmetic(ColorSpace::DeviceCmyk, &cmyk, &generated));
    assert!(!arithmetic(ColorSpace::DeviceRgb, &cmyk, &generated));
    assert!(!arithmetic(
        icc(Some(profile("sampled-rgb", 3))),
        &rgb,
        &plain
    ));
    let lab = Calibrated::lab([0.9505, 1.0, 1.089], [0.0; 3], [-100.0, 100.0, -90.0, 90.0]);
    let lab = ColorSpace::Calibrated(Arc::new(lab.unwrap()));
    assert!(!arithmetic(lab, &rgb, &plain));
    let spot = ColorSpace::Separation(Arc::new(Separation {
        colorant: Colorant::Named(Name::new(b"SpotA")),
        alternate: Arc::new(ColorSpace::DeviceRgb),
        tint,
    }));
    assert!(!arithmetic(spot, &rgb, &plain));
}
