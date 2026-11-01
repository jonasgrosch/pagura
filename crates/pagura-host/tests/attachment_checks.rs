#![forbid(unsafe_code)]
//! Fixed attachment observations, authentication and independent response limits.

use pagura::{ErrorKind, Limits};
use pagura_host::protocol::{self, Version};
use pagura_host::{Session, SessionConfig, SessionLimits};
use pagura_testkit::{fixture, frame, split_frames};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

fn input(version: Version, name: &str, password: &[u8], repair: bool) -> Result<Vec<u8>> {
    let pdf = fixture("files", &format!("{name}.pdf"));
    let mut open = vec![u8::from(repair)];
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

fn requests(input: &mut Vec<u8>, document: u8, index: u8, key: u8) {
    input.extend(frame(27, 3, &[document, 0, 0, 0, index, 0, 0, 0, key]));
    input.extend(frame(3, 4, &[1, 0, 0, 0, 0, 0, 0, 0]));
}

#[test]
fn checks_have_canonical_requests_and_preserve_older_sessions() -> Result<()> {
    let bytes = frame(27, 3, &[1, 0, 0, 0, 0, 0, 0, 0, 1]);
    assert_eq!(protocol::decode(&bytes[4..], &Limits::default())?.id, 3);
    for end in 4..bytes.len() {
        assert!(protocol::decode(&bytes[4..end], &Limits::default()).is_err());
    }
    let mut extra = bytes[4..].to_vec();
    extra.push(0);
    assert!(protocol::decode(&extra, &Limits::default()).is_err());
    for key in 5..=u8::MAX {
        let bytes = frame(27, 3, &[1, 0, 0, 0, 0, 0, 0, 0, key]);
        assert_eq!(
            protocol::decode(&bytes[4..], &Limits::default())
                .unwrap_err()
                .kind,
            ErrorKind::Syntax
        );
    }
    for &version in Version::ALL
        .iter()
        .filter(|&&version| version < Version::V1_10)
    {
        let mut input = input(version, "check-flate", b"", false)?;
        requests(&mut input, 1, 0, 1);
        let bodies = serve(Session::default(), &input)?;
        assert_eq!((bodies[2][0], bodies[2][5]), (255, 7));
        assert_eq!(bodies[3][0], 131, "older sessions remain usable");
    }
    Ok(())
}

#[test]
fn completed_checks_preserve_missing_and_mismatching_declarations() -> Result<()> {
    for (name, observations, size) in [
        ("check-flate", [1, 1], 18),
        ("check-size-mismatch", [2, 1], 18),
        ("check-checksum-mismatch", [1, 2], 18),
        ("check-both-mismatch", [2, 2], 18),
        ("check-size-only", [1, 0], 18),
        ("check-checksum-only", [0, 1], 18),
        ("check-empty-params", [0, 0], 18),
        ("empty-payload", [1, 1], 0),
    ] {
        let mut input = input(Version::V1_10, name, b"", false)?;
        requests(&mut input, 1, 0, 1);
        let config = SessionConfig::default().with_session(SessionLimits {
            reading_response_bytes: 16,
            ..SessionLimits::default()
        });
        let bodies = serve(Session::new(config), &input)?;
        let mut expected = vec![155, 3, 0, 0, 0, 6, 0, 0, 0, 0, 0];
        expected.extend(u64::try_from(size)?.to_le_bytes());
        expected.extend(observations);
        assert_eq!(bodies[2], expected, "{name}: complete literal response");
        assert_eq!(bodies[3][0], 131);
    }
    Ok(())
}

#[test]
fn checks_respect_response_and_decode_limits_and_exact_selection() -> Result<()> {
    for config in [
        SessionConfig::default().with_session(SessionLimits {
            reading_response_bytes: 15,
            ..SessionLimits::default()
        }),
        SessionConfig::new(Limits {
            decoded_bytes: 4,
            ..Limits::default()
        }),
    ] {
        let mut input = input(Version::V1_10, "check-flate", b"", false)?;
        requests(&mut input, 1, 0, 1);
        let bodies = serve(Session::new(config), &input)?;
        assert_eq!(bodies[1][0], 130, "control must open before checking");
        assert_eq!((bodies[2][0], bodies[2][5]), (255, 2));
        assert_eq!(bodies[3][0], 131, "resource refusal retains the source");
    }
    for (name, document, index, key, category) in [
        ("check-flate", 2, 0, 1, 7),
        ("check-flate", 1, 1, 1, 7),
        ("check-flate", 1, 0, 4, 7),
        ("external-stream", 1, 0, 0, 3),
    ] {
        let mut input = input(Version::V1_10, name, b"", false)?;
        requests(&mut input, document, index, key);
        let bodies = serve(Session::default(), &input)?;
        assert_eq!((bodies[2][0], bodies[2][5]), (255, category), "{name}");
        if name == "external-stream" {
            let tag = b"stream.external";
            assert!(bodies[2].windows(tag.len()).any(|bytes| bytes == tag));
        }
        assert_eq!(bodies[3][0], 131);
    }
    Ok(())
}

#[test]
fn absent_declarations_still_require_payload_authorization() -> Result<()> {
    // EFOpen fixtures are a reported extension, not strict-valid PDFs.
    for name in [
        "encrypted-no-type",
        "encrypted-explicit-no-type",
        "encrypted-identity-no-type",
    ] {
        for password in [b"".as_slice(), b"wrong", b"attachment", b"owner"] {
            let mut input = input(Version::V1_10, name, password, true)?;
            requests(&mut input, 1, 0, 0);
            let bodies = serve(Session::default(), &input)?;
            let public = name == "encrypted-identity-no-type";
            if public || password == b"attachment" || password == b"owner" {
                assert_eq!(bodies[2][0], 155);
                assert_eq!(
                    u64::from_le_bytes(bodies[2][11..19].try_into()?),
                    if public { 23 } else { 19 }
                );
                assert_eq!(&bodies[2][19..], &[0, 0]);
            } else {
                assert_eq!((bodies[2][0], bodies[2][5]), (255, 6));
            }
            assert_eq!(
                bodies[3][0], 131,
                "authorization failure preserves page access"
            );
        }
    }
    Ok(())
}
