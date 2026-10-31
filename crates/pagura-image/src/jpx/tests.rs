//! Independent encoder vectors and hostile-input budgets.

use pagura_core::{ErrorKind, Limits, Position};
use pagura_testkit::fixture;

use super::{JpxColor, decode};

#[test]
fn independent_sample_vectors() -> Result<(), Box<dyn std::error::Error>> {
    let directory = pagura_testkit::workspace_root().join("corpus/generated/jpx");
    let mut checked = 0;
    for entry in std::fs::read_dir(directory)? {
        let path = entry?.path();
        if path
            .extension()
            .is_none_or(|extension| extension != "samples")
        {
            continue;
        }
        let text = std::fs::read_to_string(&path)?;
        let mut fields = text.split_whitespace().map(str::parse::<u32>);
        let width = fields.next().ok_or("missing width")??;
        let height = fields.next().ok_or("missing height")??;
        let components = usize::try_from(fields.next().ok_or("missing components")??)?;
        let bits = u8::try_from(fields.next().ok_or("missing precision")??)?;
        let signed = fields.next().ok_or("missing signedness")?? != 0;
        let tolerance = fields.next().ok_or("missing tolerance")??;
        let data = std::fs::read(path.with_extension("jpx"))?;
        let image =
            decode(&data, &Limits::default()).map_err(|e| format!("{}: {e}", path.display()))?;
        assert_eq!(
            (image.width, image.height, image.components.len()),
            (width, height, components),
            "{}",
            path.display()
        );
        for component in &image.components {
            assert_eq!(
                (
                    component.width,
                    component.height,
                    component.bits,
                    component.signed
                ),
                (width, height, bits, signed),
                "{}",
                path.display()
            );
            for (i, &value) in component.samples.iter().enumerate() {
                let expected = fields.next().ok_or("missing sample")??;
                assert!(
                    u32::from(value).abs_diff(expected) <= tolerance,
                    "{} sample {i}: {value} vs {expected}, tolerance {tolerance}",
                    path.display()
                );
            }
        }
        assert!(fields.next().is_none());
        checked += 1;
    }
    assert!(checked >= 27);
    Ok(())
}

#[test]
fn container_color_palette_and_opacity() -> pagura_core::Result<()> {
    for (name, expected) in [
        ("color-1", JpxColor::Enumerated(17)),
        ("pdf-colour-12", JpxColor::Enumerated(12)),
        ("palette", JpxColor::Enumerated(16)),
        ("channel-alpha", JpxColor::Enumerated(16)),
    ] {
        let image = decode(&fixture("jpx", &format!("{name}.jpx")), &Limits::default())?;
        assert_eq!(image.color, expected);
        assert_eq!(
            image.alpha,
            if name == "channel-alpha" {
                Some(3)
            } else {
                None
            }
        );
    }
    for method in [2, 3] {
        let image = decode(
            &fixture("jpx", &format!("color-{method}.jpx")),
            &Limits::default(),
        )?;
        assert!(
            matches!(image.color,JpxColor::Icc(profile) if profile.len() > 128 && profile.get(16..20) == Some(b"GRAY"))
        );
    }
    Ok(())
}

#[test]
fn malformed_inputs_have_positions() -> Result<(), Box<dyn std::error::Error>> {
    let directory = pagura_testkit::workspace_root().join("corpus/generated/jpx");
    let mut checked = 0;
    for entry in std::fs::read_dir(directory)? {
        let path = entry?.path();
        if path
            .extension()
            .is_none_or(|extension| extension != "error")
        {
            continue;
        }
        let expected = std::fs::read_to_string(&path)?;
        let data = std::fs::read(path.with_extension("jpx"))?;
        let result = decode(&data, &Limits::default());
        assert!(
            matches!(result, Err(ref e) if e.kind.name() == expected.trim() && matches!(e.position, Position::Buffer(_))),
            "{}: {result:?}",
            path.display()
        );
        let position_file = path.with_extension("offset");
        if position_file.exists() {
            let offset = std::fs::read_to_string(position_file)?
                .trim()
                .parse::<usize>()?;
            assert_eq!(
                result.as_ref().err().map(|e| e.position),
                Some(Position::Buffer(offset)),
                "{}",
                path.display()
            );
        }
        checked += 1;
    }
    assert_eq!(checked, 41);
    Ok(())
}

#[test]
fn truncation_and_mutation_never_panic() {
    let container = fixture("jpx", "palette.jpx");
    for length in 0..container.len() {
        assert!(
            decode(&container[..length], &Limits::default()).is_err(),
            "container prefix {length}"
        );
    }
    let data = fixture("jpx", "reversible-2.jpx");
    for length in 0..data.len() {
        assert!(
            decode(&data[..length], &Limits::default()).is_err(),
            "prefix {length}"
        );
    }
    let limits = Limits {
        decode_work: 100_000,
        decode_bytes: 1_000_000,
        decoded_bytes: 1_000_000,
        pixels: 4096,
        ..Limits::default()
    };
    for index in 0..data.len() {
        for value in [0, 127, 255] {
            let mut changed = data.clone();
            changed[index] = value;
            let _ = decode(&changed, &limits);
        }
    }
}

#[test]
fn limits_bound_input_pixels_allocations_and_work() {
    let data = fixture("jpx", "gray.jpx");
    for limits in [
        Limits {
            stream_bytes: data.len() - 1,
            ..Limits::default()
        },
        Limits {
            pixels: 63,
            ..Limits::default()
        },
        Limits {
            decode_bytes: 100,
            ..Limits::default()
        },
        Limits {
            decoded_bytes: 100,
            ..Limits::default()
        },
        Limits {
            decode_work: 10,
            ..Limits::default()
        },
        Limits {
            objects: 0,
            ..Limits::default()
        },
    ] {
        let result = decode(&data, &limits);
        assert!(matches!(result,Err(e) if e.kind == ErrorKind::Limit));
    }
}

#[test]
fn working_memory_is_bounded_by_its_live_bytes_and_samples_apart() {
    let data = fixture("jpx", "derived-three-levels.jpx");
    let decode = |decode_bytes, decoded_bytes| {
        let limits = Limits {
            decode_bytes,
            decoded_bytes,
            ..Limits::default()
        };
        let mut budget = super::io::Budget::new(&limits);
        let result =
            super::decode_bounded(&data, &mut budget, &super::Options::default(), false).map(drop);
        (result, budget.usage())
    };
    let defaults = Limits::default();
    let (result, (peak, charged, samples)) = decode(defaults.decode_bytes, defaults.decoded_bytes);
    result.unwrap();
    // Each resolution's subbands and each code-block's coefficients are
    // freed before the next are allocated.
    assert!(
        charged > peak,
        "{charged} bytes charged, {peak} live at most"
    );
    // The working peak is admitted and a byte less refused, and so are the
    // output samples under their own budget.
    assert_eq!(decode(peak, samples).0, Ok(()));
    for (decode_bytes, decoded_bytes) in [(peak - 1, samples), (peak, samples - 1)] {
        let (refused, _) = decode(decode_bytes, decoded_bytes);
        assert_eq!(refused.unwrap_err().kind, ErrorKind::Limit);
    }
}

#[test]
fn smooth_tile_synthesis_stays_within_work_budget() -> pagura_core::Result<()> {
    let limits = Limits {
        decode_work: 250_000,
        ..Limits::default()
    };
    let image = decode(&fixture("jpx", "wavelet-budget.jpx"), &limits)?;
    assert_eq!(
        (image.width, image.height, image.components.len()),
        (64, 64, 3)
    );
    assert!(
        image
            .components
            .iter()
            .all(|c| c.samples.iter().all(|&v| v == 128))
    );
    Ok(())
}

#[test]
fn reserved_coding_modes_are_rejected() -> Result<(), Box<dyn std::error::Error>> {
    let data = fixture("jpx", "gray.jpx");
    let cod = data
        .windows(2)
        .position(|v| v == [255, 82])
        .ok_or("missing COD")?;
    for mode in [64, 128, 255] {
        let mut changed = data.clone();
        changed[cod + 12] = mode;
        let result = decode(&changed, &Limits::default());
        assert!(matches!(result,Err(e) if e.kind == ErrorKind::Syntax));
    }
    Ok(())
}

#[test]
fn packed_and_progression_boundaries_never_panic() {
    let limits = Limits {
        pixels: 4096,
        decode_bytes: 2_000_000,
        decoded_bytes: 2_000_000,
        decode_work: 200_000,
        objects: 256,
        ..Limits::default()
    };
    for name in [
        "packed-ppm",
        "packed-ppt",
        "poc-overlap",
        "combined-reversible",
    ] {
        let data = fixture("jpx", &format!("{name}.jpx"));
        for length in 0..data.len() {
            assert!(
                decode(&data[..length], &limits).is_err(),
                "{name} prefix {length}"
            );
        }
        for index in 0..data.len() {
            let mut changed = data.clone();
            changed[index] ^= 0xff;
            if let Err(error) = decode(&changed, &limits) {
                assert!(
                    matches!(error.position,Position::Buffer(offset) if offset <= changed.len()),
                    "{name} byte {index}: {error}"
                );
            }
        }
    }
}

#[test]
fn options_preserve_opacity_kind_origin_and_stored_samples() -> pagura_core::Result<()> {
    use super::{OpacityKind, Options, decode_with_options};
    for (name, kind) in [
        ("channel-alpha", OpacityKind::Straight),
        ("channel-premultiplied", OpacityKind::Premultiplied),
    ] {
        let bytes = fixture("jpx", &format!("{name}.jpx"));
        let decoded = decode_with_options(&bytes, &Limits::default(), &Options::default())?;
        let opacity = decoded
            .opacity
            .ok_or(pagura_core::Error::syntax("missing test opacity"))?;
        assert_eq!(opacity.kind, kind);
        assert_eq!(
            (
                opacity.channel,
                opacity.source_channel,
                opacity.source_component
            ),
            (3, 0, 0)
        );
        assert_eq!(opacity.palette_column, None);
        assert_eq!(decoded.image.alpha, Some(3));
        let ignored = decode_with_options(
            &bytes,
            &Limits::default(),
            &Options {
                ignore_color: true,
                ignore_opacity: true,
            },
        )?;
        assert_eq!(ignored.image.color, JpxColor::Unspecified);
        assert_eq!(ignored.opacity, None);
        assert_eq!(ignored.image.alpha, None);
        // Ignore cdef retains source channel zero; interpreting it moves exactly
        // that plane to opacity, without rescaling either it or the colour planes.
        assert_eq!(ignored.image.components[0], decoded.image.components[3]);
        assert_eq!(ignored.image.components[1], decoded.image.components[2]);
        assert_eq!(ignored.image.components[2], decoded.image.components[0]);
        assert_eq!(ignored.image.components[3], decoded.image.components[1]);
    }
    let palette = decode_with_options(
        &fixture("jpx", "palette-opacity-origin.jpx"),
        &Limits::default(),
        &Options::default(),
    )?;
    let alpha = palette
        .opacity
        .ok_or(pagura_core::Error::syntax("missing test palette opacity"))?;
    assert_eq!(
        (
            alpha.channel,
            alpha.source_channel,
            alpha.source_component,
            alpha.palette_column
        ),
        (1, 0, 0, Some(1))
    );
    assert_eq!(palette.component_geometry[0], palette.component_geometry[1]);
    let color_only = decode_with_options(
        &fixture("jpx", "channel-alpha.jpx"),
        &Limits::default(),
        &Options {
            ignore_color: true,
            ignore_opacity: false,
        },
    )?;
    assert_eq!(color_only.image.color, JpxColor::Unspecified);
    assert!(color_only.opacity.is_some());
    let channel_only = decode_with_options(
        &fixture("jpx", "channel-alpha.jpx"),
        &Limits::default(),
        &Options {
            ignore_color: false,
            ignore_opacity: true,
        },
    )?;
    assert_eq!(channel_only.image.color, JpxColor::Enumerated(16));
    assert!(channel_only.opacity.is_none());
    // A PDF colour-space override does not need to interpret an embedded space.
    let decoded = decode_with_options(
        &fixture("jpx", "pdf-colour-19.jpx"),
        &Limits::default(),
        &Options {
            ignore_color: true,
            ..Options::default()
        },
    )?;
    assert_eq!(decoded.image.color, JpxColor::Unspecified);
    Ok(())
}

#[test]
fn subsampled_components_retain_reference_grid_placement() -> Result<(), Box<dyn std::error::Error>>
{
    use super::{ComponentGeometry, Options, decode_with_options};
    let decoded = decode_with_options(
        &fixture("jpx", "component-placement.jpx"),
        &Limits::default(),
        &Options::default(),
    )?;
    let text = String::from_utf8(fixture("jpx", "component-placement.placement"))?;
    let values = text
        .split_whitespace()
        .map(str::parse::<u32>)
        .collect::<Result<Vec<_>, _>>()?;
    assert_eq!(
        [
            decoded.image.width,
            decoded.image.height,
            decoded.reference_origin[0],
            decoded.reference_origin[1]
        ],
        values[..4]
    );
    assert_eq!(decoded.image.components.len(), usize::try_from(values[4])?);
    let mut offset = 5;
    for (component, geometry) in decoded
        .image
        .components
        .iter()
        .zip(decoded.component_geometry)
    {
        assert_eq!(
            geometry,
            ComponentGeometry {
                step: [values[offset], values[offset + 1]],
                origin: [values[offset + 2], values[offset + 3]]
            }
        );
        assert_eq!(
            [component.width, component.height],
            values[offset + 4..offset + 6]
        );
        offset += 6;
        for &sample in &component.samples {
            assert_eq!(u32::from(sample), values[offset]);
            offset += 1;
        }
    }
    assert_eq!(offset, values.len());
    Ok(())
}
