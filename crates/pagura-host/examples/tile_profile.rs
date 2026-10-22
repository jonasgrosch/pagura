#![forbid(unsafe_code)]
//! Profiles tile serving through a session, as the viewer drives it: the
//! same 64 tiles of 64x64 pixels with the render store disabled, so that
//! every tile records its page and decodes its images, and with the default
//! store, so that one recording is followed by painting and encoding only.
//! Both timings include the document open. They are exploratory hypotheses, not
//! measurements under the performance policy in docs/ARCHITECTURE.md.

use pagura_host::protocol::{RequestKind, ResponseKind, Version};
use pagura_host::{Session, SessionConfig, SessionLimits};
use pagura_testkit::{frame, split_frames};
use std::time::Instant;

const TILES: u32 = 64;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args().nth(1).ok_or("expected a PDF path")?;
    let pdf = std::fs::read(path)?;
    let mut input = frame(
        RequestKind::Hello.code(),
        1,
        &Version::V1.code().to_le_bytes(),
    );
    let mut open = vec![0; 5];
    open.extend(u32::try_from(pdf.len())?.to_le_bytes());
    open.extend(pdf);
    input.extend(frame(RequestKind::Open.code(), 2, &open));
    for index in 0..TILES {
        let (x, y) = (index % 8 * 64, index / 8 * 64);
        let mut tile = [1_u32, 0].map(u32::to_le_bytes).concat();
        tile.extend(1_f64.to_le_bytes());
        tile.extend([x, y, 64, 64].map(u32::to_le_bytes).concat());
        input.extend(frame(RequestKind::RenderTile.code(), index + 3, &tile));
    }
    let uncached = SessionConfig::default().with_session(SessionLimits {
        store_bytes: 0,
        ..SessionLimits::default()
    });
    let uncached_seconds = serve(Session::new(uncached), &input)?;
    let cached_seconds = serve(Session::default(), &input)?;
    println!(
        "{{\"tiles\":{TILES},\"uncached_seconds\":{uncached_seconds},\"cached_seconds\":{cached_seconds}}}"
    );
    Ok(())
}

/// Seconds to serve `input`, which must produce only tiles after the open.
fn serve(session: Session, input: &[u8]) -> Result<f64, Box<dyn std::error::Error>> {
    let mut output = Vec::new();
    let start = Instant::now();
    session.serve(input, &mut output)?;
    let seconds = start.elapsed().as_secs_f64();
    let tile = ResponseKind::Tile.code();
    if let Some(failed) = split_frames(&output)
        .iter()
        .skip(2)
        .find(|f| f.kind() != tile)
    {
        return Err(format!("request {} failed: {:?}", failed.id(), failed.payload()).into());
    }
    Ok(seconds)
}
