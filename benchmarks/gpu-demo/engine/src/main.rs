#![forbid(unsafe_code)]
//! Experimental native/WASI host: framed stdin requests, metadata and pixels
//! or an exact GPU program on stdout. No GPU API enters the Rust engine.

use pagura_core::{Error, ErrorKind, Limits, Result, Store};
use pagura_data::Bundled;
use pagura_document::{Document, OpenOptions};
use pagura_interpret::{Interpreter, Scene};
use pagura_paint::{OverprintMode, RenderRequest, Scratch};
use pagura_raster::DeviceRect;
use std::borrow::Cow;
use std::io::{self, Read, Write};
use std::sync::Arc;
use std::time::Instant;

const MAX_FRAME: usize = 64 * 1024 * 1024;

mod svg;

struct Session {
    document: Option<Document>,
    scene: Option<(usize, Scene)>,
    svg: Option<svg::Export>,
    store: Store,
    scratch: Scratch,
    provider: Bundled,
}

impl Session {
    fn new() -> Self {
        Self {
            document: None,
            scene: None,
            svg: None,
            store: Store::new(MAX_FRAME),
            scratch: Scratch::default(),
            provider: Bundled::new(Self::limits()),
        }
    }

    fn limits() -> Limits {
        Limits {
            file_bytes: MAX_FRAME - 4,
            pixels: 16 * 1024 * 1024,
            render_bytes: 128 * 1024 * 1024,
            ..Limits::default()
        }
    }

    fn request(&mut self, bytes: &[u8]) -> Result<(String, Cow<'_, [u8]>)> {
        let operation = word(bytes, 0)?;
        if operation == 1 {
            self.scene = None;
            self.svg = None;
            self.document = None;
            self.store = Store::new(MAX_FRAME);
            self.scratch = Scratch::default();
            self.provider = Bundled::new(Self::limits());
            let start = Instant::now();
            let document = Document::open(
                Arc::<[u8]>::from(bytes.get(4..).unwrap_or_default()),
                &OpenOptions {
                    limits: Self::limits(),
                    ..OpenOptions::default()
                },
            )?;
            let pages = document.pages()?.len();
            self.document = Some(document);
            return Ok((
                format!(
                    "{{\"ok\":true,\"pages\":{pages},\"open_ms\":{}}}",
                    ms(start)
                ),
                Cow::Owned(Vec::new()),
            ));
        }
        if !matches!(operation, 2..=4) || bytes.len() != 32 {
            return Err(Error::invalid_argument("invalid demo request"));
        }
        let page_index =
            usize::try_from(word(bytes, 1)?).map_err(|_| Error::invalid_argument("page index"))?;
        let dpi = word(bytes, 2)?;
        let band = word(bytes, 3)?;
        if !(1..=600).contains(&dpi) || !(1..=256).contains(&band) {
            return Err(Error::invalid_argument(
                "demo dpi or band height out of range",
            ));
        }
        let total = Instant::now();
        let document = self
            .document
            .as_ref()
            .ok_or(Error::invalid_argument("open a document first"))?;
        let record = Instant::now();
        if self
            .scene
            .as_ref()
            .is_none_or(|(index, _)| *index != page_index)
        {
            self.svg = None;
            let scene = Interpreter::new(document)
                .with_provider(Some(&self.provider))
                .with_store(&self.store)
                .scene(document.page(page_index)?)?;
            self.scene = Some((page_index, scene));
        }
        let record_ms = ms(record);
        let scene = &self
            .scene
            .as_ref()
            .ok_or(Error::invalid_argument("no recorded scene"))?
            .1;
        let scale = f64::from(dpi) / 72.0;
        let size = scene.page.device_size(scale)?;
        let page = DeviceRect {
            x0: 0,
            y0: 0,
            x1: size.0,
            y1: size.1,
        };
        let region = if word(bytes, 6)? == 0 && word(bytes, 7)? == 0 {
            page
        } else {
            DeviceRect {
                x0: word(bytes, 4)?,
                y0: word(bytes, 5)?,
                x1: word(bytes, 6)?,
                y1: word(bytes, 7)?,
            }
        };
        if operation == 4 {
            if region != page {
                return Err(Error::invalid_argument("SVG demo exports whole pages"));
            }
            if u64::from(size.0) * u64::from(size.1) > document.limits().pixels as u64 {
                return Err(Error::limit("SVG output exceeds pixel budget"));
            }
            let start = Instant::now();
            let cache_hit = self.svg.is_some();
            if self.svg.is_none() {
                match svg::export(scene, document) {
                    Ok(export) => self.svg = Some(export),
                    Err(svg::Failure::Unsupported(reason)) => {
                        return Ok((
                            format!("{{\"ok\":true,\"fallback\":{}}}", json_string(reason)),
                            Cow::Owned(Vec::new()),
                        ));
                    }
                    Err(svg::Failure::Engine(error)) if error.kind == ErrorKind::Limit => {
                        return Ok((
                            format!(
                                "{{\"ok\":true,\"fallback\":{}}}",
                                json_string(
                                    "SVG export exceeds its memory, element or work budget"
                                )
                            ),
                            Cow::Owned(Vec::new()),
                        ));
                    }
                    Err(svg::Failure::Engine(error)) => return Err(error),
                }
            }
            let svg = self
                .svg
                .as_ref()
                .ok_or(Error::invalid_argument("no SVG export"))?;
            return Ok((
                format!(
                    "{{\"ok\":true,\"width\":{},\"height\":{},\"record_ms\":{record_ms},\"export_ms\":{},\"engine_ms\":{},\"cache_hit\":{cache_hit},\"paths\":{},\"glyphs\":{},\"images\":{},\"clips_elided\":{},\"clips_retained\":{}}}",
                    page.width(),
                    page.height(),
                    ms(start),
                    ms(total),
                    svg.paths,
                    svg.glyphs,
                    svg.images,
                    svg.clips_elided,
                    svg.clips_retained
                ),
                Cow::Borrowed(&svg.bytes),
            ));
        }

        let mut request = RenderRequest::new(scene.page.device_transform(scale), page, region);
        request.band_height = band;
        request.overprint = if scene.print_intent {
            OverprintMode::Simulate
        } else {
            OverprintMode::Ignore
        };
        let limits = document.limits();
        let start = Instant::now();
        let prepared = self
            .scratch
            .prepare(scene, &request, (&self.store, &limits))?;
        for repair in prepared.repairs() {
            document.repair(repair.clone())?;
        }
        let prepare_ms = ms(start);
        if operation == 3
            && let Some(reason) = prepared.gpu_fallback_reason()
        {
            self.scratch.recycle(prepared);
            return Ok((
                format!(
                    "{{\"ok\":true,\"fallback\":{},\"prepare_ms\":{prepare_ms}}}",
                    json_string(reason)
                ),
                Cow::Owned(Vec::new()),
            ));
        }
        let start = Instant::now();
        let mut data = Vec::new();
        if operation == 2 {
            let count = u64::from(region.width()) * u64::from(region.height()) * 4;
            let count = usize::try_from(count)
                .ok()
                .filter(|&count| count <= MAX_FRAME)
                .ok_or(Error::limit("demo output exceeds 64 MiB"))?;
            data.try_reserve_exact(count)
                .map_err(|_| Error::limit("demo output allocation failed"))?;
            prepared.with_scratch(&mut self.scratch)?.render(&mut data);
        } else {
            // Export has its own additional budget, including temporary bin
            // and packet storage; it does not change CPU preparation charges.
            let export_limits = Limits {
                render_bytes: MAX_FRAME,
                ..limits
            };
            let program = match prepared.gpu_program(&mut self.scratch, &export_limits) {
                Ok(program) => program,
                Err(error) if error.kind == ErrorKind::Limit => {
                    self.scratch.recycle(prepared);
                    return Ok((
                        format!(
                            "{{\"ok\":true,\"fallback\":{},\"prepare_ms\":{prepare_ms}}}",
                            json_string("GPU packet exceeds its export memory or work budget")
                        ),
                        Cow::Owned(Vec::new()),
                    ));
                }
                Err(error) => return Err(error),
            };
            data.try_reserve_exact(program.words().len() * 4)
                .map_err(|_| Error::limit("GPU wire allocation failed"))?;
            for word in program.words() {
                data.extend_from_slice(&word.to_le_bytes());
            }
        }
        let execute_ms = ms(start);
        let engine_ms = ms(total);
        self.scratch.recycle(prepared);
        let meta = format!(
            "{{\"ok\":true,\"width\":{},\"height\":{},\"record_ms\":{record_ms},\"prepare_ms\":{prepare_ms},\"execute_ms\":{execute_ms},\"engine_ms\":{engine_ms}}}",
            region.width(),
            region.height()
        );
        Ok((meta, Cow::Owned(data)))
    }
}

fn word(bytes: &[u8], index: usize) -> Result<u32> {
    let start = index
        .checked_mul(4)
        .ok_or(Error::invalid_argument("demo request overflow"))?;
    let raw: [u8; 4] = bytes
        .get(start..start + 4)
        .and_then(|raw| raw.try_into().ok())
        .ok_or(Error::invalid_argument("truncated demo request"))?;
    Ok(u32::from_le_bytes(raw))
}

fn ms(start: Instant) -> f64 {
    start.elapsed().as_secs_f64() * 1000.0
}

fn json_string(value: &str) -> String {
    let mut out = String::from("\"");
    for ch in value.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            ch if ch.is_control() => out.push_str(&format!("\\u{:04x}", u32::from(ch))),
            ch => out.push(ch),
        }
    }
    out.push('"');
    out
}

fn main() -> io::Result<()> {
    let mut session = Session::new();
    let mut input = io::stdin().lock();
    let mut output = io::stdout().lock();
    loop {
        let mut header = [0; 4];
        if let Err(error) = input.read_exact(&mut header) {
            if error.kind() == io::ErrorKind::UnexpectedEof {
                return Ok(());
            }
            return Err(error);
        }
        let length = u32::from_le_bytes(header) as usize;
        if !(4..=MAX_FRAME).contains(&length) {
            return Err(io::Error::other("invalid frame size"));
        }
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(length)
            .map_err(|_| io::Error::other("input allocation failed"))?;
        bytes.resize(length, 0);
        input.read_exact(&mut bytes)?;
        let (meta, data) = match session.request(&bytes) {
            Ok(result) => result,
            Err(error) => (
                format!(
                    "{{\"ok\":false,\"error\":{}}}",
                    json_string(&error.to_string())
                ),
                Cow::Owned(Vec::new()),
            ),
        };
        let length = u32::try_from(4 + meta.len() + data.len()).map_err(io::Error::other)?;
        output.write_all(&length.to_le_bytes())?;
        output.write_all(
            &u32::try_from(meta.len())
                .map_err(io::Error::other)?
                .to_le_bytes(),
        )?;
        output.write_all(meta.as_bytes())?;
        output.write_all(&data)?;
        output.flush()?;
    }
}
