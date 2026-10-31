#![forbid(unsafe_code)]
//! Full independent pixels, save refusal and repaired signed-source policy.

use pagura::editing::{SaveOptions, Transaction, features};
use pagura::signatures::{Integrity, ModificationOutcome, PermittedModifications};
use pagura::{Document, ErrorKind, OpenOptions, Recovery, RenderOptions, Rgba8, Value};
use pagura_testkit::fixture;

fn document(bytes: Vec<u8>, recovery: Recovery) -> pagura::Result<Document> {
    Document::open(
        bytes,
        &OpenOptions {
            recovery,
            ..OpenOptions::default()
        },
    )
}

#[test]
fn complete_pixels_preserve_admitted_and_declined_stream_recovery() {
    for name in [
        "undersized-joined",
        "positive-generation",
        "sparse-original",
        "omitted-content-joined",
        "omitted-content-separated",
        "omitted-scalar-joined",
        "wrong-scalar-offset",
        "generation-mismatch",
        "negative-scalar",
        "name-scalar",
        "integer-overflow",
        "fake-header-in-stream",
        "fake-header-in-string",
        "fake-header-in-comment",
        "fake-header-in-name",
        "joined-then-generation-override",
        "joined-then-clean-override",
        "missing-pdf-header",
        "encrypt-null",
        "header-extra-format",
        "leading-junk-physical-offsets",
        "leading-junk-relative-offsets",
        "all-free-size-coverage",
    ] {
        let original = fixture("original_records", &format!("{name}.pdf"));
        let doc = document(original.clone(), Recovery::Repair).unwrap();
        for scale in [1_u32, 2] {
            for background in [Rgba8::WHITE, Rgba8::TRANSPARENT] {
                let options = RenderOptions {
                    background,
                    ..RenderOptions::from_dpi(72.0 * f64::from(scale))
                };
                let pixels = pagura::render_page(&doc, doc.page(0).unwrap(), &options).unwrap();
                assert_eq!((pixels.width(), pixels.height()), (32 * scale, 32 * scale));
                let mut expected_rgb = Vec::new();
                let mut expected_rgba = Vec::new();
                for row in 0..32 * scale {
                    for x in 0..32 * scale {
                        let red = (4 * scale..12 * scale).contains(&x)
                            && (20 * scale..28 * scale).contains(&row);
                        expected_rgb.extend(if red { [255, 0, 0] } else { [255, 255, 255] });
                        expected_rgba.extend(if red {
                            [255, 0, 0, 255]
                        } else if background == Rgba8::WHITE {
                            [255, 255, 255, 255]
                        } else {
                            [0, 0, 0, 0]
                        });
                    }
                }
                assert_eq!(
                    pixels.to_rgb_over(Rgba8::WHITE),
                    expected_rgb,
                    "{name} scale{scale}"
                );
                assert_eq!(
                    pixels.data(),
                    expected_rgba.as_slice(),
                    "{name} scale{scale}"
                );
                assert_eq!(doc.original_bytes(), original);
            }
        }
    }
}

#[test]
fn scalar_admission_does_not_make_a_repaired_source_an_incremental_base() {
    let original = fixture("original_records", "undersized-joined.pdf");
    let doc = document(original.clone(), Recovery::Repair).unwrap();
    assert_eq!(doc.revisions(), []);
    let mut transaction = Transaction::new(&doc);
    assert_eq!(
        transaction.commit(&SaveOptions::default()).unwrap(),
        original
    );
    transaction.add(Value::Null.into()).unwrap();
    assert_eq!(
        transaction
            .commit(&SaveOptions::default())
            .unwrap_err()
            .kind,
        ErrorKind::Unsupported(features::REPAIRED)
    );
    assert_eq!(doc.original_bytes(), original);
}

#[test]
fn recovered_original_records_do_not_certify_a_signed_source_revision() {
    let original = fixture("cms-signatures", "rsa-sha256.pdf");
    let clean = document(original.clone(), Recovery::Strict).unwrap();
    let result = pagura::signatures(&clean).unwrap();
    let signature = result
        .fields
        .iter()
        .find_map(|field| field.signature.as_ref())
        .unwrap();
    assert_eq!(signature.results.integrity, Integrity::Verified);
    let PermittedModifications::Analysed(analysis) = &signature.results.permitted_modifications
    else {
        panic!("clean policy control required")
    };
    assert_eq!(analysis.outcome, ModificationOutcome::NoChanges);

    // Change only the decimal Size, preserving its width and every original
    // ByteRange/Contents offset. This deliberately invalidates the CMS digest;
    // neither a repaired index nor a policy result asserts integrity.
    let mut malformed = original.clone();
    let start = malformed
        .windows(b"/Size ".len())
        .rposition(|part| part == b"/Size ")
        .unwrap()
        + b"/Size ".len();
    let end = start
        + malformed[start..]
            .iter()
            .take_while(|byte| byte.is_ascii_digit())
            .count();
    assert!(end > start);
    malformed[start..end].fill(b'0');
    malformed[end - 1] = b'2';
    assert_eq!(malformed.len(), original.len());
    let strict = Document::open(
        malformed.clone(),
        &OpenOptions {
            recovery: Recovery::Strict,
            ..OpenOptions::default()
        },
    )
    .unwrap_err();
    assert_eq!(strict.kind, ErrorKind::Syntax);
    assert_eq!(
        strict.message,
        "trailer Size does not cover cross-reference entries"
    );
    assert_eq!(strict.repair, None);
    let repaired = document(malformed.clone(), Recovery::Repair).unwrap();
    assert_eq!(repaired.revisions(), []);
    let result = pagura::signatures(&repaired).unwrap();
    let signature = result
        .fields
        .iter()
        .find_map(|field| field.signature.as_ref())
        .unwrap();
    assert_ne!(signature.results.integrity, Integrity::Verified);
    assert_eq!(
        signature.results.permitted_modifications,
        PermittedModifications::Inconclusive
    );
    assert_eq!(repaired.original_bytes(), malformed);
}

#[test]
fn later_scalar_definition_preserves_existing_bounded_stream_recovery() {
    let original = fixture("original_records", "joined-then-clean-override.pdf");
    let doc = document(original.clone(), Recovery::Repair).unwrap();
    assert_eq!(
        doc.object(pagura::ObjectId::new(5, 0)).unwrap().value(),
        &Value::Integer(99)
    );
    let stream = doc.object(pagura::ObjectId::new(4, 0)).unwrap();
    assert_eq!(
        doc.stream_data(stream).unwrap().as_ref(),
        b"1 0 0 rg 4 4 8 8 re f"
    );
    assert!(
        doc.diagnostics()
            .iter()
            .any(|item| item.tag() == "stream.length-corrected")
    );
    assert_eq!(doc.original_bytes(), original);
}
