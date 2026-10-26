#![forbid(unsafe_code)]
//! Protocol 1.12 source selections, independent bytes and immutable authentication.
use pagura::{ErrorKind, Limits};
use pagura_host::protocol::{self, Version};
use pagura_host::{Session, SessionConfig, SessionLimits};
use pagura_testkit::{fixture, frame, split_frames};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
fn start(area: &str, name: &str, version: Version) -> Result<Vec<u8>> {
    let pdf = fixture(area, name);
    let mut payload = vec![1, 0, 0, 0, 0];
    payload.extend(u32::try_from(pdf.len())?.to_le_bytes());
    payload.extend(pdf);
    let mut input = frame(1, 1, &version.code().to_le_bytes());
    input.extend(frame(2, 2, &payload));
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
fn payload(scope: &[u8], related: Option<u32>) -> Vec<u8> {
    let mut out = 1_u32.to_le_bytes().to_vec();
    out.extend(scope);
    out.extend(0_u32.to_le_bytes());
    out.push(0);
    out.push(u8::from(related.is_some()));
    if let Some(index) = related {
        out.extend(index.to_le_bytes());
    }
    out
}
fn blob(bytes: &[u8]) -> Vec<u8> {
    let mut out = u32::try_from(bytes.len()).unwrap().to_le_bytes().to_vec();
    out.extend(bytes);
    out
}

#[test]
fn scoped_files_metadata_and_images_preserve_independent_payloads() -> Result<()> {
    let mut input = start("document_services", "complete.pdf", Version::V1_12)?;
    let scopes = [
        vec![0],
        vec![1],
        vec![2, 0, 0, 0, 0],
        vec![3, 10, 0, 0, 0, 0, 0],
    ];
    let mut expected = Vec::new();
    let mut request = 3;
    for scope in scopes {
        for (related, data) in [
            (None, b"primary\0payload\xff\n".as_slice()),
            (Some(0), b"first sidecar"),
            (Some(1), b"second sidecar\0"),
        ] {
            input.extend(frame(32, request, &payload(&scope, related)));
            expected.push(data);
            request += 1;
        }
    }
    input.extend(frame(35, request, &[1, 0, 0, 0, 2, 10, 0, 0, 0, 0, 0]));
    request += 1;
    input.extend(frame(38, request, &[1, 0, 0, 0, 2, 9, 0, 0, 0, 0, 0, 0, 1]));
    request += 1;
    input.extend(frame(38, request, &[1, 0, 0, 0, 1, 0, 0, 0, 0, 1, 1]));
    let bodies = serve(Session::default(), &input)?;
    for (index, data) in expected.iter().enumerate() {
        assert_eq!(bodies[index + 2][0], 160);
        assert_eq!(&bodies[index + 2][5..], blob(data));
    }
    let packet = b"<x:xmpmeta xmlns:x=\"adobe:ns:meta/\">object packet</x:xmpmeta>";
    assert_eq!(bodies[14][0], 163);
    assert_eq!(&bodies[14][5..], [vec![1], blob(packet)].concat());
    for (index, data) in [(15, vec![255, 0, 0, 0, 255, 0]), (16, vec![127])] {
        assert_eq!(bodies[index][0], 166);
        assert_eq!(
            &bodies[index][5..],
            [blob(b"encoded"), vec![0], blob(&data)].concat()
        );
    }
    Ok(())
}

#[test]
fn additions_are_canonical_and_version_gated() -> Result<()> {
    let requests = [
        (31, vec![1, 0, 0, 0, 1]),
        (32, payload(&[0], Some(0))),
        (33, payload(&[0], None)),
        (34, vec![1, 0, 0, 0, 2, 10, 0, 0, 0, 0, 0]),
        (35, vec![1, 0, 0, 0, 0]),
        (36, vec![1, 0, 0, 0]),
        (37, vec![1, 0, 0, 0, 1, 0, 0, 0, 0]),
        (38, vec![1, 0, 0, 0, 2, 9, 0, 0, 0, 0, 0, 0, 1]),
        (39, vec![1, 0, 0, 0, 0, 0, 0, 0]),
        (40, vec![1, 0, 0, 0, 1, 0, 0, 0, 0]),
    ];
    for (kind, payload) in &requests {
        let encoded = frame(*kind, 3, payload);
        let body = &encoded[4..];
        assert!(protocol::decode(body, &Limits::default()).is_ok());
        for length in 0..body.len() {
            assert!(protocol::decode(&body[..length], &Limits::default()).is_err());
        }
        let mut trailing = body.to_vec();
        trailing.push(0);
        assert_eq!(
            protocol::decode(&trailing, &Limits::default())
                .unwrap_err()
                .kind,
            ErrorKind::Syntax
        );
    }
    for &version in Version::ALL
        .iter()
        .filter(|&&version| version < Version::V1_12)
    {
        let mut input = start("document_services", "complete.pdf", version)?;
        for (index, (kind, payload)) in requests.iter().enumerate() {
            input.extend(frame(*kind, u32::try_from(index + 3)?, payload));
        }
        input.extend(frame(3, 13, &[1, 0, 0, 0, 0, 0, 0, 0]));
        let bodies = serve(Session::default(), &input)?;
        for body in &bodies[2..12] {
            assert_eq!((body[0], body[5]), (255, 7));
        }
        assert_eq!(bodies[12][0], 131);
    }
    for kind in [31, 34, 37, 40] {
        let data = frame(kind, 3, &[1, 0, 0, 0, 255]);
        assert_eq!(
            protocol::decode(&data[4..], &Limits::default())
                .unwrap_err()
                .kind,
            ErrorKind::Syntax
        );
    }
    Ok(())
}

#[test]
fn reauthentication_does_not_mutate_original_or_consume_failed_handles() -> Result<()> {
    let mut input = start("files", "encrypted-no-type.pdf", Version::V1_12)?;
    input.extend(frame(32, 3, &payload(&[0], None)));
    input.extend(frame(
        39,
        4,
        &[1_u32.to_le_bytes().to_vec(), blob(b"wrong")].concat(),
    ));
    input.extend(frame(
        39,
        5,
        &[1_u32.to_le_bytes().to_vec(), blob(b"attachment")].concat(),
    ));
    let mut authorized = payload(&[0], None);
    authorized[0] = 2;
    input.extend(frame(32, 6, &authorized));
    input.extend(frame(32, 7, &payload(&[0], None)));
    input.extend(frame(6, 8, &2_u32.to_le_bytes()));
    input.extend(frame(9, 9, &[]));
    let bodies = serve(Session::default(), &input)?;
    for index in [2, 3, 6] {
        assert_eq!((bodies[index][0], bodies[index][5]), (255, 6));
    }
    assert_eq!(bodies[4][0], 167);
    assert_eq!(&bodies[4][5..], [2, 0, 0, 0, 1, 0, 0, 0]);
    assert_eq!(&bodies[5][5..], blob(b"attached file bytes"));
    assert_eq!(bodies[8][21], 1, "only original handle remains");
    Ok(())
}

#[test]
fn response_and_snapshot_limits_refuse_work_without_partial_success() -> Result<()> {
    let mut input = start("document_services", "complete.pdf", Version::V1_12)?;
    input.extend(frame(31, 3, &[1, 0, 0, 0, 0]));
    let session = Session::new(SessionConfig::default().with_session(SessionLimits {
        reading_response_bytes: 8,
        ..SessionLimits::default()
    }));
    let bodies = serve(session, &input)?;
    assert_eq!((bodies[2][0], bodies[2][5]), (255, 2));
    let mut input = start("files", "encrypted-no-type.pdf", Version::V1_12)?;
    input.extend(frame(
        39,
        3,
        &[1_u32.to_le_bytes().to_vec(), blob(b"attachment")].concat(),
    ));
    let session = Session::new(SessionConfig::default().with_session(SessionLimits {
        max_documents: 1,
        ..SessionLimits::default()
    }));
    let bodies = serve(session, &input)?;
    assert_eq!((bodies[2][0], bodies[2][5]), (255, 2));
    Ok(())
}
