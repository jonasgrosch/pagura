#![forbid(unsafe_code)]
// Benchmark glue uses only public APIs; no competitor implementation is copied.
use hayro::hayro_interpret::InterpreterSettings;
use hayro::hayro_syntax::Pdf;
use hayro::{PixmapSettings, RenderCache, RenderSettings};
use std::error::Error;
use std::io::Write;
use std::time::Instant;

fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.as_slice() == ["--version"] {
        println!("hayro 0.8.0; standalone public-API adapter");
        return Ok(());
    }
    if !matches!(args.len(), 4 | 5) {
        return Err("expected INPUT OUTPUT DPI PAGE or INPUT OUTDIR DPI FIRST LAST".into());
    }
    let first = args[3].parse::<usize>()?;
    let last = args.last().ok_or("missing page")?.parse::<usize>()?;
    let scale = args[2].parse::<f32>()? / 72.0;
    if !scale.is_finite() || scale <= 0.0 {
        return Err("DPI must be finite and positive".into());
    }
    let started = Instant::now();
    let pdf = Pdf::new(std::fs::read(&args[0])?).map_err(|e| format!("{e:?}"))?;
    let pages = pdf.pages();
    if first > last || last >= pages.len() {
        return Err("page range outside document".into());
    }
    let opened = started.elapsed();
    let mut stages = Vec::new();
    let mut cache = None;
    for index in first..=last {
        let render_started = Instant::now();
        let settings = PixmapSettings {
            x_scale: scale,
            y_scale: scale,
            ..Default::default()
        };

        let page = pages.get(index).ok_or("missing page")?;
        let cache = cache.get_or_insert_with(RenderCache::new);
        let pixmap = hayro::render(
            page,
            cache,
            &InterpreterSettings::default(),
            &RenderSettings::default(),
            &settings,
        );
        let rendered = render_started.elapsed();
        let emit_started = Instant::now();
        let header = format!(
            "RGBA8 {} {} premultiplied\n",
            pixmap.width(),
            pixmap.height()
        );
        let output = if args.len() == 5 {
            std::path::Path::new(&args[1]).join(format!("page-{:04}.ppm", index + 1))
        } else {
            std::path::PathBuf::from(&args[1])
        };
        let mut file = std::fs::File::create(output)?;
        file.write_all(header.as_bytes())?;
        file.write_all(pixmap.data_as_u8_slice())?;
        drop(file);
        let emitted = emit_started.elapsed();
        stages.push(format!(
            "{{\"page\":{index},\"render_ns\":{},\"emit_ns\":{}}}",
            rendered.as_nanos(),
            emitted.as_nanos()
        ));
    }
    println!(
        "{{\"schema\":1,\"open_ns\":{},\"pages\":[{}],\"total_ns\":{}}}",
        opened.as_nanos(),
        stages.join(","),
        started.elapsed().as_nanos()
    );
    Ok(())
}
