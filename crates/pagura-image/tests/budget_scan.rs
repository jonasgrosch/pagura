#![forbid(unsafe_code)]
//! The smallest decode budgets of a document's images (ADR 0016): for each
//! image XObject, the least `decoded_bytes` and the least `decode_work` at
//! which it decodes, each with every other limit at its default.
//!
//! Success need not be monotone in a budget, so a bisection's answer is
//! confirmed by scanning: every budget in a window below it must fail, and
//! every budget in a window from it up must succeed. Set
//! `PAGURA_BUDGET_SCAN` to the PDF.

use pagura_core::{Limits, Recovery};
use pagura_document::{Document, OpenOptions};
use pagura_image::{DecodeRequest, ImageSource};

/// Budgets scanned on each side of the smallest one found.
const SCAN: usize = 64;

/// The least budget in `1..=most` at which `decodes` holds, by bisection,
/// confirmed by a scan of `SCAN` budgets below it (all failing) and `SCAN`
/// from it upward in steps of a thousandth (all succeeding).
fn smallest(most: usize, decodes: impl Fn(usize) -> bool) -> usize {
    assert!(decodes(most), "the image decodes within the default budget");
    let (mut low, mut high) = (0, most);
    while high - low > 1 {
        let middle = low + (high - low) / 2;
        if decodes(middle) {
            high = middle;
        } else {
            low = middle;
        }
    }
    let step = (high / 1000).max(1);
    for below in (1..=SCAN).filter_map(|k| high.checked_sub(k)) {
        assert!(
            !decodes(below),
            "decodes at {below}, below its smallest {high}"
        );
    }
    for above in (0..SCAN).map(|k| high + k * step) {
        assert!(
            decodes(above),
            "fails at {above}, above its smallest {high}"
        );
    }
    high
}

#[test]
#[ignore = "real documents: set PAGURA_BUDGET_SCAN to a PDF"]
fn images_decode_from_their_smallest_budgets_up() {
    let Some(path) = std::env::var_os("PAGURA_BUDGET_SCAN") else {
        eprintln!("skipped: set PAGURA_BUDGET_SCAN to a PDF");
        return;
    };
    let document = Document::open(std::fs::read(path).unwrap(), &OpenOptions::default()).unwrap();
    let defaults = Limits::default();
    for id in document.object_ids() {
        let Some(source) = document
            .object(id)
            .ok()
            .and_then(|object| ImageSource::from_xobject(&document, object).ok())
        else {
            continue;
        };
        let decodes = |limits: &Limits| {
            source
                .decode(&DecodeRequest::FULL, limits, Recovery::Repair)
                .is_ok()
        };
        if !decodes(&defaults) {
            continue;
        }
        let bytes = smallest(defaults.decoded_bytes, |decoded_bytes| {
            decodes(&Limits {
                decoded_bytes,
                ..defaults
            })
        });
        let work = smallest(defaults.decode_work, |decode_work| {
            decodes(&Limits {
                decode_work,
                ..defaults
            })
        });
        println!("image {id:?}: smallest decoded_bytes {bytes}, decode_work {work}");
    }
}
