//! Draws of one image share execution storage, colour memo included, and
//! render exactly as with storage of their own.

use crate::testing::{Build, mark};
use pagura_color::ColorSpace;
use pagura_core::Limits;
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
