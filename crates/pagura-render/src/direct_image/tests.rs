use super::*;
use crate::{
    Tile,
    color::Color,
    display::ImageCommand,
    transparency::{Attributes, Composite},
};
use pagura_core::{ErrorKind, Position, Rect, Value};
use pagura_document::Rotation;
use pagura_image::{DecodedImage, ImageKind};
use std::sync::Arc;

fn list(rotation: Rotation) -> DisplayList {
    let decoded = DecodedImage {
        width: 3,
        height: 1,
        bits_per_component: 8,
        components: 3,
        samples: Samples::Bytes(vec![255, 0, 0, 0, 255, 0, 0, 0, 255]),
        kind: ImageKind::Color {
            color_space: Some(Value::name(b"DeviceRGB")),
        },
        decode: None,
        mask: MaskKind::None,
        interpolate: false,
    };
    let image = Image {
        decoded,
        space: Some(Space::Rgb),
        mask: None,
        matte: None,
        decode: vec![(0.0, 1.0); 3],
        intent: None,
    };
    DisplayList {
        commands: vec![Command::Image(ImageCommand {
            image: Arc::new(image),
            transform: Transform::scale(3.0, 1.0),
            clip: None,
            stencil: Color::default(),
            composite: Composite::opaque(),
            transfer: None,
            position: Position::Unknown,
        })],
        crop: Rect::from_corners(0.0, 0.0, 3.0, 1.0).unwrap(),
        rotation,
        user_unit: 1.0,
        limits: Limits::default(),
        group: Attributes::default(),
        print_intent: false,
        separations: std::sync::OnceLock::new(),
    }
}

fn image_mut(list: &mut DisplayList) -> (&mut Transform, &mut Image) {
    let Command::Image(command) = &mut list.commands[0] else {
        unreachable!("the test list holds one image")
    };
    (
        &mut command.transform,
        Arc::get_mut(&mut command.image).unwrap(),
    )
}

fn painted(list: &DisplayList) -> Pixmap {
    paint_checked(list, &RenderOptions::default())
        .unwrap()
        .unwrap()
}

#[test]
fn all_quarter_turns_keep_source_sample_order() {
    let rgb = [[255, 0, 0, 255], [0, 255, 0, 255], [0, 0, 255, 255]];
    for (rotation, reverse, size) in [
        (Rotation::None, false, (3, 1)),
        (Rotation::Clockwise90, false, (1, 3)),
        (Rotation::Clockwise180, true, (3, 1)),
        (Rotation::Clockwise270, true, (1, 3)),
    ] {
        let result = painted(&list(rotation));
        assert_eq!((result.width(), result.height()), size);
        let mut expected = rgb;
        if reverse {
            expected.reverse();
        }
        assert_eq!(result.data(), expected.as_flattened());
    }
}

#[test]
fn a_single_partial_pixel_mixes_with_the_background_once() {
    let mut list = list(Rotation::None);
    list.crop = Rect::from_corners(0.0, 0.0, 1.0, 1.0).unwrap();
    let (transform, image) = image_mut(&mut list);
    *transform = Transform::new(0.5, 0.0, 0.0, 1.0, 0.25, 0.0);
    image.decoded.width = 1;
    image.decoded.samples = Samples::Bytes(vec![0, 0, 0]);
    assert_eq!(painted(&list).data(), [128, 128, 128, 255]);
}

#[test]
fn minified_footprints_take_rational_area_weights() {
    let mut list = list(Rotation::None);
    list.crop = Rect::from_corners(0.0, 0.0, 2.0, 1.0).unwrap();
    *image_mut(&mut list).0 = Transform::scale(2.0, 1.0);
    // Each pixel covers one and a half samples: (255 + 255/2) / 1.5 = 170.
    assert_eq!(painted(&list).data(), [170, 85, 0, 255, 0, 85, 170, 255]);
    list.limits.raster_work = 2;
    assert_eq!(
        paint_checked(&list, &RenderOptions::default())
            .unwrap_err()
            .kind,
        ErrorKind::Limit
    );
    // Errors leave the page to the exact path, which reports its own.
    assert!(paint(&list, &RenderOptions::default()).is_none());
}

#[test]
fn storage_and_work_are_bounded() {
    let mut list = list(Rotation::None);
    list.limits.raster_work = 2;
    let error = paint_checked(&list, &RenderOptions::default()).unwrap_err();
    assert_eq!(error.kind, ErrorKind::Limit);
    list.limits.raster_work = 100;
    list.limits.decoded_bytes = 1;
    let error = paint_checked(&list, &RenderOptions::default()).unwrap_err();
    assert_eq!(error.kind, ErrorKind::Limit);
}

#[test]
fn extreme_coordinates_fail_without_a_partial_surface() {
    let mut list = list(Rotation::None);
    *image_mut(&mut list).0 = Transform::new(1e-308, 0.0, 0.0, 1.0, 0.0, 0.0);
    assert!(paint(&list, &RenderOptions::default()).is_none());
}

#[test]
fn unsupported_pages_decline() {
    let mut options = RenderOptions::default();
    options.background.a = 0;
    assert!(
        paint_checked(&list(Rotation::None), &options)
            .unwrap()
            .is_none()
    );
    let mut list = list(Rotation::None);
    list.commands.push(Command::Image(match &list.commands[0] {
        Command::Image(command) => ImageCommand {
            image: Arc::clone(&command.image),
            transform: command.transform,
            clip: None,
            stencil: Color::default(),
            composite: Composite::opaque(),
            transfer: None,
            position: Position::Unknown,
        },
        _ => unreachable!("the test list holds one image"),
    }));
    assert!(
        paint_checked(&list, &RenderOptions::default())
            .unwrap()
            .is_none()
    );
}

#[test]
fn tiles_are_cut_from_the_painted_page() {
    let mut list = list(Rotation::None);
    list.crop = Rect::from_corners(0.0, 0.0, 2.0, 1.0).unwrap();
    *image_mut(&mut list).0 = Transform::scale(2.0, 1.0);
    let page = painted(&list);
    for x in 0..2 {
        let tile = Tile {
            x,
            y: 0,
            width: 1,
            height: 1,
        };
        let cut = cut(&page, tile, Limits::default()).unwrap();
        assert_eq!(cut.data(), &page.data()[x as usize * 4..x as usize * 4 + 4]);
    }
    let outside = Tile {
        x: 1,
        y: 0,
        width: 2,
        height: 1,
    };
    assert!(cut(&page, outside, Limits::default()).is_none());
}
