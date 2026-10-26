//! Public export API contracts; independent CLI pixel decoding lives in Python.

use pagura::{ErrorKind, Limits, Pixmap, Rgba8, png};

#[test]
fn surface_export_uses_straight_alpha_and_preserves_dimensions() {
    let limits = Limits::default();
    let pixels = vec![64, 32, 0, 128, 10, 20, 30, 0, 17, 23, 42, 255];
    let surface = Pixmap::from_premultiplied(3, 1, pixels, limits).unwrap();
    let encoded = png::encode_pixmap(surface, png::Level::Default, &limits).unwrap();
    let expected = png::encode(
        &[128, 64, 0, 128, 0, 0, 0, 0, 17, 23, 42, 255],
        3,
        1,
        &png::Options::default(),
    )
    .unwrap();
    assert_eq!(encoded, expected);
    assert_eq!(&encoded[..8], b"\x89PNG\r\n\x1a\n");
    assert_eq!(&encoded[16..29], &[0, 0, 0, 3, 0, 0, 0, 1, 8, 6, 0, 0, 0]);
}

#[test]
fn public_surface_export_honors_each_caller_budget() {
    for limits in [
        Limits {
            pixels: 1,
            ..Limits::default()
        },
        Limits {
            decoded_bytes: 7,
            ..Limits::default()
        },
        Limits {
            stream_bytes: 64,
            ..Limits::default()
        },
    ] {
        let surface = Pixmap::new(2, 1, Rgba8::TRANSPARENT, 2).unwrap();
        assert_eq!(
            png::encode_pixmap(surface, png::Level::Default, &limits)
                .unwrap_err()
                .kind,
            ErrorKind::Limit
        );
    }
}
