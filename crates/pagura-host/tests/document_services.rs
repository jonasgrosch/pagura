#![forbid(unsafe_code)]
//! Document services preserve lazy reads, explicit selection and old protocols.

use pagura::{ErrorKind, Limits};
use pagura_host::protocol::{self, Version};
use pagura_host::{Session, SessionConfig, SessionLimits};
use pagura_testkit::{fixture, frame, split_frames};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

fn source(area: &str, name: &str, password: &[u8]) -> Result<Vec<u8>> {
    let pdf = fixture(area, name);
    let mut payload = vec![0];
    payload.extend(u32::try_from(password.len())?.to_le_bytes());
    payload.extend(password);
    payload.extend(u32::try_from(pdf.len())?.to_le_bytes());
    payload.extend(pdf);
    Ok(frame(2, 2, &payload))
}

fn serve(session: Session, input: &[u8]) -> Result<Vec<Vec<u8>>> {
    let mut output = Vec::new();
    session.serve(input, &mut output)?;
    Ok(split_frames(&output)
        .iter()
        .map(|frame| frame.body().to_vec())
        .collect())
}

fn hello(version: Version) -> Vec<u8> {
    frame(1, 1, &version.code().to_le_bytes())
}

#[test]
fn service_requests_are_additive_bounded_and_canonical() -> Result<()> {
    let payloads = [
        (22, vec![1, 0, 0, 0]),
        (23, vec![1, 0, 0, 0]),
        (24, vec![1, 0, 0, 0]),
        (25, vec![1, 0, 0, 0, 0, 0, 0, 0, 0]),
    ];
    for (kind, payload) in &payloads {
        let bytes = frame(*kind, 3, payload);
        let body = &bytes[4..];
        assert!(protocol::decode(body, &Limits::default()).is_ok());
        for n in 0..body.len() {
            assert!(protocol::decode(&body[..n], &Limits::default()).is_err());
        }
        let mut extra = body.to_vec();
        extra.push(0);
        assert_eq!(
            protocol::decode(&extra, &Limits::default())
                .unwrap_err()
                .kind,
            ErrorKind::Syntax
        );
    }
    for key in 5..=u8::MAX {
        let bytes = frame(25, 3, &[1, 0, 0, 0, 0, 0, 0, 0, key]);
        assert_eq!(
            protocol::decode(&bytes[4..], &Limits::default())
                .unwrap_err()
                .kind,
            ErrorKind::Syntax
        );
    }
    for &version in Version::ALL
        .iter()
        .filter(|&&version| version < Version::V1_8)
    {
        let mut input = hello(version);
        input.extend(source("files", "populated.pdf", b"")?);
        for (i, (kind, payload)) in payloads.iter().enumerate() {
            input.extend(frame(*kind, u32::try_from(i + 3)?, payload));
        }
        input.extend(frame(3, 7, &[1, 0, 0, 0, 0, 0, 0, 0]));
        let bodies = serve(Session::default(), &input)?;
        for body in &bodies[2..6] {
            assert_eq!((body[0], body[5]), (255, 7));
        }
        assert_eq!(bodies[6][0], 131, "old sessions remain usable");
    }
    Ok(())
}

#[test]
fn absent_and_empty_metadata_are_distinct_literal_payloads() -> Result<()> {
    for (name, info, packet) in [
        ("absent.pdf", vec![0, 0, 0], vec![0]),
        ("null.pdf", vec![0, 0, 0], vec![0]),
        (
            "empty.pdf",
            [
                vec![1, 1, 5, 0, 0, 0, 0, 0],
                vec![0; 8],
                vec![7, 0, 0, 0],
                b"Unknown".to_vec(),
                vec![0, 1, 6, 0, 0, 0, 0, 0],
            ]
            .concat(),
            vec![1, 0, 0, 0, 0],
        ),
    ] {
        let mut input = hello(Version::V1_8);
        input.extend(source("metadata", name, b"")?);
        input.extend(frame(22, 3, &[1, 0, 0, 0]));
        input.extend(frame(24, 4, &[1, 0, 0, 0]));
        let bodies = serve(Session::default(), &input)?;
        assert_eq!(bodies[2][0], 150);
        assert_eq!(&bodies[2][5..], info);
        assert_eq!(bodies[3][0], 152);
        assert_eq!(&bodies[3][5..], packet);
    }
    Ok(())
}

#[test]
fn payloads_are_exact_and_failed_selection_keeps_the_session_alive() -> Result<()> {
    for name in [
        "populated.pdf",
        "flate.pdf",
        "stream-type-absent.pdf",
        "empty-payload.pdf",
    ] {
        let mut input = hello(Version::V1_8);
        input.extend(source("files", name, b"")?);
        input.extend(frame(25, 3, &[1, 0, 0, 0, 1, 0, 0, 0, 0])); // wrong index
        input.extend(frame(25, 4, &[1, 0, 0, 0, 0, 0, 0, 0, 4])); // absent Unix key
        input.extend(frame(25, 5, &[1, 0, 0, 0, 0, 0, 0, 0, 1])); // exact UF
        input.extend(frame(6, 6, &[1, 0, 0, 0]));
        input.extend(frame(22, 7, &[1, 0, 0, 0]));
        let bodies = serve(Session::default(), &input)?;
        for i in [2, 3, 6] {
            assert_eq!((bodies[i][0], bodies[i][5]), (255, 7));
        }
        let data: &[u8] = if name == "empty-payload.pdf" {
            b""
        } else {
            b"attachment\0bytes\xff\n"
        };
        let mut expected = u32::try_from(data.len())?.to_le_bytes().to_vec();
        expected.extend(data);
        assert_eq!(bodies[4][0], 153);
        assert_eq!(&bodies[4][5..], expected);
    }
    Ok(())
}

#[test]
fn attachment_authorization_is_lazy_and_explicit_identity_stays_public() -> Result<()> {
    for password in [b"".as_slice(), b"wrong", b"attachment", b"owner"] {
        for name in [
            "encrypted-no-type.pdf",
            "encrypted-explicit-no-type.pdf",
            "encrypted-identity-no-type.pdf",
        ] {
            let mut input = hello(Version::V1_8);
            input.extend(source("files", name, password)?);
            input.extend(frame(23, 3, &[1, 0, 0, 0]));
            input.extend(frame(25, 4, &[1, 0, 0, 0, 0, 0, 0, 0, 0]));
            input.extend(frame(3, 5, &[1, 0, 0, 0, 0, 0, 0, 0]));
            let bodies = serve(Session::default(), &input)?;
            assert_eq!(bodies[2][0], 151, "listing never decrypts payloads");
            if password == b"attachment"
                || password == b"owner"
                || name == "encrypted-identity-no-type.pdf"
            {
                assert_eq!(bodies[3][0], 153);
            } else {
                assert_eq!((bodies[3][0], bodies[3][5]), (255, 6));
            }
            assert_eq!(bodies[4][0], 131);
        }
    }
    Ok(())
}

#[test]
fn every_service_obeys_the_reading_response_budget() -> Result<()> {
    for (area, name, requests) in [
        (
            "metadata",
            "populated-flate.pdf",
            vec![(22, vec![1, 0, 0, 0]), (24, vec![1, 0, 0, 0])],
        ),
        (
            "files",
            "populated.pdf",
            vec![
                (23, vec![1, 0, 0, 0]),
                (25, vec![1, 0, 0, 0, 0, 0, 0, 0, 0]),
            ],
        ),
    ] {
        let mut input = hello(Version::V1_8);
        input.extend(source(area, name, b"")?);
        for (i, (kind, payload)) in requests.iter().enumerate() {
            input.extend(frame(*kind, u32::try_from(i + 3)?, payload));
        }
        let session = Session::new(SessionConfig::default().with_session(SessionLimits {
            reading_response_bytes: 8,
            ..SessionLimits::default()
        }));
        let bodies = serve(session, &input)?;
        for body in &bodies[2..] {
            assert_eq!((body[0], body[5]), (255, 2));
        }
    }
    Ok(())
}

#[test]
fn catalog_record_limit_is_exact_and_does_not_limit_page_access() -> Result<()> {
    for (name, count) in [("catalog-1024.pdf", 1024_u32), ("catalog-1025.pdf", 1025)] {
        let mut input = hello(Version::V1_8);
        input.extend(source("files", name, b"")?);
        input.extend(frame(23, 3, &[1, 0, 0, 0]));
        input.extend(frame(3, 4, &[1, 0, 0, 0, 0, 0, 0, 0]));
        let bodies = serve(Session::default(), &input)?;
        if count == 1024 {
            assert_eq!(bodies[2][0], 151);
            assert_eq!(&bodies[2][5..9], count.to_le_bytes());
        } else {
            assert_eq!((bodies[2][0], bodies[2][5]), (255, 2));
        }
        assert_eq!(bodies[3][0], 131);
    }
    Ok(())
}
