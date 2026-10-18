//! Test-only author for large signed revisions, independent of the PDF writer.
//! Expectations are the authored form-fill/signing operations (12.8.2.2).
use std::{collections::BTreeMap, fmt::Write};

const SLOT: &str = "@@@@@@@@@@ @@@@@@@@@@ @@@@@@@@@@ @@@@@@@@@@";
const TOKEN: &str = "<300304010000000000000000000000000000000000>";

#[derive(Default)]
struct Pdf {
    data: String,
    previous: Option<usize>,
    size: u32,
}
impl Pdf {
    fn append(&mut self, objects: &BTreeMap<u32, String>) -> BTreeMap<u32, usize> {
        let mut offsets = BTreeMap::new();
        let mut positions = BTreeMap::new();
        for (&id, body) in objects {
            offsets.insert(id, self.data.len());
            writeln!(self.data, "{id} 0 obj").unwrap();
            positions.insert(id, self.data.len());
            writeln!(self.data, "{body}\nendobj").unwrap();
            self.size = self.size.max(id + 1);
        }
        let at = self.data.len();
        writeln!(self.data, "xref").unwrap();
        if self.previous.is_none() {
            writeln!(self.data, "0 {}\n0000000000 65535 f ", self.size).unwrap();
            for id in 1..self.size {
                if let Some(offset) = offsets.get(&id) {
                    writeln!(self.data, "{offset:010} 00000 n ").unwrap();
                } else {
                    writeln!(self.data, "0000000000 00000 f ").unwrap();
                }
            }
        } else {
            for (id, offset) in offsets {
                writeln!(self.data, "{id} 1\n{offset:010} 00000 n ").unwrap();
            }
        }
        write!(self.data, "trailer\n<< /Size {} /Root 1 0 R", self.size).unwrap();
        if let Some(previous) = self.previous {
            write!(self.data, " /Prev {previous}").unwrap();
        }
        writeln!(self.data, " >>\nstartxref\n{at}\n%%EOF").unwrap();
        self.previous = Some(at);
        positions
    }
    fn sign(&mut self, at: usize, body: &str) {
        let start = at + body.find(TOKEN).unwrap();
        let end = start + TOKEN.len();
        let slot = at + body.find(SLOT).unwrap();
        let values = format!("{:10} {start:10} {end:10} {:10}", 0, self.data.len() - end);
        assert_eq!(values.len(), SLOT.len());
        self.data.replace_range(slot..slot + SLOT.len(), &values);
    }
}
fn signature(certification: bool) -> String {
    let policy = if certification {
        "/Reference [<< /TransformMethod /DocMDP /DigestMethod /SHA256 /TransformParams << /P 2 /V /1.2 >> >>]"
    } else {
        ""
    };
    format!(
        "<< /Type /Sig /Filter /Adobe.PPKLite /SubFilter /adbe.pkcs7.detached /ByteRange [{SLOT}] {policy} /Contents {TOKEN} >>"
    )
}

pub(super) fn large(content_objects: u32, approval_signatures: u32) -> Vec<u8> {
    let mut objects = BTreeMap::from([
        (
            1,
            "<< /Type /Catalog /Pages 2 0 R /AcroForm 4 0 R /Perms << /DocMDP 5 0 R >> >>".into(),
        ),
        (2, "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into()),
        (4, "<< /Fields [6 0 R 7 0 R 8 0 R] /SigFlags 3 >>".into()),
        (5, signature(true)),
        (6, "<< /FT /Sig /T (Approval) /V 5 0 R >>".into()),
        (7, "<< /FT /Tx /T (Amount) /V (old) >>".into()),
        (8, "<< /FT /Tx /T (Recipient) /V (old) >>".into()),
    ]);
    let mut page = "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 10 10] /Contents [".to_string();
    for id in 100..100 + content_objects {
        write!(page, "{id} 0 R ").unwrap();
        objects.insert(id, "<< /Length 5 >>\nstream\n0 0 m\nendstream".into());
    }
    page.push_str("] >>");
    objects.insert(3, page);
    let mut pdf = Pdf {
        data: "%PDF-2.0\n".into(),
        ..Pdf::default()
    };
    let positions = pdf.append(&objects);
    pdf.sign(positions[&5], &objects[&5]);
    if approval_signatures == 0 {
        pdf.append(&BTreeMap::from([(
            7,
            "<< /FT /Tx /T (Amount) /V (new) >>".into(),
        )]));
    }
    let mut fields = "6 0 R 7 0 R 8 0 R".to_string();
    for index in 0..approval_signatures {
        let id = 90 + 2 * index;
        let field = id + 1;
        write!(fields, " {field} 0 R").unwrap();
        let body = signature(false);
        let positions = pdf.append(&BTreeMap::from([
            (4, format!("<< /Fields [{fields}] /SigFlags 3 >>")),
            (id, body.clone()),
            (field, format!("<< /FT /Sig /T (S{index}) /V {id} 0 R >>")),
        ]));
        pdf.sign(positions[&id], &body);
    }
    pdf.data.into_bytes()
}
