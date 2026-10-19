//! Independent encoder vectors and hostile-input budgets.

use pagura_core::{ErrorKind, Limits, Position};
use pagura_testkit::fixture;

use super::{EnumeratedColorSpace, JpxColor, decode, is_codestream};

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
fn codestreams_are_told_from_jp2_files_by_their_soc_marker() {
    assert!(is_codestream(&fixture("jpx", "gray.jpx")));
    assert!(!is_codestream(&fixture("jpx", "color-1.jpx")));
    assert!(!is_codestream(&[0xff]));
    assert_eq!(EnumeratedColorSpace::new(16), EnumeratedColorSpace::SRGB);
    assert_eq!(EnumeratedColorSpace::new(2).value(), 2);
}

#[test]
fn container_color_palette_and_opacity() -> pagura_core::Result<()> {
    for (name, expected) in [
        (
            "color-1",
            JpxColor::Enumerated(EnumeratedColorSpace::GRAYSCALE),
        ),
        (
            "pdf-colour-12",
            JpxColor::Enumerated(EnumeratedColorSpace::CMYK),
        ),
        ("palette", JpxColor::Enumerated(EnumeratedColorSpace::SRGB)),
        (
            "channel-alpha",
            JpxColor::Enumerated(EnumeratedColorSpace::SRGB),
        ),
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
    assert_eq!(checked, 40);
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
fn limits_bound_pixels_work_memory_and_output() -> pagura_core::Result<()> {
    let data = fixture("jpx", "gray.jpx");
    for (limits, message) in [
        (
            Limits {
                pixels: 63,
                ..Limits::default()
            },
            "JPEG 2000 pixel budget exceeded",
        ),
        (
            Limits {
                decode_work: 10,
                ..Limits::default()
            },
            super::MESSAGES.work,
        ),
        (
            Limits {
                decode_bytes: 100,
                ..Limits::default()
            },
            super::MESSAGES.memory,
        ),
        // The 8×8 gray samples alone take 128 bytes.
        (
            Limits {
                decoded_bytes: 127,
                ..Limits::default()
            },
            super::MESSAGES.output,
        ),
    ] {
        let error = decode(&data, &limits).err();
        assert_eq!(
            error.map(|e| (e.kind, e.message)),
            Some((ErrorKind::Limit, message))
        );
    }
    // Input already in memory, content-stream operations and object counts
    // are other budgets (ADR 0016); none of them bounds a decode.
    let image = decode(
        &data,
        &Limits {
            stream_bytes: 0,
            operations: 0,
            objects: 0,
            ..Limits::default()
        },
    )?;
    assert_eq!(image.components.len(), 1);
    Ok(())
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
        .position(|v| v == super::marker::Marker::COD.bytes())
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
        decode_work: 200_000,
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
    use super::{Association, ChannelType, OpacityKind, Options, decode_with_options};
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
                ..Options::default()
            },
        )?;
        assert_eq!(ignored.image.color, JpxColor::Unspecified);
        assert_eq!(ignored.opacity, None);
        assert_eq!(ignored.image.alpha, None);
        // The definitions are returned whether or not they were interpreted.
        let opacity = if kind == OpacityKind::Straight {
            ChannelType::Opacity
        } else {
            ChannelType::PremultipliedOpacity
        };
        let expected = [
            (0, opacity, Association::Image),
            (1, ChannelType::Color, Association::from(3)),
            (2, ChannelType::Color, Association::from(1)),
            (3, ChannelType::Color, Association::from(2)),
        ];
        for definitions in [&ignored.channel_definitions, &decoded.channel_definitions] {
            let read: Vec<_> = definitions
                .iter()
                .map(|d| (d.channel, d.kind, d.association))
                .collect();
            assert_eq!(read, expected);
        }
        // Ignore cdef retains source channel zero; interpreting it moves exactly
        // that plane to opacity, without rescaling either it or the colour planes.
        assert_eq!(ignored.image.components[0], decoded.image.components[3]);
        assert_eq!(ignored.image.components[1], decoded.image.components[2]);
        assert_eq!(ignored.image.components[2], decoded.image.components[0]);
        assert_eq!(ignored.image.components[3], decoded.image.components[1]);
    }
    Ok(())
}

#[test]
fn options_report_palette_origins_and_ignore_color_and_opacity_separately()
-> pagura_core::Result<()> {
    use super::{Options, decode_with_options};
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
            ..Options::default()
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
            ..Options::default()
        },
    )?;
    assert_eq!(
        channel_only.image.color,
        JpxColor::Enumerated(EnumeratedColorSpace::SRGB)
    );
    assert!(channel_only.opacity.is_none());
    // A PDF color-space override does not need to interpret an embedded space.
    let decoded = decode_with_options(
        &fixture("jpx", "enumerated-ciejab.jpx"),
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

#[test]
fn realistic_sizes_decode_under_default_limits() -> Result<(), Box<dyn std::error::Error>> {
    let data = fixture("jpx", "large-tiles.jpx");
    let text = String::from_utf8(fixture("jpx", "large-tiles.regions"))?;
    let values = text
        .split_whitespace()
        .map(str::parse::<u32>)
        .collect::<Result<Vec<_>, _>>()?;
    let (&[width, height, tile_width, tile_height, tolerance], colors) = values
        .split_first_chunk::<5>()
        .ok_or("short region header")?;
    let image = decode(&data, &Limits::default())?;
    assert_eq!(
        (image.width, image.height, image.components.len()),
        (width, height, 3)
    );
    let row = usize::try_from(width)?;
    let tile = [usize::try_from(tile_width)?, usize::try_from(tile_height)?];
    let columns = row.div_ceil(tile[0]);
    for (channel, component) in image.components.iter().enumerate() {
        assert_eq!(
            (component.width, component.height, component.bits),
            (width, height, 8)
        );
        for (index, &sample) in component.samples.iter().enumerate() {
            let region = index / row / tile[1] * columns + index % row / tile[0];
            let expected = colors.get(region * 3 + channel).ok_or("missing region")?;
            assert!(
                u32::from(sample).abs_diff(*expected) <= tolerance,
                "channel {channel} sample {index}: {sample} vs {expected}"
            );
        }
    }
    // H1: the 50,000,000 units codecs once borrowed from the content-stream
    // `operations` budget do not cover this image's synthesis and transform.
    let error = decode(
        &data,
        &Limits {
            decode_work: 50_000_000,
            ..Limits::default()
        },
    )
    .err();
    assert_eq!(
        error.map(|e| (e.kind, e.message)),
        Some((ErrorKind::Limit, super::MESSAGES.work))
    );
    // Each tile returns its coefficient planes and code-block scratch, so
    // the decoder holds far less than the 77 MB it allocates in total.
    decode(
        &data,
        &Limits {
            decode_bytes: 16 * 1024 * 1024,
            ..Limits::default()
        },
    )?;
    Ok(())
}

#[test]
fn channel_selection_moves_planes_and_their_opacity() -> pagura_core::Result<()> {
    use super::{Options, decode_with_options};
    let bytes = fixture("jpx", "channel-alpha.jpx");
    let decoded = decode_with_options(&bytes, &Limits::default(), &Options::default())?;
    assert_eq!(decoded.image.alpha, Some(3));
    let mut selected = decoded.clone();
    selected.select_channels(&[3, 0, 2])?;
    assert_eq!(selected.image.components.len(), 3);
    assert_eq!(selected.component_geometry.len(), 3);
    for (index, source) in [3, 0, 2].into_iter().enumerate() {
        assert_eq!(
            selected.image.components[index],
            decoded.image.components[source]
        );
    }
    assert_eq!(selected.image.alpha, Some(0));
    assert_eq!(selected.opacity.map(|o| o.channel), Some(0));
    let mut dropped = decoded.clone();
    dropped.select_channels(&[1, 0])?;
    assert_eq!((dropped.image.alpha, dropped.opacity), (None, None));
    for order in [[0, 0].as_slice(), &[4], &[0, 1, 2, 3, 0]] {
        let mut invalid = decoded.clone();
        let error = invalid.select_channels(order).unwrap_err();
        assert_eq!(error.kind, ErrorKind::InvalidArgument, "{order:?}");
    }
    Ok(())
}

#[test]
fn every_enumerated_color_space_is_returned_for_the_caller_to_judge() {
    // T.801 CIEJab (19): PDF excludes it (7.4.9), but that is the PDF
    // layer's rule, not the codec's.
    let image = decode(&fixture("jpx", "enumerated-ciejab.jpx"), &Limits::default()).unwrap();
    assert_eq!(
        image.color,
        JpxColor::Enumerated(EnumeratedColorSpace::CIEJAB)
    );
}

/// Decodes `data` with repairs and returns the result with the one repair
/// reported; checks that strict decoding refuses that repair.
fn repaired(
    data: &[u8],
    repair: pagura_core::Repair,
) -> Result<(super::Decoded, Position), Box<dyn std::error::Error>> {
    let options = super::Options::default();
    let error = super::decode_with_options(data, &Limits::default(), &options)
        .err()
        .ok_or("strict decoding accepted the repair")?;
    assert_eq!(
        (error.kind, error.repair),
        (ErrorKind::Syntax, Some(repair))
    );
    let mut diagnostics = pagura_core::Diagnostics::new(pagura_core::Recovery::Repair);
    let decoded =
        super::decode_with_diagnostics(data, &Limits::default(), &options, &mut diagnostics)?;
    let [report] = diagnostics.as_slice() else {
        return Err(format!("expected one repair: {diagnostics:?}").into());
    };
    assert_eq!(report.repair(), Some(repair));
    Ok((decoded, report.position))
}

#[test]
fn inconsistent_tile_part_counts_are_a_reported_repair() -> Result<(), Box<dyn std::error::Error>> {
    // Both tile-parts declare one part (T.800 A.4.2). Their order and
    // lengths still delimit them, as an unknown count (zero) would.
    let data = fixture("jpx", "tile-part-count.jpx");
    let mut unknown = data.clone();
    let mut sot = unknown
        .windows(2)
        .position(|w| w == [0xff, 0x90])
        .ok_or("no SOT")?;
    let first = sot;
    while unknown.get(sot..sot + 2) == Some(&[0xff, 0x90]) {
        unknown[sot + 11] = 0;
        let length = u32::from_be_bytes(unknown[sot + 6..sot + 10].try_into()?);
        sot += usize::try_from(length)?;
    }
    let expected =
        super::decode_with_options(&unknown, &Limits::default(), &super::Options::default())?;
    let (decoded, position) = repaired(&data, crate::repairs::JPX_TILE_PART_COUNT)?;
    assert_eq!(decoded, expected);
    // Reported once, at the first tile-part that contradicts its count.
    let second =
        first + usize::try_from(u32::from_be_bytes(data[first + 6..first + 10].try_into()?))?;
    assert_eq!(position, Position::Buffer(second));
    Ok(())
}

#[test]
fn a_mapping_outside_the_header_serves_its_palette_as_a_repair()
-> Result<(), Box<dyn std::error::Error>> {
    let data = fixture("jpx", "palette.jpx");
    let expected =
        super::decode_with_options(&data, &Limits::default(), &super::Options::default())?;
    let find = |data: &[u8], kind: &[u8]| -> Result<usize, Box<dyn std::error::Error>> {
        Ok(data
            .windows(4)
            .position(|w| w == kind)
            .ok_or("missing box")?
            - 4)
    };
    let length = |data: &[u8], at: usize| -> Result<usize, Box<dyn std::error::Error>> {
        Ok(usize::try_from(u32::from_be_bytes(
            data[at..at + 4].try_into()?,
        ))?)
    };
    // Take the component mapping box out of the JP2 header, where T.800
    // I.5.3.5 places it, and put it just before the codestream box.
    let at = find(&data, b"cmap")?;
    let mapping = data[at..at + length(&data, at)?].to_vec();
    let mut without = [&data[..at], &data[at + mapping.len()..]].concat();
    let header = find(&without, b"jp2h")?;
    let shorter = u32::try_from(length(&without, header)? - mapping.len())?;
    without[header..header + 4].copy_from_slice(&shorter.to_be_bytes());
    let codestream = find(&without, b"jp2c")?;
    let mut moved = without.clone();
    moved.splice(codestream..codestream, mapping.iter().copied());
    let (decoded, position) = repaired(&moved, crate::repairs::JPX_MAPPING_LOCATION)?;
    assert_eq!(decoded, expected);
    assert_eq!(position, Position::Buffer(codestream));
    // After the codestream, a mapping cannot apply to it.
    let late = [without.as_slice(), &mapping].concat();
    let mut diagnostics = pagura_core::Diagnostics::new(pagura_core::Recovery::Repair);
    let error = super::decode_with_diagnostics(
        &late,
        &Limits::default(),
        &super::Options::default(),
        &mut diagnostics,
    )
    .err()
    .ok_or("late mapping accepted")?;
    assert_eq!(
        (error.message, error.position),
        (
            "misplaced or repeated JP2 component mapping",
            Position::Buffer(without.len())
        )
    );
    Ok(())
}

#[test]
fn ignoring_the_palette_returns_the_stored_indices() -> Result<(), Box<dyn std::error::Error>> {
    let data = fixture("jpx", "palette.jpx");
    let expanded =
        super::decode_with_options(&data, &Limits::default(), &super::Options::default())?;
    let options = super::Options {
        ignore_palette: true,
        ..super::Options::default()
    };
    let indices = super::decode_with_options(&data, &Limits::default(), &options)?;
    // The raw codestream holds the indices; the container's palette is not
    // applied (as for a PDF Indexed space, 7.4.9).
    let codestream = start_of_codestream(&data)?;
    let raw = decode(&data[codestream..], &Limits::default())?;
    assert_eq!(indices.image.components, raw.components);
    assert_ne!(indices.image.components, expanded.image.components);
    assert_eq!(indices.channel_definitions, Vec::new());
    Ok(())
}

/// The contents of the first codestream box of a JP2 file.
fn start_of_codestream(data: &[u8]) -> Result<usize, Box<dyn std::error::Error>> {
    Ok(data
        .windows(4)
        .position(|w| w == b"jp2c")
        .ok_or("no codestream")?
        + 4)
}
