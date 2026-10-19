#![forbid(unsafe_code)]
//! RFC 3161 2.4.2 TSTInfo on the generated `pki` fixtures.

use pagura_core::Limits;
use pagura_pki::der::Document;
use pagura_pki::timestamp::TstInfo;
use pagura_pki::x509::extensions::GeneralName;
use pagura_testkit::fixture;

fn der(name: &str) -> Vec<u8> {
    fixture("pki", &format!("{name}.der"))
}

#[test]
fn every_optional_field_is_read() {
    let bytes = der("tst-all-fields");
    let document = Document::parse(&bytes, &Limits::default()).unwrap();
    let info = TstInfo::parse(document.root()).unwrap();
    assert_eq!(info.policy.as_bytes(), [42, 3, 4]);
    assert_eq!(info.message_imprint.digest, (0..32).collect::<Vec<u8>>());
    assert_eq!(info.serial.to_u64(), Some(123));
    let time = info.gen_time;
    assert_eq!(
        (time.year(), time.hour(), time.fraction()),
        (2026, 12, "125")
    );
    let accuracy = info.accuracy.unwrap();
    assert_eq!(accuracy.seconds.unwrap().to_u64(), Some(2));
    assert_eq!((accuracy.millis, accuracy.micros), (Some(1), Some(999)));
    assert!(info.ordering);
    assert_eq!(info.nonce.unwrap().to_u64(), Some(256));
    assert!(matches!(info.tsa, Some(GeneralName::Directory(_))));
    assert_eq!(info.extensions.len(), 1);
    assert_eq!(info.element.encoded(), bytes);
}

#[test]
fn optional_fields_default_when_absent() {
    let bytes = der("tst-minimal");
    let document = Document::parse(&bytes, &Limits::default()).unwrap();
    let info = TstInfo::parse(document.root()).unwrap();
    assert_eq!(info.gen_time.fraction(), "");
    assert!(info.accuracy.is_none());
    assert!(!info.ordering);
    assert!(info.nonce.is_none());
    assert!(info.tsa.is_none());
    assert_eq!(info.extensions.len(), 0);
}

#[test]
fn serial_numbers_and_nonces_keep_their_sign() {
    let bytes = der("tst-negative-serial-nonce");
    let document = Document::parse(&bytes, &Limits::default()).unwrap();
    let info = TstInfo::parse(document.root()).unwrap();
    assert!(info.serial.is_negative());
    assert_eq!(info.nonce.unwrap().as_bytes(), [0xff, 0x7f]);
}
