//! Literal source gaps after semantic repairs, through the public rendering API.

use pagura::source::{
    ByteRange, ByteSource, LoadingLimits, SourceIdentity, SourceLimits, SparseBytes,
};
use pagura::{
    Diagnostic, Error, ErrorKind, ObjectId, OpenJob, OpenOptions, Opening, Position, ReadFault,
    ReadResult, Recovery, RenderContext, Validation,
};
use std::sync::Arc;

const ID: SourceIdentity = SourceIdentity::new(823, [31; 32]);
const PALETTE: &str =
    "<< /ColorSpace << /Pal [/Indexed /DeviceGray 0 <0001>] >> /XObject << /Fm 5 0 R >> >>";
const TYPE3: &str = "<< /Font << /F << /Type /Font /Subtype /Type3 /FontMatrix [0.001 0 0 0.001 0 0] /FontBBox [0 0 1 1] /CharProcs << >> /Encoding << /BaseEncoding /Unknown /Differences [65 /A 66 /B] >> /FirstChar 65 /LastChar 66 /Widths [500 500] /ToUnicode 5 0 R >> >> >>";

fn stream(number: u8, dictionary: &str, content: &[u8]) -> Vec<u8> {
    let mut bytes = format!(
        "{number} 0 obj << /Length {} {dictionary} >> stream\n",
        content.len()
    )
    .into_bytes();
    bytes.extend_from_slice(content);
    bytes.extend_from_slice(b"\nendstream\nendobj\n");
    bytes
}

fn fixture(
    content: &[u8],
    resources: &str,
    options: &OpenOptions,
) -> ReadResult<(Arc<SparseBytes>, RenderContext)> {
    let source = Arc::new(SparseBytes::new(
        ID,
        8192,
        &options.limits,
        &SourceLimits::default(),
    )?);
    let page = format!(
        "3 0 obj << /Type /Page /Parent 2 0 R /Resources {resources} /Contents 4 0 R >> endobj\n"
    );
    let content = stream(4, "", content);
    for (offset, bytes) in [
        (0, b"%PDF-1.7\n".as_slice()),
        (100, b"1 0 obj << /Type /Catalog /Pages 2 0 R /OCProperties << >> >> endobj\n".as_slice()),
        (600, b"2 0 obj << /Type /Pages /Count 1 /Kids [3 0 R] /MediaBox [0 0 8 8] >> endobj\n".as_slice()),
        (900, page.as_bytes()),
        (1500, content.as_slice()),
        (6500, b"xref\n0 7\n0 65535 f\n100 0 n\n600 0 n\n900 0 n\n1500 0 n\n4000 0 n\n6000 0 n\ntrailer << /Size 7 /Root 1 0 R >>\n".as_slice()),
    ] {
        source.supply(ID, offset, bytes)?;
    }
    let mut tail = vec![b' '; 1024];
    tail[932..953].copy_from_slice(b"startxref\n6500\n%%EOF\n");
    source.supply(ID, 7168, &tail)?;
    let mut job = OpenJob::new(
        source.clone(),
        options,
        LoadingLimits::default(),
        Validation::RequestedOperation,
    )?;
    for _ in 0..16 {
        match job.advance()? {
            Opening::Yielded => {}
            Opening::Ready(document) => return Ok((source, RenderContext::new(document))),
            _ => return Err(Error::invalid_argument("unexpected opening state").into()),
        }
    }
    Err(Error::limit("test opening step bound exceeded").into())
}

fn interpretation_reports() -> [Diagnostic; 3] {
    [
        Diagnostic::new(
            pagura_interpret::repairs::OPTIONAL_CONFIG,
            "OCProperties has no default configuration",
        )
        .in_object(ObjectId::new(1, 0)),
        Diagnostic::new(
            pagura_interpret::repairs::INDEXED_EXCESS,
            "Indexed lookup has wrong length",
        ),
        Diagnostic::new(
            pagura_interpret::repairs::EMPTY_PATH,
            "painting requires a current path",
        )
        .at(Position::Stream {
            object: ObjectId::new(4, 0),
            offset: 8,
        })
        .in_object(ObjectId::new(4, 0)),
    ]
}

#[test]
fn interpreter_reports_wait_for_the_attempt_and_publish_on_success_or_terminal_failure() {
    for form in [
        stream(5, "/Type /XObject /Subtype /Form /BBox [0 0 8 8]", b""),
        b"5 0 obj null endobj\n".to_vec(),
    ] {
        let (source, context) =
            fixture(b"/Pal cs f /Fm Do\n", PALETTE, &OpenOptions::default()).unwrap();
        let document = context.document();
        let page = document.page(0).unwrap();
        for _ in 0..3 {
            let Err(ReadFault::Unavailable(missing)) = context.scene(page) else {
                panic!("form wait")
            };
            assert_eq!(missing.ranges(), &[ByteRange::new(4000, 4256).unwrap()]);
            assert_eq!(document.diagnostics(), []);
        }
        source.supply(ID, 4000, &form).unwrap();
        if form == b"5 0 obj null endobj\n" {
            for _ in 0..3 {
                let error = context.scene(page).unwrap_err();
                assert_eq!(error.as_error().unwrap().kind, ErrorKind::Syntax);
                assert_eq!(
                    error.as_error().unwrap().message,
                    "XObject has no dictionary"
                );
                assert_eq!(document.diagnostics(), interpretation_reports());
            }
        } else {
            let scene = context.scene(page).unwrap();
            for _ in 0..3 {
                assert!(Arc::ptr_eq(&scene, &context.scene(page).unwrap()));
                assert_eq!(document.diagnostics(), interpretation_reports());
            }
            source.close();
            assert_eq!(
                context.scene(page).unwrap_err(),
                source.check().unwrap_err().into()
            );
            assert_eq!(document.diagnostics(), interpretation_reports());
        }
    }
}

#[test]
fn strict_interpreter_refuses_a_known_repair_before_an_unrelated_form_wait() {
    let options = OpenOptions {
        recovery: Recovery::Strict,
        ..OpenOptions::default()
    };
    let (source, context) = fixture(b"/Pal cs f /Fm Do\n", PALETTE, &options).unwrap();
    let page = context.document().page(0).unwrap();
    let error = context.scene(page).unwrap_err();
    assert_eq!(
        error.as_error().unwrap().repair,
        Some(pagura_interpret::repairs::OPTIONAL_CONFIG)
    );
    assert_eq!(context.document().diagnostics(), []);
    assert!(matches!(
        source.read_exact(ByteRange::new(4000, 4001).unwrap()),
        Err(ReadFault::Unavailable(_))
    ));
}

#[test]
fn font_cmap_reports_are_provisional_while_completed_font_reports_survive() {
    let (source, context) = fixture(b"", TYPE3, &OpenOptions::default()).unwrap();
    let document = context.document();
    let page = document.page(0).unwrap();
    let fonts = document
        .resolve_dictionary(page.resources().get(b"Font").unwrap())
        .unwrap();
    let dictionary = document
        .resolve_dictionary(fonts.get(b"F").unwrap())
        .unwrap();
    let font = pagura_text::PdfFont::load(document, dictionary).unwrap();
    let completed = Diagnostic::new(
        pagura_text::repairs::ENCODING_NAME,
        "unknown simple-font encoding name; using the font's default encoding (Table 112)",
    );
    assert_eq!(document.diagnostics(), std::slice::from_ref(&completed));
    source.supply(ID, 4000, &stream(5, "/UseCMap 6 0 R", b"1 begincodespacerange <00> <FF> endcodespacerange\n2 beginbfchar <41> <0044> <42> <D800> endbfchar\n")).unwrap();
    let cmap_object = document.object(ObjectId::new(5, 0)).unwrap();
    for _ in 0..3 {
        let Err(ReadFault::Unavailable(missing)) = font.unicode(document, dictionary, None) else {
            panic!("parent CMap wait")
        };
        assert_eq!(missing.ranges(), &[ByteRange::new(6000, 6256).unwrap()]);
        assert_eq!(document.diagnostics(), std::slice::from_ref(&completed));
        assert!(std::ptr::eq(
            cmap_object,
            document.object(ObjectId::new(5, 0)).unwrap()
        ));
    }
    source
        .supply(
            ID,
            6000,
            &stream(
                6,
                "",
                b"1 begincodespacerange <00> <FF> endcodespacerange\n",
            ),
        )
        .unwrap();
    let repaired = Diagnostic::new(
        pagura_text::repairs::TOUNICODE_UTF16,
        "invalid UTF-16BE destination omitted (9.10.3)",
    )
    .at(Position::Stream {
        object: ObjectId::new(5, 0),
        offset: 87,
    })
    .in_object(ObjectId::new(5, 0));
    for _ in 0..3 {
        let unicode = font.unicode(document, dictionary, None).unwrap();
        let glyphs = font.decode(b"A", Some(&unicode)).unwrap();
        assert_eq!(glyphs.len(), 1);
        assert_eq!(glyphs[0].unicode.as_ref().unwrap().as_str(), "D");
        assert_eq!(
            document.diagnostics(),
            [completed.clone(), repaired.clone()]
        );
    }
}
