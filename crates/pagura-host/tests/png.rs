#![forbid(unsafe_code)]
//! PNG wire, preflight and budget contracts. Python independently decodes pixels.

use pagura::{ErrorKind, Limits};
use pagura_host::protocol::{self, PngBackground, Request, Response, Version};
use pagura_host::{Session, SessionConfig, SessionLimits};
use pagura_testkit::{fixture, frame, split_frames};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

fn payload(document: u32, page: u32, scale: f64, background: u8) -> Vec<u8> {
    let mut bytes = document.to_le_bytes().to_vec();
    bytes.extend(page.to_le_bytes());
    bytes.extend(scale.to_le_bytes());
    bytes.push(background);
    bytes
}

fn source(version: Version) -> Result<Vec<u8>> {
    let mut input = frame(1, 1, &version.code().to_le_bytes());
    let pdf = fixture("classic", "blank.pdf");
    let mut open = vec![0; 5]; // Strict open, empty password.
    open.extend(u32::try_from(pdf.len())?.to_le_bytes());
    open.extend(pdf);
    input.extend(frame(2, 2, &open));
    Ok(input)
}

fn serve(session: Session, input: &[u8]) -> Result<Vec<Vec<u8>>> {
    let mut output = Vec::new();
    session.serve(input, &mut output)?;
    Ok(split_frames(&output)
        .iter()
        .map(|frame| frame.body().to_vec())
        .collect())
}

#[test]
fn png_requests_have_a_fixed_wire_layout_and_canonical_background() -> Result<()> {
    let bytes = frame(26, 3, &payload(1, 7, 2.0, 1));
    let decoded = protocol::decode(&bytes[4..], &Limits::default())?;
    let Request::PagePng(request) = decoded.request else {
        panic!("wrong request variant");
    };
    assert_eq!(request.page.document.get(), 1);
    assert_eq!(request.page.page.get(), 7);
    assert_eq!(request.scale.get(), 2.0);
    assert_eq!(request.background.get(), PngBackground::Transparent);
    for end in 4..bytes.len() {
        assert!(protocol::decode(&bytes[4..end], &Limits::default()).is_err());
    }
    let mut extra = bytes[4..].to_vec();
    extra.push(0);
    assert_eq!(
        protocol::decode(&extra, &Limits::default())
            .unwrap_err()
            .kind,
        ErrorKind::Syntax
    );
    for background in 2..=u8::MAX {
        let bytes = frame(26, 3, &payload(1, 0, 1.0, background));
        assert_eq!(
            protocol::decode(&bytes[4..], &Limits::default())
                .unwrap_err()
                .kind,
            ErrorKind::Syntax
        );
    }
    let encoded = protocol::encode(3, &Response::PagePng(vec![0x89, b'P', b'N', b'G']), 64)?;
    assert_eq!(
        encoded,
        [
            13, 0, 0, 0, 154, 3, 0, 0, 0, 4, 0, 0, 0, 0x89, b'P', b'N', b'G'
        ]
    );
    Ok(())
}

#[test]
fn png_export_preserves_older_sessions_and_refuses_invalid_arguments() -> Result<()> {
    for &version in Version::ALL
        .iter()
        .filter(|&&version| version < Version::V1_9)
    {
        let mut input = source(version)?;
        input.extend(frame(26, 3, &payload(1, 0, 1.0, 0)));
        input.extend(frame(3, 4, &[1, 0, 0, 0, 0, 0, 0, 0]));
        let bodies = serve(Session::default(), &input)?;
        assert_eq!((bodies[2][0], bodies[2][5]), (255, 7));
        assert_eq!(bodies[3][0], 131, "old sessions remain usable");
    }
    for scale in [0.0, -1.0, f64::NAN, f64::INFINITY] {
        let mut input = source(Version::V1_9)?;
        input.extend(frame(26, 3, &payload(1, 0, scale, 0)));
        input.extend(frame(26, 4, &payload(1, 0, 1.0, 0)));
        let bodies = serve(Session::default(), &input)?;
        assert_eq!((bodies[2][0], bodies[2][5]), (255, 7));
        assert_eq!(bodies[3][0], 154, "bad scale does not poison export");
        assert_eq!(&bodies[3][9..17], b"\x89PNG\r\n\x1a\n");
    }
    for request in [payload(1, 99, 1.0, 0), payload(999, 0, 1.0, 0)] {
        let mut input = source(Version::V1_9)?;
        input.extend(frame(26, 3, &request));
        let bodies = serve(Session::default(), &input)?;
        assert_eq!(bodies[2][0], 255, "invalid page/handle has no PNG response");
    }
    Ok(())
}

#[test]
fn png_surface_stream_and_frame_budgets_remain_independent() -> Result<()> {
    for limits in [
        Limits {
            pixels: 1,
            ..Limits::default()
        },
        Limits {
            decoded_bytes: 4,
            ..Limits::default()
        },
        Limits {
            stream_bytes: 64,
            ..Limits::default()
        },
    ] {
        let mut input = source(Version::V1_9)?;
        input.extend(frame(26, 3, &payload(1, 0, 1.0, 0)));
        input.extend(frame(3, 4, &[1, 0, 0, 0, 0, 0, 0, 0]));
        let bodies = serve(Session::new(limits), &input)?;
        assert_eq!(bodies[1][0], 130, "budget control must open its input");
        assert_eq!((bodies[2][0], bodies[2][5]), (255, 2));
        assert_eq!(bodies[3][0], 131, "budget refusal retains the document");
    }
    let config = SessionConfig::default().with_session(SessionLimits {
        max_frame_bytes: 1024,
        reading_response_bytes: 0,
        ..SessionLimits::default()
    });
    let mut input = source(Version::V1_9)?;
    input.extend(frame(26, 3, &payload(1, 0, 16.0, 0)));
    input.extend(frame(26, 4, &payload(1, 0, 1.0, 0)));
    let bodies = serve(Session::new(config), &input)?;
    assert_eq!((bodies[2][0], bodies[2][5]), (255, 2));
    assert_eq!(
        bodies[3][0], 154,
        "PNG does not borrow the reading-response budget"
    );
    assert!(bodies.iter().all(|body| body.len() <= 1024));
    assert_eq!(
        protocol::encode(3, &Response::PagePng(vec![0; 1024]), 1024)
            .unwrap_err()
            .kind,
        ErrorKind::Limit
    );
    Ok(())
}
