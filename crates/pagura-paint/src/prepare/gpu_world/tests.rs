//! Structural bounds and source identity, independent of a GPU driver.
use crate::testing::{Build, mark};
use pagura_core::{ErrorKind, Limits};
use pagura_interpret::{GroupId, StateId};
use pagura_raster::{DeviceRect, Rgba8};
#[test]
fn world_vectors_are_bounded_and_invariant_to_bands() {
    let mut build = Build::new(50.0, 40.0);
    let color = build.rgb([0.2, 0.4, 0.8]);
    build.rect(
        GroupId(0),
        [1.25, 2.75, 35.125, 29.5],
        color,
        mark(StateId(0)),
    );
    let mut packets = Vec::new();
    for band in [1, 7, 32, 256] {
        let p = build
            .prepare(&build.request(build.page(), band), &Limits::default())
            .unwrap();
        let words = p.gpu_world_program(&Limits::default()).unwrap();
        assert_eq!(words[0], 0x3455_5047);
        assert_eq!(words[13], 1);
        assert_eq!(words[14], 2);
        assert_eq!(words[17] as usize, words.len());
        let patches = p.gpu_world_patches(&Limits::default()).unwrap();
        assert_eq!(patches[4], 1);
        assert_eq!(patches[5], 0);
        packets.push(words);
        for limits in [
            Limits {
                render_bytes: 0,
                ..Limits::default()
            },
            Limits {
                render_work: 0,
                ..Limits::default()
            },
        ] {
            assert_eq!(
                p.gpu_world_program(&limits).unwrap_err().kind,
                ErrorKind::Limit
            );
        }
    }
    assert!(packets.windows(2).all(|p| p[0] == p[1]));
    let region = DeviceRect {
        x0: 5,
        y0: 6,
        x1: 20,
        y1: 30,
    };
    let p = build
        .prepare(&build.request(region, 32), &Limits::default())
        .unwrap();
    assert!(p.gpu_world_program(&Limits::default()).is_err());
    build.background = Rgba8::TRANSPARENT;
    let p = build
        .prepare(&build.request(build.page(), 32), &Limits::default())
        .unwrap();
    assert!(p.gpu_world_program(&Limits::default()).is_err());
}
