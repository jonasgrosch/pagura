#![forbid(unsafe_code)]
//! Independent literal palette codes, strict repair attribution and resource bounds.

use pagura_core::{Diagnostics, ErrorKind, Limits, ObjectId, Position, Recovery};
use pagura_document::{Document, OpenOptions};
use pagura_image::{jpx, repairs::JPX_PALETTE_PADDING};
use pagura_testkit::{Json, Manifest, fixture};

fn encoded(name: &str) -> pagura_core::Result<Vec<u8>> {
    let document = Document::open(
        fixture("jp2_palette", &format!("{name}.pdf")),
        &OpenOptions::default(),
    )?;
    let image = document.object(ObjectId::new(5, 0))?;
    Ok(document.image_stream(image)?.data.into_owned())
}

#[test]
fn declared_precision_and_signed_sample_codes_are_independent_of_padding()
-> Result<(), Box<dyn std::error::Error>> {
    let manifest = Manifest::load("jp2_palette");
    let mut checked = 0;
    for case in manifest.cases() {
        let Some(planes) = case.get("codec_expected_codes").and_then(Json::as_array) else {
            continue;
        };
        if planes.is_empty() {
            continue;
        }
        let depths = case
            .get("palette_depths")
            .and_then(Json::as_array)
            .ok_or("missing depths")?;
        let data = encoded(case.name())?;
        let original = data.clone();
        let padded = case.name().ends_with("-padding");
        let position = Position::Buffer(match planes.len() {
            1 => 89,
            3 => 91,
            _ => return Err("unexpected literal palette shape".into()),
        });
        for mode in [Recovery::Strict, Recovery::Repair] {
            let mut diagnostics = Diagnostics::new(mode);
            let result = jpx::decode_with_diagnostics(
                &data,
                &Limits::default(),
                &jpx::Options::default(),
                &mut diagnostics,
            );
            if padded && mode == Recovery::Strict {
                let error = result.unwrap_err();
                assert_eq!(error.kind, ErrorKind::Syntax, "{}", case.id());
                assert_eq!(error.repair, Some(JPX_PALETTE_PADDING));
                assert_eq!(error.position, position);
                assert_eq!(error.object, None);
                assert!(diagnostics.is_empty());
                continue;
            }
            let decoded = result?;
            assert_eq!((decoded.image.width, decoded.image.height), (4, 1));
            assert_eq!(decoded.image.components.len(), planes.len());
            for ((component, expected), depth) in
                decoded.image.components.iter().zip(planes).zip(depths)
            {
                let depth = u8::try_from(depth.as_u64().ok_or("invalid depth")?)?;
                assert_eq!(component.bits, (depth & 127) + 1);
                assert_eq!(component.signed, depth & 128 != 0);
                let expected = expected
                    .as_array()
                    .ok_or("missing code plane")?
                    .iter()
                    .map(|v| {
                        v.as_u64()
                            .ok_or("invalid code")
                            .and_then(|n| u16::try_from(n).map_err(|_| "large code"))
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                assert_eq!(component.samples, expected, "{}", case.id());
            }
            assert_eq!(diagnostics.as_slice().len(), usize::from(padded));
            if let Some(diagnostic) = diagnostics.as_slice().first() {
                assert_eq!(diagnostic.repair(), Some(JPX_PALETTE_PADDING));
                assert_eq!(diagnostic.position, position);
            }
        }
        assert_eq!(data, original);
        checked += 1;
    }
    assert_eq!(checked, 10);
    Ok(())
}

#[test]
fn incomplete_data_and_invalid_channel_mappings_still_fail() -> pagura_core::Result<()> {
    for fault in [
        "short-palette",
        "missing-mapping",
        "missing-eoc",
        "bad-component",
    ] {
        for suffix in ["conforming", "padding"] {
            let data = encoded(&format!("{fault}-{suffix}"))?;
            let mut diagnostics = Diagnostics::new(Recovery::Repair);
            let error = jpx::decode_with_diagnostics(
                &data,
                &Limits::default(),
                &jpx::Options::default(),
                &mut diagnostics,
            )
            .unwrap_err();
            assert_eq!(error.kind, ErrorKind::Syntax, "{fault}-{suffix}");
            assert_eq!(
                diagnostics.as_slice().len(),
                usize::from(suffix == "padding")
            );
        }
    }
    // These complete RGBA palettes are valid to the codec. Their PDF Indexed
    // opacity interaction remains an explicit whole-page unsupported case.
    for name in [
        "opacity-1-padding",
        "opacity-2-padding",
        "opacity-2-premultiplied-padding",
    ] {
        let data = encoded(name)?;
        let mut diagnostics = Diagnostics::new(Recovery::Repair);
        let decoded = jpx::decode_with_diagnostics(
            &data,
            &Limits::default(),
            &jpx::Options::default(),
            &mut diagnostics,
        )?;
        assert_eq!(decoded.image.alpha, Some(3));
        assert_eq!(decoded.image.components.len(), 4);
        let opacity = decoded
            .opacity
            .ok_or(pagura_core::Error::syntax("missing test opacity"))?;
        assert_eq!(opacity.channel, 3);
        assert_eq!(opacity.source_channel, 3);
        assert_eq!(opacity.source_component, 0);
        assert_eq!(opacity.palette_column, Some(3));
        assert_eq!(
            opacity.kind,
            if name == "opacity-1-padding" {
                jpx::OpacityKind::Straight
            } else {
                jpx::OpacityKind::Premultiplied
            }
        );
        assert_eq!(diagnostics.as_slice().len(), 1);
    }
    Ok(())
}

#[test]
fn existing_strict_fixture_and_all_original_decoder_limits_remain_authoritative()
-> pagura_core::Result<()> {
    let data = fixture("jpx", "palette-padding.jpx");
    let error = jpx::decode(&data, &Limits::default()).unwrap_err();
    assert_eq!(error.kind, ErrorKind::Syntax);
    assert_eq!(error.repair, Some(JPX_PALETTE_PADDING));
    for suffix in ["conforming", "padding"] {
        let data = encoded(&format!("rgb-mixed-small-{suffix}"))?;
        for limits in [
            Limits {
                pixels: 3,
                ..Limits::default()
            },
            Limits {
                decode_work: 0,
                ..Limits::default()
            },
            Limits {
                stream_bytes: data.len() - 1,
                ..Limits::default()
            },
            Limits {
                decoded_bytes: 1,
                ..Limits::default()
            },
        ] {
            let mut diagnostics = Diagnostics::new(Recovery::Repair);
            let error = jpx::decode_with_diagnostics(
                &data,
                &limits,
                &jpx::Options::default(),
                &mut diagnostics,
            )
            .unwrap_err();
            assert_eq!(error.kind, ErrorKind::Limit, "{suffix}");
        }
    }
    Ok(())
}
