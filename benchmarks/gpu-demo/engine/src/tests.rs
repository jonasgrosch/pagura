//! Real framed session tests: identity, cache bounds and invalidation.
use super::*;
use crate::svg::tests::pages;

fn request(session: &mut Session, args: [u32; 8]) -> (String, Vec<u8>) {
    let bytes: Vec<u8> = args.into_iter().flat_map(u32::to_le_bytes).collect();
    let (meta, bytes) = session.request(&bytes).unwrap();
    (meta, bytes.into_owned())
}
fn open(session: &mut Session, contents: &[&[u8]]) {
    let mut bytes = 1_u32.to_le_bytes().to_vec();
    bytes.extend(pages(contents));
    session.request(&bytes).unwrap();
}
fn number(meta: &str, name: &str) -> u64 {
    meta.split(&format!("\"{name}\":"))
        .nth(1)
        .unwrap()
        .split([',', '}'])
        .next()
        .unwrap()
        .parse()
        .unwrap()
}

#[test]
fn retained_cpu_reuses_preparation_and_output_without_changing_pixels() {
    let mut session = Session::new();
    open(&mut session, &[b"0.2 0.5 0.8 rg 1.25 2.75 12.3 20.1 re f"]);
    let options = [7, 0, 150, 32, 0, 0, 0, 0];
    let (first, pixels) = request(&mut session, options);
    assert!(first.contains("\"cache_hit\":false"));
    let pointer = session.output.as_ptr();
    for _ in 0..4 {
        let (meta, repeated) = request(&mut session, options);
        assert!(meta.contains("\"cache_hit\":true"));
        assert_eq!(number(&first, "prepared_id"), number(&meta, "prepared_id"));
        assert_eq!(pixels, repeated);
        assert_eq!(pointer, session.output.as_ptr());
    }
    let (_, fresh) = request(&mut session, [2, 0, 150, 32, 0, 0, 0, 0]);
    assert_eq!(pixels, fresh);
    for band in [1, 7, 256] {
        let (_, cached) = request(&mut session, [7, 0, 150, band, 1, 2, 20, 30]);
        let (_, fresh) = request(&mut session, [2, 0, 150, band, 1, 2, 20, 30]);
        assert_eq!(cached, fresh);
    }
}

#[test]
fn prepared_lru_evicts_the_oldest_of_three_resolutions() {
    let mut session = Session::new();
    open(&mut session, &[b"0 0 20 30 re f"]);
    for dpi in [72, 150, 225, 72, 300] {
        request(&mut session, [7, 0, dpi, 32, 0, 0, 0, 0]);
    }
    let (meta, _) = request(&mut session, [7, 0, 225, 32, 0, 0, 0, 0]);
    assert!(meta.contains("\"cache_hit\":true"));
    assert_eq!(number(&meta, "cache_entries"), 3);
    assert!(number(&meta, "cache_charged_bytes") <= 256 * 1024 * 1024);
    let (meta, _) = request(&mut session, [7, 0, 150, 32, 0, 0, 0, 0]);
    assert!(meta.contains("\"cache_hit\":false"));
    assert!(number(&meta, "cache_evictions") >= 2);
    let (meta, _) = request(&mut session, [9, 0, 150, 32, 0, 0, 0, 0]);
    assert_eq!(number(&meta, "cache_entries"), 0);
}

#[test]
fn page_and_document_changes_invalidate_prepared_scenes() {
    let mut session = Session::new();
    open(
        &mut session,
        &[b"0 0 20 30 re f", b"1 0 0 rg 0 0 20 30 re f"],
    );
    let (_, black) = request(&mut session, [7, 0, 72, 32, 0, 0, 0, 0]);
    let (meta, red) = request(&mut session, [7, 1, 72, 32, 0, 0, 0, 0]);
    assert!(meta.contains("\"cache_hit\":false"));
    assert_eq!(number(&meta, "cache_entries"), 1);
    assert_ne!(black, red);
    open(&mut session, &[b"0 0 1 rg 0 0 20 30 re f"]);
    let (meta, blue) = request(&mut session, [7, 0, 72, 32, 0, 0, 0, 0]);
    assert!(meta.contains("\"cache_hit\":false"));
    assert_ne!(red, blue);
    assert!(session.request(&[1, 0, 0, 0, 0]).is_err());
    assert!(
        session
            .request(&[
                7, 0, 0, 0, 0, 0, 0, 0, 72, 0, 0, 0, 32, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                0, 0, 0, 0, 0
            ])
            .is_err()
    );
}

#[test]
fn world_packet_reuses_across_dpis_and_resets_on_open() {
    let mut session = Session::new();
    open(&mut session, &[b"0.2 0.4 0.8 rg 1.25 2.75 12.3 20.1 re f"]);
    let (first, vector) = request(&mut session, [11, 0, 72, 32, 0, 0, 0, 0]);
    assert!(first.contains("\"world_cache_hit\":false"));
    for dpi in [150, 173, 225, 299, 300] {
        let (meta, reused) = request(&mut session, [11, 0, dpi, 32, 0, 0, 0, 0]);
        assert!(meta.contains("\"world_cache_hit\":true"));
        assert_eq!(vector, reused);
        let (_, patch) = request(&mut session, [12, 0, dpi, 32, 0, 0, 0, 0]);
        assert_eq!(
            u32::from_le_bytes(patch[0..4].try_into().unwrap()),
            0x3150_5747
        );
    }
    open(&mut session, &[b"0 w 0 0 m 12 20 l S"]);
    let (meta, _) = request(&mut session, [11, 0, 150, 32, 0, 0, 0, 0]);
    assert!(meta.contains("hairlines"));
    open(&mut session, &[b"1 0 0 rg 1 2 12 20 re f"]);
    let (meta, new) = request(&mut session, [11, 0, 150, 32, 0, 0, 0, 0]);
    assert!(meta.contains("\"world_cache_hit\":false"));
    assert_ne!(vector, new);
}

#[test]
fn world_refuses_device_cell_fills_and_clips() {
    let mut session = Session::new();
    for content in [
        b"2 2 m 2 2 l f".as_slice(),
        b"2 2 0 4 re f",
        b"2 2 m 4 4 l 6 6 l f",
        b"2 2 m 2 2 l W n 0 0 20 30 re f",
    ] {
        open(&mut session, &[content]);
        let (meta, bytes) = request(&mut session, [11, 0, 150, 32, 0, 0, 0, 0]);
        assert!(meta.contains("resolution dependent"), "{meta}");
        assert!(bytes.is_empty());
    }
}
