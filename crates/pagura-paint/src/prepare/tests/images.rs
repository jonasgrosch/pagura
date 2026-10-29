//! Draws of one image share execution storage, colour memo included, and
//! render exactly as with storage of their own. Optional image storage never
//! raises the budget a page needs.

use crate::source::Source;
use crate::testing::{Build, mark};
use crate::{PreparedScene, Scratch};
use pagura_color::ColorSpace;
use pagura_core::{ErrorKind, Limits};
use pagura_document::{Document, OpenOptions};
use pagura_image::ImageSource;
use pagura_interpret::{GroupId, Image, ImageMask, ImageNode, Node};
use pagura_raster::Transform;
use std::sync::Arc;

fn cmyk(interpolate: bool) -> Image {
    let document = Document::open(
        pagura_testkit::fixture("image", "samples-8-4-raw.pdf"),
        &OpenOptions::default(),
    )
    .unwrap();
    let source = document
        .object_ids()
        .find_map(|id| ImageSource::from_xobject(&document, document.object(id).ok()?).ok())
        .unwrap();
    Image {
        source: Arc::new(source),
        space: Some(Arc::new(ColorSpace::DeviceCmyk)),
        decode: Arc::from([[0.0, 1.0]; 4]),
        interpolate,
        intent: None,
        mask: ImageMask::None,
    }
}

/// 24 draws alternating two images, each draw of an image its own when
/// `shared` is false.
fn draws(images: &[Arc<Image>; 2], shared: bool) -> Build {
    let mut b = Build::new(96.0, 64.0);
    let state = b.state(|_| {});
    for i in 0..24_u32 {
        let image = &images[usize::from(i % 2 == 1)];
        let image = if shared {
            Arc::clone(image)
        } else {
            Arc::new(Image::clone(image))
        };
        let (x, y) = (f64::from(i % 6) * 15.0, f64::from(i / 6) * 15.0);
        b.node(
            GroupId(0),
            Node::Image(ImageNode {
                image,
                transform: Transform::new(13.0, 0.0, 0.0, 11.0, x + 1.0, y + 2.0),
                stencil: None,
                mark: mark(state),
            }),
        );
    }
    b
}

#[test]
fn draws_of_one_image_share_storage_and_render_as_with_their_own() {
    let images = [Arc::new(cmyk(false)), Arc::new(cmyk(true))];
    let (shared, separate) = (draws(&images, true), draws(&images, false));
    let request = shared.request(shared.page(), 16);
    let storages = |b: &Build| {
        let prepared = b.prepare(&request, &Limits::default()).unwrap();
        prepared.plan.slots.layouts.len()
    };
    assert_eq!(storages(&shared), 2);
    assert_eq!(storages(&separate), 24);
    assert_eq!(
        shared.render(&request).unwrap(),
        separate.render(&request).unwrap()
    );
}

/// The first image of the paint fixture `file`, in `space`.
fn image(file: &str, space: ColorSpace) -> Image {
    let document = Document::open(
        pagura_testkit::fixture("paint", file),
        &OpenOptions::default(),
    )
    .unwrap();
    let source = document
        .object_ids()
        .find_map(|id| ImageSource::from_xobject(&document, document.object(id).ok()?).ok())
        .unwrap();
    Image {
        source: Arc::new(source),
        space: Some(Arc::new(space)),
        decode: Arc::from([[0.0, 1.0]; 4]),
        interpolate: false,
        intent: None,
        mask: ImageMask::None,
    }
}

/// An ICCBased RGB space through the sampled, sRGB-like matrix/TRC profile
/// of the colour fixtures.
fn icc_rgb() -> ColorSpace {
    let bytes = pagura_testkit::fixture("color", "sampled-rgb.icc");
    let profile = pagura_color::Profile::parse(&bytes, 3, &Limits::default())
        .unwrap()
        .unwrap();
    ColorSpace::Icc(Arc::new(pagura_color::IccSpace {
        profile: Some(Arc::new(profile)),
        alternate: Arc::new(ColorSpace::DeviceRgb),
        components: 3,
        range: [[0.0, 1.0]; 4],
        object: None,
    }))
}

/// The pixels of `prepared`.
fn pixels(prepared: &PreparedScene) -> Vec<u8> {
    let mut scratch = Scratch::default();
    let mut rendering = prepared.with_scratch(&mut scratch).unwrap();
    let mut rows = Vec::new();
    rendering.render(&mut rows);
    rows
}

/// A page whose early image can keep optional curve values (48×36 RGB
/// samples in a matrix/TRC ICC space), followed by an image that must hold
/// far more (1024×1024 gray samples), prepares at the budget its own storage
/// needs, without the curve values, and fails one byte below it. The curve
/// values take only what the page leaves, and the pixels are the same with
/// and without them.
#[test]
fn optional_curve_values_never_raise_the_budget_a_page_needs() {
    let mut b = Build::new(96.0, 64.0);
    let state = b.state(|_| {});
    let images = [
        (
            image("image-direct-rgb-fractional.pdf", icc_rgb()),
            Transform::new(60.0, 0.0, 0.0, 45.0, 2.0, 2.0),
        ),
        (
            image("image-area-tie-1024-159.pdf", ColorSpace::DeviceGray),
            Transform::new(90.0, 0.0, 0.0, 60.0, 3.0, 2.0),
        ),
    ];
    for (image, transform) in images {
        b.node(
            GroupId(0),
            Node::Image(ImageNode {
                image: Arc::new(image),
                transform,
                stencil: None,
                mark: mark(state),
            }),
        );
    }
    let request = b.request(b.page(), 16);
    let held = |prepared: &PreparedScene| -> usize {
        prepared
            .plan
            .paints
            .iter()
            .map(|paint| match paint {
                Source::Image(image) => image.curve_bytes(),
                _ => 0,
            })
            .sum()
    };
    let roomy = b.prepare(&request, &Limits::default()).unwrap();
    let optional = held(&roomy);
    assert!(optional > 0, "the early image kept no curve values");
    let needed = roomy.charges().render_bytes() - optional;
    let limits = |render_bytes| Limits {
        render_bytes,
        ..Limits::default()
    };
    let tight = b.prepare(&request, &limits(needed)).unwrap();
    assert_eq!((held(&tight), tight.charges().render_bytes()), (0, needed));
    let short = b.prepare(&request, &limits(needed - 1)).unwrap_err();
    assert_eq!(short.kind, ErrorKind::Limit);
    let partial = b.prepare(&request, &limits(needed + optional - 1)).unwrap();
    assert_eq!(held(&partial), 0);
    assert_eq!(pixels(&tight), pixels(&roomy));
}
