#![forbid(unsafe_code)]
//! Literal wire controls for immutable sources and owned, acknowledged output.

mod lifecycle;
mod wire;

use pagura::{Error, Limits, OutputLimits, png};
use pagura_host::{ProgressiveLimits, Session, SessionConfig, SessionLimits, protocol};
use pagura_testkit::frame;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
const VERSION: [u8; 32] = [27; 32];
const SOURCE_BYTES: usize = 65536;

fn config() -> SessionConfig {
    const MIB: usize = 1024 * 1024;
    let engine = Limits {
        string_bytes: MIB,
        document_bytes: 4 * MIB,
        stream_bytes: 4 * MIB,
        decoded_bytes: 4 * MIB,
        render_bytes: 16 * MIB,
        decode_bytes: 16 * MIB,
        ..Limits::default()
    };
    SessionConfig::new(engine)
        .with_session(SessionLimits {
            store_bytes: MIB,
            total_source_bytes: 2 * SOURCE_BYTES,
            ..SessionLimits::default()
        })
        .with_progressive(ProgressiveLimits {
            active_bytes: 192 * MIB,
            source: pagura::source::SourceLimits {
                retained_bytes: SOURCE_BYTES,
                ..Default::default()
            },
            output: OutputLimits {
                slots: 1,
                bytes: MIB,
            },
            encoding: png::EncodingLimits {
                retained_bytes: 4 * MIB,
                output_slots: 1,
                ..Default::default()
            },
            ..ProgressiveLimits::default()
        })
}

struct Host {
    session: Session,
    id: u32,
}

impl Host {
    fn new(config: SessionConfig) -> Result<Self> {
        let mut host = Self {
            session: Session::new(config),
            id: 0,
        };
        host.call(1, &0x0002_0000_u32.to_le_bytes())?;
        Ok(host)
    }
    fn call(&mut self, kind: u8, payload: &[u8]) -> Result<Vec<u8>> {
        self.id += 1;
        let input = frame(kind, self.id, payload);
        let request = protocol::decode(&input[4..], &Limits::default())?;
        let response = self.session.handle(request)?;
        Ok(protocol::encode(self.id, &response, self.session.max_frame_bytes())?[4..].to_vec())
    }
    fn supply(&mut self, offset: u64, bytes: &[u8]) -> Result<Vec<u8>> {
        let mut payload = words(&[1]);
        payload.extend(VERSION);
        payload.extend(offset.to_le_bytes());
        payload.extend(u32::try_from(bytes.len())?.to_le_bytes());
        payload.extend(bytes);
        self.call(42, &payload)
    }
    fn event(&mut self, job: u32) -> Result<Vec<u8>> {
        for _ in 0..64 {
            let body = self.call(48, &words(&[job]))?;
            if body.first() != Some(&174) || body.get(9) != Some(&0) {
                return Ok(body);
            }
        }
        Err(Error::limit("test job step bound exceeded").into())
    }
}

fn words(values: &[u32]) -> Vec<u8> {
    values
        .iter()
        .flat_map(|value| value.to_le_bytes())
        .collect()
}
fn word(bytes: &[u8], offset: usize) -> Result<u32> {
    Ok(u32::from_le_bytes(
        bytes
            .get(offset..offset + 4)
            .ok_or(Error::syntax("missing test word"))?
            .try_into()?,
    ))
}
fn wide(bytes: &[u8], offset: usize) -> Result<u64> {
    Ok(u64::from_le_bytes(
        bytes
            .get(offset..offset + 8)
            .ok_or(Error::syntax("missing test wide word"))?
            .try_into()?,
    ))
}
fn creation(length: u64) -> Vec<u8> {
    [length.to_le_bytes().as_slice(), &VERSION].concat()
}
fn rendering(output: u8) -> Vec<u8> {
    [
        words(&[1, 0]),
        1_f64.to_le_bytes().to_vec(),
        vec![0, output, 0],
    ]
    .concat()
}
fn ack(job: u32, sequence: u64) -> Vec<u8> {
    [words(&[job]), sequence.to_le_bytes().to_vec()].concat()
}

/// Independently authored classic index. Object 4 and unrelated object 5 are
/// deliberately absent; no writer, engine or production parser builds the PDF.
fn sparse() -> Result<Host> {
    let mut host = Host::new(config())?;
    host.call(41, &creation(8192))?;
    for (offset, part) in [
        (0, b"%PDF-1.7\n".as_slice()),
        (100, b"1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj\n".as_slice()),
        (600, b"2 0 obj << /Type /Pages /Count 1 /Kids [3 0 R] /MediaBox [0 0 8 8] >> endobj\n".as_slice()),
        (900, b"3 0 obj << /Type /Page /Parent 2 0 R /Contents 4 0 R >> endobj\n".as_slice()),
        (6500, b"xref\n0 6\n0 65535 f\n100 0 n\n600 0 n\n900 0 n\n1500 0 n\n4000 0 n\ntrailer << /Size 6 /Root 1 0 R >>\n".as_slice()),
    ] { host.supply(offset, part)?; }
    let mut tail = vec![b' '; 1024];
    tail[932..953].copy_from_slice(b"startxref\n6500\n%%EOF\n");
    host.supply(7168, &tail)?;
    Ok(host)
}

#[test]
fn sparse_input_to_exact_pages_bands_and_png_obeys_acknowledgements() -> Result<()> {
    let mut host = sparse()?;
    assert_eq!(
        &host.call(45, &[1, 0, 0, 0, 1, 0, 0, 0, 0, 0])?[5..],
        words(&[1])
    );
    let opened = host.event(1)?;
    assert_eq!(opened[0], 174);
    assert_eq!(
        &opened[5..],
        [words(&[1]), vec![1], words(&[1]), vec![1]].concat()
    );
    assert_eq!(&host.call(46, &words(&[1, 0]))?[5..], words(&[2]));
    let page = host.event(2)?;
    let mut geometry = [
        words(&[2]),
        vec![2],
        words(&[0]),
        vec![1],
        words(&[1]),
        vec![0],
    ]
    .concat();
    geometry.extend([0_f64, 0., 8., 8.].into_iter().flat_map(f64::to_le_bytes));
    geometry.extend(0_u16.to_le_bytes());
    geometry.extend(1_f64.to_le_bytes());
    geometry.extend(words(&[8, 8]));
    assert_eq!(&page[5..], geometry);
    assert_eq!(&host.call(47, &rendering(0))?[5..], words(&[3]));
    let waiting = host.event(3)?;
    assert_eq!(waiting[0], 179);
    assert_eq!(wide(&waiting, 5)?, 1);
    assert_eq!(&waiting[13..45], VERSION);
    assert_eq!((wide(&waiting, 45)?, word(&waiting, 53)?), (6, 1));
    assert_eq!((wide(&waiting, 57)?, wide(&waiting, 65)?), (1500, 1756));
    for _ in 0..3 {
        assert_eq!(&host.event(3)?[5..], &waiting[5..]);
    }
    host.supply(1500, b"4 0 obj << /Length 13 >> stream\n")?;
    host.supply(1545, b"\nendstream\nendobj\n")?;
    let waiting = host.event(3)?;
    assert_eq!(waiting[0], 179);
    assert_eq!((wide(&waiting, 57)?, wide(&waiting, 65)?), (1532, 1545));
    host.supply(1532, b"0 0 8 8 re f\n")?;
    let prepared = host.event(3)?;
    assert_eq!(
        &prepared[5..],
        [words(&[3]), vec![3, 0], words(&[0, 0, 8, 8, 0, 0, 8, 8])].concat()
    );
    assert_eq!(&host.call(47, &rendering(1))?[5..], words(&[4]));
    assert_eq!(host.event(4)?[9], 3);
    // Prepared producers no longer read source bytes. Both survive input and
    // document closure; retained domains remain admitted until their jobs end.
    host.call(44, &words(&[1]))?;
    host.call(6, &words(&[1]))?;
    let band = host.event(3)?;
    assert_eq!((band[0], band[9]), (174, 4));
    assert_eq!(wide(&band, 10)?, 1);
    assert_eq!(
        &band[18..66],
        words(&[0, 0, 8, 8, 0, 0, 8, 8, 0, 8, 32, 256])
    );
    let pixels = [0, 0, 0, 255].repeat(64);
    assert_eq!(&band[66..], pixels);
    assert_eq!(host.event(3)?[9], 8);
    assert_eq!(host.call(49, &ack(3, 2))?[0], 255);
    assert_eq!(host.event(3)?[9], 8);
    assert_eq!(host.call(49, &ack(3, 1))?[0], 175);
    assert_eq!(host.call(49, &ack(3, 1))?[0], 255);
    assert_eq!(host.event(3)?[9], 6);
    let mut encoded = Vec::new();
    let mut sequence = 0;
    let mut complete = false;
    for _ in 0..16 {
        let chunk = host.event(4)?;
        assert_eq!(chunk[0], 174);
        if chunk[9] == 6 {
            complete = true;
            break;
        }
        assert_eq!(chunk[9], 5);
        sequence += 1;
        assert_eq!(wide(&chunk, 10)?, sequence);
        assert_eq!(
            pagura::convert::u32_to_usize(word(&chunk, 18)?),
            chunk.len() - 22
        );
        encoded.extend_from_slice(&chunk[22..]);
        assert_eq!(host.event(4)?[9], 8);
        host.call(49, &ack(4, sequence))?;
    }
    assert!(complete);
    assert_eq!(
        encoded,
        png::encode(&pixels, 8, 8, &png::Options::default())?
    );
    assert_eq!(&encoded[16..29], &[0, 0, 0, 8, 0, 0, 0, 8, 8, 6, 0, 0, 0]);
    let stats = host.call(52, &[])?;
    assert_eq!((wide(&stats, 13)?, wide(&stats, 21)?), (0, 0));
    assert_eq!(host.call(50, &words(&[4]))?[9], 0);
    Ok(())
}
