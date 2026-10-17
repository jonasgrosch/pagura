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
        checked += 1;
    }
    assert_eq!(checked, 9);
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
        operations: 100_000,
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
            decoded_bytes: 100,
            ..Limits::default()
        },
        Limits {
            operations: 10,
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
fn unsupported_modes_are_explicit() -> Result<(), Box<dyn std::error::Error>> {
    let data = fixture("jpx", "gray.jpx");
    let cod = data
        .windows(2)
        .position(|v| v == [255, 82])
        .ok_or("missing COD")?;
    for mode in [1, 4, 8, 16] {
        let mut changed = data.clone();
        changed[cod + 12] = mode;
        let result = decode(&changed, &Limits::default());
        assert!(matches!(result,Err(e) if e.feature() == Some(super::UNSUPPORTED)));
    }
    Ok(())
}
