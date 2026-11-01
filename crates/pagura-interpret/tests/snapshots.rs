#![forbid(unsafe_code)]
//! Scene dumps of representative fixtures: rectangles, a tiling cell, a
//! soft mask inside a text object, glyphs filled and stroked, Type 3
//! glyphs, a generated annotation appearance and a transparency group.
//! Set `PAGURA_BLESS=1` to rewrite the expected dumps after an intended
//! change.

use pagura_document::{Document, OpenOptions};
use pagura_interpret::{Interpreter, dump};

const CASES: &[(&str, &str)] = &[
    ("classic", "rectangle"),
    ("content", "tiling-anchored"),
    ("paint", "text-soft-mask-inside-object"),
    ("paint", "text-mode-2"),
    ("text", "extract-type3-matrix"),
    ("content", "annotation-icon-Comment"),
    ("content", "group-i0-k0"),
];

#[test]
fn scene_dumps_match_their_snapshots() {
    let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/snapshots");
    let bless = std::env::var_os("PAGURA_BLESS").is_some();
    for (area, name) in CASES {
        let bytes = pagura_testkit::fixture(area, &format!("{name}.pdf"));
        let document = Document::open(bytes, &OpenOptions::default()).unwrap();
        let page = document.page(0).unwrap();
        let scene = Interpreter::new(&document)
            .scene(page)
            .unwrap_or_else(|e| panic!("{area}/{name}: {e}"));
        let text = dump(&scene);
        let path = directory.join(format!("{area}-{name}.txt"));
        if bless {
            std::fs::write(&path, &text).unwrap();
            continue;
        }
        let expected = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("{}: {e}; set PAGURA_BLESS=1", path.display()));
        assert_eq!(text, expected, "{area}/{name}");
    }
}
