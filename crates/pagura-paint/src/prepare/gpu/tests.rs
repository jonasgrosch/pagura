//! Packet equivalence, partition invariance and conservative admission.

use super::*;
use crate::testing::{Build, mark};
use pagura_core::ErrorKind;
use pagura_interpret::{GroupId, StateId};
use pagura_raster::Rgba8;

/// Deliberately simple replay: walk every span, without using the bin index.
fn replay(program: &Program) -> Vec<u8> {
    let w = program.words();
    let unpack = |rg: u32, ba: u32| [rg & 65535, rg >> 16, ba & 65535, ba >> 16];
    let width = w[2] as usize;
    let mut pixels = vec![unpack(w[5], w[6]); width * w[3] as usize];
    for span in w[w[7] as usize..w[8] as usize].as_chunks::<8>().0 {
        let source = unpack(span[4], span[5]);
        for x in 0..span[2] {
            let coverage = if span[3] == FULL {
                255
            } else {
                let offset = span[3] + x;
                (w[w[8] as usize + offset as usize / 4] >> ((offset % 4) * 8)) & 255
            };
            let pixel = &mut pixels[span[0] as usize * width + (span[1] + x) as usize];
            for (out, value) in pixel.iter_mut().zip(source) {
                *out = (coverage * value + (255 - coverage) * *out + 127) / 255;
            }
        }
    }
    pixels
        .into_iter()
        .flatten()
        .map(|value| u8::try_from((value + 128) / 257).unwrap())
        .collect()
}

fn artwork() -> Build {
    let mut build = Build::new(77.0, 51.0);
    for i in 0_u32..18 {
        let n = f64::from(i);
        let paint = build.rgb([n / 19.0, (18.0 - n) / 19.0, 0.50001]);
        let mut info = mark(StateId(0));
        if i % 3 == 0 {
            info.clip = Some(build.clip(None, [5.25, 3.4, 65.7, 47.25]));
        }
        build.rect(
            GroupId(0),
            [n * 0.7 - 2.0, n * 0.25, n * 0.7 + 60.2, 50.8 - n],
            paint,
            info,
        );
    }
    build
}

#[test]
fn packets_preserve_cpu_pixels_on_bands_regions_and_transparent_backgrounds() {
    let mut build = artwork();
    for background in [Rgba8::WHITE, Rgba8::TRANSPARENT] {
        build.background = background;
        for band in [1, 7, 32, 256] {
            for region in [
                build.page(),
                DeviceRect {
                    x0: 3,
                    y0: 5,
                    x1: 70,
                    y1: 44,
                },
            ] {
                let prepared = build
                    .prepare(&build.request(region, band), &Limits::default())
                    .unwrap();
                assert_eq!(prepared.gpu_fallback_reason(), None);
                let mut scratch = Scratch::default();
                let packet = prepared
                    .gpu_program(&mut scratch, &Limits::default())
                    .unwrap();
                let mut cpu = Vec::new();
                prepared
                    .with_scratch(&mut scratch)
                    .unwrap()
                    .render(&mut cpu);
                assert_eq!(replay(&packet), cpu, "band {band}, region {region:?}");
            }
        }
    }
}

#[test]
fn refuses_dynamic_opacity_and_never_downgrades_f64() {
    let mut build = Build::new(8.0, 8.0);
    let paint = build.rgb([0.3, 0.5, 0.7]);
    let state = build.state(|state| state.alpha = 0.5);
    build.rect(GroupId(0), [0.0, 0.0, 8.0, 8.0], paint, mark(state));
    let prepared = build
        .prepare(&build.request(build.page(), 7), &Limits::default())
        .unwrap();
    assert!(prepared.gpu_fallback_reason().is_some());
    let error = prepared
        .gpu_program(&mut Scratch::default(), &Limits::default())
        .unwrap_err();
    assert_eq!(error.kind, ErrorKind::Unsupported(Feature::new("gpu.demo")));

    let state = build.state(|state| state.blend = BlendMode::ColorDodge);
    build.rect(GroupId(0), [0.0, 0.0, 8.0, 8.0], paint, mark(state));
    let prepared = build
        .prepare(&build.request(build.page(), 7), &Limits::default())
        .unwrap();
    assert_eq!(prepared.plan.groups[0].format, Format::F64);
    assert_eq!(
        prepared.gpu_fallback_reason(),
        Some("the page requires F32 or F64 precision")
    );
}

#[test]
fn packet_storage_and_bin_work_are_bounded() {
    let build = artwork();
    let prepared = build
        .prepare(&build.request(build.page(), 7), &Limits::default())
        .unwrap();
    for limits in [
        Limits {
            render_bytes: 16,
            ..Limits::default()
        },
        Limits {
            render_work: 0,
            ..Limits::default()
        },
    ] {
        let error = prepared
            .gpu_program(&mut Scratch::default(), &limits)
            .unwrap_err();
        assert_eq!(error.kind, ErrorKind::Limit);
    }
    // Refusal leaves the shared scratch usable by the CPU.
    let mut scratch = Scratch::default();
    assert!(
        prepared
            .gpu_program(
                &mut scratch,
                &Limits {
                    render_bytes: 16,
                    ..Limits::default()
                }
            )
            .is_err()
    );
    let mut pixels = Vec::new();
    prepared
        .with_scratch(&mut scratch)
        .unwrap()
        .render(&mut pixels);
    assert_eq!(pixels.len(), 77 * 51 * 4);
}

#[test]
fn gpu_geometry_is_optional_bounded_and_does_not_change_cpu_rendering() {
    let build = artwork();
    let limits = Limits::default();
    let prepared = build
        .prepare(&build.request(build.page(), 7), &limits)
        .unwrap();
    let mut scratch = Scratch::default();
    let mut before = Vec::new();
    prepared
        .with_scratch(&mut scratch)
        .unwrap()
        .render(&mut before);
    for local in [false, true] {
        let words = prepared.gpu_raster_program(&limits, local).unwrap();
        assert_eq!(words[0], 0x3255_5047);
        assert_eq!(words[19], u32::from(local));
        assert_eq!(words[17] as usize, words.len());
        assert!(words[14] > 0);
        let bytes = Limits {
            render_bytes: 64,
            ..limits
        };
        assert_eq!(
            prepared.gpu_raster_program(&bytes, local).unwrap_err().kind,
            ErrorKind::Limit
        );
        let work = Limits {
            render_work: 0,
            ..limits
        };
        assert_eq!(
            prepared.gpu_raster_program(&work, local).unwrap_err().kind,
            ErrorKind::Limit
        );
    }
    let mut after = Vec::new();
    prepared
        .with_scratch(&mut scratch)
        .unwrap()
        .render(&mut after);
    assert_eq!(before, after);
}

#[test]
fn gpu_geometry_requires_an_opaque_background_and_preserves_band_independence() {
    let mut build = artwork();
    let limits = Limits::default();
    let a = build
        .prepare(&build.request(build.page(), 1), &limits)
        .unwrap()
        .gpu_raster_program(&limits, true)
        .unwrap();
    let b = build
        .prepare(&build.request(build.page(), 256), &limits)
        .unwrap()
        .gpu_raster_program(&limits, true)
        .unwrap();
    assert_eq!(a, b);
    build.background = Rgba8::TRANSPARENT;
    let prepared = build
        .prepare(&build.request(build.page(), 7), &limits)
        .unwrap();
    assert!(matches!(
        prepared.gpu_raster_program(&limits, true).unwrap_err().kind,
        ErrorKind::Unsupported(_)
    ));
}
