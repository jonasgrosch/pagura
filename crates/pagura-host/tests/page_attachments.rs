#![forbid(unsafe_code)]
//! Independent page attachment identity, wire, scope, authentication and budget controls.

use pagura::{ErrorKind, Limits};
use pagura_host::protocol::{self, Version};
use pagura_host::{Session, SessionConfig, SessionLimits};
use pagura_testkit::{fixture, frame, split_frames};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

fn input(version: Version, name: &str, password: &[u8]) -> Result<Vec<u8>> {
    let pdf = fixture("file_annotations", &format!("{name}.pdf"));
    let mut open = vec![1];
    open.extend(u32::try_from(password.len())?.to_le_bytes());
    open.extend(password);
    open.extend(u32::try_from(pdf.len())?.to_le_bytes());
    open.extend(pdf);
    let mut input = frame(1, 1, &version.code().to_le_bytes());
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

fn request(kind: u8, id: u32, page: u8, index: u8, key: u8) -> Vec<u8> {
    let mut body = vec![1, 0, 0, 0, page, 0, 0, 0];
    if kind != 28 {
        body.extend([index, 0, 0, 0, key]);
    }
    frame(kind, id, &body)
}

#[test]
fn canonical_requests_and_old_sessions_are_independent_of_catalog_operations() -> Result<()> {
    for kind in [28, 29, 30] {
        let bytes = request(kind, 3, 0, 1, 1);
        assert_eq!(
            protocol::decode(&bytes[4..], &Limits::default())?
                .request
                .kind()
                .code(),
            kind
        );
        for end in 4..bytes.len() {
            assert!(protocol::decode(&bytes[4..end], &Limits::default()).is_err());
        }
        let mut extra = bytes[4..].to_vec();
        extra.push(0);
        assert!(protocol::decode(&extra, &Limits::default()).is_err());
        for &version in Version::ALL.iter().filter(|&&v| v < Version::V1_11) {
            let mut input = input(version, "mixed", b"")?;
            input.extend(&bytes);
            input.extend(frame(3, 4, &[1, 0, 0, 0, 0, 0, 0, 0]));
            let bodies = serve(Session::default(), &input)?;
            assert_eq!((bodies[2][0], bodies[2][5]), (255, 7));
            assert_eq!(bodies[3][0], 131, "version refusal preserves page access");
        }
    }
    for kind in [29, 30] {
        for key in 5..=u8::MAX {
            assert_eq!(
                protocol::decode(&request(kind, 3, 0, 0, key)[4..], &Limits::default())
                    .unwrap_err()
                    .kind,
                ErrorKind::Syntax
            );
        }
    }
    Ok(())
}

#[test]
fn listing_preserves_annotation_identity_geometry_and_original_slots() -> Result<()> {
    for (name, count, slot, object) in [
        ("populated", 1, 0, 8),
        ("page-only", 1, 0, 8),
        ("mixed", 2, 1, 9),
    ] {
        let mut input = input(Version::V1_11, name, b"")?;
        input.extend(request(28, 3, 0, 0, 0));
        let bodies = serve(Session::default(), &input)?;
        let body = &bodies[2];
        assert_eq!(
            &body[..19],
            &[
                156, 3, 0, 0, 0, count, 0, 0, 0, slot, 0, 0, 0, object, 0, 0, 0, 0, 0
            ]
        );
        for (bytes, expected) in body[19..51]
            .as_chunks::<8>()
            .0
            .iter()
            .zip([1.0, 2.0, 9.0, 12.0])
        {
            assert_eq!(f64::from_le_bytes(*bytes), expected);
        }
        if name == "populated" {
            let mut contents = vec![1, 13, 0, 0, 0];
            contents.extend("Annotation Ω".as_bytes());
            assert_eq!(&body[51..69], contents);
        }
        if name == "page-only" {
            assert_eq!(
                &body[51..53],
                &[0, 0],
                "absent Contents and icon remain absent"
            );
        }
    }
    for name in ["absent", "empty", "missing"] {
        let mut input = input(Version::V1_11, name, b"")?;
        input.extend(request(28, 3, 0, 0, 0));
        assert_eq!(
            serve(Session::default(), &input)?[2],
            [156, 3, 0, 0, 0, 0, 0, 0, 0]
        );
    }
    Ok(())
}

#[test]
fn selection_never_falls_back_to_catalog_and_decodes_only_explicit_payloads() -> Result<()> {
    for (name, slot) in [("page-only", 0), ("mixed", 1), ("mixed", 2), ("flate", 0)] {
        for key in [0, 1] {
            let mut input = input(Version::V1_11, name, b"")?;
            input.extend(request(29, 4, 0, slot, key));
            input.extend(request(30, 5, 0, slot, key));
            let bodies = serve(Session::default(), &input)?;
            let mut expected = vec![157, 4, 0, 0, 0, 18, 0, 0, 0];
            expected.extend(b"attachment\0bytes\xff\n");
            assert_eq!(bodies[2], expected);
            assert_eq!(
                &bodies[3][..19],
                &[158, 5, 0, 0, 0, 6, 0, 0, 0, 0, 0, 18, 0, 0, 0, 0, 0, 0, 0]
            );
            assert_eq!(
                &bodies[3][19..],
                if name == "flate" { &[0, 0] } else { &[1, 1] },
                "flate has no Params; other fixture declarations match complete bytes"
            );
        }
    }
    for (name, page, slot, key) in [
        ("mixed", 0, 0, 0),
        ("mixed", 0, 3, 0),
        ("mixed", 1, 1, 0),
        ("mixed", 0, 1, 4),
        ("external", 0, 0, 0),
    ] {
        for kind in [29, 30] {
            let mut input = input(Version::V1_11, name, b"")?;
            input.extend(request(kind, 3, page, slot, key));
            assert_eq!(
                serve(Session::default(), &input)?[2][0],
                255,
                "invalid selection: {name}"
            );
        }
    }
    Ok(())
}

#[test]
fn malformed_views_limits_and_authorization_never_return_successful_partial_data() -> Result<()> {
    for name in [
        "late-failure",
        "wrong-array",
        "missing-file",
        "reference-cycle",
    ] {
        for kind in [28, 29, 30] {
            let mut input = input(Version::V1_11, name, b"")?;
            input.extend(request(kind, 3, 0, 0, 0));
            assert_eq!(serve(Session::default(), &input)?[2][0], 255, "{name}");
        }
    }
    for (name, kind, cap, decoded) in [
        ("listing-limit", 28, 1_000_000, 1_000_000),
        ("populated", 28, 4, 1_000_000),
        ("populated", 29, 4, 1_000_000),
        ("populated", 30, 15, 1_000_000),
        ("flate", 29, 1024, 4),
        ("flate", 30, 1024, 4),
    ] {
        let config = SessionConfig::new(Limits {
            decoded_bytes: decoded,
            ..Limits::default()
        })
        .with_session(SessionLimits {
            reading_response_bytes: cap,
            ..SessionLimits::default()
        });
        let mut input = input(Version::V1_11, name, b"")?;
        input.extend(request(kind, 3, 0, 0, 0));
        let bodies = serve(Session::new(config), &input)?;
        assert_eq!(bodies[1][0], 130, "control opens before operation: {name}");
        assert_eq!((bodies[2][0], bodies[2][5]), (255, 2), "{name}/{kind}");
    }
    for name in ["encrypted-no-type", "encrypted-identity"] {
        for password in [b"".as_slice(), b"wrong", b"attachment", b"owner"] {
            let mut input = input(Version::V1_11, name, password)?;
            input.extend(request(28, 3, 0, 0, 0));
            input.extend(request(29, 4, 0, 0, 0));
            input.extend(request(30, 5, 0, 0, 0));
            let bodies = serve(Session::default(), &input)?;
            assert_eq!(bodies[2][0], 156, "discovery stays lazy");
            if name == "encrypted-identity" || password == b"attachment" || password == b"owner" {
                assert_eq!(bodies[3][0], 157);
                assert_eq!(bodies[4][0], 158);
            } else {
                assert_eq!((bodies[3][0], bodies[3][5]), (255, 6));
                assert_eq!((bodies[4][0], bodies[4][5]), (255, 6));
            }
        }
    }
    Ok(())
}
