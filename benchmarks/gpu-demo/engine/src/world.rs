//! One owned canonical vector packet per recorded page, never DPI-keyed.
use super::{Cow, Document, Instant, MAX_FRAME, Result, Scene, Scratch, Store, json_string, ms};
use pagura_core::{Error, ErrorKind, Limits};
use pagura_interpret::Node;
use pagura_paint::{OverprintMode, RenderRequest};
use pagura_raster::{Adjustment, DeviceRect};

pub fn export<'a>(
    cache: &'a mut Option<Vec<u8>>,
    scene: &Scene,
    document: &Document,
    store: &Store,
    scratch: &mut Scratch,
    total: Instant,
) -> Result<(String, Cow<'a, [u8]>)> {
    let hit = cache.is_some();
    let mut prepare_ms = 0.0;
    let mut export_ms = 0.0;
    if !hit {
        let mut classification_work = 0;
        if scene.groups.iter().any(|group| group.view.is_some()) {
            return Err(Error::unsupported(
                pagura_core::Feature::new("gpu.world"),
                "fixed-size annotation views are resolution dependent",
            ));
        }
        for group in &scene.groups {
            if let Some(path) = &group.bbox {
                independent_path(
                    path,
                    pagura_raster::FillRule::NonZero,
                    &document.limits(),
                    &mut classification_work,
                )?;
            }
        }
        for paint in &scene.paints {
            if let pagura_interpret::Paint::Shading(paint) = paint.as_ref() {
                independent_shading(&paint.shading)?;
            }
        }
        for clip in &scene.clips {
            independent_path(
                &clip.path,
                clip.rule,
                &document.limits(),
                &mut classification_work,
            )?;
        }
        for node in &scene.nodes {
            if let Node::Shading(shading) = node {
                independent_shading(&shading.shading)?;
            }

            if let Node::Fill(fill) = node {
                independent_path(
                    &fill.path,
                    fill.rule,
                    &document.limits(),
                    &mut classification_work,
                )?;
            }
            if let Node::Stroke(stroke) = node {
                let pen = &scene.pens[stroke.pen.index()];
                if pen.stroke.width == 0.0 || pen.adjustment != Adjustment::None {
                    return Ok((
                        format!(
                            "{{\"ok\":true,\"fallback\":{}}}",
                            json_string(
                                "zoom-independent geometry does not scale device hairlines or adjusted strokes"
                            )
                        ),
                        Cow::Owned(Vec::new()),
                    ));
                }
            }
        }
        let scale = 300.0 / 72.0;
        let (w, h) = scene.page.device_size(scale)?;
        if u64::from(w) * u64::from(h) > document.limits().pixels as u64 {
            return Err(Error::limit("canonical world pixel budget"));
        }
        let page = DeviceRect {
            x0: 0,
            y0: 0,
            x1: w,
            y1: h,
        };
        let mut request = RenderRequest::new(scene.page.device_transform(scale), page, page);
        request.overprint = if scene.print_intent {
            OverprintMode::Simulate
        } else {
            OverprintMode::Ignore
        };
        let start = Instant::now();
        let prepared = scratch.prepare(scene, &request, (store, &document.limits()))?;
        for repair in prepared.repairs() {
            document.repair(repair.clone())?;
        }
        prepare_ms = ms(start);
        let start = Instant::now();
        let words = prepared.gpu_world_program(&Limits {
            render_bytes: MAX_FRAME,
            ..document.limits()
        });
        scratch.recycle(prepared);
        match words {
            Ok(words) => {
                let mut bytes = Vec::new();
                bytes
                    .try_reserve_exact(words.len() * 4)
                    .map_err(|_| Error::limit("world wire allocation"))?;
                for word in words {
                    bytes.extend_from_slice(&word.to_le_bytes());
                }
                *cache = Some(bytes);
            }
            Err(error) if matches!(error.kind, ErrorKind::Unsupported(_) | ErrorKind::Limit) => {
                return Ok((
                    format!(
                        "{{\"ok\":true,\"fallback\":{},\"engine_ms\":{}}}",
                        json_string(&error.to_string()),
                        ms(total)
                    ),
                    Cow::Owned(Vec::new()),
                ));
            }
            Err(error) => return Err(error),
        }
        export_ms = ms(start);
    }
    let (mut width, mut height) = (
        scene.page.crop.width() * scene.page.user_unit,
        scene.page.crop.height() * scene.page.user_unit,
    );
    if matches!(
        scene.page.rotation,
        pagura_document::Rotation::Clockwise90 | pagura_document::Rotation::Clockwise270
    ) {
        std::mem::swap(&mut width, &mut height);
    }
    Ok((
        format!(
            "{{\"ok\":true,\"world_cache_hit\":{hit},\"canonical_dpi\":300,\"extent_width\":{width},\"extent_height\":{height},\"prepare_ms\":{prepare_ms},\"export_ms\":{export_ms},\"engine_ms\":{}}}",
            ms(total)
        ),
        Cow::Borrowed(
            cache
                .as_ref()
                .ok_or(Error::invalid_argument("missing world packet"))?,
        ),
    ))
}

// Recording/transformation can clear a Path certificate. Reclassify once at
// world setup so floor-cell points and device-width boundaries never scale.
fn independent_path(
    path: &pagura_raster::Path,
    rule: pagura_raster::FillRule,
    limits: &Limits,
    work: &mut usize,
) -> Result<()> {
    let mut classified = path.clone();
    classified.classify_fill(rule, limits, work)?;
    if classified.fill_region(rule) != Some(pagura_raster::FillRegion::Ordinary) {
        return Err(Error::unsupported(
            pagura_core::Feature::new("gpu.world"),
            "zero-area fills or clips are resolution dependent",
        ));
    }
    Ok(())
}

fn independent_shading(shading: &pagura_interpret::Shading) -> Result<()> {
    if shading
        .bbox
        .is_some_and(|rect| rect.width() == 0.0 || rect.height() == 0.0)
    {
        return Err(Error::unsupported(
            pagura_core::Feature::new("gpu.world"),
            "zero-area shading bounds are resolution dependent",
        ));
    }
    Ok(())
}
