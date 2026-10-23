//! Reuse admits only owners in the next render reservation.

use super::*;
use pagura_raster::{FillRule, MarkCoverage, Shape, ShapeOptions, Span, Transform};

#[test]
fn paint_headers_fit_the_new_plan_and_reuse_only_matching_capacity() {
    let sources = [Source::Solid {
        color: Box::new([0.0; 3]),
        painted: pagura_color::Painted::all(3),
    }];
    let mut previous = allocate(4096).unwrap();
    previous.resize_with(4096, PaintScratch::default);
    let required = size_of::<PaintScratch>();
    let mut bytes = Meter::new(required, "test");
    let storage = paint_storage(&sources, keep_paints(&sources, previous), &mut bytes).unwrap();
    assert_eq!(storage.len(), 1);
    assert_eq!(storage.capacity(), 1);
    assert_eq!(bytes.used(), required);
    assert_eq!(storage.iter().map(PaintScratch::bytes).sum::<usize>(), 0);

    let pointer = storage.as_ptr();
    let mut bytes = Meter::new(required, "test");
    let storage = paint_storage(&sources, keep_paints(&sources, storage), &mut bytes).unwrap();
    assert_eq!(storage.as_ptr(), pointer);
    assert_eq!(bytes.used(), required);
    let mut bytes = Meter::new(required - 1, "test");
    assert!(paint_storage(&sources, keep_paints(&sources, storage), &mut bytes).is_err());
}

#[test]
fn retained_scratch_includes_its_coverage_allocations() {
    let scene = crate::testing::Build::new(192.0, 256.0);
    let prepared = scene
        .prepare(
            &scene.request(scene.page(), 256),
            &pagura_core::Limits::default(),
        )
        .unwrap();
    let mut scratch = Scratch::default();
    let buffers = {
        let rendering = prepared.with_scratch(&mut scratch).unwrap();
        rendering.workspace.page.bytes()
            + rendering.workspace.coverage.retained_bytes()
            + rendering.workspace.boundary.0.retained_bytes()
            + rendering.rasterizer.retained_bytes()
    };
    assert!(scratch.retained_bytes() >= buffers);
}

#[test]
fn page_buffers_start_with_the_premultiplied_background() {
    let layout = Layout {
        process: pagura_color::Process::Rgb,
        spots: 0,
    };
    let mut bytes = Meter::new(usize::MAX, "test");
    let mut page = PageBuffer::new((Format::N16, layout), 2, None, &mut bytes).unwrap();
    let mut extents = Extents::new(1, &mut bytes).unwrap();
    page.reset(&mut extents, (2, 2), [10, 20, 30, 40], layout);
    let PageBuffer::N16(words) = &page else {
        unreachable!()
    };
    assert_eq!(words[1], [2570, 5140, 7710, 10280]);
    let mut wide = PageBuffer::new((Format::F64, layout), 2, None, &mut bytes).unwrap();
    wide.reset(&mut extents, (2, 2), [10, 20, 30, 40], layout);
    let PageBuffer::F64(values) = &wide else {
        unreachable!()
    };
    assert!(values.iter().all(|&v| v == 0.0));
    assert_eq!(crate::plan::area(DeviceRect::default()), 0);
}

#[test]
fn normalization_drops_incompatible_owners_before_replacement() {
    let scene = crate::testing::Build::new(8.0, 2.0);
    let limits = pagura_core::Limits::default();
    let prepared = scene
        .prepare(&scene.request(scene.page(), 2), &limits)
        .unwrap();
    let mut scratch = Scratch::default();
    let rendering = prepared.with_scratch(&mut scratch).unwrap();
    let plan = rendering.plan;
    let mut free = Meter::new(usize::MAX, "test");
    let mut previous = Workspace::new(plan, plan.sizes, None, &mut free).unwrap();
    previous.page = PageBuffer::F64(allocate(4096).unwrap());
    previous.coverage = BandCoverage::with_capacity(coverage_capacity(64, 64), &mut free).unwrap();
    previous.capacity = (64, 4096);
    previous.paint_scratch = allocate(4096).unwrap();
    previous
        .paint_scratch
        .resize_with(4096, PaintScratch::default);

    // This step has no meter or constructor: every incompatible owner has
    // gone before Scratch::fit can make a replacement rasterizer or buffer.
    let kept = Kept::new(plan, previous);
    assert!(kept.page.is_none());
    assert!(kept.coverage.is_none());
    assert!(kept.paints.is_none());
    let mut refused = Meter::new(0, "test");
    assert!(Workspace::new(plan, plan.sizes, Some(kept), &mut refused).is_err());
    assert_eq!(refused.used(), 0);
}

#[test]
fn exact_page_and_coverage_owners_keep_pointers_and_charges() {
    let scene = crate::testing::Build::new(8.0, 2.0);
    let limits = pagura_core::Limits::default();
    let prepared = scene
        .prepare(&scene.request(scene.page(), 2), &limits)
        .unwrap();
    let mut scratch = Scratch::default();
    let rendering = prepared.with_scratch(&mut scratch).unwrap();
    let plan = rendering.plan;
    let mut cold = Meter::new(usize::MAX, "test");
    let mut previous = Workspace::new(plan, plan.sizes, None, &mut cold).unwrap();
    let page = page_pointer(&previous.page);
    let coverage_bytes = previous.coverage.retained_bytes();
    let shape = Shape::fill(
        &crate::testing::rect_path([0.25, 0.25, 4.75, 1.75]),
        Transform::IDENTITY,
        FillRule::NonZero,
        &ShapeOptions {
            area: plan.area,
            limits: &limits,
        },
    )
    .unwrap()
    .unwrap();
    let mark = MarkCoverage {
        shape: Some(&shape),
        clip: None,
    };
    rendering
        .rasterizer
        .rasterize(mark, 0, &mut previous.coverage);
    let cells = partial_pointer(&previous.coverage);

    let kept = Kept::new(plan, previous);
    assert_eq!(page_pointer(kept.page.as_ref().unwrap()), page);
    assert!(kept.coverage.as_ref().unwrap().is_empty());
    let mut reused = Meter::new(cold.used(), "test");
    let mut workspace = Workspace::new(plan, plan.sizes, Some(kept), &mut reused).unwrap();
    assert_eq!(reused.used(), cold.used());
    assert_eq!(page_pointer(&workspace.page), page);
    assert_eq!(workspace.coverage.retained_bytes(), coverage_bytes);
    rendering
        .rasterizer
        .rasterize(mark, 0, &mut workspace.coverage);
    assert_eq!(partial_pointer(&workspace.coverage), cells);
}

#[test]
fn paint_normalization_keeps_headers_and_only_matching_image_owners() {
    let image = image_source();
    let mut free = Meter::new(usize::MAX, "test");
    let mut previous = allocate(1).unwrap();
    previous.push(image.make_scratch(None, &mut free).unwrap());
    let pointer = previous.as_ptr();
    let image_bytes = previous[0].bytes();
    assert!(image_bytes > 0);
    let sources = [image];
    let kept = keep_paints(&sources, previous).unwrap();
    assert_eq!(kept.as_ptr(), pointer);
    assert_eq!(kept[0].bytes(), image_bytes);
    let required = size_of::<PaintScratch>() + image_bytes;
    let mut exact = Meter::new(required, "test");
    let previous = paint_storage(&sources, Some(kept), &mut exact).unwrap();
    assert_eq!(previous.as_ptr(), pointer);
    assert_eq!(exact.used(), required);

    let sources = [Source::Solid {
        color: Box::new([0.0; 3]),
        painted: pagura_color::Painted::all(3),
    }];
    let kept = keep_paints(&sources, previous).unwrap();
    assert_eq!(kept.as_ptr(), pointer);
    assert_eq!(kept[0].bytes(), 0);
    assert!(kept[0].fits(&sources[0]));
}

fn page_pointer(page: &PageBuffer) -> *const u8 {
    match page {
        PageBuffer::F64(values) => values.as_ptr().cast(),
        PageBuffer::N16(values) => values.as_ptr().cast(),
        PageBuffer::F32(values) => values.as_ptr().cast(),
    }
}

fn partial_pointer(coverage: &BandCoverage) -> *const u8 {
    coverage
        .rows()
        .find_map(|row| {
            row.spans().find_map(|span| match span {
                Span::Partial { coverage, .. } => Some(coverage.as_ptr()),
                Span::Full { .. } => None,
            })
        })
        .unwrap()
}

fn image_source() -> Source {
    use crate::source::{ImageBudgets, ImageSampler, PaintContext};
    use pagura_color::{ColorSpace, Target};
    use pagura_core::{Limits, Store};
    use pagura_document::{Document, OpenOptions};
    use pagura_image::ImageSource;
    use pagura_interpret::{Composite, Image, ImageMask};
    use std::sync::Arc;

    let document = Document::open(
        pagura_testkit::fixture("image", "samples-1-1-raw.pdf"),
        &OpenOptions::default(),
    )
    .unwrap();
    let source = document
        .object_ids()
        .find_map(|id| ImageSource::from_xobject(&document, document.object(id).ok()?).ok())
        .unwrap();
    let image = Image {
        source: Arc::new(source),
        space: Some(Arc::new(ColorSpace::DeviceGray)),
        decode: Arc::from([[0.0, 1.0]]),
        interpolate: false,
        intent: None,
        mask: ImageMask::None,
    };
    let mut bytes = Meter::new(usize::MAX, "test");
    let mut work = Meter::new(usize::MAX, "test");
    let mut functions = Meter::new(usize::MAX, "test");
    let context = PaintContext {
        transform: Transform::IDENTITY,
        target: &Target::new(pagura_color::Process::Rgb),
        composite: &Composite::default(),
        store: &Store::new(1 << 20),
        limits: &Limits::default(),
        region: DeviceRect {
            x0: 0,
            y0: 0,
            x1: 8,
            y1: 2,
        },
    };
    Source::Image(
        ImageSampler::prepare_with(
            &image,
            Transform::scale(8.0, 2.0),
            &context,
            16,
            ImageBudgets {
                bytes: &mut bytes,
                work: &mut work,
                functions: &mut functions,
            },
        )
        .unwrap(),
    )
}
