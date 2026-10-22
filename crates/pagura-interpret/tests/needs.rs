//! The interpreter does no work a device does not need (ADR 0034): text
//! extraction reads no image, shading or colour space, and a scene builds
//! no extraction records.

use pagura_core::Result;
use pagura_document::{Document, OpenOptions};
use pagura_interpret::{
    Device, FillMark, GlyphRun, ImageMark, Interpreter, Needs, SceneRecorder, ShadingMark,
};

/// A one-page PDF of `objects` (numbered from 1; object 1 the catalog,
/// 2 the page tree, 3 the page) with a correct cross-reference table.
fn pdf(objects: &[String]) -> Vec<u8> {
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (index, object) in objects.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{object}\nendobj\n", index + 1).as_bytes());
    }
    let xref = out.len();
    out.extend_from_slice(
        format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes(),
    );
    for offset in offsets {
        out.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            objects.len() + 1
        )
        .as_bytes(),
    );
    out
}

fn stream(dictionary: &str, data: &str) -> String {
    format!(
        "<< {dictionary} /Length {} >>\nstream\n{data}\nendstream",
        data.len()
    )
}

/// A page showing "A" in a Type 3 font, then a broken image and a broken
/// shading.
fn broken_paint() -> Vec<u8> {
    let content = "BT /F1 12 Tf 10 10 Td (A) Tj ET /Im1 Do /Sh1 sh";
    pdf(&[
        "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_owned(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] /Contents 4 0 R \
         /Resources << /Font << /F1 5 0 R >> /XObject << /Im1 7 0 R >> \
         /Shading << /Sh1 << /ShadingType 9 /ColorSpace /DeviceRGB >> >> >> >>"
            .to_owned(),
        stream("", content),
        "<< /Type /Font /Subtype /Type3 /FontBBox [0 0 1000 1000] \
         /FontMatrix [0.001 0 0 0.001 0 0] /CharProcs << /A 6 0 R >> \
         /Encoding << /Type /Encoding /Differences [65 /A] >> \
         /FirstChar 65 /LastChar 65 /Widths [1000] /Resources << >> >>"
            .to_owned(),
        stream("", "1000 0 0 0 1000 1000 d1 0 0 1000 1000 re f"),
        stream(
            "/Type /XObject /Subtype /Image /Width 2 /Height 2 /ColorSpace /DeviceGray \
             /BitsPerComponent 8 /Filter /DCTDecode",
            "xyz",
        ),
    ])
}

#[test]
fn text_extraction_succeeds_where_images_and_shadings_are_broken() {
    let bytes = broken_paint();
    let document = Document::open(bytes, &OpenOptions::default()).unwrap();
    let page = document.page(0).unwrap();
    let text = Interpreter::new(&document).text(page).unwrap();
    assert_eq!(text.glyphs.len(), 1);
    assert_eq!(text.glyphs[0].unicode.as_deref(), Some("A"));
    assert!(Interpreter::new(&document).scene(page).is_err());
}

/// Counts what a device receives, with chosen needs.
struct Counter {
    needs: Needs,
    painted: usize,
    unpainted: usize,
    images: usize,
    shadings: usize,
    text: usize,
    glyphs: usize,
}

impl Counter {
    fn new(needs: Needs) -> Self {
        Self {
            needs,
            painted: 0,
            unpainted: 0,
            images: 0,
            shadings: 0,
            text: 0,
            glyphs: 0,
        }
    }
}

impl Device for Counter {
    fn needs(&self) -> Needs {
        self.needs
    }

    fn fill(&mut self, mark: &FillMark<'_>) -> Result<()> {
        if mark.paint.is_some() {
            self.painted += 1;
        } else {
            self.unpainted += 1;
        }
        Ok(())
    }

    fn glyphs(&mut self, run: &GlyphRun<'_>) -> Result<()> {
        self.glyphs += run.glyphs.len();
        self.text += run.glyphs.iter().filter(|g| g.text.is_some()).count();
        Ok(())
    }

    fn image(&mut self, _: &ImageMark<'_>) -> Result<()> {
        self.images += 1;
        Ok(())
    }

    fn shading(&mut self, _: &ShadingMark<'_>) -> Result<()> {
        self.shadings += 1;
        Ok(())
    }
}

#[test]
fn devices_without_paint_receive_no_paint_images_or_shadings() {
    let document = Document::open(broken_paint(), &OpenOptions::default()).unwrap();
    let page = document.page(0).unwrap();
    let mut device = Counter::new(Needs::TEXT);
    Interpreter::new(&document).run(page, &mut device).unwrap();
    assert_eq!((device.images, device.shadings, device.painted), (0, 0, 0));
    assert_eq!((device.glyphs, device.text), (1, 1));
}

#[test]
fn a_scene_recording_builds_no_extraction_records() {
    let document = Document::open(
        pagura_testkit::fixture("paint", "text-mode-0.pdf"),
        &OpenOptions::default(),
    )
    .unwrap();
    let page = document.page(0).unwrap();
    let mut device = Counter::new(Needs::SCENE);
    Interpreter::new(&document).run(page, &mut device).unwrap();
    assert!(device.glyphs > 0);
    assert_eq!(device.text, 0);
    let mut recorder = SceneRecorder::new(document.limits());
    Interpreter::new(&document)
        .run(page, &mut recorder)
        .unwrap();
    assert!(recorder.finish().unwrap().marks() > 0);
}
