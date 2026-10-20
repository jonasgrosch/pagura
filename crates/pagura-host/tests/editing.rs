#![forbid(unsafe_code)]
//! Independent editing frames and immutable host state.
use pagura::{Document, OpenOptions, Recovery};
use pagura_host::{Session, SessionEnd, protocol};
use pagura_testkit::{fixture, frame, split_frames};

fn opening(bytes: &[u8]) -> Result<Vec<u8>, std::num::TryFromIntError> {
    let mut payload = vec![0]; // Strict; empty password.
    payload.extend(0_u32.to_le_bytes());
    payload.extend(u32::try_from(bytes.len())?.to_le_bytes());
    payload.extend(bytes);
    Ok(payload)
}
fn saving(changes: &[u8]) -> Result<Vec<u8>, std::num::TryFromIntError> {
    let mut payload = 1_u32.to_le_bytes().to_vec();
    payload.extend(u32::try_from(changes.len())?.to_le_bytes());
    payload.extend(changes);
    Ok(payload)
}
#[test]
fn browser_save_returns_new_bytes_without_replacing_the_handle() {
    let original = fixture("forms", "render-text.pdf");
    let mut input = frame(1, 1, &protocol::VERSION_1_5.to_le_bytes());
    input.extend(frame(2, 2, &opening(&original).unwrap()));
    input.extend(frame(18, 3, &1_u32.to_le_bytes()));
    // One text change: field zero, UTF-8 ABC.
    let changes = [1, 0, 0, 0, 0, 0, 0, 0, 0, 3, 0, 0, 0, b'A', b'B', b'C'];
    input.extend(frame(19, 4, &saving(&changes).unwrap()));
    input.extend(frame(18, 5, &1_u32.to_le_bytes()));
    let mut output = Vec::new();
    assert_eq!(
        Session::default()
            .serve(&mut input.as_slice(), &mut output)
            .unwrap(),
        SessionEnd::Eof
    );
    let frames = split_frames(&output);
    assert_eq!(
        frames
            .iter()
            .map(pagura_testkit::Frame::kind)
            .collect::<Vec<_>>(),
        [129, 130, 146, 147, 146]
    );
    assert_eq!(frames[2].payload(), frames[4].payload());
    let saved = &frames[3].payload()[4..];
    assert!(saved.starts_with(&original));
    let document = Document::open(
        saved,
        &OpenOptions {
            recovery: Recovery::Strict,
            ..Default::default()
        },
    )
    .unwrap();
    let form = document.acro_form().unwrap().unwrap();
    assert!(
        matches!(&form.fields[0].value, Some(pagura::forms::FieldValue::Text(text)) if text == "ABC")
    );
}
#[test]
fn malformed_edits_are_atomic_and_old_versions_reject_new_operations() {
    let original = fixture("forms", "render-text.pdf");
    for version in [protocol::VERSION_1_4, protocol::VERSION_1_5] {
        let mut input = frame(1, 1, &version.to_le_bytes());
        input.extend(frame(2, 2, &opening(&original).unwrap()));
        input.extend(frame(19, 3, &saving(&[0, 0, 0, 0, 99]).unwrap()));
        input.extend(frame(3, 4, &[1, 0, 0, 0, 0, 0, 0, 0]));
        let mut output = Vec::new();
        assert_eq!(
            Session::default()
                .serve(&mut input.as_slice(), &mut output)
                .unwrap(),
            SessionEnd::Eof
        );
        let frames = split_frames(&output);
        assert_eq!(frames[2].kind(), 255);
        assert_eq!(frames[3].kind(), 131);
    }
}

#[test]
fn widget_geometry_is_version_gated_without_closing_the_session() {
    let original = fixture("forms", "render-text.pdf");
    for version in [protocol::VERSION_1_5, protocol::VERSION_1_6] {
        let mut input = frame(1, 1, &version.to_le_bytes());
        input.extend(frame(2, 2, &opening(&original).unwrap()));
        input.extend(frame(20, 3, &[1, 0, 0, 0, 0, 0, 0, 0]));
        input.extend(frame(3, 4, &[1, 0, 0, 0, 0, 0, 0, 0]));
        let mut output = Vec::new();
        assert_eq!(
            Session::default()
                .serve(&mut input.as_slice(), &mut output)
                .unwrap(),
            SessionEnd::Eof
        );
        let frames = split_frames(&output);
        assert_eq!(
            frames[2].kind(),
            if version == protocol::VERSION_1_6 {
                148
            } else {
                255
            }
        );
        assert_eq!(frames[3].kind(), 131);
    }
}

#[test]
fn stored_text_metrics_are_additive_and_version_gated() {
    let original = fixture("forms", "render-text.pdf");
    for version in [protocol::VERSION_1_6, protocol::VERSION_1_7] {
        let mut input = frame(1, 1, &version.to_le_bytes());
        input.extend(frame(2, 2, &opening(&original).unwrap()));
        input.extend(frame(21, 3, &[1, 0, 0, 0, 0, 0, 0, 0]));
        input.extend(frame(20, 4, &[1, 0, 0, 0, 0, 0, 0, 0]));
        let mut output = Vec::new();
        Session::default()
            .serve(&mut input.as_slice(), &mut output)
            .unwrap();
        let frames = split_frames(&output);
        assert_eq!(
            frames[2].kind(),
            if version == protocol::VERSION_1_7 {
                149
            } else {
                255
            }
        );
        assert_eq!(frames[3].kind(), 148);
    }
}
