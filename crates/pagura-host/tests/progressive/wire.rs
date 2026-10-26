use super::*;
use pagura_host::SessionEnd;

#[test]
fn version_two_waits_complete_serial_requests_and_version_one_stays_frozen() -> Result<()> {
    let mut input = frame(1, 1, &0x0002_0000_u32.to_le_bytes());
    input.extend(frame(41, 2, &creation(8192)));
    input.extend(frame(45, 3, &[1, 0, 0, 0, 1, 0, 0, 0, 0, 0]));
    input.extend(frame(48, 4, &words(&[1])));
    input.extend(frame(50, 5, &words(&[1])));
    input.extend(frame(48, 6, &words(&[1])));
    input.extend(frame(51, 7, &words(&[1])));
    input.extend(frame(44, 8, &words(&[1])));
    let mut output = Vec::new();
    assert_eq!(
        Session::new(config()).serve(input.as_slice(), &mut output)?,
        SessionEnd::Eof
    );
    let frames = pagura_testkit::split_frames(&output);
    assert_eq!(
        frames
            .iter()
            .map(|frame| frame.body()[0])
            .collect::<Vec<_>>(),
        [129, 169, 173, 179, 176, 174, 177, 172]
    );
    assert_eq!(&frames[5].body()[5..], &[1, 0, 0, 0, 7]);
    let mut legacy = frame(1, 1, &0x0001_000c_u32.to_le_bytes());
    legacy.extend(frame(41, 2, &creation(8)));
    legacy.extend(frame(9, 3, &[]));
    let mut output = Vec::new();
    assert_eq!(
        Session::new(config()).serve(legacy.as_slice(), &mut output)?,
        SessionEnd::Eof
    );
    let frames = pagura_testkit::split_frames(&output);
    assert_eq!(
        frames
            .iter()
            .map(|frame| frame.body()[0])
            .collect::<Vec<_>>(),
        [129, 255, 137]
    );
    assert_eq!(&frames[2].body()[5..], &[0; 24]);
    Ok(())
}

#[test]
fn progressive_layouts_are_exact_and_truncated_inputs_are_never_accepted() -> Result<()> {
    let mut supply = [
        words(&[1]),
        VERSION.to_vec(),
        7_u64.to_le_bytes().to_vec(),
        words(&[3]),
    ]
    .concat();
    supply.extend(b"abc");
    let requests = [
        (41, creation(99)),
        (42, supply),
        (43, words(&[1])),
        (44, words(&[1])),
        (45, vec![1, 0, 0, 0, 1, 0, 0, 0, 0, 0]),
        (46, words(&[1, 0])),
        (47, rendering(1)),
        (48, words(&[1])),
        (49, ack(1, 3)),
        (50, words(&[1])),
        (51, words(&[1])),
        (52, vec![]),
    ];
    for (kind, payload) in requests {
        let bytes = frame(kind, 1, &payload);
        let body = &bytes[4..];
        let decoded = protocol::decode(body, &Limits::default())?;
        assert_eq!(decoded.request.kind().code(), kind);
        for end in 0..body.len() {
            assert!(
                protocol::decode(&body[..end], &Limits::default()).is_err(),
                "kind {kind}, split {end}"
            );
        }
        let mut trailing = body.to_vec();
        trailing.push(0);
        assert!(protocol::decode(&trailing, &Limits::default()).is_err());
    }
    for (at, code) in [(16, 2), (17, 2), (18, 2)] {
        let mut payload = rendering(0);
        payload[at] = code;
        assert!(protocol::decode(&frame(47, 1, &payload)[4..], &Limits::default()).is_err());
    }
    for at in [4, 5] {
        let mut payload = vec![1, 0, 0, 0, 1, 0, 0, 0, 0, 0];
        payload[at] = 2;
        assert!(protocol::decode(&frame(45, 1, &payload)[4..], &Limits::default()).is_err());
    }
    Ok(())
}

#[test]
fn native_session_order_failure_is_terminal_but_operational_errors_are_not() -> Result<()> {
    let mut host = Host::new(config())?;
    let frame = frame(52, 1, &[]);
    let decoded = protocol::decode(&frame[4..], &Limits::default())?;
    let expected = host.session.handle(decoded).unwrap_err();
    let next = pagura_testkit::frame(52, 3, &[]);
    assert_eq!(
        host.session
            .handle(protocol::decode(&next[4..], &Limits::default())?)
            .unwrap_err(),
        expected
    );
    let mut host = Host::new(config())?;
    assert_eq!(host.call(48, &words(&[99]))?[5], 7);
    assert_eq!(host.call(52, &[])?[0], 178);
    Ok(())
}
