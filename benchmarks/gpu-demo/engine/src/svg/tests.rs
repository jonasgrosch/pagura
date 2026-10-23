use super::*;
use pagura_data::Bundled;
use pagura_document::OpenOptions;
use pagura_interpret::Interpreter;
use std::sync::Arc;

fn pdf(content: &[u8]) -> Vec<u8> {
    pages(&[content])
}

pub(crate) fn pages(contents: &[&[u8]]) -> Vec<u8> {
    let kids = (0..contents.len())
        .map(|i| format!("{} 0 R ", 3 + i * 2))
        .collect::<String>();
    let mut objects = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        format!(
            "<< /Type /Pages /Kids [{kids}] /Count {} >>",
            contents.len()
        )
        .into_bytes(),
    ];
    for (i, content) in contents.iter().enumerate() {
        objects.push(format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 20 30] /Resources << >> /Contents {} 0 R >>",4+i*2).into_bytes());
        objects.push(
            [
                format!("<< /Length {} >>\nstream\n", content.len()).as_bytes(),
                content,
                b"\nendstream",
            ]
            .concat(),
        );
    }
    let mut pdf = b"%PDF-1.7\n".to_vec();
    let mut offsets = vec![0];
    for (i, object) in objects.iter().enumerate() {
        offsets.push(pdf.len());
        pdf.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
        pdf.extend(object);
        pdf.extend_from_slice(b"\nendobj\n");
    }
    let xref = pdf.len();
    pdf.extend_from_slice(
        format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes(),
    );
    for offset in &offsets[1..] {
        pdf.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    pdf.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            objects.len() + 1
        )
        .as_bytes(),
    );
    pdf
}

fn scene(content: &[u8]) -> (Document, Scene) {
    let document =
        Document::open(Arc::<[u8]>::from(pdf(content)), &OpenOptions::default()).unwrap();
    let provider = Bundled::new(document.limits());
    let scene = Interpreter::new(&document)
        .with_provider(Some(&provider))
        .scene(document.page(0).unwrap())
        .unwrap();
    (document, scene)
}

#[test]
fn clips_and_curves_stay_vectors_in_the_resolution_independent_export() {
    let (document, scene) = scene(b"0 0 10 10 re W n 1 0 0 rg 1 1 m 2 7 6 8 9 2 c h f");
    {
        let output = export(&scene, &document).unwrap();
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
        export(&scene, &document),
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

#[test]
fn clip_elision_requires_a_strict_hull_in_the_whole_rectangular_chain() {
    let cases: &[(&[u8], usize)] = &[
        (b"0 0 20 30 re W n 9 10 2 2 re f", 1),
        (b"0 0 20 30 re W n 1 10 2 2 re f", 0),
        (b"0 0 10 30 re W n 0 0 20 30 re W n 9 10 2 2 re f", 0),
        (b"0 0 m 20 0 l 20 30 l h W n 9 10 2 2 re f", 0),
        (b"0 0 20 30 re W n 9 10 m 30 10 30 11 11 12 c h f", 0),
        (b"0 0 20 30 re W n 30 w 9 10 m 11 12 l S", 0),
        (
            b"-1000000000 -1000000000 2000000000 2000000000 re W n 9 10 2 2 re f",
            0,
        ),
    ];
    for &(content, elided) in cases {
        let (document, scene) = scene(content);
        let output = export(&scene, &document).unwrap();
        assert_eq!(
            output.clips_elided,
            elided,
            "{}",
            String::from_utf8_lossy(content)
        );
        assert_eq!(output.clips_retained, usize::from(elided == 0));
    }
}

#[test]
fn page_export_cache_reuses_bytes_and_resets_on_document_open() {
    let mut session = super::super::Session::new();
    let open = |content: &[u8]| [1u32.to_le_bytes().as_slice(), &pdf(content)].concat();
    let render = |dpi: u32| {
        [4u32, 0, dpi, 32, 0, 0, 0, 0]
            .into_iter()
            .flat_map(u32::to_le_bytes)
            .collect::<Vec<_>>()
    };
    session.request(&open(b"1 0 0 rg 1 1 2 2 re f")).unwrap();
    let first = {
        let (meta, data) = session.request(&render(72)).unwrap();
        assert!(meta.contains("\"cache_hit\":false"));
        data.into_owned()
    };
    let (meta, data) = session.request(&render(150)).unwrap();
    assert!(meta.contains("\"cache_hit\":true"));
    assert_eq!(data.as_ref(), first);
    session.request(&open(b"0 1 0 rg 1 1 2 2 re f")).unwrap();
    let (meta, data) = session.request(&render(72)).unwrap();
    assert!(meta.contains("\"cache_hit\":false"));
    assert_ne!(data.as_ref(), first);
}

#[test]
fn page_change_invalidates_the_export_and_cannot_reuse_other_page_resources() {
    let mut session = super::super::Session::new();
    let pdf = pages(&[b"1 0 0 rg 1 1 2 2 re f", b"0 1 0 rg 1 1 2 2 re f"]);
    session
        .request(&[1u32.to_le_bytes().as_slice(), &pdf].concat())
        .unwrap();
    let render = |page: u32| {
        [4u32, page, 72, 32, 0, 0, 0, 0]
            .into_iter()
            .flat_map(u32::to_le_bytes)
            .collect::<Vec<_>>()
    };
    let first = session.request(&render(0)).unwrap().1.into_owned();
    let (meta, second) = session.request(&render(1)).unwrap();
    assert!(meta.contains("\"cache_hit\":false"));
    assert_ne!(first, second.as_ref());
    let (meta, again) = session.request(&render(0)).unwrap();
    assert!(meta.contains("\"cache_hit\":false"));
    assert_eq!(first, again.as_ref());
}
