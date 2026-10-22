use super::*;
use pagura_data::Bundled;
use pagura_document::OpenOptions;
use pagura_interpret::Interpreter;
use std::sync::Arc;

fn scene(content: &[u8]) -> (Document, Scene) {
    let objects = [
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 20 30] /Resources << >> /Contents 4 0 R >>"
            .to_vec(),
        [
            format!("<< /Length {} >>\nstream\n", content.len()).as_bytes(),
            content,
            b"\nendstream",
        ]
        .concat(),
    ];
    let mut pdf = b"%PDF-1.7\n".to_vec();
    let mut offsets = vec![0];
    for (i, object) in objects.iter().enumerate() {
        offsets.push(pdf.len());
        pdf.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
        pdf.extend(object);
        pdf.extend_from_slice(b"\nendobj\n");
    }
    let xref = pdf.len();
    pdf.extend_from_slice(b"xref\n0 5\n0000000000 65535 f \n");
    for offset in &offsets[1..] {
        pdf.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    pdf.extend_from_slice(
        format!("trailer\n<< /Size 5 /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n").as_bytes(),
    );
    let document = Document::open(Arc::<[u8]>::from(pdf), &OpenOptions::default()).unwrap();
    let provider = Bundled::new(document.limits());
    let scene = Interpreter::new(&document)
        .with_provider(Some(&provider))
        .scene(document.page(0).unwrap())
        .unwrap();
    (document, scene)
}

#[test]
fn clips_and_curves_stay_vectors_at_every_resolution() {
    let (document, scene) = scene(b"0 0 10 10 re W n 1 0 0 rg 1 1 m 2 7 6 8 9 2 c h f");
    for dpi in [72, 150, 300] {
        let output = export(&scene, &document, dpi).unwrap();
        let xml = String::from_utf8(output.bytes).unwrap();
        assert!(xml.contains("<clipPath"));
        assert!(xml.contains("C2 7 6 8 9 2"));
        assert!(!xml.contains("data:image"));
        assert_eq!((output.paths, output.images), (1, 0));
    }
}

#[test]
fn unsupported_transfer_and_masks_refuse_the_export() {
    let (document, mut scene) = scene(b"0 0 20 30 re f");
    let state = Arc::make_mut(&mut scene.states[0]);
    state.soft_mask = Some(pagura_interpret::MaskId(0));
    assert!(matches!(
        export(&scene, &document, 72),
        Err(Failure::Unsupported(_))
    ));
}

#[test]
fn generated_attributes_reject_nonfinite_geometry_and_oversized_xml() {
    assert!(matrix(Transform::scale(f64::INFINITY, 1.0)).is_err());
    let mut xml = Xml::new();
    xml.text = "x".repeat(super::super::MAX_FRAME);
    assert!(xml.add(format_args!("x")).is_err());
}
