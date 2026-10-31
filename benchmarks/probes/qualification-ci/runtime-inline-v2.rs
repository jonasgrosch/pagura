#![forbid(unsafe_code)]
//! Hosted native/WASI independent oracle controls for compatibility gaps.
use pagura::{Document, OpenOptions, Recovery, RenderOptions, Rgba8};
use std::{error::Error, fs, sync::Arc};

const REVISION: &str = env!("PAGURA_QUAL_SOURCE_REVISION");

fn main() -> Result<(), Box<dyn Error>> {
    println!("qualification-source={REVISION}");
    let mut args = std::env::args().skip(1);
    let input = args.next().ok_or("input required")?;
    let output = args.next().ok_or("output required")?;
    let recovery = match args.next().as_deref() {
        Some("repair") => Recovery::Repair,
        Some("strict") => Recovery::Strict,
        _ => return Err("expected strict or repair".into()),
    };
    let dpi: f64 = args.next().ok_or("dpi required")?.parse()?;
    let flags: Vec<_> = args.collect();
    if flags.iter().any(|f| f != "controls" && f != "aggregate" && f != "skip-raster-budget" && !f.starts_with("page=")) {
        return Err("unexpected argument".into());
    }
    let indices: Vec<_> = flags.iter().filter_map(|f| f.strip_prefix("page=")).collect();
    if indices.len() > 1 { return Err("duplicate page index".into()); }
    let page_index = indices.first().map_or(Ok(0), |value| value.parse::<usize>())?;
    println!("qualification-page={page_index}");
    let controls = flags.iter().any(|f| f == "controls");
    let raster_budget = !flags.iter().any(|f| f == "skip-raster-budget");
    let aggregate = flags.iter().any(|f| f == "aggregate");
    let document = Arc::new(Document::open(
        fs::read(&input)?,
        &OpenOptions {
            recovery,
            ..OpenOptions::default()
        },
    )?);
    let context = pagura::render_context(Arc::clone(&document));
    let options = RenderOptions {
        background: Rgba8::WHITE,
        ..RenderOptions::from_dpi(dpi)
    };
    if page_index > 0 {
        // Exercise later-page rendering after the same context rendered page0.
        let first = context.render_page(document.page(0)?, &options)?;
        if first.width() == 0 || first.height() == 0 { return Err("empty prior page".into()); }
        drop(first);
        println!("prior-page-rendered=0");
    }
    let pixels = match context.render_page(document.page(page_index)?, &options) {
        Ok(pixels) => pixels,
        Err(original) => {
            if controls {
                let wanted = format!("{original:?}");
                let mut retained = None;
                for _ in 0..2 {
                    let mut callbacks = 0;
                    let result = match context.scene(document.page(page_index)?) {
                        Ok(scene) => context.render_bands(
                            &scene, &options, None, |_| false,
                            |_, _| -> Result<(), Box<dyn Error>> { callbacks += 1; Ok(()) },
                        ),
                        Err(error) => Err(error),
                    };
                    let error = result.err().ok_or("malformed inline page unexpectedly streamed")?;
                    if callbacks != 0 || format!("{error:?}") != wanted {
                        return Err("malformed inline page emitted callback or changed its error".into());
                    }
                    let current = context.retained_bytes();
                    if let Some(previous) = retained {
                        if previous != current { return Err("malformed inline page grew Store on repeat".into()); }
                    }
                    retained = Some(current);
                }
                println!("full-failure-no-callback-repeat-store=pass");
            }
            return Err(original.into());
        }
    };
    if controls {
        let scene = context.scene(document.page(page_index)?)?;
        let mut streamed = Vec::new();
        let mut rows = 0;
        context.render_bands(
            &scene, &options, None, |_| false,
            |range, bytes| -> Result<(), Box<dyn Error>> {
                if range.start != rows { return Err("band row discontinuity".into()); }
                rows = range.end;
                streamed.extend_from_slice(bytes);
                Ok(())
            },
        )??;
        if rows != pixels.height() || streamed != pixels.data() {
            return Err("full streamed band RGBA mismatch".into());
        }
        drop(streamed);
        // Four uneven tiles make both nonzero region origins and seams observable.
        let (width, height) = (pixels.width(), pixels.height());
        let xs = if width > 1 { vec![0, width / 2, width] } else { vec![0, width] };
        let ys = if height > 1 { vec![0, height / 2, height] } else { vec![0, height] };
        for yw in ys.windows(2) {
            for xw in xs.windows(2) {
                let tile = pagura::Tile { x: xw[0], y: yw[0], width: xw[1] - xw[0], height: yw[1] - yw[0] };
                let actual = context.render_tile(document.page(page_index)?, &options, tile)?;
                for row in 0..tile.height as usize {
                    let first = ((tile.y as usize + row) * width as usize + tile.x as usize) * 4;
                    let expected = &pixels.data()[first..first + tile.width as usize * 4];
                    let first = row * tile.width as usize * 4;
                    if &actual.data()[first..first + tile.width as usize * 4] != expected {
                        return Err("tile RGBA mismatch".into());
                    }
                }
            }
        }
        // Same retained context must also reproduce the whole result after tile renders.
        let warm = context.render_page(document.page(page_index)?, &options)?;
        if warm.data() != pixels.data() { return Err("warm RGBA mismatch after tiles".into()); }
        let transparent_options = RenderOptions { background: Rgba8::TRANSPARENT, ..options };
        let transparent = context.render_page(document.page(page_index)?, &transparent_options)?;
        let transparent_warm = context.render_page(document.page(page_index)?, &transparent_options)?;
        if transparent_warm.data() != transparent.data() {
            return Err("transparent warm RGBA mismatch".into());
        }
        std::io::Write::write_all(
            &mut fs::OpenOptions::new().write(true).create_new(true)
                .open(format!("{output}.transparent.rgba"))?,
            transparent.data(),
        )?;
        let mut budgets = vec![
            ("render-bytes", pagura::Limits { render_bytes: 0, ..pagura::Limits::default() }),
            ("render-work", pagura::Limits { render_work: 0, ..pagura::Limits::default() }),
        ];
        if raster_budget {
            budgets.push(("raster-work", pagura::Limits { raster_work: 0, ..pagura::Limits::default() }));
        }
        if aggregate {
            budgets.push(("aggregate-render-work", pagura::Limits { render_work: 1_000_000, ..pagura::Limits::default() }));
        }
        for (name, limits) in budgets {
            let low_provider = pagura::provider::default_provider(limits);
            let low_document = Arc::new(Document::open(fs::read(&input)?, &OpenOptions { limits, recovery, ..OpenOptions::default() })?);
            let low = pagura::RenderContext::with_store_capacity(low_document, low_provider, 64 * 1024 * 1024);
            let mut previous = None;
            let mut retained = None;
            for _ in 0..2 {
                let mut callbacks = 0;
                let result = match low.scene(low.document().page(page_index)?) {
                    Ok(scene) => low.render_bands(
                        &scene, &options, None, |_| false,
                        |_, _| -> Result<(), Box<dyn Error>> { callbacks += 1; Ok(()) },
                    ),
                    Err(error) => Err(error),
                };
                let error = result.err().ok_or("low budget unexpectedly rendered")?;
                if error.kind != pagura::ErrorKind::Limit || callbacks != 0 {
                    return Err("low budget emitted callback or wrong error".into());
                }
                let current = format!("{error:?}");
                if let Some(previous) = &previous {
                    if previous != &current { return Err("low budget error changed".into()); }
                }
                previous = Some(current);
                let current = low.retained_bytes();
                if current > 64 * 1024 * 1024 { return Err("low budget Store exceeded capacity".into()); }
                if let Some(previous) = retained {
                    if previous != current { return Err("low budget Store grew on repeat".into()); }
                }
                retained = Some(current);
            }
            println!("low-budget-{name}-no-callback-repeat-store=pass");
        }
        println!("full-band-tile-warm-controls=pass");
    }
    let mut bytes = format!("P6\n{} {}\n255\n", pixels.width(), pixels.height()).into_bytes();
    bytes.extend(pixels.to_rgb_over(Rgba8::WHITE));
    // These are fresh hosted scratch paths, never document-selected paths.
    let rgba_output = format!("{output}.rgba");
    std::io::Write::write_all(
        &mut fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(output)?,
        &bytes,
    )?;
    std::io::Write::write_all(
        &mut fs::OpenOptions::new().write(true).create_new(true).open(rgba_output)?,
        pixels.data(),
    )?;
    for diagnostic in document.diagnostics() {
        println!("{}", diagnostic.tag());
    }
    Ok(())
}
